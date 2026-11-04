//! Consumer-owned CFD input cases. Imported results are deliberately separate.

use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use vernon_graph::{CookContext, CookResult};

use crate::types::MeshEvidence;

/// Mesh builds omit old meshes; case setup preserves an optional mesh; solver
/// inputs require all five mesh tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::enum_variant_names)]
pub(crate) enum CaseStageKind {
    MeshInputs,
    CaseSetupInputs,
    SolverInputs,
}

/// Which upstream directory supplied the imported initial-field snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InitialFieldSource {
    ZeroOrig,
    CurrentZero,
    Missing,
}

#[derive(Debug)]
pub(crate) struct StagedCase {
    pub(crate) path: PathBuf,
    pub(crate) reused: bool,
    pub(crate) initial_fields: InitialFieldSource,
}

#[derive(Clone, Copy)]
struct StageLimits {
    entries: usize,
    depth: usize,
    bytes: u64,
    retained: usize,
}

impl Default for StageLimits {
    fn default() -> Self {
        Self {
            entries: 32_768,
            depth: 32,
            bytes: 16 * 1024 * 1024 * 1024,
            retained: 32,
        }
    }
}

const MARKER: &str = ".vernon-stage.json";
const MARKER_VERSION: u32 = 2;
const MARKER_BYTES: u64 = 1024 * 1024;
const COPY_CHUNK: usize = 64 * 1024;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StageMarker {
    version: u32,
    owner: PathBuf,
    source: Option<PathBuf>,
    kind: CaseStageKind,
    recipe_key: String,
    input_fingerprint: String,
    initial_fields: InitialFieldSource,
}

#[derive(Debug, PartialEq, Eq)]
struct InputEntry {
    source: PathBuf,
    target: PathBuf,
    directory: bool,
    bytes: u64,
    modified: SystemTime,
}

#[derive(Debug, PartialEq, Eq)]
struct InputSnapshot {
    entries: Vec<InputEntry>,
    initial_fields: InitialFieldSource,
}

fn cancelled(ctx: &CookContext) -> CookResult<()> {
    if vernon_core::process::cancel_requested() {
        Err(ctx.fail("case staging cancelled"))
    } else {
        Ok(())
    }
}

fn link(meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

fn ordinary_metadata(ctx: &CookContext, path: &Path) -> CookResult<Metadata> {
    cancelled(ctx)?;
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| ctx.fail(format!("cannot inspect {}: {e}", path.display())))?;
    if link(&meta) || (!meta.is_dir() && !meta.is_file()) {
        return Err(ctx.fail(format!(
            "case staging refuses links, reparse points and special files: {}",
            path.display()
        )));
    }
    Ok(meta)
}

fn absolute(ctx: &CookContext, path: &Path) -> CookResult<PathBuf> {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(ctx.fail(format!(
            "case staging refuses parent-directory components: {}",
            path.display()
        )));
    }
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        let cwd = std::env::current_dir()
            .map_err(|e| ctx.fail(format!("cannot resolve case path: {e}")))?;
        let result = cwd.join(path);
        if !result.is_absolute() {
            return Err(ctx.fail(format!("case path is not absolute: {}", path.display())));
        }
        Ok(result)
    }
}

/// Check ancestors before canonicalizing: canonicalize alone would follow a
/// junction in an otherwise ordinary-looking source or work directory.
fn checked_path(ctx: &CookContext, path: &Path, directory: bool) -> CookResult<PathBuf> {
    let path = absolute(ctx, path)?;
    for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let meta = ordinary_metadata(ctx, ancestor)?;
        if (ancestor != path.as_path() || directory) && !meta.is_dir() {
            return Err(ctx.fail(format!(
                "case directory is not an ordinary directory: {}",
                ancestor.display()
            )));
        }
        if ancestor == path.as_path() && !directory && !meta.is_file() {
            return Err(ctx.fail(format!(
                "case input is not an ordinary file: {}",
                path.display()
            )));
        }
    }
    std::fs::canonicalize(&path)
        .map_err(|e| ctx.fail(format!("cannot resolve {}: {e}", path.display())))
}

/// Graph contexts can name a work directory that has never been created.
/// Resolve a checked existing base first, so overlap is rejected before mkdir.
fn prepare_work(ctx: &CookContext, source: Option<&Path>) -> CookResult<PathBuf> {
    let mut base = absolute(ctx, &ctx.work_dir)?;
    let mut missing = Vec::new();
    let base = loop {
        cancelled(ctx)?;
        match std::fs::symlink_metadata(&base) {
            Ok(_) => break checked_path(ctx, &base, true)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if missing.len() == 32 {
                    return Err(
                        ctx.fail("case work directory exceeds its missing-parent depth budget")
                    );
                }
                missing.push(
                    base.file_name()
                        .ok_or_else(|| {
                            ctx.fail("case work directory has no existing filesystem root")
                        })?
                        .to_owned(),
                );
                if !base.pop() {
                    return Err(ctx.fail("case work directory has no existing parent"));
                }
            }
            Err(e) => {
                return Err(ctx.fail(format!(
                    "cannot inspect case work directory {}: {e}",
                    base.display()
                )))
            }
        }
    };
    let mut proposed = base.clone();
    for name in missing.iter().rev() {
        proposed.push(name);
    }
    if source.is_some_and(|source| overlaps(source, &proposed)) {
        return Err(ctx.fail("case staging source and consumer work directory must not overlap"));
    }
    let mut work = base;
    for name in missing.into_iter().rev() {
        cancelled(ctx)?;
        checked_path(ctx, &work, true)?;
        work.push(name);
        match std::fs::create_dir(&work) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                return Err(ctx.fail(format!(
                    "cannot create case work directory {}: {e}",
                    work.display()
                )))
            }
        }
        if !ordinary_metadata(ctx, &work)?.is_dir() {
            return Err(ctx.fail(format!(
                "case work path is not a directory: {}",
                work.display()
            )));
        }
    }
    checked_path(ctx, &work, true)
}

fn overlaps(a: &Path, b: &Path) -> bool {
    fn prefix(a: &Path, b: &Path) -> bool {
        let mut b = b.components();
        a.components().all(|a| {
            b.next().is_some_and(|b| {
                #[cfg(windows)]
                {
                    a.as_os_str().to_string_lossy().to_lowercase()
                        == b.as_os_str().to_string_lossy().to_lowercase()
                }
                #[cfg(not(windows))]
                {
                    a == b
                }
            })
        })
    }
    prefix(a, b) || prefix(b, a)
}

fn relative(ctx: &CookContext, path: &Path) -> CookResult<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        Err(ctx.fail(format!("unsafe relative case path: {}", path.display())))
    } else {
        Ok(())
    }
}

/// The entry budget is consumed while enumerating, before allocating a full
/// listing. Unselected root output folders are not traversed.
fn children(ctx: &CookContext, path: &Path, remaining: &mut usize) -> CookResult<Vec<PathBuf>> {
    cancelled(ctx)?;
    let rd = std::fs::read_dir(path)
        .map_err(|e| ctx.fail(format!("cannot list {}: {e}", path.display())))?;
    let mut names = Vec::new();
    for entry in rd {
        cancelled(ctx)?;
        if *remaining == 0 {
            return Err(ctx.fail("case staging exceeds its directory entry budget"));
        }
        *remaining -= 1;
        let entry = entry.map_err(|e| ctx.fail(format!("cannot list {}: {e}", path.display())))?;
        ordinary_metadata(ctx, &entry.path())?;
        names.push(PathBuf::from(entry.file_name()));
    }
    names.sort();
    Ok(names)
}

fn excluded(kind: CaseStageKind, path: &Path) -> bool {
    kind == CaseStageKind::MeshInputs
        && (path.starts_with("constant/polyMesh")
            || path == Path::new(MESH_EVIDENCE)
            || path.starts_with("constant/extendedFeatureEdgeMesh")
            || (path.starts_with("constant/triSurface")
                && path
                    .extension()
                    .is_some_and(|e| e == "eMesh" || e == "extendedFeatureEdgeMesh")))
}

struct Scanner<'a> {
    ctx: &'a CookContext,
    root: &'a Path,
    kind: CaseStageKind,
    limits: StageLimits,
    remaining: usize,
    bytes: u64,
    entries: Vec<InputEntry>,
}

impl Scanner<'_> {
    fn collect(&mut self, source: PathBuf, target: PathBuf) -> CookResult<()> {
        relative(self.ctx, &source)?;
        relative(self.ctx, &target)?;
        let path = self.root.join(&source);
        let meta = ordinary_metadata(self.ctx, &path)?;
        if excluded(self.kind, &source) {
            return Ok(());
        }
        if target.components().count() > self.limits.depth {
            return Err(self
                .ctx
                .fail("case staging exceeds its directory depth budget"));
        }
        // Reserve one entry for the ownership marker written after the copy.
        if self.entries.len().saturating_add(1) >= self.limits.entries {
            return Err(self
                .ctx
                .fail("case staging exceeds its output entry budget"));
        }
        if meta.is_file() {
            self.bytes = self
                .bytes
                .checked_add(meta.len())
                .filter(|n| *n <= self.limits.bytes)
                .ok_or_else(|| self.ctx.fail("case staging exceeds its copied byte budget"))?;
        }
        self.entries.push(InputEntry {
            source: source.clone(),
            target: target.clone(),
            directory: meta.is_dir(),
            bytes: meta.len(),
            modified: meta.modified().map_err(|e| {
                self.ctx
                    .fail(format!("cannot read timestamp of {}: {e}", path.display()))
            })?,
        });
        if meta.is_dir() {
            for name in children(self.ctx, &path, &mut self.remaining)? {
                self.collect(source.join(&name), target.join(name))?;
            }
        }
        Ok(())
    }
}

fn snapshot(
    ctx: &CookContext,
    source: &Path,
    kind: CaseStageKind,
    limits: StageLimits,
) -> CookResult<InputSnapshot> {
    let mut scan = Scanner {
        ctx,
        root: source,
        kind,
        limits,
        remaining: limits.entries,
        bytes: 0,
        entries: Vec::new(),
    };
    let names = children(ctx, source, &mut scan.remaining)?;
    for name in ["system", "constant", "vernon_case.json"] {
        if names.iter().any(|p| p == Path::new(name)) {
            let meta = ordinary_metadata(ctx, &source.join(name))?;
            if meta.is_dir() != (name != "vernon_case.json") {
                return Err(ctx.fail(format!("case input has the wrong file kind: {name}")));
            }
            scan.collect(name.into(), name.into())?;
        }
    }
    let (initial_fields, initial) = if names.iter().any(|p| p == Path::new("0.orig")) {
        (InitialFieldSource::ZeroOrig, Some("0.orig"))
    } else if names.iter().any(|p| p == Path::new("0")) {
        (InitialFieldSource::CurrentZero, Some("0"))
    } else {
        (InitialFieldSource::Missing, None)
    };
    if let Some(initial) = initial {
        if !ordinary_metadata(ctx, &source.join(initial))?.is_dir() {
            return Err(ctx.fail(format!("initial fields are not a directory: {initial}")));
        }
        scan.collect(initial.into(), "0".into())?;
        scan.collect(initial.into(), "0.orig".into())?;
    }
    if kind == CaseStageKind::SolverInputs {
        for name in ["points", "faces", "owner", "neighbour", "boundary"] {
            let target = Path::new("constant/polyMesh").join(name);
            if !scan
                .entries
                .iter()
                .any(|e| e.target == target && !e.directory && e.bytes > 0)
            {
                return Err(ctx.fail(format!(
                    "solver inputs require a nonempty ordinary mesh file: {}",
                    target.display()
                )));
            }
        }
    }
    scan.entries.sort_by(|a, b| a.target.cmp(&b.target));
    Ok(InputSnapshot {
        entries: scan.entries,
        initial_fields,
    })
}

// Content fingerprints are deterministic across chunk boundaries and exclude
// timestamps: merely reading or touching the source must not rewrite the clone.
struct Fingerprint(u64);

impl Fingerprint {
    fn feed(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn entry(&mut self, entry: &InputEntry) {
        let name = entry.target.as_os_str().as_encoded_bytes();
        self.entry_named(entry, name);
    }

    fn entry_named(&mut self, entry: &InputEntry, name: &[u8]) {
        self.feed(&[u8::from(entry.directory)]);
        self.feed(&(name.len() as u64).to_le_bytes());
        self.feed(name);
        if !entry.directory {
            self.feed(&entry.bytes.to_le_bytes());
        }
    }
}

fn unchanged(ctx: &CookContext, entry: &InputEntry, meta: &Metadata) -> CookResult<()> {
    if link(meta)
        || meta.is_dir() != entry.directory
        || meta.len() != entry.bytes
        || meta.modified().ok() != Some(entry.modified)
    {
        Err(ctx.fail(format!(
            "case input changed while staging: {}",
            entry.source.display()
        )))
    } else {
        Ok(())
    }
}

/// Hash selected contents, optionally copying those exact bytes into a newly
/// created staging tree. Every read/copy is bounded by the inventoried length.
fn transfer(
    ctx: &CookContext,
    source: &Path,
    inputs: &InputSnapshot,
    destination: Option<&Path>,
) -> CookResult<String> {
    transfer_framed(ctx, source, inputs, destination, false)
}

fn transfer_framed(
    ctx: &CookContext,
    source: &Path,
    inputs: &InputSnapshot,
    destination: Option<&Path>,
    portable_ascii_paths: bool,
) -> CookResult<String> {
    let mut fingerprint = Fingerprint(0xcbf2_9ce4_8422_2325);
    let mut buffer = vec![0; COPY_CHUNK];
    for entry in &inputs.entries {
        cancelled(ctx)?;
        if portable_ascii_paths {
            let mut name = Vec::new();
            for component in entry.target.components() {
                let Component::Normal(component) = component else {
                    return Err(ctx.fail("portable surface identifiers require ordinary relative paths"));
                };
                let component = component.to_str().filter(|s| s.is_ascii())
                    .ok_or_else(|| ctx.fail("portable surface identifiers require ASCII path components"))?;
                if !name.is_empty() {
                    name.push(b'/');
                }
                name.extend_from_slice(component.as_bytes());
            }
            fingerprint.entry_named(entry, &name);
        } else {
            // Existing case and mesh fingerprint framing remains native.
            fingerprint.entry(entry);
        }
        let input = source.join(&entry.source);
        checked_path(ctx, &input, entry.directory)?;
        unchanged(ctx, entry, &ordinary_metadata(ctx, &input)?)?;
        let target = destination.map(|d| d.join(&entry.target));
        if let Some(target) = &target {
            checked_path(
                ctx,
                target
                    .parent()
                    .ok_or_else(|| ctx.fail("case target has no parent"))?,
                true,
            )?;
        }
        if entry.directory {
            if let Some(target) = target {
                std::fs::create_dir(&target).map_err(|e| {
                    ctx.fail(format!("cannot stage directory {}: {e}", target.display()))
                })?;
            }
            continue;
        }
        let mut reader = File::open(&input)
            .map_err(|e| ctx.fail(format!("cannot read {}: {e}", input.display())))?;
        unchanged(
            ctx,
            entry,
            &reader.metadata().map_err(|e| {
                ctx.fail(format!(
                    "cannot inspect open input {}: {e}",
                    input.display()
                ))
            })?,
        )?;
        let mut writer = target
            .as_ref()
            .map(|target| {
                OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(target)
                    .map_err(|e| ctx.fail(format!("cannot stage file {}: {e}", target.display())))
            })
            .transpose()?;
        let mut read = 0_u64;
        loop {
            cancelled(ctx)?;
            let count = buffer.len().min(
                (entry.bytes - read)
                    .saturating_add(1)
                    .min(COPY_CHUNK as u64) as usize,
            );
            let n = reader
                .read(&mut buffer[..count])
                .map_err(|e| ctx.fail(format!("cannot read {}: {e}", input.display())))?;
            if n == 0 {
                break;
            }
            read += n as u64;
            if read > entry.bytes {
                return Err(ctx.fail(format!(
                    "case input grew while staging: {}",
                    input.display()
                )));
            }
            fingerprint.feed(&buffer[..n]);
            if let Some(writer) = &mut writer {
                writer
                    .write_all(&buffer[..n])
                    .map_err(|e| ctx.fail(format!("cannot copy {}: {e}", input.display())))?;
            }
        }
        if read != entry.bytes {
            return Err(ctx.fail(format!(
                "case input shrank while staging: {}",
                input.display()
            )));
        }
        unchanged(
            ctx,
            entry,
            &reader.metadata().map_err(|e| {
                ctx.fail(format!(
                    "cannot inspect open input {}: {e}",
                    input.display()
                ))
            })?,
        )?;
        checked_path(ctx, &input, false)?;
        unchanged(ctx, entry, &ordinary_metadata(ctx, &input)?)?;
    }
    Ok(format!("{:016x}", fingerprint.0))
}

fn validate_tree(ctx: &CookContext, root: &Path, limits: StageLimits) -> CookResult<()> {
    checked_path(ctx, root, true)?;
    let mut remaining = limits.entries;
    let mut pending = vec![(root.to_path_buf(), 0_usize)];
    while let Some((path, depth)) = pending.pop() {
        for name in children(ctx, &path, &mut remaining)? {
            let child = path.join(name);
            let meta = ordinary_metadata(ctx, &child)?;
            if depth + 1 > limits.depth {
                return Err(ctx.fail("existing case exceeds its directory depth budget"));
            }
            if meta.is_dir() {
                pending.push((child, depth + 1));
            }
        }
    }
    Ok(())
}

fn read_marker(ctx: &CookContext, case: &Path, owner: &Path) -> CookResult<StageMarker> {
    let path = case.join(MARKER);
    checked_path(ctx, &path, false)?;
    let mut bytes = Vec::new();
    File::open(&path)
        .map_err(|e| {
            ctx.fail(format!(
                "cannot open case ownership marker {}: {e}",
                path.display()
            ))
        })?
        .take(MARKER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| {
            ctx.fail(format!(
                "cannot read case ownership marker {}: {e}",
                path.display()
            ))
        })?;
    if bytes.len() as u64 > MARKER_BYTES {
        return Err(ctx.fail("case ownership marker exceeds its byte budget"));
    }
    let decoded: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        ctx.fail(format!(
            "case ownership marker is invalid at {}: {e}",
            path.display()
        ))
    })?;
    let version = decoded.get("version").and_then(serde_json::Value::as_u64);
    if version != Some(u64::from(MARKER_VERSION)) {
        let version = version
            .map(|v| v.to_string())
            .unwrap_or_else(|| "missing or invalid".into());
        return Err(ctx.fail(format!("unsupported case ownership marker version {version}; expected {MARKER_VERSION}; existing files preserved at {}", case.display())));
    }
    let marker: StageMarker = serde_json::from_value(decoded).map_err(|e| {
        ctx.fail(format!(
            "case ownership marker is invalid at {}: {e}",
            path.display()
        ))
    })?;
    if marker.version != MARKER_VERSION
        || marker.owner.as_path() != owner
        || (marker.source.is_none()
            && (marker.kind != CaseStageKind::CaseSetupInputs
                || marker.initial_fields != InitialFieldSource::Missing))
        || marker.recipe_key.is_empty()
        || marker.input_fingerprint.len() != 16
        || !marker
            .input_fingerprint
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err(ctx.fail(format!(
            "case ownership marker does not authorize this destination: {}",
            case.display()
        )));
    }
    Ok(marker)
}

fn absent(ctx: &CookContext, path: &Path) -> CookResult<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(ctx.fail(format!(
            "cannot inspect reserved case path {}: {e}",
            path.display()
        ))),
    }
}

fn unused(ctx: &CookContext, work: &Path, prefix: &str, count: usize) -> CookResult<PathBuf> {
    for i in 0..count {
        cancelled(ctx)?;
        let path = work.join(format!("{prefix}{i}"));
        if absent(ctx, &path)? {
            return Ok(path);
        }
    }
    Err(ctx.fail(format!("case staging has no free {prefix} slot within its {count}-generation budget; preserved generations require explicit cleanup")))
}

/// Read-only fingerprint of complete selected case inputs, including optional
/// mesh tables and physics dictionaries. Prefer `0.orig` to solver-mutated `0`.
/// The canonical source is scanned; its absolute spelling is not hashed.
/// This does not create a work directory or read/write ownership markers.
pub(crate) fn case_input_fingerprint(ctx: &CookContext, source: &Path) -> CookResult<String> {
    let source = checked_path(ctx, source, true)?;
    let limits = StageLimits::default();
    let inputs = snapshot(ctx, &source, CaseStageKind::CaseSetupInputs, limits)?;
    let fingerprint = transfer(ctx, &source, &inputs, None)?;
    if snapshot(ctx, &source, CaseStageKind::CaseSetupInputs, limits)? != inputs {
        return Err(ctx.fail(
            "case inputs changed while fingerprinting; no complete fingerprint was certified",
        ));
    }
    cancelled(ctx)?;
    Ok(fingerprint)
}

/// Read-only triSurface-fnv1a64-v1 identifier of the three prepared half-model
/// surface files, with canonical ASCII forward-slash paths on every platform.
pub(crate) fn tri_surface_fingerprint(ctx: &CookContext, case: &Path) -> CookResult<String> {
    tri_surface_fingerprint_with_limits(ctx, case, StageLimits::default())
}

#[derive(Debug, PartialEq, Eq)]
struct TriSurfaceInventory {
    sources: InputSnapshot,
    derived: Vec<InputEntry>,
}

fn tri_surface_inventory(
    ctx: &CookContext,
    case: &Path,
    limits: StageLimits,
) -> CookResult<TriSurfaceInventory> {
    let relative = Path::new("constant").join("triSurface");
    let directory = checked_path(ctx, &case.join(&relative), true)?;
    let mut remaining = limits.entries;
    let names = children(ctx, &directory, &mut remaining)?;
    let source_names = ["body.stl", "wheel_fr.stl", "wheel_rr.stl"];
    let derived_names = ["body.eMesh", "wheel_fr.eMesh", "wheel_rr.eMesh"];
    if source_names.iter().any(|name| !names.iter().any(|found| found == Path::new(name))) {
        return Err(ctx.fail("prepared half-model surfaces require body.stl, wheel_fr.stl and wheel_rr.stl"));
    }
    let mut scan = Scanner {
        ctx,
        root: case,
        kind: CaseStageKind::CaseSetupInputs,
        // children already enforces the complete entry budget. Scanner's extra
        // reserved slot is for a stage marker; this read-only helper writes none.
        limits: StageLimits { entries: limits.entries.saturating_add(1), ..limits },
        remaining,
        bytes: 0,
        entries: Vec::new(),
    };
    for name in names {
        let source = source_names.iter().any(|expected| name.as_path() == Path::new(expected));
        let derived = derived_names.iter().any(|expected| name.as_path() == Path::new(expected));
        if !source && !derived {
            return Err(ctx.fail(format!("unknown prepared-surface entry: {}", name.display())));
        }
        let path = relative.join(&name);
        let meta = ordinary_metadata(ctx, &case.join(&path))?;
        if !meta.is_file() || (source && meta.len() == 0) {
            return Err(ctx.fail(format!("prepared-surface entry must be an ordinary {}file: {}", if source { "nonempty " } else { "" }, name.display())));
        }
        let before = scan.entries.len();
        scan.collect(path.clone(), path)?;
        if scan.entries.len() != before + 1
            || scan.entries[before].directory
            || (source && scan.entries[before].bytes == 0)
        {
            return Err(ctx.fail("prepared-surface entry changed its file kind during inventory"));
        }
    }
    let (sources, derived) = scan.entries.into_iter()
        .partition(|entry| entry.source.extension().is_some_and(|extension| extension == "stl"));
    Ok(TriSurfaceInventory {
        sources: InputSnapshot { entries: sources, initial_fields: InitialFieldSource::Missing },
        derived,
    })
}

fn tri_surface_fingerprint_with_limits(
    ctx: &CookContext,
    case: &Path,
    limits: StageLimits,
) -> CookResult<String> {
    let case = checked_path(ctx, case, true)?;
    let inventory = tri_surface_inventory(ctx, &case, limits)?;
    let fingerprint = transfer_framed(ctx, &case, &inventory.sources, None, true)?;
    if tri_surface_inventory(ctx, &case, limits)? != inventory {
        return Err(ctx.fail("prepared surface inventory changed while fingerprinting; no identifier was certified"));
    }
    cancelled(ctx)?;
    Ok(fingerprint)
}

const MESH_EVIDENCE: &str = "constant/.vernon-mesh-evidence.json";
const MESH_EVIDENCE_BYTES: u64 = 4096;
const MESH_EVIDENCE_VERSION: u32 = 1;

/// Stable lexical order and only the five base tables. Solver-created zones,
/// dictionaries and the evidence record itself do not change this fingerprint.
fn mesh_snapshot(
    ctx: &CookContext,
    case: &Path,
    limits: StageLimits,
) -> CookResult<InputSnapshot> {
    let mut scan = Scanner {
        ctx,
        root: case,
        kind: CaseStageKind::SolverInputs,
        limits,
        remaining: limits.entries,
        bytes: 0,
        entries: Vec::new(),
    };
    for name in ["boundary", "faces", "neighbour", "owner", "points"] {
        // Match the native component joins used by the ordinary case inventory:
        // path spelling participates in the existing content-hash framing.
        let path = Path::new("constant").join("polyMesh").join(name);
        checked_path(ctx, &case.join(&path), false)?;
        let before = scan.entries.len();
        scan.collect(path.clone(), path)?;
        if scan.entries.len() != before + 1
            || scan.entries[before].directory
            || scan.entries[before].bytes == 0
        {
            return Err(ctx.fail(format!("mesh evidence requires a nonempty ordinary {name} table")));
        }
    }
    Ok(InputSnapshot {
        entries: scan.entries,
        initial_fields: InitialFieldSource::Missing,
    })
}

fn mesh_content(
    ctx: &CookContext,
    case: &Path,
    limits: StageLimits,
) -> CookResult<(InputSnapshot, String)> {
    let inputs = mesh_snapshot(ctx, case, limits)?;
    let fingerprint = transfer(ctx, case, &inputs, None)?;
    if mesh_snapshot(ctx, case, limits)? != inputs {
        return Err(ctx.fail("mesh tables changed while fingerprinting; no mesh evidence was certified"));
    }
    cancelled(ctx)?;
    Ok((inputs, fingerprint))
}

fn validate_mesh_evidence(ctx: &CookContext, evidence: &MeshEvidence) -> CookResult<()> {
    if evidence.version != MESH_EVIDENCE_VERSION {
        return Err(ctx.fail(format!("unsupported mesh evidence version {}; expected {MESH_EVIDENCE_VERSION}", evidence.version)));
    }
    if evidence.cells == 0 {
        return Err(ctx.fail("mesh evidence cells must be a positive integer"));
    }
    if !(0.0..70.0).contains(&evidence.max_nonortho) {
        return Err(ctx.fail("mesh evidence non-orthogonality must be finite, nonnegative and below 70 degrees"));
    }
    if !(0.0..4.0).contains(&evidence.max_skew) {
        return Err(ctx.fail("mesh evidence skewness must be finite, nonnegative and below 4"));
    }
    if evidence.mesh_fingerprint.len() != 16
        || !evidence.mesh_fingerprint.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(ctx.fail("mesh evidence has an invalid mesh fingerprint"));
    }
    Ok(())
}

fn mesh_record_entry(ctx: &CookContext, case: &Path) -> CookResult<InputEntry> {
    let path = case.join(MESH_EVIDENCE);
    checked_path(ctx, &path, false)?;
    let meta = ordinary_metadata(ctx, &path)?;
    if !meta.is_file() || meta.len() > MESH_EVIDENCE_BYTES {
        return Err(ctx.fail("mesh evidence record is not an ordinary file within its byte budget"));
    }
    Ok(InputEntry {
        source: MESH_EVIDENCE.into(),
        target: MESH_EVIDENCE.into(),
        directory: false,
        bytes: meta.len(),
        modified: meta.modified().map_err(|e| ctx.fail(format!("cannot inspect mesh evidence timestamp: {e}")))?,
    })
}

/// Record actual successful mesh checks in a consumer-owned case.
pub(crate) fn write_mesh_evidence(
    ctx: &CookContext,
    case: &Path,
    cells: u64,
    max_nonortho: f64,
    max_skew: f64,
) -> CookResult<MeshEvidence> {
    write_mesh_evidence_with_limits(
        ctx,
        case,
        cells,
        max_nonortho,
        max_skew,
        StageLimits::default(),
    )
}

fn write_mesh_evidence_with_limits(
    ctx: &CookContext,
    case: &Path,
    cells: u64,
    max_nonortho: f64,
    max_skew: f64,
    limits: StageLimits,
) -> CookResult<MeshEvidence> {
    let mut evidence = MeshEvidence {
        version: MESH_EVIDENCE_VERSION,
        cells,
        mesh_fingerprint: "0000000000000000".into(),
        max_nonortho,
        max_skew,
    };
    validate_mesh_evidence(ctx, &evidence)?;
    let work = checked_path(ctx, &ctx.work_dir, true)?;
    let case = checked_path(ctx, case, true)?;
    if !case.starts_with(&work) {
        return Err(ctx.fail("mesh evidence may be written only under the consumer work directory"));
    }
    let path = case.join(MESH_EVIDENCE);
    let constant = checked_path(ctx, path.parent().unwrap(), true)?;
    let previous = if absent(ctx, &path)? {
        None
    } else {
        Some(mesh_record_entry(ctx, &case)?)
    };
    let (inputs, fingerprint) = mesh_content(ctx, &case, limits)?;
    evidence.mesh_fingerprint = fingerprint;
    let serialized = serde_json::to_vec_pretty(&evidence)
        .map_err(|e| ctx.fail(format!("cannot serialize mesh evidence: {e}")))?;
    if serialized.len() as u64 > MESH_EVIDENCE_BYTES {
        return Err(ctx.fail("mesh evidence record exceeds its byte budget"));
    }
    // Publish one complete record after all checks. A canceled/failed write
    // cannot truncate a prior record into a partially readable certificate.
    let temporary = unused(ctx, &constant, ".vernon-mesh-evidence.pending-", limits.retained)?;
    let mut writer = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| ctx.fail(format!("cannot create temporary mesh evidence: {e}")))?;
    let publish = (|| -> CookResult<()> {
        writer.write_all(&serialized)
            .and_then(|()| writer.flush())
            .map_err(|e| ctx.fail(format!("cannot write mesh evidence: {e}")))?;
        drop(writer);
        if mesh_snapshot(ctx, &case, limits)? != inputs {
            return Err(ctx.fail("mesh tables changed before evidence publication"));
        }
        checked_path(ctx, &constant, true)?;
        checked_path(ctx, &temporary, false)?;
        match &previous {
            Some(previous) => {
                checked_path(ctx, &path, false)?;
                unchanged(ctx, previous, &ordinary_metadata(ctx, &path)?)?;
            }
            None if !absent(ctx, &path)? => return Err(ctx.fail("mesh evidence destination appeared before publication")),
            None => {}
        }
        cancelled(ctx)?;
        std::fs::rename(&temporary, &path)
            .map_err(|e| ctx.fail(format!("cannot publish mesh evidence: {e}")))
    })();
    if publish.is_err() {
        // This is only the single file successfully created_new above.
        let _ = std::fs::remove_file(&temporary);
    }
    publish?;
    Ok(evidence)
}

/// Read a bounded record and verify it against this case's actual mesh tables.
pub(crate) fn read_mesh_evidence(ctx: &CookContext, case: &Path) -> CookResult<MeshEvidence> {
    read_mesh_evidence_with_limits(ctx, case, StageLimits::default())
}

fn read_mesh_evidence_with_limits(
    ctx: &CookContext,
    case: &Path,
    limits: StageLimits,
) -> CookResult<MeshEvidence> {
    let case = checked_path(ctx, case, true)?;
    let entry = mesh_record_entry(ctx, &case)?;
    let path = case.join(MESH_EVIDENCE);
    let mut reader = File::open(&path)
        .map_err(|e| ctx.fail(format!("cannot open mesh evidence: {e}")))?;
    unchanged(ctx, &entry, &reader.metadata().map_err(|e| ctx.fail(format!("cannot inspect open mesh evidence: {e}")))?)?;
    let mut bytes = Vec::new();
    (&mut reader).take(MESH_EVIDENCE_BYTES + 1).read_to_end(&mut bytes)
        .map_err(|e| ctx.fail(format!("cannot read mesh evidence: {e}")))?;
    cancelled(ctx)?;
    if bytes.len() as u64 != entry.bytes {
        return Err(ctx.fail("mesh evidence changed or exceeded its byte budget while reading"));
    }
    unchanged(ctx, &entry, &reader.metadata().map_err(|e| ctx.fail(format!("cannot inspect open mesh evidence: {e}")))?)?;
    let evidence: MeshEvidence = serde_json::from_slice(&bytes)
        .map_err(|e| ctx.fail(format!("invalid mesh evidence record: {e}")))?;
    validate_mesh_evidence(ctx, &evidence)?;
    let (_, fingerprint) = mesh_content(ctx, &case, limits)?;
    if evidence.mesh_fingerprint != fingerprint {
        return Err(ctx.fail("mesh evidence does not match the current five base mesh tables"));
    }
    checked_path(ctx, &path, false)?;
    unchanged(ctx, &entry, &ordinary_metadata(ctx, &path)?)?;
    cancelled(ctx)?;
    Ok(evidence)
}

/// Clone immutable inputs into `ctx.work_dir/case`, never into the upstream.
/// `recipe_key` must include the caller's template version and every override
/// affecting its generated files. A requested external run must pass `fresh`.
/// Only ordinary selected input files are copied; links/reparse points are
/// rejected. The source must remain quiescent while staging. This is not an
/// adversarial, operating-system-level filesystem snapshot.
pub(crate) fn stage_case(
    ctx: &CookContext,
    upstream: &Path,
    kind: CaseStageKind,
    recipe_key: &str,
    fresh: bool,
) -> CookResult<StagedCase> {
    stage_case_with_limits(
        ctx,
        upstream,
        kind,
        recipe_key,
        fresh,
        StageLimits::default(),
    )
}

/// Case setup may initialize an owned empty template without an upstream.
/// Other staging kinds still require an explicit source case.
pub(crate) fn stage_case_optional(
    ctx: &CookContext,
    upstream: Option<&Path>,
    kind: CaseStageKind,
    recipe_key: &str,
    fresh: bool,
) -> CookResult<StagedCase> {
    stage_case_optional_with_limits(
        ctx,
        upstream,
        kind,
        recipe_key,
        fresh,
        StageLimits::default(),
    )
}

fn stage_case_with_limits(
    ctx: &CookContext,
    upstream: &Path,
    kind: CaseStageKind,
    recipe_key: &str,
    fresh: bool,
    limits: StageLimits,
) -> CookResult<StagedCase> {
    stage_case_optional_with_limits(ctx, Some(upstream), kind, recipe_key, fresh, limits)
}

fn stage_case_optional_with_limits(
    ctx: &CookContext,
    upstream: Option<&Path>,
    kind: CaseStageKind,
    recipe_key: &str,
    fresh: bool,
    limits: StageLimits,
) -> CookResult<StagedCase> {
    cancelled(ctx)?;
    if upstream.is_none() && kind != CaseStageKind::CaseSetupInputs {
        return Err(ctx.fail(format!("case staging {kind:?} requires an upstream source")));
    }
    if recipe_key.is_empty() {
        return Err(ctx.fail("case staging requires a nonempty versioned recipe key"));
    }
    let source = upstream
        .map(|source| checked_path(ctx, source, true))
        .transpose()?;
    let work = prepare_work(ctx, source.as_deref())?;
    if source
        .as_deref()
        .is_some_and(|source| overlaps(source, &work))
    {
        return Err(ctx.fail("case staging source and consumer work directory must not overlap"));
    }
    let case = ctx.work_dir.join("case");
    let current = if absent(ctx, &case)? {
        None
    } else {
        validate_tree(ctx, &case, limits)?;
        Some(read_marker(ctx, &case, &work)?)
    };
    let (inputs, fingerprint) = if let Some(source) = &source {
        let inputs = snapshot(ctx, source, kind, limits)?;
        let fingerprint = transfer(ctx, source, &inputs, None)?;
        if snapshot(ctx, source, kind, limits)? != inputs {
            return Err(
                ctx.fail("case inputs changed while fingerprinting; no clone was published")
            );
        }
        (inputs, fingerprint)
    } else {
        // The real empty inventory has the same FNV initial value as transfer
        // with no entries. No source directory is read, created or substituted.
        (
            InputSnapshot {
                entries: Vec::new(),
                initial_fields: InitialFieldSource::Missing,
            },
            "cbf29ce484222325".into(),
        )
    };
    let marker = StageMarker {
        version: MARKER_VERSION,
        owner: work.clone(),
        source: source.clone(),
        kind,
        recipe_key: recipe_key.into(),
        input_fingerprint: fingerprint,
        initial_fields: inputs.initial_fields,
    };
    if !fresh && current.as_ref() == Some(&marker) {
        cancelled(ctx)?;
        return Ok(StagedCase {
            path: case,
            reused: true,
            initial_fields: inputs.initial_fields,
        });
    }
    // Preflight retention before writing anything. Never delete a prior case,
    // even if its extra user files are not known to this input-cloning helper.
    let previous = current
        .as_ref()
        .map(|_| unused(ctx, &work, "case.previous-", limits.retained))
        .transpose()?;
    let serialized = serde_json::to_vec_pretty(&marker)
        .map_err(|e| ctx.fail(format!("cannot serialize case ownership marker: {e}")))?;
    if serialized.len() as u64 > MARKER_BYTES {
        return Err(ctx.fail("case ownership marker exceeds its byte budget"));
    }
    let incoming = unused(ctx, &work, ".case.staging-", limits.retained)?;
    std::fs::create_dir(&incoming).map_err(|e| {
        ctx.fail(format!(
            "cannot create case staging directory {}: {e}",
            incoming.display()
        ))
    })?;
    let prepare = || -> CookResult<()> {
        if let Some(source) = &source {
            let copied = transfer(ctx, source, &inputs, Some(&incoming))?;
            if copied != marker.input_fingerprint || snapshot(ctx, source, kind, limits)? != inputs
            {
                return Err(ctx.fail("case inputs changed while copying; no clone was published"));
            }
        }
        let path = incoming.join(MARKER);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut file| file.write_all(&serialized))
            .map_err(|e| {
                ctx.fail(format!(
                    "cannot write case ownership marker {}: {e}",
                    path.display()
                ))
            })?;
        cancelled(ctx)?;
        checked_path(ctx, &ctx.work_dir, true)?;
        validate_tree(ctx, &incoming, limits)?;
        match &current {
            Some(expected) => {
                validate_tree(ctx, &case, limits)?;
                if read_marker(ctx, &case, &work)? != *expected {
                    return Err(ctx.fail("active case ownership changed during staging"));
                }
            }
            None if !absent(ctx, &case)? => {
                return Err(ctx.fail("case destination appeared during staging"))
            }
            None => {}
        }
        cancelled(ctx)?;
        if let Some(previous) = &previous {
            if !absent(ctx, previous)? {
                return Err(ctx.fail("reserved previous-case path appeared during staging"));
            }
            std::fs::rename(&case, previous).map_err(|e| {
                ctx.fail(format!(
                    "cannot retain previous case at {}: {e}",
                    previous.display()
                ))
            })?;
        }
        let publish = cancelled(ctx).and_then(|()| {
            if !absent(ctx, &case)? {
                return Err(ctx.fail("case destination appeared before promotion"));
            }
            std::fs::rename(&incoming, &case)
                .map_err(|e| ctx.fail(format!("cannot promote staged case: {e}")))
        });
        if let Err(error) = publish {
            let rollback = if let Some(previous) = &previous {
                match absent(ctx, &case) {
                    Ok(true) => match std::fs::rename(previous, &case) {
                        Ok(()) => "previous case restored".to_string(),
                        Err(e) => format!(
                            "rollback failed: {e}; previous case preserved at {}",
                            previous.display()
                        ),
                    },
                    _ => format!(
                        "destination is occupied or unreadable; previous case preserved at {}",
                        previous.display()
                    ),
                }
            } else {
                "no previous case was moved".into()
            };
            return Err(ctx.fail(format!("cannot publish staged case: {error}; {rollback}")));
        }
        Ok(())
    };
    prepare().map_err(|e| {
        ctx.fail(format!(
            "{e}; staging files preserved at {}",
            incoming.display()
        ))
    })?;
    Ok(StagedCase {
        path: case,
        reused: false,
        initial_fields: inputs.initial_fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::{ctx, tmp};
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    const RECIPE: &str = "case-template-v1:baseline";

    fn put(root: &Path, name: &str, bytes: &[u8]) {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn input_case(name: &str) -> PathBuf {
        let source = tmp(name);
        put(&source, "system/controlDict", b"control baseline\n");
        put(&source, "system/blockMeshDict", b"block baseline\n");
        put(&source, "constant/transportProperties", b"nu 1.5e-5;\n");
        put(
            &source,
            "constant/triSurface/body.stl",
            b"solid body\nendsolid body\n",
        );
        put(&source, "0/U", b"solver-mutated U\n");
        put(&source, "0.orig/U", b"immutable U\n");
        put(
            &source,
            "vernon_case.json",
            b"{\"flavour\":\"opencfd2606\"}\n",
        );
        source
    }

    fn mesh(root: &Path) {
        for name in ["points", "faces", "owner", "neighbour", "boundary"] {
            put(
                root,
                &format!("constant/polyMesh/{name}"),
                format!("FoamFile {{ object {name}; }}\n0()\n").as_bytes(),
            );
        }
    }

    const TRI_SURFACE_GOLDEN: &str = "67577c1e3392b746";

    fn tri_surface_case(name: &str) -> (CookContext, PathBuf) {
        let context = ctx(name);
        let case = context.work_dir.join("case");
        for name in ["body", "wheel_fr", "wheel_rr"] {
            put(&case, &format!("constant/triSurface/{name}.stl"), format!("solid {name}\nendsolid {name}\n").as_bytes());
        }
        (context, case)
    }

    #[test]
    fn wp9_tri_surface_fingerprint_has_independent_portable_framing_golden() {
        let (context, case) = tri_surface_case("tri-surface-golden");
        // Independently evaluated FNV-1a64 bytes, not an oracle from the helper:
        // per lexical file:0,u64LE(pathlen),ASCII constant/triSurface/name.stl,
        // u64LE(contentlen),exact UTF-8 bytes. Content lengths are25,33,33.
        assert_eq!(tri_surface_fingerprint(&context, &case).unwrap(), TRI_SURFACE_GOLDEN);
    }

    #[test]
    fn wp9_tri_surface_fingerprint_excludes_absolute_root_and_timestamps() {
        let (a_context, a) = tri_surface_case("tri-surface-root-a");
        let (b_context, b) = tri_surface_case("tri-surface-root-b");
        let path = b.join("constant/triSurface/body.stl");
        let bytes = std::fs::read(&path).unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(tri_surface_fingerprint(&a_context, &a).unwrap(), TRI_SURFACE_GOLDEN);
        assert_eq!(tri_surface_fingerprint(&b_context, &b).unwrap(), TRI_SURFACE_GOLDEN);
    }

    #[test]
    #[cfg(windows)]
    fn wp9_tri_surface_fingerprint_is_independent_of_windows_path_spelling() {
        let (context, case) = tri_surface_case("tri-surface-path-spelling");
        let forward = PathBuf::from(case.to_str().unwrap().replace('\\', "/"));
        assert_eq!(tri_surface_fingerprint(&context, &case).unwrap(), TRI_SURFACE_GOLDEN);
        assert_eq!(tri_surface_fingerprint(&context, &forward).unwrap(), TRI_SURFACE_GOLDEN);
    }

    #[test]
    fn wp9_tri_surface_fingerprint_is_read_only_without_a_created_work_directory() {
        let (_, case) = tri_surface_case("tri-surface-upstream");
        let mut consumer = ctx("tri-surface-read-consumer");
        consumer.work_dir = consumer.work_dir.join("not-created/work");
        let before = files(&case);
        assert!(!consumer.work_dir.exists());
        assert_eq!(tri_surface_fingerprint(&consumer, &case).unwrap(), TRI_SURFACE_GOLDEN);
        assert!(!consumer.work_dir.exists(), "a read-only identifier must not initialize an owned case");
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_tri_surface_fingerprint_detects_same_length_surface_changes() {
        let (context, case) = tri_surface_case("tri-surface-byte-change");
        let before = tri_surface_fingerprint(&context, &case).unwrap();
        let path = case.join("constant/triSurface/body.stl");
        let mut changed = std::fs::read(&path).unwrap();
        changed[0] ^= 1;
        std::fs::write(path, changed).unwrap();
        assert_ne!(tri_surface_fingerprint(&context, &case).unwrap(), before, "same-size content changes must change the identifier");
    }

    #[test]
    fn wp9_tri_surface_fingerprint_rejects_missing_case_without_creating_it() {
        let context = ctx("tri-surface-missing-case");
        let case = context.work_dir.join("not-created/case");
        assert!(tri_surface_fingerprint(&context, &case).is_err());
        assert!(!case.exists());
    }

    #[test]
    fn wp9_tri_surface_fingerprint_rejects_missing_or_empty_expected_files() {
        for name in ["body", "wheel_fr", "wheel_rr"] {
            for missing in [false, true] {
                let (context, case) = tri_surface_case("tri-surface-incomplete");
                let path = case.join(format!("constant/triSurface/{name}.stl"));
                if missing {
                    std::fs::remove_file(path).unwrap();
                } else {
                    std::fs::write(path, b"").unwrap();
                }
                let before = files(&case);
                assert!(tri_surface_fingerprint(&context, &case).is_err(), "cannot identify missing or empty {name}");
                assert_eq!(files(&case), before);
            }
        }
    }

    #[test]
    fn wp9_tri_surface_fingerprint_rejects_additional_surface_inputs() {
        for extra in ["extra", "wheel_fl", "wheel_rl"] {
            let (context, case) = tri_surface_case("tri-surface-extra");
            put(&case, &format!("constant/triSurface/{extra}.stl"), b"solid extra\nendsolid extra\n");
            let before = files(&case);
            assert!(tri_surface_fingerprint(&context, &case).is_err(), "the registered half-model input inventory cannot silently omit {extra}");
            assert_eq!(files(&case), before);
        }
    }

    #[test]
    fn wp9_tri_surface_fingerprint_ignores_only_known_ordinary_derived_companions() {
        let (context, case) = tri_surface_case("tri-surface-derived");
        for name in ["body", "wheel_fr", "wheel_rr"] {
            put(&case, &format!("constant/triSurface/{name}.eMesh"), b"generated feature edges\n");
        }
        assert_eq!(tri_surface_fingerprint(&context, &case).unwrap(), TRI_SURFACE_GOLDEN);
        put(&case, "constant/triSurface/body.eMesh", b"");
        std::fs::remove_file(case.join("constant/triSurface/wheel_fr.eMesh")).unwrap();
        let before = files(&case);
        assert_eq!(tri_surface_fingerprint(&context, &case).unwrap(), TRI_SURFACE_GOLDEN, "optional derived edges do not establish or alter source geometry identity");
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_tri_surface_fingerprint_rejects_unknown_companion_entries() {
        for name in ["notes.txt", "extra.eMesh", "body.obj"] {
            let (context, case) = tri_surface_case("tri-surface-unknown-entry");
            put(&case, &format!("constant/triSurface/{name}"), b"unknown companion\n");
            assert!(tri_surface_fingerprint(&context, &case).is_err(), "unknown {name} cannot be silently filtered from the prepared inventory");
        }
    }

    #[test]
    fn wp9_tri_surface_fingerprint_rejects_nested_surface_inputs() {
        let (context, case) = tri_surface_case("tri-surface-nested");
        put(&case, "constant/triSurface/nested/extra.stl", b"nested surface\n");
        let before = files(&case);
        assert!(tri_surface_fingerprint(&context, &case).is_err(), "the three-file inventory is direct, not a recursive surface search");
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_tri_surface_fingerprint_rejects_directory_instead_of_surface_file() {
        let (context, case) = tri_surface_case("tri-surface-wrong-kind");
        let path = case.join("constant/triSurface/body.stl");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(path).unwrap();
        assert!(tri_surface_fingerprint(&context, &case).is_err());
    }

    #[test]
    fn wp9_tri_surface_fingerprint_honors_cancellation_without_writes() {
        let (context, case) = tri_surface_case("tri-surface-cancel");
        let before = files(&case);
        let result = vernon_core::process::with_cancel(Arc::new(AtomicBool::new(true)), || tri_surface_fingerprint(&context, &case));
        assert!(result.is_err());
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_tri_surface_fingerprint_enforces_aggregate_byte_entry_and_depth_budgets() {
        let (context, case) = tri_surface_case("tri-surface-budgets");
        let before = files(&case);
        for limits in [
            StageLimits { bytes: 90, ..StageLimits::default() },
            StageLimits { entries: 2, ..StageLimits::default() },
            StageLimits { depth: 2, ..StageLimits::default() },
        ] {
            assert!(tri_surface_fingerprint_with_limits(&context, &case, limits).is_err());
            assert_eq!(files(&case), before);
        }
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn wp9_tri_surface_fingerprint_rejects_linked_surface_directory() {
        let (context, case) = tri_surface_case("tri-surface-link-case");
        let (_, outside) = tri_surface_case("tri-surface-link-outside");
        let before = files(&outside);
        let path = case.join("constant/triSurface");
        std::fs::rename(&path, case.join("constant/retained-original-surfaces")).unwrap();
        directory_link(&outside.join("constant/triSurface"), &path);
        assert!(tri_surface_fingerprint(&context, &case).is_err(), "a junction is not a prepared local surface inventory");
        assert_eq!(files(&outside), before);
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn wp9_tri_surface_fingerprint_rejects_linked_case_ancestor() {
        let (context, case) = tri_surface_case("tri-surface-ancestor-source");
        let alias_owner = ctx("tri-surface-ancestor-alias");
        let alias = alias_owner.work_dir.join("case-alias");
        directory_link(&case, &alias);
        let before = files(&case);
        assert!(tri_surface_fingerprint(&context, &alias).is_err());
        assert_eq!(files(&case), before);
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn wp9_tri_surface_fingerprint_rejects_derived_companion_junction() {
        let (context, case) = tri_surface_case("tri-surface-derived-link");
        let outside = tmp("tri-surface-derived-link-outside");
        put(&outside, "sentinel", b"untouched target\n");
        let before = files(&outside);
        directory_link(&outside, &case.join("constant/triSurface/body.eMesh"));
        assert!(tri_surface_fingerprint(&context, &case).is_err(), "ignored derived content still must not be a link or directory");
        assert_eq!(files(&outside), before);
    }

    fn evidence_case(name: &str) -> (CookContext, PathBuf) {
        let context = ctx(name);
        let case = context.work_dir.join("case");
        mesh(&case);
        (context, case)
    }

    /// Build a valid reader fixture using the existing bounded content hasher,
    /// independently of the evidence writer under test. Only five tables exist.
    fn evidence_fixture(context: &CookContext, case: &Path) -> MeshEvidence {
        let mut inputs = snapshot(context, case, CaseStageKind::CaseSetupInputs, StageLimits::default()).unwrap();
        inputs.entries.retain(|entry| !entry.directory && entry.source.starts_with("constant/polyMesh"));
        assert_eq!(inputs.entries.len(), 5);
        let evidence = MeshEvidence {
            version: 1,
            cells: 12_345_678,
            mesh_fingerprint: transfer(context, case, &inputs, None).unwrap(),
            max_nonortho: 62.5,
            max_skew: 2.25,
        };
        put(case, MESH_EVIDENCE, &serde_json::to_vec(&evidence).unwrap());
        evidence
    }

    #[test]
    fn wp9_mesh_evidence_write_publishes_bound_record_and_read_roundtrips() {
        let (context, case) = evidence_case("mesh-evidence-write");
        let evidence = write_mesh_evidence(&context, &case, 12_345_678, 62.5, 2.25).unwrap();
        assert!(case.join(MESH_EVIDENCE).is_file(), "successful evidence must be persisted in the copied constant inventory");
        assert_eq!(evidence.version, 1);
        assert_eq!(evidence.cells, 12_345_678);
        assert_eq!(evidence.mesh_fingerprint.len(), 16);
        assert!(evidence.mesh_fingerprint.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(read_mesh_evidence(&context, &case).unwrap(), evidence);
    }

    #[test]
    fn wp9_mesh_evidence_reads_a_real_record_without_modifying_inputs() {
        let (context, case) = evidence_case("mesh-evidence-read");
        let expected = evidence_fixture(&context, &case);
        let before = files(&case);
        assert_eq!(read_mesh_evidence(&context, &case).unwrap(), expected);
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_mesh_evidence_missing_record_does_not_adopt_legacy_mesh() {
        let (context, case) = evidence_case("mesh-evidence-legacy");
        put(&case, "logs/checkMesh.log", b"cells: 12000000\nMesh OK.\n");
        put(&case, "vernon_mesh.stamp", b"certified\n");
        let before = files(&case);
        assert!(read_mesh_evidence(&context, &case).is_err(), "legacy logs and literal stamps cannot substitute for bound evidence");
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_mesh_evidence_rejects_schema_and_field_errors() {
        let (context, case) = evidence_case("mesh-evidence-schema");
        let good = serde_json::to_value(evidence_fixture(&context, &case)).unwrap();
        for (name, bad) in [
            ("version", serde_json::json!(2)),
            ("cells", serde_json::json!(0)),
            ("cells", serde_json::json!(-1)),
            ("cells", serde_json::json!(12.5)),
            ("mesh_fingerprint", serde_json::json!("bad")),
            ("max_nonortho", serde_json::json!(70.0)),
            ("max_nonortho", serde_json::json!(-1.0)),
            ("max_skew", serde_json::json!(4.0)),
            ("max_skew", serde_json::Value::Null),
            ("unexpected", serde_json::json!(true)),
        ] {
            let mut record = good.clone();
            record[name] = bad;
            put(&case, MESH_EVIDENCE, &serde_json::to_vec(&record).unwrap());
            assert!(read_mesh_evidence(&context, &case).is_err(), "invalid evidence field {name} must fail closed");
        }
        let mut record = good;
        record.as_object_mut().unwrap().remove("cells");
        put(&case, MESH_EVIDENCE, &serde_json::to_vec(&record).unwrap());
        assert!(read_mesh_evidence(&context, &case).is_err());
        put(&case, MESH_EVIDENCE, b"not JSON");
        assert!(read_mesh_evidence(&context, &case).is_err());
    }

    #[test]
    fn wp9_mesh_evidence_rejects_oversized_record() {
        let (context, case) = evidence_case("mesh-evidence-large-record");
        let mut bytes = serde_json::to_vec(&evidence_fixture(&context, &case)).unwrap();
        bytes.resize(MESH_EVIDENCE_BYTES as usize + 1, b' ');
        put(&case, MESH_EVIDENCE, &bytes);
        assert!(read_mesh_evidence(&context, &case).is_err(), "even syntactically valid oversized records must not be read unboundedly");
    }

    #[test]
    fn wp9_mesh_evidence_rejects_changed_table_contents() {
        let (context, case) = evidence_case("mesh-evidence-tamper");
        evidence_fixture(&context, &case);
        let path = case.join("constant/polyMesh/points");
        let mut changed = std::fs::read(&path).unwrap();
        changed[0] ^= 1;
        std::fs::write(path, changed).unwrap();
        assert!(read_mesh_evidence(&context, &case).is_err(), "same-length mesh edits must invalidate earlier evidence");
    }

    #[test]
    fn wp9_mesh_evidence_rejects_missing_or_empty_base_tables() {
        for name in ["points", "faces", "owner", "neighbour", "boundary"] {
            for missing in [false, true] {
                let (context, case) = evidence_case("mesh-evidence-incomplete");
                evidence_fixture(&context, &case);
                let path = case.join("constant/polyMesh").join(name);
                if missing {
                    std::fs::remove_file(path).unwrap();
                } else {
                    std::fs::write(path, []).unwrap();
                }
                assert!(read_mesh_evidence(&context, &case).is_err(), "missing or empty {name} must invalidate evidence");
                assert!(write_mesh_evidence(&context, &case, 12_345_678, 62.5, 2.25).is_err(), "cannot certify missing or empty {name}");
            }
        }
    }

    #[test]
    fn wp9_mesh_evidence_rejects_failed_or_nonfinite_quality_before_publication() {
        for (cells, angle, skew) in [
            (0, 62.5, 2.25),
            (12_000_000, 70.0, 2.25),
            (12_000_000, -0.1, 2.25),
            (12_000_000, f64::NAN, 2.25),
            (12_000_000, 62.5, 4.0),
            (12_000_000, 62.5, -0.1),
            (12_000_000, 62.5, f64::INFINITY),
        ] {
            let (context, case) = evidence_case("mesh-evidence-bad-quality");
            let before = files(&case);
            assert!(write_mesh_evidence(&context, &case, cells, angle, skew).is_err(), "failed measurements must not produce evidence");
            assert_eq!(files(&case), before);
        }
    }

    #[test]
    fn wp9_mesh_evidence_survives_owned_clone_with_solver_generated_zones() {
        let (context, case) = evidence_case("mesh-evidence-clone-source");
        let expected = evidence_fixture(&context, &case);
        let consumer = ctx("mesh-evidence-clone-consumer");
        let staged = stage_case(&consumer, &case, CaseStageKind::SolverInputs, RECIPE, false).unwrap();
        assert!(staged.path.join(MESH_EVIDENCE).is_file());
        put(&staged.path, "constant/polyMesh/cellZones", b"new topoSet zone\n");
        put(&staged.path, "system/fvSolution", b"changed solver settings\n");
        put(&staged.path, "logs/solver.log", b"new run log\n");
        assert_eq!(read_mesh_evidence(&consumer, &staged.path).unwrap(), expected, "mesh evidence binds only the five base tables, not solver zones or dictionaries");
        let reused = stage_case(&consumer, &case, CaseStageKind::SolverInputs, RECIPE, false).unwrap();
        assert!(reused.reused);
        assert_eq!(read_mesh_evidence(&consumer, &reused.path).unwrap(), expected);
    }

    #[test]
    fn wp9_mesh_evidence_mesh_inputs_do_not_copy_old_certification() {
        let (context, case) = evidence_case("mesh-evidence-remesh-source");
        evidence_fixture(&context, &case);
        let consumer = ctx("mesh-evidence-remesh-consumer");
        let staged = stage_case(&consumer, &case, CaseStageKind::MeshInputs, RECIPE, false).unwrap();
        assert!(!staged.path.join(MESH_EVIDENCE).exists(), "a new mesh plan must not inherit an old mesh's certification");
    }

    #[test]
    fn wp9_mesh_evidence_reader_honors_cancel_without_rewriting_record() {
        let (context, case) = evidence_case("mesh-evidence-read-cancel");
        evidence_fixture(&context, &case);
        let before = files(&case);
        let result = vernon_core::process::with_cancel(Arc::new(AtomicBool::new(true)), || read_mesh_evidence(&context, &case));
        assert!(result.is_err());
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_mesh_evidence_writer_honors_cancel_without_publication() {
        let (context, case) = evidence_case("mesh-evidence-write-cancel");
        let before = files(&case);
        let result = vernon_core::process::with_cancel(Arc::new(AtomicBool::new(true)), || write_mesh_evidence(&context, &case, 12_000_000, 62.5, 2.25));
        assert!(result.is_err());
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_mesh_evidence_mesh_reads_and_writes_enforce_small_test_budgets() {
        let (context, case) = evidence_case("mesh-evidence-budgets");
        evidence_fixture(&context, &case);
        let before = files(&case);
        for limits in [
            StageLimits { bytes: 1, ..StageLimits::default() },
            StageLimits { entries: 4, ..StageLimits::default() },
            StageLimits { depth: 2, ..StageLimits::default() },
        ] {
            assert!(read_mesh_evidence_with_limits(&context, &case, limits).is_err());
            assert!(write_mesh_evidence_with_limits(&context, &case, 12_000_000, 62.5, 2.25, limits).is_err());
            assert_eq!(files(&case), before);
        }
    }

    #[test]
    fn wp9_mesh_evidence_writer_refuses_upstream_outside_its_work_directory() {
        let (source_context, case) = evidence_case("mesh-evidence-outside-source");
        evidence_fixture(&source_context, &case);
        let before = files(&case);
        let consumer = ctx("mesh-evidence-outside-consumer");
        assert!(write_mesh_evidence(&consumer, &case, 12_000_000, 62.5, 2.25).is_err(), "writes belong only to the consumer work directory");
        assert_eq!(files(&case), before);
        assert!(read_mesh_evidence(&consumer, &case).is_ok(), "the read-only accessor may inspect an upstream case");
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn wp9_mesh_evidence_rejects_mesh_junction_without_touching_target() {
        let (context, case) = evidence_case("mesh-evidence-link-case");
        let (outside_context, outside) = evidence_case("mesh-evidence-link-outside");
        evidence_fixture(&outside_context, &outside);
        let before = files(&outside);
        let old = case.join("constant/polyMesh");
        let retained = case.join("constant/old-polyMesh");
        std::fs::rename(&old, &retained).unwrap();
        directory_link(&outside.join("constant/polyMesh"), &old);
        put(&case, MESH_EVIDENCE, &std::fs::read(outside.join(MESH_EVIDENCE)).unwrap());
        assert!(read_mesh_evidence(&context, &case).is_err());
        assert!(write_mesh_evidence(&context, &case, 12_000_000, 62.5, 2.25).is_err());
        assert_eq!(files(&outside), before);
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn wp9_mesh_evidence_rejects_record_junction() {
        let (context, case) = evidence_case("mesh-evidence-record-link");
        let outside = tmp("mesh-evidence-record-link-outside");
        put(&outside, "sentinel", b"preserve\n");
        directory_link(&outside, &case.join(MESH_EVIDENCE));
        let before = files(&outside);
        assert!(read_mesh_evidence(&context, &case).is_err());
        assert!(write_mesh_evidence(&context, &case, 12_000_000, 62.5, 2.25).is_err());
        assert_eq!(files(&outside), before);
    }

    fn files(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, (Vec<u8>, SystemTime)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let meta = std::fs::symlink_metadata(&path).unwrap();
                assert!(!meta.file_type().is_symlink());
                if meta.is_dir() {
                    walk(root, &path, out);
                } else {
                    out.insert(
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        (std::fs::read(&path).unwrap(), meta.modified().unwrap()),
                    );
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, root, &mut out);
        out
    }

    fn uncreated_context(name: &str) -> CookContext {
        let mut consumer = ctx(name);
        consumer.work_dir = consumer.work_dir.join("fresh-parent/node");
        assert!(!consumer.work_dir.exists());
        consumer
    }

    #[test]
    fn wp9_stage_optional_initializes_empty_template_without_work_directory() {
        let consumer = uncreated_context("stage-optional-empty");
        let staged = stage_case_optional(
            &consumer,
            None,
            CaseStageKind::CaseSetupInputs,
            RECIPE,
            false,
        )
        .unwrap();
        assert_eq!(staged.path, consumer.work_dir.join("case"));
        assert!(!staged.reused);
        assert_eq!(staged.initial_fields, InitialFieldSource::Missing);
        assert!(staged.path.join(MARKER).is_file());
        assert_eq!(
            std::fs::read_dir(&staged.path).unwrap().count(),
            1,
            "empty initialization must not create or copy a dummy source case"
        );
    }

    #[test]
    fn wp9_stage_optional_unchanged_empty_template_reuses_generation() {
        let consumer = uncreated_context("stage-optional-reuse");
        let first = stage_case_optional(
            &consumer,
            None,
            CaseStageKind::CaseSetupInputs,
            RECIPE,
            false,
        )
        .unwrap();
        put(
            &first.path,
            "0.orig/U",
            b"writer-generated initial fields\n",
        );
        put(
            &first.path,
            "system/controlDict",
            b"writer-generated controls\n",
        );
        let before = files(&first.path);
        let modified = std::fs::metadata(&first.path).unwrap().modified().unwrap();
        let second = stage_case_optional(
            &consumer,
            None,
            CaseStageKind::CaseSetupInputs,
            RECIPE,
            false,
        )
        .unwrap();
        assert!(second.reused);
        assert_eq!(
            second.initial_fields,
            InitialFieldSource::Missing,
            "locally generated fields must not be relabelled as imported upstream fields"
        );
        assert_eq!(files(&second.path), before);
        assert_eq!(
            std::fs::metadata(&second.path).unwrap().modified().unwrap(),
            modified
        );
        assert!(!consumer.work_dir.join("case.previous-0").exists());
    }

    #[test]
    fn wp9_stage_optional_empty_template_refresh_retains_owned_generations() {
        let consumer = uncreated_context("stage-optional-refresh");
        let mut active = stage_case_optional(
            &consumer,
            None,
            CaseStageKind::CaseSetupInputs,
            RECIPE,
            false,
        )
        .unwrap();
        for (i, (key, fresh)) in [(RECIPE, true), ("case-template-v2:changed", false)]
            .into_iter()
            .enumerate()
        {
            let note = format!("preserve user note {i}\n");
            put(&active.path, "user.txt", note.as_bytes());
            put(
                &active.path,
                "constant/MRFProperties",
                b"previous writer-generated physics\n",
            );
            put(
                &active.path,
                "postProcessing/forceCoeffs/0/coefficient.dat",
                b"previous result\n",
            );
            active =
                stage_case_optional(&consumer, None, CaseStageKind::CaseSetupInputs, key, fresh)
                    .unwrap();
            assert!(!active.reused);
            assert_eq!(active.initial_fields, InitialFieldSource::Missing);
            for name in ["user.txt", "constant", "postProcessing"] {
                assert!(!active.path.join(name).exists());
            }
            assert_eq!(
                std::fs::read(
                    consumer
                        .work_dir
                        .join(format!("case.previous-{i}/user.txt"))
                )
                .unwrap(),
                note.as_bytes()
            );
        }
    }

    #[test]
    fn wp9_stage_optional_case_setup_without_mesh_creates_work_and_reuses() {
        let source = input_case("stage-optional-no-mesh-source");
        let before = files(&source);
        let consumer = uncreated_context("stage-optional-no-mesh-consumer");
        let first = stage_case_optional(
            &consumer,
            Some(&source),
            CaseStageKind::CaseSetupInputs,
            RECIPE,
            false,
        )
        .unwrap();
        assert_eq!(first.path, consumer.work_dir.join("case"));
        assert!(!first.path.join("constant/polyMesh").exists());
        assert!(first.path.join("constant/triSurface/body.stl").is_file());
        let copied = files(&first.path);
        let second = stage_case_optional(
            &consumer,
            Some(&source),
            CaseStageKind::CaseSetupInputs,
            RECIPE,
            false,
        )
        .unwrap();
        assert!(second.reused);
        assert_eq!(files(&second.path), copied);
        assert_eq!(files(&source), before);
    }

    #[test]
    fn wp9_stage_optional_case_setup_preserves_complete_mesh() {
        let source = input_case("stage-optional-mesh-source");
        mesh(&source);
        let before = files(&source);
        let consumer = uncreated_context("stage-optional-mesh-consumer");
        let staged = stage_case_optional(
            &consumer,
            Some(&source),
            CaseStageKind::CaseSetupInputs,
            RECIPE,
            false,
        )
        .unwrap();
        for name in ["points", "faces", "owner", "neighbour", "boundary"] {
            let relative = Path::new("constant/polyMesh").join(name);
            assert_eq!(
                std::fs::read(staged.path.join(&relative)).unwrap(),
                std::fs::read(source.join(relative)).unwrap()
            );
        }
        let before_reuse = files(&staged.path);
        assert!(
            stage_case_optional(
                &consumer,
                Some(&source),
                CaseStageKind::CaseSetupInputs,
                RECIPE,
                false
            )
            .unwrap()
            .reused
        );
        assert_eq!(files(&staged.path), before_reuse);
        assert_eq!(files(&source), before);
    }

    #[test]
    fn wp9_stage_optional_case_setup_preserves_partial_mesh_without_certifying_solver_inputs() {
        let source = input_case("stage-optional-partial-source");
        put(
            &source,
            "constant/polyMesh/boundary",
            b"partial mesh input\n",
        );
        let consumer = uncreated_context("stage-optional-partial-consumer");
        let staged = stage_case_optional(
            &consumer,
            Some(&source),
            CaseStageKind::CaseSetupInputs,
            RECIPE,
            false,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(staged.path.join("constant/polyMesh/boundary")).unwrap(),
            b"partial mesh input\n"
        );
        assert!(!staged.path.join("constant/polyMesh/points").exists());
        assert!(stage_case(
            &uncreated_context("stage-optional-partial-solver"),
            &staged.path,
            CaseStageKind::SolverInputs,
            RECIPE,
            false
        )
        .is_err());
    }

    #[test]
    fn wp9_stage_optional_initial_field_provenance_has_three_states() {
        for (i, expected) in [
            InitialFieldSource::ZeroOrig,
            InitialFieldSource::CurrentZero,
            InitialFieldSource::Missing,
        ]
        .into_iter()
        .enumerate()
        {
            let source = input_case(&format!("stage-optional-fields-source-{i}"));
            if expected != InitialFieldSource::ZeroOrig {
                std::fs::remove_file(source.join("0.orig/U")).unwrap();
                std::fs::remove_dir(source.join("0.orig")).unwrap();
            }
            if expected == InitialFieldSource::Missing {
                std::fs::remove_file(source.join("0/U")).unwrap();
                std::fs::remove_dir(source.join("0")).unwrap();
            }
            let consumer = uncreated_context(&format!("stage-optional-fields-consumer-{i}"));
            let staged = stage_case_optional(
                &consumer,
                Some(&source),
                CaseStageKind::CaseSetupInputs,
                RECIPE,
                false,
            )
            .unwrap();
            assert_eq!(staged.initial_fields, expected);
            if expected == InitialFieldSource::Missing {
                assert!(!staged.path.join("0").exists());
                assert!(!staged.path.join("0.orig").exists());
            }
        }
    }

    #[test]
    fn wp9_stage_optional_none_rejects_mesh_and_solver_profiles() {
        for (i, kind) in [CaseStageKind::MeshInputs, CaseStageKind::SolverInputs]
            .into_iter()
            .enumerate()
        {
            let consumer = uncreated_context(&format!("stage-optional-required-source-{i}"));
            let error = stage_case_optional(&consumer, None, kind, RECIPE, false)
                .unwrap_err()
                .to_string();
            assert!(error.contains("requires an upstream"), "{error}");
            assert!(
                !consumer.work_dir.exists(),
                "missing required input must fail before initializing an output case"
            );
        }
    }

    #[test]
    fn wp9_stage_optional_none_does_not_adopt_unmarked_case() {
        let consumer = ctx("stage-optional-unmarked");
        put(
            &consumer.work_dir,
            "case/user.txt",
            b"preserve unowned user data\n",
        );
        let before = files(&consumer.work_dir);
        assert!(stage_case_optional(
            &consumer,
            None,
            CaseStageKind::CaseSetupInputs,
            RECIPE,
            false
        )
        .is_err());
        assert_eq!(files(&consumer.work_dir), before);
    }

    #[test]
    fn wp9_stage_optional_v1_marker_is_refused_without_replacing_files() {
        let source = input_case("stage-optional-old-schema-source");
        let consumer = ctx("stage-optional-old-schema-consumer");
        let staged =
            stage_case(&consumer, &source, CaseStageKind::MeshInputs, RECIPE, false).unwrap();
        let path = staged.path.join(MARKER);
        let mut marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        marker["version"] = serde_json::json!(1);
        marker.as_object_mut().unwrap().remove("initial_fields");
        marker["used_0_orig"] = serde_json::json!(true);
        std::fs::write(&path, serde_json::to_vec_pretty(&marker).unwrap()).unwrap();
        put(&staged.path, "user.txt", b"old owned generation\n");
        let before = files(&consumer.work_dir);
        let error = stage_case_optional(
            &consumer,
            Some(&source),
            CaseStageKind::CaseSetupInputs,
            RECIPE,
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("version 1"),
            "old schema must have an explicit version diagnostic: {error}"
        );
        assert_eq!(files(&consumer.work_dir), before);
    }

    #[test]
    fn wp9_stage_clones_owned_inputs_without_mutating_upstream() {
        let source = input_case("stage-source");
        for name in [
            "10/U",
            "processor0/0/U",
            "postProcessing/forceCoeffs/0/coefficient.dat",
            "logs/solver.log",
            "log.simpleFoam",
            "vernon_results.stamp",
            "Allrun.solve",
            "notes.txt",
        ] {
            put(&source, name, b"old output or unrelated user file\n");
        }
        let before = files(&source);
        let source_mtime = std::fs::metadata(&source).unwrap().modified().unwrap();
        let consumer = ctx("stage-consumer");
        let staged =
            stage_case(&consumer, &source, CaseStageKind::MeshInputs, RECIPE, false).unwrap();
        assert_eq!(
            staged.path,
            consumer.work_dir.join("case"),
            "a consumer must not return its upstream case as writable output"
        );
        assert!(!staged.reused);
        assert_eq!(staged.initial_fields, InitialFieldSource::ZeroOrig);
        assert_eq!(
            std::fs::read(staged.path.join("0/U")).unwrap(),
            b"immutable U\n"
        );
        assert_eq!(
            std::fs::read(staged.path.join("0.orig/U")).unwrap(),
            b"immutable U\n"
        );
        assert_eq!(
            std::fs::read(staged.path.join("constant/transportProperties")).unwrap(),
            b"nu 1.5e-5;\n"
        );
        assert!(staged.path.join("vernon_case.json").is_file());
        for name in [
            "10",
            "processor0",
            "postProcessing",
            "logs",
            "log.simpleFoam",
            "vernon_results.stamp",
            "Allrun.solve",
            "notes.txt",
        ] {
            assert!(!staged.path.join(name).exists(), "must not clone {name}");
        }
        assert_eq!(files(&source), before);
        assert_eq!(
            std::fs::metadata(&source).unwrap().modified().unwrap(),
            source_mtime
        );
    }

    #[test]
    fn wp9_stage_creates_fresh_graph_work_directory() {
        let source = input_case("stage-new-work-source");
        let parent = ctx("stage-new-work-parent");
        let consumer = CookContext {
            node_name: "fresh graph node".into(),
            work_dir: parent.work_dir.join("new-cache-root/node"),
            frame: 0.0,
        };
        assert!(!consumer.work_dir.exists());
        let staged =
            stage_case(&consumer, &source, CaseStageKind::MeshInputs, RECIPE, false).unwrap();
        assert_eq!(staged.path, consumer.work_dir.join("case"));
        assert!(staged.path.join("system/controlDict").is_file());
    }

    #[test]
    fn wp9_stage_prefers_immutable_fields_and_records_fallback() {
        let source = input_case("stage-initial-fallback");
        std::fs::remove_file(source.join("0.orig/U")).unwrap();
        std::fs::remove_dir(source.join("0.orig")).unwrap();
        let consumer = ctx("stage-initial-consumer");
        let staged =
            stage_case(&consumer, &source, CaseStageKind::MeshInputs, RECIPE, false).unwrap();
        assert_eq!(staged.initial_fields, InitialFieldSource::CurrentZero);
        assert_eq!(
            std::fs::read(staged.path.join("0.orig/U")).unwrap(),
            b"solver-mutated U\n"
        );
        assert_eq!(
            std::fs::read(staged.path.join("0/U")).unwrap(),
            b"solver-mutated U\n"
        );
    }

    #[test]
    fn wp9_stage_mesh_inputs_exclude_old_mesh_and_generated_features() {
        let source = input_case("stage-mesh-source");
        mesh(&source);
        put(
            &source,
            "constant/triSurface/body.eMesh",
            b"old generated edges\n",
        );
        put(
            &source,
            "constant/extendedFeatureEdgeMesh/body.extendedFeatureEdgeMesh",
            b"old generated features\n",
        );
        let staged = stage_case(
            &ctx("stage-mesh-consumer"),
            &source,
            CaseStageKind::MeshInputs,
            RECIPE,
            false,
        )
        .unwrap();
        assert!(
            !staged.path.join("constant/polyMesh").exists(),
            "a new mesh must not inherit old mesh tables"
        );
        assert!(!staged.path.join("constant/triSurface/body.eMesh").exists());
        assert!(!staged
            .path
            .join("constant/extendedFeatureEdgeMesh")
            .exists());
        assert!(staged.path.join("constant/triSurface/body.stl").is_file());
    }

    #[test]
    fn wp9_stage_solver_requires_complete_nonempty_mesh_tables() {
        let source = input_case("stage-solver-source");
        put(
            &source,
            "constant/polyMesh/boundary",
            b"not a complete mesh\n",
        );
        let consumer = ctx("stage-solver-consumer");
        assert!(
            stage_case(
                &consumer,
                &source,
                CaseStageKind::SolverInputs,
                RECIPE,
                false
            )
            .is_err(),
            "boundary alone must not certify solver inputs"
        );
        assert!(!consumer.work_dir.join("case").exists());
        mesh(&source);
        put(&source, "constant/polyMesh/owner", b"");
        assert!(
            stage_case(
                &consumer,
                &source,
                CaseStageKind::SolverInputs,
                RECIPE,
                false
            )
            .is_err(),
            "empty mesh tables must fail before publishing"
        );
        mesh(&source);
        let staged = stage_case(
            &consumer,
            &source,
            CaseStageKind::SolverInputs,
            RECIPE,
            false,
        )
        .unwrap();
        assert_eq!(staged.path, consumer.work_dir.join("case"));
        for name in ["points", "faces", "owner", "neighbour", "boundary"] {
            assert_eq!(
                std::fs::read(staged.path.join("constant/polyMesh").join(name)).unwrap(),
                std::fs::read(source.join("constant/polyMesh").join(name)).unwrap()
            );
        }
    }

    #[test]
    fn wp9_stage_unchanged_planning_preserves_file_and_directory_times() {
        let source = input_case("stage-stable-source");
        let consumer = ctx("stage-stable-consumer");
        let first =
            stage_case(&consumer, &source, CaseStageKind::MeshInputs, RECIPE, false).unwrap();
        let payload = first.path.join("system/controlDict");
        let sentinel_time = SystemTime::UNIX_EPOCH + Duration::from_secs(123_456);
        std::fs::File::options()
            .write(true)
            .open(&payload)
            .unwrap()
            .set_modified(sentinel_time)
            .unwrap();
        let before = files(&first.path);
        let dir_time = std::fs::metadata(&first.path).unwrap().modified().unwrap();
        let second =
            stage_case(&consumer, &source, CaseStageKind::MeshInputs, RECIPE, false).unwrap();
        assert!(
            second.reused,
            "identical input content and recipe should reuse the marked clone"
        );
        assert_eq!(second.path, first.path);
        assert_eq!(
            files(&second.path),
            before,
            "unchanged staging must not rewrite copied files or its marker"
        );
        assert_eq!(
            std::fs::metadata(&second.path).unwrap().modified().unwrap(),
            dir_time
        );
    }

    #[test]
    fn wp9_stage_changed_and_removed_inputs_replace_active_case_and_preserve_previous() {
        let source = input_case("stage-change-source");
        put(&source, "system/topoSetDict", b"old optional physics\n");
        let consumer = ctx("stage-change-consumer");
        let first =
            stage_case(&consumer, &source, CaseStageKind::MeshInputs, RECIPE, false).unwrap();
        assert_eq!(first.path, consumer.work_dir.join("case"));
        put(&first.path, "user-notes.txt", b"do not delete my notes\n");
        put(&source, "system/controlDict", b"control changed!\n");
        std::fs::remove_file(source.join("system/topoSetDict")).unwrap();
        let next =
            stage_case(&consumer, &source, CaseStageKind::MeshInputs, RECIPE, false).unwrap();
        assert!(!next.reused);
        assert!(!next.path.join("system/topoSetDict").exists());
        assert!(!next.path.join("user-notes.txt").exists());
        assert_eq!(
            std::fs::read(next.path.join("system/controlDict")).unwrap(),
            b"control changed!\n"
        );
        let previous = consumer.work_dir.join("case.previous-0");
        assert_eq!(
            std::fs::read(previous.join("user-notes.txt")).unwrap(),
            b"do not delete my notes\n"
        );
        assert_eq!(
            std::fs::read(previous.join("system/topoSetDict")).unwrap(),
            b"old optional physics\n"
        );
    }

    #[test]
    fn wp9_stage_recipe_changes_drop_caller_generated_optional_physics() {
        let source = input_case("stage-recipe-source");
        let consumer = ctx("stage-recipe-consumer");
        let first = stage_case(
            &consumer,
            &source,
            CaseStageKind::MeshInputs,
            "template-v1:mrf-enabled",
            false,
        )
        .unwrap();
        assert_eq!(first.path, consumer.work_dir.join("case"));
        put(
            &first.path,
            "constant/MRFProperties",
            b"caller generated rotating region\n",
        );
        let next = stage_case(
            &consumer,
            &source,
            CaseStageKind::MeshInputs,
            "template-v1:mrf-disabled",
            false,
        )
        .unwrap();
        assert!(!next.reused);
        assert!(!next.path.join("constant/MRFProperties").exists());
        assert!(consumer
            .work_dir
            .join("case.previous-0/constant/MRFProperties")
            .is_file());
    }

    #[test]
    fn wp9_stage_fresh_run_does_not_relabel_previous_results() {
        let source = input_case("stage-fresh-source");
        mesh(&source);
        let consumer = ctx("stage-fresh-consumer");
        let first = stage_case(
            &consumer,
            &source,
            CaseStageKind::SolverInputs,
            RECIPE,
            false,
        )
        .unwrap();
        assert_eq!(first.path, consumer.work_dir.join("case"));
        for name in [
            "postProcessing/forceCoeffs/0/coefficient.dat",
            "logs/solver.log",
            "vernon_results.stamp",
            "100/U",
        ] {
            put(&first.path, name, b"previous completed result\n");
        }
        let fresh = stage_case(
            &consumer,
            &source,
            CaseStageKind::SolverInputs,
            RECIPE,
            true,
        )
        .unwrap();
        assert!(!fresh.reused);
        for name in ["postProcessing", "logs", "vernon_results.stamp", "100"] {
            assert!(
                !fresh.path.join(name).exists(),
                "new attempt must not inherit {name}"
            );
            assert!(
                consumer
                    .work_dir
                    .join("case.previous-0")
                    .join(name)
                    .exists(),
                "prior results must remain available for explicit import"
            );
        }
        assert!(fresh.path.join("constant/polyMesh/points").is_file());
    }

    #[test]
    fn wp9_stage_refuses_unmarked_or_corrupt_owned_destination() {
        let source = input_case("stage-unmarked-source");
        let consumer = ctx("stage-unmarked-consumer");
        put(&consumer.work_dir, "case/user.txt", b"user data\n");
        assert!(
            stage_case(&consumer, &source, CaseStageKind::MeshInputs, RECIPE, false).is_err(),
            "an existing unmarked destination must not be adopted or replaced"
        );
        assert_eq!(
            std::fs::read(consumer.work_dir.join("case/user.txt")).unwrap(),
            b"user data\n"
        );
        let other = ctx("stage-corrupt-consumer");
        stage_case(&other, &source, CaseStageKind::MeshInputs, RECIPE, false).unwrap();
        put(
            &other.work_dir,
            "case/.vernon-stage.json",
            b"{invalid marker\n",
        );
        assert!(
            stage_case(&other, &source, CaseStageKind::MeshInputs, RECIPE, true).is_err(),
            "corrupt ownership metadata must not authorize replacement"
        );
    }

    #[test]
    fn wp9_stage_refuses_missing_source_and_overlapping_roots() {
        let consumer = ctx("stage-missing-consumer");
        let source = consumer.work_dir.join("missing-input");
        assert!(
            stage_case(&consumer, &source, CaseStageKind::MeshInputs, RECIPE, false).is_err(),
            "missing upstream must not be silently created"
        );
        assert!(!source.exists());
        let overlap = input_case("stage-overlap");
        let inside = CookContext {
            node_name: "nested consumer".into(),
            work_dir: overlap.join("consumer"),
            frame: 0.0,
        };
        std::fs::create_dir_all(&inside.work_dir).unwrap();
        assert!(stage_case(&inside, &overlap, CaseStageKind::MeshInputs, RECIPE, false).is_err());
        assert!(!inside.work_dir.join("case").exists());
    }

    fn limit_failure(name: &str, limits: StageLimits) {
        let source = input_case(name);
        put(&source, "system/nested/deeper/config", b"bounded scan\n");
        let consumer = ctx(&format!("{name}-consumer"));
        assert!(
            stage_case_with_limits(
                &consumer,
                &source,
                CaseStageKind::MeshInputs,
                RECIPE,
                false,
                limits
            )
            .is_err(),
            "incomplete staging must not produce a successful path"
        );
        assert!(!consumer.work_dir.join("case").exists());
    }

    #[test]
    fn wp9_stage_entry_budget_is_not_a_partial_success() {
        limit_failure(
            "stage-entry-limit",
            StageLimits {
                entries: 1,
                ..StageLimits::default()
            },
        );
    }

    #[test]
    fn wp9_stage_depth_budget_is_not_a_partial_success() {
        limit_failure(
            "stage-depth-limit",
            StageLimits {
                depth: 1,
                ..StageLimits::default()
            },
        );
    }

    #[test]
    fn wp9_stage_byte_budget_is_not_a_partial_success() {
        limit_failure(
            "stage-byte-limit",
            StageLimits {
                bytes: 1,
                ..StageLimits::default()
            },
        );
    }

    #[test]
    fn wp9_stage_cancel_does_not_publish_or_replace_a_case() {
        let source = input_case("stage-cancel-source");
        let consumer = ctx("stage-cancel-consumer");
        let flag = Arc::new(AtomicBool::new(true));
        let result = vernon_core::process::with_cancel(flag, || {
            stage_case(&consumer, &source, CaseStageKind::MeshInputs, RECIPE, false)
        });
        assert!(
            result.is_err(),
            "staging must honor the cook cancellation flag"
        );
        assert!(!consumer.work_dir.join("case").exists());
    }

    #[test]
    fn wp9_stage_retention_exhaustion_preserves_active_case_and_user_data() {
        let source = input_case("stage-retained-source");
        let consumer = ctx("stage-retained-consumer");
        let first =
            stage_case(&consumer, &source, CaseStageKind::MeshInputs, RECIPE, false).unwrap();
        assert_eq!(first.path, consumer.work_dir.join("case"));
        for i in 0..2 {
            put(
                &consumer.work_dir,
                &format!("case.previous-{i}/user.txt"),
                format!("preserved {i}\n").as_bytes(),
            );
        }
        let before = files(&consumer.work_dir);
        let result = stage_case_with_limits(
            &consumer,
            &source,
            CaseStageKind::MeshInputs,
            RECIPE,
            true,
            StageLimits {
                retained: 2,
                ..StageLimits::default()
            },
        );
        assert!(
            result.is_err(),
            "a full retention budget must not delete a prior generation"
        );
        assert_eq!(files(&consumer.work_dir), before);
    }

    #[cfg(any(unix, windows))]
    fn directory_link(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // A local directory junction requires no administrator privilege.
            // Typed arguments create only this test-owned fixture; no deletion.
            // cmd's mklink treats forward slashes inside mixed-separator paths
            // as switches, unlike the Rust filesystem APIs used by the tests.
            let link = link.as_os_str().to_string_lossy().replace('/', "\\");
            let target = target.as_os_str().to_string_lossy().replace('/', "\\");
            let out = std::process::Command::new("cmd.exe")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .creation_flags(0x0800_0000)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "junction fixture failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn wp9_stage_rejects_source_links_and_linked_ancestors() {
        let source = input_case("stage-link-source");
        let outside = tmp("stage-link-outside");
        put(&outside, "user.txt", b"outside sentinel\n");
        directory_link(&outside, &source.join("system/escape"));
        let before = files(&outside);
        let consumer = ctx("stage-link-consumer");
        assert!(
            stage_case(&consumer, &source, CaseStageKind::MeshInputs, RECIPE, false).is_err(),
            "links and junctions must never be traversed"
        );
        assert!(!consumer.work_dir.join("case").exists());
        assert_eq!(files(&outside), before);

        let root = tmp("stage-link-parent");
        let real = input_case("stage-real-parent");
        directory_link(&real, &root.join("alias"));
        let nested = root.join("alias/constant");
        assert!(
            stage_case(&consumer, &nested, CaseStageKind::MeshInputs, RECIPE, false).is_err(),
            "a link in an ancestor must also be rejected"
        );
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn wp9_stage_rejects_linked_destination_without_writing_outside() {
        let source = input_case("stage-output-link-source");
        let outside = tmp("stage-output-link-outside");
        put(&outside, "user.txt", b"outside destination sentinel\n");
        let consumer = ctx("stage-output-link-consumer");
        directory_link(&outside, &consumer.work_dir.join("case"));
        let before = files(&outside);
        assert!(stage_case(&consumer, &source, CaseStageKind::MeshInputs, RECIPE, false).is_err());
        assert_eq!(files(&outside), before);
    }
}

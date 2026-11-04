//! Input-bound reference envelopes, not geometry admission or authentication.
//! Callers must independently recognize actual geometry and the supported
//! protocol through `reference`; a readable record alone proves neither.

use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use vernon_graph::{CookContext, CookResult};

use crate::reference::ReferenceBinding;
use crate::util::case_input_fingerprint;

pub(crate) const FILE_NAME: &str = "vernon_replication.json";
const VERSION: u32 = 1;
const RECORD_BYTES: usize = 4096;
const TEMP_PREFIX: &str = ".vernon-replication.pending-";
const TEMP_SLOTS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Setup,
    Solved,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub(crate) version: u32,
    pub(crate) phase: Phase,
    pub(crate) binding: ReferenceBinding,
    pub(crate) input_fingerprint: String,
}

fn canceled(ctx: &CookContext) -> CookResult<()> {
    if vernon_core::process::cancel_requested() {
        Err(ctx.fail("reference envelope operation canceled"))
    } else {
        Ok(())
    }
}

fn linked(meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

/// No cancellation check here: failure cleanup must still be able to validate
/// its single owned temporary file after the cook cancellation flag is set.
fn ordinary_path(ctx: &CookContext, path: &Path, directory: bool) -> CookResult<Metadata> {
    let mut leaf = None;
    for part in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let meta = std::fs::symlink_metadata(part)
            .map_err(|e| ctx.fail(format!("cannot inspect {}: {e}", part.display())))?;
        let want_directory = part != path || directory;
        if linked(&meta) || (want_directory && !meta.is_dir()) || (!want_directory && !meta.is_file()) {
            return Err(ctx.fail(format!("reference envelopes refuse links, reparse points and nonordinary paths: {}", part.display())));
        }
        leaf = Some(meta);
    }
    leaf.ok_or_else(|| ctx.fail("reference envelope path has no filesystem component"))
}

fn checked_directory(ctx: &CookContext, path: &Path) -> CookResult<PathBuf> {
    canceled(ctx)?;
    if path.components().any(|part| matches!(part, Component::ParentDir)) {
        return Err(ctx.fail("reference envelope paths cannot contain parent-directory components"));
    }
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| ctx.fail(format!("cannot resolve reference envelope path: {e}")))?
            .join(path)
    };
    ordinary_path(ctx, &path, true)?;
    std::fs::canonicalize(&path)
        .map_err(|e| ctx.fail(format!("cannot resolve {}: {e}", path.display())))
}

fn owned_case(ctx: &CookContext, case: &Path) -> CookResult<PathBuf> {
    let work = checked_directory(ctx, &ctx.work_dir)?;
    let case = checked_directory(ctx, case)?;
    if !case.starts_with(work) {
        return Err(ctx.fail("reference envelope writes and removal require a case within ctx.work_dir"));
    }
    Ok(case)
}

#[derive(Debug, PartialEq, Eq)]
struct FileState {
    bytes: u64,
    modified: SystemTime,
}

fn state(ctx: &CookContext, meta: &Metadata) -> CookResult<FileState> {
    Ok(FileState {
        bytes: meta.len(),
        modified: meta.modified().map_err(|e| ctx.fail(format!("cannot inspect reference envelope timestamp: {e}")))?,
    })
}

fn file_state(ctx: &CookContext, path: &Path) -> CookResult<Option<FileState>> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ctx.fail(format!("cannot inspect {}: {e}", path.display()))),
        Ok(_) => state(ctx, &ordinary_path(ctx, path, false)?).map(Some),
    }
}

fn identifier_ok(value: &str) -> bool {
    value.len() == 16 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn expected_inputs(ctx: &CookContext, case: &Path, expected: &str) -> CookResult<()> {
    canceled(ctx)?;
    if !identifier_ok(expected) || case_input_fingerprint(ctx, case)? != expected {
        return Err(ctx.fail("reference envelope expected input fingerprint does not match the current physical inputs"));
    }
    Ok(())
}

fn temporary_path(ctx: &CookContext, case: &Path) -> CookResult<PathBuf> {
    for index in 0..TEMP_SLOTS {
        canceled(ctx)?;
        let path = case.join(format!("{TEMP_PREFIX}{index}"));
        if file_state(ctx, &path)?.is_none() {
            return Ok(path);
        }
    }
    Err(ctx.fail("reference envelope temporary file budget is full; existing files were preserved"))
}

pub(crate) fn write(
    ctx: &CookContext,
    case: &Path,
    phase: Phase,
    binding: &ReferenceBinding,
    expected_fingerprint: &str,
) -> CookResult<()> {
    let case = owned_case(ctx, case)?;
    let target = case.join(FILE_NAME);
    let previous = file_state(ctx, &target)?;
    expected_inputs(ctx, &case, expected_fingerprint)?;
    let record = Record {
        version: VERSION,
        phase,
        binding: binding.clone(),
        input_fingerprint: expected_fingerprint.into(),
    };
    let bytes = serde_json::to_vec_pretty(&record)
        .map_err(|e| ctx.fail(format!("cannot encode reference envelope: {e}")))?;
    if bytes.len() > RECORD_BYTES {
        return Err(ctx.fail("reference envelope exceeds its 4096-byte budget"));
    }
    // This is structural JSON validation only. In particular, serde writes
    // nonfinite floats as null; do not publish a record that cannot be read.
    serde_json::from_slice::<Record>(&bytes)
        .map_err(|e| ctx.fail(format!("reference envelope cannot roundtrip as structured JSON: {e}")))?;
    let temporary = temporary_path(ctx, &case)?;
    ordinary_path(ctx, &case, true)?;
    let mut writer = OpenOptions::new().write(true).create_new(true).open(&temporary)
        .map_err(|e| ctx.fail(format!("cannot create reference envelope temporary file: {e}")))?;
    let publish = (|| -> CookResult<()> {
        writer.write_all(&bytes).and_then(|()| writer.flush())
            .map_err(|e| ctx.fail(format!("cannot write reference envelope: {e}")))?;
        let written = state(ctx, &writer.metadata()
            .map_err(|e| ctx.fail(format!("cannot inspect reference envelope temporary file: {e}")))?)?;
        drop(writer);
        // Check inputs again AFTER preparing the complete temporary record,
        // BEFORE promotion: an unsuccessful attempt preserves the old proof.
        expected_inputs(ctx, &case, expected_fingerprint)?;
        if file_state(ctx, &target)? != previous {
            return Err(ctx.fail("reference envelope destination changed before publication"));
        }
        if file_state(ctx, &temporary)? != Some(written) {
            return Err(ctx.fail("reference envelope temporary file changed before publication"));
        }
        canceled(ctx)?;
        std::fs::rename(&temporary, &target)
            .map_err(|e| ctx.fail(format!("cannot atomically publish reference envelope: {e}")))
    })();
    if let Err(original) = publish {
        // Remove only the ordinary single file successfully create_new above.
        // Never follow a replacement ancestor/link to clean an outside path.
        let cleanup = ordinary_path(ctx, &temporary, false)
            .and_then(|_| std::fs::remove_file(&temporary)
                .map_err(|e| ctx.fail(format!("cannot remove owned reference temporary file: {e}"))));
        return match cleanup {
            Ok(()) => Err(original),
            Err(cleanup) => Err(ctx.fail(format!("{original}; temporary cleanup refused or failed: {cleanup}"))),
        };
    }
    Ok(())
}

pub(crate) fn read(ctx: &CookContext, case: &Path, expected_phase: Phase) -> CookResult<Option<Record>> {
    let case = checked_directory(ctx, case)?;
    let path = case.join(FILE_NAME);
    let Some(before) = file_state(ctx, &path)? else { return Ok(None) };
    if before.bytes > RECORD_BYTES as u64 {
        return Err(ctx.fail("reference envelope exceeds its 4096-byte read budget"));
    }
    let mut file = File::open(&path)
        .map_err(|e| ctx.fail(format!("cannot open reference envelope: {e}")))?;
    if state(ctx, &file.metadata().map_err(|e| ctx.fail(format!("cannot inspect open reference envelope: {e}")))?)? != before {
        return Err(ctx.fail("reference envelope changed while opening"));
    }
    let mut bytes = Vec::new();
    (&mut file).take(RECORD_BYTES as u64 + 1).read_to_end(&mut bytes)
        .map_err(|e| ctx.fail(format!("cannot read reference envelope: {e}")))?;
    canceled(ctx)?;
    if bytes.len() as u64 != before.bytes
        || state(ctx, &file.metadata().map_err(|e| ctx.fail(format!("cannot inspect open reference envelope: {e}")))?)? != before
    {
        return Err(ctx.fail("reference envelope changed or exceeded its budget while reading"));
    }
    let record: Record = serde_json::from_slice(&bytes)
        .map_err(|e| ctx.fail(format!("invalid reference envelope JSON: {e}")))?;
    if record.version != VERSION || record.phase != expected_phase || !identifier_ok(&record.input_fingerprint) {
        return Err(ctx.fail("reference envelope version, phase or input identifier is invalid"));
    }
    expected_inputs(ctx, &case, &record.input_fingerprint)?;
    if file_state(ctx, &path)? != Some(before) {
        return Err(ctx.fail("reference envelope changed while verifying physical inputs"));
    }
    canceled(ctx)?;
    Ok(Some(record))
}

pub(crate) fn remove(ctx: &CookContext, case: &Path) -> CookResult<()> {
    let case = owned_case(ctx, case)?;
    let path = case.join(FILE_NAME);
    if file_state(ctx, &path)?.is_none() {
        return Ok(());
    }
    canceled(ctx)?;
    ordinary_path(ctx, &path, false)?;
    std::fs::remove_file(&path)
        .map_err(|e| ctx.fail(format!("cannot remove owned reference envelope: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::GeometryBinding;
    use crate::util::{case_input_fingerprint, stage_case, CaseStageKind};
    use crate::util::testutil::ctx;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::SystemTime;

    fn put(case: &Path, name: &str, bytes: &[u8]) {
        let path = case.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn tiny_case(name: &str) -> (CookContext, PathBuf) {
        let context = ctx(name);
        let case = context.work_dir.join("case");
        put(&case, "system/controlDict", b"unit solver inputs\n");
        put(&case, "constant/physicalProperties", b"unit physical inputs\n");
        put(&case, "0/U", b"unit initial field\n");
        put(&case, "0.orig/U", b"unit initial field\n");
        put(&case, "vernon_case.json", b"{}\n");
        (context, case)
    }

    fn binding() -> ReferenceBinding {
        // Deliberately unadmitted metadata. Envelope IO must not be confused
        // with the real registry's geometry or protocol eligibility decision.
        ReferenceBinding {
            version: 1,
            profile: "synthetic-unadmitted-envelope-fixture-v1".into(),
            geometry: GeometryBinding {
                version: 1,
                candidate: "synthetic-envelope-only".into(),
                preparation_revision: "unit-envelope-v1".into(),
                tri_surface_fingerprint: "67577c1e3392b746".into(),
            },
            u_ms: 16.0,
            rho: 1.184,
            nu: 1.567e-5,
            full_area_m2: 2.16,
            reference_length_m: 4.6,
            half_model: true,
            ground_mode: "moving".into(),
            wheel_mode: "rotating".into(),
            wheel_omega_rad_s: -51.0,
        }
    }

    /// Independent reader fixture; never calls the record writer under test.
    fn record_fixture(context: &CookContext, case: &Path, phase: Phase) -> Record {
        let record = Record {
            version: VERSION,
            phase,
            binding: binding(),
            input_fingerprint: case_input_fingerprint(context, case).unwrap(),
        };
        put(case, FILE_NAME, &serde_json::to_vec(&record).unwrap());
        record
    }

    fn files(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
        fn collect(root: &Path, directory: &Path, out: &mut BTreeMap<PathBuf, (Vec<u8>, SystemTime)>) {
            for entry in std::fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                let meta = std::fs::symlink_metadata(&path).unwrap();
                assert!(!meta.file_type().is_symlink());
                if meta.is_dir() {
                    collect(root, &path, out);
                } else {
                    out.insert(path.strip_prefix(root).unwrap().into(), (std::fs::read(&path).unwrap(), meta.modified().unwrap()));
                }
            }
        }
        let mut out = BTreeMap::new();
        collect(root, root, &mut out);
        out
    }

    #[test]
    fn wp9_reference_record_writes_setup_then_solved_without_circular_hash() {
        let (context, case) = tiny_case("reference-record-roundtrip");
        let fingerprint = case_input_fingerprint(&context, &case).unwrap();
        for phase in [Phase::Setup, Phase::Solved] {
            write(&context, &case, phase, &binding(), &fingerprint).unwrap();
            assert!(case.join(FILE_NAME).is_file(), "successful writes must persist one complete envelope");
            let record = read(&context, &case, phase).unwrap().expect("written envelope must be readable");
            assert_eq!(record.version, VERSION);
            assert_eq!(record.phase, phase);
            assert_eq!(record.binding, binding());
            assert_eq!(record.input_fingerprint, fingerprint);
            assert_eq!(case_input_fingerprint(&context, &case).unwrap(), fingerprint, "the root sidecar must be excluded from the physical input hash");
        }
    }

    #[test]
    fn wp9_reference_record_reads_independent_unadmitted_fixture_without_rewriting() {
        let (context, case) = tiny_case("reference-record-independent");
        let expected = record_fixture(&context, &case, Phase::Setup);
        let before = files(&case);
        assert_eq!(read(&context, &case, Phase::Setup).unwrap(), Some(expected));
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_reference_record_rejects_setup_solved_phase_swaps() {
        let (context, case) = tiny_case("reference-record-phase");
        record_fixture(&context, &case, Phase::Setup);
        assert!(read(&context, &case, Phase::Solved).is_err());
        record_fixture(&context, &case, Phase::Solved);
        assert!(read(&context, &case, Phase::Setup).is_err());
    }

    #[test]
    fn wp9_reference_record_rejects_same_size_physical_input_edit() {
        let (context, case) = tiny_case("reference-record-edited-physics");
        record_fixture(&context, &case, Phase::Solved);
        let path = case.join("constant/physicalProperties");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] ^= 1;
        std::fs::write(path, bytes).unwrap();
        let before = files(&case);
        assert!(read(&context, &case, Phase::Solved).is_err());
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_reference_record_failed_expected_hash_preserves_existing_proof() {
        let (context, case) = tiny_case("reference-record-expected-mismatch");
        record_fixture(&context, &case, Phase::Setup);
        let before = files(&case);
        let wrong = "0000000000000000";
        assert_ne!(case_input_fingerprint(&context, &case).unwrap(), wrong);
        assert!(write(&context, &case, Phase::Solved, &binding(), wrong).is_err());
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_reference_record_changed_physics_cannot_be_blessed_with_a_new_hash() {
        let (context, case) = tiny_case("reference-record-write-old-physics");
        let old = record_fixture(&context, &case, Phase::Setup);
        put(&case, "0.orig/U", b"a different initial field\n");
        let before = files(&case);
        assert!(write(&context, &case, Phase::Solved, &binding(), &old.input_fingerprint).is_err());
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_reference_record_missing_file_is_none_and_read_does_not_create_work() {
        let (_, case) = tiny_case("reference-record-missing-source");
        let mut consumer = ctx("reference-record-missing-reader");
        consumer.work_dir = consumer.work_dir.join("not-created/work");
        let before = files(&case);
        assert_eq!(read(&consumer, &case, Phase::Setup).unwrap(), None);
        assert!(!consumer.work_dir.exists());
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_reference_record_upstream_read_is_allowed_without_owned_work() {
        let (source_context, case) = tiny_case("reference-record-upstream");
        let expected = record_fixture(&source_context, &case, Phase::Solved);
        let mut consumer = ctx("reference-record-upstream-reader");
        consumer.work_dir = consumer.work_dir.join("not-created/work");
        let before = files(&case);
        assert_eq!(read(&consumer, &case, Phase::Solved).unwrap(), Some(expected));
        assert!(!consumer.work_dir.exists());
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_reference_record_missing_case_is_error_without_creation() {
        let context = ctx("reference-record-no-case");
        let case = context.work_dir.join("missing/case");
        assert!(read(&context, &case, Phase::Setup).is_err());
        assert!(!case.exists());
    }

    #[test]
    fn wp9_reference_record_rejects_invalid_schema_phase_and_input_id() {
        let (context, case) = tiny_case("reference-record-schema");
        let good = serde_json::to_value(record_fixture(&context, &case, Phase::Setup)).unwrap();
        for (field, value) in [
            ("version", serde_json::json!(2)),
            ("phase", serde_json::json!("other")),
            ("input_fingerprint", serde_json::json!("bad")),
            ("input_fingerprint", serde_json::json!("zzzzzzzzzzzzzzzz")),
            ("input_fingerprint", serde_json::json!("0000000000000000")),
            ("unexpected", serde_json::json!(true)),
            ("binding", serde_json::Value::Null),
        ] {
            let mut bad = good.clone();
            bad[field] = value;
            put(&case, FILE_NAME, &serde_json::to_vec(&bad).unwrap());
            assert!(read(&context, &case, Phase::Setup).is_err(), "invalid {field} cannot be a bound reference envelope");
        }
        let mut missing = good;
        missing.as_object_mut().unwrap().remove("input_fingerprint");
        put(&case, FILE_NAME, &serde_json::to_vec(&missing).unwrap());
        assert!(read(&context, &case, Phase::Setup).is_err());
    }

    #[test]
    fn wp9_reference_record_rejects_malformed_empty_and_oversized_json() {
        let (context, case) = tiny_case("reference-record-json-bounds");
        let good = serde_json::to_vec(&record_fixture(&context, &case, Phase::Setup)).unwrap();
        let mut oversized = good;
        oversized.resize(RECORD_BYTES + 1, b' ');
        for bytes in [b"".to_vec(), b"not JSON".to_vec(), oversized] {
            put(&case, FILE_NAME, &bytes);
            assert!(read(&context, &case, Phase::Setup).is_err());
        }
    }

    #[test]
    fn wp9_reference_record_oversized_write_preserves_existing_record() {
        let (context, case) = tiny_case("reference-record-large-write");
        let old = record_fixture(&context, &case, Phase::Setup);
        let mut big = binding();
        big.geometry.preparation_revision = "x".repeat(RECORD_BYTES + 1);
        let before = files(&case);
        assert!(write(&context, &case, Phase::Solved, &big, &old.input_fingerprint).is_err());
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_reference_record_write_rejects_invalid_expected_identifier() {
        let (context, case) = tiny_case("reference-record-invalid-expected");
        let before = files(&case);
        for invalid in ["", "bad", "zzzzzzzzzzzzzzzz"] {
            assert!(write(&context, &case, Phase::Setup, &binding(), invalid).is_err());
            assert_eq!(files(&case), before);
        }
    }

    #[test]
    fn wp9_reference_record_cancellation_preserves_proof_for_all_operations() {
        let (context, case) = tiny_case("reference-record-cancel");
        let old = record_fixture(&context, &case, Phase::Setup);
        let before = files(&case);
        assert!(vernon_core::process::with_cancel(Arc::new(AtomicBool::new(true)), || read(&context, &case, Phase::Setup)).is_err());
        assert!(vernon_core::process::with_cancel(Arc::new(AtomicBool::new(true)), || write(&context, &case, Phase::Solved, &binding(), &old.input_fingerprint)).is_err());
        assert!(vernon_core::process::with_cancel(Arc::new(AtomicBool::new(true)), || remove(&context, &case)).is_err());
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_reference_record_owned_remove_is_fixed_and_missing_remove_is_noop() {
        let (context, case) = tiny_case("reference-record-remove");
        record_fixture(&context, &case, Phase::Setup);
        let mut expected = files(&case);
        expected.remove(Path::new(FILE_NAME));
        remove(&context, &case).unwrap();
        assert!(!case.join(FILE_NAME).exists());
        assert_eq!(files(&case), expected);
        remove(&context, &case).unwrap();
        assert_eq!(files(&case), expected);
    }

    #[test]
    fn wp9_reference_record_outside_write_and_remove_preserve_source() {
        let (source_context, case) = tiny_case("reference-record-outside");
        let old = record_fixture(&source_context, &case, Phase::Setup);
        let before = files(&case);
        let consumer = ctx("reference-record-other-owner");
        assert!(write(&consumer, &case, Phase::Solved, &binding(), &old.input_fingerprint).is_err());
        assert!(remove(&consumer, &case).is_err());
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_reference_record_full_temp_budget_preserves_existing_files() {
        let (context, case) = tiny_case("reference-record-temp-budget");
        let old = record_fixture(&context, &case, Phase::Setup);
        for index in 0..TEMP_SLOTS {
            put(&case, &format!("{TEMP_PREFIX}{index}"), b"preexisting reserved file\n");
        }
        let before = files(&case);
        assert!(write(&context, &case, Phase::Solved, &binding(), &old.input_fingerprint).is_err());
        assert_eq!(files(&case), before);
    }

    #[test]
    fn wp9_reference_record_case_clone_does_not_automatically_copy_proof() {
        let (source_context, case) = tiny_case("reference-record-clone-source");
        let old = record_fixture(&source_context, &case, Phase::Setup);
        let consumer = ctx("reference-record-clone-consumer");
        let cloned = stage_case(&consumer, &case, CaseStageKind::CaseSetupInputs, "reference-record-clone-v1", false).unwrap();
        assert!(!cloned.path.join(FILE_NAME).exists());
        assert_eq!(case_input_fingerprint(&consumer, &cloned.path).unwrap(), old.input_fingerprint);
    }

    #[cfg(any(unix, windows))]
    fn directory_link(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let target = target.to_str().unwrap().replace('/', "\\");
            let link = link.to_str().unwrap().replace('/', "\\");
            let out = std::process::Command::new("cmd.exe")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .creation_flags(0x0800_0000)
                .output().unwrap();
            assert!(out.status.success(), "junction fixture failed: {}", String::from_utf8_lossy(&out.stderr));
        }
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn wp9_reference_record_linked_target_is_refused_without_touching_destination() {
        let (context, case) = tiny_case("reference-record-target-link");
        let outside = ctx("reference-record-target-outside");
        put(&outside.work_dir, "sentinel", b"preserved\n");
        let before = files(&outside.work_dir);
        let fingerprint = case_input_fingerprint(&context, &case).unwrap();
        directory_link(&outside.work_dir, &case.join(FILE_NAME));
        assert!(read(&context, &case, Phase::Setup).is_err());
        assert!(write(&context, &case, Phase::Setup, &binding(), &fingerprint).is_err());
        assert!(remove(&context, &case).is_err());
        assert_eq!(files(&outside.work_dir), before);
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn wp9_reference_record_linked_case_ancestor_is_refused() {
        let (source_context, case) = tiny_case("reference-record-ancestor-source");
        let old = record_fixture(&source_context, &case, Phase::Setup);
        let consumer = ctx("reference-record-ancestor-consumer");
        let alias = consumer.work_dir.join("case-alias");
        directory_link(&case, &alias);
        let before = files(&case);
        assert!(read(&consumer, &alias, Phase::Setup).is_err());
        assert!(write(&consumer, &alias, Phase::Solved, &binding(), &old.input_fingerprint).is_err());
        assert!(remove(&consumer, &alias).is_err());
        assert_eq!(files(&case), before);
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn wp9_reference_record_linked_temp_is_refused_and_old_record_preserved() {
        let (context, case) = tiny_case("reference-record-temp-link");
        let old = record_fixture(&context, &case, Phase::Setup);
        let bytes = std::fs::read(case.join(FILE_NAME)).unwrap();
        let outside = ctx("reference-record-temp-outside");
        put(&outside.work_dir, "sentinel", b"preserved\n");
        let before = files(&outside.work_dir);
        directory_link(&outside.work_dir, &case.join(format!("{TEMP_PREFIX}0")));
        assert!(write(&context, &case, Phase::Solved, &binding(), &old.input_fingerprint).is_err());
        assert_eq!(std::fs::read(case.join(FILE_NAME)).unwrap(), bytes);
        assert_eq!(files(&outside.work_dir), before);
    }
}

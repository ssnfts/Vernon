//! Process driver for the open-source photogrammetry program: command-line
//! builders kept in one table, option names resolved against the program's
//! own help text (they drift between releases), workspace handling that
//! never deletes more than the one database file it owns, run-time and
//! pair-count estimates, and a runner that streams both output pipes into a
//! log file and kills the child if the node is dropped.
//!
//! Nothing here needs the program to be installed: every node writes the
//! command lines it would run to `commands.txt` and returns a report that
//! says plainly whether anything was run.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use glam::DVec3;
use vernon_core::{Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{
    CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec,
};
use vernon_io::{Format, ReadOptions, WriteOptions};

use crate::colmap_model::{points_to_geometry, read_model_dir};
use crate::registration::principal_frame;
use crate::util::{self, base, set_metric_flag};

/// Environment variable naming the executable.
pub const ENV_EXE: &str = "VERNON_COLMAP";
/// Image extensions counted in the image folder.
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "bmp"];
/// Seconds of sparse reconstruction per image in the anchor project
/// (13 h for about 1,500 photos). A planning figure, not a benchmark.
pub const SPARSE_SECONDS_PER_IMAGE: f64 = 13.0 * 3600.0 / 1500.0;
/// Seconds of dense reconstruction per image in the same project (37 h).
pub const DENSE_SECONDS_PER_IMAGE: f64 = 37.0 * 3600.0 / 1500.0;
/// Exhaustive matching is flagged above this many image pairs.
pub const EXHAUSTIVE_PAIR_WARNING: u64 = 125_000;

// ---------------------------------------------------------------------------
// Command-line builders
// ---------------------------------------------------------------------------

/// First candidate present in the program's help text; the first candidate
/// when there is no help text or none matches.
pub fn resolve_option(help: Option<&str>, candidates: &[&str]) -> String {
    if let Some(h) = help {
        if let Some(c) = candidates.iter().find(|c| h.contains(**c)) {
            return (*c).to_string();
        }
    }
    candidates[0].to_string()
}

fn flag(name: &str) -> String {
    format!("--{name}")
}

fn b01(v: bool) -> String {
    (if v { "1" } else { "0" }).to_string()
}

/// Feature extraction settings.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractorSettings {
    pub camera_model: String,
    pub single_camera: bool,
    pub max_image_size: i64,
    pub max_features: i64,
    pub use_gpu: bool,
    pub feature_type: String,
}

impl Default for ExtractorSettings {
    fn default() -> Self {
        ExtractorSettings {
            camera_model: "SIMPLE_RADIAL".into(),
            single_camera: false,
            max_image_size: 3200,
            max_features: 8192,
            use_gpu: true,
            feature_type: "SIFT".into(),
        }
    }
}

/// Arguments of the feature extractor.
pub fn extractor_args(db: &Path, images: &Path, s: &ExtractorSettings, help: Option<&str>) -> Vec<String> {
    let size = resolve_option(help, &["FeatureExtraction.max_image_size", "SiftExtraction.max_image_size"]);
    let mut a = vec![
        "feature_extractor".to_string(),
        flag("database_path"),
        db.to_string_lossy().into_owned(),
        flag("image_path"),
        images.to_string_lossy().into_owned(),
        flag("ImageReader.camera_model"),
        s.camera_model.clone(),
        flag("ImageReader.single_camera"),
        b01(s.single_camera),
        flag(&size),
        s.max_image_size.to_string(),
        flag("SiftExtraction.max_num_features"),
        s.max_features.to_string(),
    ];
    if !s.use_gpu {
        let gpu = resolve_option(help, &["FeatureExtraction.use_gpu", "SiftExtraction.use_gpu"]);
        a.push(flag(&gpu));
        a.push("0".into());
    }
    if s.feature_type != "SIFT" {
        a.push(flag("FeatureExtraction.type"));
        a.push(s.feature_type.clone());
    }
    a
}

/// Matching strategy.
#[derive(Debug, Clone, PartialEq)]
pub struct MatcherSettings {
    pub kind: String,
    pub guided: bool,
    pub use_gpu: bool,
    pub overlap: i64,
    pub neighbors: i64,
    pub vocab_tree: String,
}

/// Arguments of the chosen matcher.
pub fn matcher_args(db: &Path, m: &MatcherSettings, help: Option<&str>) -> Result<Vec<String>, String> {
    let db_s = db.to_string_lossy().into_owned();
    let mut a = match m.kind.as_str() {
        "exhaustive" => vec!["exhaustive_matcher".to_string(), flag("database_path"), db_s],
        "sequential" => vec![
            "sequential_matcher".to_string(),
            flag("database_path"),
            db_s,
            flag("SequentialMatching.overlap"),
            m.overlap.to_string(),
        ],
        "vocab_tree" => {
            let mut v = vec!["vocab_tree_matcher".to_string(), flag("database_path"), db_s];
            if !m.vocab_tree.is_empty() {
                v.push(flag("VocabTreeMatching.vocab_tree_path"));
                v.push(m.vocab_tree.clone());
            }
            v
        }
        "spatial" => vec![
            "spatial_matcher".to_string(),
            flag("database_path"),
            db_s,
            flag("SpatialMatching.max_num_neighbors"),
            m.neighbors.to_string(),
        ],
        other => return Err(format!("unknown matcher '{other}'")),
    };
    if m.guided {
        a.push(flag(&resolve_option(
            help,
            &["FeatureMatching.guided_matching", "SiftMatching.guided_matching"],
        )));
        a.push("1".into());
    }
    if !m.use_gpu {
        a.push(flag(&resolve_option(help, &["FeatureMatching.use_gpu", "SiftMatching.use_gpu"])));
        a.push("0".into());
    }
    Ok(a)
}

/// Sparse mapper settings.
#[derive(Debug, Clone, PartialEq)]
pub struct MapperSettings {
    pub kind: String,
    pub refine_focal: bool,
    pub refine_principal: bool,
    pub init_pair: Option<(i64, i64)>,
}

/// Arguments of the sparse mapper.
pub fn mapper_args(db: &Path, images: &Path, out: &Path, m: &MapperSettings) -> Result<Vec<String>, String> {
    let cmd = match m.kind.as_str() {
        "incremental" => "mapper",
        "global" => "global_mapper",
        "hierarchical" => "hierarchical_mapper",
        other => return Err(format!("unknown mapper '{other}'")),
    };
    let mut a = vec![
        cmd.to_string(),
        flag("database_path"),
        db.to_string_lossy().into_owned(),
        flag("image_path"),
        images.to_string_lossy().into_owned(),
        flag("output_path"),
        out.to_string_lossy().into_owned(),
    ];
    if m.kind == "incremental" {
        if !m.refine_focal {
            a.push(flag("Mapper.ba_refine_focal_length"));
            a.push("0".into());
        }
        if m.refine_principal {
            a.push(flag("Mapper.ba_refine_principal_point"));
            a.push("1".into());
        }
        if let Some((i, j)) = m.init_pair {
            a.push(flag("Mapper.init_image_id1"));
            a.push(i.to_string());
            a.push(flag("Mapper.init_image_id2"));
            a.push(j.to_string());
        }
    }
    Ok(a)
}

/// Arguments converting a binary model to text tables.
pub fn model_to_text_args(model: &Path) -> Vec<String> {
    vec![
        "model_converter".into(),
        flag("input_path"),
        model.to_string_lossy().into_owned(),
        flag("output_path"),
        model.to_string_lossy().into_owned(),
        flag("output_type"),
        "TXT".into(),
    ]
}

/// Dense reconstruction settings; zero means the program's own default.
#[derive(Debug, Clone, PartialEq)]
pub struct DenseSettings {
    pub max_image_size: i64,
    pub geom_consistency: bool,
    pub min_num_pixels: i64,
    pub window_radius: i64,
}

/// The three dense steps in order: undistort, stereo, fusion.
pub fn dense_steps(images: &Path, model: &Path, dense: &Path, d: &DenseSettings) -> Vec<Vec<String>> {
    let dense_s = dense.to_string_lossy().into_owned();
    let mut stereo = vec![
        "patch_match_stereo".to_string(),
        flag("workspace_path"),
        dense_s.clone(),
        flag("workspace_format"),
        "COLMAP".into(),
        flag("PatchMatchStereo.geom_consistency"),
        (if d.geom_consistency { "true" } else { "false" }).to_string(),
    ];
    if d.max_image_size > 0 {
        stereo.push(flag("PatchMatchStereo.max_image_size"));
        stereo.push(d.max_image_size.to_string());
    }
    if d.window_radius > 0 {
        stereo.push(flag("PatchMatchStereo.window_radius"));
        stereo.push(d.window_radius.to_string());
    }
    let mut fusion = vec![
        "stereo_fusion".to_string(),
        flag("workspace_path"),
        dense_s.clone(),
        flag("workspace_format"),
        "COLMAP".into(),
        flag("input_type"),
        "geometric".into(),
        flag("output_path"),
        dense.join("fused.ply").to_string_lossy().into_owned(),
    ];
    if d.min_num_pixels > 0 {
        fusion.push(flag("StereoFusion.min_num_pixels"));
        fusion.push(d.min_num_pixels.to_string());
    }
    vec![
        vec![
            "image_undistorter".to_string(),
            flag("image_path"),
            images.to_string_lossy().into_owned(),
            flag("input_path"),
            model.to_string_lossy().into_owned(),
            flag("output_path"),
            dense_s,
            flag("output_type"),
            "COLMAP".into(),
        ],
        stereo,
        fusion,
    ]
}

/// Number of image pairs an exhaustive matcher compares.
pub fn pair_count(n: u64) -> u64 {
    n * n.saturating_sub(1) / 2
}

/// Count image files below `dir`.
pub fn count_images(dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| {
            let p = e.path();
            if p.is_dir() {
                count_images(&p)
            } else {
                let ext = p.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
                usize::from(IMAGE_EXTENSIONS.contains(&ext.as_str()))
            }
        })
        .sum()
}

// ---------------------------------------------------------------------------
// Executable lookup and the runner
// ---------------------------------------------------------------------------

fn path_candidates(name: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![format!("{name}.exe"), format!("{name}.bat"), format!("{name}.cmd"), name.to_string()]
    } else {
        vec![name.to_string()]
    }
}

/// Search `PATH` for a program.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for cand in path_candidates(name) {
            let p = dir.join(&cand);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// The executable: an explicit path or name, then the environment variable,
/// then `colmap` on the path.
pub fn find_executable(explicit: &str) -> Option<PathBuf> {
    let explicit = explicit.trim();
    if !explicit.is_empty() {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Some(p);
        }
        if !explicit.contains(['/', '\\']) {
            return which(explicit);
        }
        return None;
    }
    if let Ok(v) = std::env::var(ENV_EXE) {
        let p = PathBuf::from(v.trim());
        if p.is_file() {
            return Some(p);
        }
    }
    which("colmap")
}

/// Help text of a sub-command (`exe sub -h`), both pipes joined.
pub fn help_text(exe: &Path, sub: &str) -> Option<String> {
    let out = Command::new(exe)
        .args([sub, "-h"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(s)
}

/// Outcome of one run.
#[derive(Debug, Clone)]
pub struct RunOutput {
    pub success: bool,
    pub code: Option<i32>,
    /// Last lines of the combined output.
    pub tail: String,
}

struct KillOnDrop(Option<Child>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(c) = self.0.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn read_lines(r: impl std::io::Read + Send + 'static) -> std::thread::JoinHandle<Vec<String>> {
    std::thread::spawn(move || {
        let mut rd = BufReader::new(r);
        let mut out = Vec::new();
        let mut buf = Vec::new();
        while let Ok(n) = rd.read_until(b'\n', &mut buf) {
            if n == 0 {
                break;
            }
            out.push(String::from_utf8_lossy(&buf).trim_end().to_string());
            buf.clear();
        }
        out
    })
}

/// Run a program, reading stdout and stderr on separate threads and
/// appending a transcript to `log`.
pub fn run_tool(exe: &Path, args: &[String], cwd: &Path, log: &Path) -> Result<RunOutput, String> {
    let child = Command::new(exe)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
    let mut guard = KillOnDrop(Some(child));
    let child = guard.0.as_mut().ok_or("child vanished")?;
    let out_t = child.stdout.take().map(read_lines);
    let err_t = child.stderr.take().map(read_lines);
    let status = child.wait().map_err(|e| format!("waiting for {}: {e}", exe.display()))?;
    guard.0 = None;
    let stdout = out_t.map(|t| t.join().unwrap_or_default()).unwrap_or_default();
    let stderr = err_t.map(|t| t.join().unwrap_or_default()).unwrap_or_default();
    let mut text = format!("$ {} {}\n", exe.display(), args.join(" "));
    text.push_str("--- stdout ---\n");
    text.push_str(&stdout.join("\n"));
    text.push_str("\n--- stderr ---\n");
    text.push_str(&stderr.join("\n"));
    text.push_str(&format!("\n--- exit: {:?} ---\n\n", status.code()));
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
        let _ = f.write_all(text.as_bytes());
    }
    let tail: Vec<&String> = stderr.iter().chain(stdout.iter()).filter(|l| !l.is_empty()).collect();
    let skip = tail.len().saturating_sub(12);
    Ok(RunOutput {
        success: status.success(),
        code: status.code(),
        tail: tail[skip..].iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n"),
    })
}

/// One planned command.
#[derive(Debug, Clone)]
pub struct Step {
    pub args: Vec<String>,
}

fn quote(a: &str) -> String {
    if a.contains(' ') {
        format!("\"{a}\"")
    } else {
        a.to_string()
    }
}

/// Write the planned commands, one per line, so they can be run by hand.
pub fn write_commands(path: &Path, exe_name: &str, steps: &[Step]) -> std::io::Result<()> {
    let text: String = steps
        .iter()
        .map(|s| format!("{exe_name} {}\n", s.args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")))
        .collect();
    std::fs::write(path, text)
}

/// Run the steps in order, stopping at the first failure.
pub fn run_steps(exe: &Path, steps: &[Step], cwd: &Path, log: &Path) -> Result<(), String> {
    for s in steps {
        let name = s.args.first().cloned().unwrap_or_default();
        let out = run_tool(exe, &s.args, cwd, log)?;
        if !out.success {
            return Err(format!(
                "'{name}' failed (exit {:?}); see {}. Last output:\n{}",
                out.code,
                log.display(),
                out.tail
            ));
        }
    }
    Ok(())
}

/// Whether an NVIDIA driver tool is on the path (a CUDA hint, not a proof).
pub fn cuda_hint() -> bool {
    which("nvidia-smi").is_some()
}

// ---------------------------------------------------------------------------
// Node plumbing
// ---------------------------------------------------------------------------

fn tool_params() -> Vec<ParamSpec> {
    vec![
        ParamSpec::bool("run", "Run", false).help("Start the program. Off by default: the commands are only written to commands.txt."),
        ParamSpec::path("executable", "Executable", &[], false).help("Path to the photogrammetry program. Empty searches the VERNON_COLMAP variable and the system path."),
        ParamSpec::bool("overwrite", "Overwrite database", false).help("Remove this node's own database file before running. Nothing else is ever deleted."),
    ]
}

fn image_dir_param() -> ParamSpec {
    ParamSpec::string("image_dir", "Image folder", "").help("Folder with the photos, searched recursively.")
}

struct Plan<'a> {
    ctx: &'a CookContext,
    run: bool,
    exe: Option<PathBuf>,
    exe_arg: String,
}

impl<'a> Plan<'a> {
    fn new(ctx: &'a CookContext, params: &Params) -> CookResult<Self> {
        std::fs::create_dir_all(&ctx.work_dir)?;
        let exe_arg = params.str("executable")?.to_string();
        Ok(Plan {
            ctx,
            run: params.bool("run")?,
            exe: find_executable(&exe_arg),
            exe_arg,
        })
    }

    fn log(&self) -> PathBuf {
        self.ctx.work_dir.join("colmap.log")
    }

    /// Write commands.txt; run the steps when asked and possible. Returns
    /// whether the steps ran.
    fn execute(&self, rep: &mut Report, steps: &[Step], needs_cuda: bool) -> CookResult<bool> {
        write_commands(&self.ctx.work_dir.join("commands.txt"), "colmap", steps)?;
        if !self.run {
            rep.note(
                Severity::Warning,
                "run is off: nothing was started; the planned commands are in commands.txt in the node's work folder",
            );
            return Ok(false);
        }
        let Some(exe) = &self.exe else {
            rep.note(
                Severity::Warning,
                format!(
                    "photogrammetry program not found (executable '{}', {ENV_EXE}, system path); nothing was run, commands are in commands.txt",
                    self.exe_arg
                ),
            );
            return Ok(false);
        };
        if needs_cuda && !cuda_hint() {
            rep.note(
                Severity::Warning,
                "dense stereo needs a CUDA graphics card and no NVIDIA driver tools were found; nothing was run, commands are in commands.txt",
            );
            return Ok(false);
        }
        run_steps(exe, steps, &self.ctx.work_dir, &self.log()).map_err(|m| self.ctx.fail(m))?;
        rep.note(Severity::Info, format!("ran {} command(s); transcript in {}", steps.len(), self.log().display()));
        Ok(true)
    }
}

fn check_image_dir(ctx: &CookContext, dir: &str, run: bool) -> CookResult<(PathBuf, usize)> {
    let p = PathBuf::from(dir);
    let n = count_images(&p);
    if run && (dir.is_empty() || !p.is_dir()) {
        return Err(ctx.fail(format!("image folder '{dir}' does not exist")));
    }
    if run && n == 0 {
        return Err(ctx.fail(format!("no images found below '{dir}'")));
    }
    Ok((p, n))
}

/// Feature extraction and matching.
pub struct SfmFeaturesMatch;

impl NodeOp for SfmFeaturesMatch {
    fn info(&self) -> NodeTypeInfo {
        let mut info = base("capture.sfm_features_match", "Feature Match")
            .output(PortSpec::new("database", DataKind::File))
            .output(PortSpec::new("report", DataKind::Report))
            .param(image_dir_param())
            .param(
                ParamSpec::choice(
                    "camera_model",
                    "Camera model",
                    "SIMPLE_RADIAL",
                    &["SIMPLE_RADIAL", "PINHOLE", "SIMPLE_PINHOLE", "RADIAL", "OPENCV"],
                )
                .help("Use the simplest model that keeps the reprojection error low."),
            )
            .param(ParamSpec::bool("single_camera", "Single camera", false).help("Share one set of intrinsics between all photos; only when every photo used the same settings."))
            .param(ParamSpec::int("max_image_size", "Maximum image size", 3200, 256, 20000).unit("px"))
            .param(ParamSpec::int("max_features", "Maximum features", 8192, 256, 100_000))
            .param(ParamSpec::choice("feature_type", "Feature type", "SIFT", &["SIFT", "ALIKED_N16ROT", "ALIKED_N32", "LOMA_B"]))
            .param(
                ParamSpec::choice("matcher", "Matcher", "exhaustive", &["exhaustive", "sequential", "vocab_tree", "spatial"])
                    .help("Exhaustive compares every pair and is only sensible for a few hundred photos."),
            )
            .param(ParamSpec::int("overlap", "Sequential overlap", 10, 1, 500).help("Neighbouring photos compared by the sequential matcher."))
            .param(ParamSpec::int("neighbors", "Spatial neighbours", 50, 1, 1000))
            .param(ParamSpec::string("vocab_tree", "Vocabulary tree", "").help("Path of the vocabulary tree file for the vocab_tree matcher."))
            .param(ParamSpec::bool("guided", "Guided matching", false).help("More matches on difficult surfaces, slower."))
            .param(ParamSpec::bool("use_gpu", "Use GPU", true));
        for p in tool_params() {
            info = info.param(p);
        }
        info.help("Extracts features from the photos and matches them, by driving an external open-source photogrammetry program. Writes the commands it would run, warns when exhaustive matching would be too slow, and runs them only when asked.")
            .emulates("structure-from-motion feature extraction and matching stage")
            .external("COLMAP")
    }

    fn cook(&self, ctx: &CookContext, _inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let plan = Plan::new(ctx, params)?;
        let (images, n) = check_image_dir(ctx, params.str("image_dir")?, plan.run)?;
        let db = ctx.work_dir.join("database.db");
        let ext = ExtractorSettings {
            camera_model: params.str("camera_model")?.to_string(),
            single_camera: params.bool("single_camera")?,
            max_image_size: params.int("max_image_size")?,
            max_features: params.int("max_features")?,
            use_gpu: params.bool("use_gpu")?,
            feature_type: params.str("feature_type")?.to_string(),
        };
        let mat = MatcherSettings {
            kind: params.str("matcher")?.to_string(),
            guided: params.bool("guided")?,
            use_gpu: ext.use_gpu,
            overlap: params.int("overlap")?,
            neighbors: params.int("neighbors")?,
            vocab_tree: params.str("vocab_tree")?.to_string(),
        };
        let (h_ext, h_mat) = match (&plan.exe, plan.run) {
            (Some(e), true) => (help_text(e, "feature_extractor"), help_text(e, &format!("{}_matcher", mat.kind))),
            _ => (None, None),
        };
        let steps = vec![
            Step { args: extractor_args(&db, &images, &ext, h_ext.as_deref()) },
            Step { args: matcher_args(&db, &mat, h_mat.as_deref()).map_err(|m| ctx.fail(m))? },
        ];
        let mut rep = Report::new("Feature Match");
        let pairs = match mat.kind.as_str() {
            "exhaustive" => pair_count(n as u64),
            "sequential" => n as u64 * mat.overlap.max(0) as u64,
            _ => 0,
        };
        rep.metric("images", Metric::new(n as f64, "images"));
        rep.metric("image_pairs", Metric::new(pairs as f64, "pairs"));
        rep.metric("eta_sparse", Metric::new(n as f64 * SPARSE_SECONDS_PER_IMAGE / 3600.0, "h"));
        if mat.kind == "exhaustive" && pairs > EXHAUSTIVE_PAIR_WARNING {
            rep.note(
                Severity::Warning,
                format!("exhaustive matching of {n} images compares {pairs} pairs; use the sequential or vocab_tree matcher for a set this large"),
            );
        }
        if mat.kind == "vocab_tree" && mat.vocab_tree.is_empty() {
            rep.note(Severity::Warning, "no vocabulary tree file given; the program will try to download one");
        }
        rep.note(
            Severity::Info,
            "run-time estimate scales a published 1,500-photo project (13 h for the sparse model) linearly; treat it as a planning figure",
        );
        if !ext.use_gpu {
            rep.note(Severity::Info, "CPU-only extraction and matching: expect the full estimate or longer");
        }
        if db.exists() {
            if params.bool("overwrite")? && plan.run {
                std::fs::remove_file(&db)?;
                rep.note(Severity::Info, "removed the existing database before running (overwrite is on)");
            } else {
                rep.note(Severity::Info, "an existing database was kept; the program resumes from it");
            }
        }
        plan.execute(&mut rep, &steps, false)?;
        Ok(vec![Data::File(db), Data::report(rep)])
    }
}

fn empty_cloud() -> Geometry {
    let mut g = Geometry::new();
    set_metric_flag(&mut g, false);
    g
}

/// Sparse reconstruction.
pub struct SfmReconstruct;

impl NodeOp for SfmReconstruct {
    fn info(&self) -> NodeTypeInfo {
        let mut info = base("capture.sfm_reconstruct", "Sparse Reconstruct")
            .input(PortSpec::optional("database", DataKind::File))
            .output(PortSpec::new("model", DataKind::File))
            .output(PortSpec::new("cloud", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(image_dir_param())
            .param(ParamSpec::choice("mapper", "Mapper", "incremental", &["incremental", "global", "hierarchical"]).help("Incremental is the careful default; global and hierarchical suit very large sets."))
            .param(ParamSpec::bool("refine_focal", "Refine focal length", true))
            .param(ParamSpec::bool("refine_principal", "Refine principal point", false))
            .param(ParamSpec::int("init_image_1", "Initial image A", -1, -1, 1_000_000).help("Image id to start from; -1 lets the program choose."))
            .param(ParamSpec::int("init_image_2", "Initial image B", -1, -1, 1_000_000));
        for p in tool_params() {
            info = info.param(p);
        }
        info.help("Builds the sparse camera poses and 3D points from matched photos by driving an external open-source photogrammetry program, then reads the points into a cloud. The cloud has an arbitrary scale and is marked non-metric until a scale node has run.")
            .emulates("incremental structure-from-motion sparse reconstruction")
            .external("COLMAP")
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let plan = Plan::new(ctx, params)?;
        let (images, n) = check_image_dir(ctx, params.str("image_dir")?, plan.run)?;
        let db = match inputs.first().and_then(|d| d.as_ref()).and_then(|d| d.as_file()) {
            Some(p) => p.clone(),
            None => ctx.work_dir.join("database.db"),
        };
        let sparse = ctx.work_dir.join("sparse");
        let model = sparse.join("0");
        let (i1, i2) = (params.int("init_image_1")?, params.int("init_image_2")?);
        let settings = MapperSettings {
            kind: params.str("mapper")?.to_string(),
            refine_focal: params.bool("refine_focal")?,
            refine_principal: params.bool("refine_principal")?,
            init_pair: (i1 >= 0 && i2 >= 0).then_some((i1, i2)),
        };
        if (i1 >= 0) != (i2 >= 0) {
            return Err(ctx.fail("set both initial images or neither"));
        }
        std::fs::create_dir_all(&sparse)?;
        let steps = vec![
            Step { args: mapper_args(&db, &images, &sparse, &settings).map_err(|m| ctx.fail(m))? },
            Step { args: model_to_text_args(&model) },
        ];
        let mut rep = Report::new("Sparse Reconstruct");
        rep.metric("images", Metric::new(n as f64, "images"));
        rep.metric("eta_sparse", Metric::new(n as f64 * SPARSE_SECONDS_PER_IMAGE / 3600.0, "h"));
        plan.execute(&mut rep, &steps, false)?;
        let cloud = match read_model_dir(&model, false) {
            Ok(m) if !m.points.is_empty() => {
                let mean_err = m.points.iter().map(|p| p.error).sum::<f64>() / m.points.len() as f64;
                rep.metric("sparse_points", Metric::new(m.points.len() as f64, "points"));
                rep.metric("mean_reprojection_error", Metric::new(mean_err, "px"));
                rep.note(Severity::Info, "scale is arbitrary: the cloud is marked non-metric until Solve Scale Bars or Scale From Distance has run");
                points_to_geometry(&m.points)
            }
            Ok(_) => {
                rep.metric("sparse_points", Metric::new(0.0, "points"));
                rep.note(Severity::Warning, "no sparse model found yet: the cloud is empty");
                empty_cloud()
            }
            Err(m) => {
                rep.note(Severity::Error, format!("could not read the sparse model: {m}"));
                empty_cloud()
            }
        };
        Ok(vec![Data::File(model), Data::geometry(cloud), Data::report(rep)])
    }
}

/// Dense reconstruction.
pub struct MvsDense;

impl NodeOp for MvsDense {
    fn info(&self) -> NodeTypeInfo {
        let mut info = base("capture.mvs_dense", "Dense Reconstruct")
            .input(PortSpec::new("model", DataKind::File))
            .output(PortSpec::new("cloud", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(image_dir_param())
            .param(ParamSpec::int("max_image_size", "Maximum image size", 0, 0, 20000).unit("px").help("0 keeps the program's default."))
            .param(ParamSpec::bool("geom_consistency", "Geometric consistency", true).help("Better depth maps, more graphics memory."))
            .param(ParamSpec::int("min_num_pixels", "Minimum pixels", 0, 0, 100).help("Raise to cut noise; 0 keeps the program's default."))
            .param(ParamSpec::int("window_radius", "Window radius", 0, 0, 50).help("Larger windows help on textureless surfaces; 0 keeps the program's default."));
        for p in tool_params() {
            info = info.param(p);
        }
        info.help("Turns a sparse model into a dense point cloud (undistort, depth maps, fusion) by driving an external open-source photogrammetry program. Needs a CUDA graphics card for the depth-map step.")
            .emulates("multi-view stereo dense reconstruction")
            .external("COLMAP")
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let plan = Plan::new(ctx, params)?;
        let model = inputs
            .first()
            .and_then(|d| d.as_ref())
            .and_then(|d| d.as_file())
            .cloned()
            .ok_or_else(|| ctx.fail("connect the sparse model folder"))?;
        let (images, n) = check_image_dir(ctx, params.str("image_dir")?, plan.run)?;
        let dense = ctx.work_dir.join("dense");
        let d = DenseSettings {
            max_image_size: params.int("max_image_size")?,
            geom_consistency: params.bool("geom_consistency")?,
            min_num_pixels: params.int("min_num_pixels")?,
            window_radius: params.int("window_radius")?,
        };
        let steps: Vec<Step> = dense_steps(&images, &model, &dense, &d).into_iter().map(|args| Step { args }).collect();
        let mut rep = Report::new("Dense Reconstruct");
        rep.metric("images", Metric::new(n as f64, "images"));
        rep.metric("eta_dense", Metric::new(n as f64 * DENSE_SECONDS_PER_IMAGE / 3600.0, "h"));
        plan.execute(&mut rep, &steps, true)?;
        let fused = dense.join("fused.ply");
        let cloud = if fused.is_file() {
            let bytes = std::fs::read(&fused)?;
            let mut g = vernon_io::read_bytes(Format::Ply, &bytes, &ReadOptions::default())
                .map_err(|e| ctx.fail(format!("cannot read {}: {e}", fused.display())))?;
            set_metric_flag(&mut g, false);
            rep.metric("dense_points", Metric::new(g.point_count() as f64, "points"));
            g
        } else {
            rep.metric("dense_points", Metric::new(0.0, "points"));
            rep.note(Severity::Warning, "no dense cloud found yet: the cloud is empty");
            empty_cloud()
        };
        Ok(vec![Data::geometry(cloud), Data::report(rep)])
    }
}

/// Grid size for the height-field placeholder: twice the median spacing.
fn auto_cell(points: &[DVec3]) -> f64 {
    let diag = {
        let mut b = vernon_core::Aabb::empty();
        points.iter().for_each(|p| b.grow(*p));
        b.size().length()
    };
    (diag / (points.len().max(1) as f64).sqrt()).max(1e-6) * 1.5
}

/// Height-field triangulation over the best-fit plane: one averaged vertex
/// per occupied cell, two triangles for every fully occupied 2 x 2 block.
/// Only meaningful for panel-like clouds seen from one side.
pub fn heightfield_mesh(points: &[DVec3], cell: f64) -> Geometry {
    use std::collections::HashMap;
    let (c, frame) = principal_frame(points);
    let (u, v, w) = (frame.x_axis, frame.y_axis, frame.z_axis);
    let cell = if cell > 0.0 { cell } else { auto_cell(points) };
    let mut cells: HashMap<(i64, i64), (DVec3, u32)> = HashMap::new();
    for p in points {
        let d = *p - c;
        let key = ((d.dot(u) / cell).floor() as i64, (d.dot(v) / cell).floor() as i64);
        let e = cells.entry(key).or_insert((DVec3::ZERO, 0));
        e.0 += *p;
        e.1 += 1;
    }
    let mut keys: Vec<(i64, i64)> = cells.keys().copied().collect();
    keys.sort();
    let index: HashMap<(i64, i64), u32> = keys.iter().enumerate().map(|(i, k)| (*k, i as u32)).collect();
    let pts: Vec<DVec3> = keys.iter().map(|k| cells[k].0 / cells[k].1 as f64).collect();
    let mut faces = Vec::new();
    for &(i, j) in &keys {
        if let (Some(&a), Some(&b), Some(&c2), Some(&d)) =
            (index.get(&(i, j)), index.get(&(i + 1, j)), index.get(&(i + 1, j + 1)), index.get(&(i, j + 1)))
        {
            faces.push(vec![a, b, c2]);
            faces.push(vec![a, c2, d]);
        }
    }
    let mut g = Geometry::from_polygons(pts, faces);
    // Face normals follow +w (u x v = w), so the winding above is consistent.
    let _ = w;
    g.compute_normals();
    g
}

/// Meshing of a dense cloud.
pub struct MvsMesh;

impl NodeOp for MvsMesh {
    fn info(&self) -> NodeTypeInfo {
        let mut info = base("capture.mvs_mesh", "Mesh Dense Cloud")
            .input(PortSpec::new("cloud", DataKind::Geometry))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("mesher", "Mesher", "poisson", &["poisson", "delaunay"]).help("Poisson needs point normals; Delaunay needs the dense workspace folder."))
            .param(ParamSpec::string("dense_workspace", "Dense workspace", "").help("Folder of the dense reconstruction, used by the Delaunay mesher."))
            .param(ParamSpec::float("target_face_ratio", "Target face ratio", 0.1, 0.01, 1.0).help("Share of faces kept by the simplifier; 1 skips simplification."))
            .param(ParamSpec::float("boundary_weight", "Boundary weight", 1000.0, 0.0, 100_000.0))
            .param(ParamSpec::float("cell", "Placeholder cell", 0.0, 0.0, 1000.0).unit("mm").help("Grid size of the height-field stand-in used when the program is not run; 0 chooses it from the point count."));
        for p in tool_params() {
            info = info.param(p);
        }
        info.help("Turns a dense point cloud into a triangle mesh by driving an external open-source photogrammetry program. When the program is not run, a simple height-field triangulation stands in so the pipeline keeps flowing; it only suits panel-like clouds seen from one side.")
            .emulates("Poisson and Delaunay surface reconstruction with mesh simplification")
            .external("COLMAP")
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let cloud = inputs::geometry(inputs, 0)?;
        if cloud.points.len() < 4 {
            return Err(ctx.fail("the cloud has too few points to mesh"));
        }
        let plan = Plan::new(ctx, params)?;
        let mut rep = Report::new("Mesh Dense Cloud");
        rep.metric("input_points", Metric::new(cloud.point_count() as f64, "points"));
        let ratio = params.float("target_face_ratio")?;
        let mesher = params.str("mesher")?.to_string();
        let input_ply = ctx.work_dir.join("cloud.ply");
        let meshed = ctx.work_dir.join("meshed.ply");
        let simplified = ctx.work_dir.join("meshed_simplified.ply");
        let mut steps = Vec::new();
        let workspace = params.str("dense_workspace")?;
        let input_arg = if mesher == "delaunay" && !workspace.is_empty() {
            PathBuf::from(workspace)
        } else {
            if mesher == "delaunay" {
                rep.note(Severity::Warning, "the Delaunay mesher needs a dense workspace folder; using the Poisson mesher instead");
            }
            input_ply.clone()
        };
        let use_delaunay = mesher == "delaunay" && !workspace.is_empty();
        steps.push(Step {
            args: vec![
                (if use_delaunay { "delaunay_mesher" } else { "poisson_mesher" }).to_string(),
                flag("input_path"),
                input_arg.to_string_lossy().into_owned(),
                flag("output_path"),
                meshed.to_string_lossy().into_owned(),
            ],
        });
        if ratio < 1.0 {
            steps.push(Step {
                args: vec![
                    "mesh_simplifier".to_string(),
                    flag("input_path"),
                    meshed.to_string_lossy().into_owned(),
                    flag("output_path"),
                    simplified.to_string_lossy().into_owned(),
                    flag("MeshSimplification.target_face_ratio"),
                    ratio.to_string(),
                    flag("MeshSimplification.boundary_weight"),
                    params.float("boundary_weight")?.to_string(),
                ],
            });
        }
        if plan.run && plan.exe.is_some() {
            let bytes = vernon_io::write_bytes(Format::Ply, &cloud, &WriteOptions::default()).map_err(|e| ctx.fail(e.to_string()))?;
            std::fs::write(&input_ply, bytes)?;
            if !util::metric_flag(&cloud).unwrap_or(false) {
                rep.note(Severity::Warning, "the cloud is not marked metric; the mesh inherits the arbitrary scale");
            }
        }
        let ran = plan.execute(&mut rep, &steps, false)?;
        let result_path = if ratio < 1.0 { &simplified } else { &meshed };
        let mut mesh = if ran && result_path.is_file() {
            let bytes = std::fs::read(result_path)?;
            vernon_io::read_bytes(Format::Ply, &bytes, &ReadOptions::default())
                .map_err(|e| ctx.fail(format!("cannot read {}: {e}", result_path.display())))?
        } else {
            util::placeholder(
                &mut rep,
                "a height-field triangulation over the best-fit plane stands in for the Poisson or Delaunay surface; the real mesher builds a closed surface from the oriented points",
            );
            heightfield_mesh(&cloud.points, params.float("cell")?)
        };
        if let Some(flagv) = util::metric_flag(&cloud) {
            set_metric_flag(&mut mesh, flagv);
        }
        rep.metric("faces", Metric::new(mesh.prim_count() as f64, "faces"));
        rep.metric("vertices", Metric::new(mesh.point_count() as f64, "points"));
        Ok(vec![Data::geometry(mesh), Data::report(rep)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vernon_graph::ParamValue;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vernon-capture-colmap-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn ctx(dir: &Path) -> CookContext {
        CookContext {
            node_name: "t".into(),
            work_dir: dir.to_path_buf(),
            frame: 0.0,
        }
    }

    fn defaults(op: &dyn NodeOp) -> Params {
        Params::from_specs(&op.info().params)
    }

    #[test]
    fn spec_golden_20_extractor_argv_snapshot() {
        let a = extractor_args(Path::new("db.db"), Path::new("imgs"), &ExtractorSettings::default(), None);
        let joined = a.join(" ");
        assert!(
            joined.contains("--ImageReader.camera_model SIMPLE_RADIAL --ImageReader.single_camera 0 --FeatureExtraction.max_image_size 3200 --SiftExtraction.max_num_features 8192"),
            "{joined}"
        );
        assert!(joined.starts_with("feature_extractor --database_path db.db --image_path imgs"));
        // A help text that only knows the old option name switches the spelling.
        let old = "Options:\n  --SiftExtraction.max_image_size arg (=3200)\n  --SiftExtraction.max_num_features arg\n";
        let b = extractor_args(Path::new("db.db"), Path::new("imgs"), &ExtractorSettings::default(), Some(old)).join(" ");
        assert!(b.contains("--SiftExtraction.max_image_size 3200"), "{b}");
        assert!(!b.contains("FeatureExtraction.max_image_size"));
        // The new name wins when the help lists it.
        let new = "  --FeatureExtraction.max_image_size arg (=3200)\n";
        assert!(extractor_args(Path::new("d"), Path::new("i"), &ExtractorSettings::default(), Some(new))
            .join(" ")
            .contains("--FeatureExtraction.max_image_size 3200"));
        // CPU-only and a non-default feature type add their options.
        let s = ExtractorSettings {
            use_gpu: false,
            feature_type: "ALIKED_N32".into(),
            ..ExtractorSettings::default()
        };
        let c = extractor_args(Path::new("d"), Path::new("i"), &s, None).join(" ");
        assert!(c.contains("--FeatureExtraction.use_gpu 0") && c.contains("--FeatureExtraction.type ALIKED_N32"));
    }

    #[test]
    fn matcher_mapper_and_dense_argv() {
        let m = MatcherSettings {
            kind: "sequential".into(),
            guided: true,
            use_gpu: true,
            overlap: 7,
            neighbors: 50,
            vocab_tree: String::new(),
        };
        let a = matcher_args(Path::new("db"), &m, None).unwrap().join(" ");
        assert_eq!(a, "sequential_matcher --database_path db --SequentialMatching.overlap 7 --FeatureMatching.guided_matching 1");
        assert!(matcher_args(Path::new("db"), &MatcherSettings { kind: "x".into(), ..m.clone() }, None).is_err());
        let mp = MapperSettings {
            kind: "incremental".into(),
            refine_focal: false,
            refine_principal: true,
            init_pair: Some((3, 9)),
        };
        let a = mapper_args(Path::new("db"), Path::new("im"), Path::new("sp"), &mp).unwrap().join(" ");
        assert!(a.starts_with("mapper --database_path db --image_path im --output_path sp"));
        assert!(a.contains("--Mapper.ba_refine_focal_length 0") && a.contains("--Mapper.ba_refine_principal_point 1"));
        assert!(a.contains("--Mapper.init_image_id1 3 --Mapper.init_image_id2 9"));
        let g = mapper_args(Path::new("db"), Path::new("im"), Path::new("sp"), &MapperSettings { kind: "global".into(), ..mp }).unwrap();
        assert_eq!(g[0], "global_mapper");
        // The dense chain runs in order: undistort, depth maps, fusion.
        let steps = dense_steps(
            Path::new("im"),
            Path::new("sparse/0"),
            Path::new("dense"),
            &DenseSettings {
                max_image_size: 1600,
                geom_consistency: true,
                min_num_pixels: 3,
                window_radius: 0,
            },
        );
        let names: Vec<&str> = steps.iter().map(|s| s[0].as_str()).collect();
        assert_eq!(names, ["image_undistorter", "patch_match_stereo", "stereo_fusion"]);
        assert!(steps[1].join(" ").contains("--PatchMatchStereo.max_image_size 1600"));
        assert!(steps[2].join(" ").contains("--StereoFusion.min_num_pixels 3"));
    }

    #[test]
    fn pair_count_and_eta_for_the_report_case() {
        assert_eq!(pair_count(1500), 1_124_250);
        assert_eq!(pair_count(0), 0);
        assert!(pair_count(1500) > EXHAUSTIVE_PAIR_WARNING);
        let hours = 1500.0 * SPARSE_SECONDS_PER_IMAGE / 3600.0;
        assert!((hours - 13.0).abs() < 1e-9);
        assert!(hours >= 10.0, "the estimate must not promise less than 10 h for 1,500 photos");
    }

    #[test]
    fn features_match_node_warns_and_writes_commands_without_the_program() {
        let work = scratch("fm");
        let imgs = scratch("fm-imgs");
        for i in 0..3 {
            std::fs::write(imgs.join(format!("p{i}.jpg")), b"x").unwrap();
        }
        let op = SfmFeaturesMatch;
        let mut p = defaults(&op);
        p.set("image_dir", ParamValue::Str(imgs.to_string_lossy().into_owned()));
        let out = op.cook(&ctx(&work), &[], &p).unwrap();
        assert_eq!(out.len(), 2);
        let rep = out[1].as_report().unwrap();
        assert_eq!(rep.metrics["images"].value, 3.0);
        assert_eq!(rep.metrics["image_pairs"].value, 3.0);
        assert!(rep.notes.iter().any(|(s, m)| *s == Severity::Warning && m.contains("run is off")));
        let cmds = std::fs::read_to_string(work.join("commands.txt")).unwrap();
        assert!(cmds.contains("feature_extractor") && cmds.contains("exhaustive_matcher"));
        // Many images with the exhaustive matcher produce the warning.
        let big = scratch("fm-big");
        for i in 0..520 {
            std::fs::write(big.join(format!("p{i}.jpg")), b"x").unwrap();
        }
        p.set("image_dir", ParamValue::Str(big.to_string_lossy().into_owned()));
        let out = op.cook(&ctx(&scratch("fm2")), &[], &p).unwrap();
        assert!(out[1].as_report().unwrap().notes.iter().any(|(s, m)| *s == Severity::Warning && m.contains("exhaustive matching")));
        // run = true with a missing program is a warning, not a crash.
        p.set("run", ParamValue::Bool(true));
        p.set("executable", ParamValue::Str("definitely-not-installed-colmap".into()));
        let out = op.cook(&ctx(&scratch("fm3")), &[], &p).unwrap();
        assert!(out[1].as_report().unwrap().notes.iter().any(|(_, m)| m.contains("not found")));
        // run = true with a missing image folder is an error.
        p.set("image_dir", ParamValue::Str("no/such/folder".into()));
        assert!(op.cook(&ctx(&scratch("fm4")), &[], &p).is_err());
    }

    #[test]
    fn runner_streams_logs_and_reports_failures() {
        let work = scratch("runner");
        let exe = std::env::current_exe().unwrap();
        let log = work.join("run.log");
        let ok = run_tool(&exe, &["--list".to_string()], &work, &log).unwrap();
        assert!(ok.success, "{ok:?}");
        let bad = run_tool(&exe, &["--definitely-not-a-flag".to_string()], &work, &log).unwrap();
        assert!(!bad.success);
        assert!(!bad.tail.is_empty());
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("--- stdout ---") && text.contains("--- stderr ---"));
        let steps = [
            Step { args: vec!["--list".into()] },
            Step { args: vec!["--definitely-not-a-flag".into()] },
        ];
        let err = run_steps(&exe, &steps, &work, &log).unwrap_err();
        assert!(err.contains("definitely-not-a-flag") && err.contains("failed"));
        assert!(run_tool(Path::new("no-such-program-xyz"), &[], &work, &log).is_err());
        // Option resolution against a real help text.
        let help = help_text(&exe, "--help").unwrap();
        assert_eq!(resolve_option(Some(&help), &["--no-such-option", "--test-threads"]), "--test-threads");
        assert_eq!(resolve_option(None, &["a", "b"]), "a");
    }

    #[test]
    fn overwrite_only_touches_the_database_file() {
        let work = scratch("overwrite");
        let imgs = scratch("overwrite-imgs");
        std::fs::write(imgs.join("a.jpg"), b"x").unwrap();
        std::fs::write(work.join("database.db"), b"precious").unwrap();
        std::fs::write(work.join("notes.txt"), b"keep").unwrap();
        let op = SfmFeaturesMatch;
        let mut p = defaults(&op);
        p.set("image_dir", ParamValue::Str(imgs.to_string_lossy().into_owned()));
        p.set("run", ParamValue::Bool(true));
        // The test harness stands in for the program and fails every step.
        p.set("executable", ParamValue::Str(std::env::current_exe().unwrap().to_string_lossy().into_owned()));
        assert!(op.cook(&ctx(&work), &[], &p).is_err());
        assert_eq!(std::fs::read(work.join("database.db")).unwrap(), b"precious", "overwrite=false keeps the database");
        p.set("overwrite", ParamValue::Bool(true));
        assert!(op.cook(&ctx(&work), &[], &p).is_err());
        assert!(!work.join("database.db").exists());
        assert_eq!(std::fs::read(work.join("notes.txt")).unwrap(), b"keep");
        assert!(work.join("colmap.log").exists());
    }

    #[test]
    fn reconstruct_reads_an_existing_text_model() {
        let work = scratch("recon");
        let model = work.join("sparse").join("0");
        std::fs::create_dir_all(&model).unwrap();
        std::fs::write(
            model.join("points3D.txt"),
            "# 3D point list\n1 1.0 2.0 3.0 255 0 0 0.5 1 0\n2 4.0 5.0 6.0 0 255 0 1.5 1 1 2 0\n",
        )
        .unwrap();
        let op = SfmReconstruct;
        let out = op.cook(&ctx(&work), &[], &defaults(&op)).unwrap();
        let g = out[1].as_geometry().unwrap();
        assert_eq!(g.point_count(), 2);
        assert_eq!(util::metric_flag(g), Some(false));
        let rep = out[2].as_report().unwrap();
        assert!((rep.metrics["mean_reprojection_error"].value - 1.0).abs() < 1e-12);
        // Nothing reconstructed yet: an empty, non-metric cloud and a warning.
        let out = op.cook(&ctx(&scratch("recon2")), &[], &defaults(&op)).unwrap();
        assert_eq!(out[1].as_geometry().unwrap().point_count(), 0);
        let mut bad = defaults(&op);
        bad.set("init_image_1", ParamValue::Int(3));
        assert!(op.cook(&ctx(&scratch("recon3")), &[], &bad).is_err());
    }

    #[test]
    fn dense_node_needs_a_model_and_returns_an_empty_cloud_without_a_run() {
        let op = MvsDense;
        assert!(op.cook(&ctx(&scratch("dense0")), &[None], &defaults(&op)).is_err());
        let out = op
            .cook(&ctx(&scratch("dense1")), &[Some(Data::File(PathBuf::from("sparse/0")))], &defaults(&op))
            .unwrap();
        assert_eq!(out[0].as_geometry().unwrap().point_count(), 0);
        let cmds = std::fs::read_to_string(scratch("dense1").join("commands.txt")).unwrap();
        let pos = |s: &str| cmds.find(s).unwrap();
        assert!(pos("image_undistorter") < pos("patch_match_stereo") && pos("patch_match_stereo") < pos("stereo_fusion"));
    }

    #[test]
    fn mesh_node_falls_back_to_a_labelled_height_field() {
        let mut pts = Vec::new();
        for i in 0..30 {
            for j in 0..30 {
                pts.push(DVec3::new(i as f64 * 2.0, j as f64 * 2.0, (i as f64 * 0.1).sin() * 3.0));
            }
        }
        let op = MvsMesh;
        let mut cloud = Geometry::from_points(pts);
        set_metric_flag(&mut cloud, true);
        let mut p = defaults(&op);
        p.set("cell", ParamValue::Float(2.0));
        let out = op.cook(&ctx(&scratch("mesh")), &[Some(Data::geometry(cloud))], &p).unwrap();
        let m = out[0].as_geometry().unwrap();
        m.validate().unwrap();
        assert_eq!(m.point_count(), 900);
        assert_eq!(m.prim_count(), 2 * 29 * 29);
        assert_eq!(util::metric_flag(m), Some(true));
        assert!(out[1].as_report().unwrap().notes.iter().any(|(s, t)| *s == Severity::Warning && t.starts_with("white-box placeholder")));
        // Normals point consistently along one side of the panel.
        let nz: Vec<f64> = m.prim_attribs.vec3s("N").unwrap().iter().map(|n| n.z).collect();
        assert!(nz.iter().all(|z| z.abs() > 0.9) && (nz.iter().all(|z| *z > 0.0) || nz.iter().all(|z| *z < 0.0)));
        assert!(op.cook(&ctx(&scratch("mesh2")), &[Some(Data::geometry(Geometry::from_points(vec![DVec3::ZERO])))], &p).is_err());
    }

    #[test]
    fn executable_lookup_prefers_an_explicit_existing_file() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(find_executable(exe.to_str().unwrap()), Some(exe));
        assert_eq!(find_executable("no/such/dir/colmap"), None);
        assert!(count_images(Path::new("no/such/dir")) == 0);
    }
}

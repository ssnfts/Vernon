//! Opt-in public-sample acceptance through the real capture nodes.
//!
//! Set VERNON_COLMAP_DATA to prepared photos (or a dataset root containing
//! `images/`) and VERNON_PHOTOGRAMMETRY_EXE to the external executable.
//! `VERNON_COLMAP_DATA=download:south-building` explicitly downloads the
//! official archive to a fresh temporary directory and extracts photos only.
//! Both tests stay ignored during normal offline workspace verification.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use vernon_core::process::{self, RunOptions};
use vernon_core::{Geometry, Report, Severity};
use vernon_graph::{CookContext, Data, NodeOp, ParamValue, Params};
use vernon_nodes_capture::colmap::{find_executable, run_tool, MvsDense, MvsMesh, SfmFeaturesMatch, SfmReconstruct, IMAGE_EXTENSIONS};
use vernon_nodes_capture::colmap_model::read_model_dir;
use vernon_nodes_capture::scale::ScaleFromDistance;
use vernon_nodes_capture::util::metric_flag;

const SAMPLE_URL: &str = "https://github.com/colmap/colmap/releases/download/3.11.1/south-building.zip";
const SAMPLE_BYTES: u64 = 419_421_847;
const IMAGE_LIMIT: usize = 20;

struct Fixture {
    root: PathBuf,
    images: PathBuf,
    executable: PathBuf,
}

fn bounded_command(command: &mut Command, work: &Path, name: &str, timeout: Duration) {
    let log = work.join(format!("{name}.log"));
    command.current_dir(work);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let exit = process::run(command, &log, &RunOptions::with_timeout(timeout))
        .unwrap_or_else(|e| panic!("{name} could not start: {e}; use prepared local photos instead; log {}", log.display()));
    assert!(exit.success(), "{name} failed: {exit:?}; log {}", log.display());
}

fn download_sample(root: &Path) -> PathBuf {
    let archive = root.join("south-building.zip");
    let images = root.join("downloaded-images");
    let mut curl = Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
    curl.args(["--fail", "--location", "--proto", "=https", "--connect-timeout", "30", "--max-time", "900", "--max-filesize"])
        .arg(SAMPLE_BYTES.to_string())
        .arg("--output").arg(&archive).arg(SAMPLE_URL);
    bounded_command(&mut curl, root, "dataset-download", Duration::from_secs(920));
    assert_eq!(std::fs::metadata(&archive).unwrap().len(), SAMPLE_BYTES, "unexpected sample archive size");

    // Use the OS's existing ZIP implementation; no archive/model dependency
    // enters Vernon. Only photo files are extracted, with flattened leaf names.
    let script = root.join("extract-photos.ps1");
    std::fs::write(&script, r#"param([string]$ArchivePath, [string]$ImagesPath, [string]$HashPath)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
(Get-FileHash -LiteralPath $ArchivePath -Algorithm SHA256).Hash.ToLowerInvariant() | Set-Content -LiteralPath $HashPath
[System.IO.Directory]::CreateDirectory($ImagesPath) | Out-Null
$zipFile = [System.IO.Compression.ZipFile]::OpenRead($ArchivePath)
try {
    foreach ($entry in $zipFile.Entries) {
        $leaf = [System.IO.Path]::GetFileName($entry.FullName)
        $extension = [System.IO.Path]::GetExtension($leaf).ToLowerInvariant()
        if ($extension -notin @('.jpg', '.jpeg', '.png', '.tif', '.tiff', '.bmp')) { continue }
        $destination = [System.IO.Path]::Combine($ImagesPath, $leaf)
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $destination, $false)
    }
} finally { $zipFile.Dispose() }
"#).unwrap();
    let mut powershell = Command::new(if cfg!(windows) { "powershell.exe" } else { "pwsh" });
    powershell.args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(script).arg("-ArchivePath").arg(archive).arg("-ImagesPath").arg(&images)
        .arg("-HashPath").arg(root.join("archive-sha256.txt"));
    bounded_command(&mut powershell, root, "dataset-extract", Duration::from_secs(300));
    images
}

fn image_paths(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display())) {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            image_paths(&entry.path(), out);
        } else if kind.is_file() && entry.path().extension().and_then(|e| e.to_str())
            .is_some_and(|e| IMAGE_EXTENSIONS.iter().any(|allowed| e.eq_ignore_ascii_case(allowed))) {
            out.push(entry.path());
        }
    }
}

fn prepare_fixture() -> Fixture {
    let data = std::env::var_os("VERNON_COLMAP_DATA")
        .expect("explicit external acceptance requires VERNON_COLMAP_DATA: set it to prepared photos or download:south-building");
    let executable = find_executable("").expect("VERNON_COLMAP_DATA is set: provide VERNON_PHOTOGRAMMETRY_EXE or an executable on PATH");
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let root = std::env::temp_dir().join(format!("vernon-wp8-real-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    eprintln!("WP8 fresh acceptance work={}", root.display());
    let source = if data == "download:south-building" {
        download_sample(&root)
    } else {
        let source = PathBuf::from(data).canonicalize().expect("VERNON_COLMAP_DATA must name an existing photo directory");
        assert!(source.is_dir(), "VERNON_COLMAP_DATA must name prepared photos, not a database/model/archive");
        if source.join("images").is_dir() { source.join("images") } else { source }
    };
    let mut photos = Vec::new();
    image_paths(&source, &mut photos);
    photos.sort();
    assert!(photos.len() >= 12, "public-sample acceptance needs at least 12 sequential photos; found {}", photos.len());
    photos.truncate(IMAGE_LIMIT);
    let images = root.join("images");
    std::fs::create_dir(&images).unwrap();
    let mut selected = Vec::new();
    for photo in photos {
        let relative = photo.strip_prefix(&source).unwrap();
        let destination = images.join(relative);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::copy(&photo, &destination).unwrap();
        selected.push(relative.to_string_lossy().into_owned());
    }
    std::fs::write(root.join("fixture.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "source": source, "official_sample_url": SAMPLE_URL,
        "official_archive_bytes": SAMPLE_BYTES, "selected_images": selected,
        "work": root, "executable": executable,
        "note": "Only photos copied; every database/model/dense/mesh output starts absent. A computed download hash is observational; the release provides no official digest."
    })).unwrap()).unwrap();
    let version_log = root.join("version.log");
    let version = run_tool(&executable, &["-h".into()], &root, &version_log, Duration::from_secs(30))
        .expect("external version/help query runs");
    assert!(version.success, "version/help query failed: {version:?}");
    let version_text = std::fs::read_to_string(version_log).unwrap();
    let version_line = version_text.lines().find(|line| line.contains("COLMAP")).unwrap_or("version text unavailable; inspect version.log");
    eprintln!("WP8 {version_line}; selected_images={}", selected.len());
    Fixture { root, images, executable }
}

fn context(fixture: &Fixture, name: &str) -> CookContext {
    let work_dir = fixture.root.join(name);
    std::fs::create_dir(&work_dir).unwrap();
    CookContext { node_name: name.into(), work_dir, frame: 0.0 }
}

fn run_params(op: &dyn NodeOp, fixture: &Fixture) -> Params {
    let mut params = Params::from_specs(&op.info().params);
    params.set("run", ParamValue::Bool(true));
    params.set("executable", ParamValue::Str(fixture.executable.to_string_lossy().into_owned()));
    params.set("timeout_min", ParamValue::Int(20));
    if op.info().params.iter().any(|p| p.name == "image_dir") {
        params.set("image_dir", ParamValue::Str(fixture.images.to_string_lossy().into_owned()));
    }
    params
}

fn checked_report<'a>(outputs: &'a [Data], ctx: &CookContext, elapsed: Duration, expected_commands: usize) -> &'a Report {
    let report = outputs.iter().find_map(Data::as_report).expect("node emits a Report");
    std::fs::write(ctx.work_dir.join("report.json"), serde_json::to_vec_pretty(report.as_ref()).unwrap()).unwrap();
    eprintln!("WP8 stage={} elapsed_s={:.3} metrics={:?} work={}", ctx.node_name, elapsed.as_secs_f64(), report.metrics, ctx.work_dir.display());
    assert!(report.passes(), "{} failed: {report:#?}; inspect {}", ctx.node_name, ctx.work_dir.display());
    assert!(report.metrics.values().all(|m| m.value.is_finite()), "nonfinite report metric");
    assert!(!report.notes.iter().any(|(severity, text)| *severity == Severity::Error || text.starts_with("white-box placeholder:")), "{report:#?}");
    assert_eq!(report.metrics["external_run_requested"].value, 1.0, "{report:#?}");
    assert_eq!(report.metrics["external_run_succeeded"].value, 1.0, "{report:#?}");
    assert_eq!(report.metrics["external_run_failed"].value, 0.0, "{report:#?}");
    assert!(report.metrics["completed_steps"].value >= expected_commands as f64, "{report:#?}");
    let transcript = std::fs::read_to_string(ctx.work_dir.join("colmap.log")).expect("real command transcript exists");
    let exits: Vec<_> = transcript.lines().filter(|line| line.starts_with("--- exit:")).collect();
    assert!(exits.len() >= expected_commands, "missing command exit evidence: {exits:?}");
    assert!(exits.iter().all(|line| *line == "--- exit: 0 ---"), "nonzero command exit: {exits:?}");
    report
}

fn valid_geometry(geometry: &Geometry, minimum_points: usize, expected_metric: bool) {
    geometry.validate().unwrap();
    assert!(geometry.points.iter().all(|point| point.is_finite()), "nonfinite geometry point");
    assert!(geometry.point_count() >= minimum_points, "only {} points", geometry.point_count());
    assert_eq!(metric_flag(geometry), Some(expected_metric));
}

fn check_scale(fixture: &Fixture, source: &Data, name: &str) {
    let geometry = source.as_geometry().unwrap();
    let first = geometry.points[0];
    let (index_b, last) = geometry.points.iter().enumerate()
        .max_by(|(_, a), (_, b)| a.distance_squared(first).total_cmp(&b.distance_squared(first))).unwrap();
    let measured = first.distance(*last);
    assert!(measured > 1e-9);
    let picks = Geometry::from_points(vec![first, *last]);
    let op = ScaleFromDistance;
    let mut params = Params::from_specs(&op.info().params);
    params.set("true_length", ParamValue::Float(100.0));
    params.set("uncertainty", ParamValue::Float(0.001));
    let ctx = context(fixture, name);
    let outputs = op.cook(&ctx, &[Some(source.clone()), Some(Data::geometry(picks))], &params).unwrap();
    let scaled = outputs[0].as_geometry().unwrap();
    valid_geometry(scaled, geometry.point_count(), true);
    assert_eq!(scaled.prim_count(), geometry.prim_count());
    assert!((scaled.points[0].distance(scaled.points[index_b]) - 100.0).abs() < 1e-8);
    let report = outputs[1].as_report().unwrap();
    assert!(report.passes(), "{report:#?}");
    let expected = 100.0 / measured;
    assert!((report.metrics["scale_factor"].value - expected).abs() <= 1e-12 * expected.abs().max(1.0));
    std::fs::write(ctx.work_dir.join("report.json"), serde_json::to_vec_pretty(report.as_ref()).unwrap()).unwrap();
    eprintln!("WP8 scale={} known_length_mm=100 measured={measured:.12} scale_factor={expected:.12}", name);
}

fn run_public_sample(include_dense_mesh: bool) {
    let fixture = prepare_fixture();
    let started = Instant::now();
    let feature_ctx = context(&fixture, "features");
    assert!(!feature_ctx.work_dir.join("database.db").exists());
    let op = SfmFeaturesMatch;
    let mut params = run_params(&op, &fixture);
    params.set("max_image_size", ParamValue::Int(800));
    params.set("max_features", ParamValue::Int(4096));
    params.set("matcher", ParamValue::Str("exhaustive".into()));
    params.set("use_gpu", ParamValue::Bool(false));
    let step = Instant::now();
    let features = op.cook(&feature_ctx, &[], &params).expect("feature extraction/matching node cooks");
    checked_report(&features, &feature_ctx, step.elapsed(), 2);
    let database = features[0].as_file().unwrap();
    assert!(std::fs::metadata(database).unwrap().len() > 0, "fresh database is empty");

    let sparse_ctx = context(&fixture, "sparse");
    assert!(!sparse_ctx.work_dir.join("sparse").exists());
    let op = SfmReconstruct;
    let mut params = run_params(&op, &fixture);
    params.set("mapper", ParamValue::Str("incremental".into()));
    let step = Instant::now();
    let sparse = op.cook(&sparse_ctx, &[Some(features[0].clone())], &params).expect("sparse reconstruction node cooks");
    checked_report(&sparse, &sparse_ctx, step.elapsed(), 2);
    let model_dir = sparse[0].as_file().unwrap();
    assert!(model_dir.starts_with(&sparse_ctx.work_dir), "model escaped fresh node work directory");
    let model = read_model_dir(model_dir, true).expect("fresh sparse points and converted poses import");
    assert!(model.images.len() >= 3, "only {} registered images", model.images.len());
    assert!(model.points.len() >= 100, "only {} sparse points", model.points.len());
    valid_geometry(sparse[1].as_geometry().unwrap(), 100, false);
    check_scale(&fixture, &sparse[1], "scale-sparse");
    eprintln!("WP8 registered_images={} sparse_points={}", model.images.len(), model.points.len());

    if include_dense_mesh {
        let dense_ctx = context(&fixture, "dense");
        assert!(!dense_ctx.work_dir.join("dense").exists());
        let op = MvsDense;
        let mut params = run_params(&op, &fixture);
        params.set("max_image_size", ParamValue::Int(800));
        params.set("geom_consistency", ParamValue::Bool(true));
        params.set("min_num_pixels", ParamValue::Int(3));
        params.set("window_radius", ParamValue::Int(3));
        let step = Instant::now();
        let dense = op.cook(&dense_ctx, &[Some(sparse[0].clone())], &params).expect("dense reconstruction node cooks");
        checked_report(&dense, &dense_ctx, step.elapsed(), 3);
        valid_geometry(dense[0].as_geometry().unwrap(), 1000, false);
        assert!(dense_ctx.work_dir.join("dense/fused.ply").is_file());

        let mesh_ctx = context(&fixture, "mesh");
        assert!(!mesh_ctx.work_dir.join("meshed.ply").exists());
        let op = MvsMesh;
        let mut params = run_params(&op, &fixture);
        params.set("mesher", ParamValue::Str("poisson".into()));
        params.set("target_face_ratio", ParamValue::Float(1.0));
        let step = Instant::now();
        let mesh = op.cook(&mesh_ctx, &[Some(dense[0].clone())], &params).expect("external meshing node cooks");
        checked_report(&mesh, &mesh_ctx, step.elapsed(), 1);
        let geometry = mesh[0].as_geometry().unwrap();
        valid_geometry(geometry, 3, false);
        let triangles = geometry.triangles().len();
        assert!(triangles >= 100, "only {triangles} mesh triangles");
        assert!(mesh_ctx.work_dir.join("meshed.ply").is_file());
        check_scale(&fixture, &mesh[0], "scale-mesh");
        eprintln!("WP8 dense_points={} mesh_vertices={} mesh_triangles={triangles}", dense[0].as_geometry().unwrap().point_count(), geometry.point_count());
        eprintln!(
            "WP8_REAL_COMPLETE registered_images={} sparse_points={} dense_points={} mesh_vertices={} mesh_triangles={triangles} scale_reference_mm=100 elapsed_s={:.3} evidence={}",
            model.images.len(), model.points.len(), dense[0].as_geometry().unwrap().point_count(), geometry.point_count(),
            started.elapsed().as_secs_f64(), fixture.root.display()
        );
    }
    eprintln!("WP8 ACCEPTED dense_mesh={include_dense_mesh} elapsed_s={:.3} evidence={}", started.elapsed().as_secs_f64(), fixture.root.display());
}

#[test]
#[ignore = "needs COLMAP"]
fn colmap_public_sample_sparse_and_scale() {
    run_public_sample(false);
}

#[test]
#[ignore = "needs COLMAP"]
fn colmap_public_sample_dense_mesh_and_scale() {
    run_public_sample(true);
}

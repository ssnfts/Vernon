//! Slicer hand-off: preset files, the command line, probing the installed
//! build for its output flag, and running it as a separate process.

use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use vernon_core::process::{self, Exit, RunOptions};

use glam::DVec3;
use serde_json::{json, Value};

use crate::common::{material, Material};

/// Equivalent filament diameter of a pellet extruder for a flow coefficient.
pub fn pellet_diameter(coef: f64) -> f64 {
    (4.0 / (coef * PI)).sqrt()
}

/// Flow coefficient for an equivalent filament diameter.
pub fn pellet_coef(diameter: f64) -> f64 {
    4.0 / (PI * diameter * diameter)
}

fn num(v: f64) -> String {
    let s = format!("{v:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[derive(Debug, Clone)]
pub struct ProfileSpec {
    pub flavour: String,
    pub bed: DVec3,
    pub nozzle_mm: f64,
    pub material: String,
    pub layer_height: f64,
    pub walls: i64,
    pub infill_pct: f64,
    pub relative_e: bool,
    pub firmware_retraction: bool,
    pub disable_m73: bool,
    pub pellet: bool,
    pub pellet_coef: f64,
    pub bed_temp_formula: String,
}

/// Three preset documents: (process, printer, filament). Key names follow the
/// public preset layout of the slicer family; values are strings.
pub fn build_profiles(s: &ProfileSpec) -> (Value, Value, Value) {
    let m: Material = material(&s.material);
    let area = ["0x0".to_string(), format!("{}x0", num(s.bed.x)), format!("{}x{}", num(s.bed.x), num(s.bed.y)), format!("0x{}", num(s.bed.y))];
    let on = |b: bool| if b { "1" } else { "0" };
    let process = json!({
        "type": "process",
        "name": "vernon_process",
        "from": "User",
        "inherits": "",
        "layer_height": num(s.layer_height),
        "initial_layer_print_height": num((s.layer_height * 1.25).max(s.layer_height)),
        "wall_loops": s.walls.to_string(),
        "sparse_infill_density": format!("{}%", num(s.infill_pct)),
        "top_shell_layers": "5",
        "bottom_shell_layers": "4",
    });
    let printer = json!({
        "type": "machine",
        "name": "vernon_printer",
        "from": "User",
        "inherits": "",
        "gcode_flavor": s.flavour,
        "printable_area": area,
        "printable_height": num(s.bed.z),
        "nozzle_diameter": [num(s.nozzle_mm)],
        "use_relative_e_distances": on(s.relative_e),
        "use_firmware_retraction": on(s.firmware_retraction),
        "disable_m73": on(s.disable_m73),
        "bed_temperature_formula": s.bed_temp_formula,
        "build_plate_tilt_x": "0",
        "build_plate_tilt_y": "0",
        "pellet_modded_printer": on(s.pellet),
        "pellet_flow_coefficient": num(s.pellet_coef),
    });
    let filament = json!({
        "type": "filament",
        "name": "vernon_filament",
        "from": "User",
        "inherits": "",
        "filament_type": [m.id.to_ascii_uppercase().replace('_', "-")],
        "nozzle_temperature": [num(m.nozzle_c)],
        "hot_plate_temp": [num(m.bed_c)],
        "filament_density": [num(m.density)],
        "filament_diameter": [if s.pellet { num(pellet_diameter(s.pellet_coef)) } else { "1.75".to_string() }],
    });
    (process, printer, filament)
}

#[derive(Debug, Clone, Default)]
pub struct SliceArgs {
    pub input: String,
    /// Process then printer preset files.
    pub load_settings: Vec<String>,
    pub load_filaments: Vec<String>,
    /// 0 = all plates.
    pub plate: i64,
    pub arrange: bool,
    pub export_3mf: Option<String>,
    pub output_flag: Option<(String, String)>,
    pub mtcpp: Option<u64>,
    pub mstpp: Option<u64>,
    pub extra: Vec<String>,
}

/// Triangle limit to pass for a model of `triangles` faces; None while the
/// default limit of one million is enough.
pub fn mtcpp_for(triangles: usize) -> Option<u64> {
    if triangles <= 1_000_000 {
        return None;
    }
    let want = (triangles as f64 * 1.2 / 100_000.0).ceil() as u64;
    Some(want * 100_000)
}

/// Argument list in the order input, presets, slice, export, limits, output.
pub fn build_args(a: &SliceArgs) -> Vec<String> {
    let mut v = vec![a.input.clone()];
    if !a.load_settings.is_empty() {
        v.push("--load-settings".into());
        v.push(a.load_settings.join(";"));
    }
    if !a.load_filaments.is_empty() {
        v.push("--load-filaments".into());
        v.push(a.load_filaments.join(";"));
    }
    if a.arrange {
        v.push("--arrange".into());
        v.push("1".into());
    }
    v.push("--slice".into());
    v.push(a.plate.to_string());
    if let Some(o) = &a.export_3mf {
        v.push("--export-3mf".into());
        v.push(o.clone());
    }
    if let Some(n) = a.mtcpp {
        v.push("--mtcpp".into());
        v.push(n.to_string());
    }
    if let Some(n) = a.mstpp {
        v.push("--mstpp".into());
        v.push(n.to_string());
    }
    if let Some((flag, dir)) = &a.output_flag {
        v.push(flag.clone());
        v.push(dir.clone());
    }
    v.extend(a.extra.iter().cloned());
    v
}

/// One-line command for logs, quoting arguments that need it.
pub fn display_command(exe: &str, args: &[String]) -> String {
    let q = |s: &str| if s.contains([' ', ';', '"']) { format!("\"{}\"", s.replace('"', "\\\"")) } else { s.to_string() };
    std::iter::once(exe).chain(args.iter().map(String::as_str)).map(q).collect::<Vec<_>>().join(" ")
}

/// Find the flag that sets the G-code output folder in the `--help` text.
pub fn probe_output_flag(help: &str) -> Option<String> {
    let flags: Vec<&str> = help
        .split(|c: char| c.is_whitespace() || c == ',' || c == '[' || c == ']' || c == '<' || c == '>')
        .filter(|t| t.starts_with("--"))
        .collect();
    ["--outputdir", "--output-dir", "--output_dir", "--outdir"].iter().find(|c| flags.contains(c)).map(|s| s.to_string())
}

pub fn describe_exit(code: Option<i32>) -> String {
    match code {
        Some(0) => "ok".into(),
        Some(-2) => "invalid parameter".into(),
        Some(-3) => "input file not found".into(),
        Some(c) => format!("exit code {c}"),
        None => "ended without an exit code".into(),
    }
}

/// First existing file among `names` in the PATH directories.
pub fn find_in_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for n in names {
            for cand in [dir.join(n), dir.join(format!("{n}.exe"))] {
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

pub const SLICER_NAMES: [&str; 4] = ["orca-slicer", "OrcaSlicer", "prusa-slicer", "PrusaSlicer"];

#[derive(Debug, Clone)]
pub struct RunOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// Run a program to completion (or the timeout) through the shared runner
/// ([`vernon_core::process`]), which kills the whole process tree at the
/// deadline or when the cook is cancelled. Both streams go to a temporary
/// log file (never a pipe a grandchild could hold open) and come back
/// combined in `stdout`; `stderr` is empty.
pub fn run_capture(exe: &Path, args: &[String], cwd: &Path, timeout: Duration) -> std::io::Result<RunOutput> {
    static RUNS: AtomicU64 = AtomicU64::new(0);
    let log = std::env::temp_dir().join(format!("vernon-slicer-{}-{}.log", std::process::id(), RUNS.fetch_add(1, Ordering::Relaxed)));
    let mut cmd = Command::new(exe);
    cmd.args(args).current_dir(cwd);
    let opts = RunOptions::with_timeout(timeout.max(Duration::from_millis(1)));
    let run = process::run(&mut cmd, &log, &opts);
    let text = std::fs::read(&log).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    let (code, timed_out) = match run? {
        Exit::Code(c) => (Some(c), false),
        Exit::TimedOut | Exit::Cancelled => (None, true),
    };
    Ok(RunOutput { code, stdout: text, stderr: String::new(), timed_out })
}

/// G-code files (and packaged G-code) found under `dir`, two levels deep.
pub fn find_gcode(dir: &Path) -> Vec<PathBuf> {
    fn walk(d: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && depth > 0 {
                walk(&p, depth - 1, out);
            } else if p.is_file() {
                let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
                if n.ends_with(".gcode") || n.ends_with(".gcode.3mf") {
                    out.push(p);
                }
            }
        }
    }
    let mut v = Vec::new();
    walk(dir, 2, &mut v);
    v.sort();
    v
}

#[cfg(test)]
pub mod mock {
    use std::path::{Path, PathBuf};

    /// A tiny stand-in program: prints a help text and writes one G-code file.
    pub fn mock_slicer(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        #[cfg(windows)]
        {
            let p = dir.join("mockslicer.cmd");
            let body = "@echo off\r\nif \"%1\"==\"--help\" (\r\n  echo   --outputdir arg  output folder\r\n  exit /b 0\r\n)\r\necho ;LAYER_CHANGE> plate_1.gcode\r\necho G1 X1 E1>> plate_1.gcode\r\nexit /b 0\r\n";
            std::fs::write(&p, body).unwrap();
            p
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = dir.join("mockslicer.sh");
            let body = "#!/bin/sh\nif [ \"$1\" = \"--help\" ]; then echo '  --outputdir arg  output folder'; exit 0; fi\necho ';LAYER_CHANGE' > plate_1.gcode\necho 'G1 X1 E1' >> plate_1.gcode\nexit 0\n";
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::mock_slicer;
    use super::*;

    fn spec() -> ProfileSpec {
        ProfileSpec {
            flavour: "klipper".into(),
            bed: DVec3::new(350.0, 350.0, 350.0),
            nozzle_mm: 0.4,
            material: "asa".into(),
            layer_height: 0.2,
            walls: 3,
            infill_pct: 15.0,
            relative_e: true,
            firmware_retraction: false,
            disable_m73: false,
            pellet: false,
            pellet_coef: 0.4157,
            bed_temp_formula: "by_highest_temp".into(),
        }
    }

    #[test]
    fn pellet_round_trip_golden() {
        let c = pellet_coef(1.75);
        assert!((c - 0.4157).abs() < 1e-4, "{c}");
        assert!((pellet_diameter(c) - 1.75).abs() < 1e-12);
    }

    #[test]
    fn profile_documents() {
        let (p, m, f) = build_profiles(&spec());
        assert_eq!(p["layer_height"], "0.2");
        assert_eq!(p["sparse_infill_density"], "15%");
        assert_eq!(m["gcode_flavor"], "klipper");
        assert_eq!(m["printable_area"][2], "350x350");
        assert_eq!(m["use_relative_e_distances"], "1");
        assert_eq!(m["pellet_modded_printer"], "0");
        assert_eq!(f["filament_type"][0], "ASA");
        assert_eq!(f["nozzle_temperature"][0], "250");
        let mut s = spec();
        s.pellet = true;
        s.material = "pet_cf".into();
        let (_, m, f) = build_profiles(&s);
        assert_eq!(m["pellet_modded_printer"], "1");
        assert_eq!(f["filament_type"][0], "PET-CF");
        assert!((f["filament_diameter"][0].as_str().unwrap().parse::<f64>().unwrap() - 1.75).abs() < 1e-3);
    }

    #[test]
    fn command_line_matches_documented_form() {
        let a = SliceArgs {
            input: "model.3mf".into(),
            load_settings: vec!["process.json".into(), "printer.json".into()],
            load_filaments: vec!["fil.json".into()],
            plate: 0,
            export_3mf: Some("out.3mf".into()),
            ..Default::default()
        };
        let args = build_args(&a);
        assert_eq!(args, ["model.3mf", "--load-settings", "process.json;printer.json", "--load-filaments", "fil.json", "--slice", "0", "--export-3mf", "out.3mf"]);
        assert_eq!(
            display_command("slicer", &args),
            "slicer model.3mf --load-settings \"process.json;printer.json\" --load-filaments fil.json --slice 0 --export-3mf out.3mf"
        );
    }

    #[test]
    fn triangle_limit_rule() {
        assert_eq!(mtcpp_for(900_000), None);
        assert_eq!(mtcpp_for(1_000_000), None);
        assert_eq!(mtcpp_for(1_200_000), Some(1_500_000));
        let a = SliceArgs { input: "m.3mf".into(), mtcpp: mtcpp_for(1_200_000), mstpp: Some(600), arrange: true, ..Default::default() };
        let v = build_args(&a);
        assert!(v.windows(2).any(|w| w == ["--mtcpp", "1500000"]) && v.windows(2).any(|w| w == ["--mstpp", "600"]) && v.windows(2).any(|w| w == ["--arrange", "1"]));
    }

    #[test]
    fn output_flag_probe() {
        let help = "Usage: tool [options]\n  --slice arg        slice\n  --outputdir <dir>  output folder\n  --info\n";
        assert_eq!(probe_output_flag(help).as_deref(), Some("--outputdir"));
        assert_eq!(probe_output_flag("--slice only\n"), None);
    }

    #[test]
    fn exit_descriptions() {
        assert_eq!(describe_exit(Some(0)), "ok");
        assert_eq!(describe_exit(Some(-2)), "invalid parameter");
        assert_eq!(describe_exit(Some(-3)), "input file not found");
        assert_eq!(describe_exit(Some(7)), "exit code 7");
    }

    #[test]
    fn mock_process_help_and_run() {
        let dir = std::env::temp_dir().join(format!("vernon-print-mock-{}", std::process::id()));
        let exe = mock_slicer(&dir);
        let help = run_capture(&exe, &["--help".to_string()], &dir, Duration::from_secs(20)).unwrap();
        assert_eq!(help.code, Some(0));
        assert_eq!(probe_output_flag(&help.stdout).as_deref(), Some("--outputdir"));
        let out_dir = dir.join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        let run = run_capture(&exe, &["model.3mf".to_string(), "--slice".to_string(), "0".to_string()], &out_dir, Duration::from_secs(20)).unwrap();
        assert_eq!(run.code, Some(0));
        assert!(!run.timed_out);
        let found = find_gcode(&out_dir);
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("plate_1.gcode"));
    }

    #[test]
    fn missing_program_is_an_io_error() {
        assert!(run_capture(Path::new("definitely-not-a-program-vernon"), &[], Path::new("."), Duration::from_secs(1)).is_err());
    }
}

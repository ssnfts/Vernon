//! Package and hand-off nodes: 3MF write and read, slicer presets, external
//! slice run, G-code summary and conformance, remote upload, script bridge.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use vernon_core::{Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::common::{info, material, path_or_default, placeholder, printer_bed, v3_json, work_dir, MATERIAL_IDS, PRINTER_IDS};
use crate::gcode::{self, SummaryOptions, FLAVOURS};
use crate::mesh;
use crate::package::{self, UNIT_IDS};
use crate::slicer::{self, ProfileSpec, SliceArgs};
use crate::upload::{self, UploadParams};

fn file_input(inp: &[Option<Data>], index: usize, name: &str) -> CookResult<PathBuf> {
    match inp.get(index).and_then(|d| d.as_ref()) {
        Some(Data::File(p)) => Ok(p.clone()),
        Some(other) => Err(CookError::WrongInput { index, expected: DataKind::File, found: other.kind() }),
        None => Err(CookError::MissingInput { index, name: name.into() }),
    }
}

fn opt_file(inp: &[Option<Data>], index: usize) -> Option<PathBuf> {
    inp.get(index).and_then(|d| d.as_ref()).and_then(|d| d.as_file().cloned())
}

fn open_gcode(ctx: &CookContext, path: &Path) -> CookResult<BufReader<File>> {
    File::open(path).map(BufReader::new).map_err(|e| ctx.fail(format!("cannot open {}: {e}", path.display())))
}

fn s(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

// ------------------------------------------------------- package write/read

pub struct PackageWrite;

impl NodeOp for PackageWrite {
    fn info(&self) -> NodeTypeInfo {
        info("print.package_write", "Package Writer", "Write the parts to a 3MF package, one object per part, after checking that every shell is closed and wound outward.")
            .emulates("3MF export of slicer plates")
            .implemented()
            .input(PortSpec::new("parts", DataKind::Geometry))
            .param(ParamSpec::path("path", "File", &["3mf"], true).help("Output file; empty writes parts.3mf in the node folder."))
            .param(ParamSpec::choice("unit", "Unit", "millimeter", &UNIT_IDS).help("Length unit stored in the package."))
            .param(ParamSpec::bool("strict", "Require closed shells", true).help("Refuse parts with open or non-manifold edges."))
            .param(ParamSpec::bool("stamp_time", "Stamp build time", false).help("Record the current time as the package creation date. Off keeps the package bytes identical between cooks of the same graph."))
            .output(PortSpec::new("package", DataKind::File))
            .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let path = path_or_default(ctx, p.str("path")?, "parts.3mf")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let date = if p.bool("stamp_time")? { None } else { Some(package::REPRODUCIBLE_DATE) };
        let st = package::write_package_dated(&geo, &path, package::unit_of(p.str("unit")?), p.bool("strict")?, date).map_err(|e| ctx.fail(e))?;
        let mut rep = Report::new("Package writer");
        rep.metric("part_count", Metric::new(st.parts as f64, "count"));
        rep.metric("triangle_count", Metric::new(st.triangles as f64, "count"));
        rep.metric("volume_mm3", Metric::new(st.volume_mm3, "mm3"));
        rep.metric("open_edges", Metric::new(st.open_edges as f64, "count").with_limit(0.0, true));
        rep.metric("non_manifold_edges", Metric::new(st.non_manifold_edges as f64, "count").with_limit(0.0, true));
        rep.metric("file_bytes", Metric::new(st.bytes as f64, "bytes"));
        rep.metric("roundtrip_volume_error_rel", Metric::new(st.roundtrip_error, "ratio").with_limit(1e-6, true));
        if st.flipped_parts > 0 {
            rep.note(Severity::Warning, format!("{} part(s) were wound inward and were turned outward", st.flipped_parts));
        }
        Ok(vec![Data::File(path), Data::report(rep)])
    }
}

pub struct PackageRead;

impl NodeOp for PackageRead {
    fn info(&self) -> NodeTypeInfo {
        info("print.package_read", "Package Reader", "Read a 3MF package into millimetre parts, applying its unit and placement transforms.")
            .emulates("3MF import of slicer plates")
            .implemented()
            .input(PortSpec::optional("package", DataKind::File))
            .param(ParamSpec::path("path", "File", &["3mf"], false).help("Package to read when no file is connected."))
            .output(PortSpec::new("parts", DataKind::Geometry))
            .output(PortSpec::new("manifest", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let path = match opt_file(inp, 0) {
            Some(f) => f,
            None if !p.str("path")?.trim().is_empty() => PathBuf::from(p.str("path")?),
            None => return Err(ctx.fail("choose a package file or connect one")),
        };
        let geo = vernon_io::read(&path, &vernon_io::ReadOptions::default()).map_err(|e| ctx.fail(e.to_string()))?;
        let mut rep = Report::new("Package reader");
        let mut entries = Vec::new();
        let (mut tris, mut open, mut vol) = (0usize, 0usize, 0.0);
        for id in mesh::unique_parts(&geo) {
            let g = mesh::extract_part(&geo, id);
            let b = g.bounds();
            let e = mesh::edge_stats(&g);
            let v = mesh::signed_volume(&g);
            tris += g.triangles().len();
            open += e.open;
            vol += v;
            entries.push(json!({"part": id, "triangles": g.triangles().len(), "volume_mm3": v, "open_edges": e.open, "bbox_min": v3_json(b.min), "bbox_max": v3_json(b.max)}));
        }
        rep.metric("part_count", Metric::new(entries.len() as f64, "count"));
        rep.metric("triangle_count", Metric::new(tris as f64, "count"));
        rep.metric("volume_mm3", Metric::new(vol, "mm3"));
        rep.metric("open_edges", Metric::new(open as f64, "count").with_limit(0.0, true));
        Ok(vec![Data::geometry(geo), Data::json(json!({"file": s(&path), "parts": entries})), Data::report(rep)])
    }
}

// ----------------------------------------------------------- profile build

pub struct ProfileBuild;

impl NodeOp for ProfileBuild {
    fn info(&self) -> NodeTypeInfo {
        info("print.profile_build", "Profile Builder", "Write the process, printer and filament preset files for the slicer from a few high-level choices.")
            .emulates("slicer preset files for command-line runs")
            .param(ParamSpec::choice("flavour", "G-code flavour", "klipper", &FLAVOURS).help("Dialect of the output G-code."))
            .param(ParamSpec::choice("printer", "Printer preset", "bed_300", &PRINTER_IDS).help("Build volume preset; custom uses the bed size below."))
            .param(ParamSpec::vec3("bed", "Custom bed size", glam::DVec3::new(256.0, 256.0, 256.0)).unit("mm").help("Used when the preset is custom."))
            .param(ParamSpec::choice("material", "Material", "asa", &MATERIAL_IDS).help("Sets the temperatures and density."))
            .param(ParamSpec::float("nozzle", "Nozzle diameter", 0.4, 0.2, 1.2).unit("mm").help("Nozzle bore."))
            .param(ParamSpec::float("layer_height", "Layer height", 0.2, 0.05, 0.6).unit("mm").help("Layer thickness."))
            .param(ParamSpec::int("walls", "Wall loops", 3, 1, 10).help("Perimeter count."))
            .param(ParamSpec::float("infill", "Infill density", 15.0, 0.0, 100.0).unit("%").help("Sparse infill density."))
            .param(ParamSpec::bool("relative_e", "Relative extrusion", true).help("Extruder moves are relative; needed for multi-part purge logic."))
            .param(ParamSpec::bool("firmware_retraction", "Firmware retraction", false).help("Let the firmware retract (G10/G11)."))
            .param(ParamSpec::bool("disable_remaining_time", "Omit remaining-time commands", false).help("Leave out the remaining-time progress lines."))
            .param(ParamSpec::bool("pellet", "Pellet extruder", false).help("Describe the extruder by a pellet flow coefficient."))
            .param(ParamSpec::float("pellet_coef", "Pellet flow coefficient", 0.4157, 0.05, 5.0).help("Equivalent filament diameter d = sqrt(4 / (coef * pi))."))
            .param(ParamSpec::choice("bed_temp_formula", "Bed temperature rule", "by_highest_temp", &["by_first_filament", "by_highest_temp"]).help("Which filament sets the bed temperature."))
            .param(ParamSpec::float("chamber_c", "Chamber temperature", 0.0, 0.0, 90.0).unit("°C").help("Heated-chamber set point; 0 leaves chamber control off. Presets cap it at what the printer can hold; high-temperature materials want 45 to 65. The report warns when the material wants more."))
            .param(ParamSpec::float("pressure_advance", "Pressure advance", 0.0, 0.0, 2.0).help("Pressure-advance (linear advance) factor from a calibration print; 0 leaves it off."))
            .param(ParamSpec::float("flow_ratio", "Flow ratio", 1.0, 0.8, 1.2).help("Extrusion multiplier from a calibration print; 1 is as designed."))
            .param(ParamSpec::float("max_volumetric_speed", "Max volumetric speed", 0.0, 0.0, 60.0).unit("mm³/s").help("Largest melt flow the hot end keeps up with, from a calibration print; 0 sets no limit."))
            .param(ParamSpec::bool("z_anti_alias", "Z anti-aliasing", false).help("Vary the surface of shallow slopes between layers to hide the staircase; the remedy for what the stair-step estimate finds."))
            .output(PortSpec::new("profiles", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, ctx: &CookContext, _inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let bed = printer_bed(p.str("printer")?, p.vec3("bed")?);
        let preset = crate::common::printer(p.str("printer")?);
        let chamber_c = p.float("chamber_c")?;
        if let Some(pr) = preset {
            if chamber_c > pr.chamber_max_c {
                return Err(ctx.fail(format!(
                    "the printer preset {} holds a chamber of {:.0} deg C at most (asked for {chamber_c:.0}); lower the chamber temperature or use the custom preset",
                    pr.id, pr.chamber_max_c
                )));
            }
        }
        let spec = ProfileSpec {
            flavour: p.str("flavour")?.to_string(),
            bed,
            nozzle_mm: p.float("nozzle")?,
            material: p.str("material")?.to_string(),
            layer_height: p.float("layer_height")?,
            walls: p.int("walls")?,
            infill_pct: p.float("infill")?,
            relative_e: p.bool("relative_e")?,
            firmware_retraction: p.bool("firmware_retraction")?,
            disable_m73: p.bool("disable_remaining_time")?,
            pellet: p.bool("pellet")?,
            pellet_coef: p.float("pellet_coef")?,
            bed_temp_formula: p.str("bed_temp_formula")?.to_string(),
            chamber_c,
            chamber_capable: preset.is_none_or(|pr| pr.chamber_max_c > 0.0),
            pressure_advance: p.float("pressure_advance")?,
            flow_ratio: p.float("flow_ratio")?,
            max_volumetric_speed: p.float("max_volumetric_speed")?,
            z_anti_alias: p.bool("z_anti_alias")?,
        };
        if spec.layer_height > spec.nozzle_mm * 0.8 {
            return Err(ctx.fail("layer height must stay below 80 % of the nozzle diameter"));
        }
        let (process, printer, filament) = slicer::build_profiles(&spec);
        let dir = work_dir(ctx)?;
        let mut paths = Vec::new();
        for (name, doc) in [("process.json", &process), ("printer.json", &printer), ("filament.json", &filament)] {
            let f = dir.join(name);
            std::fs::write(&f, serde_json::to_string_pretty(doc).map_err(|e| ctx.fail(e.to_string()))?)?;
            paths.push(f);
        }
        let m = material(&spec.material);
        let mut rep = Report::new("Profile builder");
        rep.metric("preset_count", Metric::new(3.0, "count"));
        rep.metric("nozzle_temp_c", Metric::new(m.nozzle_c, "deg C"));
        rep.metric("bed_temp_c", Metric::new(m.bed_c, "deg C"));
        rep.metric("chamber_temp_c", Metric::new(spec.chamber_c, "deg C"));
        rep.metric("material_chamber_c", Metric::new(m.chamber_c, "deg C"));
        if m.chamber_c > spec.chamber_c {
            rep.note(
                Severity::Warning,
                format!(
                    "{} prints best in a chamber of about {:.0} deg C (set: {:.0}); expect warping and weak layer bonds in a cold chamber",
                    m.id.to_ascii_uppercase(),
                    m.chamber_c,
                    spec.chamber_c
                ),
            );
        }
        if spec.pellet {
            rep.metric("equivalent_filament_diameter_mm", Metric::new(slicer::pellet_diameter(spec.pellet_coef), "mm"));
        }
        placeholder(&mut rep, "the preset key names follow the public preset layout from memory; check them against the installed slicer before relying on a run");
        let out = json!({
            "process": s(&paths[0]), "printer": s(&paths[1]), "filament": s(&paths[2]),
            "load_settings": [s(&paths[0]), s(&paths[1])], "load_filaments": [s(&paths[2])],
            "flavour": spec.flavour, "bed_mm": v3_json(bed),
        });
        Ok(vec![Data::json(out), Data::report(rep)])
    }
}

// ---------------------------------------------------------- external slice

pub struct SliceExternal;

fn json_paths(v: &Value, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        Some(Value::String(x)) => x.split(';').filter(|t| !t.is_empty()).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

impl NodeOp for SliceExternal {
    fn info(&self) -> NodeTypeInfo {
        info("print.slice_external", "External Slice Runner", "Slice a package with the installed command-line slicer, as a separate process, and read back the G-code.")
            .emulates("command-line slicing of a project package")
            .implemented()
            .external("OrcaSlicer")
            .input(PortSpec::optional("package", DataKind::File))
            .input(PortSpec::optional("parts", DataKind::Geometry))
            .input(PortSpec::optional("profiles", DataKind::Json))
            .param(ParamSpec::bool("run", "Run slicer", false).help("Off writes the command only; on starts the slicer program once and switches itself off again.").action())
            .param(ParamSpec::path("executable", "Slicer program", &[], false).help("Path of the slicer program; empty looks for one on the search path."))
            .param(ParamSpec::int("plate", "Plate", 0, 0, 99).help("0 slices every plate."))
            .param(ParamSpec::bool("arrange", "Auto arrange", false).help("Let the slicer arrange the objects."))
            .param(ParamSpec::int("mtcpp", "Triangle limit per plate", 0, 0, 100_000_000).help("0 raises the limit only when the model needs it."))
            .param(ParamSpec::int("mstpp", "Time limit per plate", 600, 10, 86_400).unit("s").help("Longest slicing time per plate."))
            .param(ParamSpec::path("process_preset", "Process preset", &["json"], false).help("Used when no profiles are connected."))
            .param(ParamSpec::path("printer_preset", "Printer preset", &["json"], false).help("Used when no profiles are connected."))
            .param(ParamSpec::path("filament_preset", "Filament preset", &["json"], false).help("Used when no profiles are connected."))
            .param(ParamSpec::string("extra_args", "Extra arguments", "").help("Further arguments, separated by spaces."))
            .output(PortSpec::new("gcode", DataKind::File))
            .output(PortSpec::new("result", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let dir = work_dir(ctx)?;
        let mut rep = Report::new("External slice run");
        let mut triangles = 0usize;
        // Package: connected file, or the connected parts written as 3MF.
        let package = if let Some(f) = opt_file(inp, 0) {
            if let Ok(g) = vernon_io::read(&f, &vernon_io::ReadOptions::default()) {
                triangles = g.triangles().len();
            }
            f
        } else if let Some(g) = inputs::opt_geometry(inp, 1) {
            let f = dir.join("model.3mf");
            let st = package::write_package(&g, &f, vernon_io::FileUnit::Millimetre, false).map_err(|e| ctx.fail(e))?;
            triangles = st.triangles;
            if st.open_edges > 0 {
                rep.note(Severity::Warning, format!("the parts have {} open edges; the slicer may repair or reject them", st.open_edges));
            }
            f
        } else {
            return Err(ctx.fail("connect a 3MF package or the parts to slice"));
        };
        let (mut settings, mut filaments) = (Vec::new(), Vec::new());
        if let Some(j) = inp.get(2).and_then(|d| d.as_ref()).and_then(|d| d.as_json()) {
            settings = json_paths(j, "load_settings");
            filaments = json_paths(j, "load_filaments");
        } else {
            for k in ["process_preset", "printer_preset"] {
                if !p.str(k)?.trim().is_empty() {
                    settings.push(p.str(k)?.to_string());
                }
            }
            if !p.str("filament_preset")?.trim().is_empty() {
                filaments.push(p.str("filament_preset")?.to_string());
            }
        }
        if settings.is_empty() && filaments.is_empty() {
            rep.note(Severity::Warning, "no presets given; the slicer defaults apply");
        }
        let out_dir = dir.join("slice");
        std::fs::create_dir_all(&out_dir)?;
        for old in slicer::find_gcode(&out_dir) {
            let _ = std::fs::remove_file(old);
        }
        let mtcpp = match p.int("mtcpp")? {
            0 => slicer::mtcpp_for(triangles),
            n => Some(n as u64),
        };
        let mut args = SliceArgs {
            input: s(&package),
            load_settings: settings,
            load_filaments: filaments,
            plate: p.int("plate")?,
            arrange: p.bool("arrange")?,
            export_3mf: Some(s(&out_dir.join("out.3mf"))),
            mtcpp,
            mstpp: Some(p.int("mstpp")? as u64),
            extra: p.str("extra_args")?.split_whitespace().map(str::to_string).collect(),
            ..Default::default()
        };
        let exe = if p.str("executable")?.trim().is_empty() { slicer::find_in_path(&slicer::SLICER_NAMES) } else { Some(PathBuf::from(p.str("executable")?)).filter(|e| e.is_file()) };
        rep.metric("triangle_count", Metric::new(triangles as f64, "count"));
        let mut result = json!({"run": false});
        let mut gcode_out: Option<PathBuf> = None;
        let mut ran = false;
        if p.bool("run")? {
            match &exe {
                None => {
                    rep.note(Severity::Warning, "no slicer program found; set the program path. Only the command was written.");
                }
                Some(exe) => {
                    // The output-folder flag differs between builds: ask the installed one.
                    if let Ok(h) = slicer::run_capture(exe, &["--help".to_string()], &out_dir, Duration::from_secs(30)) {
                        if let Some(flag) = slicer::probe_output_flag(&format!("{}\n{}", h.stdout, h.stderr)) {
                            args.output_flag = Some((flag, s(&out_dir)));
                        }
                    }
                    let argv = slicer::build_args(&args);
                    let cmd = slicer::display_command(&s(exe), &argv);
                    let timeout = Duration::from_secs(args.mstpp.unwrap_or(600).saturating_mul(if args.plate == 0 { 4 } else { 1 }) + 120);
                    let run = slicer::run_capture(exe, &argv, &out_dir, timeout).map_err(|e| CookError::External { tool: "slicer".into(), message: e.to_string() })?;
                    ran = true;
                    let code = run.code;
                    rep.metric("exit_code", Metric::new(f64::from(code.unwrap_or(-1)), "code").with_limit(0.0, true));
                    result = json!({"run": true, "command": cmd, "exit_code": code, "exit": slicer::describe_exit(code), "timed_out": run.timed_out});
                    if run.timed_out {
                        return Err(CookError::External { tool: "slicer".into(), message: format!("stopped after {} s", timeout.as_secs()) });
                    }
                    if code != Some(0) {
                        let tail: String = format!("{}{}", run.stderr, run.stdout).chars().rev().take(400).collect::<String>().chars().rev().collect();
                        return Err(CookError::External { tool: "slicer".into(), message: format!("{}: {}", slicer::describe_exit(code), tail.trim()) });
                    }
                    gcode_out = slicer::find_gcode(&out_dir).into_iter().next();
                    if gcode_out.is_none() {
                        rep.note(Severity::Error, "the slicer finished without writing G-code");
                    }
                }
            }
        } else {
            rep.note(Severity::Warning, "slicer not run: Run slicer is off, the command was written for review");
        }
        let argv = slicer::build_args(&args);
        let cmd_text = slicer::display_command(&exe.as_ref().map(|e| s(e)).unwrap_or_else(|| "slicer".into()), &argv);
        let cmd_file = dir.join("slice_command.txt");
        std::fs::write(&cmd_file, format!("{cmd_text}\n"))?;
        if !ran {
            result = json!({"run": false, "command": cmd_text});
        }
        if let Some(g) = &gcode_out {
            if g.extension().is_some_and(|e| e.eq_ignore_ascii_case("gcode")) {
                let sm = gcode::summarize(open_gcode(ctx, g)?, &SummaryOptions::default());
                rep.metric("layer_count", Metric::new(sm.layers as f64, "count"));
                rep.metric("time_s", Metric::new(sm.time_s, "s"));
                rep.metric("filament_mm", Metric::new(sm.filament_mm, "mm"));
                rep.metric("filament_g", Metric::new(sm.filament_g, "g"));
            }
            result["gcode"] = json!(s(g));
        }
        Ok(vec![Data::File(gcode_out.unwrap_or(cmd_file)), Data::json(result), Data::report(rep)])
    }
}

// ----------------------------------------------------------- gcode summary

pub struct GcodeSummaryNode;

impl NodeOp for GcodeSummaryNode {
    fn info(&self) -> NodeTypeInfo {
        info("print.gcode_summary", "G-code Summary", "Read a G-code file and report layers, print time, filament length and mass, and the likely dialect.")
            .emulates("print statistics of slicer previews")
            .implemented()
            .input(PortSpec::new("gcode", DataKind::File))
            .param(ParamSpec::choice("extruder_mode", "Extruder mode", "auto", &["auto", "relative", "absolute"]).help("Auto follows the mode commands in the file."))
            .param(ParamSpec::float("filament_diameter", "Filament diameter", 1.75, 1.0, 3.0).unit("mm").help("Used to turn length into mass."))
            .param(ParamSpec::choice("material", "Material", "pla", &MATERIAL_IDS).help("Sets the density for the mass estimate."))
            .output(PortSpec::new("summary", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let path = file_input(inp, 0, "gcode")?;
        let opts = SummaryOptions {
            relative_e: match p.str("extruder_mode")? {
                "relative" => Some(true),
                "absolute" => Some(false),
                _ => None,
            },
            filament_diameter: p.float("filament_diameter")?,
            density: material(p.str("material")?).density,
        };
        let sm = gcode::summarize(open_gcode(ctx, &path)?, &opts);
        let mut rep = Report::new("G-code summary");
        rep.metric("layer_count", Metric::new(sm.layers as f64, "count"));
        rep.metric("time_s", Metric::new(sm.time_s, "s"));
        rep.metric("filament_mm", Metric::new(sm.filament_mm, "mm"));
        rep.metric("filament_g", Metric::new(sm.filament_g, "g"));
        rep.metric("move_count", Metric::new(sm.moves as f64, "count"));
        rep.metric("max_z_mm", Metric::new(sm.max_z, "mm"));
        rep.note(Severity::Info, format!("dialect guess: {}; extruder {}", sm.flavour, if sm.relative_e { "relative" } else { "absolute" }));
        if sm.time_source == "estimate" {
            rep.note(Severity::Warning, "no time in the file; the figure is a rough estimate from feed rates without acceleration");
        }
        let j = json!({"layers": sm.layers, "time_s": sm.time_s, "time_source": sm.time_source, "filament_mm": sm.filament_mm, "filament_g": sm.filament_g,
                       "flavour": sm.flavour, "relative_e": sm.relative_e, "lines": sm.lines, "moves": sm.moves, "max_z_mm": sm.max_z});
        Ok(vec![Data::json(j), Data::report(rep)])
    }
}

// ----------------------------------------------------------- flavour check

pub struct FlavourCheck;

impl NodeOp for FlavourCheck {
    fn info(&self) -> NodeTypeInfo {
        info("print.flavour_check", "Flavour Conformance", "Check that a G-code file only uses commands of the chosen firmware dialect and that object-cancel markers are well formed.")
            .emulates("G-code dialect validation")
            .input(PortSpec::new("gcode", DataKind::File))
            .param(ParamSpec::choice("flavour", "Flavour", "klipper", &FLAVOURS).help("Dialect the file must follow."))
            .param(ParamSpec::bool("strict", "Strict", false).help("Report violations as errors instead of warnings."))
            .output(PortSpec::new("violations", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let path = file_input(inp, 0, "gcode")?;
        let flavour = p.str("flavour")?;
        let v = gcode::check_flavour(open_gcode(ctx, &path)?, flavour);
        let defines = gcode::count_object_defines(open_gcode(ctx, &path)?);
        let mut rep = Report::new("Flavour conformance");
        rep.metric("violation_count", Metric::new(v.len() as f64, "count").with_limit(0.0, true));
        rep.metric("object_define_count", Metric::new(defines as f64, "count"));
        let sev = if p.bool("strict")? { Severity::Error } else { Severity::Warning };
        for x in v.iter().take(50) {
            rep.note(sev, if x.line > 0 { format!("line {}: {}", x.line, x.message) } else { x.message.clone() });
        }
        placeholder(&mut rep, "only object-cancel markers, pressure-advance commands and firmware-specific commands are checked; the full command tables per firmware are not yet covered");
        let j = Value::Array(v.iter().map(|x| json!({"line": x.line, "message": x.message})).collect());
        Ok(vec![Data::json(j), Data::report(rep)])
    }
}

// ----------------------------------------------------------- remote upload

pub struct RemoteUpload;

impl NodeOp for RemoteUpload {
    fn info(&self) -> NodeTypeInfo {
        info("print.remote_upload", "Remote Upload", "Send a G-code file to a printer host with a checksum, then compare the host's time estimate with ours. Starting the print needs a separate confirmation.")
            .emulates("printer host upload of slicers")
            .implemented()
            .input(PortSpec::new("gcode", DataKind::File))
            .param(ParamSpec::bool("send", "Send", false).help("Off only checks the file; on uploads it to the host once and switches itself off again.").action())
            .param(ParamSpec::string("host_url", "Host address", "http://127.0.0.1:7125").help("Address of the printer host, including the port of its file service."))
            .param(ParamSpec::string("api_key", "Access key", "").help("Stored in the graph file in clear text; leave empty when the host needs none."))
            .param(ParamSpec::choice("root", "Folder root", "gcodes", &["gcodes", "config"]).help("Top-level folder on the host."))
            .param(ParamSpec::string("path", "Sub-folder", "").help("Folder under the root; the host creates it when missing."))
            .param(ParamSpec::bool("print", "Start print", false).help("Ask the host to start the job after the upload."))
            .param(ParamSpec::bool("confirm_print", "Confirm start", false).help("Both switches must be on before a print is started."))
            .param(ParamSpec::bool("check_metadata", "Compare estimate", true).help("Fetch the host's metadata and compare the time estimate."))
            .param(ParamSpec::float("timeout", "Timeout", 30.0, 1.0, 600.0).unit("s").help("Network timeout."))
            .output(PortSpec::new("gcode", DataKind::File))
            .output(PortSpec::new("result", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let path = file_input(inp, 0, "gcode")?;
        let mut rep = Report::new("Remote upload");
        let size = std::fs::metadata(&path).map_err(|e| ctx.fail(format!("cannot read {}: {e}", path.display())))?.len();
        rep.metric("local_size_bytes", Metric::new(size as f64, "bytes"));
        if !p.bool("send")? {
            rep.note(Severity::Warning, "upload skipped: Send is off");
            return Ok(vec![Data::File(path), Data::json(json!({"sent": false})), Data::report(rep)]);
        }
        let want_print = p.bool("print")?;
        let print = want_print && p.bool("confirm_print")?;
        if want_print && !print {
            rep.note(Severity::Warning, "the print was not started: Confirm start is off");
        }
        let timeout = Duration::from_secs_f64(p.float("timeout")?);
        let params = UploadParams { api_key: p.str("api_key")?, root: p.str("root")?, path: p.str("path")?.trim_matches('/'), print, checksum_override: None, timeout };
        let host = p.str("host_url")?;
        let o = upload::upload(host, &path, &params).map_err(|e| ctx.fail(e))?;
        rep.metric("http_status", Metric::new(f64::from(o.status), "code"));
        if o.status != 201 {
            let msg = o.body.pointer("/error/message").and_then(Value::as_str).unwrap_or("no details");
            return Err(ctx.fail(format!("the host refused the upload (HTTP {}): {msg}", o.status)));
        }
        rep.metric("size_match", Metric::new(if o.verified { 1.0 } else { 0.0 }, "flag").with_limit(1.0, false));
        rep.metric("print_started", Metric::new(if o.print_started { 1.0 } else { 0.0 }, "flag"));
        if !o.verified {
            rep.note(Severity::Error, format!("the host reports {:?} bytes, the local file has {size}", o.item_size));
        }
        let mut result = json!({"sent": true, "status": o.status, "item_path": o.item_path, "item_size": o.item_size, "print_started": o.print_started, "checksum": o.checksum});
        if p.bool("check_metadata")? {
            let name = o.item_path.clone().unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
            match upload::fetch_metadata(host, &name, params.api_key, timeout) {
                Ok(meta) => {
                    if let Some(remote) = meta.get("estimated_time").and_then(Value::as_f64) {
                        let local = gcode::summarize(open_gcode(ctx, &path)?, &SummaryOptions::default()).time_s;
                        let diff = (remote - local).abs() / local.max(1.0);
                        rep.metric("estimate_difference", Metric::new(diff, "ratio").with_limit(0.25, true));
                        if diff > 0.25 {
                            rep.note(Severity::Warning, format!("the host estimates {remote:.0} s, we estimate {local:.0} s"));
                        }
                    }
                    result["metadata"] = meta;
                }
                Err(e) => {
                    rep.note(Severity::Warning, format!("metadata not available: {e}"));
                }
            }
        }
        Ok(vec![Data::File(path), Data::json(result), Data::report(rep)])
    }
}

// ----------------------------------------------------------- script bridge

pub struct ScriptBridge;

impl NodeOp for ScriptBridge {
    fn info(&self) -> NodeTypeInfo {
        info("print.script_bridge", "Script Bridge", "Write a small example plugin script that marks where a slicer's scripting hooks would call back into this graph.")
            .emulates("slicer scripting plugin interface")
            .input(PortSpec::optional("graph", DataKind::Json))
            .param(ParamSpec::string("name", "Plugin name", "vernon_hook").help("File name stem of the script."))
            .output(PortSpec::new("script", DataKind::File))
            .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, ctx: &CookContext, _inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let stem: String = p.str("name")?.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        let stem = if stem.is_empty() { "vernon_hook".to_string() } else { stem };
        let f = work_dir(ctx)?.join(format!("{stem}.py"));
        let text = "# Example slicer plugin written by Vernon (placeholder).\n# The slicer's plugin interface is still changing, so this file only marks\n# the places where a hook would hand data back to the graph.\n\nHOOKS = [\n    \"slice_finished\",\n    \"gcode_exported\",\n    \"upload_finished\",\n]\n\n\ndef on_event(name, payload):\n    \"\"\"Called by the slicer; Vernon does not run this file itself.\"\"\"\n    return None\n";
        std::fs::write(&f, text)?;
        let mut rep = Report::new("Script bridge");
        rep.metric("hook_count", Metric::new(3.0, "count"));
        placeholder(&mut rep, "writes an inert example; the real node will generate hooks once the slicer's plugin interface is stable");
        Ok(vec![Data::File(f), Data::report(rep)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testutil::{cook, tmp};
    use crate::mesh::testutil::tri_box;
    use glam::DVec3;
    use vernon_graph::ParamValue;

    fn metric(d: &Data, name: &str) -> f64 {
        d.as_report().unwrap().metrics[name].value
    }

    fn write_gcode(name: &str, text: &str) -> PathBuf {
        let f = tmp(name).join("job.gcode");
        std::fs::write(&f, text).unwrap();
        f
    }

    #[test]
    fn package_write_then_read_nodes() {
        let g = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        let out = cook(&PackageWrite, "pw", &[Some(Data::geometry(g))], &[]).unwrap();
        assert_eq!(metric(&out[1], "part_count"), 1.0);
        assert!(out[1].as_report().unwrap().passes());
        let file = out[0].as_file().unwrap().clone();
        assert!(file.exists());
        let back = cook(&PackageRead, "pr", &[Some(Data::File(file))], &[]).unwrap();
        assert!((metric(&back[2], "volume_mm3") - 1000.0).abs() < 1e-6);
        assert_eq!(back[1].as_json().unwrap()["parts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn package_write_refuses_open_shell() {
        let mut g = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        g.prims.pop();
        assert!(cook(&PackageWrite, "pw_open", &[Some(Data::geometry(g))], &[]).is_err());
    }

    #[test]
    fn package_read_without_file_is_an_error() {
        assert!(cook(&PackageRead, "pr_none", &[], &[]).is_err());
    }

    #[test]
    fn profile_builder_writes_three_files() {
        let out = cook(&ProfileBuild, "prof", &[], &[("pellet", ParamValue::Bool(true))]).unwrap();
        let j = out[0].as_json().unwrap();
        for k in ["process", "printer", "filament"] {
            assert!(Path::new(j[k].as_str().unwrap()).exists());
        }
        let printer: Value = serde_json::from_str(&std::fs::read_to_string(j["printer"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(printer["gcode_flavor"], "klipper");
        assert_eq!(printer["pellet_modded_printer"], "1");
        assert!((metric(&out[1], "equivalent_filament_diameter_mm") - 1.75).abs() < 1e-3);
        assert!(cook(&ProfileBuild, "prof_bad", &[], &[("layer_height", ParamValue::Float(0.5))]).is_err());
    }

    #[test]
    fn profile_builder_checks_the_chamber_against_preset_and_material() {
        // The default open-frame preset cannot hold a chamber.
        let hot = [("chamber_c", ParamValue::Float(60.0))];
        let err = cook(&ProfileBuild, "prof_hot", &[], &hot).unwrap_err().to_string();
        assert!(err.contains("chamber"), "{err}");
        // A heated preset can; ASA wants about 45 deg C and gets it.
        let set = [("printer", ParamValue::Str("bed_350".into())), ("chamber_c", ParamValue::Float(60.0)), ("pressure_advance", ParamValue::Float(0.05))];
        let out = cook(&ProfileBuild, "prof_warm", &[], &set).unwrap();
        let rep = out[1].as_report().unwrap();
        assert_eq!(metric(&out[1], "chamber_temp_c"), 60.0);
        assert!(!rep.notes.iter().any(|(s, t)| *s == Severity::Warning && t.contains("chamber")));
        let j = out[0].as_json().unwrap();
        let filament: Value = serde_json::from_str(&std::fs::read_to_string(j["filament"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(filament["chamber_temperatures"][0], "60");
        assert_eq!(filament["pressure_advance"][0], "0.05");
        // A cold chamber for ASA is a warning, not a failure.
        let cold = cook(&ProfileBuild, "prof_cold", &[], &[]).unwrap();
        assert!(cold[1].as_report().unwrap().notes.iter().any(|(s, t)| *s == Severity::Warning && t.contains("chamber")));
        // Old scenes that name a preset by its former id still resolve.
        assert_eq!(crate::common::printer_bed("k2_plus", glam::DVec3::ZERO), glam::DVec3::splat(350.0));
    }

    #[test]
    fn slice_external_writes_command_when_not_run() {
        let g = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        let prof = cook(&ProfileBuild, "prof2", &[], &[]).unwrap();
        let out = cook(&SliceExternal, "slice_dry", &[None, Some(Data::geometry(g)), Some(prof[0].clone())], &[]).unwrap();
        let cmd = std::fs::read_to_string(out[0].as_file().unwrap()).unwrap();
        assert!(cmd.contains("--load-settings") && cmd.contains("--slice 0") && cmd.contains("--export-3mf"), "{cmd}");
        assert!(out[2].as_report().unwrap().notes.iter().any(|(s, _)| *s == Severity::Warning));
        assert!(cook(&SliceExternal, "slice_none", &[], &[]).is_err());
    }

    #[test]
    fn slice_external_runs_a_stand_in_program() {
        let exe = slicer::mock::mock_slicer(&tmp("mock_exe"));
        let g = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        let set: &[(&str, ParamValue)] = &[("run", ParamValue::Bool(true)), ("executable", ParamValue::Str(s(&exe)))];
        let out = cook(&SliceExternal, "slice_run", &[None, Some(Data::geometry(g))], set).unwrap();
        let f = out[0].as_file().unwrap();
        assert!(f.file_name().unwrap().to_string_lossy().ends_with("plate_1.gcode"), "{f:?}");
        assert_eq!(metric(&out[2], "exit_code"), 0.0);
        assert_eq!(metric(&out[2], "layer_count"), 1.0);
        assert!(out[1].as_json().unwrap()["command"].as_str().unwrap().contains("--outputdir"));
    }

    #[test]
    fn slice_external_without_a_program_only_warns() {
        let g = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        let set: &[(&str, ParamValue)] = &[("run", ParamValue::Bool(true)), ("executable", ParamValue::Str("no-such-program-vernon".into()))];
        let out = cook(&SliceExternal, "slice_nope", &[None, Some(Data::geometry(g))], set).unwrap();
        assert!(out[2].as_report().unwrap().notes.iter().any(|(_, t)| t.contains("no slicer program")));
    }

    #[test]
    fn gcode_summary_node() {
        let f = write_gcode("gs", "; estimated printing time (normal mode) = 10m 0s\nM83\n;LAYER_CHANGE\nG1 X10 E2\n;LAYER_CHANGE\nG1 X20 E2\n");
        let out = cook(&GcodeSummaryNode, "gs", &[Some(Data::File(f))], &[]).unwrap();
        assert_eq!(metric(&out[1], "layer_count"), 2.0);
        assert_eq!(metric(&out[1], "time_s"), 600.0);
        assert_eq!(metric(&out[1], "filament_mm"), 4.0);
        assert!(cook(&GcodeSummaryNode, "gs_none", &[], &[]).is_err());
        assert!(cook(&GcodeSummaryNode, "gs_missing", &[Some(Data::File(PathBuf::from("no/such/file.gcode")))], &[]).is_err());
    }

    #[test]
    fn flavour_check_node_klipper_vs_marlin() {
        let text = "EXCLUDE_OBJECT_DEFINE NAME=a\nEXCLUDE_OBJECT_DEFINE NAME=b\nEXCLUDE_OBJECT_START NAME=a\nG1 X1 E1\nEXCLUDE_OBJECT_END NAME=a\nEXCLUDE_OBJECT_START NAME=b\nG1 X2 E1\nEXCLUDE_OBJECT_END NAME=b\n";
        let f = write_gcode("fc", text);
        let ok = cook(&FlavourCheck, "fc_k", &[Some(Data::File(f.clone()))], &[]).unwrap();
        assert_eq!(metric(&ok[1], "violation_count"), 0.0);
        assert_eq!(metric(&ok[1], "object_define_count"), 2.0);
        let bad = cook(&FlavourCheck, "fc_m", &[Some(Data::File(f))], &[("flavour", ParamValue::Str("marlin".into())), ("strict", ParamValue::Bool(true))]).unwrap();
        assert!(metric(&bad[1], "violation_count") > 0.0);
        assert!(!bad[1].as_report().unwrap().passes());
    }

    #[test]
    fn remote_upload_is_off_by_default_and_works_against_a_mock_host() {
        let f = write_gcode("ru", "M73 P0 R2\nG1 X1 E1\n");
        let off = cook(&RemoteUpload, "ru_off", &[Some(Data::File(f.clone()))], &[]).unwrap();
        assert_eq!(off[1].as_json().unwrap()["sent"], false);
        let (url, h) = upload::mock::serve(2, 120.0);
        let set: &[(&str, ParamValue)] = &[("send", ParamValue::Bool(true)), ("host_url", ParamValue::Str(url)), ("print", ParamValue::Bool(true))];
        let out = cook(&RemoteUpload, "ru_on", &[Some(Data::File(f))], set).unwrap();
        assert_eq!(metric(&out[2], "http_status"), 201.0);
        assert_eq!(metric(&out[2], "size_match"), 1.0);
        assert_eq!(metric(&out[2], "print_started"), 0.0, "print needs the confirmation switch");
        assert!(out[2].as_report().unwrap().notes.iter().any(|(_, t)| t.contains("Confirm start")));
        // host says 120 s, the file says 120 s (M73 R2)
        assert!(metric(&out[2], "estimate_difference") < 0.01);
        h.join().unwrap();
    }

    #[test]
    fn remote_upload_unreachable_host_is_an_error() {
        let f = write_gcode("ru_down", "G1 X1 E1\n");
        let set: &[(&str, ParamValue)] = &[("send", ParamValue::Bool(true)), ("host_url", ParamValue::Str("http://127.0.0.1:1".into())), ("timeout", ParamValue::Float(1.0))];
        assert!(cook(&RemoteUpload, "ru_x", &[Some(Data::File(f))], set).is_err());
    }

    #[test]
    fn script_bridge_writes_an_inert_file() {
        let out = cook(&ScriptBridge, "sb", &[], &[("name", ParamValue::Str("my hook!".into()))]).unwrap();
        let f = out[0].as_file().unwrap();
        assert!(f.file_name().unwrap().to_string_lossy() == "myhook.py");
        assert!(std::fs::read_to_string(f).unwrap().contains("HOOKS"));
    }
}

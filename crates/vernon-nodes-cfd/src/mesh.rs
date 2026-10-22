//! Meshing nodes: dictionary writer, external mesh build, mesh/wall check.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use vernon_core::{Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::extproc::{run_chain, RunOpts};
use crate::foam::{self, Flavour, SnappySpec, FLAVOURS};
use crate::types::{domain_in, RefGeom};
use crate::util::{case_dir, ensure_dir, file_in, json_in, placeholder, write_file, FnNode};

// ------------------------------------------------------------------------ STL

/// ASCII STL of the given primitives, millimetres scaled to metres.
pub(crate) fn stl_ascii(name: &str, g: &Geometry, prims: &[usize]) -> String {
    let mut s = format!("solid {name}\n");
    for &pi in prims {
        let v = &g.prims[pi].vertices;
        if !g.prims[pi].closed || v.len() < 3 {
            continue;
        }
        for i in 1..v.len() - 1 {
            let (a, b, c) = (g.points[v[0] as usize] * 1e-3, g.points[v[i] as usize] * 1e-3, g.points[v[i + 1] as usize] * 1e-3);
            let n = (b - a).cross(c - a).normalize_or_zero();
            s += &format!("  facet normal {:e} {:e} {:e}\n    outer loop\n", n.x, n.y, n.z);
            for p in [a, b, c] {
                s += &format!("      vertex {:e} {:e} {:e}\n", p.x, p.y, p.z);
            }
            s += "    endloop\n  endfacet\n";
        }
    }
    s + &format!("endsolid {name}\n")
}

/// Prim indices per patch name, in order of first appearance.
pub(crate) fn prims_by_patch(g: &Geometry) -> Vec<(String, Vec<usize>)> {
    let names: Vec<String> = match g.prim_attribs.strs("patch") {
        Ok(n) => n.to_vec(),
        Err(_) => vec!["body".to_string(); g.prims.len()],
    };
    let mut out: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, n) in names.iter().enumerate() {
        match out.iter_mut().find(|(k, _)| k == n) {
            Some((_, v)) => v.push(i),
            None => out.push((n.clone(), vec![i])),
        }
    }
    out
}

// -------------------------------------------------------------- run sequences

pub(crate) fn mesh_steps(f: Flavour, np: u32, has_stl: bool) -> Vec<(String, Vec<String>)> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut steps = Vec::new();
    if has_stl {
        steps.push(("surfaceFeatures".to_string(), s(&[if f.old_feature_extract() { "surfaceFeatureExtract" } else { "surfaceFeatures" }])));
    }
    steps.push(("blockMesh".into(), s(&["blockMesh"])));
    if np > 1 {
        steps.push(("decomposePar".into(), s(&["decomposePar", "-force"])));
        steps.push(("snappyHexMesh".into(), vec!["mpirun".into(), "-np".into(), np.to_string(), "snappyHexMesh".into(), "-overwrite".into(), "-parallel".into()]));
        steps.push(("reconstructParMesh".into(), s(&["reconstructParMesh", "-constant"])));
    } else {
        steps.push(("snappyHexMesh".into(), s(&["snappyHexMesh", "-overwrite"])));
    }
    steps.push(("checkMesh".into(), s(&["checkMesh", "-allTopology", "-allGeometry"])));
    steps
}

pub(crate) fn solve_steps(f: Flavour, np: u32, potential: bool, topo: bool) -> Vec<(String, Vec<String>)> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut steps = Vec::new();
    if topo {
        steps.push(("topoSet".to_string(), s(&["topoSet"])));
    }
    if potential {
        steps.push(("potentialFoam".into(), s(&["potentialFoam"])));
    }
    let solver = f.solver_argv();
    if np > 1 {
        steps.push(("decomposePar".into(), s(&["decomposePar", "-force"])));
        let mut v: Vec<String> = vec!["mpirun".into(), "-np".into(), np.to_string()];
        v.extend(solver);
        v.push("-parallel".into());
        steps.push(("solver".into(), v));
        steps.push(("reconstructPar".into(), s(&["reconstructPar", "-latestTime"])));
    } else {
        steps.push(("solver".into(), solver));
    }
    steps
}

pub(crate) fn script_text(steps: &[(String, Vec<String>)]) -> String {
    let mut o = String::from("#!/bin/sh\ncd \"${0%/*}\" || exit 1\nset -e\n");
    for (name, cmd) in steps {
        o += &format!("{} > log.{name} 2>&1\n", cmd.join(" "));
    }
    o
}

// ------------------------------------------------------------------ mesh setup

fn setup_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.mesh_setup", "Mesh Dictionaries", Stage::Cfd)
        .input(PortSpec::optional("plan", DataKind::Json))
        .input(PortSpec::optional("domain", DataKind::Json))
        .input(PortSpec::optional("vehicle", DataKind::Geometry))
        .output(PortSpec::new("case", DataKind::File))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::choice("flavour", "OpenFOAM flavour", "foundation11", FLAVOURS).help("File names and keywords differ between releases"))
        .param(ParamSpec::int("n_layers", "Prism layers", 5, 0, 15))
        .param(ParamSpec::float("expansion_ratio", "Layer expansion ratio", 1.2, 1.1, 1.4))
        .param(ParamSpec::choice("layer_mode", "Layer sizing", "relative_final", &["relative_final", "first_layer_yplus"]).help("Final layer relative to the local cell, or first-layer height from the target y+"))
        .param(ParamSpec::float("final_layer_rel", "Final layer thickness", 0.5, 0.3, 1.0).help("Relative to the local cell size"))
        .param(ParamSpec::float("min_thickness_rel", "Minimum layer thickness", 0.1, 0.01, 0.5).help("Relative to the local cell size"))
        .param(ParamSpec::float("yplus_target", "Target y+ (first-layer mode)", 30.0, 1.0, 300.0))
        .param(ParamSpec::int("n_cells_between_levels", "Cells between levels", 3, 1, 5))
        .param(ParamSpec::float("feature_angle", "Resolve feature angle", 30.0, 0.0, 180.0).unit("deg"))
        .param(ParamSpec::float("max_non_ortho", "Max non-orthogonality", 65.0, 40.0, 75.0).unit("deg"))
        .param(ParamSpec::float("max_global_cells_m", "Max cells", 30.0, 0.1, 500.0).unit("million"))
        .param(ParamSpec::int("snap_iterations", "Snap iterations", 50, 10, 200))
        .param(ParamSpec::int("np", "Processes", 8, 1, 512))
        .help("Writes the background-mesh, surface-feature, refinement, prism-layer and quality dictionaries plus the vehicle surfaces (metres, one named solid per part) into an OpenFOAM case directory.")
        .emulates("CFD GUI front end: snappy mesh dictionary writer")
        .spec("docs/specs/automotive-cfd.md")
        .external("OpenFOAM")
        .implemented()
}

fn setup_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let flavour = Flavour::parse(p.str("flavour")?).ok_or_else(|| ctx.fail("unknown OpenFOAM flavour"))?;
    let plan_in = json_in(ins, 0)?;
    let rf = match plan_in.as_ref().and_then(|v| v.get("reference")) {
        Some(r) => RefGeom::from_json(r)?,
        None => RefGeom::reference_car(),
    };
    let domain = domain_in(ins, 1, &rf)?;
    let plan: Value = match plan_in {
        Some(v) => (*v).clone(),
        None => {
            let pp = Params::from_specs(&crate::tunnel::plan_info().params);
            let out = crate::tunnel::plan_cook(ctx, &[Some(Data::json(rf.to_json())), Some(Data::json(domain.to_json()))], &pp)?;
            out[0].as_json().map(|v| (**v).clone()).unwrap_or(Value::Null)
        }
    };
    let surfaces = inputs::opt_geometry(ins, 2);
    let case = case_dir(ctx, None)?;
    let mut report = Report::new("Mesh dictionaries");

    // surfaces
    let mut patches: Vec<String> = Vec::new();
    let mut written = 0usize;
    if let Some(g) = &surfaces {
        g.validate().map_err(|e| ctx.fail(e.to_string()))?;
        let tri = case.join("constant").join("triSurface");
        ensure_dir(&tri)?;
        for (name, prims) in prims_by_patch(g) {
            let keep: Vec<usize> = if domain.half_model {
                prims
                    .into_iter()
                    .filter(|&pi| g.prims[pi].vertices.iter().any(|&v| g.points[v as usize].y >= -1.0))
                    .collect()
            } else {
                prims
            };
            if keep.is_empty() {
                continue;
            }
            write_file(&tri.join(format!("{name}.stl")), &stl_ascii(&name, g, &keep))?;
            patches.push(name);
            written += 1;
        }
    } else {
        patches = plan["patches"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        report.note(Severity::Warning, "no vehicle surfaces wired: dictionaries were written but there is nothing to mesh yet");
    }
    if patches.is_empty() {
        patches.push("body".into());
    }

    let dx0 = plan["dx0_m"].as_f64().unwrap_or(0.32);
    let (u_tau, nu) = (plan["u_tau"].as_f64().unwrap_or(1.0), plan["nu"].as_f64().unwrap_or(1.5e-5));
    let n_layers = p.int("n_layers")? as u32;
    let expansion = p.float("expansion_ratio")?;
    let first_mode = p.str("layer_mode")? == "first_layer_yplus";
    let body_max = plan["levels"]["body"][1].as_i64().unwrap_or(5);
    let body_cell = dx0 / 2f64.powi(body_max as i32);
    let final_rel = p.float("final_layer_rel")?;
    let first_layer = if first_mode {
        p.float("yplus_target")? * nu / u_tau
    } else if n_layers > 0 {
        final_rel * body_cell / expansion.powi(n_layers as i32 - 1)
    } else {
        0.0
    };
    let loc = {
        let (lo, hi) = (domain.min_m, domain.max_m);
        [lo[0] + 0.5137 * dx0, 0.5 * (lo[1] + hi[1]) + 0.0137 * dx0, hi[2] - 0.5137 * dx0]
    };
    let spec = SnappySpec {
        plan: &plan,
        patches: patches.clone(),
        n_layers,
        expansion,
        first_layer_mode: first_mode,
        final_rel,
        first_layer_m: first_layer,
        min_thickness_rel: p.float("min_thickness_rel")?,
        n_cells_between: p.int("n_cells_between_levels")? as u32,
        feature_angle: p.float("feature_angle")?,
        max_global_cells: (p.float("max_global_cells_m")? * 1e6) as u64,
        max_local_cells: ((p.float("max_global_cells_m")? * 1e6) as u64).min(100_000_000),
        snap_iter: p.int("snap_iterations")? as u32,
        location: loc,
    };
    let np = p.int("np")? as u32;
    write_file(&case.join("system/blockMeshDict"), &foam::block_mesh_dict(&domain, dx0))?;
    write_file(&case.join("system/snappyHexMeshDict"), &foam::snappy_dict(&spec))?;
    write_file(&case.join("system/meshQualityDict"), &foam::mesh_quality_dict(p.float("max_non_ortho")?))?;
    write_file(&case.join("system/decomposeParDict"), &foam::decompose_par_dict(np))?;
    let (feat, feat_path) = foam::surface_features_dict(flavour, &patches);
    write_file(&case.join(feat_path), &feat)?;
    write_file(&case.join("Allrun.mesh"), &script_text(&mesh_steps(flavour, np, written > 0)))?;
    let manifest = json!({
        "flavour": flavour.name(), "np": np, "dx0_m": dx0, "patches": patches,
        "domain": domain.to_json(), "reference": rf.to_json(), "plan": plan,
    });
    write_file(&case.join("vernon_case.json"), &serde_json::to_string_pretty(&manifest).unwrap_or_default())?;

    let n = foam::base_cells(&domain, dx0);
    report
        .metric("base_cells", Metric::new((n[0] * n[1] * n[2]) as f64, "count"))
        .metric("base_nx", Metric::new(n[0] as f64, "count"))
        .metric("base_ny", Metric::new(n[1] as f64, "count"))
        .metric("base_nz", Metric::new(n[2] as f64, "count"))
        .metric("finest_cell_mm", Metric::new(plan["finest_cell_m"].as_f64().unwrap_or(0.0) * 1e3, "mm"))
        .metric("first_layer_mm", Metric::new(first_layer * 1e3, "mm"))
        .metric("first_layer_yplus_estimate", Metric::new(first_layer * u_tau / nu, "-"))
        .metric("surfaces_written", Metric::new(written as f64, "count").with_limit(1.0, false));
    if n_layers > 0 {
        let total = first_layer * (expansion.powi(n_layers as i32) - 1.0) / (expansion - 1.0).max(1e-9);
        report.metric("layer_total_over_cell", Metric::new(total / body_cell, "-"));
    }
    Ok(vec![Data::File(case), Data::report(report)])
}

// ------------------------------------------------------------------ mesh build

fn run_params(prefix_default_np: i64) -> Vec<ParamSpec> {
    vec![
        ParamSpec::bool("run", "Run now", false).help("Off by default: a re-cook must never launch hours of CPU. Turn on to run the tools."),
        ParamSpec::int("np", "Processes", prefix_default_np, 1, 512),
        ParamSpec::choice("runner", "Runner", "native", &["native", "wsl", "docker"]),
        ParamSpec::string("image", "Container image", "").help("Docker runner only"),
        ParamSpec::path("bin_dir", "Tool directory", &[], false).help("Folder with the OpenFOAM executables; empty searches the PATH"),
        ParamSpec::int("timeout_min", "Time limit", 0, 0, 100_000).unit("min").help("0 = none"),
    ]
}

pub(crate) fn run_opts(p: &Params) -> CookResult<RunOpts> {
    Ok(RunOpts {
        runner: p.str("runner")?.to_string(),
        image: p.str("image")?.to_string(),
        bin_dir: p.str("bin_dir")?.to_string(),
        timeout_min: p.int("timeout_min")?.max(0) as u64,
    })
}

pub(crate) fn manifest(case: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(case.join("vernon_case.json")).ok()?).ok()
}

fn build_info() -> NodeTypeInfo {
    let mut i = NodeTypeInfo::new("cfd.mesh_build", "Build Volume Mesh", Stage::Cfd)
        .input(PortSpec::new("case", DataKind::File))
        .output(PortSpec::new("case", DataKind::File))
        .output(PortSpec::new("report", DataKind::Report));
    for ps in run_params(8) {
        i = i.param(ps);
    }
    i.param(ParamSpec::int("max_cells_m", "Cell budget", 20, 1, 500).unit("million").help("The built mesh is flagged when it exceeds this"))
        .help("Runs the surface-feature, background-mesh, castellated/snap/layer and mesh-check tools in the case directory, one log per step. Off by default; without the tools it only reports what it would run.")
        .emulates("CFD GUI front end: volume meshing")
        .spec("docs/specs/automotive-cfd.md")
        .external("OpenFOAM")
}

fn build_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let case = file_in(ins, 0)?.ok_or(CookError::MissingInput { index: 0, name: "case".into() })?;
    if !case.join("system/blockMeshDict").exists() {
        return Err(ctx.fail("case has no system/blockMeshDict; connect the mesh dictionary writer"));
    }
    let flavour = manifest(&case)
        .and_then(|m| m["flavour"].as_str().and_then(Flavour::parse))
        .unwrap_or(Flavour::Foundation(11));
    let np = p.int("np")? as u32;
    let steps = mesh_steps(flavour, np, case.join("constant/triSurface").is_dir());
    write_file(&case.join("Allrun.mesh"), &script_text(&steps))?;
    let mut report = Report::new("Volume mesh build");
    let mut ran = false;
    if p.bool("run")? {
        let res = run_chain(&run_opts(p)?, &case, &steps);
        ran = res.failed.is_none();
        if let Some(f) = res.failed {
            report.note(Severity::Error, format!("mesh build stopped: {f}"));
        }
        report.metric("steps_completed", Metric::new(res.completed.len() as f64, "count"));
    } else {
        placeholder(
            &mut report,
            "run surfaceFeatures, blockMesh, snappyHexMesh and checkMesh in the case directory (run is off; Allrun.mesh lists the steps)",
        );
    }
    let built = case.join("constant/polyMesh/boundary").exists();
    report.metric("mesh_built", Metric::new(if built { 1.0 } else { 0.0 }, "-").with_limit(1.0, false));
    if let Some(stats) = read_check_mesh(&case) {
        if let Some(c) = stats.cells {
            let budget = p.float("max_cells_m").unwrap_or(20.0) * 1e6;
            report.metric("cells", Metric::new(c, "count").with_limit(budget, true));
        }
    }
    let _ = ran;
    Ok(vec![Data::File(case), Data::report(report)])
}

// ---------------------------------------------------------------- mesh quality

#[derive(Debug, Default, Clone, PartialEq)]
pub struct MeshStats {
    pub cells: Option<f64>,
    pub max_non_ortho: Option<f64>,
    pub failed_checks: Option<f64>,
    pub layer_cov_pct: Option<f64>,
    pub yplus_mean: Option<f64>,
}

fn after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.find(key).map(|i| &line[i + key.len()..])
}

fn first_number(s: &str) -> Option<f64> {
    s.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == 'E' || c == '+'))
        .find(|t| !t.is_empty())
        .and_then(|t| t.parse().ok())
}

/// Parse a `checkMesh` log (and the layer coverage printed by the mesher, if given).
pub fn parse_check_mesh(text: &str) -> MeshStats {
    let mut s = MeshStats::default();
    for line in text.lines() {
        let t = line.trim();
        if s.cells.is_none() {
            if let Some(r) = t.strip_prefix("cells:") {
                s.cells = first_number(r);
            }
        }
        if let Some(r) = after(t, "non-orthogonality Max:") {
            s.max_non_ortho = first_number(r);
        }
        if t.starts_with("Mesh OK") {
            s.failed_checks = Some(0.0);
        }
        if let Some(r) = after(t, "Failed ") {
            if r.contains("mesh check") {
                s.failed_checks = first_number(r);
            }
        }
    }
    s
}

/// Last "Extruding X out of Y faces (Z%)" percentage in a mesher log.
pub fn parse_layer_coverage(text: &str) -> Option<f64> {
    text.lines()
        .filter(|l| l.contains("Extruding") && l.contains("faces"))
        .filter_map(|l| l.rsplit('(').next().and_then(first_number))
        .next_back()
}

/// Mean of the average y+ over wall patches in the last time block of `yPlus.dat`.
pub fn parse_yplus_dat(text: &str) -> Option<f64> {
    let mut last_time: Option<String> = None;
    let mut rows: Vec<(String, f64)> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let tok: Vec<&str> = t.split_whitespace().collect();
        if tok.len() < 5 {
            continue;
        }
        if last_time.as_deref() != Some(tok[0]) {
            last_time = Some(tok[0].to_string());
            rows.clear();
        }
        if tok[1] != "ground" {
            if let Ok(avg) = tok[4].parse::<f64>() {
                rows.push((tok[1].to_string(), avg));
            }
        }
    }
    (!rows.is_empty()).then(|| rows.iter().map(|r| r.1).sum::<f64>() / rows.len() as f64)
}

fn read_check_mesh(dir: &Path) -> Option<MeshStats> {
    let log = [dir.join("log.checkMesh"), dir.join("logs/checkMesh.log")].into_iter().find(|p| p.is_file())?;
    let mut st = parse_check_mesh(&std::fs::read_to_string(log).ok()?);
    if let Ok(t) = std::fs::read_to_string(dir.join("log.snappyHexMesh")) {
        st.layer_cov_pct = parse_layer_coverage(&t);
    }
    if let Some(y) = find_files(&dir.join("postProcessing"), "yPlus.dat").into_iter().next_back() {
        st.yplus_mean = std::fs::read_to_string(y).ok().and_then(|t| parse_yplus_dat(&t));
    }
    Some(st)
}

/// All files named `name` under `root`, sorted.
pub(crate) fn find_files(root: &Path, name: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.file_name().is_some_and(|n| n == name) {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

fn quality_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.mesh_quality", "Mesh and Wall Check", Stage::Cfd)
        .input(PortSpec::optional("case", DataKind::File))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::float("non_ortho_warn", "Non-orthogonality warning", 65.0, 40.0, 75.0).unit("deg"))
        .param(ParamSpec::float("non_ortho_fail", "Non-orthogonality limit", 70.0, 50.0, 85.0).unit("deg"))
        .param(ParamSpec::float("layer_cov_min_pct", "Min layer coverage", 70.0, 0.0, 100.0).unit("%"))
        .param(ParamSpec::float("yplus_min", "y+ lower bound", 30.0, 0.0, 300.0))
        .param(ParamSpec::float("yplus_max", "y+ upper bound", 300.0, 30.0, 1000.0))
        .param(ParamSpec::float("cells_target_m", "Target cells (0 = skip)", 0.0, 0.0, 500.0).unit("million"))
        .param(ParamSpec::float("max_non_ortho", "Override: max non-orthogonality (-1 = from log)", -1.0, -1.0, 90.0).unit("deg"))
        .param(ParamSpec::float("cells", "Override: cell count (-1 = from log)", -1.0, -1.0, 1e9))
        .param(ParamSpec::float("layer_cov_pct", "Override: layer coverage (-1 = from log)", -1.0, -1.0, 100.0).unit("%"))
        .param(ParamSpec::float("yplus_mean", "Override: mean y+ (-1 = from log)", -1.0, -1.0, 10_000.0))
        .param(ParamSpec::float("failed_checks", "Override: failed checks (-1 = from log)", -1.0, -1.0, 100.0))
        .help("Reads the mesh-check and wall-distance logs of a case and gates the mesh: non-orthogonality (warn/fail), failed checks, prism-layer coverage, mean y+ band and cell budget. Manual values override the logs.")
        .emulates("CFD GUI front end: mesh quality report")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

fn quality_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let mut st = match file_in(ins, 0)? {
        Some(f) if f.is_dir() => read_check_mesh(&f).unwrap_or_default(),
        Some(f) if f.is_file() => {
            let mut s = parse_check_mesh(&std::fs::read_to_string(&f).map_err(|e| ctx.fail(e.to_string()))?);
            s.layer_cov_pct = None;
            s
        }
        _ => MeshStats::default(),
    };
    let ov = |name: &str| -> CookResult<Option<f64>> {
        let v = p.float(name)?;
        Ok((v >= 0.0).then_some(v))
    };
    st.max_non_ortho = ov("max_non_ortho")?.or(st.max_non_ortho);
    st.cells = ov("cells")?.or(st.cells);
    st.layer_cov_pct = ov("layer_cov_pct")?.or(st.layer_cov_pct);
    st.yplus_mean = ov("yplus_mean")?.or(st.yplus_mean);
    st.failed_checks = ov("failed_checks")?.or(st.failed_checks);
    Ok(vec![Data::report(evaluate_mesh(&st, p)?)])
}

fn evaluate_mesh(st: &MeshStats, p: &Params) -> CookResult<Report> {
    let mut r = Report::new("Mesh and wall check");
    let (warn, fail) = (p.float("non_ortho_warn")?, p.float("non_ortho_fail")?);
    let checked = st != &MeshStats::default();
    r.metric("mesh_checked", Metric::new(f64::from(checked), "-").with_limit(1.0, false));
    if !checked {
        r.note(Severity::Warning, "no mesh data found: build the mesh, or enter values by hand");
    }
    if let Some(c) = st.cells {
        r.metric("cells", Metric::new(c, "count"));
        let target = p.float("cells_target_m")? * 1e6;
        if target > 0.0 {
            r.metric("cell_ratio", Metric::new(c / target, "-").with_limit(1.2, true));
            if c / target < 0.5 {
                r.note(Severity::Warning, "cell count is under half of the target: the mesh is probably too coarse");
            }
            if c / target > 1.2 {
                r.note(Severity::Error, "cell count exceeds the target by more than 20%");
            }
        }
    }
    if let Some(n) = st.max_non_ortho {
        r.metric("max_non_ortho_deg", Metric::new(n, "deg").with_limit(fail, true));
        if n >= fail {
            r.note(Severity::Error, format!("maximum non-orthogonality {n:.1} deg reaches the {fail} deg limit"));
        } else if n >= warn {
            r.note(Severity::Warning, format!("maximum non-orthogonality {n:.1} deg is above the {warn} deg warning level; limit the Laplacian scheme"));
        }
    }
    if let Some(f) = st.failed_checks {
        r.metric("failed_checks", Metric::new(f, "count").with_limit(0.0, true));
    }
    if let Some(l) = st.layer_cov_pct {
        let min = p.float("layer_cov_min_pct")?;
        r.metric("layer_coverage_pct", Metric::new(l, "%").with_limit(min, false));
        if l < min {
            r.note(Severity::Warning, "prism-layer coverage is below the minimum");
        }
    }
    if let Some(y) = st.yplus_mean {
        let (lo, hi) = (p.float("yplus_min")?, p.float("yplus_max")?);
        let ok = (lo..=hi).contains(&y);
        r.metric("yplus_mean", Metric::new(y, "-"));
        r.metric("yplus_in_band", Metric::new(f64::from(ok), "-").with_limit(1.0, false));
        if !ok {
            r.note(Severity::Warning, format!("mean y+ {y:.0} is outside the wall-function band {lo:.0}-{hi:.0}"));
        }
    }
    Ok(r)
}

pub(crate) fn register(reg: &mut Registry) {
    reg.add(FnNode::new(setup_info, setup_cook));
    reg.add(FnNode::new(build_info, build_cook));
    reg.add(FnNode::new(quality_info, quality_cook));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Domain;
    use crate::util::testutil::*;
    use vernon_core::DVec3;
    use vernon_graph::ParamValue;

    fn tagged_vehicle() -> Geometry {
        let (body, wheels) = synthetic_car();
        let ins = vec![Some(Data::geometry(body)), Some(Data::geometry(wheels)), None];
        let p = Params::from_specs(&crate::vehicle::prep_info().params);
        let out = crate::vehicle::prep_cook(&ctx("mesh-prep"), &ins, &p).unwrap();
        (**out[0].as_geometry().unwrap()).clone()
    }

    #[test]
    fn g7_block_mesh_cells_and_level_size() {
        let d = Domain {
            min_m: [-13.8, 0.0, 0.0],
            max_m: [32.2, 9.49, 11.38],
            half_model: true,
            yaw_deg: 0.0,
            blockage_pct: 1.0,
            cross_section_m2: 216.0,
            ref_length_m: 4.6,
            up_l: 3.0,
            down_l: 6.0,
        };
        assert_eq!(foam::base_cells(&d, 0.32), [144, 30, 36]);
        let dict = foam::block_mesh_dict(&d, 0.32);
        assert!(dict.contains("hex (0 1 2 3 4 5 6 7) (144 30 36) simpleGrading (1 1 1)"));
        foam::check_syntax(&dict).unwrap();
        assert!((0.32f64 / 32.0 - 0.01).abs() < 1e-12);
        assert!(dict.contains("symmetryPlane"));
    }

    #[test]
    fn setup_writes_valid_dictionaries_and_stl() {
        let ctx = ctx("setup");
        let veh = tagged_vehicle();
        let p = Params::from_specs(&setup_info().params);
        let out = setup_cook(&ctx, &[None, None, Some(Data::geometry(veh))], &p).unwrap();
        let case = out[0].as_file().unwrap().clone();
        for f in [
            "system/blockMeshDict",
            "system/snappyHexMeshDict",
            "system/meshQualityDict",
            "system/decomposeParDict",
            "system/surfaceFeaturesDict",
        ] {
            let t = std::fs::read_to_string(case.join(f)).unwrap();
            foam::check_syntax(&t).unwrap_or_else(|e| panic!("{f}: {e}"));
        }
        let snappy = std::fs::read_to_string(case.join("system/snappyHexMeshDict")).unwrap();
        assert!(snappy.contains("nSurfaceLayers 5;"));
        assert!(snappy.contains("expansionRatio 1.2;"));
        assert!(snappy.contains("searchableBox"));
        assert!(snappy.contains("\"wheel_.*\""));
        assert!(!snappy.contains("regionBox"));
        let stl = std::fs::read_to_string(case.join("constant/triSurface/body.stl")).unwrap();
        assert!(stl.starts_with("solid body"));
        // millimetres became metres: the synthetic car is 4 m long
        let max_x = stl
            .lines()
            .filter_map(|l| l.trim().strip_prefix("vertex "))
            .filter_map(|l| l.split_whitespace().next().and_then(|x| x.parse::<f64>().ok()))
            .fold(f64::MIN, f64::max);
        assert!((max_x - 4.0).abs() < 1e-6, "{max_x}");
        assert!(case.join("constant/triSurface/wheel_fr.stl").exists());
        assert!(manifest(&case).is_some());
        let r = out[1].as_report().unwrap();
        assert!(r.passes(), "{r:?}");
        assert!(r.metrics["first_layer_mm"].value > 0.0);
    }

    #[test]
    fn setup_without_surfaces_warns_and_fails_the_gate() {
        let ctx = ctx("setup0");
        let p = Params::from_specs(&setup_info().params);
        let out = setup_cook(&ctx, &[], &p).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(!r.passes());
        assert!(r.notes.iter().any(|(s, _)| *s == Severity::Warning));
    }

    #[test]
    fn layer_modes_follow_the_two_key_rule() {
        let ctx = ctx("layers");
        let mut p = Params::from_specs(&setup_info().params);
        p.set("layer_mode", ParamValue::Str("first_layer_yplus".into()));
        let out = setup_cook(&ctx, &[], &p).unwrap();
        let case = out[0].as_file().unwrap();
        let t = std::fs::read_to_string(case.join("system/snappyHexMeshDict")).unwrap();
        assert!(t.contains("firstLayerThickness") && !t.contains("finalLayerThickness"));
        assert!(t.contains("relativeSizes false;"));
        // y+ 30 at 30 m/s over 4.6 m: 0.414 mm
        assert!((out[1].as_report().unwrap().metrics["first_layer_mm"].value / 0.414 - 1.0).abs() < 0.02);
        p.set("n_layers", ParamValue::Int(0));
        let out = setup_cook(&ctx, &[], &p).unwrap();
        let t = std::fs::read_to_string(out[0].as_file().unwrap().join("system/snappyHexMeshDict")).unwrap();
        assert!(t.contains("addLayers       false;"));
    }

    #[test]
    fn half_model_drops_the_far_side_surfaces() {
        let ctx = ctx("half");
        let veh = tagged_vehicle();
        let mut p = Params::from_specs(&setup_info().params);
        let out = setup_cook(&ctx, &[None, None, Some(Data::geometry(veh.clone()))], &p).unwrap();
        // wheel_fl sits at y < 0 and is dropped, wheel_fr is kept
        let case = out[0].as_file().unwrap();
        assert!(case.join("constant/triSurface/wheel_fr.stl").exists());
        assert!(!case.join("constant/triSurface/wheel_fl.stl").exists());
        // a full-car domain keeps everything
        let dom = Data::json(crate::tunnel::size_domain(&RefGeom::reference_car(), 1.0, 3.0, 6.0, 5.0 / 3.0, false, 0.0).to_json());
        p.set("flavour", ParamValue::Str("opencfd2312".into()));
        let ctx2 = crate::util::testutil::ctx("half-full");
        let out = setup_cook(&ctx2, &[None, Some(dom), Some(Data::geometry(veh))], &p).unwrap();
        assert!(out[0].as_file().unwrap().join("constant/triSurface/wheel_fl.stl").exists());
    }

    #[test]
    fn stl_has_one_facet_per_triangle() {
        let g = Geometry::make_box(DVec3::splat(1000.0));
        let all: Vec<usize> = (0..g.prims.len()).collect();
        let s = stl_ascii("cube", &g, &all);
        assert_eq!(s.matches("facet normal").count(), 12);
        assert!(s.trim_end().ends_with("endsolid cube"));
    }

    const CHECK_MESH: &str = "Mesh stats\n    points:           2100000\n    cells:            2000000\n\nChecking geometry...\n    Mesh non-orthogonality Max: 78.2 average: 9.1\n    Max skewness = 2.1 OK.\n\nFailed 2 mesh checks.\n";

    #[test]
    fn parses_check_mesh_and_layer_logs() {
        let s = parse_check_mesh(CHECK_MESH);
        assert_eq!(s.cells, Some(2_000_000.0));
        assert_eq!(s.max_non_ortho, Some(78.2));
        assert_eq!(s.failed_checks, Some(2.0));
        assert_eq!(parse_check_mesh("Mesh OK.\n").failed_checks, Some(0.0));
        assert_eq!(parse_layer_coverage("Extruding 100 out of 200 faces (50%).\nExtruding 180 out of 200 faces (90%).\n"), Some(90.0));
        let y = "# Time patch min max average\n0 body 10 400 80\n0 ground 1 2 3\n500 body 20 300 60\n500 wheel_fr 30 500 100\n500 ground 1 2 3\n";
        assert_eq!(parse_yplus_dat(y), Some(80.0));
    }

    #[test]
    fn g27_poor_mesh_warns_fails_and_flags_yplus() {
        let ctx = ctx("quality");
        let mut p = Params::from_specs(&quality_info().params);
        p.set("yplus_mean", ParamValue::Float(500.0));
        let log = ctx.work_dir.join("log.checkMesh");
        std::fs::write(&log, CHECK_MESH).unwrap();
        let out = quality_cook(&ctx, &[Some(Data::File(log))], &p).unwrap();
        let r = out[0].as_report().unwrap();
        assert!(!r.passes());
        assert_eq!(r.metrics["max_non_ortho_deg"].value, 78.2);
        assert_eq!(r.metrics["yplus_in_band"].value, 0.0);
        assert!(r.notes.iter().any(|(s, t)| *s == Severity::Error && t.contains("78.2")));
        // 67 degrees only warns
        let mut p2 = Params::from_specs(&quality_info().params);
        p2.set("max_non_ortho", ParamValue::Float(67.0));
        p2.set("failed_checks", ParamValue::Float(0.0));
        p2.set("yplus_mean", ParamValue::Float(120.0));
        let r = quality_cook(&ctx, &[None], &p2).unwrap();
        let r = r[0].as_report().unwrap();
        assert!(r.passes());
        assert!(r.notes.iter().any(|(s, _)| *s == Severity::Warning));
        // nothing to check fails closed
        let r = quality_cook(&ctx, &[None], &Params::from_specs(&quality_info().params)).unwrap();
        assert!(!r[0].as_report().unwrap().passes());
    }

    #[test]
    fn build_without_run_writes_script_and_fails_closed() {
        let ctx = ctx("build");
        let setup = setup_cook(&ctx, &[], &Params::from_specs(&setup_info().params)).unwrap();
        let case = setup[0].as_file().unwrap().clone();
        let p = Params::from_specs(&build_info().params);
        let out = build_cook(&ctx, &[Some(Data::File(case.clone()))], &p).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(!r.passes());
        assert!(r.notes.iter().any(|(_, t)| t.starts_with("white-box placeholder:")));
        let script = std::fs::read_to_string(case.join("Allrun.mesh")).unwrap();
        assert!(script.contains("blockMesh") && script.contains("checkMesh -allTopology -allGeometry"));
        // missing tool with run on is a reported error, not a panic or fake success
        let mut p = p;
        p.set("run", ParamValue::Bool(true));
        p.set("bin_dir", ParamValue::Str(ctx.work_dir.join("nowhere").display().to_string()));
        let out = build_cook(&ctx, &[Some(Data::File(case))], &p).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(r.notes.iter().any(|(s, t)| *s == Severity::Error && t.contains("not found")));
        assert!(!r.passes());
        assert!(build_cook(&ctx, &[Some(Data::File(ctx.work_dir.join("empty")))], &p).is_err());
    }

    #[test]
    fn step_sequences() {
        let f = Flavour::Foundation(11);
        let m: Vec<String> = mesh_steps(f, 8, true).into_iter().map(|s| s.0).collect();
        assert_eq!(m, ["surfaceFeatures", "blockMesh", "decomposePar", "snappyHexMesh", "reconstructParMesh", "checkMesh"]);
        let s = solve_steps(f, 4, true, true);
        assert_eq!(s.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["topoSet", "potentialFoam", "decomposePar", "solver", "reconstructPar"]);
        assert_eq!(s[3].1, ["mpirun", "-np", "4", "simpleFoam", "-parallel"]);
        assert_eq!(solve_steps(Flavour::Foundation(12), 1, false, false)[0].1[0], "foamRun");
    }
}

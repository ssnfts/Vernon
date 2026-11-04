//! Meshing nodes: dictionary writer, external mesh build, mesh/wall check.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use vernon_core::{Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::case::{owned_context, recipe_key};
use crate::extproc::{run_chain, run_chain_with_wsl, ChainResult, RunOpts, WslOpts};
use crate::foam::{self, Flavour, SnappySpec, FLAVOURS};
use crate::types::{domain_in, RefGeom};
use crate::util::{case_dir, case_input_fingerprint, ensure_dir, file_in, json_in, placeholder, read_mesh_evidence, stage_case, write_file, write_mesh_evidence, CaseStageKind, FnNode};

// ------------------------------------------------------------------ memory

/// Memory per million cells for a meshing run and a steady two-equation solve, GB.
/// The project note gives 1 to 2 GB per million cells as a rule of thumb, which makes
/// a 32 to 64 GB machine the size for a mesh of about 24 million cells.
pub const GB_PER_MCELL: (f64, f64) = (1.0, 2.0);

/// Low and high memory estimate in GB for a cell count in millions.
pub fn ram_estimate_gb(cells_millions: f64) -> (f64, f64) {
    (GB_PER_MCELL.0 * cells_millions, GB_PER_MCELL.1 * cells_millions)
}

/// Memory metrics and notes for a cell count in millions; `available_gb` of 0 skips the comparison.
pub(crate) fn ram_report(r: &mut Report, cells_millions: f64, available_gb: f64, what: &str) {
    let (lo, hi) = ram_estimate_gb(cells_millions);
    r.metric("ram_estimate_gb_low", Metric::new(lo, "GB"));
    r.metric("ram_estimate_gb_high", Metric::new(hi, "GB"));
    r.note(Severity::Info, format!("{what}: {cells_millions:.1} million cells needs roughly {lo:.0} to {hi:.0} GB of memory (1 to 2 GB per million cells)"));
    if available_gb > 0.0 {
        r.metric("ram_available_gb", Metric::new(available_gb, "GB"));
        if hi > available_gb {
            let sev = if lo > available_gb { Severity::Error } else { Severity::Warning };
            r.note(sev, format!("{what}: the high estimate of {hi:.0} GB exceeds the {available_gb:.0} GB available; lower the cell limit or coarsen the plan"));
        }
    }
}

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
    // The default check set defines "Mesh OK" for the gates. The optional
    // all-geometry checks also flag concave cells that layered meshes normally
    // have, so they would fail every such mesh.
    steps.push(("checkMesh".into(), s(&["checkMesh"])));
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

/// The transient recipe starts from its uniform fields: no potential-flow step.
pub(crate) fn transient_steps(np: u32) -> Vec<(String, Vec<String>)> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    if np > 1 {
        vec![
            ("decomposePar".into(), s(&["decomposePar", "-force"])),
            ("solver".into(), vec!["mpirun".into(), "-np".into(), np.to_string(), "pimpleFoam".into(), "-parallel".into()]),
            ("reconstructPar".into(), s(&["reconstructPar", "-latestTime"])),
        ]
    } else {
        vec![("solver".into(), s(&["pimpleFoam"]))]
    }
}

pub(crate) fn script_text(steps: &[(String, Vec<String>)]) -> String {
    let mut o = String::from("#!/bin/sh\ncd \"${0%/*}\" || exit 1\nset -e\n");
    for (name, cmd) in steps {
        o += &format!("{} > log.{name} 2>&1\n", cmd.join(" "));
    }
    o
}

// ------------------------------------------------------------------ mesh setup

/// Versioned writer inputs, not caller-supplied dictionary text or a scope label.
/// The plan is already held separately in the same input-bound case manifest.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct MeshingRecipe {
    version: u32,
    patches: Vec<String>,
    n_layers: u32,
    expansion: f64,
    first_layer_mode: bool,
    final_rel: f64,
    first_layer_m: f64,
    min_thickness_rel: f64,
    n_cells_between: u32,
    feature_angle: f64,
    max_global_cells: u64,
    max_local_cells: u64,
    snap_iter: u32,
    location: [f64; 3],
    max_non_ortho: f64,
}

impl MeshingRecipe {
    fn from_spec(spec: &SnappySpec<'_>, max_non_ortho: f64) -> Self {
        Self {
            version: 1,
            patches: spec.patches.clone(),
            n_layers: spec.n_layers,
            expansion: spec.expansion,
            first_layer_mode: spec.first_layer_mode,
            final_rel: spec.final_rel,
            first_layer_m: spec.first_layer_m,
            min_thickness_rel: spec.min_thickness_rel,
            n_cells_between: spec.n_cells_between,
            feature_angle: spec.feature_angle,
            max_global_cells: spec.max_global_cells,
            max_local_cells: spec.max_local_cells,
            snap_iter: spec.snap_iter,
            location: spec.location,
            max_non_ortho,
        }
    }

    fn spec<'a>(&self, plan: &'a Value) -> SnappySpec<'a> {
        SnappySpec {
            plan,
            patches: self.patches.clone(),
            n_layers: self.n_layers,
            expansion: self.expansion,
            first_layer_mode: self.first_layer_mode,
            final_rel: self.final_rel,
            first_layer_m: self.first_layer_m,
            min_thickness_rel: self.min_thickness_rel,
            n_cells_between: self.n_cells_between,
            feature_angle: self.feature_angle,
            max_global_cells: self.max_global_cells,
            max_local_cells: self.max_local_cells,
            snap_iter: self.snap_iter,
            location: self.location,
        }
    }

    fn admits_reference(&self) -> bool {
        let bounded = |value: f64, lo: f64, hi: f64| value.is_finite() && (lo..=hi).contains(&value);
        self.version == 1
            && self.patches.len() == 3
            && ["body", "wheel_fr", "wheel_rr"].iter().all(|name| self.patches.iter().filter(|patch| patch.as_str() == *name).count() == 1)
            && self.n_layers == 5
            && bounded(self.expansion, 1.1, 1.4)
            && bounded(self.final_rel, 0.3, 1.0)
            && bounded(self.first_layer_m, 1e-12, 1.0)
            && bounded(self.min_thickness_rel, 0.01, 0.5)
            && (1..=5).contains(&self.n_cells_between)
            && bounded(self.feature_angle, 0.0, 180.0)
            && (100_000..=500_000_000).contains(&self.max_global_cells)
            && (1..=self.max_global_cells.min(100_000_000)).contains(&self.max_local_cells)
            && (10..=200).contains(&self.snap_iter)
            && self.location.iter().all(|&coordinate| bounded(coordinate, -1e6, 1e6))
            && bounded(self.max_non_ortho, 40.0, 75.0)
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MeshingRecipeBox {
    name: String,
    min: [f64; 3],
    max: [f64; 3],
    level: u32,
}

/// Validate every plan value interpolated by snappy_dict, without relying on its
/// permissive defaults. In particular box names must remain plain identifiers.
fn reference_meshing_plan_valid(plan: &Value) -> bool {
    let level = |value: &Value| value.as_u64().is_some_and(|level| level <= 12);
    let levels = &plan["levels"];
    let Some(body) = levels["body"].as_array().filter(|body| body.len() == 2) else {
        return false;
    };
    if !body.iter().all(level)
        || body[0].as_u64() > body[1].as_u64()
        || !["wheel", "underbody", "kit"].iter().all(|key| level(&levels[*key]))
        || !matches!(plan["underbody_mode"].as_str(), Some("box" | "distance"))
        || !plan["underbody_dist_m"].as_f64().is_some_and(|distance| distance.is_finite() && (1e-6..=1e3).contains(&distance))
    {
        return false;
    }
    let Some(boxes) = plan["boxes"].as_array().filter(|boxes| boxes.len() <= 64) else {
        return false;
    };
    let mut names = std::collections::BTreeSet::new();
    for value in boxes {
        let Ok(b) = serde_json::from_value::<MeshingRecipeBox>(value.clone()) else {
            return false;
        };
        if b.name.is_empty()
            || b.name.len() > 64
            || !b.name.bytes().next().is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            || !b.name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || ["body", "wheel_fr", "wheel_rr", "underbody"].contains(&b.name.as_str())
            || !names.insert(b.name)
            || b.level > 12
            || !(0..3).all(|i| b.min[i].is_finite() && b.max[i].is_finite() && b.min[i].abs() <= 1e6 && b.max[i].abs() <= 1e6 && b.min[i] < b.max[i])
        {
            return false;
        }
    }
    true
}

/// A current builder hash alone cannot prove all reference surfaces were used.
/// Re-render bounded structured inputs and compare complete actual dictionaries;
/// legacy, malformed, unsupported or edited inputs remain usable but unqualified.
pub(crate) fn reference_meshing_recipe_matches(_ctx: &CookContext, case: &Path, manifest: &Value) -> CookResult<bool> {
    let Some(value) = manifest.get("meshing_recipe") else {
        return Ok(false);
    };
    let Ok(recipe) = serde_json::from_value::<MeshingRecipe>(value.clone()) else {
        return Ok(false);
    };
    let Some(flavour_name) = manifest["flavour"].as_str().filter(|name| FLAVOURS.contains(name)) else {
        return Ok(false);
    };
    let Some(flavour) = Flavour::parse(flavour_name) else {
        return Ok(false);
    };
    if !recipe.admits_reference()
        || manifest["patches"] != json!(recipe.patches)
        || !reference_meshing_plan_valid(&manifest["plan"])
    {
        return Ok(false);
    }
    let (features, feature_path) = foam::surface_features_dict(flavour, &recipe.patches);
    for (relative, expected) in [
        ("system/snappyHexMeshDict", foam::snappy_dict(&recipe.spec(&manifest["plan"]))),
        ("system/meshQualityDict", foam::mesh_quality_dict(recipe.max_non_ortho)),
        (feature_path, features),
    ] {
        if !mesh_text(case, relative, 1024 * 1024).is_ok_and(|actual| actual == expected) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn setup_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.mesh_setup", "Mesh Dictionaries", Stage::Cfd)
        .input(PortSpec::optional("plan", DataKind::Json))
        .input(PortSpec::optional("domain", DataKind::Json))
        .input(PortSpec::optional("vehicle", DataKind::Geometry))
        .output(PortSpec::new("case", DataKind::File))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::choice("flavour", "OpenFOAM flavour", "foundation11", FLAVOURS).help("File names and keywords differ between releases"))
        .param(ParamSpec::int("n_layers", "Prism layers", 5, 0, 15).help("Number of prism layers on the vehicle surfaces. 0 turns layers off."))
        .param(ParamSpec::float("expansion_ratio", "Layer expansion ratio", 1.2, 1.1, 1.4).help("Growth ratio between successive prism layers. Smaller is smoother but needs more layers for the same depth."))
        .param(ParamSpec::choice("layer_mode", "Layer sizing", "relative_final", &["relative_final", "first_layer_yplus"]).help("Final layer relative to the local cell, or first-layer height from the target y+"))
        .param(ParamSpec::float("final_layer_rel", "Final layer thickness", 0.5, 0.3, 1.0).help("Relative to the local cell size"))
        .param(ParamSpec::float("min_thickness_rel", "Minimum layer thickness", 0.1, 0.01, 0.5).help("Relative to the local cell size"))
        .param(ParamSpec::float("yplus_target", "Target y+ (first-layer mode)", 30.0, 1.0, 300.0).help("Target y+ that sets the first layer height when layer sizing is by y+."))
        .param(ParamSpec::int("n_cells_between_levels", "Cells between levels", 3, 1, 5).help("Buffer cells kept between refinement levels. More gives a smoother size transition and a larger mesh."))
        .param(ParamSpec::float("feature_angle", "Resolve feature angle", 30.0, 0.0, 180.0).unit("deg").help("Edges sharper than this angle are captured as features of the surface mesh, in degrees."))
        .param(ParamSpec::float("max_non_ortho", "Max non-orthogonality", 65.0, 40.0, 75.0).unit("deg").help("Largest face non-orthogonality the mesh quality dictionary tolerates, in degrees. The mesher backs the snap off where it would exceed it."))
        .param(ParamSpec::float("max_global_cells_m", "Max cells", 30.0, 0.1, 500.0).unit("million").help("Upper bound on the total cell count handed to the mesher, in millions. Memory use follows it, see the estimate in the report."))
        .param(ParamSpec::float("ram_gb", "Available memory", 0.0, 0.0, 4096.0).unit("GB").help("Memory of the machine that will mesh and solve. When the high estimate (2 GB per million cells at the cell limit) exceeds it the report warns; 0 skips the check."))
        .param(ParamSpec::int("snap_iterations", "Snap iterations", 50, 10, 200).help("Number of snapping iterations. More pulls the mesh closer to the surface and takes longer."))
        .param(ParamSpec::int("np", "Processes", 8, 1, 512).help("Number of processes the mesh is decomposed over when it is built."))
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
    for (path, text) in foam::mesh_only_system() {
        write_file(&case.join(path), &text)?;
    }
    write_file(&case.join("system/blockMeshDict"), &foam::block_mesh_dict(&domain, dx0))?;
    write_file(&case.join("system/snappyHexMeshDict"), &foam::snappy_dict(&spec))?;
    let max_non_ortho = p.float("max_non_ortho")?;
    write_file(&case.join("system/meshQualityDict"), &foam::mesh_quality_dict(max_non_ortho))?;
    write_file(&case.join("system/decomposeParDict"), &foam::decompose_par_dict(np))?;
    let (feat, feat_path) = foam::surface_features_dict(flavour, &patches);
    write_file(&case.join(feat_path), &feat)?;
    write_file(&case.join("Allrun.mesh"), &script_text(&mesh_steps(flavour, np, written > 0)))?;
    let meshing_recipe = MeshingRecipe::from_spec(&spec, max_non_ortho);
    let manifest = json!({
        "flavour": flavour.name(), "np": np, "dx0_m": dx0, "patches": patches,
        "domain": domain.to_json(), "reference": rf.to_json(), "plan": plan,
        "meshing_recipe": meshing_recipe,
    });
    write_file(&case.join("vernon_case.json"), &serde_json::to_string_pretty(&manifest).unwrap_or_default())?;

    ram_report(&mut report, p.float("max_global_cells_m")?, p.float("ram_gb")?, "at the cell limit");
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
        ParamSpec::bool("run", "Run now", false).help("Runs the tools once and switches itself off again, so a re-cook never relaunches hours of CPU.").action(),
        ParamSpec::int("np", "Processes", prefix_default_np, 1, 512).help("Number of processes the run is decomposed over."),
        ParamSpec::choice("runner", "Runner", "native", &["native", "wsl", "docker"]).help("Where the solver programs run: directly on this machine, in a Linux subsystem, or in a container."),
        ParamSpec::string("image", "Container image", "").help("Docker runner only"),
        ParamSpec::path("bin_dir", "Tool directory", &[], false).help("Folder with the OpenFOAM executables; empty searches the PATH"),
        ParamSpec::int("timeout_min", "Time limit", 720, 0, 100_000).unit("min").help("The run (and every process it started) is stopped after this long; 0 = one day"),
        ParamSpec::string("wsl_distribution", "Linux distribution", "").help("Linux subsystem runner only: the installed distribution to run in"),
        ParamSpec::string("wsl_user", "Linux user", "").help("Linux subsystem runner only: the account that runs the tools"),
        ParamSpec::string("wsl_environment_file", "Linux environment file", "").help("Linux subsystem runner only: absolute Linux path of the script that sets up the tools' environment"),
    ]
}

/// Run `steps` in an owned case. A Linux run needs its whole explicit context
/// before anything starts; its control files stay outside the movable case.
pub(crate) fn run_owned_chain(owned: &CookContext, p: &Params, case: &Path, steps: &[(String, Vec<String>)]) -> CookResult<ChainResult> {
    let opts = run_opts(p)?;
    if opts.runner != "wsl" {
        return Ok(run_chain(&opts, case, steps));
    }
    let wsl = WslOpts {
        distribution: p.str("wsl_distribution")?.to_string(),
        user: p.str("wsl_user")?.to_string(),
        environment_file: p.str("wsl_environment_file")?.to_string(),
    };
    for (field, value) in [("distribution", &wsl.distribution), ("user", &wsl.user), ("environment file", &wsl.environment_file)] {
        if value.trim().is_empty() {
            return Ok(ChainResult {
                failed: Some(format!("the Linux subsystem runner needs an explicit {field}; nothing was started")),
                ..Default::default()
            });
        }
    }
    let control = owned.work_dir.join(".wsl-control");
    ensure_dir(&control)?;
    Ok(run_chain_with_wsl(&opts, &wsl, case, &control, steps))
}

/// A path that never exists: the File output of a run without a usable result.
pub(crate) fn unavailable_output(owned: &CookContext) -> PathBuf {
    owned.work_dir.join("unavailable-output")
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
    for ps in run_params(1) {
        i = i.param(ps);
    }
    i.param(ParamSpec::int("max_cells_m", "Cell budget", 20, 1, 500).unit("million").help("The built mesh is flagged when it exceeds this"))
        .help("Runs the surface-feature, background-mesh, castellated/snap/layer and mesh-check tools in the case directory, one log per step. Off by default; without the tools it only reports what it would run.")
        .emulates("CFD GUI front end: volume meshing")
        .spec("docs/specs/automotive-cfd.md")
        .external("OpenFOAM")
}

/// In a case whose mesh passed [`mesh_certified`] after a run here.
const MESH_STAMP: &str = "vernon_mesh.stamp";
/// A current builder stamp binds the volume tables to their exact input case.
/// Geometry recognition is a separate requirement at the case writer.
pub(crate) fn certified_input_fingerprint(ctx: &CookContext, case: &Path) -> CookResult<Option<String>> {
    match std::fs::symlink_metadata(case.join(MESH_STAMP)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ctx.fail(format!("cannot inspect mesh input stamp: {e}"))),
        Ok(_) => {}
    }
    let stamp = mesh_text(case, MESH_STAMP, 64).map_err(|e| ctx.fail(e))?;
    let stamp = stamp.trim();
    if stamp.len() != 16 || !stamp.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(None);
    }
    let current = case_input_fingerprint(ctx, case)?;
    if stamp != current {
        return Ok(None);
    }
    read_mesh_evidence(ctx, case)?;
    Ok(Some(current))
}
const MESH_LOG_BYTES: u64 = 64 * 1024 * 1024;

fn mesh_text_metadata(case: &Path, path: &Path) -> Result<std::fs::Metadata, String> {
    for part in path.ancestors().take_while(|p| *p != case) {
        let meta = std::fs::symlink_metadata(part)
            .map_err(|e| format!("cannot inspect {}: {e}", part.display()))?;
        let linked = meta.file_type().is_symlink();
        #[cfg(windows)]
        let linked = {
            use std::os::windows::fs::MetadataExt;
            linked || meta.file_attributes() & 0x400 != 0
        };
        if linked || (part == path && !meta.is_file()) || (part != path && !meta.is_dir()) {
            return Err(format!("mesh evidence refuses links and nonordinary paths: {}", part.display()));
        }
    }
    std::fs::symlink_metadata(path).map_err(|e| format!("cannot inspect {}: {e}", path.display()))
}

/// Bounded immutable text read for the fresh log and the small input stamp.
/// Cached mesh measurements come from the bound record, never from this log.
pub(crate) fn mesh_text(case: &Path, relative: &str, limit: u64) -> Result<String, String> {
    let path = case.join(relative);
    let before = mesh_text_metadata(case, &path)?;
    let modified = before.modified().map_err(|e| format!("cannot inspect {} timestamp: {e}", path.display()))?;
    if before.len() > limit {
        return Err(format!("{} exceeds its {limit}-byte read budget", path.display()));
    }
    let mut reader = std::fs::File::open(&path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        if vernon_core::process::cancel_requested() {
            return Err("mesh evidence read canceled".into());
        }
        let count = buffer.len().min((limit - bytes.len() as u64).saturating_add(1) as usize);
        let read = reader.read(&mut buffer[..count])
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() as u64 > limit {
            return Err(format!("{} grew beyond its read budget", path.display()));
        }
    }
    let after = mesh_text_metadata(case, &path)?;
    let opened = reader.metadata().map_err(|e| format!("cannot inspect open {}: {e}", path.display()))?;
    for meta in [&after, &opened] {
        if meta.len() != before.len()
            || meta.modified().map_err(|e| format!("cannot inspect {} timestamp: {e}", path.display()))? != modified
            || bytes.len() as u64 != before.len()
        {
            return Err(format!("{} changed while reading mesh evidence", path.display()));
        }
    }
    String::from_utf8(bytes).map_err(|e| format!("{} is not UTF-8 text: {e}", path.display()))
}

/// Positive evidence for a built mesh: all five nonempty tables, and a fresh
/// mesh-check log with valid cells, a pass and both strict quality limits.
fn mesh_certified(case: &Path) -> (MeshStats, Result<(), String>) {
    let mesh = case.join("constant/polyMesh");
    for name in ["points", "faces", "owner", "neighbour", "boundary"] {
        if !std::fs::metadata(mesh.join(name)).is_ok_and(|m| m.is_file() && m.len() > 0) {
            return (MeshStats::default(), Err(format!("the mesh table {name} is missing or empty")));
        }
    }
    let log = match mesh_text(case, "logs/checkMesh.log", MESH_LOG_BYTES) {
        Ok(log) => log,
        Err(why) => return (MeshStats::default(), Err(why)),
    };
    let st = parse_check_mesh(&log);
    let verdict = if st.invalid_fields != 0 {
        Err("the mesh-check log holds invalid or conflicting values".to_string())
    } else if !st.cells.is_some_and(|c| c.is_finite() && c > 0.0 && c.fract() == 0.0) {
        Err("the mesh-check log has no positive cell count".to_string())
    } else if !st.mesh_ok || st.failed_checks != Some(0.0) {
        Err("the mesh check did not report success".to_string())
    } else if !st.max_non_ortho.is_some_and(|n| (0.0..70.0).contains(&n)) {
        Err("maximum non-orthogonality is missing or not below 70 deg".to_string())
    } else if !st.max_skewness.is_some_and(|s| (0.0..4.0).contains(&s)) {
        Err("maximum skewness is missing or not below 4".to_string())
    } else {
        Ok(())
    };
    (st, verdict)
}

fn build_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let source = file_in(ins, 0)?.ok_or(CookError::MissingInput { index: 0, name: "case".into() })?;
    if !source.join("system/blockMeshDict").exists() {
        return Err(ctx.fail("case has no system/blockMeshDict; connect the mesh dictionary writer"));
    }
    let owned = owned_context(ctx);
    let requested = p.bool("run")?;
    let np = p.int("np")? as u32;
    let mut report = Report::new("Volume mesh build");
    report.metric("external_run_requested", Metric::new(f64::from(requested), "-"));
    let refuse = |mut report: Report, steps: usize, why: String| {
        report.note(Severity::Error, why);
        report
            .metric("steps_completed", Metric::new(steps as f64, "count"))
            .metric("external_run_succeeded", Metric::new(0.0, "-"))
            .metric("external_run_failed", Metric::new(f64::from(requested), "-"))
            .metric("mesh_built", Metric::new(0.0, "-").with_limit(1.0, false));
        vec![Data::File(unavailable_output(&owned)), Data::report(report)]
    };
    // A requested run starts from a fresh generation of the inputs alone.
    let key = recipe_key("mesh-inputs/v1", ins, p)?;
    let staged = match stage_case(&owned, &source, CaseStageKind::MeshInputs, &key, requested) {
        Ok(staged) => staged,
        Err(e) if requested => return Ok(refuse(report, 0, format!("mesh build not started: {e}"))),
        Err(e) => return Err(e),
    };
    let case = staged.path;
    let flavour = manifest(&case)
        .and_then(|m| m["flavour"].as_str().and_then(Flavour::parse))
        .unwrap_or(Flavour::Foundation(11));
    let steps = mesh_steps(flavour, np, case.join("constant/triSurface").is_dir());
    if !staged.reused {
        write_file(&case.join("system/decomposeParDict"), &foam::decompose_par_dict(np))?;
        write_file(&case.join("Allrun.mesh"), &script_text(&steps))?;
    }
    let (stats, verdict) = if requested {
        let res = run_owned_chain(&owned, p, &case, &steps)?;
        let completed = res.completed.len();
        if let Some(f) = res.failed {
            return Ok(refuse(report, completed, format!("mesh build stopped: {f}")));
        }
        let (stats, verdict) = mesh_certified(&case);
        if let Err(why) = verdict {
            return Ok(refuse(report, completed, format!("the tools exited cleanly but the mesh is not certified: {why}")));
        }
        let publish = (|| -> CookResult<()> {
            // The log reader uses f64, so reject values outside its exact
            // integer range rather than saturating an unchecked u64 cast.
            let cells = stats.cells.filter(|c| *c > 0.0 && *c <= 9_007_199_254_740_991.0 && c.fract() == 0.0)
                .ok_or_else(|| owned.fail("measured mesh cells are outside the exact supported integer range"))?;
            let angle = stats.max_non_ortho.ok_or_else(|| owned.fail("missing measured non-orthogonality"))?;
            let skew = stats.max_skewness.ok_or_else(|| owned.fail("missing measured skewness"))?;
            write_mesh_evidence(&owned, &case, cells as u64, angle, skew)?;
            // The constant inventory includes the new record; the root stamp
            // is excluded, so this ordering avoids a self-invalidating stamp.
            let fingerprint = case_input_fingerprint(&owned, &case)?;
            read_mesh_evidence(&owned, &case)?;
            write_file(&case.join(MESH_STAMP), &fingerprint)
        })();
        if let Err(why) = publish {
            return Ok(refuse(report, completed, format!("the measured mesh evidence cannot be published: {why}")));
        }
        report.metric("steps_completed", Metric::new(completed as f64, "count"));
        (stats, Ok(()))
    } else if case.join(MESH_STAMP).exists() {
        let cached = (|| -> CookResult<MeshStats> {
            let stamp = mesh_text(&case, MESH_STAMP, 64).map_err(|e| owned.fail(e))?;
            if stamp.trim() != case_input_fingerprint(&owned, &case)? {
                return Err(owned.fail("cached mesh stamp is legacy or no longer matches the case inputs"));
            }
            let evidence = read_mesh_evidence(&owned, &case)?;
            Ok(MeshStats {
                cells: Some(evidence.cells as f64),
                max_non_ortho: Some(evidence.max_nonortho),
                max_skewness: Some(evidence.max_skew),
                mesh_ok: true,
                failed_checks: Some(0.0),
                ..MeshStats::default()
            })
        })();
        match cached {
            Ok(stats) => (stats, Ok(())),
            Err(why) => return Ok(refuse(report, 0, format!("cached mesh cannot be certified: {why}"))),
        }
    } else if case.join("constant/polyMesh").exists() || case.join("logs").exists() {
        return Ok(refuse(report, 0, "the last run of these inputs left no certified mesh; run again".into()));
    } else {
        placeholder(
            &mut report,
            "run surfaceFeatures, blockMesh, snappyHexMesh and checkMesh in the case directory (run is off; Allrun.mesh lists the steps)",
        );
        (MeshStats::default(), Err(String::new()))
    };
    let built = verdict.is_ok();
    report
        .metric("external_run_succeeded", Metric::new(f64::from(requested && built), "-"))
        .metric("external_run_failed", Metric::new(0.0, "-"))
        .metric("mesh_built", Metric::new(f64::from(built), "-").with_limit(1.0, false));
    if built {
        if let Some(c) = stats.cells {
            let budget = p.float("max_cells_m").unwrap_or(20.0) * 1e6;
            report.metric("cells", Metric::new(c, "count").with_limit(budget, true));
        }
        if let Some(n) = stats.max_non_ortho {
            report.metric("max_non_ortho_deg", Metric::new(n, "deg").with_limit(70.0, true));
        }
        if let Some(s) = stats.max_skewness {
            report.metric("max_skewness", Metric::new(s, "-").with_limit(4.0, true));
        }
    }
    Ok(vec![Data::File(case), Data::report(report)])
}

// ---------------------------------------------------------------- mesh quality

#[derive(Debug, Default, Clone, PartialEq)]
pub struct MeshStats {
    pub cells: Option<f64>,
    pub max_non_ortho: Option<f64>,
    pub max_skewness: Option<f64>,
    pub mesh_ok: bool,
    pub invalid_fields: u8,
    pub manual_overrides: usize,
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

const BAD_CELLS:u8=1;
const BAD_NON_ORTHO:u8=2;
const BAD_SKEWNESS:u8=4;
const BAD_STATUS:u8=8;

fn mesh_number(slot:&mut Option<f64>,text:&str,bad:&mut u8,field:u8,count:bool) {
    // A failing check prints "Max skewness = 5.3, 8 highly skew faces".
    let value=text.split_whitespace().next().and_then(|t|t.trim_end_matches(',').parse::<f64>().ok());
    let Some(n)=value else{*bad|=field;return};
    if !n.is_finite()||n<0.0||(count&&n.fract()!=0.0){*bad|=field;}
    if let Some(old)=slot {
        if *old!=n {*bad|=field;}
    }else{*slot=Some(n);}
}

/// Parse a `checkMesh` log (and the layer coverage printed by the mesher, if given).
pub fn parse_check_mesh(text: &str) -> MeshStats {
    let mut s = MeshStats::default();
    for line in text.lines() {
        let t = line.trim();
        if let Some(r) = t.strip_prefix("cells:") {
            mesh_number(&mut s.cells,r,&mut s.invalid_fields,BAD_CELLS,true);
        }
        if let Some(r) = after(t, "non-orthogonality Max:") {
            mesh_number(&mut s.max_non_ortho,r,&mut s.invalid_fields,BAD_NON_ORTHO,false);
        }
        if let Some(r) = after(t, "Max skewness =") {
            mesh_number(&mut s.max_skewness,r,&mut s.invalid_fields,BAD_SKEWNESS,false);
        }
        if matches!(t,"Mesh OK"|"Mesh OK.") {
            s.mesh_ok = true;
            mesh_number(&mut s.failed_checks,"0",&mut s.invalid_fields,BAD_STATUS,true);
        }
        if let Some(r) = after(t, "Failed ") {
            if r.contains("mesh check") {
                mesh_number(&mut s.failed_checks,r,&mut s.invalid_fields,BAD_STATUS,true);
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

/// Mean of the average y+ over the body patches (`body`, `body_*`) in the last
/// time block of `yPlus.dat`. Wheels, ground and other walls are left out, so
/// this is the body-only wall-treatment figure the spec gates on.
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
        if tok[1] == "body" || tok[1].starts_with("body_") {
            if let Ok(avg) = tok[4].parse::<f64>() {
                rows.push((tok[1].to_string(), avg));
            }
        }
    }
    (!rows.is_empty()).then(|| rows.iter().map(|r| r.1).sum::<f64>() / rows.len() as f64)
}

fn read_check_mesh(dir: &Path) -> Option<MeshStats> {
    let log = [dir.join("logs/checkMesh.log"), dir.join("log.checkMesh")].into_iter().find(|p| p.is_file())?;
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
        .param(ParamSpec::float("non_ortho_warn", "Non-orthogonality warning", 65.0, 40.0, 75.0).unit("deg").help("Worst non-orthogonality at or above this adds a warning, in degrees. Limit the Laplacian scheme when it fires."))
        .param(ParamSpec::float("non_ortho_fail", "Non-orthogonality limit", 70.0, 50.0, 70.0).unit("deg").help("Worst non-orthogonality at or above this fails the check. The absolute maximum is 70 degrees. Rebuild the mesh with less aggressive snapping or layers."))
        .param(ParamSpec::float("layer_cov_min_pct", "Min layer coverage", 70.0, 0.0, 100.0).unit("%").help("Smallest acceptable share of wall area covered by prism layers. Below it the report warns that the wall treatment is unreliable."))
        .param(ParamSpec::float("yplus_min", "y+ lower bound", 30.0, 0.0, 300.0).help("Lower edge of the wall-function y+ band. A mean y+ below it adds a warning."))
        .param(ParamSpec::float("yplus_max", "y+ upper bound", 300.0, 30.0, 1000.0).help("Upper edge of the wall-function y+ band. A mean y+ above it adds a warning."))
        .param(ParamSpec::float("cells_target_m", "Target cells (0 = skip)", 0.0, 0.0, 500.0).unit("million").help("Intended cell count in millions. 0 skips the check. A count under half of it warns that the mesh is too coarse; over 120 % is an error."))
        .param(ParamSpec::float("ram_gb", "Available memory", 0.0, 0.0, 4096.0).unit("GB").help("Memory of the machine that will solve. When the high estimate (2 GB per million cells) for the built cell count exceeds it the report warns, or fails when even the low estimate (1 GB per million) does; 0 skips the check."))
        .param(ParamSpec::float("max_non_ortho", "Override: max non-orthogonality (-1 = from log)", -1.0, -1.0, 90.0).unit("deg").help("Worst non-orthogonality in degrees entered by hand instead of read from the mesh-check log. -1 uses the log."))
        .param(ParamSpec::float("max_skewness", "Override: max skewness (-1 = from log)", -1.0, -1.0, 1000.0).help("Maximum mesh skewness entered by hand. -1 reads the log. Values at or above 4 fail the check."))
        .param(ParamSpec::float("cells", "Override: cell count (-1 = from log)", -1.0, -1.0, 1e9).help("Cell count entered by hand instead of read from the mesh-check log. -1 uses the log."))
        .param(ParamSpec::float("layer_cov_pct", "Override: layer coverage (-1 = from log)", -1.0, -1.0, 100.0).unit("%").help("Prism-layer coverage in percent entered by hand instead of read from the log. -1 uses the log."))
        .param(ParamSpec::float("yplus_mean", "Override: mean y+ (-1 = from log)", -1.0, -1.0, 10_000.0).help("Mean wall y+ entered by hand instead of read from the log. -1 uses the log."))
        .param(ParamSpec::float("failed_checks", "Override: failed checks (-1 = from log)", -1.0, -1.0, 100.0).help("Number of failed mesh checks entered by hand instead of read from the log. -1 uses the log."))
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
    if let Some(v)=ov("max_non_ortho")? {st.max_non_ortho=Some(v);st.invalid_fields&=!BAD_NON_ORTHO;st.manual_overrides+=1;}
    if let Some(v)=ov("max_skewness")? {st.max_skewness=Some(v);st.invalid_fields&=!BAD_SKEWNESS;st.manual_overrides+=1;}
    if let Some(v)=ov("cells")? {st.cells=Some(v);st.invalid_fields&=!BAD_CELLS;st.manual_overrides+=1;}
    if let Some(v) = ov("layer_cov_pct")? {
        st.layer_cov_pct = Some(v);
        st.manual_overrides += 1;
    }
    if let Some(v) = ov("yplus_mean")? {
        st.yplus_mean = Some(v);
        st.manual_overrides += 1;
    }
    if let Some(v)=ov("failed_checks")? {
        st.failed_checks=Some(v); st.mesh_ok=v==0.0; st.invalid_fields&=!BAD_STATUS;st.manual_overrides+=1;
    }
    Ok(vec![Data::report(evaluate_mesh(&st, p)?)])
}

fn evaluate_mesh(st: &MeshStats, p: &Params) -> CookResult<Report> {
    let mut r = Report::new("Mesh and wall check");
    let (warn, fail) = (p.float("non_ortho_warn")?, p.float("non_ortho_fail")?.min(70.0));
    let nonnegative=|v:Option<f64>|v.is_some_and(|n|n.is_finite()&&n>=0.0);
    let count=|v:Option<f64>|v.is_some_and(|n|n.is_finite()&&n>=0.0&&n.fract()==0.0);
    let checked = count(st.cells)&&st.cells.is_some_and(|c|c>0.0)&&nonnegative(st.max_non_ortho)
        &&nonnegative(st.max_skewness)&&count(st.failed_checks)&&st.mesh_ok&&st.invalid_fields==0;
    r.metric("mesh_checked", Metric::new(f64::from(checked), "-").with_limit(1.0, false));
    if !checked {
        r.note(Severity::Warning, "mesh evidence is incomplete, invalid or unsuccessful: require positive cells, non-orthogonality, skewness and a successful mesh check (or complete manual values)");
    }
    if st.invalid_fields!=0 {r.note(Severity::Error,"mesh-check log contains invalid or conflicting required values");}
    r.metric("manual_mesh_overrides",Metric::new(st.manual_overrides as f64,"count"));
    if st.manual_overrides>0 {r.note(Severity::Info,"manual mesh values override log evidence; no new external mesh check is certified by these overrides");}
    if let Some(c) = st.cells.filter(|v|v.is_finite()&&*v>0.0&&v.fract()==0.0) {
        r.metric("cells", Metric::new(c, "count"));
        ram_report(&mut r, c / 1e6, p.float("ram_gb")?, "for the built mesh");
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
    if let Some(n) = st.max_non_ortho.filter(|v|v.is_finite()&&*v>=0.0) {
        r.metric("max_non_ortho_deg", Metric::new(n, "deg").with_limit(fail, true));
        r.metric("non_ortho_below_limit",Metric::new(f64::from(n<fail),"-").with_limit(1.0,false));
        if n >= fail {
            r.note(Severity::Error, format!("maximum non-orthogonality {n:.1} deg reaches the {fail} deg limit"));
        } else if n >= warn {
            r.note(Severity::Warning, format!("maximum non-orthogonality {n:.1} deg is above the {warn} deg warning level; limit the Laplacian scheme"));
        }
    }
    if let Some(s)=st.max_skewness.filter(|v|v.is_finite()&&*v>=0.0) {
        r.metric("max_skewness",Metric::new(s,"-"));
        r.metric("skewness_below_limit",Metric::new(f64::from(s<4.0),"-").with_limit(1.0,false));
        if s>=4.0 {r.note(Severity::Error,format!("maximum skewness {s} reaches the strict limit 4"));}
    }
    if let Some(f) = st.failed_checks.filter(|v|v.is_finite()&&*v>=0.0&&v.fract()==0.0) {
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

    // Deliberately independent of the production serializer/reader: a structured
    // fixture rendered through the existing dictionary writer is positive proof.
    fn wp9_recipe_fixture(consumer: &CookContext) -> (PathBuf, Value) {
        let case = consumer.work_dir.join("recipe-case");
        ensure_dir(&case.join("system")).unwrap();
        let manifest = json!({
            "flavour": "foundation11", "patches": ["body", "wheel_fr", "wheel_rr"],
            "plan": {
                "dx0_m": 0.32,
                "levels": {"body": [4, 5], "wheel": 5, "underbody": 6, "kit": 6},
                "boxes": [{"name": "nearWake", "min": [4.0, 0.0, 0.0], "max": [8.0, 1.2, 1.8], "level": 4}],
                "underbody_mode": "box", "underbody_dist_m": 0.1
            },
            "meshing_recipe": {
                "version": 1, "patches": ["body", "wheel_fr", "wheel_rr"],
                "n_layers": 5, "expansion": 1.2, "first_layer_mode": false,
                "final_rel": 0.5, "first_layer_m": 0.0024112654320987654,
                "min_thickness_rel": 0.1, "n_cells_between": 3, "feature_angle": 30.0,
                "max_global_cells": 30000000, "max_local_cells": 30000000,
                "snap_iter": 50, "location": [-12.0, 4.0, 10.0], "max_non_ortho": 65.0
            }
        });
        wp9_recipe_render(&case, &manifest);
        (case, manifest)
    }

    // This permissive fixture renderer also writes edited inputs exactly as the
    // existing renderer accepts them, so malformed structured controls cannot be
    // rejected merely because the actual dictionary happened not to match.
    fn wp9_recipe_render(case: &Path, manifest: &Value) {
        let r = &manifest["meshing_recipe"];
        let f = |key: &str| r[key].as_f64().unwrap_or(f64::NAN);
        let u = |key: &str| r[key].as_u64().unwrap_or(0);
        let patches: Vec<String> = r["patches"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        let spec = SnappySpec {
            plan: &manifest["plan"], patches: patches.clone(), n_layers: u("n_layers") as u32,
            expansion: f("expansion"), first_layer_mode: r["first_layer_mode"].as_bool().unwrap_or(false),
            final_rel: f("final_rel"), first_layer_m: f("first_layer_m"), min_thickness_rel: f("min_thickness_rel"),
            n_cells_between: u("n_cells_between") as u32, feature_angle: f("feature_angle"),
            max_global_cells: u("max_global_cells"), max_local_cells: u("max_local_cells"), snap_iter: u("snap_iter") as u32,
            location: std::array::from_fn(|i| r["location"][i].as_f64().unwrap_or(f64::NAN)),
        };
        write_file(&case.join("system/snappyHexMeshDict"), &foam::snappy_dict(&spec)).unwrap();
        write_file(&case.join("system/meshQualityDict"), &foam::mesh_quality_dict(f("max_non_ortho"))).unwrap();
        let flavour = Flavour::parse(manifest["flavour"].as_str().unwrap()).unwrap();
        let (features, path) = foam::surface_features_dict(flavour, &patches);
        write_file(&case.join(path), &features).unwrap();
        write_file(&case.join("vernon_case.json"), &manifest.to_string()).unwrap();
    }

    #[test]
    fn wp9_reference_recipe_accepts_independent_structured_fixture() {
        let consumer = ctx("wp9-recipe-positive");
        let (case, mut manifest) = wp9_recipe_fixture(&consumer);
        assert!(reference_meshing_recipe_matches(&consumer, &case, &manifest).unwrap());
        manifest["meshing_recipe"]["first_layer_mode"] = json!(true);
        wp9_recipe_render(&case, &manifest);
        assert!(reference_meshing_recipe_matches(&consumer, &case, &manifest).unwrap());
        manifest["flavour"] = json!("opencfd2312");
        wp9_recipe_render(&case, &manifest);
        assert!(reference_meshing_recipe_matches(&consumer, &case, &manifest).unwrap());
    }

    #[test]
    fn wp9_reference_recipe_setup_serializes_actual_controls() {
        let consumer = ctx("wp9-recipe-setup");
        let mut p = Params::from_specs(&setup_info().params);
        p.set("expansion_ratio", ParamValue::Float(1.3));
        p.set("feature_angle", ParamValue::Float(42.0));
        p.set("max_non_ortho", ParamValue::Float(62.0));
        let out = setup_cook(&consumer, &[None, None, Some(Data::geometry(tagged_vehicle()))], &p).unwrap();
        let case = out[0].as_file().unwrap();
        let manifest = manifest(case).unwrap();
        assert_eq!(manifest["meshing_recipe"]["version"], 1);
        assert_eq!(manifest["meshing_recipe"]["expansion"], 1.3);
        assert_eq!(manifest["meshing_recipe"]["feature_angle"], 42.0);
        assert_eq!(manifest["meshing_recipe"]["max_non_ortho"], 62.0);
        assert!(reference_meshing_recipe_matches(&consumer, case, &manifest).unwrap());
    }

    #[test]
    fn wp9_reference_recipe_rejects_missing_legacy_and_unsupported_metadata() {
        let consumer = ctx("wp9-recipe-metadata");
        let (case, manifest) = wp9_recipe_fixture(&consumer);
        for bad in [Value::Null, json!("trusted"), json!({"version": 0}), json!({"version": 2})] {
            let mut changed = manifest.clone();
            changed["meshing_recipe"] = bad;
            assert!(!reference_meshing_recipe_matches(&consumer, &case, &changed).unwrap());
        }
        for version in [0, 2] {
            let mut changed = manifest.clone();
            changed["meshing_recipe"]["version"] = json!(version);
            assert!(!reference_meshing_recipe_matches(&consumer, &case, &changed).unwrap(), "unsupported version with otherwise complete valid controls");
        }
        let mut changed = manifest.clone();
        changed["meshing_recipe"].as_object_mut().unwrap().remove("first_layer_m");
        assert!(!reference_meshing_recipe_matches(&consumer, &case, &changed).unwrap(), "a missing control cannot fall back to a default");
        let mut changed = manifest.clone();
        changed["patches"] = json!(["body", "wheel_fr"]);
        assert!(!reference_meshing_recipe_matches(&consumer, &case, &changed).unwrap(), "recipe and case patch declarations must agree");
        let mut changed = manifest.clone();
        changed["flavour"] = json!("foundation999");
        assert!(!reference_meshing_recipe_matches(&consumer, &case, &changed).unwrap());
        let mut changed = manifest.clone();
        changed.as_object_mut().unwrap().remove("meshing_recipe");
        assert!(!reference_meshing_recipe_matches(&consumer, &case, &changed).unwrap());
        let mut changed = manifest;
        changed["meshing_recipe"]["expected_dictionary"] = json!(std::fs::read_to_string(case.join("system/snappyHexMeshDict")).unwrap());
        assert!(!reference_meshing_recipe_matches(&consumer, &case, &changed).unwrap(), "free-form expected text is not an admitted recipe field");
    }

    #[test]
    fn wp9_reference_recipe_rejects_consistently_rendered_bad_controls_and_plan() {
        let consumer = ctx("wp9-recipe-invalid");
        let (case, manifest) = wp9_recipe_fixture(&consumer);
        for (pointer, value) in [
            ("/meshing_recipe/n_layers", json!(0)),
            ("/meshing_recipe/expansion", Value::Null), // JSON cannot encode NaN; actual text below contains it.
            ("/meshing_recipe/expansion", json!("NaN")),
            ("/meshing_recipe/expansion", json!(1e100)),
            ("/meshing_recipe/first_layer_m", json!(-0.1)),
            ("/meshing_recipe/min_thickness_rel", json!(0.0)),
            ("/meshing_recipe/n_cells_between", json!(999)),
            ("/meshing_recipe/snap_iter", json!(0)),
            ("/meshing_recipe/feature_angle", json!(181.0)),
            ("/meshing_recipe/max_global_cells", json!(0)),
            ("/meshing_recipe/max_local_cells", json!(40000000)),
            ("/meshing_recipe/location/0", json!(1e100)),
            ("/meshing_recipe/max_non_ortho", json!(90.0)),
            ("/plan/levels/body/0", json!(8)),
            ("/plan/levels/wheel", json!(-1)),
            ("/plan/boxes/0/name", json!("nearWake; #include \"elsewhere\"")),
            ("/plan/boxes/0/name", json!("body")),
            ("/plan/boxes/0/level", json!(99)),
            ("/plan/boxes/0/min/0", json!(9.0)),
            ("/plan/boxes/0/max/0", Value::Null),
            ("/plan/underbody_mode", json!("unsupported")),
        ] {
            let mut changed = manifest.clone();
            *changed.pointer_mut(pointer).unwrap() = value.clone();
            wp9_recipe_render(&case, &changed);
            assert!(!reference_meshing_recipe_matches(&consumer, &case, &changed).unwrap(), "admitted {pointer}={value}");
        }
        for patches in [json!(["body", "wheel_fr", "wheel_fr"]), json!(["body", "wheel_fr", "unknown"]), json!(["body", "wheel_fr"])] {
            let mut changed = manifest.clone();
            changed["patches"] = patches.clone();
            changed["meshing_recipe"]["patches"] = patches.clone();
            wp9_recipe_render(&case, &changed);
            assert!(!reference_meshing_recipe_matches(&consumer, &case, &changed).unwrap(), "admitted patches {patches}");
        }
        let mut changed = manifest;
        let duplicate = changed["plan"]["boxes"][0].clone();
        changed["plan"]["boxes"].as_array_mut().unwrap().push(duplicate);
        wp9_recipe_render(&case, &changed);
        assert!(!reference_meshing_recipe_matches(&consumer, &case, &changed).unwrap(), "duplicate box names are ambiguous even when rendered consistently");
    }

    #[test]
    fn wp9_reference_recipe_rejects_wheel_omission_despite_matching_full_builder_hash() {
        let (consumer, _, _, built) = wp9_unit_built_mesh("wp9-recipe-builder-hash");
        let case = built[0].as_file().unwrap();
        let (_, manifest) = wp9_recipe_fixture(&consumer);
        wp9_recipe_render(case, &manifest);
        let path = case.join("system/snappyHexMeshDict");
        let original = std::fs::read_to_string(&path).unwrap();
        // The STL may still exist; the actual builder dictionary omits it.
        std::fs::write(&path, original.replace("wheel_rr", "omitted_wheel")).unwrap();
        let hash = case_input_fingerprint(&consumer, case).unwrap();
        std::fs::write(case.join(MESH_STAMP), &hash).unwrap();
        assert_eq!(certified_input_fingerprint(&consumer, case).unwrap(), Some(hash));
        assert!(!reference_meshing_recipe_matches(&consumer, case, &manifest).unwrap(), "a builder stamp certifies its inputs, not the presence of every reference wheel");
    }

    #[test]
    fn wp9_reference_recipe_compares_complete_quality_and_feature_dictionaries() {
        let consumer = ctx("wp9-recipe-complete-dictionaries");
        let (case, manifest) = wp9_recipe_fixture(&consumer);
        for relative in ["system/snappyHexMeshDict", "system/meshQualityDict", "system/surfaceFeaturesDict"] {
            let path = case.join(relative);
            let original = std::fs::read_to_string(&path).unwrap();
            std::fs::write(&path, format!("{original}\n#include \"overrideDict\"\n")).unwrap();
            assert!(!reference_meshing_recipe_matches(&consumer, &case, &manifest).unwrap(), "edited {relative} remained admitted");
            std::fs::write(&path, original).unwrap();
        }
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

    fn wp9_quality_report(log:&str)->Report {
        evaluate_mesh(&parse_check_mesh(log),&Params::from_specs(&quality_info().params)).unwrap()
    }

    #[test]
    fn wp9_mesh_quality_requires_complete_finite_evidence() {
        let good="cells: 320\nMesh non-orthogonality Max: 0 average: 0\nMax skewness = 0 OK.\nMesh OK.\n";
        assert!(wp9_quality_report(good).passes());
        for incomplete in ["cells: 320\n".to_string(),good.replace("Max skewness = 0 OK.\n",""),
            good.replace("Mesh OK.\n",""),good.replace("cells: 320","cells: 1e999"),
            good.replace("cells: 320","cells: -1")] {
            assert!(!wp9_quality_report(&incomplete).passes(),"cannot certify missing or invalid mesh evidence: {incomplete}");
        }
    }

    #[test]
    fn wp9_mesh_quality_enforces_strict_non_ortho_and_skewness_limits() {
        for (non_ortho,skew,passes) in [(69.9,3.9,true),(70.0,3.9,false),(69.9,4.0,false)] {
            let text=format!("cells: 320\nMesh non-orthogonality Max: {non_ortho} average: 0\nMax skewness = {skew} OK.\nMesh OK.\n");
            assert_eq!(wp9_quality_report(&text).passes(),passes,"non_ortho={non_ortho}, skew={skew}");
        }
    }

    #[test]
    fn wp9_mesh_quality_reports_actual_skewness() {
        let r=wp9_quality_report("cells: 320\nMesh non-orthogonality Max: 12 average: 2\nMax skewness = 1.25 OK.\nMesh OK.\n");
        assert_eq!(r.metrics.get("max_skewness").map(|m|m.value),Some(1.25));
    }

    #[test]
    fn wp9_mesh_quality_cannot_raise_the_absolute_non_ortho_limit() {
        let mut p=Params::from_specs(&quality_info().params);
        p.set("non_ortho_fail",ParamValue::Float(80.0));
        let st=parse_check_mesh("cells: 320\nMesh non-orthogonality Max: 75 average: 0\nMax skewness = 1 OK.\nMesh OK.\n");
        assert!(!evaluate_mesh(&st,&p).unwrap().passes());
    }

    #[test]
    fn a_failed_skewness_check_keeps_its_value_and_reports_failure() {
        // The tool's own wording on a real road-car mesh.
        let log="cells: 3758498\nMesh non-orthogonality Max: 64.996984 average: 6.9259079\n ***Max skewness = 5.3097583, 8 highly skew faces detected which may impair the quality of the results\n\nFailed 1 mesh checks.\n";
        let st=parse_check_mesh(log);
        assert_eq!(st.invalid_fields,0);
        assert_eq!(st.max_skewness,Some(5.3097583));
        assert_eq!(st.failed_checks,Some(1.0));
        assert!(!st.mesh_ok);
        assert!(!wp9_quality_report(log).passes());
    }

    #[test]
    fn wp9_mesh_quality_retains_invalid_failure_evidence() {
        let good="cells: 320\nMesh non-orthogonality Max: 0 average: 0\nMax skewness = 0 OK.\nMesh OK.\n";
        for bad in ["Failed -1 mesh checks.\n","Failed nan mesh checks.\n"] {
            assert!(!wp9_quality_report(&format!("{good}{bad}")).passes(),"invalid status must not be sanitized: {bad}");
        }
    }

    #[test]
    fn wp9_mesh_quality_does_not_skip_invalid_tokens_or_mix_records() {
        let good="cells: 320\nMesh non-orthogonality Max: 0 average: 0\nMax skewness = 0 OK.\nMesh OK.\n";
        for bad in [good.replace("Max skewness = 0 OK.","Max skewness = NaN (0 faces)"),
            good.replace("cells: 320","cells: NaN (320)"),
            format!("{good}Max skewness = 5 OK.\nMax skewness = 0 OK.\n"),
            format!("{good}Mesh non-orthogonality Max: 75 average: 0\nMesh non-orthogonality Max: 0 average: 0\n")] {
            assert!(!wp9_quality_report(&bad).passes(),"invalid or conflicting required evidence cannot certify a mesh: {bad}");
        }
    }

    #[test]
    fn wp9_wall_overrides_are_counted_and_labelled() {
        let ctx = ctx("quality-wall-manual");
        let logs = ctx.work_dir.join("logs");
        let yplus = ctx.work_dir.join("postProcessing/yPlus1/500");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::create_dir_all(&yplus).unwrap();
        std::fs::write(logs.join("checkMesh.log"), "cells: 320\nMesh non-orthogonality Max: 0 average: 0\nMax skewness = 0 OK.\nMesh OK.\n").unwrap();
        std::fs::write(ctx.work_dir.join("log.snappyHexMesh"), "Extruding 180 out of 200 faces (90%).\n").unwrap();
        std::fs::write(yplus.join("yPlus.dat"), "# Time patch min max average\n500 body 20 300 80\n").unwrap();
        let input = Some(Data::File(ctx.work_dir.clone()));
        let defaults = Params::from_specs(&quality_info().params);
        let measured = quality_cook(&ctx, std::slice::from_ref(&input), &defaults).unwrap();
        let measured = measured[0].as_report().unwrap();
        assert!(measured.passes());
        assert_eq!(measured.metrics["layer_coverage_pct"].value, 90.0);
        assert_eq!(measured.metrics["yplus_mean"].value, 80.0);
        assert_eq!(measured.metrics["manual_mesh_overrides"].value, 0.0);
        for (layer, yplus, count) in [(Some(80.0), None, 1.0), (None, Some(120.0), 1.0), (Some(80.0), Some(120.0), 2.0)] {
            let mut params = defaults.clone();
            if let Some(value) = layer { params.set("layer_cov_pct", ParamValue::Float(value)); }
            if let Some(value) = yplus { params.set("yplus_mean", ParamValue::Float(value)); }
            let out = quality_cook(&ctx, std::slice::from_ref(&input), &params).unwrap();
            let report = out[0].as_report().unwrap();
            assert!(report.passes(), "{report:?}");
            assert_eq!(report.metrics["layer_coverage_pct"].value, layer.unwrap_or(90.0));
            assert_eq!(report.metrics["yplus_mean"].value, yplus.unwrap_or(80.0));
            assert_eq!(report.metrics["manual_mesh_overrides"].value, count, "wall-only overrides must not appear measured");
            assert!(report.notes.iter().any(|(severity, text)| *severity == Severity::Info && text.contains("manual mesh values override log evidence")), "{report:?}");
        }
    }

    #[test]
    fn parses_check_mesh_and_layer_logs() {
        let s = parse_check_mesh(CHECK_MESH);
        assert_eq!(s.cells, Some(2_000_000.0));
        assert_eq!(s.max_non_ortho, Some(78.2));
        assert_eq!(s.failed_checks, Some(2.0));
        assert_eq!(parse_check_mesh("Mesh OK.\n").failed_checks, Some(0.0));
        assert_eq!(parse_layer_coverage("Extruding 100 out of 200 faces (50%).\nExtruding 180 out of 200 faces (90%).\n"), Some(90.0));
        let y = "# Time patch min max average\n0 body 10 400 80\n0 ground 1 2 3\n500 body 20 300 60\n500 wheel_fr 30 500 100\n500 ground 1 2 3\n";
        assert_eq!(parse_yplus_dat(y), Some(60.0), "wheels and ground are not the body");
        assert_eq!(parse_yplus_dat("# Time patch min max average\n500 wheel_fr 30 500 100\n"), None, "no body patch means no body y+");
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
        p2.set("cells",ParamValue::Float(320.0));
        p2.set("max_skewness",ParamValue::Float(1.0));
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
    fn memory_estimate_follows_one_to_two_gb_per_million_cells() {
        assert_eq!(ram_estimate_gb(24.0), (24.0, 48.0));
        let ctx = ctx("ram");
        // the mesh dictionaries estimate memory at the cell limit
        let mut ps = Params::from_specs(&setup_info().params);
        ps.set("max_global_cells_m", ParamValue::Float(24.0));
        let out = setup_cook(&ctx, &[], &ps).unwrap();
        let r = out[1].as_report().unwrap();
        assert_eq!(r.metrics["ram_estimate_gb_low"].value, 24.0);
        assert_eq!(r.metrics["ram_estimate_gb_high"].value, 48.0);
        assert!(!r.metrics.contains_key("ram_available_gb"), "no machine given, no comparison");
        // a 32 GB machine can run the low estimate but not the high one: a warning
        ps.set("ram_gb", ParamValue::Float(32.0));
        let r = setup_cook(&ctx, &[], &ps).unwrap()[1].as_report().unwrap().clone();
        assert!(r.notes.iter().any(|(s, t)| *s == Severity::Warning && t.contains("exceeds the 32 GB")), "{r:?}");
        // 64 GB covers both
        ps.set("ram_gb", ParamValue::Float(64.0));
        let r = setup_cook(&ctx, &[], &ps).unwrap()[1].as_report().unwrap().clone();
        assert!(!r.notes.iter().any(|(_, t)| t.contains("exceeds")), "{r:?}");
        // the quality check uses the built cell count and fails when even the low estimate does not fit
        let mut pq = Params::from_specs(&quality_info().params);
        pq.set("cells", ParamValue::Float(24.0e6));
        pq.set("max_non_ortho", ParamValue::Float(60.0));
        pq.set("failed_checks", ParamValue::Float(0.0));
        pq.set("ram_gb", ParamValue::Float(16.0));
        let r = quality_cook(&ctx, &[None], &pq).unwrap()[0].as_report().unwrap().clone();
        assert_eq!(r.metrics["ram_estimate_gb_high"].value, 48.0);
        assert!(r.notes.iter().any(|(s, t)| *s == Severity::Error && t.contains("exceeds the 16 GB")), "{r:?}");
        assert!(!r.passes());
    }

    #[test]
    fn wp9_build_defaults_are_serial_with_explicit_wsl_fields() {
        let p = Params::from_specs(&build_info().params);
        assert_eq!(p.int("np").unwrap(), 1, "parallel execution still requires qualification");
        for name in ["wsl_distribution", "wsl_user", "wsl_environment_file"] {
            assert_eq!(p.str(name).unwrap(), "", "planning must not assume a Linux execution context");
        }
    }

    fn wp9_build_source(name: &str) -> PathBuf {
        let upstream = ctx(name);
        let out = setup_cook(&upstream, &[], &Params::from_specs(&setup_info().params)).unwrap();
        let case = out[0].as_file().unwrap().clone();
        for name in ["points", "faces", "owner", "neighbour", "boundary"] {
            write_file(&case.join("constant/polyMesh").join(name), "upstream mesh must stay upstream\n").unwrap();
        }
        write_file(&case.join("logs/checkMesh.log"), "cells: 320\nMesh non-orthogonality Max: 0 average: 0\nMax skewness = 0 OK.\nMesh OK.\n").unwrap();
        write_file(&case.join("postProcessing/forceCoeffs/0/coefficient.dat"), "upstream result\n").unwrap();
        write_file(&case.join("500/U"), "upstream solved field\n").unwrap();
        case
    }

    fn wp9_build_snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        fn collect(root: &Path, dir: &Path, files: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    collect(root, &path, files);
                } else {
                    files.insert(path.strip_prefix(root).unwrap().to_path_buf(), std::fs::read(&path).unwrap());
                }
            }
        }
        let mut files = std::collections::BTreeMap::new();
        collect(root, root, &mut files);
        files
    }

    #[test]
    fn wp9_build_planning_owns_inputs_and_omits_old_mesh_results() {
        let source = wp9_build_source("wp9-build-plan-source");
        let original = wp9_build_snapshot(&source);
        let consumer = ctx("wp9-build-plan-consumer");
        let mut p = Params::from_specs(&build_info().params);
        p.set("np", ParamValue::Int(3));
        let out = build_cook(&consumer, &[Some(Data::File(source.clone()))], &p).unwrap();
        let owned = out[0].as_file().unwrap();
        assert_ne!(owned, &source, "mesh build must never write in the upstream case");
        assert_eq!(std::fs::canonicalize(owned).unwrap(), std::fs::canonicalize(consumer.work_dir.join("native/case")).unwrap());
        assert!(!owned.join("constant/polyMesh").exists());
        assert!(!owned.join("logs").exists());
        assert!(!owned.join("postProcessing").exists());
        assert!(!owned.join("500").exists());
        let decompose = std::fs::read_to_string(owned.join("system/decomposeParDict")).unwrap();
        assert!(decompose.contains("numberOfSubdomains 3;"), "{decompose}");
        let script = std::fs::read_to_string(owned.join("Allrun.mesh")).unwrap();
        assert!(script.contains("mpirun -np 3 snappyHexMesh"), "{script}");
        let report = out[1].as_report().unwrap();
        assert!(!report.passes());
        assert_eq!(report.metrics["mesh_built"].value, 0.0);
        assert_eq!(wp9_build_snapshot(&source), original);
    }

    #[test]
    fn wp9_build_failed_run_cannot_publish_upstream_or_previous_mesh() {
        let source = wp9_build_source("wp9-build-failed-source");
        let original = wp9_build_snapshot(&source);
        let consumer = ctx("wp9-build-failed-consumer");
        let mut p = Params::from_specs(&build_info().params);
        p.set("run", ParamValue::Bool(true));
        p.set("np", ParamValue::Int(1));
        p.set("bin_dir", ParamValue::Str(source.join("no-executables-here").display().to_string()));
        for attempt in 0..2 {
            let out = build_cook(&consumer, &[Some(Data::File(source.clone()))], &p).unwrap();
            let output = out[0].as_file().unwrap();
            assert!(!output.exists(), "failed mesh attempt {attempt} published a usable File: {output:?}");
            assert!(output.starts_with(&consumer.work_dir));
            let report = out[1].as_report().unwrap();
            assert!(!report.passes());
            assert_eq!(report.metrics["mesh_built"].value, 0.0);
            assert_eq!(report.metrics["steps_completed"].value, 0.0);
            assert_eq!(report.metrics["external_run_requested"].value, 1.0);
            assert_eq!(report.metrics["external_run_succeeded"].value, 0.0);
            assert_eq!(report.metrics["external_run_failed"].value, 1.0);
            let owned = consumer.work_dir.join("native/case");
            assert!(!owned.join("constant/polyMesh").exists(), "a requested run imported old mesh files");
            assert!(!owned.join("postProcessing").exists());
            assert!(!owned.join("500").exists());
            // A later requested attempt must discard even results in its own old generation.
            write_file(&owned.join("constant/polyMesh/boundary"), "stale previous attempt\n").unwrap();
            write_file(&owned.join("logs/checkMesh.log"), "Mesh OK.\n").unwrap();
        }
        assert_eq!(wp9_build_snapshot(&source), original);
    }

    #[test]
    fn wp9_build_blank_wsl_context_fails_before_any_step() {
        let source = wp9_build_source("wp9-build-wsl-source");
        let consumer = ctx("wp9-build-wsl-consumer");
        let mut p = Params::from_specs(&build_info().params);
        p.set("run", ParamValue::Bool(true));
        p.set("runner", ParamValue::Str("wsl".into()));
        let out = build_cook(&consumer, &[Some(Data::File(source))], &p).unwrap();
        let report = out[1].as_report().unwrap();
        assert!(!out[0].as_file().unwrap().exists());
        assert!(!report.passes());
        assert_eq!(report.metrics["steps_completed"].value, 0.0);
        assert!(report.notes.iter().any(|(s, text)| *s == Severity::Error && text.contains("distribution")), "{report:?}");
        assert!(!consumer.work_dir.join("native/.wsl-control").exists(), "blank execution context must not probe WSL");
    }

    // Original unit tools exercise command/evidence/cache behavior only. They
    // neither run a mesher nor establish that these fake tables form a mesh.
    fn wp9_fixture_mesh_tools(dir: &Path, log: &str, tables: usize, empty_points: bool) {
        std::fs::create_dir_all(dir).unwrap();
        for tool in ["blockMesh", "snappyHexMesh", "checkMesh"] {
            let text = if cfg!(windows) {
                let mut text = String::from("@echo off\r\necho unit-invocation>>unit-mesh-invocations.txt\r\n");
                if tool == "blockMesh" {
                    text.push_str("if not exist constant\\polyMesh mkdir constant\\polyMesh\r\n");
                    for name in ["points", "faces", "owner", "neighbour", "boundary"].into_iter().take(tables) {
                        if empty_points && name == "points" {
                            text.push_str("type nul > constant\\polyMesh\\points\r\n");
                        } else {
                            text.push_str(&format!("echo fake negative fixture>constant\\polyMesh\\{name}\r\n"));
                        }
                    }
                } else if tool == "checkMesh" {
                    for line in log.lines() { text.push_str(&format!("echo {line}\r\n")); }
                }
                text.push_str("exit /b 0\r\n");
                text
            } else {
                let mut text = String::from("#!/bin/sh\nset -e\nprintf '%s\\n' unit-invocation >> unit-mesh-invocations.txt\n");
                if tool == "blockMesh" {
                    text.push_str("mkdir -p constant/polyMesh\n");
                    for name in ["points", "faces", "owner", "neighbour", "boundary"].into_iter().take(tables) {
                        if empty_points && name == "points" {
                            text.push_str(": > constant/polyMesh/points\n");
                        } else {
                            text.push_str(&format!("printf '%s\\n' 'fake negative fixture' > constant/polyMesh/{name}\n"));
                        }
                    }
                } else if tool == "checkMesh" {
                    text.push_str(&format!("cat <<'WP9_NEGATIVE_FIXTURE'\n{log}WP9_NEGATIVE_FIXTURE\n"));
                }
                text.push_str("exit 0\n");
                text
            };
            let path = dir.join(if cfg!(windows) { format!("{tool}.cmd") } else { tool.into() });
            std::fs::write(&path, text).unwrap();
            #[cfg(unix)] {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
    }

    #[test]
    fn wp9_build_zero_exit_fixtures_cannot_certify_bad_mesh_evidence() {
        let good = "cells: 320\nMesh non-orthogonality Max: 0 average: 0\nMax skewness = 0 OK.\nMesh OK.\n";
        for (name, log, tables, empty) in [
            ("missing-table", good.to_string(), 4, false),
            ("empty-points", good.to_string(), 5, true),
            ("missing-status", good.replace("Mesh OK.\n", ""), 5, false),
            ("nan-cells", good.replace("cells: 320", "cells: NaN"), 5, false),
            ("zero-cells", good.replace("cells: 320", "cells: 0"), 5, false),
            ("angle-limit", good.replace("Max: 0", "Max: 70"), 5, false),
            ("skew-limit", good.replace("skewness = 0", "skewness = 4"), 5, false),
        ] {
            let upstream = ctx(&format!("wp9-negative-{name}-source"));
            let source = setup_cook(&upstream, &[], &Params::from_specs(&setup_info().params)).unwrap()[0].as_file().unwrap().clone();
            let tools = upstream.work_dir.join("negative-fixture-tools");
            wp9_fixture_mesh_tools(&tools, &log, tables, empty);
            let consumer = ctx(&format!("wp9-negative-{name}-consumer"));
            let mut p = Params::from_specs(&build_info().params);
            p.set("run", ParamValue::Bool(true));
            p.set("np", ParamValue::Int(1));
            p.set("bin_dir", ParamValue::Str(tools.display().to_string()));
            let out = build_cook(&consumer, &[Some(Data::File(source))], &p).unwrap();
            let report = out[1].as_report().unwrap();
            assert_eq!(report.metrics["steps_completed"].value, 3.0, "all fake commands must exit zero before the negative postcondition is tested: {name}: {report:?}");
            assert!(!report.passes(), "zero exit codes cannot certify {name}: {report:?}");
            assert!(!out[0].as_file().unwrap().exists(), "bad evidence published a usable case for {name}");
            assert_eq!(report.metrics["mesh_built"].value, 0.0, "{name}");
        }
    }

    fn wp9_unit_built_mesh(name: &str) -> (CookContext, PathBuf, Params, Vec<Data>) {
        let upstream = ctx(&format!("{name}-source"));
        let source = setup_cook(&upstream, &[], &Params::from_specs(&setup_info().params))
            .unwrap()[0].as_file().unwrap().clone();
        let original = wp9_build_snapshot(&source);
        let tools = upstream.work_dir.join("unit-mesh-tools");
        let good = "cells: 320\nMesh non-orthogonality Max: 12 average: 2\nMax skewness = 1.5 OK.\nMesh OK.\n";
        wp9_fixture_mesh_tools(&tools, good, 5, false);
        let consumer = ctx(&format!("{name}-consumer"));
        let mut p = Params::from_specs(&build_info().params);
        p.set("run", ParamValue::Bool(true));
        p.set("np", ParamValue::Int(1));
        p.set("bin_dir", ParamValue::Str(tools.display().to_string()));
        let out = build_cook(&consumer, &[Some(Data::File(source.clone()))], &p).unwrap();
        let report = out[1].as_report().unwrap();
        assert!(report.passes(), "the unit command fixture must reach its tested postcondition: {report:?}");
        assert_eq!(report.metrics["steps_completed"].value, 3.0);
        assert_eq!(wp9_build_snapshot(&source), original, "the node must leave upstream inputs untouched");
        let calls = std::fs::read_to_string(out[0].as_file().unwrap().join("unit-mesh-invocations.txt")).unwrap();
        assert_eq!(calls.lines().count(), 3, "only the three fixture tools should execute");
        (consumer, source, p, out)
    }

    fn wp9_unit_cache_refused(consumer: &CookContext, source: &Path, p: &Params, old_case: &Path) {
        let before = wp9_build_snapshot(old_case);
        let out = build_cook(consumer, &[Some(Data::File(source.to_path_buf()))], p).unwrap();
        let report = out[1].as_report().unwrap();
        assert!(!report.passes(), "changed or unproven cached meshes cannot pass: {report:?}");
        assert_eq!(report.metrics["mesh_built"].value, 0.0);
        assert_eq!(report.metrics["external_run_requested"].value, 0.0);
        assert_eq!(report.metrics["external_run_succeeded"].value, 0.0);
        assert_eq!(report.metrics["external_run_failed"].value, 0.0);
        assert!(!out[0].as_file().unwrap().exists(), "cache refusal cannot publish a live old artifact");
        assert_eq!(wp9_build_snapshot(old_case), before, "checking a cache must neither run tools nor repair/delete its files");
    }

    #[test]
    fn wp9_reference_source_requires_current_builder_stamp_and_measured_tables() {
        let (consumer, _, _, built) = wp9_unit_built_mesh("wp9-reference-mesh-inputs");
        let case = built[0].as_file().unwrap();
        let expected = case_input_fingerprint(&consumer, case).unwrap();
        assert_eq!(certified_input_fingerprint(&consumer, case).unwrap(), Some(expected.clone()));
        std::fs::write(case.join(MESH_STAMP), "certified\n").unwrap();
        assert!(certified_input_fingerprint(&consumer, case).unwrap().is_none(), "legacy literal is not input-bound proof");
        std::fs::write(case.join(MESH_STAMP), &expected).unwrap();
        let points = case.join("constant/polyMesh/points");
        let original = std::fs::read(&points).unwrap();
        let mut changed = original.clone(); changed[0] ^= 1;
        std::fs::write(&points, changed).unwrap();
        assert!(certified_input_fingerprint(&consumer, case).unwrap().is_none(), "edited volume mesh kept reference admission");
        std::fs::write(points, original).unwrap();
        let record = case.join("constant/.vernon-mesh-evidence.json");
        std::fs::remove_file(record).unwrap();
        // Even an independently matching full hash is insufficient without measurements.
        std::fs::write(case.join(MESH_STAMP), case_input_fingerprint(&consumer, case).unwrap()).unwrap();
        assert!(certified_input_fingerprint(&consumer, case).is_err(), "stamp alone invented measured mesh evidence");
    }

    #[test]
    fn wp9_build_unit_success_records_evidence_and_reuses_it_with_run_off() {
        let (consumer, source, mut p, built) = wp9_unit_built_mesh("wp9-unit-record-cache");
        let case = built[0].as_file().unwrap();
        let evidence = crate::util::read_mesh_evidence(&consumer, case)
            .expect("a successful build must publish measured, mesh-bound evidence");
        assert_eq!(evidence.cells, 320, "actual log count, not estimated background cells");
        assert_eq!(evidence.max_nonortho, 12.0);
        assert_eq!(evidence.max_skew, 1.5);
        let stamp = std::fs::read_to_string(case.join(MESH_STAMP)).unwrap();
        assert_eq!(stamp.trim(), crate::util::case_input_fingerprint(&consumer, case).unwrap());
        assert_ne!(stamp.trim(), "certified");
        let before = wp9_build_snapshot(case);
        p.set("run", ParamValue::Bool(false));
        let cached = build_cook(&consumer, &[Some(Data::File(source))], &p).unwrap();
        let report = cached[1].as_report().unwrap();
        assert!(report.passes(), "unchanged measured mesh should remain usable with run off: {report:?}");
        assert_eq!(cached[0].as_file().unwrap(), case);
        assert_eq!(report.metrics["mesh_built"].value, 1.0);
        assert_eq!(report.metrics["external_run_requested"].value, 0.0);
        assert_eq!(report.metrics["external_run_succeeded"].value, 0.0);
        assert_eq!(report.metrics["external_run_failed"].value, 0.0);
        assert_eq!(wp9_build_snapshot(case), before, "cached reads must not rewrite or rerun the case");
    }

    #[test]
    fn wp9_build_unit_cached_mesh_rejects_changed_base_tables() {
        for name in ["points", "faces", "owner", "neighbour", "boundary"] {
            let (consumer, source, mut p, built) = wp9_unit_built_mesh(&format!("wp9-unit-edited-{name}"));
            let case = built[0].as_file().unwrap();
            let path = case.join("constant/polyMesh").join(name);
            let mut changed = std::fs::read(&path).unwrap();
            changed[0] ^= 1;
            std::fs::write(path, changed).unwrap();
            p.set("run", ParamValue::Bool(false));
            wp9_unit_cache_refused(&consumer, &source, &p, case);
        }
    }

    #[test]
    fn wp9_build_unit_cached_mesh_rejects_changed_system_dictionary() {
        let (consumer, source, mut p, built) = wp9_unit_built_mesh("wp9-unit-edited-dictionary");
        let case = built[0].as_file().unwrap();
        let path = case.join("system/blockMeshDict");
        let mut changed = std::fs::read(&path).unwrap();
        changed.extend_from_slice(b"\n// changed after measured build\n");
        std::fs::write(path, changed).unwrap();
        p.set("run", ParamValue::Bool(false));
        wp9_unit_cache_refused(&consumer, &source, &p, case);
    }

    #[test]
    fn wp9_build_unit_cached_mesh_rejects_legacy_literal_stamp() {
        let (consumer, source, mut p, built) = wp9_unit_built_mesh("wp9-unit-legacy-stamp");
        let case = built[0].as_file().unwrap();
        std::fs::write(case.join(MESH_STAMP), b"certified\n").unwrap();
        p.set("run", ParamValue::Bool(false));
        wp9_unit_cache_refused(&consumer, &source, &p, case);
    }

    #[test]
    fn wp9_build_unit_cached_mesh_requires_record_even_with_matching_case_stamp() {
        let (consumer, source, mut p, built) = wp9_unit_built_mesh("wp9-unit-missing-record");
        let case = built[0].as_file().unwrap();
        let record = case.join("constant/.vernon-mesh-evidence.json");
        if record.exists() {
            std::fs::remove_file(record).unwrap();
        }
        // Negative fixture: a matching input stamp still cannot invent measured proof.
        let stamp = crate::util::case_input_fingerprint(&consumer, case).unwrap();
        std::fs::write(case.join(MESH_STAMP), stamp).unwrap();
        p.set("run", ParamValue::Bool(false));
        wp9_unit_cache_refused(&consumer, &source, &p, case);
    }

    #[test]
    fn wp9_build_unit_cached_mesh_reports_bound_counts_despite_edited_log() {
        let (consumer, source, mut p, built) = wp9_unit_built_mesh("wp9-unit-edited-count-log");
        let case = built[0].as_file().unwrap();
        let log = case.join("logs/checkMesh.log");
        let changed = std::fs::read_to_string(&log).unwrap().replace("cells: 320", "cells: 999999");
        std::fs::write(log, changed).unwrap();
        let before = wp9_build_snapshot(case);
        p.set("run", ParamValue::Bool(false));
        let out = build_cook(&consumer, &[Some(Data::File(source))], &p).unwrap();
        let report = out[1].as_report().unwrap();
        assert!(report.passes(), "the mesh-bound record remains valid despite a diagnostic-log edit: {report:?}");
        assert_eq!(report.metrics["cells"].value, 320.0, "unbound log text must not replace the recorded measured count");
        assert_eq!(report.metrics["external_run_requested"].value, 0.0);
        assert_eq!(report.metrics["external_run_succeeded"].value, 0.0);
        assert_eq!(wp9_build_snapshot(case), before);
    }

    #[test]
    fn build_without_run_writes_script_and_fails_closed() {
        let upstream = ctx("build-source");
        let ctx = ctx("build");
        let setup = setup_cook(&upstream, &[], &Params::from_specs(&setup_info().params)).unwrap();
        let case = setup[0].as_file().unwrap().clone();
        let p = Params::from_specs(&build_info().params);
        let out = build_cook(&ctx, &[Some(Data::File(case.clone()))], &p).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(!r.passes());
        assert!(r.notes.iter().any(|(_, t)| t.starts_with("white-box placeholder:")));
        let script = std::fs::read_to_string(out[0].as_file().unwrap().join("Allrun.mesh")).unwrap();
        assert!(script.contains("blockMesh") && script.contains("checkMesh > log.checkMesh"));
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

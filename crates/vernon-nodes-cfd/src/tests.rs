//! Crate-level tests: registration, smoke cooking of every node, and the full
//! chain from surfaces to site loads driven only through the registry.

use std::path::Path;

use serde_json::Value;
use vernon_core::Geometry;
use vernon_graph::{CookContext, Data, DataKind, NodeTypeInfo, ParamKind, ParamValue, Params, Registry, Stage};

use crate::coeffs::ForceResult;
use crate::types::Series;
use crate::util::testutil::{ctx, synthetic_car, tmp};

const EXPECTED: [&str; 21] = [
    "air_state",
    "frontal_area",
    "symmetry_check",
    "vehicle_prep",
    "tunnel_domain",
    "ground_wheels",
    "porous_core",
    "refinement_plan",
    "mesh_levels",
    "mesh_setup",
    "mesh_build",
    "mesh_quality",
    "case_setup",
    "solve_steady",
    "force_coeffs",
    "run_convergence",
    "mesh_study",
    "surface_pressure",
    "compare_ab",
    "aero_loads",
    "benchmark_check",
];

/// Names that must never reach a user-visible string.
const FORBIDDEN: [&str; 16] = [
    "rhino",
    "fusion",
    "autodesk",
    "3ds max",
    "quicksurface",
    "geomagic",
    "zeiss",
    "airshaper",
    "simscale",
    "ansys",
    "optimumlap",
    "plasticity",
    "polycam",
    "ladybug",
    "cloudcompare",
    "houdini",
];

fn registry() -> Registry {
    let mut reg = Registry::new();
    crate::register(&mut reg);
    reg
}

fn cook(reg: &Registry, id: &str, ins: Vec<Option<Data>>, set: &[(&str, ParamValue)]) -> Vec<Data> {
    let info = reg.info(id).unwrap_or_else(|| panic!("{id} not registered"));
    let mut p = Params::from_specs(&info.params);
    for (k, v) in set {
        p.set(k, v.clone());
    }
    reg.get(id).unwrap().cook(&ctx(id), &ins, &p).unwrap_or_else(|e| panic!("{id}: {e}"))
}

#[test]
fn registers_the_whole_stage() {
    let reg = registry();
    assert_eq!(reg.len(), EXPECTED.len());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for name in EXPECTED {
        let id = format!("cfd.{name}");
        let info = reg.info(&id).unwrap_or_else(|| panic!("missing {id}"));
        assert_eq!(info.stage, Stage::Cfd, "{id}");
        assert!(!info.label.is_empty() && !info.help.is_empty(), "{id}: label and help");
        assert!(!info.emulates.is_empty(), "{id}: emulates");
        let spec = info.spec.as_deref().unwrap_or_else(|| panic!("{id}: no spec"));
        assert!(spec.starts_with("docs/specs/") && spec.ends_with(".md"), "{id}: {spec}");
        assert!(root.join(spec).is_file(), "{id}: spec file {spec} missing");
        assert!(!info.outputs.is_empty(), "{id}: outputs");
        for p in &info.params {
            assert!(!p.label.is_empty(), "{id}.{} has no label", p.name);
        }
    }
}

#[test]
fn external_tool_nodes_name_the_tool_and_default_to_not_running() {
    let reg = registry();
    for id in ["cfd.mesh_build", "cfd.solve_steady", "cfd.case_setup"] {
        assert_eq!(reg.info(id).unwrap().external_tool.as_deref(), Some("OpenFOAM"), "{id}");
    }
    for id in ["cfd.mesh_build", "cfd.solve_steady"] {
        let info = reg.info(id).unwrap();
        let run = info.params.iter().find(|p| p.name == "run").unwrap_or_else(|| panic!("{id}: no run switch"));
        assert_eq!(run.default_value(), ParamValue::Bool(false), "{id}");
    }
}

#[test]
fn no_vendor_names_in_user_visible_strings() {
    let reg = registry();
    for info in reg.types() {
        let mut text = vec![info.id.clone(), info.label.clone(), info.help.clone()];
        for p in &info.params {
            text.extend([p.name.clone(), p.label.clone(), p.help.clone(), p.unit.clone().unwrap_or_default()]);
        }
        for port in info.inputs.iter().chain(&info.outputs) {
            text.push(port.name.clone());
        }
        for t in text {
            let low = t.to_lowercase();
            for bad in FORBIDDEN {
                assert!(!low.contains(bad), "{}: '{t}' contains '{bad}'", info.id);
            }
        }
    }
}

#[test]
fn param_defaults_are_inside_their_ranges() {
    for info in registry().types() {
        for p in &info.params {
            match &p.kind {
                ParamKind::Float { default, min, max } => {
                    assert!((*min..=*max).contains(default), "{}.{}: {default} not in {min}..{max}", info.id, p.name);
                }
                ParamKind::Int { default, min, max } => {
                    assert!((*min..=*max).contains(default), "{}.{}", info.id, p.name);
                }
                ParamKind::Enum { default, options } => assert!(options.contains(default), "{}.{}", info.id, p.name),
                _ => {}
            }
        }
    }
}

fn force_json(cd: f64) -> Value {
    ForceResult {
        source: "forces".into(),
        u_ms: 30.0,
        rho: 1.225,
        a_front_m2: 2.0,
        aref_m2: 1.0,
        half_model: true,
        cd,
        cda: cd * 2.0,
        window: 100,
        samples: 500,
        ..ForceResult::default()
    }
    .to_json()
}

fn series_json() -> Value {
    let n = 400;
    let mut s = Series { time: (1..=n).map(f64::from).collect(), ..Series::default() };
    s.columns.insert("Cd".into(), vec![0.32; n as usize]);
    s.columns.insert("Cl".into(), vec![-0.25; n as usize]);
    s.columns.insert("res_p".into(), vec![1e-6; n as usize]);
    s.meta.insert("Aref".into(), 1.0);
    s.to_json()
}

/// Inputs for a node from nothing but its declared ports.
fn smoke_inputs(info: &NodeTypeInfo, car: &Geometry, wheels: &Geometry, work: &Path) -> Vec<Option<Data>> {
    let mut both = car.clone();
    both.merge(wheels);
    info.inputs
        .iter()
        .map(|port| match port.name.as_str() {
            "surfaces" => Some(Data::geometry(both.clone())),
            "body" | "surface" => Some(Data::geometry(car.clone())),
            "wheels" => Some(Data::geometry(wheels.clone())),
            "history" => Some(Data::json(series_json())),
            "baseline" | "kit" => Some(Data::json(force_json(0.32))),
            "fine" => Some(Data::json(force_json(0.2825))),
            "medium" => Some(Data::json(force_json(0.2862))),
            "coarse" => Some(Data::json(force_json(0.2925))),
            "case" if port.required => Some(Data::File(work.join("no-such-case"))),
            _ => None,
        })
        .collect()
}

#[test]
fn every_node_cooks_with_default_params_or_fails_cleanly() {
    let reg = registry();
    let (car, wheels) = synthetic_car();
    for info in reg.types() {
        let work = tmp(&format!("smoke-{}", info.id));
        let cx = CookContext { node_name: info.id.clone(), work_dir: work.clone(), frame: 0.0 };
        let ins = smoke_inputs(info, &car, &wheels, &work);
        let params = Params::from_specs(&info.params);
        match reg.get(&info.id).unwrap().cook(&cx, &ins, &params) {
            Ok(out) => {
                assert_eq!(out.len(), info.outputs.len(), "{}: output count", info.id);
                for (d, spec) in out.iter().zip(&info.outputs) {
                    assert_eq!(d.kind(), spec.kind, "{}: output '{}' kind", info.id, spec.name);
                    if let Some(r) = d.as_report() {
                        assert!(r.metrics.values().all(|m| m.value.is_finite()), "{}: non-finite metric", info.id);
                    }
                }
            }
            // a clean, explained error (missing case, no solver data...) is acceptable
            Err(e) => assert!(!e.to_string().is_empty(), "{}: empty error", info.id),
        }
    }
}

#[test]
fn the_solver_dependent_nodes_carry_the_placeholder_warning_and_never_pass() {
    let reg = registry();
    let work = tmp("stub-warning");
    let (car, wheels) = synthetic_car();
    for id in ["cfd.solve_steady", "cfd.benchmark_check", "cfd.surface_pressure"] {
        let info = reg.info(id).unwrap();
        let mut ins = smoke_inputs(info, &car, &wheels, &work);
        if id == "cfd.solve_steady" {
            let case = work.join("case");
            std::fs::create_dir_all(case.join("system")).unwrap();
            std::fs::write(case.join("system/controlDict"), "x").unwrap();
            ins[0] = Some(Data::File(case));
        }
        let out = reg.get(id).unwrap().cook(&ctx(id), &ins, &Params::from_specs(&info.params)).unwrap();
        let rep = out.iter().find_map(|d| d.as_report()).unwrap();
        assert!(rep.notes.iter().any(|(_, t)| t.starts_with("white-box placeholder:")), "{id}");
        assert!(!rep.passes(), "{id}: a placeholder must never pass its gate");
    }
}

/// Recorded solver histories dropped into a case directory.
fn write_histories(case: &Path, aref: f64, cd: f64, cl: f64, n: usize) {
    let mk = |sub: &str, file: &str, text: String| {
        let d = case.join("postProcessing").join(sub).join("0");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(file), text).unwrap();
    };
    let mut coeff = format!("# magUInf : 30\n# Aref : {aref}\n# Time Cd Cl\n");
    let mut info = String::from("# Time Ux_initial p_initial k_initial omega_initial\n");
    for i in 1..=n {
        let wobble = 1e-4 * ((i % 5) as f64 - 2.0);
        coeff += &format!("{i} {} {}\n", cd + wobble, cl + wobble);
        info += &format!("{i} 1e-6 2e-6 1e-6 1e-6\n");
    }
    mk("forceCoeffs1", "coefficient.dat", coeff);
    mk("solverInfo1", "solverInfo.dat", info);
}

#[test]
fn full_chain_from_surfaces_to_site_loads() {
    let reg = registry();
    let (body, wheels) = synthetic_car();
    let g = Data::geometry;
    let j = |d: &Data| Some(Data::Json(d.as_json().unwrap().clone()));
    let prep = cook(&reg, "cfd.vehicle_prep", vec![Some(g(body)), Some(g(wheels)), None], &[]);
    let (vehicle, reference) = (Some(prep[0].clone()), j(&prep[1]));
    let air = cook(&reg, "cfd.air_state", vec![], &[("t_c", ParamValue::Float(45.0)), ("rh_pct", ParamValue::Float(30.0))]);
    let domain = cook(&reg, "cfd.tunnel_domain", vec![reference.clone()], &[]);
    let bc = cook(&reg, "cfd.ground_wheels", vec![j(&domain[0]), reference.clone(), j(&air[0])], &[]);
    let plan = cook(&reg, "cfd.refinement_plan", vec![reference.clone(), j(&domain[0]), None, j(&air[0])], &[]);
    let dicts = cook(&reg, "cfd.mesh_setup", vec![j(&plan[0]), j(&domain[0]), vehicle], &[]);
    let case = cook(&reg, "cfd.case_setup", vec![Some(dicts[0].clone()), j(&air[0]), j(&bc[0]), None, reference.clone(), j(&domain[0])], &[]);
    let case_dir = case[0].as_file().unwrap().clone();
    assert!(case_dir.join("system/controlDict").is_file());

    // the solver has not run: the chain fails closed, no invented numbers
    let solve = cook(&reg, "cfd.solve_steady", vec![Some(case[0].clone())], &[]);
    assert!(!solve[2].as_report().unwrap().passes());
    let fc = cook(&reg, "cfd.force_coeffs", vec![j(&solve[1]), reference.clone(), j(&air[0])], &[]);
    assert!(!fc[1].as_report().unwrap().passes());
    assert!(ForceResult::from_json(fc[0].as_json().unwrap()).unwrap().placeholder);

    // recorded histories appear (half-model reference area, as the case writer set it)
    let a_front = prep[1].as_json().unwrap()["a_front_m2"].as_f64().unwrap();
    let half = domain[0].as_json().unwrap()["half_model"].as_bool().unwrap();
    let aref = if half { a_front / 2.0 } else { a_front };
    write_histories(&case_dir, aref, 0.32, -0.25, 1500);
    let solve = cook(&reg, "cfd.solve_steady", vec![Some(case[0].clone())], &[]);
    assert!(solve[2].as_report().unwrap().passes(), "{:?}", solve[2].as_report());
    let conv = cook(&reg, "cfd.run_convergence", vec![j(&solve[1])], &[("window", ParamValue::Int(300))]);
    assert!(conv[1].as_report().unwrap().passes(), "{:?}", conv[1].as_report());
    let fc = cook(&reg, "cfd.force_coeffs", vec![j(&solve[1]), reference, j(&air[0])], &[("window", ParamValue::Int(300)), ("half_model", ParamValue::Bool(half))]);
    let rep = fc[1].as_report().unwrap();
    assert!(rep.passes(), "{rep:?}");
    let res = ForceResult::from_json(fc[0].as_json().unwrap()).unwrap();
    assert!((res.cd - 0.32).abs() < 1e-3 && (res.cda - 0.32 * a_front).abs() < 1e-3 * a_front, "{res:?}");
    assert!(res.cla < 0.0);
    // the case writer's mesh and boundary recipe travels with the result for the A/B check
    let recipe = res.recipe.as_ref().expect("recipe stamped on the force result");
    let bc_json = bc[0].as_json().unwrap();
    assert_eq!(recipe.ground_mode, bc_json["ground_mode"].as_str().unwrap());
    assert_eq!(recipe.wheel_mode, bc_json["wheel_mode"].as_str().unwrap());
    assert!(recipe.dx0_m.is_some() && recipe.base_cells.is_some() && recipe.levels.is_some(), "{recipe:?}");

    // three meshes, then a baseline-vs-kit comparison and the site loads
    let study = cook(&reg, "cfd.mesh_study", vec![Some(Data::json(force_json(0.2825))), Some(Data::json(force_json(0.2862))), Some(Data::json(force_json(0.2925)))], &[]);
    assert!(study[2].as_report().unwrap().passes());
    let ab = cook(&reg, "cfd.compare_ab", vec![Some(Data::json(force_json(0.3000))), Some(Data::json(force_json(0.3300)))], &[]);
    assert_eq!(ab[0].as_json().unwrap()["verdict"], "worse");
    let loads = cook(&reg, "cfd.aero_loads", vec![j(&fc[0]), j(&air[0])], &[]);
    assert!(loads[1].as_report().unwrap().passes());
    let rho = loads[0].as_json().unwrap()["rho"].as_f64().unwrap();
    assert!((rho - 1.098).abs() < 0.002, "the Dubai air state reaches the site loads: {rho}");
}

/// Linux context of the real runs. A missing setting fails the test instead
/// of skipping it, so a run without the tools can never count as acceptance.
fn wsl_params(np: i64) -> Vec<(&'static str, ParamValue)> {
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k} for the real OpenFOAM tests"));
    vec![
        ("run", ParamValue::Bool(true)),
        ("runner", ParamValue::Str("wsl".into())),
        ("np", ParamValue::Int(np)),
        ("timeout_min", ParamValue::Int(60)),
        ("wsl_distribution", ParamValue::Str(var("VERNON_WSL_DISTRO"))),
        ("wsl_user", ParamValue::Str(var("VERNON_WSL_USER"))),
        ("wsl_environment_file", ParamValue::Str(var("VERNON_WSL_ENV_FILE"))),
    ]
}

fn print_report(what: &str, d: &Data) {
    let r = d.as_report().unwrap();
    println!("--- {what}: passes={}", r.passes());
    for (k, m) in &r.metrics {
        println!("  {k} = {} {}", m.value, m.unit);
    }
    for (s, t) in &r.notes {
        println!("  [{s:?}] {t}");
    }
}

/// The whole chain on a small car shape with the real tools: the writers, the
/// mesh build, the solver and the force reader, driven through the registry.
#[test]
#[ignore = "needs WSL and OpenFOAM"]
fn real_openfoam_chain_on_a_small_car() {
    let reg = registry();
    let (body, wheels) = synthetic_car();
    let g = Data::geometry;
    let j = |d: &Data| Some(Data::Json(d.as_json().unwrap().clone()));
    let prep = cook(&reg, "cfd.vehicle_prep", vec![Some(g(body)), Some(g(wheels)), None], &[]);
    let (vehicle, reference) = (Some(prep[0].clone()), j(&prep[1]));
    let air = cook(&reg, "cfd.air_state", vec![], &[]);
    let domain = cook(&reg, "cfd.tunnel_domain", vec![reference.clone()], &[]);
    let bc = cook(&reg, "cfd.ground_wheels", vec![j(&domain[0]), reference.clone(), j(&air[0])], &[]);
    let plan = cook(&reg, "cfd.refinement_plan", vec![reference.clone(), j(&domain[0]), None, j(&air[0])], &[
        ("dx0_m", ParamValue::Float(0.64)),
        ("body_min", ParamValue::Int(2)),
        ("body_max", ParamValue::Int(3)),
        ("wheel_level", ParamValue::Int(3)),
        ("underbody_level", ParamValue::Int(2)),
        ("near_wake_level", ParamValue::Int(2)),
        ("far_wake_level", ParamValue::Int(1)),
    ]);
    let flavour = ("flavour", ParamValue::Str("opencfd2606".into()));
    let dicts = cook(&reg, "cfd.mesh_setup", vec![j(&plan[0]), j(&domain[0]), vehicle], &[flavour, ("n_layers", ParamValue::Int(2)), ("np", ParamValue::Int(8))]);
    print_report("mesh setup", &dicts[1]);

    let built = cook(&reg, "cfd.mesh_build", vec![Some(dicts[0].clone())], &wsl_params(8));
    print_report("mesh build", &built[1]);
    println!("mesh case: {}", built[0].as_file().unwrap().display());
    assert!(built[1].as_report().unwrap().passes(), "the built mesh must pass its gates");
    let quality = cook(&reg, "cfd.mesh_quality", vec![Some(built[0].clone())], &[]);
    print_report("mesh quality", &quality[0]);

    let case = cook(&reg, "cfd.case_setup", vec![Some(built[0].clone()), j(&air[0]), j(&bc[0]), None, reference.clone(), j(&domain[0])], &[
        ("n_iter", ParamValue::Int(500)),
        ("window", ParamValue::Int(100)),
        ("write_interval", ParamValue::Int(500)),
        ("np", ParamValue::Int(8)),
    ]);
    print_report("case setup", &case[1]);
    let solve = cook(&reg, "cfd.solve_steady", vec![Some(case[0].clone())], &wsl_params(8));
    print_report("solve", &solve[2]);
    println!("solved case: {}", solve[0].as_file().unwrap().display());
    assert!(solve[2].as_report().unwrap().passes(), "the solver must leave a certified history");

    let half = domain[0].as_json().unwrap()["half_model"].as_bool().unwrap();
    let fc = cook(&reg, "cfd.force_coeffs", vec![j(&solve[1]), reference, j(&air[0])], &[("window", ParamValue::Int(100)), ("half_model", ParamValue::Bool(half))]);
    print_report("forces", &fc[1]);
    let res = ForceResult::from_json(fc[0].as_json().unwrap()).unwrap();
    assert!(!res.placeholder);
    // A bluff car shape on a coarse mesh: only plausibility, no accuracy claim.
    assert!(res.cd.is_finite() && res.cd > 0.05 && res.cd < 3.0, "{res:?}");
    assert!(res.cda > 0.0, "{res:?}");
    let history = crate::types::Series::from_json(solve[1].as_json().unwrap()).unwrap();
    let solved = history.solve_evidence.as_ref().expect("real measured mesh evidence must survive case setup and solve");
    assert_eq!(solved.mesh.cells as f64, built[1].as_report().unwrap().metrics["cells"].value);
    assert!(solved.reference_scope.is_none(), "a synthetic capability body is no reference geometry");
    let qualification = res.qualification.as_ref().expect("the real force history must carry same-window diagnostic evidence");
    assert_eq!(&qualification.solve, solved);
    assert_eq!(qualification.window, 100);
    assert_eq!(qualification.samples, history.len());
    assert_eq!(qualification.column, "Fx");
    println!("WP9_NATIVE_CHAIN_COMPLETE cells={} samples={} converged={} capability_only=true", solved.mesh.cells, history.len(), qualification.convergence.converged);
}

/// The published slant body in millimetres, nose at x = 0, ground at z = 0:
/// 1044 x 389 x 288 with a 100 radius front, 50 clearance and a 222 long
/// slant. The rounded front is a lofted inset of the section. Four 30 diameter
/// stilts run from below the ground into the body. Their diameter is published
/// in text; their x and y positions (202 and 672, +/-121.5) are assumed, as no
/// public text source for them was found.
fn slant_body(angle_deg: f64) -> Geometry {
    use vernon_core::attrib::AttribArray;
    use vernon_core::DVec3;
    let (length, half_w, z_low, z_high, radius, slant_len) = (1044.0_f64, 194.5_f64, 50.0_f64, 338.0_f64, 100.0_f64, 222.0_f64);
    let (run, drop) = (slant_len * angle_deg.to_radians().cos(), slant_len * angle_deg.to_radians().sin());
    let mut stations: Vec<(f64, f64, f64)> = Vec::new(); // x, inset, top drop
    for i in 0..=16 {
        let a = std::f64::consts::FRAC_PI_2 * f64::from(i) / 16.0;
        stations.push((radius * (1.0 - a.cos()), radius * (1.0 - a.sin()), 0.0));
    }
    stations.push((length - run, 0.0, 0.0));
    stations.push((length, 0.0, drop));
    let mut pts = Vec::new();
    for &(x, d, top) in &stations {
        pts.push(DVec3::new(x, -half_w + d, z_low + d));
        pts.push(DVec3::new(x, half_w - d, z_low + d));
        pts.push(DVec3::new(x, half_w - d, z_high - d - top));
        pts.push(DVec3::new(x, -half_w + d, z_high - d - top));
    }
    let mut faces: Vec<Vec<u32>> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    faces.push(vec![3, 2, 1, 0]);
    names.push("body".into());
    for s in 0..stations.len() as u32 - 1 {
        for c in 0..4u32 {
            let d = (c + 1) % 4;
            faces.push(vec![4 * s + c, 4 * s + d, 4 * (s + 1) + d, 4 * (s + 1) + c]);
            let slanted = s as usize == stations.len() - 2 && c == 2;
            names.push(if slanted { "body_slant" } else { "body" }.into());
        }
    }
    let last = 4 * (stations.len() as u32 - 1);
    faces.push(vec![last, last + 1, last + 2, last + 3]);
    names.push("body".into());
    for (x, y) in [(202.0, -121.5), (202.0, 121.5), (672.0, -121.5), (672.0, 121.5)] {
        let base = pts.len() as u32;
        let segs = 24u32;
        for z in [-10.0, 60.0] {
            for i in 0..segs {
                let a = std::f64::consts::TAU * f64::from(i) / f64::from(segs);
                pts.push(DVec3::new(x + 15.0 * a.cos(), y + 15.0 * a.sin(), z));
            }
        }
        for i in 0..segs {
            let j = (i + 1) % segs;
            faces.push(vec![base + i, base + j, base + segs + j, base + segs + i]);
            names.push("body_stilts".into());
        }
        faces.push((base..base + segs).rev().collect());
        names.push("body_stilts".into());
        faces.push((base + segs..base + 2 * segs).collect());
        names.push("body_stilts".into());
    }
    let mut g = Geometry::from_polygons(pts, faces);
    g.prim_attribs.set("patch", AttribArray::Str(names));
    g
}

#[test]
fn benchmark_measures_the_slant_share_from_the_case_and_fails_closed() {
    let reg = registry();
    let case = tmp("slant-share");
    std::fs::write(case.join("vernon_case.json"), r#"{"patches": ["body", "body_slant"]}"#).unwrap();
    let history = |name: &str, total: f64, pressure: f64| {
        let d = case.join("postProcessing").join(format!("forces_{name}")).join("0");
        std::fs::create_dir_all(&d).unwrap();
        let mut t = String::from("# Force\n# Time\ttotal_x total_y total_z\tpressure_x pressure_y pressure_z\tviscous_x viscous_y viscous_z\n");
        for i in 1..=10 {
            // Early rows differ, so a wrong window would give a wrong share.
            let f = if i <= 5 { 3.0 } else { 1.0 };
            t += &format!("{i} {} 0 0 {} 0 0 0 0 0\n", f * total, f * pressure);
        }
        std::fs::write(d.join("force.dat"), t).unwrap();
    };
    history("body", 60.0, 50.0);
    history("body_slant", 40.0, 38.0);
    let result = || Some(Data::json(force_json(0.3)));
    let set = [("case_id", ParamValue::Str("ahmed_25".into())), ("share_window", ParamValue::Int(5))];
    let out = cook(&reg, "cfd.benchmark_check", vec![result(), None, None, Some(Data::File(case.clone())), None], &set);
    let r = out[2].as_report().unwrap();
    assert!((r.metrics["slant_share_pct_ahmed_25"].value - 38.0).abs() < 1e-12, "{r:?}");
    assert!((r.metrics["slant_share_error_pts_ahmed_25"].value - 17.0).abs() < 1e-12, "{r:?}");

    // A case without the monitor reports that, never a share of zero.
    std::fs::remove_dir_all(case.join("postProcessing").join("forces_body")).unwrap();
    let out = cook(&reg, "cfd.benchmark_check", vec![result(), None, None, Some(Data::File(case)), None], &set);
    let r = out[2].as_report().unwrap();
    assert!(!r.metrics.contains_key("slant_share_pct_ahmed_25"));
    assert!(r.notes.iter().any(|(_, t)| t.contains("not measured")), "{r:?}");
    assert!(!r.passes(), "{r:?}");
    assert_eq!(r.metrics["within_band"].value, 0.0);
}

#[test]
fn slant_body_has_the_published_size_and_closed_parts() {
    for (angle, base_top) in [(25.0_f64, 338.0 - 222.0 * 25f64.to_radians().sin()), (35.0, 338.0 - 222.0 * 35f64.to_radians().sin())] {
        let g = slant_body(angle);
        g.validate().unwrap();
        let (b, names) = (g.bounds(), g.prim_attribs.strs("patch").unwrap());
        assert_eq!((b.min.x, b.max.x, b.min.y, b.max.y, b.max.z), (0.0, 1044.0, -194.5, 194.5, 338.0));
        assert_eq!(names.iter().filter(|n| *n == "body_slant").count(), 1);
        let slant = names.iter().position(|n| n == "body_slant").unwrap();
        let rear = g.prims[slant].vertices.iter().map(|&v| g.points[v as usize]).find(|p| p.x == 1044.0).unwrap();
        assert!((rear.z - base_top).abs() < 1e-9, "{}", rear.z);
        // Every edge of the body is shared by exactly two faces, in opposite directions.
        let mut edges = std::collections::HashMap::new();
        for (i, prim) in g.prims.iter().enumerate() {
            if names[i] == "body_stilts" {
                continue;
            }
            let v = &prim.vertices;
            for k in 0..v.len() {
                *edges.entry((v[k], v[(k + 1) % v.len()])).or_insert(0) += 1;
            }
        }
        assert!(edges.iter().all(|(&(a, b), &n)| n == 1 && edges.get(&(b, a)) == Some(&1)));
    }
}

/// The transient recipe through the real solver for a few time steps: it must
/// select the model, advance physical time and exit cleanly. This shows the
/// runtime accepts the files; it says nothing about detached-eddy accuracy.
#[test]
#[ignore = "needs WSL and OpenFOAM"]
fn real_openfoam_accepts_the_transient_recipe() {
    let reg = registry();
    let (body, wheels) = synthetic_car();
    let g = Data::geometry;
    let j = |d: &Data| Some(Data::Json(d.as_json().unwrap().clone()));
    let prep = cook(&reg, "cfd.vehicle_prep", vec![Some(g(body)), Some(g(wheels)), None], &[]);
    let (vehicle, reference) = (Some(prep[0].clone()), j(&prep[1]));
    let air = cook(&reg, "cfd.air_state", vec![], &[]);
    let domain = cook(&reg, "cfd.tunnel_domain", vec![reference.clone()], &[]);
    let bc = cook(&reg, "cfd.ground_wheels", vec![j(&domain[0]), reference.clone(), j(&air[0])], &[]);
    let plan = cook(&reg, "cfd.refinement_plan", vec![reference.clone(), j(&domain[0]), None, j(&air[0])], &[
        ("dx0_m", ParamValue::Float(0.64)),
        ("body_min", ParamValue::Int(2)),
        ("body_max", ParamValue::Int(3)),
        ("wheel_level", ParamValue::Int(3)),
        ("underbody_level", ParamValue::Int(2)),
        ("near_wake_level", ParamValue::Int(2)),
        ("far_wake_level", ParamValue::Int(1)),
    ]);
    let dicts = cook(&reg, "cfd.mesh_setup", vec![j(&plan[0]), j(&domain[0]), vehicle], &[("flavour", ParamValue::Str("opencfd2606".into())), ("n_layers", ParamValue::Int(2)), ("np", ParamValue::Int(8))]);
    let built = cook(&reg, "cfd.mesh_build", vec![Some(dicts[0].clone())], &wsl_params(8));
    assert!(built[1].as_report().unwrap().passes());
    let case = cook(&reg, "cfd.case_setup", vec![Some(built[0].clone()), j(&air[0]), j(&bc[0]), None, reference, j(&domain[0])], &[
        ("solver_mode", ParamValue::Str("transient_ddes".into())),
        ("end_time_s", ParamValue::Float(0.002)),
        ("delta_t_s", ParamValue::Float(2e-4)),
        ("max_delta_t_s", ParamValue::Float(2e-4)),
        ("write_interval_s", ParamValue::Float(0.001)),
        ("np", ParamValue::Int(1)),
    ]);
    let dir = case[0].as_file().unwrap().clone();
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k}"));
    let wsl = crate::extproc::WslOpts { distribution: var("VERNON_WSL_DISTRO"), user: var("VERNON_WSL_USER"), environment_file: var("VERNON_WSL_ENV_FILE") };
    let opts = crate::extproc::RunOpts { runner: "wsl".into(), image: String::new(), bin_dir: String::new(), timeout_min: 30 };
    let control = dir.parent().unwrap().join(".wsl-control");
    std::fs::create_dir_all(&control).unwrap();
    let steps = crate::mesh::transient_steps(1);
    let res = crate::extproc::run_chain_with_wsl(&opts, &wsl, &dir, &control, &steps);
    let log = std::fs::read_to_string(dir.join("logs/solver.log")).unwrap_or_default();
    println!("{}", log.lines().rev().take(25).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"));
    assert!(res.failed.is_none(), "{:?}", res.failed);
    assert!(log.contains("kOmegaSSTDDES"), "the log must name the selected model");
    assert!(log.contains("Selecting LES delta type maxDeltaxyz"), "the log must name the selected delta");
    assert!(log.contains("Courant Number") && log.contains("\nEnd"), "the run must advance and finish");
    assert!(log.lines().filter(|l| l.starts_with("Time = ")).count() >= 5, "at least a few physical time steps");
    assert!(dir.join("postProcessing/forceCoeffs1").is_dir());
}

/// One slant-body run with the real tools. Angle, base cell size and iteration
/// count come from the environment so one queue can run the whole study; a
/// result file holds what the study test then checks.
#[test]
#[ignore = "needs WSL and OpenFOAM"]
fn real_openfoam_slant_body_run() {
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k}"));
    let angle: f64 = var("VERNON_AHMED_ANGLE").parse().unwrap();
    let dx0: f64 = var("VERNON_AHMED_DX0").parse().unwrap();
    let n_iter: i64 = var("VERNON_AHMED_ITER").parse().unwrap();
    let np: i64 = var("VERNON_AHMED_NP").parse().unwrap();
    let out_file = var("VERNON_AHMED_OUT");
    let window = n_iter / 3;

    let reg = registry();
    let g = Data::geometry;
    let j = |d: &Data| Some(Data::Json(d.as_json().unwrap().clone()));
    let body = slant_body(angle);
    let prep = cook(&reg, "cfd.vehicle_prep", vec![Some(g(body.clone())), None, None], &[]);
    // The published reference area and length, not the stilted silhouette.
    let mut reference = prep[1].as_json().unwrap().as_ref().clone();
    reference["a_front_m2"] = serde_json::json!(0.112);
    reference["length_m"] = serde_json::json!(1.044);
    let reference = Some(Data::json(reference));
    let air = cook(&reg, "cfd.air_state", vec![], &[("u_ms", ParamValue::Float(40.0))]);
    let domain = cook(&reg, "cfd.tunnel_domain", vec![reference.clone()], &[
        ("blockage_pct", ParamValue::Float(2.0)),
        ("up_l", ParamValue::Float(3.0)),
        ("down_l", ParamValue::Float(5.0)),
    ]);
    let bc = cook(&reg, "cfd.ground_wheels", vec![j(&domain[0]), reference.clone(), j(&air[0])], &[
        ("ground_mode", ParamValue::Str("stationary".into())),
        ("wheel_mode", ParamValue::Str("static".into())),
        ("u_ms", ParamValue::Float(40.0)),
    ]);
    let plan = cook(&reg, "cfd.refinement_plan", vec![reference.clone(), j(&domain[0]), None, j(&air[0])], &[
        ("dx0_m", ParamValue::Float(dx0)),
        ("body_min", ParamValue::Int(4)),
        ("body_max", ParamValue::Int(4)),
        ("underbody_level", ParamValue::Int(3)),
        ("near_wake_level", ParamValue::Int(3)),
        ("far_wake_level", ParamValue::Int(2)),
    ]);
    let dicts = cook(&reg, "cfd.mesh_setup", vec![j(&plan[0]), j(&domain[0]), Some(g(body))], &[
        ("flavour", ParamValue::Str("opencfd2606".into())),
        ("n_layers", ParamValue::Int(5)),
        ("np", ParamValue::Int(np)),
    ]);
    print_report("mesh setup", &dicts[1]);
    let built = cook(&reg, "cfd.mesh_build", vec![Some(dicts[0].clone())], &wsl_params(np));
    print_report("mesh build", &built[1]);
    println!("mesh case: {}", built[0].as_file().unwrap().display());
    assert!(built[1].as_report().unwrap().passes(), "the built mesh must pass its gates");
    let quality = cook(&reg, "cfd.mesh_quality", vec![Some(built[0].clone())], &[]);
    print_report("mesh quality", &quality[0]);
    let case = cook(&reg, "cfd.case_setup", vec![Some(built[0].clone()), j(&air[0]), j(&bc[0]), None, reference.clone(), j(&domain[0])], &[
        ("n_iter", ParamValue::Int(n_iter)),
        ("window", ParamValue::Int(window)),
        ("write_interval", ParamValue::Int(n_iter)),
        ("np", ParamValue::Int(np)),
    ]);
    print_report("case setup", &case[1]);
    let mut run = wsl_params(np);
    run.iter_mut().find(|(k, _)| *k == "timeout_min").unwrap().1 = ParamValue::Int(600);
    let solve = cook(&reg, "cfd.solve_steady", vec![Some(case[0].clone())], &run);
    print_report("solve", &solve[2]);
    let solved = solve[0].as_file().unwrap().clone();
    println!("solved case: {}", solved.display());
    assert!(solve[2].as_report().unwrap().passes(), "the solver must leave a certified history");

    let conv = cook(&reg, "cfd.run_convergence", vec![j(&solve[1])], &[("window", ParamValue::Int(window))]);
    print_report("convergence", &conv[1]);
    let fc = cook(&reg, "cfd.force_coeffs", vec![j(&solve[1]), reference, j(&air[0])], &[("window", ParamValue::Int(window)), ("half_model", ParamValue::Bool(true))]);
    print_report("forces", &fc[1]);
    let res = ForceResult::from_json(fc[0].as_json().unwrap()).unwrap();
    assert!(!res.placeholder && res.cd.is_finite(), "{res:?}");

    let w = window as usize;
    let (body_p, body_t) = crate::coeffs::patch_force_x(&solved, "body", w).unwrap();
    let (slant_p, slant_t) = crate::coeffs::patch_force_x(&solved, "body_slant", w).unwrap();
    let (_, stilts_t) = crate::coeffs::patch_force_x(&solved, "body_stilts", w).unwrap();
    let total = body_t + slant_t + stilts_t;
    let row = serde_json::json!({
        "angle_deg": angle, "dx0_m": dx0, "iterations": solve[2].as_report().unwrap().metrics["iterations_read"].value,
        "cells": quality[0].as_report().unwrap().metrics["cells"].value,
        "max_non_ortho_deg": quality[0].as_report().unwrap().metrics["max_non_ortho_deg"].value,
        "max_skewness": quality[0].as_report().unwrap().metrics["max_skewness"].value,
        "layer_coverage_pct": quality[0].as_report().unwrap().metrics.get("layer_coverage_pct").map(|m| m.value),
        "converged": conv[1].as_report().unwrap().passes(),
        "cd": res.cd, "cl": res.cl,
        "slant_share_pct": 100.0 * slant_p / total,
        "stilt_share_pct": 100.0 * stilts_t / total,
        "body_pressure_share_pct": 100.0 * body_p / total,
        "result": fc[0].as_json().unwrap().as_ref(),
        "case": solved.display().to_string(),
    });
    println!("{row}");
    std::fs::write(out_file, serde_json::to_string_pretty(&row).unwrap()).unwrap();
}

/// A real history holds both the forces and the solver's own coefficients. A
/// wrong half-model setting, density or speed must fail, not double the drag.
#[test]
fn force_coefficients_catch_settings_that_are_not_the_runs() {
    let n = 400_u32;
    let mut s = Series { time: (1..=n).map(f64::from).collect(), ..Series::default() };
    // Half of a 2.0 m2 car at 30 m/s and 1.225 kg/m3 with Cd 0.32.
    let q = 0.5 * 1.225 * 30.0 * 30.0;
    s.columns.insert("Cd".into(), vec![0.32; n as usize]);
    s.columns.insert("Cl".into(), vec![-0.1; n as usize]);
    s.columns.insert("Fx".into(), vec![0.32 * q; n as usize]);
    s.columns.insert("Fz".into(), vec![-0.1 * q; n as usize]);
    s.meta.insert("Aref".into(), 1.0);
    s.meta.insert("magUInf".into(), 30.0);
    let mut rf = crate::types::RefGeom::reference_car();
    rf.a_front_m2 = 2.0;
    let run = |half: bool, rho: f64| {
        let spec = crate::coeffs::ForceSpec { rf: &rf, half_model: half, rho, window: 100, u_wire: None, u_param: 30.0, cofr_x: None, drag_z: None };
        crate::coeffs::compute_forces(&s, &spec).unwrap()
    };
    let good = run(true, 1.225);
    assert!((good.result.cd - 0.32).abs() < 1e-9);
    assert_eq!(good.aref_mismatch_pct, 0.0);
    assert!(good.solver_cd_mismatch_pct.unwrap() < 1e-9);

    let wrong_half = run(false, 1.225);
    assert!(wrong_half.aref_mismatch_pct > 1.0);
    assert!(wrong_half.solver_cd_mismatch_pct.unwrap() > 1.0);
    assert!(wrong_half.notes.iter().any(|(sev, _)| *sev == vernon_core::report::Severity::Error));

    let wrong_density = run(true, 1.098);
    assert_eq!(wrong_density.aref_mismatch_pct, 0.0);
    assert!(wrong_density.solver_cd_mismatch_pct.unwrap() > 1.0);
}

/// Drag and its iteration spread from the finished slant-body runs of
/// 2026-10-31, fine to coarse (`real_openfoam_slant_body_run`, 2.03, 0.99 and
/// 0.50 million cells).
const SLANT_25: [(f64, f64); 3] = [(0.368674, 0.012156), (0.370936, 0.006741), (0.394400, 0.005532)];
const SLANT_35: [(f64, f64); 3] = [(0.314428, 0.002703), (0.471618, 0.007463), (0.485866, 0.012089)];

#[test]
fn recorded_slant_body_runs_do_not_pass_the_mesh_study() {
    let reg = registry();
    let input = |(cd, sd): (f64, f64)| {
        let mut v = force_json(cd);
        v["cd_sd"] = serde_json::json!(sd);
        Some(Data::json(v))
    };
    let study = |runs: [(f64, f64); 3]| cook(&reg, "cfd.mesh_study", runs.into_iter().map(input).collect(), &[("r21", ParamValue::Float(1.2697)), ("r32", ParamValue::Float(1.2550))]);

    // 25 degrees: the two finest grids agree, but only inside the run's own
    // scatter, and the order is too high to believe. It must not be accepted.
    let out = study(SLANT_25);
    let r = out[2].as_report().unwrap();
    assert!(r.metrics["diff_fine_medium_pct"].value < 2.0, "{r:?}");
    assert!(r.metrics["iteration_scatter_pct"].value > 3.0, "{r:?}");
    assert!(r.metrics["order_p"].value > 4.0, "{r:?}");
    assert_eq!(out[0].as_json().unwrap()["pass"], false);
    assert!(!r.passes());

    // 35 degrees: the finest grid lands in a different flow state.
    let out = study(SLANT_35);
    let r = out[2].as_report().unwrap();
    assert!(r.metrics["diff_fine_medium_pct"].value > 40.0, "{r:?}");
    assert_eq!(out[0].as_json().unwrap()["pass"], false);

    // Only the finest pair has the experimental drag ordering.
    let ordering = |a, b| {
        let out = cook(&reg, "cfd.benchmark_check", vec![input(a), input(b), None, None, None], &[("case_id", ParamValue::Str("ahmed_set".into()))]);
        out[2].as_report().unwrap().metrics["drag_ordering_ok"].value
    };
    assert_eq!(ordering(SLANT_25[0], SLANT_35[0]), 1.0);
    assert_eq!(ordering(SLANT_25[1], SLANT_35[1]), 0.0);
    assert_eq!(ordering(SLANT_25[2], SLANT_35[2]), 0.0);
}

/// The finished runs on disk through the real study and benchmark nodes.
#[test]
#[ignore = "needs the finished slant-body runs"]
fn real_slant_body_runs_through_the_study_and_benchmark() {
    let dir = std::path::PathBuf::from(std::env::var("VERNON_AHMED_RESULTS").expect("set VERNON_AHMED_RESULTS"));
    let load = |label: String| -> Value { serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{label}.json"))).unwrap()).unwrap() };
    let reg = registry();
    let result = |row: &Value| Some(Data::json(row["result"].clone()));
    let mut finest = Vec::new();
    for angle in ["a25", "a35"] {
        let rows = ["g3", "g2", "g1"].map(|g| load(format!("{angle}-{g}")));
        let cells = |i: usize| rows[i]["cells"].as_f64().unwrap();
        let (r21, r32) = ((cells(0) / cells(1)).cbrt(), (cells(1) / cells(2)).cbrt());
        println!("{angle}: r21 {r21:.4}, r32 {r32:.4}");
        let out = cook(&reg, "cfd.mesh_study", rows.iter().map(result).collect(), &[("r21", ParamValue::Float(r21)), ("r32", ParamValue::Float(r32))]);
        print_report(&format!("mesh study {angle}"), &out[2]);
        assert_eq!(out[0].as_json().unwrap()["pass"], false, "no slant-body study is accepted yet");
        finest.push(rows[0].clone());
    }
    let case = |row: &Value| Some(Data::File(row["case"].as_str().unwrap().into()));
    let out = cook(&reg, "cfd.benchmark_check", vec![result(&finest[0]), result(&finest[1]), None, case(&finest[0]), case(&finest[1])], &[("case_id", ParamValue::Str("ahmed_set".into()))]);
    print_report("benchmark", &out[2]);
    let r = out[2].as_report().unwrap();
    assert_eq!(r.metrics["drag_ordering_ok"].value, 1.0);
    assert!((r.metrics["slant_share_pct_ahmed_25"].value - finest[0]["slant_share_pct"].as_f64().unwrap()).abs() < 1e-9);
    assert!((r.metrics["slant_share_pct_ahmed_35"].value - finest[1]["slant_share_pct"].as_f64().unwrap()).abs() < 1e-9);
}

/// One road-car rear shape with the real tools, from the six original surface
/// files kept outside the repository. Setting `VERNON_CAR_MESH_ONLY` stops after
/// the mesh checks. This runs the accepted published-replication recipe, not an
/// independently authenticated reconstruction of the original experiment.
#[test]
#[ignore = "needs WSL, OpenFOAM and the local road-car surfaces"]
fn real_openfoam_road_car_run() {
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k}"));
    let dir = std::path::PathBuf::from(var("VERNON_CAR_DIR"));
    let rear = var("VERNON_CAR_REAR");
    let dx0: f64 = var("VERNON_CAR_DX0").parse().unwrap();
    let n_iter: i64 = std::env::var("VERNON_CAR_ITER").unwrap_or_else(|_| "7000".into()).parse().unwrap();
    let np: i64 = var("VERNON_CAR_NP").parse().unwrap();
    let out_file = var("VERNON_CAR_OUT");
    let window = 1000;
    let mesh_only = match std::env::var("VERNON_CAR_MESH_ONLY") {
        Ok(value) if value == "1" => true,
        Ok(value) if value == "0" => false,
        Err(std::env::VarError::NotPresent) => false,
        _ => panic!("VERNON_CAR_MESH_ONLY must be absent, 0 or 1"),
    };
    assert!(matches!(rear.as_str(), "Fastback" | "Estate" | "Notchback"));
    assert!(dx0.is_finite() && dx0 > 0.0);
    assert!((2000..=7000).contains(&n_iter), "two fixed 1000-sample windows need at least 2000 iterations; writer maximum is 7000");
    assert!((1..=512).contains(&np));
    let read = |name: &str| vernon_io::read(&dir.join(format!("part_{name}.stl")), &Default::default()).unwrap();
    let mut body = read("01_Body");
    for part in ["02_UB_Detailed", &format!("03_{rear}"), "07_Mirror"] {
        body.merge(&read(part));
    }
    let mut wheels = read("05_Wheels_Front");
    wheels.merge(&read("06_Wheels_Rear"));
    println!("surfaces read: body {} faces, wheels {} faces", body.prims.len(), wheels.prims.len());

    let reg = registry();
    let g = Data::geometry;
    let j = |d: &Data| Some(Data::Json(d.as_json().unwrap().clone()));
    let prep = cook(&reg, "cfd.vehicle_prep", vec![Some(g(body)), Some(g(wheels)), None], &[
        ("nose_direction", ParamValue::Str("-x".into())), ("half_model", ParamValue::Bool(true)),
        ("align_origin", ParamValue::Bool(true)), ("sink_mm", ParamValue::Float(2.0)),
    ]);
    print_report("vehicle", &prep[2]);
    let measured = prep[1].as_json().unwrap();
    println!("measured reference: {measured}");
    // The published reference area and length, as the benchmark preset gives them.
    let mut reference = measured.as_ref().clone();
    reference["a_front_m2"] = serde_json::json!(2.16);
    reference["length_m"] = serde_json::json!(4.6);
    let reference = Some(Data::json(reference));
    let air_value = serde_json::json!({"rho": 1.184, "nu": 1.567e-5, "mu": 1.855328e-5, "u_ms": 16.0, "q": 151.552});
    let air = Data::json(air_value.clone());
    let domain = cook(&reg, "cfd.tunnel_domain", vec![reference.clone()], &[
        ("half_model", ParamValue::Bool(true)), ("yaw_deg", ParamValue::Float(0.0)),
    ]);
    let bc = cook(&reg, "cfd.ground_wheels", vec![j(&domain[0]), reference.clone(), j(&air)], &[
        ("u_ms", ParamValue::Float(16.0)), ("r_dyn_mm", ParamValue::Float(16000.0 / 51.0)),
        ("ground_mode", ParamValue::Str("moving".into())), ("wheel_mode", ParamValue::Str("rotating".into())),
        ("axis", ParamValue::Vec3(glam::DVec3::Y)), ("belt_start", ParamValue::Bool(false)),
    ]);
    print_report("ground and wheels", &bc[1]);
    let plan = cook(&reg, "cfd.refinement_plan", vec![reference.clone(), j(&domain[0]), None, j(&air)], &[("dx0_m", ParamValue::Float(dx0))]);
    let dicts = cook(&reg, "cfd.mesh_setup", vec![j(&plan[0]), j(&domain[0]), Some(prep[0].clone())], &[
        ("flavour", ParamValue::Str("opencfd2606".into())),
        ("n_layers", ParamValue::Int(5)),
        ("np", ParamValue::Int(np)),
        ("max_global_cells_m", ParamValue::Float(14.0)),
        ("ram_gb", ParamValue::Float(15.52)),
    ]);
    print_report("mesh setup", &dicts[1]);
    let long = |minutes: i64| {
        let mut run = wsl_params(np);
        run.iter_mut().find(|(k, _)| *k == "timeout_min").unwrap().1 = ParamValue::Int(minutes);
        run
    };
    // Leave one hour for outer-worker cleanup within the fixed six-hour sizing bound.
    let built = cook(&reg, "cfd.mesh_build", vec![Some(dicts[0].clone())], &long(300));
    print_report("mesh build", &built[1]);
    println!("mesh case: {}", built[0].as_file().unwrap().display());
    assert!(built[1].as_report().unwrap().passes(), "the built mesh must pass its gates");
    let quality = cook(&reg, "cfd.mesh_quality", vec![Some(built[0].clone())], &[("ram_gb", ParamValue::Float(15.52))]);
    print_report("mesh quality", &quality[0]);
    let q = quality[0].as_report().unwrap();
    let mut row = serde_json::json!({
        "rear": rear, "dx0_m": dx0, "cells": q.metrics["cells"].value,
        "max_non_ortho_deg": q.metrics["max_non_ortho_deg"].value,
        "max_skewness": q.metrics["max_skewness"].value,
        "layer_coverage_pct": q.metrics.get("layer_coverage_pct").map(|m| m.value),
        "a_front_measured_m2": measured["a_front_m2"],
        "profile": "road_car_detailed_gs_16ms_v1", "scope": "published_replication",
        "original_experiment_authenticated": false, "physical_validation": false,
        "mesh_only": mesh_only, "solver_ran": false, "np": np,
        "n_iter": n_iter, "window": window, "write_interval": 1000,
        "air": air_value, "boundary": bc[0].as_json().unwrap().as_ref(),
        "reference": reference.as_ref().unwrap().as_json().unwrap().as_ref(),
        "domain": domain[0].as_json().unwrap().as_ref(),
        "normalization_stage": "prepared_inputs",
        "normalization": {"u_ms": 16.0, "rho": 1.184, "nu": 1.567e-5, "q_pa": 151.552, "a_front_m2": 2.16, "aref_m2": 1.08, "half_model": true},
        "preparation": {"nose_direction": "-x", "half_model": true, "align_origin": true, "sink_mm": 2.0},
        "max_global_cells_m": 14.0, "ram_gb": 15.52,
        "mesh_case": built[0].as_file().unwrap(), "case": built[0].as_file().unwrap(),
        "mesh_setup_report": dicts[1].as_report().unwrap().as_ref(),
        "mesh_quality_report": q.as_ref(),
        "mesh_quality_passes": q.passes(),
    });
    if !mesh_only {
        let case = cook(&reg, "cfd.case_setup", vec![Some(built[0].clone()), j(&air), j(&bc[0]), None, reference.clone(), j(&domain[0])], &[
            ("n_iter", ParamValue::Int(n_iter)),
            ("window", ParamValue::Int(window)),
            ("write_interval", ParamValue::Int(1000)),
            ("np", ParamValue::Int(np)),
            ("solver_mode", ParamValue::Str("steady_sst".into())),
            ("u_ms", ParamValue::Float(16.0)), ("rho_inf", ParamValue::Float(1.184)), ("nu", ParamValue::Float(1.567e-5)),
            ("turbulence_intensity_pct", ParamValue::Float(0.25)), ("nut_ratio", ParamValue::Float(10.0)),
            ("residual_target", ParamValue::Float(1e-5)),
        ]);
        print_report("case setup", &case[1]);
        let solve = cook(&reg, "cfd.solve_steady", vec![Some(case[0].clone())], &long(1800));
        print_report("solve", &solve[2]);
        println!("solved case: {}", solve[0].as_file().unwrap().display());
        assert!(solve[2].as_report().unwrap().passes(), "the solver must leave a certified history");
        let fc = cook(&reg, "cfd.force_coeffs", vec![j(&solve[1]), reference, j(&air)], &[("window", ParamValue::Int(window)), ("half_model", ParamValue::Bool(true))]);
        print_report("forces", &fc[1]);
        let res = ForceResult::from_json(fc[0].as_json().unwrap()).unwrap();
        assert!(!res.placeholder && res.cd.is_finite(), "{res:?}");
        assert_eq!(res.window, 1000);
        assert!((res.u_ms - 16.0).abs() < 1e-12 && (res.rho - 1.184).abs() < 1e-12);
        assert!((res.a_front_m2 - 2.16).abs() < 1e-12 && (res.aref_m2 - 1.08).abs() < 1e-12 && res.half_model);
        let column = match res.source.as_str() { "forces" => "Fx", "coefficients" => "Cd", other => panic!("unexpected force source {other}") };
        let conv = cook(&reg, "cfd.run_convergence", vec![j(&solve[1])], &[("window", ParamValue::Int(window)), ("column", ParamValue::Str(column.into()))]);
        print_report("convergence", &conv[1]);
        let qualification = res.qualification.as_ref().expect("the real run must retain measured solve/mesh evidence");
        assert_eq!(qualification.column, column);
        assert_eq!(qualification.window, 1000);
        assert_eq!(qualification.convergence.converged, conv[1].as_report().unwrap().passes());
        assert_eq!(fc[1].as_report().unwrap().metrics[vernon_core::keys::CFD_ITERATION_QUALIFIED].value, f64::from(qualification.convergence.converged));
        let bench = cook(&reg, "cfd.benchmark_check", vec![j(&fc[0]), None, None, None, None], &[("case_id", ParamValue::Str(format!("drivaer_{}", rear.to_lowercase())))]);
        print_report("benchmark", &bench[2]);
        row["converged"] = serde_json::json!(conv[1].as_report().unwrap().passes());
        row["cd"] = serde_json::json!(res.cd);
        row["cl"] = serde_json::json!(res.cl);
        let benchmark_report = bench[2].as_report().unwrap();
        row["coefficient_band_ok"] = serde_json::json!(benchmark_report.metrics["coefficient_band_ok"].value == 1.0);
        row["benchmark_validated"] = serde_json::json!(benchmark_report.metrics["benchmark_validated"].value == 1.0 && benchmark_report.passes());
        row["within_band"] = row["benchmark_validated"].clone();
        row["benchmark_report"] = serde_json::to_value(benchmark_report.as_ref()).unwrap();
        row["force_report"] = serde_json::to_value(fc[1].as_report().unwrap().as_ref()).unwrap();
        row["convergence"] = conv[0].as_json().unwrap().as_ref().clone();
        row["solver_ran"] = serde_json::json!(true);
        row["normalization_stage"] = serde_json::json!("force_result");
        row["normalization"] = serde_json::json!({"u_ms": res.u_ms, "rho": res.rho, "q_pa": res.q_pa, "a_front_m2": res.a_front_m2, "aref_m2": res.aref_m2, "half_model": res.half_model});
        row["result"] = fc[0].as_json().unwrap().as_ref().clone();
        row["case"] = serde_json::json!(solve[0].as_file().unwrap().display().to_string());
    }
    println!("{row}");
    std::fs::write(&out_file, serde_json::to_string_pretty(&row).unwrap()).unwrap();
    let saved: Value = serde_json::from_slice(&std::fs::read(&out_file).unwrap()).unwrap();
    assert_eq!(saved, row, "the saved run summary must match the diagnostic row");
    assert_eq!(saved["window"], 1000);
    assert_eq!(saved["physical_validation"], false);
    if mesh_only {
        assert_eq!(saved["solver_ran"], false);
        assert!(saved["cells"].as_f64().is_some_and(|cells| cells.is_finite() && cells > 0.0));
        assert!(!saved.as_object().unwrap().contains_key("result"));
        println!("WP9_REPLICATION_MESH_ONLY_COMPLETE rear={rear} cells={} solver_runs=0", saved["cells"]);
    }
}

#[test]
fn data_kinds_of_declared_ports_match_the_documented_chain() {
    // the Json hand-offs the chain relies on are declared as Json, files as File
    let reg = registry();
    let kind = |id: &str, name: &str| reg.info(id).unwrap().outputs.iter().find(|p| p.name == name).unwrap().kind;
    assert_eq!(kind("cfd.force_coeffs", "result"), DataKind::Json);
    assert_eq!(kind("cfd.mesh_setup", "case"), DataKind::File);
    assert_eq!(kind("cfd.surface_pressure", "surface"), DataKind::Geometry);
    assert_eq!(kind("cfd.compare_ab", "delta"), DataKind::Json);
}

#[test]
fn saved_json_rows_read_back_bit_exact() {
    // values from a real sizing row that came back one ulp off without float_roundtrip
    let row = serde_json::json!({"min_m": [-13.799999999999999, 0.0], "yplus": 93.69359645412291});
    let text = serde_json::to_string_pretty(&row).unwrap();
    let saved: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(saved, row);
    assert_eq!(saved["min_m"][0].as_f64().unwrap().to_bits(), (-13.799999999999999f64).to_bits());
}

/// Export the three local replication assemblies and meshing dictionaries only.
/// No mesh build, solver, shell or external process is invoked by this fixture.
#[test]
#[ignore = "needs the local original road-car surfaces and an explicit output path"]
fn replication_geometry_preparation() {
    let var = |key: &str| std::env::var(key).unwrap_or_else(|_| panic!("set {key}"));
    let dir = std::path::PathBuf::from(var("VERNON_CAR_DIR"));
    let output = std::path::PathBuf::from(var("VERNON_REPLICATION_GEOMETRY_OUT"));
    assert!(dir.is_dir(), "the original component directory must exist");
    assert!(output.is_absolute(), "the export manifest needs an explicit absolute output path");
    let reg = registry();
    let j = |d: &Data| Some(Data::Json(d.as_json().unwrap().clone()));
    let air_value = serde_json::json!({"rho": 1.184, "nu": 1.567e-5, "mu": 1.855328e-5, "u_ms": 16.0, "q": 151.552});
    let air = Data::json(air_value.clone());
    let preparation = serde_json::json!({
        "nose_direction": "-x", "half_model": true, "align_origin": true, "sink_mm": 2.0,
        "input_length_unit": "mm", "export_length_unit": "m", "export_length_scale": 0.001,
        "flow_direction": "+x", "ground_mode": "moving", "wheel_mode": "rotating",
        "r_dyn_mm": 16000.0 / 51.0, "axis": [0.0, 1.0, 0.0], "belt_start": false,
        "dx0_m": 0.32, "flavour": "opencfd2606", "np": 4, "n_layers": 5,
        "max_global_cells_m": 14.0, "ram_gb": 15.52,
        "a_front_full_m2": 2.16, "a_front_half_m2": 1.08, "length_m": 4.6,
    });
    let mut rows = Vec::new();
    for rear in ["Fastback", "Estate", "Notchback"] {
        let names = [
            "part_01_Body.stl".to_string(), "part_02_UB_Detailed.stl".to_string(),
            format!("part_03_{rear}.stl"), "part_07_Mirror.stl".to_string(),
            "part_05_Wheels_Front.stl".to_string(), "part_06_Wheels_Rear.stl".to_string(),
        ];
        let source_files: Vec<Value> = names.iter().map(|name| {
            let path = dir.join(name);
            let bytes = std::fs::metadata(&path).unwrap().len();
            assert!(bytes > 0, "{rear}: empty original component {name}");
            serde_json::json!({"filename": name, "path": path, "bytes": bytes})
        }).collect();
        let read = |name: &str| vernon_io::read(&dir.join(name), &Default::default()).unwrap();
        let mut body = read(&names[0]);
        for name in &names[1..4] {
            body.merge(&read(name));
        }
        let mut wheels = read(&names[4]);
        wheels.merge(&read(&names[5]));
        let (body_bounds, wheel_bounds) = (body.bounds(), wheels.bounds());
        let raw_min = body_bounds.min.min(wheel_bounds.min);
        let raw_max = body_bounds.max.max(wheel_bounds.max);
        let translation_mm = [-raw_min.x, -0.5 * (raw_min.y + raw_max.y), -2.0 - wheel_bounds.min.z];
        let prep = cook(&reg, "cfd.vehicle_prep", vec![Some(Data::geometry(body)), Some(Data::geometry(wheels)), None], &[
            ("nose_direction", ParamValue::Str("-x".into())), ("half_model", ParamValue::Bool(true)),
            ("align_origin", ParamValue::Bool(true)), ("sink_mm", ParamValue::Float(2.0)),
        ]);
        print_report(&format!("{rear} vehicle preparation"), &prep[2]);
        let measured = prep[1].as_json().unwrap().as_ref().clone();
        assert_eq!(measured["half_model"], true);
        assert!(measured["nose_x_m"].as_f64().unwrap().abs() < 1e-9);
        assert!((measured["z_min_m"].as_f64().unwrap() + 0.002).abs() < 1e-9);
        assert!(measured["x_front_axle_m"].as_f64().unwrap() < measured["x_rear_axle_m"].as_f64().unwrap());
        assert!(measured["a_front_m2"].as_f64().unwrap().is_finite() && measured["a_front_m2"].as_f64().unwrap() > 0.0);
        assert_eq!(prep[2].as_report().unwrap().metrics["wheels_found"].value, 4.0);
        let mut reference_value = measured.clone();
        reference_value["a_front_m2"] = serde_json::json!(2.16);
        reference_value["length_m"] = serde_json::json!(4.6);
        let reference = Data::json(reference_value.clone());
        let domain = cook(&reg, "cfd.tunnel_domain", vec![j(&reference)], &[
            ("half_model", ParamValue::Bool(true)), ("yaw_deg", ParamValue::Float(0.0)),
        ]);
        let boundary = cook(&reg, "cfd.ground_wheels", vec![j(&domain[0]), j(&reference), j(&air)], &[
            ("u_ms", ParamValue::Float(16.0)), ("r_dyn_mm", ParamValue::Float(16000.0 / 51.0)),
            ("ground_mode", ParamValue::Str("moving".into())), ("wheel_mode", ParamValue::Str("rotating".into())),
            ("axis", ParamValue::Vec3(glam::DVec3::Y)), ("belt_start", ParamValue::Bool(false)),
        ]);
        print_report(&format!("{rear} ground and wheels"), &boundary[1]);
        let plan = cook(&reg, "cfd.refinement_plan", vec![j(&reference), j(&domain[0]), None, j(&air)], &[("dx0_m", ParamValue::Float(0.32))]);
        let dicts = cook(&reg, "cfd.mesh_setup", vec![j(&plan[0]), j(&domain[0]), Some(prep[0].clone())], &[
            ("flavour", ParamValue::Str("opencfd2606".into())), ("np", ParamValue::Int(4)),
            ("n_layers", ParamValue::Int(5)), ("max_global_cells_m", ParamValue::Float(14.0)),
            ("ram_gb", ParamValue::Float(15.52)),
        ]);
        print_report(&format!("{rear} dictionaries only; no physical mesh"), &dicts[1]);
        let case = dicts[0].as_file().unwrap();
        let manifest: Value = serde_json::from_slice(&std::fs::read(case.join("vernon_case.json")).unwrap()).unwrap();
        assert!(crate::mesh::reference_meshing_recipe_matches(&ctx("replication-recipe-reader"), case, &manifest).unwrap(), "actual prepared dictionaries must match their structured meshing recipe");
        assert_eq!(manifest["flavour"], "opencfd2606");
        assert_eq!(manifest["np"], 4);
        assert_eq!(manifest["domain"]["half_model"], true);
        assert_eq!(manifest["reference"]["a_front_m2"], 2.16);
        assert_eq!(manifest["reference"]["length_m"], 4.6);
        assert_eq!(manifest["plan"]["u_ms"], 16.0);
        assert_eq!(manifest["plan"]["nu"], 1.567e-5);
        let bc = boundary[0].as_json().unwrap();
        assert_eq!(bc["u_ms"], 16.0);
        assert_eq!(bc["ground_mode"], "moving");
        assert_eq!(bc["wheel_mode"], "rotating");
        assert!((bc["omega_rad_s"].as_f64().unwrap() + 51.0).abs() < 1e-10);
        let surface_dir = case.join("constant/triSurface");
        let mut surface_names: Vec<String> = std::fs::read_dir(&surface_dir).unwrap().map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("stl"))
            .map(|path| path.file_name().unwrap().to_str().unwrap().to_string()).collect();
        surface_names.sort();
        assert_eq!(surface_names, ["body.stl", "wheel_fr.stl", "wheel_rr.stl"]);
        let surfaces: Vec<Value> = surface_names.iter().map(|name| {
            let path = surface_dir.join(name);
            let bytes = std::fs::metadata(&path).unwrap().len();
            assert!(bytes > 0, "{rear}: empty prepared surface {name}");
            serde_json::json!({"filename": name, "path": path, "bytes": bytes})
        }).collect();
        for name in ["blockMeshDict", "snappyHexMeshDict", "meshQualityDict", "decomposeParDict"] {
            assert!(std::fs::metadata(case.join("system").join(name)).unwrap().len() > 0);
        }
        assert!(!case.join("constant/polyMesh/points").exists(), "this fixture must not produce a volume mesh");
        let geometry_binding = crate::reference::geometry_binding(&ctx("replication-geometry-reader"), case)
            .unwrap().expect("fresh prepared bytes must match the independently captured geometry registry");
        assert_eq!(geometry_binding.candidate, format!("drivaer_{}", rear.to_lowercase()));
        let tri_surface_fingerprint = geometry_binding.tri_surface_fingerprint.clone();
        let reference_spec = crate::types::RefGeom::from_json(&reference_value).unwrap();
        let domain_spec = crate::types::Domain::from_json(domain[0].as_json().unwrap().as_ref()).unwrap();
        let boundary_spec = crate::types::BcSet::from_json(bc.as_ref()).unwrap();
        assert_eq!(boundary_spec.wheels.len(), 2, "a prepared half model must carry only its two right wheel boundaries");
        let case_spec = crate::foam::CaseSpec {
            flavour: crate::foam::Flavour::OpenCfd(2606), domain: &domain_spec, rf: &reference_spec,
            bc: &boundary_spec, zones: &[], u: 16.0, nu: 1.567e-5, rho: 1.184,
            i_pct: 0.25, nut_ratio: 10.0, n_iter: 7000, window: 1000, residual: 1e-5,
            write_interval: 1000, nonortho_over_70: false,
            monitor_patches: manifest["patches"].as_array().unwrap().iter().map(|patch| patch.as_str().unwrap().to_string()).collect(),
        };
        let protocol_binding = crate::reference::match_case_protocol(&geometry_binding, &case_spec, "steady_sst")
            .expect("actual prepared reference/domain/wheel settings must match the published-replication protocol");
        assert!(crate::reference::binding_supported(&protocol_binding));
        assert!(!case.join("vernon_replication.json").exists(), "a geometry-only protocol check must not create solve or reference evidence");
        let report = dicts[1].as_report().unwrap();
        assert_eq!(report.metrics["surfaces_written"].value, 3.0);
        assert_eq!(report.metrics["ram_available_gb"].value, 15.52);
        let memory_capacity_limited = report.metrics["ram_estimate_gb_high"].value > report.metrics["ram_available_gb"].value;
        assert!(memory_capacity_limited, "the current memory limitation must remain visible");
        rows.push(serde_json::json!({
            "rear": rear, "case": case, "source_files": source_files, "surfaces": surfaces,
            "tri_surface_fingerprint": tri_surface_fingerprint,
            "geometry_binding": geometry_binding, "protocol_binding": protocol_binding,
            "measured_reference": measured, "reference": reference_value, "preparation": preparation,
            "translation_mm": translation_mm, "air": air_value, "domain": domain[0].as_json().unwrap().as_ref(),
            "boundary": bc.as_ref(), "plan": plan[0].as_json().unwrap().as_ref(),
            "reports": {"vehicle": prep[2].as_report().unwrap().as_ref(), "boundary": boundary[1].as_report().unwrap().as_ref(), "mesh_setup": report.as_ref()},
            "memory_capacity_limited": memory_capacity_limited, "mesh_setup_report_passes": report.passes(),
            "physical_validation": false, "reference_accepted": false, "component_sha_verified": false, "prepared_sha_verified": false,
        }));
    }
    assert_eq!(rows.len(), 3);
    let result = serde_json::json!({"schema_version": 1, "cases": rows, "external_programs": 0, "physical_validation": false, "reference_accepted": false});
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&output, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    let saved: Value = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    assert_eq!(saved["cases"].as_array().unwrap().len(), 3);
    assert_eq!(saved["external_programs"], 0);
    assert_eq!(saved["physical_validation"], false);
    println!("WP9_REPLICATION_GEOMETRY_COMPLETE cases=3 external_programs=0");
}

#[test]
fn window_percentiles_use_the_shared_nearest_rank_convention() {
    // 1..10: nearest-rank p95 is 10 (linear interpolation would give 9.55),
    // the same number the clean and surface stages gate on.
    let v: Vec<f64> = (1..=10).map(f64::from).collect();
    let s = crate::coeffs::window_stats(&v).unwrap();
    assert_eq!(s.p95, 10.0);
    assert_eq!(s.p05, 1.0);
    assert!((s.sd - vernon_core::stats::sample_std(&v)).abs() < 1e-15);
}

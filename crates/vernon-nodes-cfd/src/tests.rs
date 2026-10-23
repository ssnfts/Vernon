//! Crate-level tests: registration, smoke cooking of every node, and the full
//! chain from surfaces to site loads driven only through the registry.

use std::path::Path;

use serde_json::Value;
use vernon_core::Geometry;
use vernon_graph::{CookContext, Data, DataKind, NodeTypeInfo, ParamKind, ParamValue, Params, Registry, Stage};

use crate::coeffs::ForceResult;
use crate::types::Series;
use crate::util::testutil::{ctx, synthetic_car, tmp};

const EXPECTED: [&str; 20] = [
    "air_state",
    "frontal_area",
    "symmetry_check",
    "vehicle_prep",
    "tunnel_domain",
    "ground_wheels",
    "porous_core",
    "refinement_plan",
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

//! Crate-level tests: registration, smoke cooking of every node, and the full
//! chain driven only through the registry.

use glam::DVec3;
use vernon_core::{Geometry, Metric, Report};
use vernon_graph::{CookContext, Data, DataKind, ParamKind, ParamValue, Params, Registry, Stage};

use crate::mesh::{box_tets, tet4_to_tet10};
use crate::util::testing::ctx;

const EXPECTED: [&str; 20] = [
    "fea.material_fdm",
    "fea.layer_axes",
    "fea.material_derate",
    "fea.temperature_map",
    "fea.creep_plan",
    "fea.material_table",
    "fea.tet_mesh",
    "fea.mesh_check",
    "fea.mount_sites",
    "fea.load_aero",
    "fea.load_bump",
    "fea.load_cases",
    "fea.deck_write",
    "fea.solve_external",
    "fea.solve_linear",
    "fea.result_read",
    "fea.safety_factor",
    "fea.mount_report",
    "fea.mesh_study",
    "fea.hand_check",
];

/// Names that must never reach a user-visible string.
const FORBIDDEN: [&str; 17] = [
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
    "abaqus",
];

fn registry() -> Registry {
    let mut r = Registry::new();
    crate::register(&mut r);
    r
}

#[test]
fn registration_is_complete_and_well_formed() {
    let reg = registry();
    assert_eq!(reg.len(), EXPECTED.len());
    for id in EXPECTED {
        let info = reg.info(id).unwrap_or_else(|| panic!("{id} is not registered"));
        assert_eq!(info.stage, Stage::Fea, "{id}");
        assert!(id.starts_with("fea."), "{id}");
        assert!(!info.help.trim().is_empty(), "{id} has no help");
        assert!(!info.emulates.is_empty(), "{id} has no emulates");
        assert_eq!(info.spec.as_deref(), Some("docs/specs/structural-fea.md"), "{id}");
        assert!(!info.outputs.is_empty(), "{id} has no outputs");
        for p in &info.params {
            assert!(!p.label.is_empty(), "{id}.{}", p.name);
            if let ParamKind::Float { default, min, max } = p.kind {
                assert!(min <= default && default <= max, "{id}.{} default {default} outside {min}..{max}", p.name);
            }
            if let ParamKind::Int { default, min, max } = p.kind {
                assert!(min <= default && default <= max, "{id}.{} default {default} outside {min}..{max}", p.name);
            }
            if let ParamKind::Enum { default, options } = &p.kind {
                assert!(options.contains(default), "{id}.{}", p.name);
            }
        }
        // user-visible strings carry no vendor names
        let mut text = vec![info.label.clone(), info.help.clone()];
        for p in &info.params {
            text.push(p.label.clone());
            text.push(p.help.clone());
            if let Some(u) = &p.unit {
                text.push(u.clone());
            }
            if let ParamKind::Enum { options, .. } = &p.kind {
                text.extend(options.iter().cloned());
            }
        }
        for port in info.inputs.iter().chain(info.outputs.iter()) {
            text.push(port.name.clone());
        }
        for t in text {
            let low = t.to_lowercase();
            for bad in FORBIDDEN {
                assert!(!low.contains(bad), "{id}: '{t}' contains '{bad}'");
            }
        }
    }
    // tools run as separate programs are named, the rest is white-box or implemented
    for (id, tool) in [("fea.deck_write", "CalculiX"), ("fea.solve_external", "CalculiX"), ("fea.result_read", "CalculiX"), ("fea.tet_mesh", "fTetWild")] {
        assert_eq!(reg.info(id).unwrap().external_tool.as_deref(), Some(tool), "{id}");
    }
    assert!(reg.info("fea.tet_mesh").unwrap().whitebox, "the real mesher's output has not been checked");
    for id in ["fea.solve_linear", "fea.material_fdm", "fea.material_table", "fea.load_aero", "fea.deck_write", "fea.result_read", "fea.safety_factor", "fea.mount_report", "fea.hand_check"] {
        assert!(!reg.info(id).unwrap().whitebox, "{id} is implemented for real");
    }
}

struct Model {
    mesh: Data,
    surface: Data,
    card: Data,
    mounts: Data,
    cases: Data,
}

fn cook(reg: &Registry, id: &str, ins: Vec<Option<Data>>, tweak: &[(&str, ParamValue)]) -> Vec<Data> {
    let op = reg.get(id).unwrap();
    let info = reg.info(id).unwrap();
    let mut p = Params::from_specs(&info.params);
    for (k, v) in tweak {
        p.set(k, v.clone());
    }
    let c: CookContext = ctx(&id.replace('.', "_"));
    let out = op.cook(&c, &ins, &p).unwrap_or_else(|e| panic!("{id}: {e}"));
    assert_eq!(out.len(), info.outputs.len(), "{id} output count");
    for (d, port) in out.iter().zip(info.outputs.iter()) {
        assert!(port.kind == DataKind::Any || d.kind() == port.kind, "{id}.{} has the wrong kind", port.name);
    }
    out
}

fn model(reg: &Registry) -> Model {
    let mesh = Data::geometry(tet4_to_tet10(&box_tets(DVec3::new(60.0, 40.0, 6.0), [12, 8, 2])).unwrap());
    let surface = Data::geometry(Geometry::make_box(DVec3::new(60.0, 40.0, 6.0)));
    let card = cook(reg, "fea.material_fdm", vec![None], &[])[0].clone();
    let mounts = cook(reg, "fea.mount_sites", vec![Some(mesh.clone())], &[("site_a", ParamValue::Vec3(DVec3::new(-25.0, 0.0, 3.0))), ("model", ParamValue::Str("fixed_patch".into()))])[0].clone();
    let aero = cook(reg, "fea.load_aero", vec![Some(mesh.clone())], &[])[0].clone();
    let bump = cook(reg, "fea.load_bump", vec![Some(mesh.clone()), Some(card.clone())], &[])[0].clone();
    let cases = cook(reg, "fea.load_cases", vec![Some(aero.clone()), Some(bump.clone()), Some(mounts.clone())], &[])[0].clone();
    Model { mesh, surface, card, mounts, cases }
}

#[test]
fn every_node_cooks_on_a_small_model() {
    let reg = registry();
    let m = model(&reg);
    let m_in = |i: &Data| Some(i.clone());
    let decks = cook(&reg, "fea.deck_write", vec![m_in(&m.mesh), m_in(&m.card), m_in(&m.mounts), m_in(&m.cases)], &[]);
    cook(&reg, "fea.solve_external", vec![m_in(&decks[0])], &[]);
    let lin = cook(&reg, "fea.solve_linear", vec![m_in(&m.mesh), m_in(&m.card), m_in(&m.mounts), m_in(&m.cases)], &[]);
    let fields = cook(&reg, "fea.result_read", vec![m_in(&lin[0]), m_in(&m.mesh)], &[]);
    let sf = cook(&reg, "fea.safety_factor", vec![m_in(&fields[0]), m_in(&fields[1]), m_in(&m.card), None, m_in(&m.mounts)], &[]);
    cook(&reg, "fea.mount_report", vec![m_in(&fields[1]), m_in(&m.cases), m_in(&m.mounts), m_in(&sf[2])], &[]);
    cook(&reg, "fea.hand_check", vec![m_in(&m.card), m_in(&m.mounts), None], &[]);
    let mut r = Report::new("level");
    for k in ["max_disp_mm", "mount_vm_avg_mpa", "sf_peak_mount_min", "sf_mount_min"] {
        r.metric(k, Metric::new(1.0, ""));
    }
    let lvl = Data::report(r);
    cook(&reg, "fea.mesh_study", vec![m_in(&lvl), m_in(&lvl), m_in(&lvl)], &[]);
    cook(&reg, "fea.material_derate", vec![m_in(&m.card)], &[]);
    cook(&reg, "fea.temperature_map", vec![m_in(&m.mesh)], &[]);
    cook(&reg, "fea.temperature_map", vec![m_in(&m.mesh), m_in(&hot_surface(&m.surface)), m_in(&m.card)], &[]);
    cook(&reg, "fea.layer_axes", vec![None], &[]);
    cook(&reg, "fea.creep_plan", vec![m_in(&m.card)], &[]);
    cook(&reg, "fea.material_table", vec![], &[]);
    cook(&reg, "fea.mesh_check", vec![m_in(&m.mesh)], &[]);
    cook(&reg, "fea.tet_mesh", vec![m_in(&m.surface)], &[("edge_len_mm", ParamValue::Float(6.0))]);
}

fn hot_surface(surface: &Data) -> Data {
    let mut g = (**surface.as_geometry().unwrap()).clone();
    let n = g.points.len();
    g.point_attribs.set("temp_c", vernon_core::AttribArray::Float(vec![60.0; n]));
    Data::geometry(g)
}

#[test]
fn nodes_with_missing_inputs_fail_cleanly() {
    let reg = registry();
    for id in EXPECTED {
        let info = reg.info(id).unwrap();
        if info.inputs.iter().all(|p| !p.required) {
            continue;
        }
        let ins: Vec<Option<Data>> = vec![None; info.inputs.len()];
        let p = Params::from_specs(&info.params);
        let res = reg.get(id).unwrap().cook(&ctx("missing"), &ins, &p);
        assert!(res.is_err(), "{id} cooked with required inputs missing");
    }
}

#[test]
fn full_chain_on_the_reference_solver_gates_by_element_order() {
    let reg = registry();
    let m = model(&reg);
    let s = |d: &Data| Some(d.clone());
    let decks = cook(&reg, "fea.deck_write", vec![s(&m.mesh), s(&m.card), s(&m.mounts), s(&m.cases)], &[]);
    // the external solver is not run: no result files exist and the report fails, nothing is invented
    let ext = cook(&reg, "fea.solve_external", vec![s(&decks[0])], &[]);
    assert!(!ext[1].as_report().unwrap().passes());
    let op = reg.get("fea.result_read").unwrap();
    let p = Params::from_specs(&reg.info("fea.result_read").unwrap().params);
    assert!(op.cook(&ctx("chain"), &[s(&ext[0]), s(&m.mesh)], &p).is_err());
    // ten-node reference results count; four-node ones stay flagged as estimates
    let lin = cook(&reg, "fea.solve_linear", vec![s(&m.mesh), s(&m.card), s(&m.mounts), s(&m.cases)], &[]);
    let fields = cook(&reg, "fea.result_read", vec![s(&lin[0]), s(&m.mesh)], &[]);
    let sf = cook(&reg, "fea.safety_factor", vec![s(&fields[0]), s(&fields[1]), s(&m.card), None, s(&m.mounts)], &[]);
    let rep = sf[1].as_report().unwrap();
    assert!(rep.metrics["sf_min"].value > 0.0 && !rep.metrics.contains_key("estimate_only"));
    let linear = Data::geometry(box_tets(DVec3::new(60.0, 40.0, 6.0), [36, 24, 4]));
    let mounts = cook(&reg, "fea.mount_sites", vec![s(&linear)], &[("site_a", ParamValue::Vec3(DVec3::new(-25.0, 0.0, 3.0))), ("model", ParamValue::Str("fixed_patch".into()))]);
    let aero = cook(&reg, "fea.load_aero", vec![s(&linear)], &[]);
    let cases = cook(&reg, "fea.load_cases", vec![s(&aero[0]), None, s(&mounts[0])], &[]);
    let lin = cook(&reg, "fea.solve_linear", vec![s(&linear), s(&m.card), s(&mounts[0]), s(&cases[0])], &[]);
    let fields = cook(&reg, "fea.result_read", vec![s(&lin[0]), s(&linear)], &[]);
    let sf = cook(&reg, "fea.safety_factor", vec![s(&fields[0]), s(&fields[1]), s(&m.card), None, s(&mounts[0])], &[]);
    let rep = sf[1].as_report().unwrap();
    assert!(!rep.passes());
    assert_eq!(rep.metrics["estimate_only"].value, 1.0);
    let cases_in = s(&cases[0]);
    let mount = cook(&reg, "fea.mount_report", vec![s(&fields[1]), cases_in, s(&mounts[0]), s(&sf[2])], &[]);
    assert_eq!(mount[0].as_report().unwrap().metrics["estimate_only"].value, 1.0, "the mount report carries the flag");
}

#[test]
fn speed_sweep_without_a_new_solve_scales_with_the_inverse_square() {
    // G9: SF at 250 km/h = SF at 200 km/h * 0.64 for an aero-only load
    let reg = registry();
    let m = model(&reg);
    let s = |d: &Data| Some(d.clone());
    let sf_at = |v: f64| {
        let aero = cook(&reg, "fea.load_aero", vec![s(&m.mesh)], &[("v_kmh", ParamValue::Float(v))]);
        let cases = cook(&reg, "fea.load_cases", vec![s(&aero[0]), None, s(&m.mounts)], &[]);
        let lin = cook(&reg, "fea.solve_linear", vec![s(&m.mesh), s(&m.card), s(&m.mounts), s(&cases[0])], &[]);
        let fields = cook(&reg, "fea.result_read", vec![s(&lin[0]), s(&m.mesh)], &[]);
        let sf = cook(&reg, "fea.safety_factor", vec![s(&fields[0]), s(&fields[1]), s(&m.card), None, s(&m.mounts)], &[]);
        sf[1].as_report().unwrap().metrics["sf_min"].value
    };
    let (a, b) = (sf_at(200.0), sf_at(250.0));
    assert!((b / a - 0.64).abs() < 1e-3, "{a} {b}");
    // G12: the speed at which the safety factor reaches the target follows from the square law
    let aero = cook(&reg, "fea.load_aero", vec![s(&m.mesh)], &[]);
    let cases = cook(&reg, "fea.load_cases", vec![s(&aero[0]), None, s(&m.mounts)], &[]);
    let lin = cook(&reg, "fea.solve_linear", vec![s(&m.mesh), s(&m.card), s(&m.mounts), s(&cases[0])], &[]);
    let fields = cook(&reg, "fea.result_read", vec![s(&lin[0]), s(&m.mesh)], &[]);
    let sf = cook(&reg, "fea.safety_factor", vec![s(&fields[0]), s(&fields[1]), s(&m.card), None, s(&m.mounts)], &[]);
    let rep = sf[1].as_report().unwrap();
    // the over-speed case (1.25 x 200 km/h) governs: SF there is SF(200) / 1.5625
    let v_lim = rep.metrics["speed_limit_kmh"].value;
    let expect = 200.0 * 1.25 * (rep.metrics["sf_min"].value / 3.0).sqrt();
    assert!((v_lim - expect).abs() < 1e-6 * expect, "{v_lim} {expect}");
}

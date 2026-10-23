//! Tests: registration, smoke cooks and golden numbers from the spec
//! (`docs/specs/performance-validation.md`, section 5).

use std::collections::HashSet;

use glam::DVec3;
use serde_json::{json, Value};
use vernon_core::{AttribArray, Geometry, Metric, Report};
use vernon_graph::{CookContext, CookResult, Data, DataKind, ParamKind, ParamValue, Params, Registry, Stage};

use crate::coast::{self, CoastOpts, Method, Run};
use crate::cp::{self, CpOpts, TapIn, TuftClass, TuftThresholds};
use crate::gate::{self, GateInput};
use crate::lapsim::{self, LapOptions, SweepParam, TimeRule};
use crate::nodes_calc::parse_curve;
use crate::nodes_track::parse_segment_text;
use crate::physics::{self, Env, Vehicle, G};
use crate::track::{track_from_path, ClosedMode, PathOpts, Track};

fn reg() -> Registry {
    let mut r = Registry::new();
    crate::register(&mut r);
    r
}

fn ctx() -> CookContext {
    CookContext { node_name: "test".into(), work_dir: std::env::temp_dir(), frame: 0.0 }
}

fn cook(id: &str, inputs: Vec<Option<Data>>, set: &[(&str, ParamValue)]) -> CookResult<Vec<Data>> {
    let r = reg();
    let op = r.get(id).unwrap_or_else(|| panic!("no node {id}"));
    let info = op.info();
    let mut p = Params::from_specs(&info.params);
    for (k, v) in set {
        p.set(k, v.clone());
    }
    op.cook(&ctx(), &inputs, &p)
}

fn f(x: f64) -> ParamValue {
    ParamValue::Float(x)
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-300)
}

fn close(a: f64, b: f64, tol: f64, what: &str) {
    assert!((a - b).abs() <= tol, "{what}: {a} vs {b} (tol {tol})");
}

// the spec's reference car
const RHO: f64 = 1.11;

fn ref_car() -> Vehicle {
    Vehicle { mass_kg: 1400.0, power_w: 150_000.0, cda_m2: 0.64, cla_m2: 0.5, mu_x: 1.0, mu_y: 1.0, crr: 0.012, ..Vehicle::default() }
}

fn stadium(ds: f64) -> Track {
    Track::from_segments(&parse_segment_text("S400, L180deg@40, S400, L180deg@40").unwrap(), true, ds).unwrap()
}

fn lap(veh: &Vehicle, rho: f64, tr: &Track) -> f64 {
    lapsim::lap_time(veh, rho, tr, &LapOptions::default()).unwrap()
}

// ------------------------------------------------------------------ registration

const BANNED: [&str; 22] = [
    "rhino", "fusion", "autodesk", "3ds max", "quicksurface", "geomagic", "zeiss", "airshaper", "simscale", "ansys",
    "optimumlap", "optimumg", "plasticity", "polycam", "ladybug", "cloudcompare", "houdini", "openlap", "magnehelic",
    "edgar", "squish", "orca",
];

fn assert_clean(what: &str, text: &str) {
    let low = text.to_lowercase();
    for b in BANNED {
        assert!(!low.contains(b), "{what} contains '{b}': {text}");
    }
}

const EXPECTED: [&str; 18] = [
    "perf.air_density",
    "perf.aero_loads",
    "perf.duct_heat",
    "perf.top_speed",
    "perf.vehicle",
    "perf.gg_envelope",
    "perf.corner_limits",
    "perf.track_from_path",
    "perf.track_from_segments",
    "perf.track_edit",
    "perf.lap_sim",
    "perf.lap_sweep",
    "perf.lap_delta",
    "perf.coastdown_fit",
    "perf.pressure_cp",
    "perf.cp_compare",
    "perf.tuft_analysis",
    "perf.validation_gate",
];

#[test]
fn registration_ids_prefixes_help_and_spec() {
    let r = reg();
    assert_eq!(r.len(), EXPECTED.len());
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut ids = HashSet::new();
    for info in r.types() {
        assert!(ids.insert(info.id.clone()), "duplicate {}", info.id);
        assert!(info.id.starts_with("perf."), "{}", info.id);
        assert_eq!(info.stage, Stage::Performance, "{}", info.id);
        assert!(!info.help.trim().is_empty() && !info.label.trim().is_empty(), "{}", info.id);
        assert!(!info.emulates.is_empty(), "{} has no emulates", info.id);
        assert!(!info.outputs.is_empty(), "{} has no outputs", info.id);
        let spec = info.spec.as_deref().unwrap_or_else(|| panic!("{} has no spec", info.id));
        assert!(root.join(spec).is_file(), "spec {spec} missing");
        assert_clean(&info.id, &info.label);
        assert_clean(&info.id, &info.help);
        let mut names = HashSet::new();
        for p in &info.params {
            assert!(names.insert(p.name.clone()), "{} duplicate param {}", info.id, p.name);
            assert!(!p.help.trim().is_empty(), "{}.{} has no help", info.id, p.name);
            assert_clean(&info.id, &p.label);
            assert_clean(&info.id, &p.help);
            match &p.kind {
                ParamKind::Enum { default, options } => {
                    assert!(options.contains(default), "{}.{} default not an option", info.id, p.name);
                    options.iter().for_each(|o| assert_clean(&info.id, o));
                }
                ParamKind::Float { default, min, max } => assert!(min <= default && default <= max, "{}.{} range", info.id, p.name),
                ParamKind::Int { default, min, max } => assert!(min <= default && default <= max, "{}.{} range", info.id, p.name),
                _ => {}
            }
        }
        for port in info.inputs.iter().chain(&info.outputs) {
            assert_clean(&info.id, &port.name);
        }
    }
    for id in EXPECTED {
        assert!(ids.contains(id), "missing {id}");
    }
    let wb: Vec<&str> = r.types().filter(|i| i.whitebox).map(|i| i.id.as_str()).collect();
    assert_eq!(wb, vec!["perf.tuft_analysis"]);
}

// ------------------------------------------------------------------ smoke

fn sphere_with_cp() -> Geometry {
    // octahedron-like fan: a unit-ish plate of two triangles with a Cp field
    let pts = vec![DVec3::new(0.0, 0.0, 0.0), DVec3::new(100.0, 0.0, 0.0), DVec3::new(100.0, 100.0, 0.0), DVec3::new(0.0, 100.0, 0.0)];
    let mut g = Geometry::from_polygons(pts, vec![vec![0, 1, 2], vec![0, 2, 3]]);
    g.point_attribs.set("Cp", AttribArray::Float(vec![-0.2, -0.4, -0.4, -0.2]));
    g
}

fn circle_curve(r_m: f64, n: usize) -> Geometry {
    let pts: Vec<DVec3> = (0..n)
        .map(|i| {
            let a = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
            DVec3::new(r_m * 1000.0 * a.cos(), r_m * 1000.0 * a.sin(), 0.0)
        })
        .collect();
    let mut g = Geometry::from_points(pts);
    g.add_prim(vernon_core::Prim { vertices: (0..n as u32).collect(), closed: true });
    g
}

fn coast_log_json(runs: usize) -> Value {
    let a = 0.012 * 1400.0 * G;
    let b = 0.5 * RHO * 0.64;
    let mut list = Vec::new();
    for k in 0..runs {
        let (t, v) = coast::simulate_log(a, b, 1400.0, 120.0 / 3.6, 15.0 / 3.6, 10.0);
        list.push(json!({"t": t, "v_ms": v, "dir": if k % 2 == 0 { 1 } else { -1 }, "temp_c": 40.0, "p_pa": 101325.0}));
    }
    json!({"runs": list})
}

fn taps_json() -> Value {
    json!([
        {"id": "a", "x": 10.0, "y": 10.0, "z": 0.0, "dp_pa": -91.0, "speed_ms": 27.78, "rho": 1.11},
        {"id": "b", "x": 60.0, "y": 50.0, "z": 0.0, "dp_pa": -103.0, "speed_ms": 27.78, "rho": 1.11}
    ])
}

fn input_for(name: &str, kind: DataKind) -> Option<Data> {
    let veh = ref_car().to_json();
    match (name, kind) {
        ("env", _) => Some(Data::json(Env::from_rho(RHO).to_json())),
        ("vehicle" | "vehicle_a", _) => Some(Data::json(veh)),
        ("vehicle_b", _) => {
            let mut v = ref_car();
            v.cda_m2 = 0.69;
            Some(Data::json(v.to_json()))
        }
        ("track", _) => Some(Data::json(stadium(1.0).to_json(0.001, 3.0))),
        ("curve", _) => Some(Data::geometry(circle_curve(15.25, 120))),
        ("aero", _) => {
            let mut r = Report::new("aero");
            r.metric("CdA", Metric::new(0.62, "m2")).metric("ClA", Metric::new(-0.4, "m2"));
            Some(Data::report(r))
        }
        ("log", _) => Some(Data::json(coast_log_json(4))),
        ("taps", _) => Some(Data::json(taps_json())),
        ("cp", DataKind::Json) => {
            let taps = cp::parse_taps_json(&taps_json()).unwrap();
            let o = CpOpts { rho_default: None, speed_default: None, speed_sd_ms: 0.3, dp_sd_pa: 3.0, rho_sd_frac: 0.01 };
            Some(Data::json(cp::cp_table_json(&cp::compute_cp(&taps, &o).unwrap())))
        }
        ("cp" | "sweep" | "top_speed", DataKind::Report) => {
            let mut r = Report::new("upstream");
            r.metric("rms_cp", Metric::new(0.03, "-"))
                .metric("sign_disagree", Metric::new(0.0, "taps"))
                .metric("dt_dcda", Metric::new(0.53, "s/m2"))
                .metric("dt_dcla", Metric::new(-0.19, "s/m2"))
                .metric("dvmax_dcda_kmh_per_m2", Metric::new(-130.0, "km/h per m2"));
            Some(Data::report(r))
        }
        ("surface", _) => Some(Data::geometry(sphere_with_cp())),
        ("fit", _) => Some(Data::json(json!({"cda_m2": 0.64, "cda_ci95": [0.6, 0.68], "min_detectable_dcda_m2": 0.02}))),
        _ => None,
    }
}

#[test]
fn smoke_every_node_cooks_with_defaults() {
    let r = reg();
    for info in r.types() {
        let op = r.get(&info.id).unwrap();
        let p = Params::from_specs(&info.params);
        let inputs: Vec<Option<Data>> = info.inputs.iter().map(|port| input_for(&port.name, port.kind)).collect();
        let out = op.cook(&ctx(), &inputs, &p).unwrap_or_else(|e| panic!("{} failed on synthetic input: {e}", info.id));
        assert_eq!(out.len(), info.outputs.len(), "{} output count", info.id);
        for (d, port) in out.iter().zip(&info.outputs) {
            assert_eq!(d.kind(), port.kind, "{} port {}", info.id, port.name);
        }
        // no inputs at all: a clean error or a result, never a panic
        let none: Vec<Option<Data>> = vec![None; info.inputs.len()];
        if let Ok(out) = op.cook(&ctx(), &none, &p) {
            assert_eq!(out.len(), info.outputs.len(), "{} output count without inputs", info.id);
        }
    }
}

// ------------------------------------------------------------------ 1-4, 21-23: closed forms

#[test]
fn golden_density() {
    let dry = physics::air_density(40.0, 101325.0, 0.0, 287.05);
    assert!(rel(dry, 1.12721) < 1e-4, "{dry}");
    close(100.0 * (dry / 1.22501 - 1.0), -7.98, 0.02, "dry 40 C vs 15 C");
    let humid = physics::air_density(45.0, 101325.0, 0.30, 287.058);
    assert!(rel(humid, 1.09757) < 1e-4, "{humid}");
    close(physics::sat_pressure_pa(45.0), 9582.0, 5.0, "p_sat");
    // the note prints 1.127 and 1.098
    assert_eq!(format!("{dry:.3}"), "1.127");
    assert_eq!(format!("{humid:.3}"), "1.098");
}

#[test]
fn golden_aero_loads() {
    let v = 200.0 / 3.6;
    let q = physics::dyn_pressure(RHO, v);
    close(q, 1712.96, 0.01, "q");
    close(q * 0.64, 1096.3, 0.05, "drag");
    close(physics::aero_power(RHO, 0.64, v), 60905.0, 5.0, "power");
    close(q * 0.5, 856.48, 0.01, "downforce");
    close(q * 0.5 / G, 87.34, 0.01, "kgf");
    let q250 = physics::dyn_pressure(RHO, 250.0 / 3.6);
    close(q250, 2676.50, 0.01, "q250");
    close(q250 * 0.5, 1338.25, 0.01, "downforce 250");
    close(q250 / q, 1.5625, 1e-12, "q ratio");
}

#[test]
fn golden_aero_loads_node() {
    let out = cook("perf.aero_loads", vec![Some(Data::json(Env::from_rho(RHO).to_json())), None], &[("use_vehicle", ParamValue::Bool(false)), ("design_kmh", f(200.0)), ("speeds_kmh", ParamValue::Str("100, 200, 250".into()))]).unwrap();
    let rep = out[1].as_report().unwrap();
    close(rep.metrics["drag_design"].value, 1096.3, 0.05, "drag");
    close(rep.metrics["downforce_design"].value, 856.48, 0.01, "down");
    close(rep.metrics["power_design"].value, 60.905, 0.005, "kW");
    let t = out[0].as_json().unwrap();
    let rows = t["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    close(rows[2][4].as_f64().unwrap(), 1338.25, 0.01, "downforce at 250");
}

#[test]
fn golden_corner_gain() {
    let m = 1400.0;
    for (r, cla, v0, v1, gain) in [(50.0, 0.5, 22.143, 22.254, 0.499), (200.0, 0.5, 44.287, 45.192, 2.043), (200.0, 1.0, 44.287, 46.154, 4.217)] {
        let k = physics::corner_speed(1.0, RHO, cla, m, r);
        let k0 = physics::corner_speed(1.0, RHO, 0.0, m, r);
        close(k0, v0, 0.001, "no-aero");
        close(k, v1, 0.001, "with aero");
        close(100.0 * (k / k0 - 1.0), gain, 0.002, "gain");
    }
    close(1.0 * RHO * 0.5 * 50.0 / (2.0 * m), 0.009911, 1e-6, "k at R 50");
    close(1.0 * RHO * 0.5 * 200.0 / (2.0 * m), 0.039643, 1e-6, "k at R 200");
    // the vehicle's own limit agrees with the closed form
    let veh = ref_car();
    close(veh.corner_limit(RHO, 1.0 / 200.0), 45.192, 0.001, "vehicle corner limit");
}

#[test]
fn golden_top_speed() {
    let mk = |cda: f64| Vehicle { cda_m2: cda, cla_m2: 0.0, rr_includes_downforce: false, ..ref_car() };
    let t0 = mk(0.64).top_speed(RHO).v_ms * 3.6;
    let t1 = mk(0.69).top_speed(RHO).v_ms * 3.6;
    let t2 = mk(0.62).top_speed(RHO).v_ms * 3.6;
    close(t0, 262.67, 0.05, "0.64");
    close(t1, 256.35, 0.05, "0.69");
    close(t2, 265.39, 0.05, "0.62");
    close(100.0 * (t1 / t0 - 1.0), -2.41, 0.02, "speed change");
    close(t2 - t0, 2.71, 0.05, "0.02 less drag");
    let ts = mk(0.64).top_speed(RHO);
    close(ts.p_aero_w + ts.p_roll_w, 150_000.0, 1.0, "power balance");
}

#[test]
fn golden_top_speed_node_and_sensitivity() {
    let out = cook(
        "perf.top_speed",
        vec![Some(Data::json(Env::from_rho(RHO).to_json())), None],
        &[("use_vehicle", ParamValue::Bool(false)), ("cda_m2", f(0.64))],
    )
    .unwrap();
    let rep = out[0].as_report().unwrap();
    close(rep.metrics["v_max"].value, 262.67, 0.05, "node v_max");
    let dv = rep.metrics["dvmax_dcda_kmh_per_m2"].value;
    assert!((-140.0..-120.0).contains(&dv), "{dv}");
}

#[test]
fn golden_braking_and_envelope() {
    let a200 = physics::braking_decel(1.0, RHO, 0.64, 0.5, 1400.0, 200.0 / 3.6);
    let a100 = physics::braking_decel(1.0, RHO, 0.64, 0.5, 1400.0, 100.0 / 3.6);
    assert!(rel(a200, 11.2015) < 1e-6 * 10.0, "{a200}");
    assert!(rel(a100, 10.1554) < 1e-5, "{a100}");
    close(a200 / G, 1.1422, 1e-4, "g");
    close(100.0 * (a200 / G - 1.0), 14.22, 0.01, "+%");
    close(100.0 * (a100 / G - 1.0), 3.56, 0.01, "+% at 100");
    // with rolling on the normal load, the lap solver's deceleration includes it
    let veh = Vehicle { crr: 0.012, ..ref_car() };
    let (a_dec, _) = veh.decel(RHO, 200.0 / 3.6, 0.0);
    assert!(rel(a_dec, 11.3265) < 1e-4, "{a_dec}");
    // grip envelope at 50 m/s, no rolling term
    let veh0 = Vehicle { crr: 0.0, ..ref_car() };
    let n = veh0.normal_load(RHO, 50.0);
    let ay_max = veh0.mu_y_at(n) * n / veh0.mass_kg;
    assert!(rel(ay_max, 10.3022) < 1e-5, "{ay_max}");
    for (frac, want) in [(0.6, 8.2417), (0.8, 6.1813), (1.0, 0.0)] {
        let g = veh0.grip(RHO, 50.0, frac * ay_max / 2500.0);
        close(g.fx_max / veh0.mass_kg, want, 1e-3, "ellipse");
    }
}

#[test]
fn golden_balance_and_duct() {
    close(physics::aero_balance_pct(0.2, 0.3), 40.0, 1e-12, "balance");
    let m1 = physics::duct_mass_flow(RHO, 20.0, 0.01);
    close(m1, 0.2220, 1e-9, "mdot");
    close(m1 * 1005.0 * 30.0, 6693.3, 1e-6, "Q");
    let m2 = physics::duct_mass_flow(RHO, 30.0, 0.02);
    close(m2, 0.6660, 1e-9, "mdot 2");
    close(m2 * 1005.0 * 25.0, 16733.25, 1e-6, "Q 2");
    // node round trip and the inverse
    let out = cook("perf.duct_heat", vec![Some(Data::json(Env::from_rho(RHO).to_json()))], &[("duct_speed_ms", f(20.0)), ("duct_area_m2", f(0.01)), ("delta_t_k", f(30.0))]).unwrap();
    close(out[0].as_report().unwrap().metrics["heat"].value, 6693.3, 1e-6, "node Q");
    let inv = cook("perf.duct_heat", vec![Some(Data::json(Env::from_rho(RHO).to_json()))], &[("solve_for", ParamValue::Str("delta_t".into())), ("heat_w", f(6693.3))]).unwrap();
    close(inv[0].as_report().unwrap().metrics["delta_t"].value, 30.0, 1e-6, "node dT");
}

#[test]
fn aero_balance_node_column() {
    let out = cook(
        "perf.aero_loads",
        vec![None, None],
        &[("use_vehicle", ParamValue::Bool(false)), ("cla_front_m2", f(0.2)), ("cla_rear_m2", f(0.3)), ("scale_from_rho", f(1.225)), ("rho_kgm3", f(RHO))],
    )
    .unwrap();
    let rep = out[1].as_report().unwrap();
    close(rep.metrics["front_balance"].value, 40.0, 1e-9, "balance metric");
    close(rep.metrics["density_factor"].value, RHO / 1.225, 1e-12, "density factor");
    let names: Vec<&str> = out[0].as_json().unwrap()["columns"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"front_balance_pct") && names.contains(&"drag_ref_n"));
}

// ------------------------------------------------------------------ 5-11: lap solver

#[test]
fn golden_launch_constant_power() {
    // no drag, no rolling resistance, no aero: traction-limited then constant power
    let veh = Vehicle { cda_m2: 0.0, cla_m2: 0.0, crr: 0.0, mu_x: 1.0, v_cap_ms: 150.0, ..ref_car() };
    let tr = Track::straight(300.0, 0.5).unwrap();
    let res = lapsim::solve(&veh, RHO, &tr, &LapOptions::default()).unwrap();
    let v_trac = 150_000.0 / (1400.0 * G);
    close(v_trac, 10.9255, 1e-4, "traction end speed");
    let t100 = res.time_to_speed(100.0 / 3.6).unwrap();
    assert!(rel(t100, 4.1579) < 0.005, "0-100 km/h {t100}");
    let t_trac = res.time_to_speed(v_trac).unwrap();
    assert!(rel(t_trac, 1.1141) < 0.01, "traction time {t_trac}");
    // distance to the end of the traction phase
    let i = res.v.iter().position(|v| *v >= v_trac).unwrap();
    let frac = (v_trac - res.v[i - 1]) / (res.v[i] - res.v[i - 1]);
    let s_trac = res.s[i - 1] + frac * (res.s[i] - res.s[i - 1]);
    assert!(rel(s_trac, 6.086) < 0.01, "traction distance {s_trac}");
    // standing start gives a finite time
    assert!(res.lap_time_s.is_finite() && res.lap_time_s > 0.0);
}

#[test]
fn golden_skidpad() {
    let veh = Vehicle { cda_m2: 0.0, cla_m2: 0.0, crr: 0.0, mu_x: 1.5, mu_y: 1.5, ..ref_car() };
    let tr = Track::circle(15.25, 360).unwrap();
    let res = lapsim::solve(&veh, RHO, &tr, &LapOptions::default()).unwrap();
    let v = (1.5f64 * G * 15.25).sqrt();
    close(v, 14.9776, 1e-4, "skidpad speed");
    assert!(rel(res.lap_time_s, 6.3975) < 1e-4, "{}", res.lap_time_s);
    assert!(rel(res.lap_time_s, 2.0 * std::f64::consts::PI * 15.25 / v) < 1e-6, "{}", res.lap_time_s);
    assert!(res.v.iter().all(|x| rel(*x, v) < 1e-6));
}

#[test]
fn golden_stadium_base_case() {
    let tr = stadium(1.0);
    close(tr.length(), 1051.33, 0.01, "length");
    let res = lapsim::solve(&ref_car(), RHO, &tr, &LapOptions::default()).unwrap();
    close(res.lap_time_s, 35.896, 0.05, "base lap");
    close(res.v_max() * 3.6, 163.56, 0.5, "v_max");
    close(res.v_min() * 3.6, 71.55, 0.5, "v_min");
    // speed trace: node i is s = i m from the start of straight 1
    for (s, kmh) in [(100.0, 120.35), (200.0, 144.23), (300.0, 160.77), (320.0, 163.56), (399.0, 73.39), (463.0, 71.55), (550.0, 89.20), (600.0, 111.81), (800.0, 156.93), (900.0, 109.60)] {
        let got = res.speed_at(s) * 3.6;
        assert!((got - kmh).abs() <= 0.5_f64.max(0.003 * kmh), "s={s}: {got} vs {kmh}");
    }
    assert_eq!(res.violations, 0);
    // events: two straights, two bends
    assert_eq!(res.segments.len(), 4);
    assert!(res.segments[1].v_min * 3.6 < 72.0);
    let total: f64 = res.segments.iter().map(|s| s.time_s).sum();
    close(total, res.lap_time_s, 1e-9, "segment times add up");
}

#[test]
fn golden_stadium_variants() {
    let tr = stadium(1.0);
    let base = ref_car();
    let cases: Vec<(&str, Vehicle, f64, f64)> = vec![
        ("rho 1.225", base.clone(), 1.225, 35.922),
        ("ClA 0", Vehicle { cla_m2: 0.0, ..base.clone() }, RHO, 35.992),
        ("ClA 1", Vehicle { cla_m2: 1.0, ..base.clone() }, RHO, 35.801),
        ("CdA 0.69", Vehicle { cda_m2: 0.69, ..base.clone() }, RHO, 35.923),
        ("CdA 0.59", Vehicle { cda_m2: 0.59, ..base.clone() }, RHO, 35.870),
        ("no aero", Vehicle { cda_m2: 0.0, cla_m2: 0.0, ..base.clone() }, RHO, 35.672),
        ("mu 1.2", Vehicle { mu_x: 1.2, mu_y: 1.2, ..base.clone() }, RHO, 33.959),
        ("mass 1500", Vehicle { mass_kg: 1500.0, ..base.clone() }, RHO, 36.213),
        ("rot inertia 0.05", Vehicle { rot_inertia_frac: 0.05, ..base.clone() }, RHO, 36.173),
        ("brake cap 0.8 g", Vehicle { brake_max_g: 0.8, ..base.clone() }, RHO, 36.252),
    ];
    for (name, veh, rho, want) in cases {
        let got = lap(&veh, rho, &tr);
        assert!((got - want).abs() <= 0.05_f64.max(0.0015 * want), "{name}: {got} vs {want}");
    }
}

#[test]
fn golden_stadium_discretisation() {
    for (ds, want) in [(0.5, 35.901), (2.0, 35.893), (5.0, 35.876)] {
        let tr = stadium(ds);
        let o = LapOptions { ds_max_m: 5.0, ..LapOptions::default() };
        let t = lapsim::lap_time(&ref_car(), RHO, &tr, &o).unwrap();
        assert!(rel(t, 35.896) < 0.001, "ds {ds}: {t}");
        assert!((t - want).abs() < 0.03, "ds {ds}: {t} vs {want}");
    }
    // a coarse track is subdivided to the largest step
    let coarse = Track::from_segments(&parse_segment_text("S400, L180deg@40, S400, L180deg@40").unwrap(), true, 20.0).unwrap();
    let t = lapsim::lap_time(&ref_car(), RHO, &coarse, &LapOptions { ds_max_m: 1.0, ..LapOptions::default() }).unwrap();
    assert!(rel(t, 35.896) < 0.002, "{t}");
}

#[test]
fn golden_density_and_power() {
    let tr = stadium(1.0);
    close(lap(&ref_car(), RHO, &tr), 35.896, 0.05, "fixed power at 1.11");
    let na = Vehicle { power_density_exponent: 1.0, rho_ref: 1.225, ..ref_car() };
    let t = lap(&na, RHO, &tr);
    close(t, 36.375, 0.06, "power scaled with density");
    let t_ref = lap(&na, 1.225, &tr);
    close(t_ref, 35.922, 0.05, "at the reference density");
    assert!(((t / t_ref - 1.0) * 100.0 - 1.3).abs() < 0.2, "{}", (t / t_ref - 1.0) * 100.0);
}

#[test]
fn golden_sensitivities_and_monotonic_sweeps() {
    let tr = stadium(1.0);
    let o = LapOptions::default();
    let veh = ref_car();
    let d_cda = lapsim::sensitivity(&veh, RHO, &tr, &o, SweepParam::CdA, 0.032).unwrap();
    let d_cla = lapsim::sensitivity(&veh, RHO, &tr, &o, SweepParam::ClA, 0.025).unwrap();
    close(d_cda, 0.53, 0.04, "dt/dCdA");
    close(d_cla, -0.19, 0.03, "dt/dClA");
    close(-d_cda / d_cla, 2.8, 0.4, "break-even ratio");
    // node-level sweep: monotonic for every directional parameter
    let track = Data::json(stadium(2.0).to_json(0.001, 3.0));
    for name in ["cda_m2", "cla_m2", "mass_kg", "mu", "power_w"] {
        let out = cook(
            "perf.lap_sweep",
            vec![Some(Data::json(veh.to_json())), Some(track.clone()), Some(Data::json(Env::from_rho(RHO).to_json()))],
            &[("param", ParamValue::Str(name.into())), ("n", ParamValue::Int(5))],
        )
        .unwrap();
        let rep = out[1].as_report().unwrap();
        assert_eq!(rep.metrics["monotonic"].value, 1.0, "{name}");
        let rows = out[0].as_json().unwrap()["rows"].as_array().unwrap().clone();
        assert_eq!(rows.len(), 5);
        if name == "cda_m2" {
            close(rep.metrics["dt_dcda_per_0p01"].value, 0.0053, 0.0006, "s per 0.01 m2");
            close(rep.metrics["breakeven_dcla_per_dcda"].value, 2.8, 0.5, "break-even");
        }
    }
}

#[test]
fn sweep_is_identical_at_any_thread_count() {
    let track = Data::json(stadium(2.0).to_json(0.001, 3.0));
    let run = |threads: usize| {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        pool.install(|| {
            cook(
                "perf.lap_sweep",
                vec![Some(Data::json(ref_car().to_json())), Some(track.clone()), Some(Data::json(Env::from_rho(RHO).to_json()))],
                &[("param", ParamValue::Str("cda_m2".into())), ("n", ParamValue::Int(9)), ("second_param", ParamValue::Str("mass_kg".into())), ("second_n", ParamValue::Int(3))],
            )
            .unwrap()[0]
                .as_json()
                .unwrap()
                .to_string()
        })
    };
    assert_eq!(run(1), run(8));
}

#[test]
fn solver_invariants() {
    let tr = stadium(1.0);
    let veh = ref_car();
    let res = lapsim::solve(&veh, RHO, &tr, &LapOptions::default()).unwrap();
    for i in 0..res.v.len() {
        assert!(res.v[i] <= res.v_lim[i] * (1.0 + 1e-9), "v above the limit at {i}");
    }
    assert_eq!(res.violations, 0, "grip envelope violations");
    // rotating the start node does not change the lap time
    for k in [1usize, 17, 400, 777] {
        let t = lapsim::lap_time(&veh, RHO, &tr.rotated(k), &LapOptions::default()).unwrap();
        assert!((t - res.lap_time_s).abs() < 1e-3, "rotated by {k}: {t} vs {}", res.lap_time_s);
    }
    // reversing a symmetric circuit gives the same time
    let t_rev = lapsim::lap_time(&veh, RHO, &tr.reversed(), &LapOptions::default()).unwrap();
    assert!((t_rev - res.lap_time_s).abs() < 0.01, "{t_rev}");
    // mirror too
    let t_mir = lapsim::lap_time(&veh, RHO, &tr.mirrored(), &LapOptions::default()).unwrap();
    assert!((t_mir - res.lap_time_s).abs() < 1e-9);
    // trapezoid and sum(ds/v) rules agree closely
    let o2 = LapOptions { time_rule: TimeRule::SumDsOverV, ..LapOptions::default() };
    let t2 = lapsim::lap_time(&veh, RHO, &tr, &o2).unwrap();
    assert!(rel(t2, res.lap_time_s) < 0.01, "{t2}");
    // holding the limit speed on arcs changes the time by well under 0.1 s
    let o3 = LapOptions { hold_limit_speed: true, ..LapOptions::default() };
    let t3 = lapsim::lap_time(&veh, RHO, &tr, &o3).unwrap();
    assert!((t3 - res.lap_time_s).abs() < 0.1 && t3 <= res.lap_time_s + 1e-9, "{t3}");
    // energy is positive and the wheel energy is below the source energy
    assert!(res.energy_wheel_j > 0.0 && res.energy_source_j > res.energy_wheel_j);
}

#[test]
fn geared_drivetrain_and_rpm() {
    let mut veh = ref_car();
    veh.engine_curve = parse_curve("1000:200, 3000:300, 5000:320, 7000:280").unwrap();
    veh.gear_ratios = vec![8.0, 5.0, 3.5, 2.5];
    veh.final_drive = 1.0;
    veh.wheel_radius_m = 0.33;
    veh.validate().unwrap();
    // rpm cut-off in first gear: 7000 rpm at 30.2 m/s
    let v_cut = 7000.0 * 2.0 * std::f64::consts::PI / 60.0 * 0.33 / 8.0;
    close(v_cut, 30.2, 0.05, "cut-off speed");
    close(veh.engine_rpm(8.0, v_cut), 7000.0, 1e-6, "rpm");
    let d = veh.drive(RHO, 20.0);
    assert!(d.force > 0.0 && d.gear >= 1 && d.rpm > 0.0);
    // above the first-gear cut-off a taller gear takes over
    let d2 = veh.drive(RHO, 40.0);
    assert!(d2.gear > 1 && d2.force > 0.0);
    let res = lapsim::solve(&veh, RHO, &stadium(2.0), &LapOptions::default()).unwrap();
    assert!(res.gear.iter().any(|g| *g > 0) && res.lap_time_s.is_finite());
    // a curve without gears (or gears without a curve) falls back to wheel power
    let half = Vehicle { gear_ratios: vec![], ..veh };
    assert!(!half.is_geared());
}

#[test]
fn friction_load_sensitivity_lowers_corner_speed() {
    let a = ref_car();
    let b = Vehicle { mu_drop_per_kn: 0.05, ..ref_car() };
    let (va, vb) = (a.corner_limit(RHO, 1.0 / 100.0), b.corner_limit(RHO, 1.0 / 100.0));
    assert!(vb < va + 1e-9);
    assert!(lap(&b, RHO, &stadium(2.0)) >= lap(&a, RHO, &stadium(2.0)) - 1e-9);
}

#[test]
fn vehicle_json_round_trip_and_validation() {
    let v = Vehicle { cda_offset_m2: 0.01, ..ref_car() };
    let j = v.to_json();
    assert_eq!(j["kind"], "vehicle.v1");
    assert_eq!(Vehicle::from_json(&j).unwrap(), v);
    assert!(Vehicle::from_json(&json!({"mass_kg": -1.0})).is_err());
    assert!(Vehicle::from_json(&json!({"mu_x": 0.0})).is_err());
    let d = Vehicle::from_json(&json!({})).unwrap();
    assert_eq!(d, Vehicle::default());
    // calibration offset adds to CdA and never rewrites it
    close(v.cda_eff(), 0.65, 1e-12, "cda_eff");
    assert!(lap(&v, RHO, &stadium(2.0)) > lap(&ref_car(), RHO, &stadium(2.0)));
}

#[test]
fn vehicle_node_takes_aero_from_the_airflow_stage() {
    let mut rep = Report::new("cfd");
    rep.metric("CdA", Metric::new(0.58, "m2")).metric("ClA", Metric::new(-0.7, "m2"));
    let out = cook("perf.vehicle", vec![Some(Data::report(rep.clone()))], &[]).unwrap();
    let v = Vehicle::from_json(out[0].as_json().unwrap()).unwrap();
    close(v.cda_m2, 0.58, 1e-12, "CdA");
    close(v.cla_m2, 0.7, 1e-12, "downforce flips sign");
    let out2 = cook("perf.vehicle", vec![Some(Data::report(rep))], &[("lift_positive", ParamValue::Bool(false))]).unwrap();
    close(Vehicle::from_json(out2[0].as_json().unwrap()).unwrap().cla_m2, -0.7, 1e-12, "no flip");
    let bad = cook("perf.vehicle", vec![None], &[("engine_curve", ParamValue::Str("not a curve".into()))]);
    assert!(bad.is_err());
}

// ------------------------------------------------------------------ tracks

#[test]
fn golden_track_from_path_polygon() {
    let n = 360;
    let r = 15.25;
    let pts: Vec<[f64; 2]> = (0..n)
        .map(|i| {
            let a = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
            [r * a.cos(), r * a.sin()]
        })
        .collect();
    let t = track_from_path(&pts, true, &PathOpts::default()).unwrap();
    assert!(t.closed);
    assert!(rel(t.length(), 95.82) < 0.001, "{}", t.length());
    for k in &t.kappa {
        assert!(rel(*k, 0.065574) < 0.001, "{k}");
    }
    // auto-detection works without the hint, with the first point repeated at the end
    let mut open = pts.clone();
    open.push(pts[0]);
    let t2 = track_from_path(&open, false, &PathOpts::default()).unwrap();
    assert!(t2.closed);
    // a straight line has no curvature and stays open
    let line: Vec<[f64; 2]> = (0..50).map(|i| [i as f64 * 3.0, 0.0]).collect();
    let t3 = track_from_path(&line, false, &PathOpts::default()).unwrap();
    assert!(!t3.closed);
    assert!(t3.kappa.iter().all(|k| k.abs() < 1e-12));
    close(t3.length(), 147.0, 0.01, "line length");
    // clockwise travel gives negative curvature
    let cw: Vec<[f64; 2]> = pts.iter().rev().copied().collect();
    let t4 = track_from_path(&cw, true, &PathOpts::default()).unwrap();
    assert!(t4.kappa.iter().all(|k| *k < 0.0));
    // forced modes
    let opts = PathOpts { closed: ClosedMode::Open, ..PathOpts::default() };
    assert!(!track_from_path(&pts, true, &opts).unwrap().closed);
}

/// Deterministic standard-normal noise (LCG + Box-Muller).
struct Noise(u64);

impl Noise {
    fn uniform(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64 + 1.0) / ((1u64 << 53) as f64 + 1.0)
    }

    fn normal(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }
}

#[test]
fn noisy_circle_curvature_is_recovered() {
    let mut rng = Noise(7);
    let r = 50.0;
    let n = 628;
    let pts: Vec<[f64; 2]> = (0..n)
        .map(|i| {
            let a = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
            [r * a.cos() + 0.3 * rng.normal(), r * a.sin() + 0.3 * rng.normal()]
        })
        .collect();
    let t = track_from_path(&pts, true, &PathOpts::default()).unwrap();
    let mean: f64 = t.kappa.iter().sum::<f64>() / t.kappa.len() as f64;
    assert!(rel(mean, 1.0 / r) < 0.10, "mean curvature {mean}");
}

#[test]
fn segments_and_edits() {
    let specs = parse_segment_text("S100, L90deg@50, R30@20").unwrap();
    assert_eq!(specs.len(), 3);
    close(specs[1].length_m, 50.0 * std::f64::consts::FRAC_PI_2, 1e-9, "arc length from angle");
    assert!(parse_segment_text("X10").is_err());
    assert!(parse_segment_text("L10").is_err());
    let json_specs = crate::track::parse_segments(&json!([[0, 100.0, 0.0], [1, 62.8, 40.0], {"type": "right", "angle_deg": 90.0, "radius_m": 25.0}])).unwrap();
    assert_eq!(json_specs.len(), 3);
    let t = Track::from_segments(&specs, false, 2.0).unwrap();
    assert!(!t.closed && t.nodes() == t.intervals() + 1);
    let segs = t.segment_list(0.001, 3.0);
    assert_eq!(segs.len(), 3);
    assert_eq!(segs[1].kind, crate::track::SegKind::Left);
    close(segs[1].radius_m, 50.0, 0.5, "corner radius recovered");
    // edits
    let rev = t.reversed();
    close(rev.length(), t.length(), 1e-9, "reverse keeps length");
    assert!(rev.kappa.first().unwrap().abs() > 0.0 || rev.kappa.last().unwrap().abs() == 0.0);
    let sc = t.scaled(2.0);
    close(sc.length(), 2.0 * t.length(), 1e-9, "scale length");
    close(sc.kappa.iter().fold(0.0f64, |m, k| m.max(k.abs())), 0.5 * t.kappa.iter().fold(0.0f64, |m, k| m.max(k.abs())), 1e-12, "scale curvature");
    // JSON round trip
    let back = Track::from_json(&t.to_json(0.001, 3.0)).unwrap();
    assert_eq!(back.ds.len(), t.ds.len());
    assert!(Track::new(true, vec![1.0, 1.0], vec![0.0, 0.0]).is_err());
    // the stadium closes into a loop
    let (d, h) = stadium(1.0).closure();
    assert!(d < 1.0 && h.abs() < 0.02, "closure {d} {h}");
    // node round trip
    let out = cook("perf.track_from_segments", vec![None], &[]).unwrap();
    let rep = out[1].as_report().unwrap();
    close(rep.metrics["length"].value, 1051.33, 0.01, "node track length");
    assert_eq!(rep.metrics["segments"].value, 4.0);
    let edited = cook("perf.track_edit", vec![Some(out[0].clone())], &[("reverse", ParamValue::Bool(true)), ("scale", f(2.0))]).unwrap();
    close(edited[1].as_report().unwrap().metrics["length"].value, 2102.66, 0.02, "edited length");
}

#[test]
fn lap_sim_node_end_to_end() {
    let env = Data::json(Env::from_rho(RHO).to_json());
    let veh = cook(
        "perf.vehicle",
        vec![None],
        &[("cla_m2", f(0.5)), ("cda_m2", f(0.64))],
    )
    .unwrap();
    let trk = cook("perf.track_from_segments", vec![None], &[]).unwrap();
    let out = cook("perf.lap_sim", vec![Some(veh[0].clone()), Some(trk[0].clone()), Some(env)], &[]).unwrap();
    let rep = out[2].as_report().unwrap();
    close(rep.metrics["lap_time"].value, 35.896, 0.05, "node lap time");
    assert!(rep.passes());
    assert_eq!(rep.metrics["envelope_violations"].value, 0.0);
    let lap_json = out[0].as_json().unwrap();
    assert_eq!(lap_json["kind"], "lap_result.v1");
    let n = lap_json["v_ms"].as_array().unwrap().len();
    assert_eq!(lap_json["t_s"].as_array().unwrap().len(), n);
    assert_eq!(lap_json["mode"].as_array().unwrap().len(), n);
    let geo = out[1].as_geometry().unwrap();
    assert_eq!(geo.point_count(), n);
    assert!(geo.point_attribs.contains("Cd") && geo.point_attribs.contains("v_kmh"));
    geo.validate().unwrap();
    // an open track with a standing start also works, with an end speed
    let straight = cook("perf.track_from_segments", vec![None], &[("segments_text", ParamValue::Str("S300".into())), ("closed", ParamValue::Bool(false))]).unwrap();
    let veh2 = Data::json(ref_car().to_json());
    let o = cook("perf.lap_sim", vec![Some(veh2), Some(straight[0].clone()), None], &[("end_speed_ms", f(20.0))]).unwrap();
    let t = o[2].as_report().unwrap().metrics["lap_time"].value;
    assert!(t.is_finite() && t > 5.0, "{t}");
}

#[test]
fn lap_delta_matches_the_individual_laps() {
    let a = ref_car();
    let b = Vehicle { cda_m2: 0.69, ..ref_car() };
    let trk = Data::json(stadium(1.0).to_json(0.001, 3.0));
    let out = cook(
        "perf.lap_delta",
        vec![Some(Data::json(a.to_json())), Some(Data::json(b.to_json())), Some(trk), Some(Data::json(Env::from_rho(RHO).to_json()))],
        &[],
    )
    .unwrap();
    let rep = out[1].as_report().unwrap();
    close(rep.metrics["delta_lap_time"].value, 35.923 - 35.896, 0.01, "delta");
    close(rep.metrics["delta_cda"].value, 0.05, 1e-12, "delta CdA");
    let rows = out[0].as_json().unwrap()["rows"].as_array().unwrap().clone();
    let total: f64 = rows.iter().map(|r| r[6].as_f64().unwrap()).sum();
    close(total, rep.metrics["delta_lap_time"].value, 1e-9, "segment deltas add up");
}

#[test]
fn gg_and_corner_nodes() {
    let veh = Data::json(Vehicle { crr: 0.0, ..ref_car() }.to_json());
    let env = Data::json(Env::from_rho(RHO).to_json());
    let out = cook("perf.gg_envelope", vec![Some(veh.clone()), Some(env.clone())], &[("n_speeds", ParamValue::Int(10)), ("v_max_ms", f(50.0)), ("ay_points", ParamValue::Int(11))]).unwrap();
    let rows = out[0].as_json().unwrap()["rows"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 110);
    // last speed block is 50 m/s; row at ay_frac 0.8 (index 8) has the ellipse limit of the spec
    let r = &rows[99 + 8];
    close(r[0].as_f64().unwrap(), 50.0, 1e-9, "speed");
    close(r[3].as_f64().unwrap(), 6.1813, 1e-3, "longitudinal limit at 0.8");
    // pure braking column carries drag
    let r0 = &rows[99];
    close(r0[5].as_f64().unwrap(), physics::braking_decel(1.0, RHO, 0.64, 0.5, 1400.0, 50.0), 1e-9, "braking with drag");
    let c = cook("perf.corner_limits", vec![Some(veh), Some(env)], &[("radii_m", ParamValue::Str("50, 200".into())), ("compare", ParamValue::Bool(true)), ("compare_cla_m2", f(1.0))]).unwrap();
    let rep = c[1].as_report().unwrap();
    close(rep.metrics["gain_R50"].value, 0.499, 0.002, "gain at 50 m");
    close(rep.metrics["gain_R200"].value, 2.043, 0.002, "gain at 200 m");
    let rows = c[0].as_json().unwrap()["rows"].as_array().unwrap().clone();
    close(rows[1][7].as_f64().unwrap(), 4.217, 0.002, "alt gain at 200 m");
}

// ------------------------------------------------------------------ coastdown

const A0: f64 = 0.012 * 1400.0 * G;
const B0: f64 = 0.5 * RHO * 0.64;

fn run_of(t: Vec<f64>, v: Vec<f64>, dir: i32, wind: f64) -> Run {
    Run { id: 0, t, v, wind_ms: wind, grade: 0.0, temp_c: None, p_pa: None, rh: 0.0, dir }
}

fn opts() -> CoastOpts {
    let mut o = CoastOpts::new(1400.0);
    o.rho_default = Some(RHO);
    o.min_runs = 2;
    o
}

#[test]
fn golden_coastdown_closed_form() {
    close(A0, 164.752, 0.001, "A");
    close(B0, 0.3552, 1e-9, "B");
    close((A0 / B0).sqrt(), 21.537, 0.001, "characteristic speed");
    let (v100, v60) = (100.0 / 3.6, 60.0 / 3.6);
    close(coast::coast_time(A0, B0, 1400.0, v100, v60), 46.243, 0.01, "100-60 time");
    close(coast::coast_distance(A0, B0, 1400.0, v100, v60), 1005.77, 0.05, "100-60 distance");
    close(coast::coast_time(A0, B0, 1400.0, 120.0 / 3.6, 40.0 / 3.6), 95.324, 0.01, "120-40 time");
    close(coast::coast_distance(A0, B0, 1400.0, 120.0 / 3.6, 40.0 / 3.6), 1944.04, 0.05, "120-40 distance");
    close(coast::coast_decel(A0, B0, 1400.0, v100), 0.3134, 1e-4, "decel at 100");
    close(coast::coast_time(A0, B0, 1400.0, 120.0 / 3.6, 20.0 / 3.6), 136.3, 0.05, "120-20 time");
    // the speed curve inverts the time
    let v = coast::coast_speed(A0, B0, 1400.0, v100, 46.243);
    close(v, v60, 1e-3, "speed after 46.243 s");
}

#[test]
fn coastdown_noise_free_log_recovers_the_truth() {
    let (t, v) = coast::simulate_log(A0, B0, 1400.0, 120.0 / 3.6, 20.0 / 3.6, 10.0);
    for method in [Method::Integrated, Method::Accel] {
        let mut o = opts();
        o.method = method;
        let fit = coast::fit_coastdown(&[run_of(t.clone(), v.clone(), 1, 0.0)], &o).unwrap();
        let (tol_crr, tol_cda) = if method == Method::Integrated { (1e-5, 1e-4) } else { (1e-4, 2e-3) };
        close(fit.crr, 0.012, tol_crr, "Crr");
        close(fit.cda, 0.64, tol_cda, "CdA");
    }
    // the speed window selects the part of the run that is used
    let mut o = opts();
    o.v_hi_ms = 100.0 / 3.6;
    o.v_lo_ms = 40.0 / 3.6;
    let fit = coast::fit_coastdown(&[run_of(t, v, 1, 0.0)], &o).unwrap();
    close(fit.cda, 0.64, 1e-3, "CdA in a narrower window");
}

fn noisy_run(rng: &mut Noise, sigma_kmh: f64, dir: i32, wind: f64) -> Run {
    // generate with wind: air speed u = v + wind
    let a = A0;
    let b = B0;
    let (t, u) = coast::simulate_log(a, b, 1400.0, 120.0 / 3.6 + wind, 20.0 / 3.6 + wind, 10.0);
    let v: Vec<f64> = u.iter().map(|x| x - wind + sigma_kmh / 3.6 * rng.normal()).collect();
    run_of(t, v, dir, 0.0)
}

fn noise_trials(sigma_kmh: f64, trials: usize, seed: u64) -> (Vec<f64>, Vec<f64>) {
    let mut rng = Noise(seed);
    let mut cdas = Vec::new();
    let mut crrs = Vec::new();
    for _ in 0..trials {
        let run = noisy_run(&mut rng, sigma_kmh, 1, 0.0);
        let mut o = opts();
        o.method = Method::Accel;
        o.hampel = false;
        let fit = coast::fit_coastdown(&[run], &o).unwrap();
        cdas.push(fit.cda);
        crrs.push(fit.crr);
    }
    (cdas, crrs)
}

#[test]
fn coastdown_scatter_scales_with_speed_noise_and_errors_anticorrelate() {
    // white speed noise at 10 Hz: every sample enters the regression, so the scatter is small;
    // real logs add correlated errors (wind, GPS) which the run-to-run scatter then captures
    let (c1, r1) = noise_trials(0.1, 60, 11);
    let (c5, _) = noise_trials(0.5, 60, 12);
    let (s1, s5) = (crate::util::sd(&c1), crate::util::sd(&c5));
    assert!(s1 > 0.0 && s1 < 0.03, "CdA SD at 0.1 km/h {s1}");
    assert!((3.0..8.0).contains(&(s5 / s1)), "scatter ratio {}", s5 / s1);
    assert!((crate::util::mean(&c1) - 0.64).abs() < 0.01);
    assert!((crate::util::mean(&r1) - 0.012).abs() < 0.0005);
    let (mc, mr) = (crate::util::mean(&c1), crate::util::mean(&r1));
    let (sc, sr) = (crate::util::sd(&c1), crate::util::sd(&r1));
    let cov: f64 = c1.iter().zip(&r1).map(|(a, b)| (a - mc) * (b - mr)).sum::<f64>() / 59.0;
    assert!(cov / (sc * sr) < -0.5, "correlation {}", cov / (sc * sr));
}

#[test]
fn coastdown_pairing_cancels_wind_and_mass_error_scales_cda() {
    let mut rng = Noise(3);
    let head = noisy_run(&mut rng, 0.0, 1, 2.0);
    let tail = noisy_run(&mut rng, 0.0, -1, -2.0);
    let mut o = opts();
    o.wind_max_ms = 0.0;
    let h = coast::fit_coastdown(std::slice::from_ref(&head), &o).unwrap();
    let t = coast::fit_coastdown(std::slice::from_ref(&tail), &o).unwrap();
    assert!(h.cda > 0.66 && t.cda < 0.62, "single runs are biased by wind: {} {}", h.cda, t.cda);
    let pair = coast::fit_coastdown(&[head, tail], &o).unwrap();
    assert_eq!(pair.n_pairs_used, 1);
    close(pair.cda, 0.64, 0.64 * 0.01, "paired CdA");
    close(pair.crr, 0.012, 0.0004, "paired Crr");
    // mass 3 percent low gives CdA 3 percent low
    let (tt, vv) = coast::simulate_log(A0, B0, 1400.0, 120.0 / 3.6, 20.0 / 3.6, 10.0);
    let mut lo = opts();
    lo.mass_kg = 1400.0 * 0.97;
    let fit = coast::fit_coastdown(&[run_of(tt, vv, 1, 0.0)], &lo).unwrap();
    assert!((fit.cda / 0.64 - 0.97).abs() < 0.003, "{}", fit.cda);
}

#[test]
fn coastdown_wind_column_is_used_and_rejection_works() {
    let (t, u) = coast::simulate_log(A0, B0, 1400.0, 120.0 / 3.6 + 2.0, 20.0 / 3.6 + 2.0, 10.0);
    let v: Vec<f64> = u.iter().map(|x| x - 2.0).collect();
    let mut run = run_of(t, v, 1, 2.0);
    let mut o = opts();
    o.wind_max_ms = 2.7;
    let fit = coast::fit_coastdown(std::slice::from_ref(&run), &o).unwrap();
    close(fit.cda, 0.64, 0.64 * 0.002, "CdA with the wind component known");
    run.wind_ms = 5.0;
    assert!(coast::fit_coastdown(std::slice::from_ref(&run), &o).is_err(), "windy run rejected");
}

#[test]
fn coastdown_fixed_crr_bias_is_reported() {
    // Crr held 25 percent too high biases CdA low (about -20 percent over 120 to 20 km/h)
    let (t, v) = coast::simulate_log(A0, B0, 1400.0, 120.0 / 3.6, 20.0 / 3.6, 10.0);
    let mut o = opts();
    o.crr_fixed = 0.015;
    o.method = Method::Accel;
    let fit = coast::fit_coastdown(&[run_of(t.clone(), v.clone(), 1, 0.0)], &o).unwrap();
    let bias = fit.cda / 0.64 - 1.0;
    assert!((-0.40..-0.15).contains(&bias), "fixed-Crr bias {bias}");
    // through the node, with a CSV file and the sensitivity note
    let dir = std::env::temp_dir().join(format!("vernon-perf-coast-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("log.csv");
    let mut csv = String::from("t,v_kmh,dir,temp_c,p_pa\n");
    for (a, b) in t.iter().zip(&v) {
        csv.push_str(&format!("{a},{},1,40,101325\n", b * 3.6));
    }
    std::fs::write(&path, csv).unwrap();
    let out = cook("perf.coastdown_fit", vec![Some(Data::File(path))], &[("crr_fixed", f(0.012)), ("min_runs", ParamValue::Int(1))]).unwrap();
    let rep = out[1].as_report().unwrap();
    assert!(rep.metrics.contains_key("cda_shift_if_crr_plus25_pct"));
    let shift = rep.metrics["cda_shift_if_crr_plus25_pct"].value;
    assert!(shift < -15.0 && shift > -45.0, "{shift}");
    close(rep.metrics["CdA"].value, 0.64, 0.01, "node CdA");
}

#[test]
fn coastdown_node_pairs_runs_and_reports_detectability() {
    let mut rng = Noise(5);
    let mut runs = Vec::new();
    for k in 0..10 {
        let run = noisy_run(&mut rng, 0.2, if k % 2 == 0 { 1 } else { -1 }, 0.0);
        runs.push(json!({"t": run.t, "v_ms": run.v, "dir": run.dir, "temp_c": 40.0, "p_pa": 101325.0}));
    }
    let out = cook("perf.coastdown_fit", vec![Some(Data::json(json!({"runs": runs})))], &[("mass_kg", f(1400.0)), ("method", ParamValue::Str("integrated".into()))]).unwrap();
    let rep = out[1].as_report().unwrap();
    close(rep.metrics["CdA"].value, 0.64, 0.03, "CdA from ten noisy runs");
    assert_eq!(rep.metrics["valid_runs"].value, 10.0);
    assert!(rep.metrics["valid_runs"].passes().unwrap());
    let md = rep.metrics["min_detectable_dcda"].value;
    assert!(md > 0.0 && md < 0.1, "{md}");
    let fit = out[0].as_json().unwrap();
    assert_eq!(fit["kind"], "coast_fit.v1");
    assert!(fit["corr_cda_crr"].as_f64().unwrap() < -0.5);
    // too few runs fails the quality metric
    let few = cook("perf.coastdown_fit", vec![Some(Data::json(coast_log_json(2)))], &[]).unwrap();
    assert!(!few[1].as_report().unwrap().passes());
}

#[test]
fn hampel_removes_a_speed_spike() {
    let (t, mut v) = coast::simulate_log(A0, B0, 1400.0, 120.0 / 3.6, 20.0 / 3.6, 10.0);
    v[400] += 8.0;
    let keep = coast::hampel_keep(&t, &v, 3.0, 4.4478);
    assert!(!keep[400]);
    assert_eq!(keep.iter().filter(|k| !**k).count(), 1);
    let mut o = opts();
    o.hampel = true;
    let fit = coast::fit_coastdown(&[run_of(t, v, 1, 0.0)], &o).unwrap();
    close(fit.cda, 0.64, 0.01, "CdA with a spike removed");
}

// ------------------------------------------------------------------ pressure and Cp

#[test]
fn golden_cp_conversion() {
    let taps = vec![
        TapIn { id: "before".into(), pos: DVec3::ZERO, dp_pa: -91.0, speed_ms: Some(100.0 / 3.6), rho: Some(RHO) },
        TapIn { id: "after".into(), pos: DVec3::ZERO, dp_pa: -103.0, speed_ms: Some(100.0 / 3.6), rho: Some(RHO) },
    ];
    let o = CpOpts { rho_default: None, speed_default: None, speed_sd_ms: 0.3, dp_sd_pa: 3.0, rho_sd_frac: 0.01 };
    let rows = cp::compute_cp(&taps, &o).unwrap();
    close(rows[0].q_pa, 428.24, 0.01, "q");
    close(rows[0].cp, -0.2125, 1e-4, "Cp before");
    close(rows[1].cp, -0.2405, 1e-4, "Cp after");
    close(rows[1].cp - rows[0].cp, -0.0280, 1e-4, "delta");
    close(1.0 / rows[0].q_pa, 0.00234, 1e-5, "1 Pa in Cp");
    // a 1 km/h speed error scales Cp by about 2 percent
    let fast = TapIn { speed_ms: Some(101.0 / 3.6), ..taps[0].clone() };
    let cp_fast = cp::compute_cp(&[fast], &o).unwrap()[0].cp;
    close((cp_fast / rows[0].cp - 1.0).abs(), 0.0199, 0.001, "speed sensitivity");
    // the wrong density rescales Cp by 9.4 percent
    let wrong = TapIn { rho: Some(1.225), ..taps[0].clone() };
    let cp_wrong = cp::compute_cp(&[wrong], &o).unwrap()[0].cp;
    close(100.0 * (1.0 - cp_wrong / rows[0].cp), 9.39, 0.05, "density sensitivity");
    // uncertainty grows with speed error
    assert!(rows[0].cp_sd > 3.0 / 428.24);
    // missing speed or density is a clear error
    let bare = TapIn { speed_ms: None, ..taps[0].clone() };
    assert!(cp::compute_cp(&[bare], &o).unwrap_err().contains("speed"));
    // inches of water convert
    let csv = "id,x,y,z,dp_inwc,speed_kmh,rho\nA,0,0,0,1.0,100,1.11\n";
    let t = cp::parse_taps_csv(csv).unwrap();
    close(t[0].dp_pa, 249.09, 1e-9, "in. w.c.");
    close(t[0].speed_ms.unwrap(), 100.0 / 3.6, 1e-12, "km/h");
}

#[test]
fn pressure_cp_node_checks_the_gauge_range() {
    let out = cook(
        "perf.pressure_cp",
        vec![Some(Data::json(taps_json())), None],
        &[("gauge_range_pa", f(100.0))],
    )
    .unwrap();
    let rep = out[1].as_report().unwrap();
    close(rep.metrics["max_abs_dp"].value, 103.0, 1e-9, "max dp");
    assert!(!rep.passes(), "gauge headroom below 1.5 fails the metric");
    let ok = cook("perf.pressure_cp", vec![Some(Data::json(taps_json())), None], &[("gauge_range_pa", f(249.0))]).unwrap();
    assert!(ok[1].as_report().unwrap().passes());
    // no density anywhere is an error, not a silent default
    let no_rho = json!([{"id": "a", "dp_pa": -50.0, "speed_ms": 25.0}]);
    assert!(cook("perf.pressure_cp", vec![Some(Data::json(no_rho)), None], &[]).is_err());
}

fn plate(cp_value: f64) -> Geometry {
    let pts = vec![DVec3::new(-500.0, -500.0, 0.0), DVec3::new(500.0, -500.0, 0.0), DVec3::new(500.0, 500.0, 0.0), DVec3::new(-500.0, 500.0, 0.0)];
    let mut g = Geometry::from_polygons(pts, vec![vec![0, 1, 2, 3]]);
    g.prim_attribs.set("Cp", AttribArray::Float(vec![cp_value]));
    g
}

/// A plate whose Cp varies linearly with x: Cp = -0.3 + 0.0004 x (x in mm).
fn ramp_plate() -> Geometry {
    let n = 21;
    let mut pts = Vec::new();
    let mut faces = Vec::new();
    for j in 0..n {
        for i in 0..n {
            pts.push(DVec3::new(-500.0 + 50.0 * i as f64, -500.0 + 50.0 * j as f64, 0.0));
        }
    }
    for j in 0..n - 1 {
        for i in 0..n - 1 {
            let a = (j * n + i) as u32;
            faces.push(vec![a, a + 1, a + n as u32 + 1, a + n as u32]);
        }
    }
    let mut g = Geometry::from_polygons(pts.clone(), faces);
    g.point_attribs.set("Cp", AttribArray::Float(pts.iter().map(|p| -0.3 + 0.0004 * p.x).collect()));
    g
}

fn cp_rows(values: &[(f64, f64, f64)]) -> Vec<cp::CpRow> {
    values
        .iter()
        .enumerate()
        .map(|(i, &(x, y, c))| cp::CpRow { id: format!("t{i}"), pos: DVec3::new(x, y, 3.0), dp_pa: 0.0, speed_ms: 27.8, rho: RHO, q_pa: 428.0, cp: c, cp_sd: 0.01 })
        .collect()
}

#[test]
fn golden_cp_compare() {
    let surf = ramp_plate();
    let xs = [-400.0, -300.0, -200.0, -100.0, 0.0, 100.0, 200.0, 300.0, 400.0, 450.0];
    // ten taps whose measured Cp equals the simulation plus a small error
    let errs = [0.02, -0.02, 0.03, -0.03, 0.01, -0.01, 0.0, 0.02, -0.02, 0.03];
    let truth = |x: f64| -0.3 + 0.0004 * x;
    let good: Vec<(f64, f64, f64)> = xs.iter().zip(errs).map(|(&x, e)| (x, 17.0, truth(x) + e)).collect();
    let res = cp::compare(&cp_rows(&good), &surf, 20.0, 0.05, 2.0).unwrap();
    assert_eq!(res.failed, 0);
    assert!(res.rms < 0.03, "{}", res.rms);
    assert_eq!(res.sign_agree, 10);
    // the simulated value at each tap is the field value (linear field, symmetric disc)
    for (r, &(x, _, _)) in res.rows.iter().zip(&good) {
        close(r.cp_sim, truth(x), 0.01, "sampled surface Cp");
        close(r.offset_mm, 3.0, 1e-9, "offset");
    }
    // one tap with the wrong sign: the sign count drops and the node warns whatever the RMS
    let mut bad = good.clone();
    bad[8].2 = -(truth(400.0)); // simulated -0.14 -> measured +0.14
    let res2 = cp::compare(&cp_rows(&bad), &surf, 20.0, 0.05, 2.0).unwrap();
    assert_eq!(res2.sign_agree, 9);
    assert_eq!(res2.failed, 1);
    let table = cp::cp_table_json(&cp_rows(&bad));
    let out = cook("perf.cp_compare", vec![Some(Data::json(table)), Some(Data::geometry(surf.clone()))], &[]).unwrap();
    let rep = out[1].as_report().unwrap();
    assert_eq!(rep.metrics["sign_disagree"].value, 1.0);
    assert!(!rep.passes());
    assert!(rep.notes.iter().any(|(_, t)| t.contains("sign agreement 9/10")));
    // a 0.10 error fails the 0.05 tolerance
    let off: Vec<(f64, f64, f64)> = good.iter().map(|&(x, y, c)| (x, y, c + 0.10)).collect();
    assert_eq!(cp::compare(&cp_rows(&off), &surf, 20.0, 0.05, 2.0).unwrap().failed, 10);
    // a face-attribute surface works too, and a surface without Cp is a clear error
    let flat = plate(-0.25);
    let r3 = cp::compare(&cp_rows(&[(0.0, 0.0, -0.24)]), &flat, 20.0, 0.05, 2.0).unwrap();
    close(r3.rows[0].cp_sim, -0.25, 1e-12, "face field");
    assert!(cp::compare(&cp_rows(&[(0.0, 0.0, 0.0)]), &Geometry::from_polygons(flat.points.clone(), vec![vec![0, 1, 2, 3]]), 20.0, 0.05, 2.0).is_err());
}

#[test]
fn closest_point_on_triangle_cases() {
    let (a, b, c) = (DVec3::ZERO, DVec3::new(10.0, 0.0, 0.0), DVec3::new(0.0, 10.0, 0.0));
    let (q, w) = cp::closest_on_triangle(DVec3::new(2.0, 2.0, 5.0), a, b, c);
    close(q.distance(DVec3::new(2.0, 2.0, 0.0)), 0.0, 1e-12, "interior");
    close(w.iter().sum::<f64>(), 1.0, 1e-12, "barycentric sum");
    let (q2, _) = cp::closest_on_triangle(DVec3::new(-3.0, -3.0, 1.0), a, b, c);
    close(q2.distance(a), 0.0, 1e-12, "vertex");
    let (q3, _) = cp::closest_on_triangle(DVec3::new(8.0, 8.0, 0.0), a, b, c);
    close(q3.distance(DVec3::new(5.0, 5.0, 0.0)), 0.0, 1e-12, "hypotenuse");
}

// ------------------------------------------------------------------ tufts

#[test]
fn golden_tuft_classes() {
    let th = TuftThresholds { attached_deg: 30.0, reversed_deg: 120.0, sd_attached_deg: 15.0, sd_separated_deg: 30.0 };
    let series = |centre: f64| -> Vec<f64> { (0..20).map(|i| centre + 5.0 * if i % 2 == 0 { 1.0 } else { -1.0 }).collect() };
    let want = [(0.0, TuftClass::Attached), (20.0, TuftClass::Attached), (90.0, TuftClass::Separated), (180.0, TuftClass::Reversed)];
    for (centre, class) in want {
        let (got, off, sd) = cp::classify_tuft(&series(centre), 0.0, &th);
        assert_eq!(got, class, "tuft at {centre}: offset {off}, sd {sd}");
    }
    // a fluttering tuft is separated even when centred on the flow
    let wild: Vec<f64> = (0..20).map(|i| if i % 2 == 0 { 70.0 } else { -70.0 }).collect();
    assert_eq!(cp::classify_tuft(&wild, 0.0, &th).0, TuftClass::Separated);
    // angles wrap: 350 and 10 degrees are 20 apart
    close(cp::angle_diff_deg(350.0, 10.0), -20.0, 1e-12, "wrap");
    let (m, sd) = cp::circular_stats_deg(&[350.0, 10.0]);
    assert!(m.abs() < 1e-9 && sd < 15.0);
    // the node: supplied series are classified for real, no placeholder warning
    let tufts = json!({"tufts": [
        {"id": "a", "x": 0.0, "y": 0.0, "z": 0.0, "angles_deg": series(0.0)},
        {"id": "b", "x": 50.0, "y": 0.0, "z": 0.0, "angles_deg": series(180.0)}
    ]});
    let out = cook("perf.tuft_analysis", vec![None, None, Some(Data::json(tufts))], &[]).unwrap();
    let rep = out[2].as_report().unwrap();
    close(rep.metrics["attached_fraction"].value, 0.5, 1e-12, "attached fraction");
    close(rep.metrics["reversed_fraction"].value, 0.5, 1e-12, "reversed fraction");
    assert!(!rep.notes.iter().any(|(_, t)| t.contains("white-box placeholder")));
    assert_eq!(out[1].as_geometry().unwrap().point_count(), 2);
    // without series the node says it is a placeholder
    let ph = cook("perf.tuft_analysis", vec![None, None, None], &[]).unwrap();
    assert!(ph[2].as_report().unwrap().notes.iter().any(|(_, t)| t.starts_with("white-box placeholder")));
}

// ------------------------------------------------------------------ gate

#[test]
fn golden_gate_rules() {
    let base = GateInput { cda_tol_m2: 0.05, cp_rms_limit: 0.1, ..GateInput::default() };
    // claimed 0.01 m2 against a 0.084 m2 noise floor warns, and still prints the lap value
    let small = GateInput { claimed_dcda_m2: 0.01, min_detectable_dcda: Some(2.8 * 0.03), lap_value_s: Some(0.0053), ..base.clone() };
    let r = gate::evaluate(&small);
    assert!(r.notes.iter().any(|(_, t)| t.starts_with("below noise floor")));
    assert_eq!(r.metrics["status"].value, 1.0);
    close(r.metrics["lap_value"].value, 0.0053, 1e-12, "lap value");
    // claimed 0.10 passes the detectability rule
    let big = GateInput { claimed_dcda_m2: 0.10, min_detectable_dcda: Some(2.8 * 0.03), lap_value_s: Some(0.053), ..base.clone() };
    let r2 = gate::evaluate(&big);
    assert_eq!(r2.metrics["status"].value, 0.0);
    // CFD CdA outside the coastdown interval: a warning, or a failure when asked to fail
    let miss = GateInput { cfd_cda: Some(0.80), coast_cda: Some((0.64, 0.62, 0.66)), lap_value_s: Some(0.0), ..base.clone() };
    let w = gate::evaluate(&miss);
    assert_eq!(w.metrics["status"].value, 1.0);
    assert!(w.passes());
    let hard = gate::evaluate(&GateInput { fail_on_miss: true, ..miss.clone() });
    assert_eq!(hard.metrics["status"].value, 2.0);
    assert!(!hard.passes());
    close(hard.metrics["cfd_outside_coast_ci"].value, 0.14, 1e-12, "excess");
    // inside the interval or within tolerance is fine
    let near = GateInput { cfd_cda: Some(0.69), coast_cda: Some((0.64, 0.62, 0.66)), lap_value_s: Some(0.0), fail_on_miss: true, ..base.clone() };
    assert_eq!(gate::evaluate(&near).metrics["status"].value, 0.0);
    // Cp: RMS above the limit warns; a sign disagreement warns even with a tiny RMS
    let cp_bad = GateInput { cp_rms: Some(0.12), cp_sign_disagree: Some(0.0), lap_value_s: Some(0.0), ..base.clone() };
    assert_eq!(gate::evaluate(&cp_bad).metrics["status"].value, 1.0);
    let cp_sign = GateInput { cp_rms: Some(0.02), cp_sign_disagree: Some(1.0), lap_value_s: Some(0.0), ..base.clone() };
    assert_eq!(gate::evaluate(&cp_sign).metrics["status"].value, 1.0);
    // no lap value available is itself a warning
    assert_eq!(gate::evaluate(&base).metrics["status"].value, 1.0);
}

#[test]
fn golden_gate_round_trip() {
    // tests 4, 10 and 13 through the node: claimed dCdA +0.05 against a 0.02 m2 noise floor
    let veh = Vehicle { rr_includes_downforce: false, ..ref_car() };
    let fit = json!({"cda_m2": 0.64, "cda_ci95": [0.60, 0.68], "min_detectable_dcda_m2": 0.02});
    let out = cook(
        "perf.validation_gate",
        vec![None, None, Some(Data::json(fit)), None, Some(Data::json(veh.to_json())), Some(Data::json(stadium(1.0).to_json(0.001, 3.0))), Some(Data::json(Env::from_rho(RHO).to_json()))],
        &[("claimed_dcda_m2", f(0.05))],
    )
    .unwrap();
    let rep = out[0].as_report().unwrap();
    close(rep.metrics["lap_value"].value, 0.027, 0.012, "lap value of +0.05 m2");
    close(rep.metrics["top_speed_value"].value, -6.32, 0.05, "top-speed value");
    assert!(rep.metrics["detectability_ratio"].value > 1.0);
    assert!(!rep.notes.iter().any(|(_, t)| t.starts_with("below noise floor")));
    assert_eq!(rep.metrics["status"].value, 0.0);
    // claimed 0.01 m2 gives the noise-floor warning
    let small = cook(
        "perf.validation_gate",
        vec![None, None, Some(Data::json(json!({"cda_m2": 0.64, "cda_ci95": [0.60, 0.68], "min_detectable_dcda_m2": 0.084}))), None, Some(Data::json(veh.to_json())), Some(Data::json(stadium(1.0).to_json(0.001, 3.0))), Some(Data::json(Env::from_rho(RHO).to_json()))],
        &[("claimed_dcda_m2", f(0.01))],
    )
    .unwrap();
    let r = small[0].as_report().unwrap();
    assert!(r.notes.iter().any(|(_, t)| t.starts_with("below noise floor")));
    close(r.metrics["lap_value"].value, 0.0053, 0.003, "lap value of +0.01 m2");
    // linear fallback from a sweep report when no vehicle is connected
    let mut sweep = Report::new("sweep");
    sweep.metric("dt_dcda", Metric::new(0.53, "s/m2")).metric("dt_dcla", Metric::new(-0.19, "s/m2"));
    let lin = cook("perf.validation_gate", vec![Some(Data::report(sweep)), None, None, None, None, None, None], &[("claimed_dcda_m2", f(0.05))]).unwrap();
    close(lin[0].as_report().unwrap().metrics["lap_value"].value, 0.0265, 1e-9, "linear lap value");
}

// ------------------------------------------------------------------ misc plumbing

#[test]
fn env_json_and_air_density_node() {
    let out = cook("perf.air_density", vec![], &[("temp_c", f(45.0)), ("rel_humidity", f(0.30)), ("dry_gas_const", f(287.058))]).unwrap();
    let env = Env::from_json(out[0].as_json().unwrap()).unwrap();
    assert!(rel(env.rho, 1.09757) < 1e-4, "{}", env.rho);
    close(out[1].as_report().unwrap().metrics["rho_vs_isa_pct"].value, -10.4, 0.1, "vs ISA");
    assert!(Env::from_json(&json!({"rho": -1.0})).is_err());
    assert!(Env::from_json(&json!({})).is_err());
    let from_state = Env::from_json(&json!({"T_c": 40.0, "p_pa": 101325.0})).unwrap();
    assert!(rel(from_state.rho, 1.12718) < 1e-4);
}

#[test]
fn bad_inputs_are_clean_errors() {
    // wrong kinds and missing required ports
    assert!(cook("perf.lap_sim", vec![None, None, None], &[]).is_err());
    assert!(cook("perf.lap_sim", vec![Some(Data::json(json!({"mass_kg": -5.0}))), Some(Data::json(stadium(1.0).to_json(0.001, 3.0))), None], &[]).is_err());
    assert!(cook("perf.lap_sweep", vec![Some(Data::json(ref_car().to_json())), Some(Data::json(json!({"ds": [1.0]}))), None], &[]).is_err());
    assert!(cook("perf.aero_loads", vec![None, None], &[("speeds_kmh", ParamValue::Str("fast".into()))]).is_err());
    assert!(cook("perf.track_from_segments", vec![None], &[("segments_text", ParamValue::Str("S-5".into()))]).is_err());
    assert!(cook("perf.coastdown_fit", vec![Some(Data::json(json!({"t": [0.0, 1.0]})))], &[]).is_err());
    assert!(cook("perf.pressure_cp", vec![Some(Data::json(json!([])))], &[]).is_err());
    assert!(cook("perf.top_speed", vec![Some(Data::json(json!({"rho": 0.0}))), None], &[]).is_err());
    // malformed geometry is an error, not a panic
    let mut broken = sphere_with_cp();
    broken.prims[0].vertices[0] = 99;
    let table = Data::json(cp::cp_table_json(&cp_rows(&[(10.0, 10.0, -0.2)])));
    assert!(cook("perf.cp_compare", vec![Some(table), Some(Data::geometry(broken.clone()))], &[]).is_err());
    assert!(cook("perf.track_from_path", vec![Some(Data::geometry(broken)), None], &[]).is_err());
    // the sign of the helper used by the sweep range
    assert!(SweepParam::parse("nonsense").is_none());
    assert_eq!(SweepParam::parse("mu").unwrap().name(), "mu");
}


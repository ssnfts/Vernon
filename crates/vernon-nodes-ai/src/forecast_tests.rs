use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use vernon_graph::{CookContext, Data, NodeOp, ParamValue, Params};

use super::*;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vernon_fc_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ctx(dir: &Path) -> CookContext {
    CookContext { node_name: "t".into(), work_dir: dir.to_path_buf(), frame: 0.0 }
}

fn params() -> Params {
    Params::from_specs(&ForecastNode.info().params)
}

fn ramp(n: usize) -> Vec<f64> {
    (0..n).map(|i| 10f64.powf(-1.0 - 0.01 * i as f64)).collect()
}

fn cook(dir: &Path, ins: &[Option<Data>], p: &Params) -> Result<Vec<Data>, String> {
    ForecastNode.cook(&ctx(dir), ins, p).map_err(|e| e.to_string())
}

fn json_in(v: Value) -> Vec<Option<Data>> {
    vec![Some(Data::json(v))]
}

const LOG: &str = "\
Create time

Time = 1

smoothSolver:  Solving for Ux, Initial residual = 0.5, Final residual = 0.01, No Iterations 3
smoothSolver:  Solving for Uy, Initial residual = 0.4, Final residual = 0.01, No Iterations 3
GAMG:  Solving for p, Initial residual = 0.9, Final residual = 0.05, No Iterations 7
time step continuity errors : sum local = 1e-3, global = 1e-5, cumulative = 1e-5
GAMG:  Solving for p, Initial residual = 0.3, Final residual = 0.002, No Iterations 4
smoothSolver:  Solving for k, Initial residual = 0.2, Final residual = 0.01, No Iterations 2
ExecutionTime = 1.2 s  ClockTime = 2 s

Time = 2

smoothSolver:  Solving for Ux, Initial residual = 0.25, Final residual = 0.01, No Iterations 3
smoothSolver:  Solving for Uy, Initial residual = 0.2, Final residual = 0.01, No Iterations 3
GAMG:  Solving for p, Initial residual = 0.5, Final residual = 0.05, No Iterations 7
GAMG:  Solving for p, Initial residual = 0.1, Final residual = 0.002, No Iterations 4
smoothSolver:  Solving for k, Initial residual = 1e-1, Final residual = 0.01, No Iterations 2
ExecutionTime = 2.4 s  ClockTime = 4 s

Time = 3

smoothSolver:  Solving for Ux, Initial residual = 0.125, Final residual = 0.01, No Iterations 3
GAMG:  Solving for p, Initial residual = 0.25, Final residual = 0.05, No Iterations 7
";

#[test]
fn json_input_shapes() {
    let a = series_from_json(&json!({"series": {"Ux": [1.0, 2.0], "p": [3.0]}})).unwrap();
    assert_eq!(a.len(), 2);
    assert_eq!(a[0], Series { name: "Ux".into(), values: vec![1.0, 2.0] });
    let b = series_from_json(&json!({"Ux": [1, 2, 3]})).unwrap();
    assert_eq!(b[0].values, vec![1.0, 2.0, 3.0]);
    // a series literally called "series" is a plain object entry
    let c = series_from_json(&json!({"series": [1.0, 2.0]})).unwrap();
    assert_eq!(c[0].name, "series");
    assert!(series_from_json(&json!([1, 2])).is_err());
    assert!(series_from_json(&json!({"Ux": "no"})).unwrap_err().contains("Ux"));
    assert!(series_from_json(&json!({"Ux": [1, "x"]})).unwrap_err().contains("not a number"));
}

#[test]
fn log_parser_takes_first_occurrence_per_time_block() {
    let s = parse_openfoam_log(LOG);
    let get = |n: &str| s.iter().find(|x| x.name == n).unwrap().values.clone();
    assert_eq!(s.len(), 4);
    assert_eq!(get("Ux"), vec![0.5, 0.25, 0.125]);
    assert_eq!(get("Uy"), vec![0.4, 0.2]);
    assert_eq!(get("p"), vec![0.9, 0.5, 0.25]);
    assert_eq!(get("k"), vec![0.2, 0.1]);
    assert!(parse_openfoam_log("nothing here").is_empty());
}

#[test]
fn refusals_have_their_own_messages() {
    assert!(validate(&[], true).unwrap_err().contains("no series"));
    let many: Vec<Series> = (0..17).map(|i| Series { name: format!("s{i}"), values: ramp(40) }).collect();
    assert!(validate(&many, true).unwrap_err().contains("at most 16"));
    let short = [Series { name: "a".into(), values: ramp(31) }];
    assert!(validate(&short, true).unwrap_err().contains("at least 32"));
    let mut v = ramp(40);
    v[5] = f64::NAN;
    assert!(validate(&[Series { name: "a".into(), values: v }], true).unwrap_err().contains("non-finite"));
    let mut v = ramp(40);
    v[7] = 0.0;
    let s = [Series { name: "a".into(), values: v }];
    assert!(validate(&s, true).unwrap_err().contains("strictly positive"));
    assert!(validate(&s, false).is_ok());
    assert!(validate(&[Series { name: "a".into(), values: ramp(32) }], true).is_ok());
}

#[test]
fn refusals_reach_the_node() {
    let dir = tmp("refuse");
    let p = params();
    assert!(cook(&dir, &[], &p).unwrap_err().contains("no series"));
    assert!(cook(&dir, &json_in(json!({"a": [1.0, 2.0]})), &p).unwrap_err().contains("at least 32"));
    let mut bad = params();
    bad.set("log_path", ParamValue::Str(dir.join("missing.log").to_string_lossy().into()));
    assert!(cook(&dir, &[], &bad).unwrap_err().contains("cannot read solver log"));
    let empty = dir.join("empty.log");
    std::fs::write(&empty, "hello").unwrap();
    bad.set("log_path", ParamValue::Str(empty.to_string_lossy().into()));
    assert!(cook(&dir, &[], &bad).unwrap_err().contains("no 'Solving for"));
}

#[test]
fn log_transform_round_trip() {
    for v in [1e-9, 3.7e-4, 1.0, 250.0] {
        let back = from_model(to_model(v, true), true);
        assert!((back - v).abs() / v < 1e-12);
        assert_eq!(from_model(to_model(v, false), false), v);
    }
    assert!((to_model(1e-5, true) + 5.0).abs() < 1e-12);
}

#[test]
fn crossing_detection() {
    let med = [1e-3, 1e-4, 1e-5, 1e-6];
    assert_eq!(crossing(1e-2, &med, 5e-5), Crossing::Step(3));
    assert_eq!(crossing(1e-2, &med, 1e-9), Crossing::NotWithinHorizon);
    assert_eq!(crossing(1e-7, &med, 1e-5), Crossing::AlreadyBelow);
    assert_eq!(crossing(1e-2, &med, 1e-5), Crossing::Step(4));
}

fn fake_result(horizon: usize, names: &[&str]) -> Value {
    let col = |f: f64| -> Vec<f64> { (0..horizon).map(|i| -2.0 - 0.1 * i as f64 + f).collect() };
    let rows: Vec<Value> = names.iter().map(|n| json!({"name": n, "context_length": 40, "q10": col(-0.2), "q50": col(0.0), "q90": col(0.2)})).collect();
    json!({"model": "amazon/chronos-2", "revision": "abc123", "device": "cuda", "horizon": horizon, "series": rows,
        "peak_vram_GB": 0.9, "peak_rss_GB": 2.5, "seconds_load": 3.0, "seconds_infer": 0.4})
}

fn series_of(names: &[&str]) -> Vec<Series> {
    names.iter().map(|n| Series { name: n.to_string(), values: ramp(40) }).collect()
}

#[test]
fn result_parsing_from_fake_result_folder() {
    let dir = tmp("parse");
    let res = fake_result(30, &["Ux", "p"]);
    let outs = build_outputs(&res, &series_of(&["Ux", "p"]), true, 1.5e-4, &dir).unwrap();
    let Data::Json(f) = &outs[0] else { panic!() };
    let ux = &f["series"]["Ux"];
    assert_eq!(ux["context_length"], 40);
    assert_eq!(ux["q50"].as_array().unwrap().len(), 30);
    // log space -2.0 is 1e-2 in the original space
    assert!((ux["q50"][0].as_f64().unwrap() - 1e-2).abs() < 1e-12);
    for i in 0..30 {
        let (a, b, c) = (ux["q10"][i].as_f64().unwrap(), ux["q50"][i].as_f64().unwrap(), ux["q90"][i].as_f64().unwrap());
        assert!(a <= b && b <= c);
    }
    // median log is -2 - 0.1 i; it first drops below 1.5e-4 at i = 19, which is step 20
    assert_eq!(ux["crossing"]["status"], "crosses");
    assert_eq!(ux["crossing"]["step"], 20);
    let Data::Report(r) = &outs[1] else { panic!() };
    assert_eq!(r.metrics["ai_series_count"].value, 2.0);
    assert_eq!(r.metrics["ai_ux_crossing_step"].value, 20.0);
    assert_eq!(r.metrics["ai_peak_vram_gb"].value, 0.9);
    assert!(r.metrics.keys().all(|k| k.starts_with("ai_")));
    let Data::Json(m) = &outs[2] else { panic!() };
    assert_eq!(m["revision"], "abc123");
    // a result with a short quantile array is rejected
    let mut bad = res.clone();
    bad["series"][0]["q50"] = json!([1.0]);
    assert!(build_outputs(&bad, &series_of(&["Ux", "p"]), true, 1.5e-4, &dir).is_err());
    assert!(build_outputs(&res, &series_of(&["other"]), true, 1.5e-4, &dir).is_err());
}

#[test]
fn advisory_fields_are_always_present() {
    let dir = tmp("advisory");
    let check = |outs: &[Data]| {
        let Data::Json(f) = &outs[0] else { panic!() };
        assert_eq!(f["advisory"], true);
        assert_eq!(f["label"], FORECAST_LABEL);
        assert_eq!(f["cfd_validation"], CFD_VALIDATION);
        let Data::Report(r) = &outs[1] else { panic!() };
        assert_eq!(r.notes[0].1, FORECAST_LABEL);
        assert!(r.metrics.keys().all(|k| k.starts_with("ai_")));
    };
    let idle = cook(&dir, &json_in(json!({"a": ramp(40)})), &params()).unwrap();
    let Data::Json(f) = &idle[0] else { panic!() };
    assert_eq!(f["status"], "not_run");
    check(&idle);
    let done = build_outputs(&fake_result(10, &["a"]), &series_of(&["a"]), true, 1e-5, &dir).unwrap();
    check(&done);
}

#[test]
fn cached_result_reloads_without_run() {
    let dir = tmp("cache");
    let series = series_of(&["Ux"]);
    let key = work_key(&series, 25, true);
    let art = dir.join(format!("forecast_{key:016x}"));
    std::fs::create_dir_all(&art).unwrap();
    std::fs::write(art.join("result.json"), serde_json::to_vec(&fake_result(25, &["Ux"])).unwrap()).unwrap();
    let mut p = params();
    p.set("horizon", ParamValue::Int(25));
    let ins = json_in(json!({"series": {"Ux": series[0].values}}));
    let outs = cook(&dir, &ins, &p).unwrap();
    let Data::Json(f) = &outs[0] else { panic!() };
    assert_eq!(f["status"], "complete");
    assert_eq!(f["series"]["Ux"]["q50"].as_array().unwrap().len(), 25);
    assert!(art.join("manifest.json").is_file());
    // different inputs do not hit the cache
    p.set("horizon", ParamValue::Int(26));
    let Data::Json(f) = &cook(&dir, &ins, &p).unwrap()[0] else { panic!() };
    assert_eq!(f["status"], "not_run");
    assert_ne!(work_key(&series, 25, true), work_key(&series, 25, false));
}

#[test]
fn node_is_registered_and_described() {
    let info = ForecastNode.info();
    assert_eq!(info.id, "cfd.ai_convergence_forecast");
    assert_eq!(info.label, "AI Convergence Forecast");
}

#[test]
#[ignore = "needs the installed Chronos-2 model and WSL (set VERNON_AI3D_ROOT)"]
fn real_chronos_forecasts() {
    let root = std::env::var(bridge::ROOT_ENV).expect("VERNON_AI3D_ROOT");
    let dir = std::env::var("VERNON_AI_E2E_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("vernon_ai_e2e")).join("chronos");
    std::fs::create_dir_all(&dir).unwrap();
    let f = |i: usize| 10f64.powf(-1.0 - 0.004 * i as f64) * (1.0 + 0.05 * (0.3 * i as f64).sin());
    let series = vec![Series { name: "synthetic".into(), values: (0..600).map(f).collect() }];
    let mut p = params();
    p.set("ai_root", ParamValue::Str(root));
    p.set("run", ParamValue::Bool(true));
    p.set("horizon", ParamValue::Int(200));
    let t0 = std::time::Instant::now();
    let outs = forecast_series(&ctx(&dir), &p, &series).unwrap();
    println!("wall {:.1}s", t0.elapsed().as_secs_f64());
    let Data::Json(fc) = &outs[0] else { panic!() };
    let s = &fc["series"]["synthetic"];
    let get = |k: &str| -> Vec<f64> { s[k].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect() };
    let (q10, q50, q90) = (get("q10"), get("q50"), get("q90"));
    assert_eq!((q10.len(), q50.len(), q90.len()), (200, 200, 200));
    let true_last = 10f64.powf(-1.0 - 0.004 * 799.0);
    let last = *q50.last().unwrap();
    println!("median at step 200 = {last:e}, true continuation = {true_last:e}, ratio = {:.3}", last / true_last);
    println!("q10/q50/q90 at last step = {:e} / {:e} / {:e}", q10[199], q50[199], q90[199]);
    println!("crossing = {}", s["crossing"]);
    let Data::Json(m) = &outs[2] else { panic!() };
    println!("{}", serde_json::to_string_pretty(&**m).unwrap());
    for i in 0..200 {
        assert!(q10[i] <= q50[i] && q50[i] <= q90[i], "quantile order broken at step {i}");
    }
    assert!(last / true_last < 3.0 && true_last / last < 3.0, "median {last:e} not within 3x of {true_last:e}");
}

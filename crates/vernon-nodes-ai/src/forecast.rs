//! `cfd.ai_convergence_forecast`: Chronos-2 forecast of residual histories.
//!
//! ADVICE ONLY. The model is a general time-series model, not trained on CFD. A forecast
//! says where a number series may go if it keeps behaving as it has; it is not evidence
//! of convergence. No acceptance gate reads these outputs (metric names start with `ai_`),
//! and nothing here stops, accepts or relabels a solver run.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Map, Value};
use vernon_core::report::{Metric, Severity};
use vernon_core::Report;
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::bridge::{self, check_resources, with_margin, BridgeError, Install};
use crate::{fnv1a, CFD_VALIDATION};

pub const FORECAST_LABEL: &str = "AI forecast: advisory only";
pub const FORECAST_BRIDGE: &str = include_str!("../../../scripts/ai/chronos_bridge.py");
/// Shortest history the node accepts.
pub const MIN_POINTS: usize = 32;
pub const MAX_SERIES: usize = 16;
/// Chronos-2 predicts at most 64 output patches of 16 steps.
pub const MAX_HORIZON: i64 = 1024;
/// Measured on this project's RTX 4090 Laptop; see docs/specs/ai-local/forecast.md (single run).
pub const PEAK_VRAM_GB: f64 = 0.49;
pub const PEAK_RSS_GB: f64 = 1.5;

pub fn register(reg: &mut Registry) {
    reg.add(ForecastNode);
}

/// One named number series.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub name: String,
    pub values: Vec<f64>,
}

/// Accepts `{"series": {"name": [..]}}` or a plain object of name -> array.
pub fn series_from_json(v: &Value) -> Result<Vec<Series>, String> {
    let obj = match v.get("series") {
        Some(Value::Object(inner)) => inner,
        _ => v.as_object().ok_or("the series input must be a JSON object of name -> number array")?,
    };
    let mut out = Vec::new();
    for (name, val) in obj {
        let arr = val.as_array().ok_or_else(|| format!("series '{name}' is not an array of numbers"))?;
        let mut values = Vec::with_capacity(arr.len());
        for x in arr {
            values.push(x.as_f64().ok_or_else(|| format!("series '{name}' holds a value that is not a number"))?);
        }
        out.push(Series { name: name.clone(), values });
    }
    Ok(out)
}

/// One series per field: the first `Initial residual` of each field inside each `Time = N` block.
pub fn parse_openfoam_log(text: &str) -> Vec<Series> {
    let mut out: Vec<Series> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("Time = ") {
            seen.clear();
            continue;
        }
        let Some(at) = t.find("Solving for ") else { continue };
        let rest = &t[at + "Solving for ".len()..];
        let Some((field, rest)) = rest.split_once(',') else { continue };
        let Some(rest) = rest.trim().strip_prefix("Initial residual = ") else { continue };
        let num = rest.split(',').next().unwrap_or("").trim();
        let Ok(v) = num.parse::<f64>() else { continue };
        let field = field.trim().to_string();
        if seen.contains(&field) {
            continue;
        }
        seen.push(field.clone());
        match out.iter_mut().find(|s| s.name == field) {
            Some(s) => s.values.push(v),
            None => out.push(Series { name: field, values: vec![v] }),
        }
    }
    out
}

/// Each refusal has its own message.
pub fn validate(series: &[Series], log_transform: bool) -> Result<(), String> {
    if series.is_empty() {
        return Err("no series to forecast: connect a series input or set a solver log path".into());
    }
    if series.len() > MAX_SERIES {
        return Err(format!("{} series given; at most {MAX_SERIES} are forecast at once", series.len()));
    }
    for s in series {
        if s.values.len() < MIN_POINTS {
            return Err(format!("series '{}' has {} points; at least {MIN_POINTS} are needed", s.name, s.values.len()));
        }
        if let Some(i) = s.values.iter().position(|v| !v.is_finite()) {
            return Err(format!("series '{}' has a non-finite value at index {i}", s.name));
        }
        if log_transform {
            if let Some(i) = s.values.iter().position(|v| *v <= 0.0) {
                return Err(format!("series '{}' has a value <= 0 at index {i}; the log transform needs strictly positive values (switch it off to forecast the raw numbers)", s.name));
            }
        }
    }
    Ok(())
}

pub fn to_model(v: f64, log_transform: bool) -> f64 {
    if log_transform {
        v.log10()
    } else {
        v
    }
}

pub fn from_model(v: f64, log_transform: bool) -> f64 {
    if log_transform {
        10f64.powf(v)
    } else {
        v
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Crossing {
    AlreadyBelow,
    /// First forecast step (1-based) where the median is below the target.
    Step(usize),
    NotWithinHorizon,
}

pub fn crossing(last_observed: f64, median: &[f64], target: f64) -> Crossing {
    if last_observed < target {
        return Crossing::AlreadyBelow;
    }
    match median.iter().position(|m| *m < target) {
        Some(i) => Crossing::Step(i + 1),
        None => Crossing::NotWithinHorizon,
    }
}

fn crossing_json(c: Crossing) -> Value {
    match c {
        Crossing::AlreadyBelow => json!({"status": "already_below", "step": 0}),
        Crossing::Step(n) => json!({"status": "crosses", "step": n}),
        Crossing::NotWithinHorizon => json!({"status": "not within horizon", "step": null}),
    }
}

/// Folder name key: the series (names and exact values), the horizon and the transform.
pub fn work_key(series: &[Series], horizon: i64, log_transform: bool) -> u64 {
    let mut b = Vec::new();
    for s in series {
        b.extend_from_slice(s.name.as_bytes());
        b.push(0);
        for v in &s.values {
            b.extend_from_slice(&v.to_bits().to_le_bytes());
        }
        b.push(1);
    }
    b.extend_from_slice(format!("|{horizon}|{log_transform}").as_bytes());
    fnv1a(&b)
}

fn metric_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect()
}

fn advisory_base(status: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("advisory".into(), json!(true));
    m.insert("label".into(), json!(FORECAST_LABEL));
    m.insert("cfd_validation".into(), json!(CFD_VALIDATION));
    m.insert("status".into(), json!(status));
    m
}

fn nums(v: &Value, what: &str, horizon: usize) -> Result<Vec<f64>, String> {
    let arr = v.as_array().ok_or_else(|| format!("result has no {what} array"))?;
    let out: Vec<f64> = arr.iter().filter_map(Value::as_f64).collect();
    if out.len() != arr.len() || out.len() != horizon || out.iter().any(|x| !x.is_finite()) {
        return Err(format!("result {what} is not {horizon} finite numbers"));
    }
    Ok(out)
}

/// Turn a bridge `result.json` into the three node outputs (forecast, report, manifest).
pub fn build_outputs(res: &Value, series: &[Series], log_transform: bool, target: f64, dir: &Path) -> Result<Vec<Data>, String> {
    let horizon = res.get("horizon").and_then(Value::as_u64).ok_or("result has no horizon")? as usize;
    let rows = res.get("series").and_then(Value::as_array).ok_or("result has no series")?;
    let mut rep = Report::new(FORECAST_LABEL);
    rep.note(Severity::Warning, FORECAST_LABEL);
    rep.note(
        Severity::Info,
        "a forecast is not evidence of convergence: the model is a general time-series model, not trained on CFD; check convergence with the solver's own residual and monitor criteria",
    );
    let mut per_series = Map::new();
    for s in series {
        let row = rows.iter().find(|r| r.get("name").and_then(Value::as_str) == Some(s.name.as_str())).ok_or_else(|| format!("result has no series '{}'", s.name))?;
        let back = |v: Vec<f64>| -> Vec<f64> { v.into_iter().map(|x| from_model(x, log_transform)).collect() };
        let (q10, q50, q90) = (back(nums(&row["q10"], "q10", horizon)?), back(nums(&row["q50"], "q50", horizon)?), back(nums(&row["q90"], "q90", horizon)?));
        let last = *s.values.last().unwrap_or(&f64::NAN);
        let cross = crossing(last, &q50, target);
        let ctxlen = row.get("context_length").and_then(Value::as_u64).unwrap_or(s.values.len() as u64);
        let mn = metric_name(&s.name);
        rep.metric(format!("ai_{mn}_context_length"), Metric::new(ctxlen as f64, "count"));
        rep.metric(format!("ai_{mn}_median_final"), Metric::new(*q50.last().unwrap_or(&f64::NAN), "value"));
        match cross {
            Crossing::Step(n) => {
                rep.metric(format!("ai_{mn}_crossing_step"), Metric::new(n as f64, "steps"));
            }
            Crossing::AlreadyBelow => {
                rep.note(Severity::Info, format!("{}: the last value is already below the target {target:e}", s.name));
            }
            Crossing::NotWithinHorizon => {
                rep.note(Severity::Info, format!("{}: the median does not cross below {target:e} within {horizon} steps", s.name));
            }
        }
        per_series.insert(
            s.name.clone(),
            json!({"context_length": ctxlen, "quantile_levels": [0.1, 0.5, 0.9], "q10": q10, "q50": q50, "q90": q90, "crossing": crossing_json(cross)}),
        );
    }
    rep.metric("ai_series_count", Metric::new(series.len() as f64, "count"));
    rep.metric("ai_horizon", Metric::new(horizon as f64, "steps"));
    for (k, unit) in [("peak_vram_GB", "GB"), ("peak_rss_GB", "GB"), ("seconds_load", "s"), ("seconds_infer", "s")] {
        if let Some(v) = res.get(k).and_then(Value::as_f64) {
            rep.metric(format!("ai_{}", k.to_lowercase()), Metric::new(v, unit));
        }
    }
    let mut fc = advisory_base("complete");
    fc.insert("horizon".into(), json!(horizon));
    fc.insert("log_transform".into(), json!(log_transform));
    fc.insert("target".into(), json!(target));
    fc.insert("series".into(), Value::Object(per_series));
    let manifest = json!({
        "status": "complete", "advisory": true, "label": FORECAST_LABEL, "cfd_validation": CFD_VALIDATION, "artifact_dir": dir.to_string_lossy(),
        "model": res.get("model"), "revision": res.get("revision"), "device": res.get("device"), "horizon": horizon, "log_transform": log_transform,
        "series_names": series.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
        "peak_vram_GB": res.get("peak_vram_GB"), "peak_rss_GB": res.get("peak_rss_GB"),
        "seconds_load": res.get("seconds_load"), "seconds_infer": res.get("seconds_infer"),
    });
    Ok(vec![Data::json(Value::Object(fc)), Data::report(rep), Data::json(manifest)])
}

fn idle(status: &str, why: &str, dir: Option<&Path>) -> Vec<Data> {
    let mut rep = Report::new(FORECAST_LABEL);
    rep.note(Severity::Warning, FORECAST_LABEL);
    rep.note(Severity::Info, why);
    let mut fc = advisory_base(status);
    fc.insert("series".into(), json!({}));
    let mut m = advisory_base(status);
    if let Some(d) = dir {
        m.insert("artifact_dir".into(), json!(d.to_string_lossy()));
    }
    vec![Data::json(Value::Object(fc)), Data::report(rep), Data::json(Value::Object(m))]
}

/// Everything `cook` does once the series are known: validate, reload or run, build outputs.
pub fn forecast_series(ctx: &CookContext, p: &Params, series: &[Series]) -> CookResult<Vec<Data>> {
    let log_transform = p.bool("log_transform")?;
    validate(series, log_transform).map_err(|e| ctx.fail(e))?;
    let horizon = p.int("horizon")?.clamp(1, MAX_HORIZON);
    let target = p.float("target")?;
    let dir = ctx.work_dir.join(format!("forecast_{:016x}", work_key(series, horizon, log_transform)));
    let result_path = dir.join("result.json");
    if !result_path.is_file() {
        if !p.bool("run")? {
            return Ok(idle("not_run", "not run: switch on 'Run model' to forecast", Some(&dir)));
        }
        run_model(ctx, p, &dir, series, horizon, log_transform)?;
    }
    let res: Value = serde_json::from_slice(&std::fs::read(&result_path)?).map_err(|e| ctx.fail(format!("unreadable result.json: {e}")))?;
    let outs = build_outputs(&res, series, log_transform, target, &dir).map_err(|e| ctx.fail(e))?;
    if let Data::Json(m) = &outs[2] {
        let _ = std::fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&**m).unwrap_or_default());
    }
    Ok(outs)
}

fn run_model(ctx: &CookContext, p: &Params, dir: &Path, series: &[Series], horizon: i64, log_transform: bool) -> CookResult<()> {
    let install = Install::resolve(p.str("ai_root")?, p.str("wsl_distro")?).map_err(|e| ctx.fail(e.to_string()))?;
    install.require_model_dir("Chronos").map_err(|e| ctx.fail(e.to_string()))?;
    if let Ok(have) = bridge::query_resources(&install) {
        check_resources(with_margin(PEAK_VRAM_GB, PEAK_RSS_GB), have, "Chronos-2").map_err(|e| ctx.fail(e.to_string()))?;
    }
    std::fs::create_dir_all(dir)?;
    let mut map = Map::new();
    for s in series {
        map.insert(s.name.clone(), json!(s.values.iter().map(|v| to_model(*v, log_transform)).collect::<Vec<_>>()));
    }
    let series_file = dir.join("series.json");
    std::fs::write(&series_file, serde_json::to_vec(&Value::Object(map)).unwrap_or_default())?;
    let request = json!({
        "ai_root": install.root.to_string_lossy(), "series": series_file.to_string_lossy(), "out_dir": dir.to_string_lossy(),
        "horizon": horizon, "log_transform": log_transform,
    });
    let timeout = Duration::from_secs(p.int("timeout_s")?.max(30) as u64);
    bridge::run_job(&install, "Chronos", "chronos_bridge.py", FORECAST_BRIDGE, &request, dir, timeout).map_err(|e| match e {
        BridgeError::Cancelled => CookError::Other(vernon_graph::CANCELLED.into()),
        other => ctx.fail(other.to_string()),
    })?;
    Ok(())
}

pub struct ForecastNode;

impl NodeOp for ForecastNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("cfd.ai_convergence_forecast", "AI Convergence Forecast", Stage::Cfd)
            .input(PortSpec::optional("series", DataKind::Json))
            .output(PortSpec::new("forecast", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("manifest", DataKind::Json))
            .param(ParamSpec::path("ai_root", "AI-3D folder", &[], false).help("Folder holding the Chronos install. Blank uses the VERNON_AI3D_ROOT environment variable."))
            .param(ParamSpec::string("wsl_distro", "WSL distribution", "AI-3D").help("Linux environment that holds the model's Python. Blank runs Python directly (Linux hosts)."))
            .param(ParamSpec::path("log_path", "Solver log", &[], false).help("Optional solver log. The first 'Initial residual' of each field in every 'Time =' block becomes one series."))
            .param(ParamSpec::int("horizon", "Forecast steps", 200, 1, MAX_HORIZON).help("How many steps ahead to forecast. The model supports at most 1024."))
            .param(ParamSpec::bool("log_transform", "Forecast log10", true).help("Forecast the base-10 logarithm of the series, since residuals live on a log scale. Needs strictly positive values."))
            .param(ParamSpec::float("target", "Target level", 1e-5, 0.0, 1e30).help("Report the first forecast step where the median falls below this level."))
            .param(ParamSpec::int("timeout_s", "Time limit", 600, 30, 14400).unit("s").help("The model process is stopped after this long."))
            .param(ParamSpec::bool("run", "Run model", false).action().help("Starts the model. Without it the node loads an earlier result for identical inputs, if one exists."))
            .help("Advice only. Forecasts where a residual or other monitored series may head, with 10 / 50 / 90 percent bands, using a general time-series model that was not trained on CFD. A forecast is not evidence of convergence; no check reads it and it never stops or relabels a run.")
            .emulates("time series forecasting of solver residual histories")
            .spec("docs/specs/ai-local/forecast.md")
            .implemented()
            .external("Chronos")
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let mut series = Vec::new();
        if let Some(Some(Data::Json(j))) = ins.first() {
            series = series_from_json(j).map_err(|e| ctx.fail(e))?;
        }
        let log_path = p.str("log_path")?.trim().to_string();
        if !log_path.is_empty() {
            let text = std::fs::read(&log_path).map_err(|e| ctx.fail(format!("cannot read solver log {log_path}: {e}")))?;
            let mut from_log = parse_openfoam_log(&String::from_utf8_lossy(&text));
            if from_log.is_empty() {
                return Err(ctx.fail(format!("no 'Solving for ..., Initial residual' lines found in {log_path}")));
            }
            for s in &mut from_log {
                if series.iter().any(|x| x.name == s.name) {
                    s.name = format!("log_{}", s.name);
                }
            }
            series.extend(from_log);
        }
        forecast_series(ctx, p, &series)
    }
}

#[cfg(test)]
#[path = "forecast_tests.rs"]
mod tests;

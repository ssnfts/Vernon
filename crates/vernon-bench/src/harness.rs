//! Result model, comparison helper and node-cooking helpers shared by every tier.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::time::Instant;

use serde_json::{json, Value};
use vernon_core::{Geometry, Report};
use vernon_graph::{CookContext, Data, ParamValue, Params};

#[derive(Clone, Debug)]
pub enum Status {
    Pass,
    Fail(String),
    Skipped(String),
}

impl Status {
    pub fn word(&self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail(_) => "FAIL",
            Status::Skipped(_) => "SKIPPED",
        }
    }
    pub fn detail(&self) -> &str {
        match self {
            Status::Pass => "",
            Status::Fail(s) | Status::Skipped(s) => s,
        }
    }
}

/// One numeric comparison: Vernon's value against a reference and a tolerance.
#[derive(Clone, Debug)]
pub struct Cmp {
    pub name: String,
    pub measured: f64,
    pub reference: f64,
    pub tolerance: String,
    pub unit: String,
    pub ok: bool,
}

#[derive(Clone, Debug)]
pub struct Case {
    pub id: String,
    pub tier: &'static str,
    pub title: String,
    /// Where the reference comes from (textbook, paper, tool and version).
    pub reference: String,
    pub status: Status,
    pub cmps: Vec<Cmp>,
    pub notes: Vec<String>,
    pub ms: f64,
}

impl Case {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id, "tier": self.tier, "title": self.title, "reference": self.reference,
            "status": self.status.word(), "detail": self.status.detail(), "wall_ms": self.ms,
            "comparisons": self.cmps.iter().map(|c| json!({
                "name": c.name, "measured": c.measured, "reference": c.reference,
                "tolerance": c.tolerance, "unit": c.unit, "ok": c.ok})).collect::<Vec<_>>(),
            "notes": self.notes,
        })
    }
}

/// Collects comparisons while a check runs.
pub struct Ck {
    pub cmps: Vec<Cmp>,
    pub notes: Vec<String>,
    skip: Option<String>,
}

impl Ck {
    /// |measured - reference| <= tol_abs + tol_rel * |reference|.
    pub fn close(&mut self, name: &str, measured: f64, reference: f64, tol_rel: f64, tol_abs: f64, unit: &str) {
        let ok = measured.is_finite() && (measured - reference).abs() <= tol_abs + tol_rel * reference.abs();
        let tolerance = match (tol_rel > 0.0, tol_abs > 0.0) {
            (true, true) => format!("±{:.3}% + {:e}", tol_rel * 100.0, tol_abs),
            (true, false) => format!("±{:.3}%", tol_rel * 100.0),
            _ => format!("±{:e}", tol_abs),
        };
        self.cmps.push(Cmp { name: name.into(), measured, reference, tolerance, unit: unit.into(), ok });
    }
    /// measured <= bound (reference column shows the bound).
    pub fn at_most(&mut self, name: &str, measured: f64, bound: f64, unit: &str) {
        let ok = measured.is_finite() && measured <= bound;
        self.cmps.push(Cmp { name: name.into(), measured, reference: bound, tolerance: "≤ bound".into(), unit: unit.into(), ok });
    }
    pub fn at_least(&mut self, name: &str, measured: f64, bound: f64, unit: &str) {
        let ok = measured.is_finite() && measured >= bound;
        self.cmps.push(Cmp { name: name.into(), measured, reference: bound, tolerance: "≥ bound".into(), unit: unit.into(), ok });
    }
    pub fn note(&mut self, s: impl Into<String>) {
        self.notes.push(s.into());
    }
    pub fn skip(&mut self, why: impl Into<String>) {
        self.skip = Some(why.into());
    }
}

pub fn run_case(id: &str, tier: &'static str, title: &str, reference: &str, f: impl FnOnce(&mut Ck) -> Result<(), String>) -> Case {
    let t = Instant::now();
    let mut ck = Ck { cmps: vec![], notes: vec![], skip: None };
    let res = catch_unwind(AssertUnwindSafe(|| f(&mut ck)));
    let status = match res {
        Err(p) => Status::Fail(format!("panic: {}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default())),
        Ok(Err(e)) => Status::Fail(e),
        Ok(Ok(())) => {
            if let Some(s) = ck.skip.take() {
                Status::Skipped(s)
            } else if ck.cmps.is_empty() {
                Status::Fail("the check made no comparison".into())
            } else if ck.cmps.iter().all(|c| c.ok) {
                Status::Pass
            } else {
                let bad: Vec<String> = ck.cmps.iter().filter(|c| !c.ok).map(|c| format!("{} = {:.6} vs {:.6} ({})", c.name, c.measured, c.reference, c.tolerance)).collect();
                Status::Fail(bad.join("; "))
            }
        }
    };
    Case { id: id.into(), tier, title: title.into(), reference: reference.into(), status, cmps: ck.cmps, notes: ck.notes, ms: t.elapsed().as_secs_f64() * 1e3 }
}

pub fn skipped(id: &str, tier: &'static str, title: &str, reference: &str, why: &str) -> Case {
    Case { id: id.into(), tier, title: title.into(), reference: reference.into(), status: Status::Skipped(why.into()), cmps: vec![], notes: vec![], ms: 0.0 }
}

pub fn scratch(name: &str) -> PathBuf {
    let d = std::env::var_os("VERNON_BENCH_WORK").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("vernon-bench")).join(name);
    std::fs::create_dir_all(&d).ok();
    d
}

pub fn ctx(name: &str) -> CookContext {
    CookContext { node_name: name.into(), work_dir: scratch(name), frame: 0.0 }
}

/// Cook one registered node. Fails with the node's own error text.
pub fn cook(id: &str, inputs: &[Option<Data>], set: &[(&str, ParamValue)]) -> Result<Vec<Data>, String> {
    let reg = vernon_cli::registry();
    let op = reg.get(id).ok_or_else(|| format!("node {id} is not registered"))?;
    let mut p = Params::from_specs(&op.info().params);
    for (k, v) in set {
        p.set(k, v.clone());
    }
    op.cook(&ctx(id), inputs, &p).map_err(|e| format!("{id}: {e}"))
}

pub fn geo_in(g: &Geometry) -> Option<Data> {
    Some(Data::geometry(g.clone()))
}
pub fn out_geo(o: &[Data], i: usize) -> Result<Geometry, String> {
    o.get(i).and_then(|d| d.as_geometry()).map(|g| g.as_ref().clone()).ok_or_else(|| format!("output {i} is not geometry"))
}
pub fn out_report(o: &[Data]) -> Result<Report, String> {
    o.iter().find_map(|d| d.as_report().map(|r| r.as_ref().clone())).ok_or("no report output".to_string())
}
pub fn metric(r: &Report, name: &str) -> Result<f64, String> {
    r.metrics.get(name).map(|m| m.value).ok_or_else(|| format!("report has no metric '{name}' (has: {})", r.metrics.keys().cloned().collect::<Vec<_>>().join(", ")))
}
pub fn f(v: f64) -> ParamValue {
    ParamValue::Float(v)
}
pub fn i(v: i64) -> ParamValue {
    ParamValue::Int(v)
}
pub fn s(v: &str) -> ParamValue {
    ParamValue::Str(v.into())
}
pub fn b(v: bool) -> ParamValue {
    ParamValue::Bool(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_without_comparisons_fails() {
        let c = run_case("t", "A", "t", "r", |_| Ok(()));
        assert_eq!(c.status.word(), "FAIL");
    }

    #[test]
    fn tolerance_and_skip_and_panic_are_reported() {
        assert_eq!(run_case("t", "A", "t", "r", |ck| { ck.close("x", 1.0, 1.01, 0.02, 0.0, ""); Ok(()) }).status.word(), "PASS");
        assert_eq!(run_case("t", "A", "t", "r", |ck| { ck.close("x", 1.0, 1.1, 0.02, 0.0, ""); Ok(()) }).status.word(), "FAIL");
        assert_eq!(run_case("t", "A", "t", "r", |ck| { ck.skip("no tool"); Ok(()) }).status.word(), "SKIPPED");
        assert_eq!(run_case("t", "A", "t", "r", |_| panic!("boom")).status.word(), "FAIL");
        assert_eq!(run_case("t", "A", "t", "r", |ck| { ck.close("x", f64::NAN, 1.0, 1.0, 0.0, ""); Ok(()) }).status.word(), "FAIL");
    }
}

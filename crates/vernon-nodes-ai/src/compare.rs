//! `cfd.ai_aero_compare`: difference between two AI aero previews.
//!
//! Only quantities the previews really contain are compared (area-weighted mean
//! surface pressure and shear, slice speed and pressure, all in model units).
//! There is no drag or lift here and no uncertainty estimate: the surrogate has
//! not been checked against OpenFOAM for these shapes, so a difference is a
//! screening hint, never a measured improvement.

use serde_json::{json, Value};
use vernon_core::report::{Metric, Severity};
use vernon_core::Report;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, Params, PortSpec, Registry, Stage};

use crate::aero::UNIT;
use crate::{AERO_LABEL, CFD_VALIDATION};

pub fn register(reg: &mut Registry) {
    reg.add(AeroCompareNode);
}

fn json_in(inputs: &[Option<Data>], i: usize) -> Option<Value> {
    match inputs.get(i)?.as_ref()? {
        Data::Json(v) => Some((**v).clone()),
        _ => None,
    }
}

fn num(v: &Value, path: &[&str]) -> Option<f64> {
    let mut cur = v;
    for k in path {
        cur = cur.get(*k)?;
    }
    cur.as_f64()
}

/// (name, path in summary, quantity label)
const QUANTITIES: [(&str, [&str; 2], &str); 6] = [
    ("surface_pressure_mean", ["surface", "pressure_mean"], "area-weighted mean surface pressure"),
    ("surface_pressure_min", ["surface", "pressure_min"], "minimum surface pressure"),
    ("surface_pressure_max", ["surface", "pressure_max"], "maximum surface pressure"),
    ("surface_shear_mean", ["surface", "shear_mean"], "area-weighted mean wall shear magnitude"),
    ("slice_speed_mean", ["slice", "speed_mean"], "mean speed on the slice"),
    ("slice_pressure_mean", ["slice", "pressure_mean"], "mean pressure on the slice"),
];

/// Deltas (b - a) of every quantity present in both summaries.
pub fn deltas(a: &Value, b: &Value) -> Vec<(&'static str, f64, f64, f64)> {
    QUANTITIES
        .iter()
        .filter_map(|(name, path, _)| Some((*name, num(a, path)?, num(b, path)?)))
        .map(|(n, x, y)| (n, x, y, y - x))
        .collect()
}

pub struct AeroCompareNode;

impl NodeOp for AeroCompareNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("cfd.ai_aero_compare", "AI Aero Compare", Stage::Cfd)
            .input(PortSpec::new("variant_a", DataKind::Json))
            .input(PortSpec::new("variant_b", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("comparison", DataKind::Json))
            .help("Compares two AI Aero Preview summaries (B minus A) for the quantities both contain, in model units. A screening aid only: the surrogate is unvalidated against CFD and gives no uncertainty, so small differences are not evidence of a real improvement.")
            .emulates("side-by-side comparison of design variants")
            .spec("docs/specs/ai-models.md")
            .implemented()
    }

    fn cook(&self, _ctx: &CookContext, ins: &[Option<Data>], _p: &Params) -> CookResult<Vec<Data>> {
        let mut rep = Report::new("AI aero comparison");
        rep.note(Severity::Warning, AERO_LABEL);
        rep.note(Severity::Info, format!("cfd_validation = {CFD_VALIDATION}"));
        let (a, b) = (json_in(ins, 0), json_in(ins, 1));
        let mut out = json!({"label": AERO_LABEL, "cfd_validation": CFD_VALIDATION, "unit": UNIT, "deltas": []});
        let (Some(a), Some(b)) = (a, b) else {
            rep.note(Severity::Error, "both variants must be connected to AI Aero Preview summaries");
            return Ok(vec![Data::report(rep), Data::json(out)]);
        };
        let status = |v: &Value| v.get("status").and_then(Value::as_str).unwrap_or("").to_string();
        for (tag, v) in [("A", &a), ("B", &b)] {
            if status(v) != "complete" {
                rep.note(Severity::Error, format!("variant {tag} has no completed prediction (status: {})", status(v)));
            }
            let dom = v.get("domain").and_then(Value::as_str).unwrap_or("unknown");
            if !dom.starts_with("within") {
                rep.note(Severity::Warning, format!("variant {tag} domain: {dom}"));
            }
        }
        if rep.notes.iter().any(|n| n.0 == Severity::Error) {
            return Ok(vec![Data::report(rep), Data::json(out)]);
        }
        let d = deltas(&a, &b);
        if d.is_empty() {
            rep.note(Severity::Error, "the two variants share no predicted quantity (compare like with like: both surface, or both with a slice)");
        }
        let mut rows = Vec::new();
        for (name, x, y, dx) in &d {
            rep.metric(format!("ai_delta_{name}"), Metric::new(*dx, UNIT));
            rows.push(json!({"quantity": name, "a": x, "b": y, "b_minus_a": dx}));
        }
        let plane = |v: &Value| v.get("slice").and_then(|s| s.get("plane")).cloned();
        if plane(&a) != plane(&b) && plane(&a).is_some() && plane(&b).is_some() {
            rep.note(Severity::Warning, "the slice planes differ between the variants; slice values are not comparable");
        }
        rep.note(Severity::Info, "differences carry no error bar; they are not validated against CFD and must not be read as a measured improvement");
        out["deltas"] = Value::Array(rows);
        Ok(vec![Data::report(rep), Data::json(out)])
    }
}

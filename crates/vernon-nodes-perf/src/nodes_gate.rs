//! `perf.gate_summary`: rolls the reports of a pipeline up into one verdict
//! (spec ui-dcc-conventions 4.2 #14), with the same state rules as the app's
//! Gate Dashboard and the command line (`vernon_graph::gate`).

use serde_json::json;
use vernon_core::{Metric, Report, Severity};
use vernon_graph::gate::{gate_rows, report_state, worst, GateState};
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, Params, PortSpec, Stage};

use crate::util::FnNode;

/// Report inputs of the node.
pub const INPUTS: usize = 8;

fn info() -> NodeTypeInfo {
    let mut i = NodeTypeInfo::new("perf.gate_summary", "Gate Summary", Stage::Performance).input(PortSpec::new("report_1", DataKind::Report));
    for k in 2..=INPUTS {
        i = i.input(PortSpec::optional(&format!("report_{k}"), DataKind::Report));
    }
    i.output(PortSpec::new("rows", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .help("Rolls up to eight stage reports into one verdict: every limited metric with its value, limit and margin, pass and fail counts and the worst state. Its all_pass gate fails when any connected report fails a limit or carries an error note, so a graph cooked from the command line ends with one gate verdict.")
        .emulates("gate dashboard roll-up of a staged engineering pipeline")
        .spec("docs/specs/ui-dcc-conventions.md")
        .implemented()
}

fn cook(_ctx: &CookContext, ins: &[Option<Data>], _p: &Params) -> CookResult<Vec<Data>> {
    let reports: Vec<&Report> = ins
        .iter()
        .take(INPUTS)
        .filter_map(|d| d.as_ref().and_then(Data::as_report))
        .map(|r| &**r)
        .collect();
    if reports.is_empty() {
        return Err(CookError::MissingInput { index: 0, name: "report_1".into() });
    }
    let states: Vec<GateState> = reports.iter().map(|r| report_state(r)).collect();
    let rows = gate_rows(reports.iter().copied());
    let overall = worst(states.iter().copied());
    let count = |s: GateState| states.iter().filter(|x| **x == s).count() as f64;
    let failing: Vec<_> = rows.iter().filter(|r| r.state == GateState::Fail).collect();
    let mut rep = Report::new("Gate summary");
    rep.metric("reports", Metric::new(reports.len() as f64, "count"))
        .metric("reports_pass", Metric::new(count(GateState::Pass), "count"))
        .metric("reports_fail", Metric::new(count(GateState::Fail), "count"))
        .metric("reports_no_limits", Metric::new(count(GateState::NoLimits), "count"))
        .metric("limited_metrics", Metric::new(rows.len() as f64, "count"))
        .metric("failing_metrics", Metric::new(failing.len() as f64, "count"))
        .metric("all_pass", Metric::new(f64::from(overall != GateState::Fail), "-").with_limit(1.0, false));
    for r in &failing {
        rep.note(Severity::Warning, format!("{}: {} = {:.4} {} against {:.4} (margin {:+.4})", r.report, r.metric, r.value, r.unit, r.limit, r.margin));
    }
    for (r, s) in reports.iter().zip(&states) {
        if *s == GateState::Fail && r.notes.iter().any(|(sev, _)| *sev == Severity::Error) {
            rep.note(Severity::Warning, format!("{}: carries an error note", r.title));
        }
    }
    let rows_json: Vec<_> = rows
        .iter()
        .map(|r| json!({"report": r.report, "metric": r.metric, "value": r.value, "unit": r.unit, "limit": r.limit, "margin": r.margin, "state": r.state.text()}))
        .collect();
    let out = json!({
        "rows": rows_json,
        "worst": overall.text(),
        "reports": reports.iter().zip(&states).map(|(r, s)| json!({"title": r.title, "state": s.text()})).collect::<Vec<_>>(),
    });
    Ok(vec![Data::json(out), Data::report(rep)])
}

pub fn register(reg: &mut vernon_graph::Registry) {
    reg.add(FnNode::new(info, cook));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ins: Vec<Option<Data>>) -> CookResult<Vec<Data>> {
        let ctx = CookContext { node_name: "gs".into(), work_dir: std::env::temp_dir(), frame: 0.0 };
        cook(&ctx, &ins, &Params::default())
    }

    #[test]
    fn rolls_reports_into_one_verdict() {
        let mut heat = Report::new("Heat margin");
        heat.metric("margin_k", Metric::new(-4.7, "K").with_limit(0.0, false));
        let mut fit = Report::new("Build volume fit");
        fit.metric("fits", Metric::new(1.0, "-").with_limit(1.0, false));
        let info = Report::new("Stats");
        let out = run(vec![Some(Data::report(fit.clone())), None, Some(Data::report(info.clone()))]).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(r.passes(), "{r:?}");
        assert_eq!(r.metrics["reports_pass"].value, 1.0);
        assert_eq!(r.metrics["reports_no_limits"].value, 1.0);
        assert_eq!(out[0].as_json().unwrap()["worst"], "PASS");
        let out = run(vec![Some(Data::report(fit)), Some(Data::report(heat)), Some(Data::report(info))]).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(!r.passes());
        assert_eq!(r.metrics["failing_metrics"].value, 1.0);
        let rows = out[0].as_json().unwrap();
        assert_eq!(rows["worst"], "FAIL");
        assert_eq!(rows["rows"][1]["metric"], "margin_k");
        assert!((rows["rows"][1]["margin"].as_f64().unwrap() + 4.7).abs() < 1e-12);
        assert!(run(vec![None]).is_err());
    }
}

//! Gate dashboard logic: for each pipeline stage, the latest report and
//! whether its limits pass. A state is always shown as glyph + text.

use std::sync::Arc;

use vernon_core::{Metric, Report};
use vernon_graph::{DataKind, NodeId, Stage};

use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GateState {
    /// Every limited metric passes.
    Pass,
    /// A report exists but nothing in it has a limit.
    NoLimits,
    /// No node of this stage has produced a report.
    Missing,
    /// The stage's report node failed to cook.
    Error,
    /// At least one limit fails.
    Fail,
}

impl GateState {
    pub fn glyph(self) -> &'static str {
        match self {
            GateState::Pass => "✔",
            GateState::Fail => "✖",
            GateState::NoLimits => "○",
            GateState::Missing => "·",
            GateState::Error => "⚠",
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            GateState::Pass => "PASS",
            GateState::Fail => "FAIL",
            GateState::NoLimits => "n/a",
            GateState::Missing => "no report",
            GateState::Error => "ERROR",
        }
    }
}

pub fn report_state(r: &Report) -> GateState {
    if !r.passes() {
        GateState::Fail
    } else if r.metrics.values().any(|m| m.limit.is_some()) {
        GateState::Pass
    } else {
        GateState::NoLimits
    }
}

pub fn metric_state(m: &Metric) -> GateState {
    match m.passes() {
        Some(true) => GateState::Pass,
        Some(false) => GateState::Fail,
        None => GateState::NoLimits,
    }
}

/// Signed margin to the limit in the metric's unit (positive = passing).
pub fn margin(m: &Metric) -> Option<f64> {
    m.limit
        .map(|l| if m.lower_is_better { l - m.value } else { m.value - l })
}

#[derive(Debug, Clone)]
pub struct StageGate {
    pub stage: Stage,
    pub node: Option<NodeId>,
    pub report: Option<Arc<Report>>,
    pub error: Option<String>,
    pub state: GateState,
}

/// One row per stage: the newest node of that stage that declares a report
/// output decides the gate.
pub fn stage_gates(st: &AppState) -> Vec<StageGate> {
    Stage::ALL
        .iter()
        .map(|&stage| {
            let node = st
                .graph
                .nodes
                .keys()
                .copied()
                .filter(|id| {
                    st.info(*id)
                        .is_some_and(|i| i.stage == stage && i.outputs.iter().any(|o| o.kind == DataKind::Report))
                })
                .max();
            let report = node.and_then(|n| st.report(n));
            let error = node.and_then(|n| st.error_of(n).map(str::to_string));
            let state = match (&report, &error, node) {
                (_, Some(_), _) => GateState::Error,
                (Some(r), _, _) => report_state(r),
                _ => GateState::Missing,
            };
            StageGate {
                stage,
                node,
                report,
                error,
                state,
            }
        })
        .collect()
}

/// Worst state over stages that have a report node.
pub fn overall(gates: &[StageGate]) -> GateState {
    gates
        .iter()
        .filter(|g| g.state != GateState::Missing)
        .map(|g| g.state)
        .max()
        .unwrap_or(GateState::Missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::build_registry;
    use vernon_graph::ParamValue;

    #[test]
    fn gates_follow_latest_report() {
        let mut st = AppState::new(build_registry());
        let part = st.create("geo.box").unwrap();
        let fit = st.create("print.build_fit").unwrap();
        st.cook_all();
        let gates = stage_gates(&st);
        assert_eq!(gates.len(), Stage::ALL.len());
        let print = gates.iter().find(|g| g.stage == Stage::Print).unwrap();
        assert_eq!(print.node, Some(fit));
        assert_eq!(print.state, GateState::Pass);
        let cfd = gates.iter().find(|g| g.stage == Stage::Cfd).unwrap();
        assert_eq!(cfd.state, GateState::Missing);
        // Grow the part beyond the printer volume: the gate fails.
        st.set_param(part, "size", ParamValue::Vec3(glam::DVec3::splat(600.0)), 0.0);
        st.cook_all();
        let gates = stage_gates(&st);
        assert_eq!(gates.iter().find(|g| g.stage == Stage::Print).unwrap().state, GateState::Fail);
        assert_eq!(overall(&gates), GateState::Fail);
    }

    #[test]
    fn states_have_distinct_glyph_and_text() {
        let all = [GateState::Pass, GateState::Fail, GateState::NoLimits, GateState::Missing, GateState::Error];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.glyph(), b.glyph());
                assert_ne!(a.text(), b.text());
            }
        }
        let m = Metric::new(104.7, "°C").with_limit(100.0, true);
        assert_eq!(metric_state(&m), GateState::Fail);
        assert!((margin(&m).unwrap() + 4.7).abs() < 1e-9);
        let m = Metric::new(0.92, "").with_limit(0.88, false);
        assert_eq!(metric_state(&m), GateState::Pass);
    }
}

//! Gate dashboard logic: for each pipeline stage, every report-producing
//! node and whether its limits pass. A stage's state is the worst of its
//! gates. A state is always shown as glyph + text. The state rules live in
//! `vernon_graph::gate`, shared with the command line and the gate summary node.

use std::sync::Arc;

use vernon_core::Report;
use vernon_graph::{DataKind, NodeId, Stage};

use crate::state::AppState;

pub use vernon_graph::gate::{margin, metric_state, report_state, report_state_expecting, GateState};

/// One report-producing node of a stage.
#[derive(Debug, Clone)]
pub struct NodeGate {
    pub node: NodeId,
    pub report: Option<Arc<Report>>,
    pub error: Option<String>,
    pub state: GateState,
    /// Metric names the scene declares as expected failures (`expect_fail`).
    pub expected: Vec<String>,
}

/// One stage: all of its report nodes (oldest first) and the worst state.
#[derive(Debug, Clone)]
pub struct StageGate {
    pub stage: Stage,
    pub nodes: Vec<NodeGate>,
    pub state: GateState,
}

impl StageGate {
    /// Every failing limited metric of the stage as (node, metric name).
    pub fn failing_metrics(&self) -> Vec<(NodeId, String)> {
        self.nodes
            .iter()
            .filter_map(|n| n.report.as_ref().map(|r| (n, r)))
            .flat_map(|(n, r)| {
                r.metrics
                    .iter()
                    .filter(|(k, m)| m.passes() == Some(false) && !n.expected.contains(k))
                    .map(move |(k, _)| (n.node, k.clone()))
            })
            .collect()
    }
}

/// One row per stage, rolling up every node of that stage that declares a
/// report output: the stage state is the worst state over those nodes.
pub fn stage_gates(st: &AppState) -> Vec<StageGate> {
    let mut ids: Vec<NodeId> = st.graph.nodes.keys().copied().collect();
    ids.sort();
    Stage::ALL
        .iter()
        .map(|&stage| {
            let nodes: Vec<NodeGate> = ids
                .iter()
                .copied()
                .filter(|id| {
                    st.info(*id)
                        .is_some_and(|i| i.stage == stage && i.outputs.iter().any(|o| o.kind == DataKind::Report))
                })
                .map(|node| {
                    let report = st.report(node);
                    let error = st.error_of(node).map(str::to_string);
                    let expected: Vec<String> = st
                        .graph
                        .nodes
                        .get(&node)
                        .map(|n| n.expect_fail.keys().cloned().collect())
                        .unwrap_or_default();
                    let state = match (&report, &error) {
                        (_, Some(_)) => GateState::Error,
                        (Some(r), _) => report_state_expecting(r, |m| expected.iter().any(|e| e == m)),
                        _ => GateState::Missing,
                    };
                    NodeGate {
                        node,
                        report,
                        error,
                        state,
                        expected,
                    }
                })
                .collect();
            let state = nodes.iter().map(|n| n.state).max().unwrap_or(GateState::Missing);
            StageGate { stage, nodes, state }
        })
        .collect()
}

/// Worst state over all stages; stages without reports rank lowest, so a
/// pipeline whose gates all pass reads PASS.
pub fn overall(gates: &[StageGate]) -> GateState {
    gates.iter().map(|g| g.state).max().unwrap_or(GateState::Missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::build_registry;
    use vernon_core::Metric;
    use vernon_graph::ParamValue;

    #[test]
    fn stage_rolls_up_every_report_node() {
        let mut st = AppState::new(build_registry());
        let part = st.create("geo.box").unwrap();
        let fit = st.create("print.build_fit").unwrap();
        st.cook_all();
        let gates = stage_gates(&st);
        assert_eq!(gates.len(), Stage::ALL.len());
        let print = gates.iter().find(|g| g.stage == Stage::Print).unwrap();
        assert_eq!(print.nodes.iter().map(|n| n.node).collect::<Vec<_>>(), vec![fit]);
        assert_eq!(print.state, GateState::Pass);
        assert_eq!(overall(&gates), GateState::Pass, "missing stages must not hide a PASS");
        let cfd = gates.iter().find(|g| g.stage == Stage::Cfd).unwrap();
        assert_eq!(cfd.state, GateState::Missing);
        // Grow the part beyond the printer volume, then add a newer passing
        // gate on a small part: the older failing gate still fails the stage.
        st.set_param(part, "size", ParamValue::Vec3(glam::DVec3::splat(600.0)), 0.0);
        st.create("geo.box").unwrap(); // a small part; the next fit wires to it
        let fit2 = st.create("print.build_fit").unwrap();
        st.cook_all();
        let gates = stage_gates(&st);
        let print = gates.iter().find(|g| g.stage == Stage::Print).unwrap();
        assert_eq!(print.nodes.len(), 2);
        assert_eq!(print.nodes[1].node, fit2);
        assert_eq!(print.nodes[1].state, GateState::Pass);
        assert_eq!(print.state, GateState::Fail);
        assert!(print.failing_metrics().iter().all(|(n, _)| *n == fit));
        assert!(!print.failing_metrics().is_empty());
        assert_eq!(overall(&gates), GateState::Fail);
    }

    #[test]
    fn expected_failures_do_not_fail_the_stage() {
        let mut st = AppState::new(build_registry());
        let part = st.create("geo.box").unwrap();
        let fit = st.create("print.build_fit").unwrap();
        st.set_param(part, "size", ParamValue::Vec3(glam::DVec3::splat(600.0)), 0.0);
        st.cook_all();
        let gates = stage_gates(&st);
        let failing = gates.iter().find(|g| g.stage == Stage::Print).unwrap().failing_metrics();
        assert!(!failing.is_empty());
        let node = st.graph.nodes.get_mut(&fit).unwrap();
        for (_, metric) in &failing {
            node.expect_fail.insert(metric.clone(), "oversized test part".into());
        }
        let gates = stage_gates(&st);
        let print = gates.iter().find(|g| g.stage == Stage::Print).unwrap();
        assert!(print.failing_metrics().is_empty(), "expected metrics no longer count");
        // Error notes are never excused by expect_fail (vernon-cli agrees).
        let has_error_note = print.nodes[0]
            .report
            .as_ref()
            .is_some_and(|r| r.notes.iter().any(|(s, _)| *s == vernon_core::Severity::Error));
        let want = if has_error_note { GateState::Fail } else { GateState::Pass };
        assert_eq!(print.state, want);
    }

    #[test]
    fn severity_order_keeps_missing_lowest() {
        use GateState::*;
        assert!(Fail > Error && Error > Pass && Pass > NoLimits && NoLimits > Missing);
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

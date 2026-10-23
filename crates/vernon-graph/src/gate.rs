//! Gate roll-up shared by the app's Gate Dashboard, the command line and the
//! `perf.gate_summary` node: the state of a report or metric against its
//! limits, the signed margin, and the worst state over many reports.

use vernon_core::{Metric, Report};

/// Gate states, declared from least to most severe so `max()` picks the
/// worst. `Missing` ranks lowest: a stage without reports never hides a
/// verdict from the stages that have one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GateState {
    /// No report was produced.
    Missing,
    /// A report exists but nothing in it has a limit.
    NoLimits,
    /// Every limited metric passes.
    Pass,
    /// The report's node failed to cook.
    Error,
    /// At least one limit fails (or the report carries an error note).
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

/// Like [`report_state`], but a failing metric for which `expected(name)` is
/// true (listed in the node's `expect_fail`) does not fail the report. Error
/// notes still do.
pub fn report_state_expecting(r: &Report, expected: impl Fn(&str) -> bool) -> GateState {
    let metric_fails = r
        .metrics
        .iter()
        .any(|(name, m)| m.passes() == Some(false) && !expected(name));
    let error_note = r.notes.iter().any(|(s, _)| *s == vernon_core::Severity::Error);
    if metric_fails || error_note {
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
    m.limit.map(|l| if m.lower_is_better { l - m.value } else { m.value - l })
}

/// Worst of `states`; `Missing` when there are none.
pub fn worst(states: impl IntoIterator<Item = GateState>) -> GateState {
    states.into_iter().max().unwrap_or(GateState::Missing)
}

/// One limited metric of a report.
#[derive(Debug, Clone, PartialEq)]
pub struct GateRow {
    pub report: String,
    pub metric: String,
    pub value: f64,
    pub unit: String,
    pub limit: f64,
    pub margin: f64,
    pub state: GateState,
}

/// Every limited metric of `reports`, in order.
pub fn gate_rows<'a>(reports: impl IntoIterator<Item = &'a Report>) -> Vec<GateRow> {
    let mut rows = Vec::new();
    for r in reports {
        for (name, m) in &r.metrics {
            let (Some(limit), Some(mg)) = (m.limit, margin(m)) else { continue };
            rows.push(GateRow {
                report: r.title.clone(),
                metric: name.clone(),
                value: m.value,
                unit: m.unit.clone(),
                limit,
                margin: mg,
                state: metric_state(m),
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use vernon_core::Severity;

    #[test]
    fn expected_metric_failures_are_excused_but_error_notes_are_not() {
        let mut r = Report::new("wheels");
        r.metric("wheels_found", Metric::new(0.0, "count").with_limit(4.0, false));
        r.metric("area_m2", Metric::new(2.0, "m2").with_limit(1.0, false));
        assert_eq!(report_state(&r), GateState::Fail);
        assert_eq!(report_state_expecting(&r, |m| m == "wheels_found"), GateState::Pass);
        assert_eq!(report_state_expecting(&r, |_| false), GateState::Fail);
        r.note(Severity::Error, "broken input");
        assert_eq!(report_state_expecting(&r, |m| m == "wheels_found"), GateState::Fail);
    }

    #[test]
    fn states_order_margins_and_rows() {
        use GateState::*;
        assert!(Fail > Error && Error > Pass && Pass > NoLimits && NoLimits > Missing);
        assert_eq!(worst([]), Missing);
        assert_eq!(worst([Pass, NoLimits, Missing]), Pass);
        let m = Metric::new(104.7, "degC").with_limit(100.0, true);
        assert_eq!(metric_state(&m), Fail);
        assert!((margin(&m).unwrap() + 4.7).abs() < 1e-9);
        let mut a = Report::new("heat");
        a.metric("t_max", m).metric("area", Metric::new(3.0, "m2"));
        let mut b = Report::new("fit");
        b.metric("ok", Metric::new(1.0, "-").with_limit(1.0, false));
        let rows = gate_rows([&a, &b]);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].report.as_str(), rows[0].metric.as_str(), rows[0].state), ("heat", "t_max", Fail));
        assert_eq!(rows[1].state, Pass);
        assert_eq!(report_state(&a), Fail);
        assert_eq!(report_state(&b), Pass);
        // an error note fails a report without limits
        let mut c = Report::new("c");
        assert_eq!(report_state(&c), NoLimits);
        c.note(Severity::Error, "bad");
        assert_eq!(report_state(&c), Fail);
    }
}

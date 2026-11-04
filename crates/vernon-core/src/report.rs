use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// A single measured number with its unit, e.g. `p95_deviation = 0.42 mm`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metric {
    pub value: f64,
    pub unit: String,
    /// Optional pass/fail threshold the next pipeline stage checks against.
    pub limit: Option<f64>,
    /// True when `value <= limit` passes; false when `value >= limit` passes.
    pub lower_is_better: bool,
}

impl Metric {
    /// The unit is stored in its canonical spelling and a negative zero as a
    /// plain zero (see [`crate::units`]).
    pub fn new(value: f64, unit: impl Into<String>) -> Self {
        let unit = unit.into();
        let unit = match crate::units::canonical(&unit) {
            std::borrow::Cow::Borrowed(c) if c == unit => unit,
            c => c.into_owned(),
        };
        Self {
            value: crate::units::clean_zero(value),
            unit,
            limit: None,
            lower_is_better: true,
        }
    }

    pub fn with_limit(mut self, limit: f64, lower_is_better: bool) -> Self {
        self.limit = Some(limit);
        self.lower_is_better = lower_is_better;
        self
    }

    /// `None` when no limit is set.
    pub fn passes(&self) -> Option<bool> {
        self.limit.map(|l| {
            if self.lower_is_better {
                self.value <= l
            } else {
                self.value >= l
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// The numeric hand-off between pipeline stages: every stage emits a report
/// whose metrics the next stage can gate on (deviation percentile after
/// surfacing, converged dCdA before printing, surface temperature vs HDT...).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub title: String,
    pub metrics: IndexMap<String, Metric>,
    pub notes: Vec<(Severity, String)>,
}

impl Report {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Default::default()
        }
    }

    pub fn metric(&mut self, name: impl Into<String>, metric: Metric) -> &mut Self {
        self.metrics.insert(name.into(), metric);
        self
    }

    pub fn note(&mut self, severity: Severity, text: impl Into<String>) -> &mut Self {
        self.notes.push((severity, text.into()));
        self
    }

    /// False if any metric with a limit fails or any error note exists.
    /// The first of `names` this report has, with its value. Consumers pass
    /// one of the alias lists in [`keys`] so every producer's name is found.
    pub fn first_metric<'a>(&self, names: &[&'a str]) -> Option<(&'a str, f64)> {
        names.iter().find_map(|n| self.metrics.get(*n).map(|m| (*n, m.value)))
    }

    pub fn passes(&self) -> bool {
        self.metrics.values().all(|m| m.passes() != Some(false))
            && !self.notes.iter().any(|(s, _)| *s == Severity::Error)
    }

    /// Merge another report's metrics and notes into this one, prefixing
    /// metric names with `prefix.` when `prefix` is non-empty.
    pub fn absorb(&mut self, prefix: &str, other: &Report) {
        for (k, v) in &other.metrics {
            let key = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            self.metrics.insert(key, v.clone());
        }
        self.notes.extend(other.notes.iter().cloned());
    }
}

/// Metric names that one stage writes and another reads. Producers use the
/// constants; consumers look values up through the alias lists with
/// [`Report::first_metric`] and say so when a connected report has none.
pub mod keys {
    /// Heat-deflection temperature, degC (the value a check compares with).
    pub const HDT_C: &str = "hdt_c";
    /// Heat-deflection temperature at 0.45 MPa, degC.
    pub const HDT_045_C: &str = "hdt_045_c";
    /// Heat-deflection temperature at 1.8 MPa, degC.
    pub const HDT_18_C: &str = "hdt_18_c";
    /// Sunlit surface temperature, degC.
    pub const SURFACE_TEMP_C: &str = "surface_temp_c";
    /// Hottest temperature over a mesh, degC.
    pub const MAX_TEMP_C: &str = "max_temp_c";

    /// Where a heat-deflection temperature may be found, preferred first.
    /// The 0.45 MPa value suits lightly loaded body panels.
    pub const HDT_ALIASES: [&str; 3] = [HDT_C, HDT_045_C, HDT_18_C];
    /// Where a surface temperature may be found, preferred first.
    pub const SURFACE_TEMP_ALIASES: [&str; 5] =
        [SURFACE_TEMP_C, "max_surface_temp_c", "temp_c_max", MAX_TEMP_C, "peak_temp_c"];

    /// 1 when the heat-deflection value in the same report is the annealed one.
    pub const ANNEALED: &str = "annealed";

    /// Surface-to-reference deviation, 95th percentile of |d|, mm.
    pub const DEV_P95_MM: &str = "dev_p95_mm";
    /// Scan-to-mesh deviation, 95th percentile of |d|, mm.
    pub const P95_ABS: &str = "p95_abs";
    /// Patch-fit residual, 95th percentile of |d|, mm.
    pub const FIT_P95_MM: &str = "fit_p95_mm";
    /// Where a 95th-percentile deviation may be found. The stages keep their
    /// own names because they measure different things; prefer the comparison
    /// against the reference over a fit's own residual.
    pub const DEVIATION_P95_ALIASES: [&str; 3] = [DEV_P95_MM, P95_ABS, FIT_P95_MM];

    /// Drag area, m2.
    pub const CDA: &str = "CdA";
    /// Lift area, m2 (negative is downforce).
    pub const CLA: &str = "ClA";
    /// Where a drag area may be found: reports, vehicle tables, force results.
    pub const CDA_ALIASES: [&str; 3] = [CDA, "cda_m2", "cda"];
    /// Where a lift area may be found: reports, vehicle tables, force results.
    pub const CLA_ALIASES: [&str; 3] = [CLA, "cla_m2", "cla"];
    /// Explicit iteration-readiness status carried by an airflow force report.
    /// Absence is a legacy diagnostic; this does not establish benchmark validity.
    pub const CFD_ITERATION_QUALIFIED: &str = "iteration_qualified";
    pub const CFD_ITERATION_QUALIFIED_ALIASES: [&str; 1] = [CFD_ITERATION_QUALIFIED];
}

#[cfg(test)]
mod key_tests {
    use super::*;

    #[test]
    fn first_metric_follows_alias_order() {
        let mut r = Report::new("m");
        r.metric(keys::HDT_18_C, Metric::new(90.0, "degC"));
        r.metric(keys::HDT_045_C, Metric::new(101.5, "degC"));
        assert_eq!(r.first_metric(&keys::HDT_ALIASES), Some((keys::HDT_045_C, 101.5)));
        assert_eq!(r.first_metric(&keys::SURFACE_TEMP_ALIASES), None);
        r.metric(keys::FIT_P95_MM, Metric::new(0.08, "mm"));
        r.metric(keys::DEV_P95_MM, Metric::new(0.17, "mm"));
        assert_eq!(r.first_metric(&keys::DEVIATION_P95_ALIASES), Some((keys::DEV_P95_MM, 0.17)));
        r.metric("cda_m2", Metric::new(0.64, "m2"));
        assert_eq!(r.first_metric(&keys::CDA_ALIASES), Some(("cda_m2", 0.64)));
    }
}

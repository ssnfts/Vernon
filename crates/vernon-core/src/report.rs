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
    pub fn new(value: f64, unit: impl Into<String>) -> Self {
        Self {
            value,
            unit: unit.into(),
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

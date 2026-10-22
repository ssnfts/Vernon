//! Shared helpers for the capture nodes: report builders, the `metric`
//! detail attribute, id-keyed marker clouds, JSON matrices and a tiny RNG.

use std::sync::Arc;

use glam::{DMat4, DVec3};
use serde_json::{json, Value};
use vernon_core::{AttribArray, Geometry, Metric, Report, Severity};
use vernon_graph::{CookContext, CookError, CookResult, Data, NodeTypeInfo, Stage};

/// Spec every node in this crate follows.
pub const SPEC: &str = "docs/specs/capture-scanning.md";

/// Name of the detail attribute that records whether a geometry is in true
/// millimetres (`1`) or still in an arbitrary / unverified scale (`0`).
pub const METRIC_ATTRIB: &str = "metric";

/// Start a capture-stage node description pointing at the spec.
pub fn base(id: &str, label: &str) -> NodeTypeInfo {
    NodeTypeInfo::new(id, label, Stage::Capture).spec(SPEC)
}

/// Record the metric flag on a geometry.
pub fn set_metric_flag(g: &mut Geometry, metric: bool) {
    g.detail_attribs
        .set(METRIC_ATTRIB, AttribArray::Int(vec![i64::from(metric)]));
}

/// Read the metric flag; `None` when the geometry does not carry one.
pub fn metric_flag(g: &Geometry) -> Option<bool> {
    g.detail_attribs
        .ints(METRIC_ATTRIB)
        .ok()
        .and_then(|v| v.first().map(|&x| x != 0))
}

/// Marker ids of a point cloud: the `id` point attribute (string or integer)
/// when present, otherwise the point index.
pub fn point_ids(g: &Geometry) -> Vec<String> {
    if let Ok(s) = g.point_attribs.strs("id") {
        return s.to_vec();
    }
    if let Ok(v) = g.point_attribs.ints("id") {
        return v.iter().map(|x| x.to_string()).collect();
    }
    (0..g.points.len()).map(|i| i.to_string()).collect()
}

/// True when the cloud carries an `id` point attribute.
pub fn has_ids(g: &Geometry) -> bool {
    g.point_attribs.strs("id").is_ok() || g.point_attribs.ints("id").is_ok()
}

/// Longest bounding-box edge in mm (0 for an empty geometry).
pub fn longest_edge(g: &Geometry) -> f64 {
    g.bounds().size().max_element()
}

/// Add bounding-box metrics (`bbox_x`, `bbox_y`, `bbox_z`, `bbox_diagonal`).
pub fn bbox_metrics(report: &mut Report, g: &Geometry) {
    let s = g.bounds().size();
    report.metric("bbox_x", Metric::new(s.x, "mm"));
    report.metric("bbox_y", Metric::new(s.y, "mm"));
    report.metric("bbox_z", Metric::new(s.z, "mm"));
    report.metric("bbox_diagonal", Metric::new(s.length(), "mm"));
}

/// Standard white-box warning.
pub fn placeholder(report: &mut Report, what: &str) {
    report.note(Severity::Warning, format!("white-box placeholder: {what}"));
}

/// Row-major nested 4x4 JSON of a matrix.
pub fn mat_to_json(m: &DMat4) -> Value {
    let c = m.to_cols_array_2d();
    let rows: Vec<Vec<f64>> = (0..4)
        .map(|r| (0..4).map(|col| c[col][r]).collect())
        .collect();
    json!(rows)
}

/// Parse a 4x4 matrix from nested rows, a flat list of 16 numbers (row
/// major) or an object holding either under `transform` / `matrix`.
pub fn mat_from_json(v: &Value) -> Option<DMat4> {
    match v {
        Value::Object(o) => o
            .get("transform")
            .or_else(|| o.get("matrix"))
            .and_then(mat_from_json),
        Value::Array(a) if a.len() == 16 => {
            let f: Option<Vec<f64>> = a.iter().map(Value::as_f64).collect();
            let f = f?;
            let mut cols = [[0.0; 4]; 4];
            for r in 0..4 {
                for c in 0..4 {
                    cols[c][r] = f[r * 4 + c];
                }
            }
            Some(DMat4::from_cols_array_2d(&cols))
        }
        Value::Array(a) if a.len() == 4 => {
            let mut cols = [[0.0; 4]; 4];
            for (r, row) in a.iter().enumerate() {
                let row = row.as_array()?;
                if row.len() != 4 {
                    return None;
                }
                for (c, x) in row.iter().enumerate() {
                    cols[c][r] = x.as_f64()?;
                }
            }
            Some(DMat4::from_cols_array_2d(&cols))
        }
        _ => None,
    }
}

/// JSON input at `index`, if connected and of JSON kind.
pub fn json_input(inputs: &[Option<Data>], index: usize) -> Option<Arc<Value>> {
    inputs
        .get(index)
        .and_then(|d| d.as_ref())
        .and_then(|d| d.as_json().cloned())
}

/// Fail unless `value` is finite and strictly positive.
pub fn require_positive(ctx: &CookContext, name: &str, value: f64) -> CookResult<()> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(ctx.fail(format!("parameter '{name}' must be a positive number, got {value}")))
    }
}

/// Fail unless `value` is finite and not negative.
pub fn require_non_negative(ctx: &CookContext, name: &str, value: f64) -> CookResult<()> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(ctx.fail(format!("parameter '{name}' must not be negative, got {value}")))
    }
}

/// Wrong-choice error for an enum-valued parameter.
pub fn bad_choice(ctx: &CookContext, name: &str, value: &str) -> CookError {
    ctx.fail(format!("parameter '{name}' has unknown value '{value}'"))
}

/// Deterministic xorshift64* generator (seeded; never zero state).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform integer in `[0, n)`.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }

    /// Standard normal sample (Box-Muller).
    pub fn gaussian(&mut self) -> f64 {
        let u1 = self.next_f64().max(1e-300);
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

/// Scale points about a pivot.
pub fn scale_about(g: &mut Geometry, s: f64, pivot: DVec3) {
    for p in &mut g.points {
        *p = pivot + (*p - pivot) * s;
    }
}

/// Mean of points (zero for an empty slice).
pub fn centroid(points: &[DVec3]) -> DVec3 {
    if points.is_empty() {
        return DVec3::ZERO;
    }
    points.iter().copied().sum::<DVec3>() / points.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_json_round_trip() {
        let m = DMat4::from_rotation_translation(
            glam::DQuat::from_rotation_z(0.3),
            DVec3::new(1.0, 2.0, 3.0),
        );
        let back = mat_from_json(&mat_to_json(&m)).unwrap();
        assert!(m.abs_diff_eq(back, 1e-12));
        let flat: Vec<f64> = mat_to_json(&m)
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|r| r.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()))
            .collect();
        assert!(mat_from_json(&json!(flat)).unwrap().abs_diff_eq(m, 1e-12));
        assert!(mat_from_json(&json!({"matrix": mat_to_json(&m)})).is_some());
        assert!(mat_from_json(&json!([1, 2, 3])).is_none());
    }

    #[test]
    fn rng_is_deterministic_and_gaussian_is_centred() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        assert_eq!(a.next_u64(), b.next_u64());
        let mut r = Rng::new(1);
        let n = 20000;
        let mean: f64 = (0..n).map(|_| r.gaussian()).sum::<f64>() / n as f64;
        assert!(mean.abs() < 0.05);
    }
}

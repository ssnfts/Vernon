//! Shared helpers: input access, colour ramps, deterministic sums.

use std::cmp::Ordering;
use std::sync::Arc;

use glam::DVec3;
use serde_json::Value;
use vernon_core::{Geometry, Report, Severity};
use vernon_graph::{CookError, CookResult, Data, DataKind};

pub const SOLAR_SPEC: &str = "docs/specs/solar-thermal.md";
pub const FLATTEN_SPEC: &str = "docs/specs/rhino8-mesh-tools.md";

/// A required `Json` input.
pub fn json_in(inputs: &[Option<Data>], index: usize) -> CookResult<Arc<Value>> {
    match inputs.get(index).and_then(|d| d.as_ref()) {
        Some(Data::Json(v)) => Ok(v.clone()),
        Some(other) => Err(CookError::WrongInput {
            index,
            expected: DataKind::Json,
            found: other.kind(),
        }),
        None => Err(CookError::MissingInput {
            index,
            name: format!("input {index}"),
        }),
    }
}

/// An optional `Json` input (`Ok(None)` when unconnected).
pub fn opt_json_in(inputs: &[Option<Data>], index: usize) -> CookResult<Option<Arc<Value>>> {
    match inputs.get(index).and_then(|d| d.as_ref()) {
        None => Ok(None),
        Some(Data::Json(v)) => Ok(Some(v.clone())),
        Some(other) => Err(CookError::WrongInput {
            index,
            expected: DataKind::Json,
            found: other.kind(),
        }),
    }
}

/// Mark a report as coming from a white-box placeholder.
pub fn placeholder(report: &mut Report, what: &str) {
    report.note(Severity::Warning, format!("white-box placeholder: {what}"));
}

/// Total order wrapper for `f64` (Dijkstra heaps, sorting).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Dist(pub f64);
impl Eq for Dist {}
impl PartialOrd for Dist {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Dist {
    fn cmp(&self, o: &Self) -> Ordering {
        self.0.total_cmp(&o.0)
    }
}

/// Reproducible sum: fixed 4096-element chunks, partials added in order.
pub fn det_sum(v: &[f64]) -> f64 {
    v.chunks(4096).map(|c| c.iter().sum::<f64>()).sum()
}

/// Sequential ramp, `t` in 0..1: blue, cyan, green, yellow, red.
pub fn ramp_seq(t: f64) -> DVec3 {
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let stops = [
        DVec3::new(0.10, 0.20, 0.80),
        DVec3::new(0.10, 0.75, 0.85),
        DVec3::new(0.20, 0.80, 0.25),
        DVec3::new(0.95, 0.90, 0.20),
        DVec3::new(0.90, 0.15, 0.10),
    ];
    let x = t * 4.0;
    let i = (x.floor() as usize).min(3);
    stops[i].lerp(stops[i + 1], x - i as f64)
}

/// Diverging ramp, `t` in -1..1: blue (negative), white, red (positive).
pub fn ramp_div(t: f64) -> DVec3 {
    let t = if t.is_finite() {
        t.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let white = DVec3::ONE;
    if t >= 0.0 {
        white.lerp(DVec3::new(0.85, 0.12, 0.10), t)
    } else {
        white.lerp(DVec3::new(0.12, 0.30, 0.85), -t)
    }
}

/// Parse a comma/space separated list of numbers.
pub fn parse_list(s: &str, name: &str) -> CookResult<Vec<f64>> {
    let mut out = Vec::new();
    for tok in s.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        out.push(
            tok.parse::<f64>()
                .map_err(|_| CookError::BadParam(name.into()))?,
        );
    }
    if out.is_empty() {
        return Err(CookError::BadParam(name.into()));
    }
    Ok(out)
}

/// Area of every primitive (0 for open or degenerate ones).
pub fn prim_areas(g: &Geometry) -> Vec<f64> {
    (0..g.prims.len())
        .map(|i| {
            if g.prims[i].closed {
                g.prim_area(i)
            } else {
                0.0
            }
        })
        .collect()
}

/// Area-weighted average of a per-primitive value onto points.
pub fn prim_to_point(g: &Geometry, vals: &[f64]) -> Vec<f64> {
    let mut sum = vec![0.0; g.points.len()];
    let mut wsum = vec![0.0; g.points.len()];
    for (i, p) in g.prims.iter().enumerate() {
        if !p.closed || p.vertices.len() < 3 {
            continue;
        }
        let a = g.prim_area(i).max(1e-12);
        for &v in &p.vertices {
            sum[v as usize] += vals[i] * a;
            wsum[v as usize] += a;
        }
    }
    sum.iter()
        .zip(&wsum)
        .map(|(s, w)| if *w > 0.0 { s / w } else { 0.0 })
        .collect()
}

/// Number of valid polygon faces.
pub fn face_count(g: &Geometry) -> usize {
    g.prims
        .iter()
        .filter(|p| p.closed && p.vertices.len() >= 3)
        .count()
}

pub fn finite_or(v: f64, d: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        d
    }
}

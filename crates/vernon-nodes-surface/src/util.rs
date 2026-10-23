//! Shared helpers for the surface-stage nodes.

use std::sync::Arc;

use glam::DVec3;
use vernon_core::{Geometry, Report, Severity};
use vernon_graph::{CookContext, CookError, CookResult, Data, NodeTypeInfo, Stage};

pub const SPEC_RE: &str = "docs/specs/reverse-engineering-surfacing.md";
pub const SPEC_MESH: &str = "docs/specs/rhino8-mesh-tools.md";

/// A stage-3 node description with its spec path set.
pub fn node_info(id: &str, label: &str, spec: &str) -> NodeTypeInfo {
    NodeTypeInfo::new(id, label, Stage::Surface).spec(spec)
}

/// Add the standard white-box warning to a report.
pub fn placeholder_note(report: &mut Report, what: &str) {
    report.note(Severity::Warning, format!("white-box placeholder: {what}"));
}

/// Nearest-rank percentile (`p` in 0..=100) of an ascending slice: the shared
/// convention of `vernon_core::stats`, so surface gates agree with the other stages.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    vernon_core::stats::percentile_pct(sorted, p)
}

/// Deterministic SplitMix64 generator (no external dependency).
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Approximately standard normal (Box-Muller).
    pub fn gauss(&mut self) -> f64 {
        let u1 = self.next_f64().max(1e-300);
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

/// An optional Json input.
pub fn json_input(inputs: &[Option<Data>], index: usize) -> Option<Arc<serde_json::Value>> {
    inputs.get(index).and_then(|d| d.as_ref()).and_then(|d| d.as_json().cloned())
}

/// Fail with a clear message unless `v` is finite and `> 0`.
pub fn positive(ctx: &CookContext, name: &str, v: f64) -> CookResult<f64> {
    if v.is_finite() && v > 0.0 {
        Ok(v)
    } else {
        Err(ctx.fail(format!("parameter '{name}' must be a positive number, got {v}")))
    }
}

/// Reject geometry with non-finite coordinates.
pub fn check_finite(ctx: &CookContext, geo: &Geometry) -> CookResult<()> {
    if geo.points.iter().any(|p| !p.is_finite()) {
        return Err(ctx.fail("input geometry has non-finite point coordinates"));
    }
    Ok(())
}

pub fn bbox_diagonal(geo: &Geometry) -> f64 {
    let b = geo.bounds();
    if b.is_empty() {
        0.0
    } else {
        b.size().length()
    }
}

/// Largest absolute coordinate (for f32-aware tolerances).
pub fn max_abs_coord(points: &[DVec3]) -> f64 {
    points.iter().map(|p| p.abs().max_element()).fold(0.0, f64::max)
}

/// Polyline geometry from point lists. `closed` loops repeat their first point.
pub fn polylines_geometry(lines: &[(Vec<DVec3>, bool)]) -> Geometry {
    let mut g = Geometry::new();
    for (pts, closed) in lines {
        if pts.len() < 2 {
            continue;
        }
        let base = g.points.len() as u32;
        g.points.extend_from_slice(pts);
        let mut idx: Vec<u32> = (0..pts.len() as u32).map(|i| i + base).collect();
        if *closed && pts.first() != pts.last() {
            idx.push(base);
        }
        g.prims.push(vernon_core::Prim::polyline(idx));
    }
    g.point_attribs.resize_all(g.points.len());
    g.prim_attribs.resize_all(g.prims.len());
    g
}

/// Convert a cook error from a JSON parse into a `Failed` error.
pub fn json_err(ctx: &CookContext, what: &str, e: String) -> CookError {
    ctx.fail(format!("{what}: {e}"))
}

/// Triangulate one polygon given as point indices into `pts` (any arity).
/// Returns triangles as index triples taken from `ring`. Quads pick the
/// diagonal whose triangle normals best agree with the Newell normal; larger
/// polygons are ear-clipped in the Newell plane.
pub fn triangulate_ring(pts: &[DVec3], ring: &[u32]) -> Vec<[u32; 3]> {
    let n = ring.len();
    if n < 3 {
        return vec![];
    }
    if n == 3 {
        return vec![[ring[0], ring[1], ring[2]]];
    }
    let p = |i: usize| pts[ring[i] as usize];
    let mut newell = DVec3::ZERO;
    for i in 0..n {
        let (a, b) = (p(i), p((i + 1) % n));
        newell += DVec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    let nn = newell.normalize_or_zero();
    if n == 4 {
        let tn = |a: usize, b: usize, c: usize| (p(b) - p(a)).cross(p(c) - p(a));
        let d02 = tn(0, 1, 2).dot(nn).min(tn(0, 2, 3).dot(nn));
        let d13 = tn(1, 2, 3).dot(nn).min(tn(1, 3, 0).dot(nn));
        return if d02 >= d13 {
            vec![[ring[0], ring[1], ring[2]], [ring[0], ring[2], ring[3]]]
        } else {
            vec![[ring[1], ring[2], ring[3]], [ring[1], ring[3], ring[0]]]
        };
    }
    // ear clipping
    let mut idx: Vec<usize> = (0..n).collect();
    let mut out = Vec::new();
    let mut guard = 0;
    while idx.len() > 3 && guard < n * n {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for k in 0..m {
            let (ia, ib, ic) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            let cross = (p(ib) - p(ia)).cross(p(ic) - p(ib));
            if cross.dot(nn) <= 1e-14 {
                continue;
            }
            let inside = idx.iter().any(|&j| {
                if j == ia || j == ib || j == ic {
                    return false;
                }
                point_in_tri(p(j), p(ia), p(ib), p(ic), nn)
            });
            if inside {
                continue;
            }
            out.push([ring[ia], ring[ib], ring[ic]]);
            idx.remove(k);
            clipped = true;
            break;
        }
        if !clipped {
            // degenerate: fan the rest
            break;
        }
    }
    if idx.len() >= 3 {
        for k in 1..idx.len() - 1 {
            out.push([ring[idx[0]], ring[idx[k]], ring[idx[k + 1]]]);
        }
    }
    out
}

fn point_in_tri(q: DVec3, a: DVec3, b: DVec3, c: DVec3, n: DVec3) -> bool {
    let s = |u: DVec3, v: DVec3, w: DVec3| (v - u).cross(w - u).dot(n);
    s(a, b, q) >= 0.0 && s(b, c, q) >= 0.0 && s(c, a, q) >= 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_is_nearest_rank() {
        let v = [0.0, 1.0, 2.0, 3.0, 4.0];
        assert_eq!(percentile(&v, 0.0), 0.0);
        assert_eq!(percentile(&v, 100.0), 4.0);
        assert!((percentile(&v, 50.0) - 2.0).abs() < 1e-12);
        // nearest rank, no interpolation: rank ceil(0.95 * 5) = 5
        assert_eq!(percentile(&v, 95.0), 4.0);
        let ten: Vec<f64> = (1..=10).map(f64::from).collect();
        assert_eq!(percentile(&ten, 95.0), 10.0);
    }

    #[test]
    fn rng_is_deterministic() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..10 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let x = Rng::new(1).next_f64();
        assert!((0.0..1.0).contains(&x));
    }

    #[test]
    fn dart_quad_triangulates_inside() {
        // concave "dart": reflex vertex at index 2
        let pts = vec![DVec3::new(0.0, 0.0, 0.0), DVec3::new(4.0, 0.0, 0.0), DVec3::new(1.0, 1.0, 0.0), DVec3::new(0.0, 4.0, 0.0)];
        let tris = triangulate_ring(&pts, &[0, 1, 2, 3]);
        assert_eq!(tris.len(), 2);
        let area: f64 = tris.iter().map(|t| 0.5 * (pts[t[1] as usize] - pts[t[0] as usize]).cross(pts[t[2] as usize] - pts[t[0] as usize]).z).sum();
        // polygon area by shoelace
        let shoelace = 0.5 * (0.0 * 0.0 - 4.0 * 0.0 + 4.0 * 1.0 - 1.0 * 0.0 + 1.0 * 4.0 - 0.0 * 1.0 + 0.0 * 0.0 - 0.0 * 4.0);
        assert!((area - shoelace).abs() < 1e-12);
        for t in &tris {
            assert!((pts[t[1] as usize] - pts[t[0] as usize]).cross(pts[t[2] as usize] - pts[t[0] as usize]).z > 0.0);
        }
    }
}

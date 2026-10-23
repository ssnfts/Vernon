//! Operations on patch sets that several nodes share: seam detection between
//! patch boundaries (gap and tangent-plane angle) and basic bookkeeping.

use glam::DVec3;

use crate::nurbs::{NurbsCurve, NurbsSurface, PatchSet};

/// A pair of patch boundaries that lie within the sewing tolerance.
#[derive(Debug, Clone)]
pub struct Seam {
    pub a: usize,
    pub side_a: usize,
    pub b: usize,
    pub side_b: usize,
    /// Largest distance between the two boundary curves (both directions).
    pub max_gap: f64,
    /// Largest angle between the surface normals across the seam, in degrees,
    /// ignoring the normal direction (tangent-plane angle).
    pub max_g1_deg: f64,
    /// The two patches run this border in the same direction (inconsistent orientation).
    pub opposed: bool,
}

/// Result of a seam search.
#[derive(Debug, Clone, Default)]
pub struct SeamReport {
    pub seams: Vec<Seam>,
    /// Boundaries (patch, side) that matched nothing.
    pub free_edges: Vec<(usize, usize)>,
    pub total_edges: usize,
}

impl SeamReport {
    pub fn max_gap(&self) -> f64 {
        self.seams.iter().map(|s| s.max_gap).fold(0.0, f64::max)
    }

    pub fn max_g1_deg(&self) -> f64 {
        self.seams.iter().map(|s| s.max_g1_deg).fold(0.0, f64::max)
    }

    /// Every boundary matched exactly one other boundary.
    pub fn is_closed(&self) -> bool {
        self.free_edges.is_empty() && self.total_edges > 0
    }
}

struct Edge {
    patch: usize,
    side: usize,
    curve: NurbsCurve,
    lo: DVec3,
    hi: DVec3,
    samples: Vec<DVec3>,
    degenerate: bool,
}

/// Domain parameters of the surface point at curve parameter `t` of `side`.
pub fn side_param(s: &NurbsSurface, side: usize, t: f64) -> (f64, f64) {
    let (ua, ub) = s.domain_u();
    let (va, vb) = s.domain_v();
    match side % 4 {
        0 => (t, va),
        1 => (ub, t),
        2 => (t, vb),
        _ => (ua, t),
    }
}

fn angle_unsigned_deg(a: DVec3, b: DVec3) -> f64 {
    if a.length_squared() < 1e-24 || b.length_squared() < 1e-24 {
        return 0.0;
    }
    a.normalize().dot(b.normalize()).abs().clamp(0.0, 1.0).acos().to_degrees()
}

/// Match patch boundaries within `tol` (mm). Every sample on a boundary is
/// looked up against the boundaries of the *other* patches, so a long boundary
/// facing several short ones (packed against unpacked patches) still counts as
/// covered. Gap and normal angle are measured at `samples + 1` points.
pub fn find_seams(ps: &PatchSet, tol: f64, samples: usize) -> SeamReport {
    let samples = samples.max(2);
    let mut edges: Vec<Edge> = Vec::new();
    for p in &ps.patches {
        for side in 0..4 {
            let curve = p.surface.boundary_curve(side);
            let (t0, t1) = curve.domain();
            let pts: Vec<DVec3> = (0..=samples).map(|k| curve.point(t0 + (t1 - t0) * k as f64 / samples as f64)).collect();
            let mut lo = DVec3::splat(f64::INFINITY);
            let mut hi = DVec3::splat(f64::NEG_INFINITY);
            for q in curve.control_points.iter().chain(pts.iter()) {
                lo = lo.min(*q);
                hi = hi.max(*q);
            }
            let len: f64 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
            edges.push(Edge { patch: p.id, side, curve, lo: lo - DVec3::splat(tol), hi: hi + DVec3::splat(tol), samples: pts, degenerate: len < tol });
        }
    }
    // uniform grid over edge boxes
    let cell = {
        let mut ext: Vec<f64> = edges.iter().filter(|e| !e.degenerate).map(|e| (e.hi - e.lo).max_element()).collect();
        ext.sort_by(|a, b| a.total_cmp(b));
        ext.get(ext.len() / 2).copied().unwrap_or(1.0).max(4.0 * tol).max(1e-9)
    };
    let key = |p: DVec3| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64, (p.z / cell).floor() as i64);
    let mut grid: std::collections::HashMap<(i64, i64, i64), Vec<usize>> = std::collections::HashMap::new();
    for (i, e) in edges.iter().enumerate() {
        if e.degenerate {
            continue;
        }
        let (k0, k1) = (key(e.lo), key(e.hi));
        for x in k0.0..=k1.0 {
            for y in k0.1..=k1.1 {
                for z in k0.2..=k1.2 {
                    grid.entry((x, y, z)).or_default().push(i);
                }
            }
        }
    }
    let mut rep = SeamReport { total_edges: edges.iter().filter(|e| !e.degenerate).count(), ..Default::default() };
    let mut pair_gap: std::collections::BTreeMap<(usize, usize), (f64, f64, usize, usize)> = std::collections::BTreeMap::new();
    for (i, e) in edges.iter().enumerate() {
        if e.degenerate {
            continue;
        }
        let mut free = false;
        for &q in &e.samples {
            let mut best: Option<(usize, f64, f64)> = None;
            if let Some(list) = grid.get(&key(q)) {
                for &j in list {
                    let f = &edges[j];
                    if f.patch == e.patch || q.cmplt(f.lo).any() || q.cmpgt(f.hi).any() {
                        continue;
                    }
                    let (tf, _, d) = f.curve.project(q);
                    if d <= tol && best.is_none_or(|b| d < b.1) {
                        best = Some((j, d, tf));
                    }
                }
            }
            match best {
                None => free = true,
                Some((j, d, tf)) => {
                    let (ta, _, _) = e.curve.project(q);
                    let sa = &ps.patches[e.patch].surface;
                    let sb = &ps.patches[edges[j].patch].surface;
                    let (ua, va) = side_param(sa, e.side, ta);
                    let (ub, vb) = side_param(sb, edges[j].side, tf);
                    let g1 = angle_unsigned_deg(sa.normal(ua, va), sb.normal(ub, vb));
                    let slot = pair_gap.entry((i.min(j), i.max(j))).or_insert((0.0, 0.0, 0, 0));
                    slot.0 = slot.0.max(d);
                    slot.1 = slot.1.max(g1);
                    slot.2 += 1;
                    // consistently oriented patches run a shared border in opposite directions
                    let dir = |side: usize| if side < 2 { 1.0 } else { -1.0 };
                    let (da, db) = (e.curve.derivative(ta) * dir(e.side), edges[j].curve.derivative(tf) * dir(edges[j].side));
                    if da.dot(db) > 0.0 {
                        slot.3 += 1;
                    }
                }
            }
        }
        if free {
            rep.free_edges.push((e.patch, e.side));
        }
    }
    for ((i, j), (gap, g1, count, opp)) in pair_gap {
        // a boundary that only touches another at a corner is not a seam
        if count < 3 {
            continue;
        }
        rep.seams.push(Seam { a: edges[i].patch, side_a: edges[i].side, b: edges[j].patch, side_b: edges[j].side, max_gap: gap, max_g1_deg: g1, opposed: opp * 2 > count });
    }
    rep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(x0: f64, z: impl Fn(f64) -> f64) -> NurbsSurface {
        let mut pts = Vec::new();
        for i in 0..4 {
            for j in 0..4 {
                let x = x0 + 10.0 * i as f64 / 3.0;
                pts.push(DVec3::new(x, 10.0 * j as f64 / 3.0, z(x)));
            }
        }
        NurbsSurface::clamped_uniform(3, 3, 4, 4, pts)
    }

    #[test]
    fn adjacent_sheets_form_one_seam_with_zero_angle_when_coplanar() {
        let ps = PatchSet::new(vec![sheet(0.0, |_| 0.0), sheet(10.0, |_| 0.0)]);
        let rep = find_seams(&ps, 0.01, 8);
        assert_eq!(rep.seams.len(), 1);
        assert!(rep.max_gap() < 1e-9);
        assert!(rep.max_g1_deg() < 1e-6);
        assert_eq!(rep.free_edges.len(), 6);
        assert!(!rep.is_closed());
    }

    #[test]
    fn a_kink_is_measured_as_an_angle() {
        let ps = PatchSet::new(vec![sheet(0.0, |_| 0.0), sheet(10.0, |x| (x - 10.0).abs())]);
        let rep = find_seams(&ps, 0.01, 8);
        assert_eq!(rep.seams.len(), 1);
        assert!((rep.max_g1_deg() - 45.0).abs() < 1e-6, "{}", rep.max_g1_deg());
    }

    #[test]
    fn a_gap_larger_than_the_tolerance_is_not_sewn() {
        let ps = PatchSet::new(vec![sheet(0.0, |_| 0.0), sheet(10.5, |_| 0.0)]);
        let rep = find_seams(&ps, 0.01, 8);
        assert!(rep.seams.is_empty());
        assert_eq!(rep.free_edges.len(), 8);
    }
}

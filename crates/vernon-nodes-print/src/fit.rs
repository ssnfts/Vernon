//! Build-volume fit (spec A4): try candidate build directions and z-spins,
//! keep the poses that fit the usable volume, rank them with the orientation
//! score and write the pose as a 4x4 matrix.

use glam::{DMat3, DMat4, DVec2, DVec3};

use crate::orient::{self, OrientParams, Search, Tris};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Minimum corner at (margin, margin), bottom at z = 0.
    Corner,
    /// Centred on the bed, bottom at z = 0.
    Centre,
}

#[derive(Debug, Clone)]
pub struct FitOptions {
    pub bed: DVec3,
    pub margin: f64,
    pub search: Search,
    pub z_spin: bool,
    pub allow_scaling: bool,
    pub min_scale: f64,
    pub placement: Placement,
    pub orient: OrientParams,
}

impl FitOptions {
    pub fn usable(&self) -> DVec3 {
        self.bed - DVec3::splat(2.0 * self.margin)
    }
}

#[derive(Debug, Clone)]
pub struct FitResult {
    pub up: DVec3,
    /// Spin about +Z in radians, applied after the up rotation.
    pub spin: f64,
    pub scale: f64,
    /// Extent of the placed part (after scaling) along x, y, z.
    pub extent: DVec3,
    /// min over axes of usable - extent (negative when the part does not fit).
    pub margin: f64,
    pub fits: bool,
    pub matrix: DMat4,
    pub bbox_min: DVec3,
    pub bbox_max: DVec3,
    pub score: f64,
    pub candidates: usize,
}

/// Convex hull (counter-clockwise, no repeated first point).
pub fn convex_hull(mut pts: Vec<DVec2>) -> Vec<DVec2> {
    pts.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal).then(a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: DVec2, a: DVec2, b: DVec2| (a - o).perp_dot(b - o);
    let mut h: Vec<DVec2> = Vec::with_capacity(pts.len() * 2);
    for &p in &pts {
        while h.len() >= 2 && cross(h[h.len() - 2], h[h.len() - 1], p) <= 0.0 {
            h.pop();
        }
        h.push(p);
    }
    let lower = h.len() + 1;
    for &p in pts.iter().rev().skip(1) {
        while h.len() >= lower && cross(h[h.len() - 2], h[h.len() - 1], p) <= 0.0 {
            h.pop();
        }
        h.push(p);
    }
    h.pop();
    h
}

/// The 24 proper rotations that map the axes onto the axes.
pub fn axis_rotations() -> Vec<DMat3> {
    let perms: [[usize; 3]; 6] = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
    let mut out = Vec::new();
    for perm in perms {
        for s in 0..8u8 {
            let sg = [if s & 1 == 0 { 1.0 } else { -1.0 }, if s & 2 == 0 { 1.0 } else { -1.0 }, if s & 4 == 0 { 1.0 } else { -1.0 }];
            let mut cols = [DVec3::ZERO; 3];
            for (i, &p) in perm.iter().enumerate() {
                let mut c = DVec3::ZERO;
                c[p] = sg[i];
                cols[i] = c;
            }
            let m = DMat3::from_cols(cols[0], cols[1], cols[2]);
            if m.determinant() > 0.0 {
                out.push(m);
            }
        }
    }
    out
}

struct Spin {
    angle: f64,
    w: f64,
    h: f64,
}

fn spins(xy: &[DVec2], z_spin: bool) -> Vec<Spin> {
    let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
    for &p in xy {
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let (w, h) = (hi.x - lo.x, hi.y - lo.y);
    let mut out = vec![Spin { angle: 0.0, w, h }, Spin { angle: std::f64::consts::FRAC_PI_2, w: h, h: w }];
    if !z_spin {
        return out;
    }
    let hull = convex_hull(xy.to_vec());
    let n = hull.len();
    if n < 3 {
        return out;
    }
    // Sweep: the best bed fit of a long part is often diagonal, not edge-aligned.
    for k in 1..45 {
        let a = (2.0 * k as f64).to_radians();
        let (c, s) = (a.cos(), a.sin());
        let (mut x0, mut x1, mut y0, mut y1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
        for &p in &hull {
            let (x, y) = (c * p.x - s * p.y, s * p.x + c * p.y);
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
        out.push(Spin { angle: a, w: x1 - x0, h: y1 - y0 });
        out.push(Spin { angle: a + std::f64::consts::FRAC_PI_2, w: y1 - y0, h: x1 - x0 });
    }
    // Longest hull edges first: they carry the minimum-area rectangles.
    let mut edges: Vec<(f64, usize)> = (0..n).map(|i| ((hull[(i + 1) % n] - hull[i]).length(), i)).collect();
    edges.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    for &(len, i) in edges.iter().take(48) {
        if len < 1e-12 {
            continue;
        }
        let d = (hull[(i + 1) % n] - hull[i]) / len;
        let q = DVec2::new(-d.y, d.x);
        let (mut a0, mut a1, mut b0, mut b1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
        for &p in &hull {
            let (s, t) = (p.dot(d), p.dot(q));
            a0 = a0.min(s);
            a1 = a1.max(s);
            b0 = b0.min(t);
            b1 = b1.max(t);
        }
        let (ew, eh) = (a1 - a0, b1 - b0);
        let alpha = d.y.atan2(d.x);
        out.push(Spin { angle: -alpha, w: ew, h: eh });
        out.push(Spin { angle: -alpha + std::f64::consts::FRAC_PI_2, w: eh, h: ew });
    }
    out
}

struct Cand {
    up: DVec3,
    spin: f64,
    scale: f64,
    margin: f64,
}

/// Fit one part. `points` are the part's vertex positions; `tris` its triangles.
pub fn fit_part(points: &[DVec3], tris: &Tris, o: &FitOptions) -> Option<FitResult> {
    if points.is_empty() {
        return None;
    }
    let usable = o.usable();
    let ups = orient::candidate_ups(tris, o.search);
    let mut cands: Vec<Cand> = Vec::new();
    for &up in &ups {
        let r = orient::rotation_to_z(up);
        let rotated: Vec<DVec3> = points.iter().map(|&p| r * p).collect();
        let (mut zlo, mut zhi) = (f64::INFINITY, f64::NEG_INFINITY);
        for p in &rotated {
            zlo = zlo.min(p.z);
            zhi = zhi.max(p.z);
        }
        let height = zhi - zlo;
        let xy: Vec<DVec2> = rotated.iter().map(|p| DVec2::new(p.x, p.y)).collect();
        // Among spins that fit take the most compact bounding rectangle (so a box
        // stays axis-aligned when it can); otherwise the best margin, or with
        // scaling the best scale.
        let mut best: Option<(f64, f64, f64, f64)> = None; // margin, spin, scale, area
        for s in spins(&xy, o.z_spin) {
            let margin = (usable.x - s.w).min(usable.y - s.h).min(usable.z - height);
            let scale = (usable.x / s.w.max(1e-12)).min(usable.y / s.h.max(1e-12)).min(usable.z / height.max(1e-12));
            let area = s.w * s.h;
            let better = match best {
                None => true,
                Some((bm, _, bs, ba)) => {
                    if margin >= -1e-9 || bm >= -1e-9 {
                        // fitting beats not fitting; among fitting, smaller area wins
                        if margin >= -1e-9 && bm >= -1e-9 {
                            area < ba - 1e-9
                        } else {
                            margin >= -1e-9
                        }
                    } else if o.allow_scaling {
                        scale > bs + 1e-9
                    } else {
                        margin > bm + 1e-9
                    }
                }
            };
            if better {
                best = Some((margin, s.angle, scale, area));
            }
        }
        if let Some((margin, spin, scale, _)) = best {
            cands.push(Cand { up, spin, scale, margin });
        }
    }
    if cands.is_empty() {
        return None;
    }
    let diag = tris.diagonal();
    let fitting: Vec<usize> = (0..cands.len()).filter(|&i| cands[i].margin >= -1e-9).collect();
    let (pick, scale, fits) = if !fitting.is_empty() {
        let mut best = fitting[0];
        let mut best_score = f64::INFINITY;
        for &i in &fitting {
            let s = orient::score(tris, diag, cands[i].up, &o.orient).total;
            if s < best_score - 1e-12 {
                best_score = s;
                best = i;
            }
        }
        (best, 1.0, true)
    } else if o.allow_scaling {
        let (mut best, mut bs) = (0usize, f64::NEG_INFINITY);
        for (i, c) in cands.iter().enumerate() {
            if c.scale > bs + 1e-12 {
                bs = c.scale;
                best = i;
            }
        }
        if bs >= o.min_scale {
            (best, bs, true)
        } else {
            (best, 1.0, false)
        }
    } else {
        let mut best = 0usize;
        for (i, c) in cands.iter().enumerate() {
            if c.margin > cands[best].margin + 1e-12 {
                best = i;
            }
        }
        (best, 1.0, false)
    };
    let c = &cands[pick];
    let rot = DMat3::from_rotation_z(c.spin) * orient::rotation_to_z(c.up);
    let lin = DMat4::from_mat3(rot * scale);
    let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
    for &p in points {
        let q = lin.transform_point3(p);
        lo = lo.min(q);
        hi = hi.max(q);
    }
    let extent = hi - lo;
    let shift = match o.placement {
        Placement::Corner => DVec3::new(o.margin - lo.x, o.margin - lo.y, -lo.z),
        Placement::Centre => DVec3::new(o.bed.x * 0.5 - (lo.x + hi.x) * 0.5, o.bed.y * 0.5 - (lo.y + hi.y) * 0.5, -lo.z),
    };
    let matrix = DMat4::from_translation(shift) * lin;
    let margin = (usable.x - extent.x).min(usable.y - extent.y).min(usable.z - extent.z);
    Some(FitResult {
        up: c.up,
        spin: c.spin,
        scale,
        extent,
        margin,
        fits,
        matrix,
        bbox_min: lo + shift,
        bbox_max: hi + shift,
        score: orient::score(tris, diag, c.up, &o.orient).total,
        candidates: cands.len(),
    })
}

/// Row-major 4x4 (acting on column vectors) as 16 numbers.
pub fn matrix_rows(m: &DMat4) -> Vec<f64> {
    let c = m.to_cols_array_2d();
    (0..4).flat_map(|r| (0..4).map(move |k| c[k][r])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::testutil::tri_box;
    use crate::orient::DEFAULT_WEIGHTS;

    fn opts(bed: DVec3, margin: f64) -> FitOptions {
        FitOptions {
            bed,
            margin,
            search: Search::Faces,
            z_spin: true,
            allow_scaling: false,
            min_scale: 0.5,
            placement: Placement::Centre,
            orient: OrientParams { layer_h: 0.2, theta_o_deg: 45.0, loads: vec![], k: 1.0, weights: DEFAULT_WEIGHTS },
        }
    }

    fn fit(size: DVec3, o: &FitOptions) -> FitResult {
        let g = tri_box(DVec3::new(5.0, -3.0, 7.0), size);
        let tris = Tris::new(&g);
        fit_part(&g.points, &tris, o).unwrap()
    }

    #[test]
    fn rotations_are_the_24_proper_ones() {
        let r = axis_rotations();
        assert_eq!(r.len(), 24);
        for i in 0..r.len() {
            assert!((r[i].determinant() - 1.0).abs() < 1e-12);
            for j in 0..i {
                assert!(!(r[i] - r[j]).abs_diff_eq(DMat3::ZERO, 1e-9));
            }
        }
    }

    #[test]
    fn long_part_fails_low_bed_and_fits_tall_bed() {
        let size = DVec3::new(120.0, 30.0, 20.0);
        let low = fit(size, &opts(DVec3::splat(100.0), 1.0));
        assert!(!low.fits && low.margin < 0.0);
        let tall = fit(size, &opts(DVec3::new(100.0, 100.0, 150.0), 1.0));
        assert!(tall.fits, "{tall:?}");
        assert!((tall.extent.z - 120.0).abs() < 1e-9);
    }

    #[test]
    fn short_axis_goes_up_when_it_fits() {
        let r = fit(DVec3::new(50.0, 20.0, 80.0), &opts(DVec3::splat(150.0), 2.0));
        assert!(r.fits);
        assert!(r.extent.z <= 80.0 + 1e-9);
        assert!(r.extent.x <= 146.0 && r.extent.y <= 146.0);
        // lowest score at equal fit is the flattest stance (lowest height)
        assert!((r.extent.z - 20.0).abs() < 1e-9, "{:?}", r.extent);
    }

    #[test]
    fn matrix_reproduces_placement_and_bed_pose() {
        let size = DVec3::new(60.0, 40.0, 10.0);
        let g = tri_box(DVec3::new(5.0, -3.0, 7.0), size);
        let tris = Tris::new(&g);
        let mut o = opts(DVec3::splat(200.0), 2.0);
        o.placement = Placement::Corner;
        let r = fit_part(&g.points, &tris, &o).unwrap();
        let mut moved = g.clone();
        moved.transform(r.matrix);
        let b = moved.bounds();
        assert!((b.min - r.bbox_min).length() < 1e-9 && (b.max - r.bbox_max).length() < 1e-9);
        assert!((b.min.z).abs() < 1e-9);
        assert!((b.min.x - 2.0).abs() < 1e-9 && (b.min.y - 2.0).abs() < 1e-9);
        let rows = matrix_rows(&r.matrix);
        assert_eq!(rows.len(), 16);
        assert_eq!(&rows[12..], &[0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn unscaled_fit_beats_scaled_and_scaling_only_when_allowed() {
        let size = DVec3::new(180.0, 100.0, 50.0);
        let mut o = opts(DVec3::new(200.0, 200.0, 200.0), 0.0);
        let r = fit(size, &o);
        assert!(r.fits && r.scale == 1.0);
        let big = DVec3::new(400.0, 300.0, 250.0);
        let r = fit(big, &o);
        assert!(!r.fits && r.scale == 1.0);
        o.allow_scaling = true;
        let r = fit(big, &o);
        assert!(r.fits && r.scale < 1.0 && r.scale >= 0.5, "{}", r.scale);
        assert!(r.margin > -1e-6);
    }

    #[test]
    fn spin_helps_a_diagonal_plate() {
        // 120 x 10 plate on a 100 x 100 bed only fits along the diagonal.
        let size = DVec3::new(120.0, 10.0, 2.0);
        let mut o = opts(DVec3::new(100.0, 100.0, 100.0), 0.0);
        o.z_spin = false;
        assert!(!fit(size, &o).fits);
        o.z_spin = true;
        let r = fit(size, &o);
        assert!(r.fits, "{r:?}");
        assert!(r.extent.x <= 100.0 + 1e-9 && r.extent.y <= 100.0 + 1e-9);
    }

    #[test]
    fn axis_search_matches_exhaustive_24_rotation_margin() {
        let size = DVec3::new(70.0, 30.0, 110.0);
        let bed = DVec3::new(90.0, 60.0, 120.0);
        let mut o = opts(bed, 0.0);
        o.search = Search::Axis;
        o.z_spin = false;
        let best = axis_rotations()
            .iter()
            .map(|r| {
                let e = (*r * size).abs();
                (bed.x - e.x).min(bed.y - e.y).min(bed.z - e.z)
            })
            .fold(f64::NEG_INFINITY, f64::max);
        let tall = DVec3::new(70.0, 30.0, 110.0);
        let f = fit(tall, &o);
        assert!(f.fits && best >= 0.0);
        assert!(f.margin <= best + 1e-9);
    }

    #[test]
    fn hull_of_square_with_interior_points() {
        let pts = vec![DVec2::new(0.0, 0.0), DVec2::new(2.0, 0.0), DVec2::new(2.0, 2.0), DVec2::new(0.0, 2.0), DVec2::new(1.0, 1.0)];
        assert_eq!(convex_hull(pts).len(), 4);
    }
}

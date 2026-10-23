//! Closest-point queries on a triangle soup (uniform grid), used to map
//! surface fields (pressure coefficient, surface temperature) onto the
//! solid mesh.

use glam::DVec3;

/// Closest point on triangle `abc` to `p` with its barycentric weights.
pub fn closest_point_on_triangle(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> (DVec3, [f64; 3]) {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, [1.0, 0.0, 0.0]);
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return (b, [0.0, 1.0, 0.0]);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (a + ab * v, [1.0 - v, v, 0.0]);
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return (c, [0.0, 0.0, 1.0]);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (a + ac * w, [1.0 - w, 0.0, w]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (b + (c - b) * w, [0.0, 1.0 - w, w]);
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (a + ab * v + ac * w, [1.0 - v - w, v, w])
}

/// Result of a nearest-triangle query.
#[derive(Debug, Clone, Copy)]
pub struct Nearest {
    pub tri: usize,
    pub point: DVec3,
    pub bary: [f64; 3],
    pub dist: f64,
}

/// Uniform grid over a triangle list.
pub struct TriGrid {
    tris: Vec<[DVec3; 3]>,
    lo: DVec3,
    cell: f64,
    dims: [usize; 3],
    cells: Vec<Vec<u32>>,
}

impl TriGrid {
    pub fn new(tris: Vec<[DVec3; 3]>) -> Self {
        let mut lo = DVec3::splat(f64::INFINITY);
        let mut hi = DVec3::splat(f64::NEG_INFINITY);
        for t in &tris {
            for p in t {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
        }
        if tris.is_empty() {
            return Self { tris, lo: DVec3::ZERO, cell: 1.0, dims: [1, 1, 1], cells: vec![Vec::new()] };
        }
        let size = (hi - lo).max(DVec3::splat(1e-9));
        let target = (tris.len() * 2).clamp(1, 1_000_000) as f64;
        let cell = (size.x * size.y * size.z / target).cbrt().max(size.max_element() / 200.0).max(1e-9);
        let dims = [0, 1, 2].map(|a| ((size[a] / cell).ceil() as usize).max(1));
        let mut cells: Vec<Vec<u32>> = vec![Vec::new(); dims[0] * dims[1] * dims[2]];
        for (ti, t) in tris.iter().enumerate() {
            let tlo = t[0].min(t[1]).min(t[2]);
            let thi = t[0].max(t[1]).max(t[2]);
            let i0 = Self::cell_of(lo, cell, dims, tlo);
            let i1 = Self::cell_of(lo, cell, dims, thi);
            for i in i0[0]..=i1[0] {
                for j in i0[1]..=i1[1] {
                    for k in i0[2]..=i1[2] {
                        cells[(i * dims[1] + j) * dims[2] + k].push(ti as u32);
                    }
                }
            }
        }
        Self { tris, lo, cell, dims, cells }
    }

    fn cell_of(lo: DVec3, cell: f64, dims: [usize; 3], p: DVec3) -> [usize; 3] {
        [0, 1, 2].map(|a| (((p[a] - lo[a]) / cell).floor().max(0.0) as usize).min(dims[a] - 1))
    }

    pub fn len(&self) -> usize {
        self.tris.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    pub fn triangle(&self, i: usize) -> [DVec3; 3] {
        self.tris[i]
    }

    /// Nearest triangle to `p`, optionally limited to `max_dist`.
    pub fn nearest(&self, p: DVec3, max_dist: f64) -> Option<Nearest> {
        if self.tris.is_empty() {
            return None;
        }
        let c0 = Self::cell_of(self.lo, self.cell, self.dims, p);
        let max_ring = self.dims[0].max(self.dims[1]).max(self.dims[2]);
        let mut best: Option<Nearest> = None;
        for r in 0..=max_ring {
            let lim = |a: usize| -> (usize, usize) { (c0[a].saturating_sub(r), (c0[a] + r).min(self.dims[a] - 1)) };
            let (x0, x1) = lim(0);
            let (y0, y1) = lim(1);
            let (z0, z1) = lim(2);
            for i in x0..=x1 {
                for j in y0..=y1 {
                    for k in z0..=z1 {
                        let ring = i.abs_diff(c0[0]).max(j.abs_diff(c0[1])).max(k.abs_diff(c0[2]));
                        if ring != r {
                            continue;
                        }
                        for &ti in &self.cells[(i * self.dims[1] + j) * self.dims[2] + k] {
                            let t = &self.tris[ti as usize];
                            let (q, bary) = closest_point_on_triangle(p, t[0], t[1], t[2]);
                            let d = (q - p).length();
                            if best.map_or(true, |b| d < b.dist) {
                                best = Some(Nearest { tri: ti as usize, point: q, bary, dist: d });
                            }
                        }
                    }
                }
            }
            if let Some(b) = best {
                if b.dist <= r as f64 * self.cell {
                    break;
                }
            }
            if r as f64 * self.cell > max_dist && best.is_none() {
                return None;
            }
        }
        best.filter(|b| b.dist <= max_dist)
    }
}

/// Ray / triangle intersection distance (Moller-Trumbore), `None` if it misses.
pub fn ray_triangle(o: DVec3, d: DVec3, t: &[DVec3; 3]) -> Option<f64> {
    let e1 = t[1] - t[0];
    let e2 = t[2] - t[0];
    let h = d.cross(e2);
    let a = e1.dot(h);
    if a.abs() < 1e-14 {
        return None;
    }
    let f = 1.0 / a;
    let s = o - t[0];
    let u = f * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = f * d.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let dist = f * e2.dot(q);
    (dist > 1e-9).then_some(dist)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri() -> [DVec3; 3] {
        [DVec3::ZERO, DVec3::new(10.0, 0.0, 0.0), DVec3::new(0.0, 10.0, 0.0)]
    }

    #[test]
    fn closest_point_regions() {
        let t = tri();
        let (q, w) = closest_point_on_triangle(DVec3::new(2.0, 2.0, 5.0), t[0], t[1], t[2]);
        assert!((q - DVec3::new(2.0, 2.0, 0.0)).length() < 1e-12);
        assert!((w[0] - 0.6).abs() < 1e-12 && (w[1] - 0.2).abs() < 1e-12);
        let (q, _) = closest_point_on_triangle(DVec3::new(-5.0, -5.0, 0.0), t[0], t[1], t[2]);
        assert!((q - DVec3::ZERO).length() < 1e-12);
        let (q, _) = closest_point_on_triangle(DVec3::new(8.0, 8.0, 0.0), t[0], t[1], t[2]);
        assert!((q - DVec3::new(5.0, 5.0, 0.0)).length() < 1e-12);
    }

    #[test]
    fn grid_matches_brute_force() {
        let mut tris = Vec::new();
        for i in 0..20 {
            for j in 0..20 {
                let o = DVec3::new(i as f64 * 3.0, j as f64 * 3.0, ((i * j) % 5) as f64);
                tris.push([o, o + DVec3::new(3.0, 0.0, 0.5), o + DVec3::new(0.0, 3.0, 0.2)]);
            }
        }
        let g = TriGrid::new(tris.clone());
        for p in [DVec3::new(10.1, 20.3, 7.0), DVec3::new(-30.0, 5.0, 2.0), DVec3::new(59.0, 59.0, -4.0)] {
            let n = g.nearest(p, f64::INFINITY).unwrap();
            let brute = tris
                .iter()
                .map(|t| (closest_point_on_triangle(p, t[0], t[1], t[2]).0 - p).length())
                .fold(f64::INFINITY, f64::min);
            assert!((n.dist - brute).abs() < 1e-9, "{p:?}: {} vs {brute}", n.dist);
        }
        assert!(g.nearest(DVec3::new(500.0, 500.0, 500.0), 10.0).is_none());
    }

    #[test]
    fn ray_hits_and_misses() {
        let t = tri();
        let d = ray_triangle(DVec3::new(2.0, 2.0, 5.0), DVec3::NEG_Z, &t).unwrap();
        assert!((d - 5.0).abs() < 1e-12);
        assert!(ray_triangle(DVec3::new(20.0, 2.0, 5.0), DVec3::NEG_Z, &t).is_none());
    }
}

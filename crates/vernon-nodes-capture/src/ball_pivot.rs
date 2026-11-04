//! Ball-pivoting surface reconstruction from an oriented point cloud.
//!
//! Method: F. Bernardini, J. Mittleman, H. Rushmeier, C. Silva, G. Taubin,
//! "The Ball-Pivoting Algorithm for Surface Reconstruction", IEEE TVCG 5(4),
//! 1999. A ball of radius `rho` rests on three points that contain no other
//! point; it then pivots about each front edge until it touches a fourth point,
//! which closes a new triangle. Seeds start new patches, edges whose reverse is
//! already present are glued (front joining), and the pass is repeated with
//! each larger radius so holes left by sparse sampling are filled.
//!
//! Triangles are wound so their normal agrees with the sum of the three vertex
//! normals. Directed edges are unique, so the output never contains an edge
//! used twice in the same direction.

use std::collections::{HashMap, HashSet};

use glam::DVec3;

use crate::spatial::Grid;

/// Summary of one reconstruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BpaStats {
    pub triangles: usize,
    /// Points that belong to no triangle.
    pub unused_points: usize,
    /// Closed boundary loops left in the mesh (a closed surface has 0).
    pub holes: usize,
}

const EPS_EMPTY: f64 = 1e-7;

struct Mesher<'a> {
    pts: &'a [DVec3],
    nrm: &'a [DVec3],
    tris: Vec<[u32; 3]>,
    /// directed edge -> triangle index
    de: HashMap<(u32, u32), u32>,
    nbr: Vec<Vec<u32>>,
    used: Vec<bool>,
    dead: HashSet<(u32, u32)>,
}

/// Front edge `i -> j` of a triangle whose third vertex is `o`, with the
/// centre of the ball that rests on that triangle.
#[derive(Clone, Copy)]
struct Front {
    i: u32,
    j: u32,
    o: u32,
    c: DVec3,
}

impl<'a> Mesher<'a> {
    fn nsum(&self, a: u32, b: u32, c: u32) -> DVec3 {
        self.nrm[a as usize] + self.nrm[b as usize] + self.nrm[c as usize]
    }

    /// Centre of the radius-`rho` ball through (a, b, c) on the side the
    /// triangle normal points to, if the triangle is wound along the vertex
    /// normals and fits the ball.
    fn ball_center(&self, a: u32, b: u32, c: u32, rho: f64) -> Option<DVec3> {
        let (pa, pb, pc) = (self.pts[a as usize], self.pts[b as usize], self.pts[c as usize]);
        let ab = pb - pa;
        let ac = pc - pa;
        let nn = ab.cross(ac);
        let nn2 = nn.length_squared();
        if nn2 < 1e-24 {
            return None;
        }
        let n = nn / nn2.sqrt();
        if n.dot(self.nsum(a, b, c)) <= 0.0 {
            return None;
        }
        let cc = pa + (nn.cross(ab) * ac.length_squared() + ac.cross(nn) * ab.length_squared()) / (2.0 * nn2);
        let r2 = (cc - pa).length_squared();
        if r2 > rho * rho {
            return None;
        }
        Some(cc + n * (rho * rho - r2).sqrt())
    }

    fn ball_empty(&self, grid: &Grid, c: DVec3, rho: f64, skip: [u32; 3]) -> bool {
        let mut out = Vec::new();
        grid.radius_query(self.pts, c, rho, &mut out);
        let lim = rho * (1.0 - EPS_EMPTY);
        out.iter().all(|&q| skip.contains(&q) || self.pts[q as usize].distance(c) >= lim)
    }

    fn is_front(&self, a: u32, b: u32) -> bool {
        self.de.contains_key(&(a, b)) && !self.de.contains_key(&(b, a))
    }

    /// A vertex may join a new triangle only when it is unused or on the front.
    fn on_front(&self, v: u32) -> bool {
        !self.used[v as usize] || self.nbr[v as usize].iter().any(|&w| self.is_front(v, w) || self.is_front(w, v))
    }

    fn add(&mut self, t: [u32; 3]) {
        let id = self.tris.len() as u32;
        self.tris.push(t);
        for e in 0..3 {
            let (a, b) = (t[e], t[(e + 1) % 3]);
            self.de.insert((a, b), id);
            self.nbr[a as usize].push(b);
            self.nbr[b as usize].push(a);
            self.used[a as usize] = true;
        }
    }

    /// Pivot the ball about front edge `f`; returns the new vertex and the new ball centre.
    fn pivot(&self, grid: &Grid, f: &Front, rho: f64) -> Option<(u32, DVec3)> {
        let (pi, pj) = (self.pts[f.i as usize], self.pts[f.j as usize]);
        let m = 0.5 * (pi + pj);
        let e = (pj - pi).normalize_or_zero();
        if e == DVec3::ZERO {
            return None;
        }
        let v0 = f.c - m;
        let v0 = v0 - e * e.dot(v0);
        let mut cand = Vec::new();
        grid.radius_query(self.pts, m, 2.0 * rho, &mut cand);
        let mut scored: Vec<(f64, u32, DVec3)> = Vec::new();
        for &k in &cand {
            if k == f.i || k == f.j || k == f.o {
                continue;
            }
            // new triangle across the edge is wound (j, i, k)
            let Some(c) = self.ball_center(f.j, f.i, k, rho) else { continue };
            let vk = c - m;
            let vk = vk - e * e.dot(vk);
            let mut th = e.dot(v0.cross(vk)).atan2(v0.dot(vk));
            if th < 1e-9 {
                th += std::f64::consts::TAU;
            }
            scored.push((th, k, c));
        }
        scored.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (_, k, c) in scored {
            if !self.on_front(k) {
                continue;
            }
            if self.de.contains_key(&(f.i, k)) || self.de.contains_key(&(k, f.j)) {
                continue; // would reuse a directed edge
            }
            if self.ball_empty(grid, c, rho, [f.i, f.j, k]) {
                return Some((k, c));
            }
        }
        None
    }

    fn expand(&mut self, grid: &Grid, mut stack: Vec<Front>, rho: f64) {
        while let Some(f) = stack.pop() {
            if !self.is_front(f.i, f.j) || self.dead.contains(&(f.i, f.j)) {
                continue;
            }
            match self.pivot(grid, &f, rho) {
                Some((k, c)) => {
                    self.add([f.j, f.i, k]);
                    stack.push(Front { i: f.i, j: k, o: f.j, c });
                    stack.push(Front { i: k, j: f.j, o: f.i, c });
                }
                None => {
                    self.dead.insert((f.i, f.j));
                }
            }
        }
    }

    fn find_seed(&mut self, grid: &Grid, rho: f64) -> Option<Vec<Front>> {
        for p in 0..self.pts.len() as u32 {
            if self.used[p as usize] {
                continue;
            }
            let pp = self.pts[p as usize];
            let mut near = Vec::new();
            grid.radius_query(self.pts, pp, 2.0 * rho, &mut near);
            near.retain(|&q| q != p && !self.used[q as usize]);
            near.sort_by(|&a, &b| {
                let (da, db) = (self.pts[a as usize].distance_squared(pp), self.pts[b as usize].distance_squared(pp));
                da.total_cmp(&db).then(a.cmp(&b))
            });
            near.truncate(16);
            for x in 0..near.len() {
                for y in x + 1..near.len() {
                    let (q, r) = (near[x], near[y]);
                    for t in [[p, q, r], [p, r, q]] {
                        let Some(c) = self.ball_center(t[0], t[1], t[2], rho) else { continue };
                        if !self.ball_empty(grid, c, rho, t) {
                            continue;
                        }
                        self.add(t);
                        return Some(
                            (0..3).map(|e| Front { i: t[e], j: t[(e + 1) % 3], o: t[(e + 2) % 3], c }).collect(),
                        );
                    }
                }
            }
        }
        None
    }

    fn boundary_fronts(&self, rho: f64) -> Vec<Front> {
        let mut keys: Vec<(u32, u32)> = self.de.keys().copied().filter(|&(a, b)| self.is_front(a, b)).collect();
        keys.sort_unstable();
        let mut v = Vec::new();
        for (a, b) in keys {
            let t = self.tris[self.de[&(a, b)] as usize];
            let o = *t.iter().find(|&&x| x != a && x != b).unwrap();
            if let Some(c) = self.ball_center(a, b, o, rho) {
                v.push(Front { i: a, j: b, o, c });
            }
        }
        v
    }

    fn hole_count(&self) -> usize {
        let mut parent: HashMap<u32, u32> = HashMap::new();
        fn find(p: &mut HashMap<u32, u32>, x: u32) -> u32 {
            let px = *p.entry(x).or_insert(x);
            if px == x {
                return x;
            }
            let r = find(p, px);
            p.insert(x, r);
            r
        }
        for &(a, b) in self.de.keys() {
            if self.is_front(a, b) {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                if ra != rb {
                    parent.insert(ra, rb);
                }
            }
        }
        let keys: Vec<u32> = parent.keys().copied().collect();
        let roots: HashSet<u32> = keys.into_iter().map(|k| find(&mut parent, k)).collect();
        roots.len()
    }
}

/// Reconstruct triangles from `points` with unit `normals` using ball radii
/// `radii` (mm, tried in ascending order). Slices must have equal length.
pub fn ball_pivot(points: &[[f64; 3]], normals: &[[f64; 3]], radii: &[f64]) -> (Vec<[u32; 3]>, BpaStats) {
    let n = points.len().min(normals.len());
    let pts: Vec<DVec3> = points[..n].iter().map(|p| DVec3::from_array(*p)).collect();
    let nrm: Vec<DVec3> = normals[..n].iter().map(|p| DVec3::from_array(*p).normalize_or_zero()).collect();
    let mut m = Mesher {
        pts: &pts,
        nrm: &nrm,
        tris: Vec::new(),
        de: HashMap::new(),
        nbr: vec![Vec::new(); n],
        used: vec![false; n],
        dead: HashSet::new(),
    };
    let mut rs: Vec<f64> = radii.iter().copied().filter(|r| r.is_finite() && *r > 0.0).collect();
    rs.sort_by(f64::total_cmp);
    for rho in rs {
        if n < 3 {
            break;
        }
        let grid = Grid::build(&pts, 2.0 * rho);
        m.dead.clear();
        let fronts = m.boundary_fronts(rho);
        m.expand(&grid, fronts, rho);
        while let Some(seed) = m.find_seed(&grid, rho) {
            m.expand(&grid, seed, rho);
        }
    }
    let holes = m.hole_count();
    let unused = m.used.iter().filter(|u| !**u).count();
    let tris = std::mem::take(&mut m.tris);
    let stats = BpaStats { triangles: tris.len(), unused_points: unused, holes };
    (tris, stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fibonacci-lattice sphere with exact outward normals.
    fn sphere(n: usize, r: f64) -> (Vec<[f64; 3]>, Vec<[f64; 3]>) {
        let ga = std::f64::consts::PI * (3.0 - 5f64.sqrt());
        let mut p = Vec::new();
        let mut nn = Vec::new();
        for i in 0..n {
            let z = 1.0 - 2.0 * (i as f64 + 0.5) / n as f64;
            let rr = (1.0 - z * z).sqrt();
            let th = ga * i as f64;
            let d = [rr * th.cos(), rr * th.sin(), z];
            p.push([d[0] * r, d[1] * r, d[2] * r]);
            nn.push(d);
        }
        (p, nn)
    }

    fn undirected(tris: &[[u32; 3]]) -> HashMap<(u32, u32), usize> {
        let mut m = HashMap::new();
        for t in tris {
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                *m.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        m
    }

    #[test]
    fn ball_pivot_sphere_gives_a_closed_manifold_with_euler_two() {
        let (p, n) = sphere(2000, 50.0);
        // mean spacing sqrt(4 pi R^2 / N) = 3.96 mm; the first radius resolves
        // the bulk, the larger ones close the sparse spots
        let (tris, st) = ball_pivot(&p, &n, &[4.5, 6.0, 8.0]);
        assert_eq!(st.unused_points, 0, "{st:?}");
        assert_eq!(st.holes, 0, "{st:?}");
        let e = undirected(&tris);
        assert!(e.values().all(|&c| c == 2), "open or non-manifold edge");
        assert_eq!(p.len() as i64 - e.len() as i64 + tris.len() as i64, 2);
        // consistent outward winding: signed volume ~ 4/3 pi R^3
        let vol: f64 = tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|k| DVec3::from_array(p[k as usize]));
                a.dot(b.cross(c)) / 6.0
            })
            .sum();
        let exact = 4.0 / 3.0 * std::f64::consts::PI * 50f64.powi(3);
        assert!(vol > 0.9 * exact && vol < 1.02 * exact, "volume {vol} vs {exact}");
        assert_eq!(st.triangles, tris.len());
        // a closed triangulation with V vertices has 2V - 4 triangles
        assert_eq!(tris.len(), 2 * p.len() - 4);
    }

    #[test]
    fn ball_pivot_plane_patch_triangle_count() {
        // 11 x 11 lattice at 2 mm: 10 x 10 squares -> 200 triangles
        let mut p = Vec::new();
        for j in 0..11 {
            for i in 0..11 {
                // deterministic sub-micron jitter breaks exact cocircularity
                let jt = ((i * 7 + j * 13) % 5) as f64 * 1e-4;
                p.push([i as f64 * 2.0 + jt, j as f64 * 2.0 - jt, jt]);
            }
        }
        let n = vec![[0.0, 0.0, 1.0]; p.len()];
        // square circumradius 2/sqrt 2 = 1.414 mm, so a 2 mm ball fits every cell
        let (tris, st) = ball_pivot(&p, &n, &[2.0]);
        let expect = 200.0;
        assert!((tris.len() as f64 - expect).abs() <= 0.1 * expect, "{} triangles", tris.len());
        assert_eq!(st.unused_points, 0);
        // upward winding: every triangle normal has positive z
        for t in &tris {
            let [a, b, c] = t.map(|k| DVec3::from_array(p[k as usize]));
            assert!((b - a).cross(c - a).z > 0.0);
        }
        // a single outer loop remains
        assert_eq!(st.holes, 1);
    }

    #[test]
    fn ball_pivot_points_beyond_the_ball_diameter_give_nothing() {
        // lattice spacing 10 mm, ball radius 3 mm (diameter 6 < 10)
        let mut p = Vec::new();
        for j in 0..4 {
            for i in 0..4 {
                p.push([i as f64 * 10.0, j as f64 * 10.0, 0.0]);
            }
        }
        let n = vec![[0.0, 0.0, 1.0]; p.len()];
        let (tris, st) = ball_pivot(&p, &n, &[3.0]);
        assert!(tris.is_empty());
        assert_eq!(st.unused_points, 16);
        assert_eq!(st.holes, 0);
    }

    #[test]
    fn ball_pivot_degenerate_input_is_harmless() {
        let (t, s) = ball_pivot(&[], &[], &[5.0]);
        assert!(t.is_empty() && s.triangles == 0);
        let (t, _) = ball_pivot(&[[0.0; 3]; 2], &[[0.0, 0.0, 1.0]; 2], &[5.0]);
        assert!(t.is_empty());
        let (t, _) = ball_pivot(&[[0.0; 3]; 5], &[[0.0, 0.0, 1.0]; 5], &[0.0, f64::NAN, -1.0]);
        assert!(t.is_empty());
    }
}

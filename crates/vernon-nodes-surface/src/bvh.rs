//! f64 triangle BVH with exact closest-point queries and signed distance
//! (angle-weighted pseudo-normals decide the sign at edges and vertices).

use std::collections::HashMap;

use glam::DVec3;
use rayon::prelude::*;
use vernon_core::Geometry;

use crate::topology::edge_key;
use crate::util::triangulate_ring;

#[derive(Debug, Clone, Copy)]
struct Node {
    lo: DVec3,
    hi: DVec3,
    /// Leaf: first index into `order`.
    start: u32,
    /// Leaf triangle count; 0 for inner nodes.
    count: u32,
    left: u32,
    right: u32,
}

/// Result of a closest-point query.
#[derive(Debug, Clone, Copy)]
pub struct Hit {
    pub tri: usize,
    pub point: DVec3,
    /// Unsigned distance.
    pub dist: f64,
    /// Distance with sign: positive on the side the surface normals point to.
    pub signed: f64,
}

#[derive(Clone, Copy, PartialEq)]
enum Feature {
    Face,
    Vertex(usize),
    Edge(usize),
}

pub struct TriBvh {
    pts: Vec<DVec3>,
    tris: Vec<[u32; 3]>,
    nodes: Vec<Node>,
    order: Vec<u32>,
    face_n: Vec<DVec3>,
    vert_n: HashMap<u32, DVec3>,
    edge_n: HashMap<(u32, u32), DVec3>,
}

impl TriBvh {
    /// Build over indexed triangles. Degenerate triangles are ignored.
    pub fn new(points: Vec<DVec3>, tris_in: &[[u32; 3]]) -> Self {
        let mut tris = Vec::with_capacity(tris_in.len());
        let mut face_n = Vec::with_capacity(tris_in.len());
        for t in tris_in {
            let (a, b, c) = (points[t[0] as usize], points[t[1] as usize], points[t[2] as usize]);
            let n = (b - a).cross(c - a);
            if n.length_squared() > 1e-30 {
                tris.push(*t);
                face_n.push(n.normalize());
            }
        }
        let mut vert_n: HashMap<u32, DVec3> = HashMap::new();
        let mut edge_n: HashMap<(u32, u32), DVec3> = HashMap::new();
        for (ti, t) in tris.iter().enumerate() {
            let fnrm = face_n[ti];
            for i in 0..3 {
                let (a, b, c) = (points[t[i] as usize], points[t[(i + 1) % 3] as usize], points[t[(i + 2) % 3] as usize]);
                let e1 = (b - a).normalize_or_zero();
                let e2 = (c - a).normalize_or_zero();
                let ang = e1.dot(e2).clamp(-1.0, 1.0).acos();
                *vert_n.entry(t[i]).or_insert(DVec3::ZERO) += fnrm * ang;
                *edge_n.entry(edge_key(t[i], t[(i + 1) % 3]).0).or_insert(DVec3::ZERO) += fnrm;
            }
        }
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let mut nodes = Vec::new();
        if !tris.is_empty() {
            let cents: Vec<DVec3> = tris.iter().map(|t| (points[t[0] as usize] + points[t[1] as usize] + points[t[2] as usize]) / 3.0).collect();
            let len = order.len();
            build(&points, &tris, &cents, &mut order, 0, len, &mut nodes);
        }
        Self { pts: points, tris, nodes, order, face_n, vert_n, edge_n }
    }

    /// Triangulate the closed prims of `geo` (optionally welding first so
    /// pseudo-normals are shared across coincident vertices).
    pub fn from_geometry(geo: &Geometry, weld_tol: Option<f64>) -> Self {
        match weld_tol {
            Some(tol) => {
                let w = crate::topology::weld(geo, tol);
                let mut tris = Vec::new();
                for f in &w.faces {
                    tris.extend(triangulate_ring(&w.points, f));
                }
                Self::new(w.points, &tris)
            }
            None => {
                let mut tris = Vec::new();
                for p in geo.prims.iter().filter(|p| p.closed && p.vertices.len() >= 3) {
                    tris.extend(triangulate_ring(&geo.points, &p.vertices));
                }
                Self::new(geo.points.clone(), &tris)
            }
        }
    }

    pub fn tri_count(&self) -> usize {
        self.tris.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    pub fn triangle(&self, i: usize) -> [DVec3; 3] {
        let t = self.tris[i];
        [self.pts[t[0] as usize], self.pts[t[1] as usize], self.pts[t[2] as usize]]
    }

    pub fn face_normal(&self, i: usize) -> DVec3 {
        self.face_n[i]
    }

    /// Closest point on the surface, with sign. `None` for an empty BVH.
    pub fn closest(&self, q: DVec3) -> Option<Hit> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best_d2 = f64::INFINITY;
        let mut best: Option<(usize, DVec3, Feature)> = None;
        let mut stack: Vec<u32> = vec![0];
        while let Some(ni) = stack.pop() {
            let node = &self.nodes[ni as usize];
            if box_dist2(q, node.lo, node.hi) > best_d2 {
                continue;
            }
            if node.count > 0 {
                for k in 0..node.count {
                    let ti = self.order[(node.start + k) as usize] as usize;
                    let t = self.tris[ti];
                    let (p, f) = closest_on_triangle(q, self.pts[t[0] as usize], self.pts[t[1] as usize], self.pts[t[2] as usize]);
                    let d2 = (p - q).length_squared();
                    let better = d2 < best_d2 || (d2 == best_d2 && best.is_some_and(|b| ti < b.0));
                    if better {
                        best_d2 = d2;
                        best = Some((ti, p, f));
                    }
                }
            } else {
                let l = node.left;
                let r = node.right;
                let dl = box_dist2(q, self.nodes[l as usize].lo, self.nodes[l as usize].hi);
                let dr = box_dist2(q, self.nodes[r as usize].lo, self.nodes[r as usize].hi);
                // push farther first so the nearer pops first
                if dl <= dr {
                    stack.push(r);
                    stack.push(l);
                } else {
                    stack.push(l);
                    stack.push(r);
                }
            }
        }
        let (ti, p, feat) = best?;
        let t = self.tris[ti];
        let pn = match feat {
            Feature::Face => self.face_n[ti],
            Feature::Vertex(i) => self.vert_n.get(&t[i]).copied().unwrap_or(self.face_n[ti]),
            Feature::Edge(i) => self.edge_n.get(&edge_key(t[i], t[(i + 1) % 3]).0).copied().unwrap_or(self.face_n[ti]),
        };
        let dist = best_d2.sqrt();
        let side = (q - p).dot(pn);
        let signed = if side < 0.0 { -dist } else { dist };
        Some(Hit { tri: ti, point: p, dist, signed })
    }

    /// Parallel batch of closest-point queries (results independent of thread count).
    pub fn closest_many(&self, qs: &[DVec3]) -> Vec<Option<Hit>> {
        qs.par_iter().map(|&q| self.closest(q)).collect()
    }

    pub fn bounds(&self) -> (DVec3, DVec3) {
        match self.nodes.first() {
            Some(n) => (n.lo, n.hi),
            None => (DVec3::ZERO, DVec3::ZERO),
        }
    }
}

fn build(pts: &[DVec3], tris: &[[u32; 3]], cents: &[DVec3], order: &mut [u32], lo: usize, hi: usize, nodes: &mut Vec<Node>) -> u32 {
    let mut bmin = DVec3::splat(f64::INFINITY);
    let mut bmax = DVec3::splat(f64::NEG_INFINITY);
    let mut cmin = DVec3::splat(f64::INFINITY);
    let mut cmax = DVec3::splat(f64::NEG_INFINITY);
    for &ti in &order[lo..hi] {
        for &v in &tris[ti as usize] {
            bmin = bmin.min(pts[v as usize]);
            bmax = bmax.max(pts[v as usize]);
        }
        cmin = cmin.min(cents[ti as usize]);
        cmax = cmax.max(cents[ti as usize]);
    }
    let idx = nodes.len() as u32;
    nodes.push(Node { lo: bmin, hi: bmax, start: lo as u32, count: (hi - lo) as u32, left: 0, right: 0 });
    if hi - lo <= 4 {
        return idx;
    }
    let ext = cmax - cmin;
    let axis = if ext.x >= ext.y && ext.x >= ext.z {
        0
    } else if ext.y >= ext.z {
        1
    } else {
        2
    };
    if ext[axis] <= 0.0 {
        return idx;
    }
    let mid = (lo + hi) / 2;
    order[lo..hi].select_nth_unstable_by(mid - lo, |&a, &b| cents[a as usize][axis].total_cmp(&cents[b as usize][axis]).then(a.cmp(&b)));
    let l = build(pts, tris, cents, order, lo, mid, nodes);
    let r = build(pts, tris, cents, order, mid, hi, nodes);
    let n = &mut nodes[idx as usize];
    n.count = 0;
    n.left = l;
    n.right = r;
    idx
}

fn box_dist2(q: DVec3, lo: DVec3, hi: DVec3) -> f64 {
    let d = (lo - q).max(q - hi).max(DVec3::ZERO);
    d.length_squared()
}

fn closest_on_triangle(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> (DVec3, Feature) {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, Feature::Vertex(0));
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return (b, Feature::Vertex(1));
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (a + ab * v, Feature::Edge(0));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return (c, Feature::Vertex(2));
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (a + ac * w, Feature::Edge(2));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (b + (c - b) * w, Feature::Edge(1));
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (a + ab * v + ac * w, Feature::Face)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::Rng;

    fn brute(bvh: &TriBvh, q: DVec3) -> f64 {
        (0..bvh.tri_count())
            .map(|i| {
                let [a, b, c] = bvh.triangle(i);
                (closest_on_triangle(q, a, b, c).0 - q).length()
            })
            .fold(f64::INFINITY, f64::min)
    }

    fn sphere_geo() -> Geometry {
        // simple UV sphere radius 10
        let (nu, nv) = (24usize, 12usize);
        let mut g = Geometry::new();
        for j in 0..=nv {
            let th = std::f64::consts::PI * j as f64 / nv as f64;
            for i in 0..nu {
                let ph = std::f64::consts::TAU * i as f64 / nu as f64;
                g.points.push(DVec3::new(10.0 * th.sin() * ph.cos(), 10.0 * th.sin() * ph.sin(), 10.0 * th.cos()));
            }
        }
        for j in 0..nv {
            for i in 0..nu {
                let a = (j * nu + i) as u32;
                let b = (j * nu + (i + 1) % nu) as u32;
                let c = ((j + 1) * nu + (i + 1) % nu) as u32;
                let d = ((j + 1) * nu + i) as u32;
                if j == 0 {
                    g.prims.push(vernon_core::Prim::polygon(vec![a, c, d]));
                } else if j == nv - 1 {
                    g.prims.push(vernon_core::Prim::polygon(vec![a, b, d]));
                } else {
                    g.prims.push(vernon_core::Prim::polygon(vec![a, b, c, d]));
                }
            }
        }
        g
    }

    #[test]
    fn matches_brute_force() {
        let g = sphere_geo();
        let bvh = TriBvh::from_geometry(&g, Some(1e-9));
        let mut rng = Rng::new(3);
        for _ in 0..2000 {
            let q = DVec3::new(rng.next_f64() - 0.5, rng.next_f64() - 0.5, rng.next_f64() - 0.5) * 40.0;
            let h = bvh.closest(q).unwrap();
            assert!((h.dist - brute(&bvh, q)).abs() < 1e-12);
        }
    }

    #[test]
    fn sign_is_positive_outside_for_outward_mesh() {
        let g = Geometry::make_box(DVec3::splat(10.0));
        let bvh = TriBvh::from_geometry(&g, Some(1e-9));
        // outside near a corner (vertex feature), near an edge, near a face
        for q in [DVec3::new(8.0, 8.0, 8.0), DVec3::new(7.0, 7.0, 0.0), DVec3::new(0.0, 0.0, 9.0)] {
            assert!(bvh.closest(q).unwrap().signed > 0.0, "{q:?}");
        }
        for q in [DVec3::new(0.0, 0.0, 0.0), DVec3::new(4.0, 4.0, 4.5)] {
            assert!(bvh.closest(q).unwrap().signed < 0.0, "{q:?}");
        }
        let h = bvh.closest(DVec3::new(0.0, 0.0, 8.0)).unwrap();
        assert!((h.signed - 3.0).abs() < 1e-12);
    }

    #[test]
    fn point_on_triangle_is_zero() {
        let g = Geometry::make_box(DVec3::splat(10.0));
        let bvh = TriBvh::from_geometry(&g, None);
        assert!(bvh.closest(DVec3::new(1.0, 2.0, 5.0)).unwrap().dist < 1e-12);
    }
}

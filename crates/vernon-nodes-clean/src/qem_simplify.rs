//! Standalone quadric-error edge-collapse simplifier.
//!
//! Method: M. Garland and P. Heckbert, "Surface Simplification Using Quadric
//! Error Metrics", SIGGRAPH 1997. Each vertex carries the sum of the squared
//! distance functions of the planes of its incident faces; collapsing an edge
//! costs the quadric of the merged vertex at the new position. Collapses are
//! taken from a binary heap with lazy invalidation (per-vertex version stamps).
//!
//! Additions on top of the paper:
//! * boundary preservation: every boundary edge adds a penalty plane through
//!   the edge, perpendicular to its face (the paper's section 5 "boundary"
//!   remark). A boundary vertex is only ever collapsed onto an existing
//!   boundary vertex, so surviving boundary vertices never move.
//! * the topological link condition (Dey, Edelsbrunner, Guha, Nekhayev,
//!   "Topology Preserving Edge Contraction", 1999) so a collapse can never
//!   create a non-manifold edge or vertex.
//! * a face-flip check: a collapse is rejected when it rotates an adjacent
//!   face normal by more than `max_normal_deg`.
//!
//! The cost of a collapse is a sum of squared plane distances in mm^2, so
//! `sqrt(cost)` is a local plane-deviation proxy and not the true Hausdorff
//! distance. The true achieved error is measured afterwards and returned.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use glam::DVec3;

use crate::decimate::Quadric;
use crate::trimesh::TriMesh;

/// Controls of [`qem_simplify`].
#[derive(Clone, Debug)]
pub struct SimplifyParams {
    /// Stop when this many triangles remain (`None` = no count target).
    pub target_tris: Option<usize>,
    /// Stop when the cheapest valid collapse has `sqrt(cost)` above this many
    /// mm (`None` = no error limit). With both set, whichever trips first wins.
    pub max_error_mm: Option<f64>,
    /// Largest rotation (degrees) of an adjacent face normal a collapse may cause.
    pub max_normal_deg: f64,
    /// Weight of the boundary penalty planes relative to face planes.
    pub boundary_weight: f64,
}

impl Default for SimplifyParams {
    fn default() -> Self {
        Self { target_tris: None, max_error_mm: None, max_normal_deg: 60.0, boundary_weight: 1000.0 }
    }
}

/// Simplified mesh plus the error actually achieved against the input.
#[derive(Clone, Debug)]
pub struct SimplifyResult {
    pub positions: Vec<DVec3>,
    pub tris: Vec<[u32; 3]>,
    /// Largest distance (mm) from a vertex of either mesh to the other surface.
    pub max_error: f64,
    /// Mean of the same vertex-to-surface distances (mm).
    pub mean_error: f64,
    pub collapses: usize,
}

#[derive(Clone, Copy)]
struct Cand {
    cost: f64,
    u: u32,
    v: u32,
    ver_u: u32,
    ver_v: u32,
    p: DVec3,
}

impl PartialEq for Cand {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for Cand {}
impl PartialOrd for Cand {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Cand {
    fn cmp(&self, o: &Self) -> Ordering {
        // max-heap: reverse so the cheapest pops first; ids make ties deterministic
        o.cost.total_cmp(&self.cost).then(o.u.cmp(&self.u)).then(o.v.cmp(&self.v))
    }
}

struct Work {
    pos: Vec<DVec3>,
    tris: Vec<[u32; 3]>,
    alive_f: Vec<bool>,
    alive_v: Vec<bool>,
    vfaces: Vec<Vec<u32>>,
    q: Vec<Quadric>,
    ver: Vec<u32>,
    live: usize,
}

fn face_normal(a: DVec3, b: DVec3, c: DVec3) -> DVec3 {
    (b - a).cross(c - a)
}

impl Work {
    fn faces_of(&self, v: u32) -> impl Iterator<Item = u32> + '_ {
        self.vfaces[v as usize].iter().copied().filter(|&f| self.alive_f[f as usize])
    }

    fn neighbours(&self, v: u32) -> Vec<u32> {
        let mut n: Vec<u32> = Vec::new();
        for f in self.faces_of(v) {
            for &w in &self.tris[f as usize] {
                if w != v {
                    n.push(w);
                }
            }
        }
        n.sort_unstable();
        n.dedup();
        n
    }

    /// Number of live faces containing both `a` and `b`.
    fn edge_faces(&self, a: u32, b: u32) -> usize {
        self.faces_of(a).filter(|&f| self.tris[f as usize].contains(&b)).count()
    }

    fn is_boundary_vertex(&self, v: u32) -> bool {
        self.neighbours(v).into_iter().any(|w| self.edge_faces(v, w) == 1)
    }

    fn has_face(&self, a: u32, b: u32, c: u32) -> bool {
        self.faces_of(a).any(|f| {
            let t = &self.tris[f as usize];
            t.contains(&b) && t.contains(&c)
        })
    }

    /// Link condition: the common neighbours of `u` and `v` are exactly the
    /// vertices opposite the edge, and no face pair would be merged.
    fn link_ok(&self, u: u32, v: u32) -> bool {
        let shared: Vec<u32> = self.faces_of(u).filter(|&f| self.tris[f as usize].contains(&v)).collect();
        if shared.is_empty() || shared.len() > 2 {
            return false;
        }
        let mut opp: Vec<u32> = shared
            .iter()
            .map(|&f| *self.tris[f as usize].iter().find(|&&w| w != u && w != v).unwrap())
            .collect();
        opp.sort_unstable();
        let nu = self.neighbours(u);
        let nv = self.neighbours(v);
        let common: Vec<u32> = nu.iter().copied().filter(|w| nv.binary_search(w).is_ok()).collect();
        if common != opp {
            return false;
        }
        for i in 0..common.len() {
            for j in i + 1..common.len() {
                if self.has_face(u, common[i], common[j]) && self.has_face(v, common[i], common[j]) {
                    return false;
                }
            }
        }
        // an interior edge between two boundary vertices would pinch the border
        if shared.len() == 2 && self.is_boundary_vertex(u) && self.is_boundary_vertex(v) {
            return false;
        }
        true
    }

    /// No adjacent face may flip past `cos_max` or degenerate when `u` and `v`
    /// both move to `p`.
    fn flip_ok(&self, u: u32, v: u32, p: DVec3, cos_max: f64) -> bool {
        for (w, other) in [(u, v), (v, u)] {
            for f in self.faces_of(w) {
                let t = self.tris[f as usize];
                if t.contains(&other) {
                    continue; // vanishes with the collapse
                }
                let old = face_normal(self.pos[t[0] as usize], self.pos[t[1] as usize], self.pos[t[2] as usize]);
                let np = |i: usize| if t[i] == w { p } else { self.pos[t[i] as usize] };
                let new = face_normal(np(0), np(1), np(2));
                let (lo, ln) = (old.length(), new.length());
                if lo < 1e-18 {
                    continue; // already degenerate: nothing to protect
                }
                if ln < 1e-12 * lo.max(1e-9) || old.dot(new) / (lo * ln) < cos_max {
                    return false;
                }
            }
        }
        true
    }

    /// Best valid collapse of edge (u, v), if any.
    fn best(&self, u: u32, v: u32, cos_max: f64) -> Option<Cand> {
        if !self.link_ok(u, v) {
            return None;
        }
        let q = {
            let mut q = self.q[u as usize];
            q.add(&self.q[v as usize]);
            q
        };
        let (bu, bv) = (self.is_boundary_vertex(u), self.is_boundary_vertex(v));
        let (pu, pv) = (self.pos[u as usize], self.pos[v as usize]);
        let mut cands: Vec<DVec3> = Vec::with_capacity(4);
        if bu || bv {
            // boundary vertices never move: collapse onto the boundary one
            if bu {
                cands.push(pu);
            }
            if bv {
                cands.push(pv);
            }
        } else {
            if let Some(o) = q.optimum() {
                cands.push(o);
            }
            cands.extend([pu, pv, 0.5 * (pu + pv)]);
        }
        let mut scored: Vec<(f64, DVec3)> = cands.into_iter().map(|p| (q.eval(p).max(0.0), p)).collect();
        scored.sort_by(|a, b| a.0.total_cmp(&b.0));
        scored
            .into_iter()
            .find(|&(_, p)| self.flip_ok(u, v, p, cos_max))
            .map(|(cost, p)| Cand { cost, u, v, ver_u: self.ver[u as usize], ver_v: self.ver[v as usize], p })
    }

    fn push_edges(&self, heap: &mut BinaryHeap<Cand>, u: u32, cos_max: f64) {
        for w in self.neighbours(u) {
            let (a, b) = if u < w { (u, w) } else { (w, u) };
            if let Some(c) = self.best(a, b, cos_max) {
                heap.push(c);
            }
        }
    }

    /// Merge `c.v` into `c.u` at `c.p`.
    fn collapse(&mut self, c: &Cand) {
        let (u, v) = (c.u as usize, c.v as usize);
        let vf = std::mem::take(&mut self.vfaces[v]);
        for f in vf {
            if !self.alive_f[f as usize] {
                continue;
            }
            if self.tris[f as usize].contains(&c.u) {
                self.alive_f[f as usize] = false;
                self.live -= 1;
            } else {
                for k in self.tris[f as usize].iter_mut() {
                    if *k == c.v {
                        *k = c.u;
                    }
                }
                self.vfaces[u].push(f);
            }
        }
        let qv = self.q[v];
        self.q[u].add(&qv);
        self.pos[u] = c.p;
        self.alive_v[v] = false;
        self.ver[u] += 1;
        self.ver[v] += 1;
    }
}

/// Simplify the triangle mesh `pos`/`tris`. Indices must be in range; the mesh
/// should be welded (shared vertices) for topology to be meaningful.
pub fn qem_simplify(pos: &[DVec3], tris: &[[u32; 3]], params: &SimplifyParams) -> SimplifyResult {
    let nv = pos.len();
    let mut w = Work {
        pos: pos.to_vec(),
        tris: tris.to_vec(),
        alive_f: vec![true; tris.len()],
        alive_v: vec![true; nv],
        vfaces: vec![Vec::new(); nv],
        q: vec![Quadric::default(); nv],
        ver: vec![0; nv],
        live: tris.len(),
    };
    for (fi, t) in tris.iter().enumerate() {
        for &k in t {
            w.vfaces[k as usize].push(fi as u32);
        }
        let [a, b, c] = t.map(|k| pos[k as usize]);
        let n = face_normal(a, b, c);
        if n.length() > 1e-18 {
            let n = n.normalize();
            let qp = Quadric::plane(n, -n.dot(a), 1.0);
            for &k in t {
                w.q[k as usize].add(&qp);
            }
        }
    }
    // boundary penalty planes
    let mut edge_count: std::collections::HashMap<(u32, u32), (u32, usize)> = std::collections::HashMap::new();
    for (fi, t) in tris.iter().enumerate() {
        for e in 0..3 {
            let (a, b) = (t[e], t[(e + 1) % 3]);
            let key = if a < b { (a, b) } else { (b, a) };
            let ent = edge_count.entry(key).or_insert((0, fi));
            ent.0 += 1;
        }
    }
    for (&(a, b), &(count, fi)) in &edge_count {
        if count != 1 {
            continue;
        }
        let t = tris[fi];
        let [p0, p1, p2] = t.map(|k| pos[k as usize]);
        let nf = face_normal(p0, p1, p2);
        let e = pos[b as usize] - pos[a as usize];
        let pn = e.cross(nf);
        if pn.length() < 1e-18 {
            continue;
        }
        let pn = pn.normalize();
        let qp = Quadric::plane(pn, -pn.dot(pos[a as usize]), params.boundary_weight);
        w.q[a as usize].add(&qp);
        w.q[b as usize].add(&qp);
    }

    let cos_max = params.max_normal_deg.to_radians().cos();
    let max_cost = params.max_error_mm.map_or(f64::INFINITY, |e| e * e);
    let target = params.target_tris.unwrap_or(0);
    let active = params.target_tris.is_some() || params.max_error_mm.is_some();

    let mut heap = BinaryHeap::new();
    let mut collapses = 0;
    if active {
        // seed with every undirected edge once
        let mut keys: Vec<(u32, u32)> = edge_count.keys().copied().collect();
        keys.sort_unstable();
        for (a, b) in keys {
            if let Some(c) = w.best(a, b, cos_max) {
                heap.push(c);
            }
        }
        while w.live > target {
            let Some(c) = heap.pop() else { break };
            if !w.alive_v[c.u as usize]
                || !w.alive_v[c.v as usize]
                || w.ver[c.u as usize] != c.ver_u
                || w.ver[c.v as usize] != c.ver_v
            {
                continue; // stale
            }
            if c.cost > max_cost {
                break;
            }
            // neighbours may have moved since this entry was pushed
            if !w.link_ok(c.u, c.v) || !w.flip_ok(c.u, c.v, c.p, cos_max) {
                continue;
            }
            w.collapse(&c);
            collapses += 1;
            w.push_edges(&mut heap, c.u, cos_max);
        }
    }

    // compact
    let mut remap = vec![u32::MAX; nv];
    let mut positions = Vec::new();
    let mut out = Vec::new();
    for (fi, t) in w.tris.iter().enumerate() {
        if !w.alive_f[fi] {
            continue;
        }
        let mut nt = [0u32; 3];
        for k in 0..3 {
            let o = t[k] as usize;
            if remap[o] == u32::MAX {
                remap[o] = positions.len() as u32;
                positions.push(w.pos[o]);
            }
            nt[k] = remap[o];
        }
        out.push(nt);
    }

    let (max_error, mean_error) = measure(pos, tris, &positions, &out);
    SimplifyResult { positions, tris: out, max_error, mean_error, collapses }
}

/// Vertex-to-surface distances in both directions: (max, mean).
fn measure(a_pos: &[DVec3], a_tris: &[[u32; 3]], b_pos: &[DVec3], b_tris: &[[u32; 3]]) -> (f64, f64) {
    if a_tris.is_empty() || b_tris.is_empty() {
        return (0.0, 0.0);
    }
    let ma = TriMesh::new(a_pos.to_vec(), a_tris.to_vec(), vec![0; a_tris.len()]);
    let mb = TriMesh::new(b_pos.to_vec(), b_tris.to_vec(), vec![0; b_tris.len()]);
    let (mut mx, mut sum, mut n) = (0.0f64, 0.0f64, 0usize);
    for p in a_pos {
        let d = mb.distance(*p);
        mx = mx.max(d);
        sum += d;
        n += 1;
    }
    for p in b_pos {
        let d = ma.distance(*p);
        mx = mx.max(d);
        sum += d;
        n += 1;
    }
    (mx, sum / n.max(1) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::uv_sphere;
    use std::collections::HashMap;

    fn grid(n: usize, step: f64) -> (Vec<DVec3>, Vec<[u32; 3]>) {
        let mut p = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                p.push(DVec3::new(i as f64 * step, j as f64 * step, 0.0));
            }
        }
        let id = |i: usize, j: usize| (j * (n + 1) + i) as u32;
        let mut t = Vec::new();
        for j in 0..n {
            for i in 0..n {
                t.push([id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
                t.push([id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
            }
        }
        (p, t)
    }

    /// directed edge -> count
    fn directed(tris: &[[u32; 3]]) -> HashMap<(u32, u32), usize> {
        let mut m = HashMap::new();
        for t in tris {
            for e in 0..3 {
                *m.entry((t[e], t[(e + 1) % 3])).or_insert(0) += 1;
            }
        }
        m
    }

    /// closed, 2-manifold and consistently oriented: every directed edge once,
    /// its reverse once.
    fn assert_closed_manifold(tris: &[[u32; 3]]) {
        let d = directed(tris);
        for (&(a, b), &c) in &d {
            assert_eq!(c, 1, "directed edge {a}->{b} used {c} times");
            assert_eq!(d.get(&(b, a)), Some(&1), "edge {a}-{b} lacks its opposite");
        }
    }

    #[test]
    fn qem_simplify_flat_grid_collapses_to_a_few_triangles_with_zero_error() {
        let (p, t) = grid(10, 5.0); // 200 triangles, 50 x 50 mm
        let r = qem_simplify(&p, &t, &SimplifyParams { target_tris: Some(2), ..Default::default() });
        assert!(r.tris.len() <= 8, "got {} triangles", r.tris.len());
        assert!(r.max_error < 1e-9, "error {}", r.max_error);
        // all four corners survive
        for c in [(0.0, 0.0), (50.0, 0.0), (50.0, 50.0), (0.0, 50.0)] {
            assert!(r.positions.iter().any(|q| (q.x - c.0).abs() < 1e-9 && (q.y - c.1).abs() < 1e-9));
        }
        // area is conserved (a flat sheet stays 50 x 50)
        let area: f64 = r
            .tris
            .iter()
            .map(|t| 0.5 * face_normal(r.positions[t[0] as usize], r.positions[t[1] as usize], r.positions[t[2] as usize]).length())
            .sum();
        assert!((area - 2500.0).abs() < 1e-6, "area {area}");
    }

    #[test]
    fn qem_simplify_sphere_at_ten_percent_stays_manifold_within_derived_bound() {
        let r_mm = 50.0;
        let (p, t) = uv_sphere(r_mm, 48, 24);
        let target = t.len() / 10;
        let r = qem_simplify(&p, &t, &SimplifyParams { target_tris: Some(target), ..Default::default() });
        assert!(r.tris.len() <= target + 2, "{} vs {}", r.tris.len(), target);
        assert_closed_manifold(&r.tris);
        // Euler characteristic of a sphere: V - E + F = 2
        let e = directed(&r.tris).len() / 2;
        assert_eq!(r.positions.len() as i64 - e as i64 + r.tris.len() as i64, 2);
        // Bound: N near-equilateral triangles tile 4 pi R^2, so edge L^2 =
        // 16 pi R^2 / (sqrt(3) N); the triangle centre is L^2/(6R) below the
        // sphere (circumradius L/sqrt 3, sagitta r^2/(2R)). Allow 2x for the
        // irregular valence of the collapsed mesh.
        let n = r.tris.len() as f64;
        let l2 = 16.0 * std::f64::consts::PI * r_mm * r_mm / (3f64.sqrt() * n);
        let bound = 2.0 * l2 / (6.0 * r_mm);
        assert!(r.max_error < bound, "max error {} >= bound {}", r.max_error, bound);
        assert!(r.mean_error <= r.max_error);
        // outward orientation: signed volume positive and close to 4/3 pi R^3
        let vol: f64 = r
            .tris
            .iter()
            .map(|t| r.positions[t[0] as usize].dot(r.positions[t[1] as usize].cross(r.positions[t[2] as usize])) / 6.0)
            .sum();
        let exact = 4.0 / 3.0 * std::f64::consts::PI * r_mm.powi(3);
        assert!(vol > 0.7 * exact && vol < 1.05 * exact, "volume {vol} vs {exact}");
    }

    #[test]
    fn qem_simplify_open_mesh_boundary_vertices_do_not_move() {
        // upper hemisphere: open, boundary = equator ring
        let (p, t) = uv_sphere(40.0, 32, 16);
        let keep: Vec<[u32; 3]> =
            t.iter().copied().filter(|t| t.iter().all(|&k| p[k as usize].z > -1e-9)).collect();
        let boundary_of = |pos: &[DVec3], tris: &[[u32; 3]]| -> Vec<DVec3> {
            let d = directed(tris);
            let mut b = Vec::new();
            for &(x, y) in d.keys() {
                if !d.contains_key(&(y, x)) {
                    b.push(pos[x as usize]);
                    b.push(pos[y as usize]);
                }
            }
            b
        };
        let before = boundary_of(&p, &keep);
        assert!(!before.is_empty());
        let r = qem_simplify(&p, &keep, &SimplifyParams { target_tris: Some(keep.len() / 5), ..Default::default() });
        assert!(r.tris.len() < keep.len() / 2);
        let after = boundary_of(&r.positions, &r.tris);
        assert!(!after.is_empty());
        for q in &after {
            let nearest = before.iter().map(|b| b.distance(*q)).fold(f64::INFINITY, f64::min);
            assert!(nearest < 0.01, "boundary vertex moved {nearest} mm");
        }
        // every directed edge still used at most once (no non-manifold edges)
        assert!(directed(&r.tris).values().all(|&c| c == 1));
    }

    #[test]
    fn qem_simplify_error_limit_stops_early() {
        let (p, t) = uv_sphere(50.0, 32, 16);
        let r = qem_simplify(&p, &t, &SimplifyParams { max_error_mm: Some(0.2), ..Default::default() });
        assert!(r.collapses > 0 && r.tris.len() < t.len());
        // far fewer collapses than a 10 % target would take
        assert!(r.tris.len() > t.len() / 2, "{} tris", r.tris.len());
        assert!(r.max_error < 1.0, "error {}", r.max_error);
    }

    #[test]
    fn qem_simplify_without_target_is_identity() {
        let (p, t) = uv_sphere(10.0, 8, 6);
        let r = qem_simplify(&p, &t, &SimplifyParams::default());
        assert_eq!(r.tris.len(), t.len());
        assert_eq!(r.collapses, 0);
        assert!(r.max_error < 1e-12);
    }

    #[test]
    fn qem_simplify_flip_limit_blocks_a_fold() {
        // a tent: two quads meeting at a 90 degree ridge. A tight normal
        // limit must never allow more reduction than a loose one.
        let (mut p, t) = grid(4, 10.0);
        for q in &mut p {
            q.z = if q.x > 20.0 { q.x - 20.0 } else { 0.0 };
        }
        let strict = qem_simplify(&p, &t, &SimplifyParams { max_error_mm: Some(1e-3), max_normal_deg: 5.0, ..Default::default() });
        let loose = qem_simplify(&p, &t, &SimplifyParams { max_error_mm: Some(1e-3), max_normal_deg: 80.0, ..Default::default() });
        // a 5 degree limit cannot fold either slope over the 45 degree ridge,
        // so with a 1e-3 mm cost limit only lossless collapses happen: zero error
        assert!(strict.max_error < 1e-6, "strict error {}", strict.max_error);
        assert!(strict.tris.len() >= loose.tris.len());
    }
}

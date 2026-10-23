//! Mutable triangle mesh with vertex-to-triangle adjacency, used by the
//! decimation, remeshing and hole-filling nodes. All operations keep the
//! orientation of the triangles they touch.

use std::collections::HashMap;

use glam::DVec3;
use vernon_core::Geometry;

/// Result of welding the triangles of a [`Geometry`].
pub struct Welded {
    /// Unique positions.
    pub pos: Vec<DVec3>,
    /// For each unique position, the index of the first original point.
    pub first: Vec<usize>,
    /// Original point index -> unique index.
    pub map: Vec<u32>,
    /// Triangles over unique indices; degenerate ones are dropped.
    pub tris: Vec<[u32; 3]>,
    /// Original primitive index of each triangle.
    pub src_prim: Vec<u32>,
}

/// Merge coincident points (exact match when `tol == 0`, grid snapped
/// otherwise) and re-index the triangles of `g`.
pub fn weld_triangles(g: &Geometry, tol: f64) -> Welded {
    let n = g.points.len();
    let mut map = vec![0u32; n];
    let mut first: Vec<usize> = Vec::new();
    let mut pos: Vec<DVec3> = Vec::new();
    let mut table: HashMap<[i64; 3], u32> = HashMap::with_capacity(n);
    let mut exact: HashMap<[u64; 3], u32> = HashMap::new();
    for (i, p) in g.points.iter().enumerate() {
        let id = if tol > 0.0 {
            let key = [(p.x / tol).round() as i64, (p.y / tol).round() as i64, (p.z / tol).round() as i64];
            *table.entry(key).or_insert_with(|| {
                first.push(i);
                pos.push(*p);
                (pos.len() - 1) as u32
            })
        } else {
            let key = [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()];
            *exact.entry(key).or_insert_with(|| {
                first.push(i);
                pos.push(*p);
                (pos.len() - 1) as u32
            })
        };
        map[i] = id;
    }
    let mut tris = Vec::new();
    let mut src_prim = Vec::new();
    for (pi, t) in g.triangles() {
        let v = [map[t[0] as usize], map[t[1] as usize], map[t[2] as usize]];
        if v[0] == v[1] || v[1] == v[2] || v[0] == v[2] {
            continue;
        }
        tris.push(v);
        src_prim.push(pi as u32);
    }
    Welded { pos, first, map, tris, src_prim }
}

/// Closed/open topology summary of an indexed triangle list.
#[derive(Debug, Clone, Copy, Default)]
pub struct EdgeStats {
    pub open_edges: usize,
    pub nonmanifold_edges: usize,
    pub inconsistent_edges: usize,
}

pub fn edge_stats(tris: &[[u32; 3]]) -> EdgeStats {
    let mut m: HashMap<(u32, u32), (u32, u32)> = HashMap::with_capacity(tris.len() * 2);
    for t in tris {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let (key, fwd) = if a < b { ((a, b), 1) } else { ((b, a), 0) };
            let e = m.entry(key).or_insert((0, 0));
            e.0 += 1;
            e.1 += fwd;
        }
    }
    let mut s = EdgeStats::default();
    for (_, (c, f)) in m {
        match c {
            1 => s.open_edges += 1,
            2 => {
                if f != 1 {
                    s.inconsistent_edges += 1;
                }
            }
            _ => s.nonmanifold_edges += 1,
        }
    }
    s
}

/// Build output geometry from positions and triangles (point attributes are
/// not carried; callers add what they need).
pub fn triangles_to_geometry(pos: Vec<DVec3>, tris: &[[u32; 3]]) -> Geometry {
    Geometry {
        points: pos,
        prims: tris.iter().map(|t| vernon_core::Prim::polygon(t.to_vec())).collect(),
        ..Default::default()
    }
}

/// Class of a vertex for feature-preserving operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VClass {
    Free,
    /// On a mesh boundary.
    Boundary,
    /// On a sharp edge (dihedral angle above the threshold) or a
    /// non-manifold edge.
    Feature,
}

#[derive(Clone)]
pub struct EditMesh {
    pub pos: Vec<DVec3>,
    pub tris: Vec<[u32; 3]>,
    pub alive: Vec<bool>,
    /// Alive triangles incident to each vertex.
    pub vt: Vec<Vec<u32>>,
    pub vdead: Vec<bool>,
    pub n_alive: usize,
}

fn opposite(t: &[u32; 3], u: u32, v: u32) -> Option<u32> {
    t.iter().copied().find(|&x| x != u && x != v)
}

impl EditMesh {
    pub fn new(pos: Vec<DVec3>, tris: Vec<[u32; 3]>) -> Self {
        let mut vt = vec![Vec::new(); pos.len()];
        for (i, t) in tris.iter().enumerate() {
            for &v in t {
                vt[v as usize].push(i as u32);
            }
        }
        let n = tris.len();
        let nv = pos.len();
        EditMesh { pos, tris, alive: vec![true; n], vt, vdead: vec![false; nv], n_alive: n }
    }

    pub fn raw_normal(&self, t: u32) -> DVec3 {
        let v = self.tris[t as usize];
        let (a, b, c) = (self.pos[v[0] as usize], self.pos[v[1] as usize], self.pos[v[2] as usize]);
        (b - a).cross(c - a)
    }

    pub fn edge_tris(&self, u: u32, v: u32) -> Vec<u32> {
        self.vt[u as usize]
            .iter()
            .copied()
            .filter(|&t| self.tris[t as usize].contains(&v))
            .collect()
    }

    pub fn neighbors(&self, v: u32) -> Vec<u32> {
        let mut out: Vec<u32> = Vec::with_capacity(self.vt[v as usize].len() + 2);
        for &t in &self.vt[v as usize] {
            for &x in &self.tris[t as usize] {
                if x != v {
                    out.push(x);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Neighbours with the number of incident triangles of each edge.
    pub fn neighbor_counts(&self, v: u32) -> Vec<(u32, u32)> {
        let mut all: Vec<u32> = Vec::with_capacity(self.vt[v as usize].len() * 2);
        for &t in &self.vt[v as usize] {
            for &x in &self.tris[t as usize] {
                if x != v {
                    all.push(x);
                }
            }
        }
        all.sort_unstable();
        let mut out: Vec<(u32, u32)> = Vec::new();
        for x in all {
            match out.last_mut() {
                Some(l) if l.0 == x => l.1 += 1,
                _ => out.push((x, 1)),
            }
        }
        out
    }

    pub fn is_boundary_vertex(&self, v: u32) -> bool {
        self.neighbor_counts(v).iter().any(|&(_, c)| c == 1)
    }

    pub fn valence(&self, v: u32) -> usize {
        self.neighbor_counts(v).len()
    }

    /// Boundary / sharp-edge / free classification of a vertex.
    pub fn classify(&self, v: u32, cos_crease: f64) -> VClass {
        let mut class = VClass::Free;
        for (w, c) in self.neighbor_counts(v) {
            if c == 1 {
                return VClass::Boundary;
            }
            if c > 2 {
                return VClass::Feature;
            }
            if class == VClass::Free && cos_crease > -1.0 {
                let ts = self.edge_tris(v, w);
                if ts.len() == 2 {
                    let (a, b) = (self.raw_normal(ts[0]).normalize_or_zero(), self.raw_normal(ts[1]).normalize_or_zero());
                    if a.dot(b) < cos_crease {
                        class = VClass::Feature;
                    }
                }
            }
        }
        class
    }

    /// True when edge (u, v) is a boundary edge, a non-manifold edge or a
    /// sharp edge.
    pub fn is_fixed_edge(&self, u: u32, v: u32, cos_crease: f64) -> bool {
        let ts = self.edge_tris(u, v);
        match ts.len() {
            2 => {
                let (a, b) = (self.raw_normal(ts[0]).normalize_or_zero(), self.raw_normal(ts[1]).normalize_or_zero());
                a.dot(b) < cos_crease
            }
            _ => true,
        }
    }

    pub fn remove_tri(&mut self, t: u32) {
        if !self.alive[t as usize] {
            return;
        }
        self.alive[t as usize] = false;
        self.n_alive -= 1;
        for k in 0..3 {
            let v = self.tris[t as usize][k] as usize;
            if let Some(i) = self.vt[v].iter().position(|&x| x == t) {
                self.vt[v].swap_remove(i);
            }
        }
    }

    pub fn add_tri(&mut self, a: u32, b: u32, c: u32) -> u32 {
        let id = self.tris.len() as u32;
        self.tris.push([a, b, c]);
        self.alive.push(true);
        self.n_alive += 1;
        for v in [a, b, c] {
            self.vt[v as usize].push(id);
        }
        id
    }

    pub fn add_vertex(&mut self, p: DVec3) -> u32 {
        self.pos.push(p);
        self.vt.push(Vec::new());
        self.vdead.push(false);
        (self.pos.len() - 1) as u32
    }

    /// Split edge (u, v) at `at`. Returns the new vertex, or `None` for a
    /// missing or non-manifold edge.
    pub fn split_edge(&mut self, u: u32, v: u32, at: DVec3) -> Option<u32> {
        let ts = self.edge_tris(u, v);
        if ts.is_empty() || ts.len() > 2 {
            return None;
        }
        let m = self.add_vertex(at);
        for t in ts {
            let tri = self.tris[t as usize];
            let i = tri.iter().position(|&x| x == u).unwrap();
            let forward = tri[(i + 1) % 3] == v; // u -> v along the winding
            let w = opposite(&tri, u, v).unwrap();
            self.remove_tri(t);
            if forward {
                self.add_tri(u, m, w);
                self.add_tri(m, v, w);
            } else {
                self.add_tri(v, m, w);
                self.add_tri(m, u, w);
            }
        }
        Some(m)
    }

    /// Flip interior edge (u, v). Fails when the opposite vertices are
    /// already joined or the edge is not shared by exactly two triangles.
    pub fn flip_edge(&mut self, u: u32, v: u32) -> bool {
        let ts = self.edge_tris(u, v);
        if ts.len() != 2 {
            return false;
        }
        // orient so that t1 runs u -> v
        let (mut t1, mut t2) = (ts[0], ts[1]);
        let tri1 = self.tris[t1 as usize];
        let i = tri1.iter().position(|&x| x == u).unwrap();
        if tri1[(i + 1) % 3] != v {
            std::mem::swap(&mut t1, &mut t2);
        }
        let a = opposite(&self.tris[t1 as usize], u, v).unwrap();
        let b = opposite(&self.tris[t2 as usize], u, v).unwrap();
        if a == b || self.vt[a as usize].iter().any(|&t| self.tris[t as usize].contains(&b)) {
            return false;
        }
        self.remove_tri(t1);
        self.remove_tri(t2);
        self.add_tri(a, u, b);
        self.add_tri(a, b, v);
        true
    }

    /// Replace triangle `t` by three triangles around a new vertex at `at`.
    pub fn split_tri(&mut self, t: u32, at: DVec3) -> u32 {
        let [a, b, c] = self.tris[t as usize];
        self.remove_tri(t);
        let m = self.add_vertex(at);
        self.add_tri(a, b, m);
        self.add_tri(b, c, m);
        self.add_tri(c, a, m);
        m
    }

    /// Topological test for collapsing edge (u, v): the vertices adjacent to
    /// both must be exactly the apexes of the triangles on the edge.
    pub fn link_ok(&self, u: u32, v: u32) -> bool {
        let shared = self.edge_tris(u, v).len();
        if shared == 0 || shared > 2 {
            return false;
        }
        let nu = self.neighbors(u);
        let nv = self.neighbors(v);
        let (mut i, mut j, mut common) = (0, 0, 0);
        while i < nu.len() && j < nv.len() {
            match nu[i].cmp(&nv[j]) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    common += 1;
                    i += 1;
                    j += 1;
                }
            }
        }
        common == shared
    }

    /// Would moving `u` and `v` to `p` (merging them) flip or crush a
    /// surrounding triangle? `min_cos` is the smallest allowed cosine
    /// between old and new triangle normals.
    pub fn collapse_flips(&self, u: u32, v: u32, p: DVec3, min_cos: f64) -> bool {
        for &w in [u, v].iter() {
            for &t in &self.vt[w as usize] {
                let tri = self.tris[t as usize];
                if tri.contains(&u) && tri.contains(&v) {
                    continue;
                }
                let old = self.raw_normal(t);
                let q = |x: u32| if x == u || x == v { p } else { self.pos[x as usize] };
                let new = (q(tri[1]) - q(tri[0])).cross(q(tri[2]) - q(tri[0]));
                let (lo, ln) = (old.length(), new.length());
                if ln < 1e-12 * lo.max(1e-30) || ln <= 1e-18 {
                    return true;
                }
                if lo > 0.0 && old.dot(new) < min_cos * lo * ln {
                    return true;
                }
            }
        }
        false
    }

    /// Merge `remove` into `keep`, moving `keep` to `p`. The caller has
    /// checked `link_ok`.
    pub fn collapse(&mut self, keep: u32, remove: u32, p: DVec3) {
        let ts: Vec<u32> = self.vt[remove as usize].clone();
        for t in ts {
            let tri = self.tris[t as usize];
            if tri.contains(&keep) {
                self.remove_tri(t);
            } else {
                // re-index in place
                if let Some(i) = self.vt[remove as usize].iter().position(|&x| x == t) {
                    self.vt[remove as usize].swap_remove(i);
                }
                for k in 0..3 {
                    if self.tris[t as usize][k] == remove {
                        self.tris[t as usize][k] = keep;
                    }
                }
                self.vt[keep as usize].push(t);
            }
        }
        self.pos[keep as usize] = p;
        self.vdead[remove as usize] = true;
    }

    /// Compact to used vertices. Returns positions, triangles and the old
    /// index of each new vertex.
    pub fn compact(&self) -> (Vec<DVec3>, Vec<[u32; 3]>, Vec<u32>) {
        let mut map = vec![u32::MAX; self.pos.len()];
        let mut pos = Vec::new();
        let mut old = Vec::new();
        let mut tris = Vec::with_capacity(self.n_alive);
        for (i, t) in self.tris.iter().enumerate() {
            if !self.alive[i] {
                continue;
            }
            let mut o = [0u32; 3];
            for k in 0..3 {
                let v = t[k] as usize;
                if map[v] == u32::MAX {
                    map[v] = pos.len() as u32;
                    pos.push(self.pos[v]);
                    old.push(v as u32);
                }
                o[k] = map[v];
            }
            tris.push(o);
        }
        (pos, tris, old)
    }

    /// Area-weighted vertex normal.
    pub fn vertex_normal(&self, v: u32) -> DVec3 {
        let mut n = DVec3::ZERO;
        for &t in &self.vt[v as usize] {
            n += self.raw_normal(t);
        }
        n.normalize_or_zero()
    }
}

/// Per-vertex flags from edge incidence: `boundary` (on an open edge) and
/// `feature` (on a sharp or non-manifold edge, dihedral angle above the
/// crease threshold given as a cosine).
pub fn edge_flags(pos: &[DVec3], tris: &[[u32; 3]], cos_crease: f64) -> (Vec<bool>, Vec<bool>) {
    let mut ed: Vec<(u32, u32, u32)> = Vec::with_capacity(tris.len() * 3);
    for (t, tri) in tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            ed.push((a.min(b), a.max(b), t as u32));
        }
    }
    ed.sort_unstable();
    let mut boundary = vec![false; pos.len()];
    let mut feature = vec![false; pos.len()];
    let normal = |t: u32| {
        let v = tris[t as usize];
        let (a, b, c) = (pos[v[0] as usize], pos[v[1] as usize], pos[v[2] as usize]);
        (b - a).cross(c - a).normalize_or_zero()
    };
    let mut i = 0;
    while i < ed.len() {
        let mut j = i;
        while j < ed.len() && ed[j].0 == ed[i].0 && ed[j].1 == ed[i].1 {
            j += 1;
        }
        let (u, v) = (ed[i].0 as usize, ed[i].1 as usize);
        match j - i {
            1 => {
                boundary[u] = true;
                boundary[v] = true;
            }
            2 => {
                if normal(ed[i].2).dot(normal(ed[i + 1].2)) < cos_crease {
                    feature[u] = true;
                    feature[v] = true;
                }
            }
            _ => {
                feature[u] = true;
                feature[v] = true;
            }
        }
        i = j;
    }
    (boundary, feature)
}

/// Signed volume enclosed by an oriented triangle list (zero-centred sum).
pub fn signed_volume(pos: &[DVec3], tris: &[[u32; 3]]) -> f64 {
    tris.iter()
        .map(|t| {
            let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

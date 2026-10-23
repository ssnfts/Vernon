//! Mesh topology for flattening: Euler characteristic, boundary loops,
//! shortest paths and vertex-duplicating seam cuts.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use glam::DVec3;
use vernon_core::{AttribArray, Geometry, Prim};

use crate::util::Dist;

#[derive(Clone, Debug, Default)]
pub struct TopoInfo {
    pub vertices: usize,
    pub edges: usize,
    pub faces: usize,
    pub euler: i64,
    pub loops: Vec<Vec<u32>>,
    pub non_manifold_edges: usize,
}

impl TopoInfo {
    /// One boundary loop, Euler characteristic 1, manifold.
    pub fn is_disk(&self) -> bool {
        self.loops.len() == 1 && self.euler == 1 && self.non_manifold_edges == 0
    }
}

pub fn ekey(a: u32, b: u32) -> (u32, u32) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

fn is_face(p: &Prim) -> bool {
    p.closed && p.vertices.len() >= 3
}

pub fn analyze(g: &Geometry) -> TopoInfo {
    let mut edges: BTreeMap<(u32, u32), (u32, u32, u32)> = BTreeMap::new();
    let mut verts = BTreeSet::new();
    let mut faces = 0usize;
    for p in g.prims.iter().filter(|p| is_face(p)) {
        faces += 1;
        let n = p.vertices.len();
        for s in 0..n {
            let (a, b) = (p.vertices[s], p.vertices[(s + 1) % n]);
            verts.insert(a);
            edges.entry(ekey(a, b)).or_insert((0, a, b)).0 += 1;
        }
    }
    let mut non_manifold = 0;
    let mut next: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for &(c, a, b) in edges.values() {
        if c == 1 {
            next.entry(a).or_default().push(b);
        } else if c > 2 {
            non_manifold += 1;
        }
    }
    let mut loops = Vec::new();
    while let Some(s) = next.iter().find(|(_, v)| !v.is_empty()).map(|(k, _)| *k) {
        let mut lp = vec![s];
        let mut cur = s;
        loop {
            let nxt = match next.get_mut(&cur) {
                Some(v) if !v.is_empty() => v.remove(0),
                _ => break,
            };
            if nxt == s {
                break;
            }
            lp.push(nxt);
            cur = nxt;
        }
        loops.push(lp);
    }
    TopoInfo {
        vertices: verts.len(),
        edges: edges.len(),
        faces,
        euler: verts.len() as i64 - edges.len() as i64 + faces as i64,
        loops,
        non_manifold_edges: non_manifold,
    }
}

/// Vertex adjacency with 3-D edge lengths.
pub fn adjacency(g: &Geometry) -> Vec<Vec<(u32, f64)>> {
    let mut set: BTreeSet<(u32, u32)> = BTreeSet::new();
    for p in g.prims.iter().filter(|p| is_face(p)) {
        let n = p.vertices.len();
        for s in 0..n {
            set.insert(ekey(p.vertices[s], p.vertices[(s + 1) % n]));
        }
    }
    let mut adj = vec![Vec::new(); g.points.len()];
    for (a, b) in set {
        let l = g.points[a as usize].distance(g.points[b as usize]);
        adj[a as usize].push((b, l));
        adj[b as usize].push((a, l));
    }
    adj
}

pub struct DijkstraOut {
    pub dist: Vec<f64>,
    pub parent: Vec<u32>,
    /// First settled vertex (other than the source) accepted by `stop`.
    pub hit: Option<u32>,
}

pub fn dijkstra(adj: &[Vec<(u32, f64)>], src: u32, stop: &dyn Fn(u32) -> bool) -> DijkstraOut {
    let n = adj.len();
    let mut dist = vec![f64::INFINITY; n];
    let mut parent = vec![u32::MAX; n];
    let mut heap = BinaryHeap::new();
    dist[src as usize] = 0.0;
    heap.push(Reverse((Dist(0.0), src)));
    let mut hit = None;
    while let Some(Reverse((Dist(d), u))) = heap.pop() {
        if d > dist[u as usize] {
            continue;
        }
        if u != src && stop(u) {
            hit = Some(u);
            break;
        }
        for &(v, w) in &adj[u as usize] {
            let nd = d + w;
            if nd < dist[v as usize] {
                dist[v as usize] = nd;
                parent[v as usize] = u;
                heap.push(Reverse((Dist(nd), v)));
            }
        }
    }
    DijkstraOut { dist, parent, hit }
}

pub fn path_to(parent: &[u32], src: u32, target: u32) -> Vec<u32> {
    let mut p = vec![target];
    let mut cur = target;
    while cur != src {
        cur = parent[cur as usize];
        if cur == u32::MAX {
            return vec![];
        }
        p.push(cur);
    }
    p.reverse();
    p
}

struct Uf(Vec<usize>);

impl Uf {
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.0[hi] = lo;
        }
    }
}

/// Cut the mesh along `seam` edges by duplicating vertices. Point
/// attributes are copied to the duplicates and the point attribute
/// `src_point` (Int) records the original point of every new one.
/// Points not used by any face are dropped.
pub fn cut_along(g: &Geometry, seam: &BTreeSet<(u32, u32)>) -> Geometry {
    let mut offs = Vec::with_capacity(g.prims.len());
    let mut tot = 0usize;
    for p in &g.prims {
        offs.push(tot);
        tot += p.vertices.len();
    }
    let mut occ: BTreeMap<(u32, u32), Vec<(usize, usize)>> = BTreeMap::new();
    for (pi, p) in g.prims.iter().enumerate().filter(|(_, p)| is_face(p)) {
        let n = p.vertices.len();
        for s in 0..n {
            occ.entry(ekey(p.vertices[s], p.vertices[(s + 1) % n]))
                .or_default()
                .push((pi, s));
        }
    }
    let mut uf = Uf((0..tot).collect());
    for (&(a, b), list) in &occ {
        if list.len() != 2 || seam.contains(&(a, b)) {
            continue;
        }
        let corner = |(pi, s): (usize, usize), v: u32| -> usize {
            let n = g.prims[pi].vertices.len();
            if g.prims[pi].vertices[s] == v {
                offs[pi] + s
            } else {
                offs[pi] + (s + 1) % n
            }
        };
        uf.union(corner(list[0], a), corner(list[1], a));
        uf.union(corner(list[0], b), corner(list[1], b));
    }
    let mut new_of_root = vec![u32::MAX; tot];
    let mut src: Vec<u32> = Vec::new();
    let mut rep = vec![u32::MAX; g.points.len()];
    let mut vmap: Vec<Vec<u32>> = g.prims.iter().map(|p| vec![0; p.vertices.len()]).collect();
    for (pi, p) in g.prims.iter().enumerate().filter(|(_, p)| is_face(p)) {
        for (s, &pv) in p.vertices.iter().enumerate() {
            let r = uf.find(offs[pi] + s);
            if new_of_root[r] == u32::MAX {
                new_of_root[r] = src.len() as u32;
                src.push(pv);
            }
            let id = new_of_root[r];
            vmap[pi][s] = id;
            if rep[pv as usize] == u32::MAX {
                rep[pv as usize] = id;
            }
        }
    }
    for (pi, p) in g.prims.iter().enumerate().filter(|(_, p)| !is_face(p)) {
        for (s, &v) in p.vertices.iter().enumerate() {
            if rep[v as usize] == u32::MAX {
                rep[v as usize] = src.len() as u32;
                src.push(v);
            }
            vmap[pi][s] = rep[v as usize];
        }
    }
    let src_idx: Vec<usize> = src.iter().map(|&v| v as usize).collect();
    let mut out = Geometry::new();
    out.points = src_idx.iter().map(|&v| g.points[v]).collect();
    out.point_attribs = g.point_attribs.gather_all(&src_idx);
    if !out.point_attribs.contains("src_point") {
        out.point_attribs.set(
            "src_point",
            AttribArray::Int(src.iter().map(|&v| v as i64).collect()),
        );
    }
    out.prims = g
        .prims
        .iter()
        .enumerate()
        .map(|(pi, p)| Prim {
            vertices: vmap[pi].clone(),
            closed: p.closed,
        })
        .collect();
    out.prim_attribs = g.prim_attribs.clone();
    out.detail_attribs = g.detail_attribs.clone();
    out.prim_groups = g.prim_groups.clone();
    for (name, idx) in &g.point_groups {
        let set: BTreeSet<u32> = idx.iter().copied().collect();
        let v: Vec<u32> = (0..src.len() as u32)
            .filter(|&i| set.contains(&src[i as usize]))
            .collect();
        out.point_groups.insert(name.clone(), v);
    }
    out
}

fn argmax_finite(d: &[f64]) -> Option<u32> {
    let mut best: Option<(f64, usize)> = None;
    for (i, &x) in d.iter().enumerate() {
        if x.is_finite() && !matches!(best, Some((bv, _)) if x <= bv) {
            best = Some((x, i));
        }
    }
    best.map(|(_, i)| i as u32)
}

/// Cut a mesh into a topological disk with shortest-path seams: loops are
/// joined pairwise, and a closed genus-0 mesh is slit between two far-apart
/// vertices. Returns the cut mesh and the seam polylines (3-D positions).
pub fn auto_cut(g: &Geometry) -> Result<(Geometry, Vec<Vec<DVec3>>), String> {
    let mut cur = g.clone();
    let mut paths: Vec<Vec<DVec3>> = Vec::new();
    for _ in 0..16 {
        let t = analyze(&cur);
        if t.faces == 0 {
            return Err("the mesh has no faces".into());
        }
        if t.is_disk() {
            return Ok((cur, paths));
        }
        if t.non_manifold_edges > 0 {
            return Err(format!(
                "{} non-manifold edges: clean the mesh first",
                t.non_manifold_edges
            ));
        }
        let adj = adjacency(&cur);
        let path: Vec<u32> = if t.loops.len() >= 2 {
            let other: BTreeSet<u32> = t.loops[1..].iter().flatten().copied().collect();
            let s = t.loops[0][0];
            let d = dijkstra(&adj, s, &|v| other.contains(&v));
            let hit = d.hit.ok_or("boundary loops are not connected")?;
            path_to(&d.parent, s, hit)
        } else if t.loops.is_empty() && t.euler == 2 {
            let start = (0..cur.points.len() as u32)
                .find(|&v| !adj[v as usize].is_empty())
                .ok_or("no edges")?;
            let d0 = dijkstra(&adj, start, &|_| false);
            let a = argmax_finite(&d0.dist).ok_or("empty mesh")?;
            let d1 = dijkstra(&adj, a, &|_| false);
            let b = argmax_finite(&d1.dist).ok_or("empty mesh")?;
            path_to(&d1.parent, a, b)
        } else {
            return Err(format!(
                "Euler characteristic {} with {} boundary loops: higher genus needs explicit seam polylines",
                t.euler,
                t.loops.len()
            ));
        };
        if path.len() < 2 {
            return Err("could not find a seam path".into());
        }
        let edges: BTreeSet<(u32, u32)> = path.windows(2).map(|w| ekey(w[0], w[1])).collect();
        paths.push(path.iter().map(|&v| cur.points[v as usize]).collect());
        cur = cut_along(&cur, &edges);
    }
    Err("could not reach disk topology".into())
}

/// Edges of the shortest paths joining consecutive polyline vertices,
/// after snapping each to the nearest mesh vertex within `tol`.
pub fn seam_from_polylines(
    g: &Geometry,
    seam: &Geometry,
    tol: f64,
) -> Result<BTreeSet<(u32, u32)>, String> {
    let adj = adjacency(g);
    let used: Vec<u32> = (0..g.points.len() as u32)
        .filter(|&v| !adj[v as usize].is_empty())
        .collect();
    if used.is_empty() {
        return Err("the mesh has no edges".into());
    }
    let snap = |p: DVec3| -> Result<u32, String> {
        let (mut best, mut bd) = (used[0], f64::INFINITY);
        for &v in &used {
            let d = g.points[v as usize].distance(p);
            if d < bd {
                bd = d;
                best = v;
            }
        }
        if bd > tol {
            Err(format!(
                "a seam vertex is {bd:.4} mm from the mesh (tolerance {tol} mm)"
            ))
        } else {
            Ok(best)
        }
    };
    let mut edges = BTreeSet::new();
    for prim in &seam.prims {
        let ids: Vec<u32> = prim
            .vertices
            .iter()
            .map(|&i| snap(seam.points[i as usize]))
            .collect::<Result<_, _>>()?;
        for w in ids.windows(2) {
            if w[0] == w[1] {
                continue;
            }
            let d = dijkstra(&adj, w[0], &|v| v == w[1]);
            let hit = d
                .hit
                .ok_or("two seam vertices are not connected on the mesh")?;
            for e in path_to(&d.parent, w[0], hit).windows(2) {
                edges.insert(ekey(e[0], e[1]));
            }
        }
    }
    Ok(edges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vernon_core::DVec3;

    /// Triangulated grid of `nu x nv` quads in the XY plane.
    pub fn grid(nu: usize, nv: usize) -> Geometry {
        let mut pts = Vec::new();
        for j in 0..=nv {
            for i in 0..=nu {
                pts.push(DVec3::new(i as f64, j as f64, 0.0));
            }
        }
        let id = |i: usize, j: usize| (j * (nu + 1) + i) as u32;
        let mut faces = Vec::new();
        for j in 0..nv {
            for i in 0..nu {
                faces.push(vec![id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
                faces.push(vec![id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
            }
        }
        Geometry::from_polygons(pts, faces)
    }

    #[test]
    fn grid_is_a_disk() {
        let t = analyze(&grid(4, 3));
        assert!(t.is_disk());
        assert_eq!(t.euler, 1);
        assert_eq!(t.loops[0].len(), 14);
    }

    #[test]
    fn closed_cube_cuts_to_a_disk() {
        let mut g = Geometry::make_box(DVec3::splat(10.0));
        // triangulate the quads
        let mut tris = Vec::new();
        for p in &g.prims {
            let v = &p.vertices;
            tris.push(vec![v[0], v[1], v[2]]);
            tris.push(vec![v[0], v[2], v[3]]);
        }
        g = Geometry::from_polygons(g.points.clone(), tris);
        let t = analyze(&g);
        assert_eq!((t.euler, t.loops.len()), (2, 0));
        let (cut, paths) = auto_cut(&g).unwrap();
        let t2 = analyze(&cut);
        assert!(t2.is_disk(), "{t2:?}");
        assert_eq!(paths.len(), 1);
        assert!(cut.point_attribs.ints("src_point").is_ok());
        cut.validate().unwrap();
    }

    #[test]
    fn annulus_is_joined_by_one_cut() {
        // a ring of quads around the Z axis (open tube): two loops
        let n = 12;
        let mut pts = Vec::new();
        for k in 0..2 {
            for i in 0..n {
                let a = i as f64 / n as f64 * std::f64::consts::TAU;
                pts.push(DVec3::new(a.cos() * 10.0, a.sin() * 10.0, k as f64 * 5.0));
            }
        }
        let mut faces = Vec::new();
        for i in 0..n {
            let j = (i + 1) % n;
            faces.push(vec![i as u32, j as u32, (n + j) as u32]);
            faces.push(vec![i as u32, (n + j) as u32, (n + i) as u32]);
        }
        let g = Geometry::from_polygons(pts, faces);
        let t = analyze(&g);
        assert_eq!((t.euler, t.loops.len()), (0, 2));
        let (cut, _) = auto_cut(&g).unwrap();
        assert!(analyze(&cut).is_disk());
    }
}

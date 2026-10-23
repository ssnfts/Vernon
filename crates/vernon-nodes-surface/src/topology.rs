//! Welded mesh topology: vertex weld, edge map, boundary / non-manifold
//! counts, orientation repair, signed volume and triangle adjacency.

use std::collections::HashMap;

use glam::DVec3;
use vernon_core::Geometry;

/// A welded polygon soup.
#[derive(Debug, Clone, Default)]
pub struct Welded {
    pub points: Vec<DVec3>,
    pub faces: Vec<Vec<u32>>,
    /// Source primitive index of every face.
    pub src_prim: Vec<usize>,
    /// Source point index -> welded point index.
    pub map: Vec<u32>,
}

/// Weld coincident points (within `tol`) and drop degenerate faces.
pub fn weld(geo: &Geometry, tol: f64) -> Welded {
    let max_abs = geo.points.iter().map(|p| p.abs().max_element()).fold(0.0, f64::max);
    let tol = tol.max(max_abs * 1e-13).max(1e-12);
    let cell = |p: DVec3| ((p.x / tol).floor() as i64, (p.y / tol).floor() as i64, (p.z / tol).floor() as i64);
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    let mut points: Vec<DVec3> = Vec::new();
    let mut map = Vec::with_capacity(geo.points.len());
    for &p in &geo.points {
        let (cx, cy, cz) = cell(p);
        let mut found = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(v) = grid.get(&(cx.wrapping_add(dx), cy.wrapping_add(dy), cz.wrapping_add(dz))) {
                        for &i in v {
                            if (points[i as usize] - p).length() <= tol {
                                found = Some(i);
                                break 'search;
                            }
                        }
                    }
                }
            }
        }
        let idx = match found {
            Some(i) => i,
            None => {
                let i = points.len() as u32;
                points.push(p);
                grid.entry((cx, cy, cz)).or_default().push(i);
                i
            }
        };
        map.push(idx);
    }
    let mut faces = Vec::new();
    let mut src_prim = Vec::new();
    for (pi, prim) in geo.prims.iter().enumerate() {
        if !prim.closed || prim.vertices.len() < 3 {
            continue;
        }
        let mut ring: Vec<u32> = Vec::with_capacity(prim.vertices.len());
        for &v in &prim.vertices {
            let w = map[v as usize];
            if ring.last() != Some(&w) {
                ring.push(w);
            }
        }
        while ring.len() > 1 && ring.first() == ring.last() {
            ring.pop();
        }
        // remove repeated vertices anywhere (pinched polygons)
        let mut seen = std::collections::HashSet::new();
        if ring.iter().any(|v| !seen.insert(*v)) {
            continue;
        }
        if ring.len() >= 3 {
            faces.push(ring);
            src_prim.push(pi);
        }
    }
    Welded { points, faces, src_prim, map }
}

/// Edge key `(min, max)` and whether `a -> b` runs min to max.
pub fn edge_key(a: u32, b: u32) -> ((u32, u32), bool) {
    if a < b {
        ((a, b), true)
    } else {
        ((b, a), false)
    }
}

/// Map from undirected edge to the faces using it and the direction each uses.
pub fn edge_map(faces: &[Vec<u32>]) -> HashMap<(u32, u32), Vec<(u32, bool)>> {
    let mut m: HashMap<(u32, u32), Vec<(u32, bool)>> = HashMap::new();
    for (fi, f) in faces.iter().enumerate() {
        for i in 0..f.len() {
            let (k, fwd) = edge_key(f[i], f[(i + 1) % f.len()]);
            m.entry(k).or_default().push((fi as u32, fwd));
        }
    }
    m
}

/// Topology statistics of a welded mesh.
#[derive(Debug, Clone, Default)]
pub struct TopoStats {
    pub vertices: usize,
    pub edges: usize,
    pub faces: usize,
    pub boundary_edges: usize,
    pub nonmanifold_edges: usize,
    /// Interior edges whose two faces run the same way (bad winding).
    pub inconsistent_edges: usize,
    pub euler_char: i64,
    pub components: usize,
    /// Closed, 2-manifold and consistently oriented.
    pub closed: bool,
    /// Sum of the signed volumes of the closed components.
    pub volume: f64,
}

pub fn stats(w: &Welded) -> TopoStats {
    let em = edge_map(&w.faces);
    let mut used = vec![false; w.points.len()];
    for f in &w.faces {
        for &v in f {
            used[v as usize] = true;
        }
    }
    let vertices = used.iter().filter(|&&u| u).count();
    let mut s = TopoStats { vertices, edges: em.len(), faces: w.faces.len(), ..Default::default() };
    for fs in em.values() {
        match fs.len() {
            1 => s.boundary_edges += 1,
            2 => {
                if fs[0].1 == fs[1].1 {
                    s.inconsistent_edges += 1;
                }
            }
            _ => s.nonmanifold_edges += 1,
        }
    }
    s.euler_char = vertices as i64 - s.edges as i64 + s.faces as i64;
    let comps = components(w, &em);
    s.components = comps.len();
    s.closed = s.boundary_edges == 0 && s.nonmanifold_edges == 0 && s.inconsistent_edges == 0 && s.faces > 0;
    for c in &comps {
        if component_is_closed(&w.faces, &em, c) {
            s.volume += signed_volume(&w.points, &w.faces, c);
        }
    }
    s
}

fn component_is_closed(faces: &[Vec<u32>], em: &HashMap<(u32, u32), Vec<(u32, bool)>>, comp: &[usize]) -> bool {
    comp.iter().all(|&fi| {
        let f = &faces[fi];
        (0..f.len()).all(|i| em[&edge_key(f[i], f[(i + 1) % f.len()]).0].len() == 2)
    })
}

/// Face-connected components (through shared edges).
fn components(w: &Welded, em: &HashMap<(u32, u32), Vec<(u32, bool)>>) -> Vec<Vec<usize>> {
    let n = w.faces.len();
    let mut seen = vec![false; n];
    let mut out = Vec::new();
    for s in 0..n {
        if seen[s] {
            continue;
        }
        let mut comp = vec![s];
        seen[s] = true;
        let mut head = 0;
        while head < comp.len() {
            let f = comp[head];
            head += 1;
            let ring = &w.faces[f];
            for i in 0..ring.len() {
                let key = edge_key(ring[i], ring[(i + 1) % ring.len()]).0;
                for &(g, _) in &em[&key] {
                    if !seen[g as usize] {
                        seen[g as usize] = true;
                        comp.push(g as usize);
                    }
                }
            }
        }
        comp.sort_unstable();
        out.push(comp);
    }
    out
}

/// Signed volume of the faces in `comp` (outward-wound closed surface gives > 0).
pub fn signed_volume(points: &[DVec3], faces: &[Vec<u32>], comp: &[usize]) -> f64 {
    let mut o = DVec3::ZERO;
    let mut cnt = 0.0;
    for &fi in comp {
        for &v in &faces[fi] {
            o += points[v as usize];
            cnt += 1.0;
        }
    }
    if cnt > 0.0 {
        o /= cnt;
    }
    let mut vol = 0.0;
    for &fi in comp {
        let f = &faces[fi];
        let a = points[f[0] as usize] - o;
        for i in 1..f.len() - 1 {
            let b = points[f[i] as usize] - o;
            let c = points[f[i + 1] as usize] - o;
            vol += a.dot(b.cross(c)) / 6.0;
        }
    }
    vol
}

/// Signed volume of a whole polygon mesh given as a plain `Geometry`.
pub fn geometry_volume(geo: &Geometry) -> f64 {
    let faces: Vec<Vec<u32>> = geo.prims.iter().filter(|p| p.closed && p.vertices.len() >= 3).map(|p| p.vertices.clone()).collect();
    let all: Vec<usize> = (0..faces.len()).collect();
    signed_volume(&geo.points, &faces, &all)
}

fn edge_dir(face: &[u32], a: u32, b: u32) -> Option<bool> {
    let n = face.len();
    for i in 0..n {
        let (x, y) = (face[i], face[(i + 1) % n]);
        if x == a && y == b {
            return Some(true);
        }
        if x == b && y == a {
            return Some(false);
        }
    }
    None
}

/// Result of [`orient`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OrientResult {
    /// Faces reversed to make neighbours agree.
    pub propagated_flips: usize,
    /// Closed components reversed because their volume was negative.
    pub flipped_components: usize,
}

/// Make winding consistent across manifold edges (BFS), then reverse every
/// closed component whose signed volume is negative.
pub fn orient(w: &mut Welded) -> OrientResult {
    let mut res = OrientResult::default();
    let em = edge_map(&w.faces);
    let n = w.faces.len();
    let mut seen = vec![false; n];
    let mut comps: Vec<Vec<usize>> = Vec::new();
    for s in 0..n {
        if seen[s] {
            continue;
        }
        seen[s] = true;
        let mut queue = vec![s];
        let mut head = 0;
        while head < queue.len() {
            let f = queue[head];
            head += 1;
            let ring = w.faces[f].clone();
            for i in 0..ring.len() {
                let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                let fs = &em[&edge_key(a, b).0];
                if fs.len() != 2 {
                    continue;
                }
                for &(g, _) in fs {
                    let g = g as usize;
                    if g == f || seen[g] {
                        continue;
                    }
                    seen[g] = true;
                    if edge_dir(&w.faces[g], a, b) == Some(true) {
                        w.faces[g].reverse();
                        res.propagated_flips += 1;
                    }
                    queue.push(g);
                }
            }
        }
        comps.push(queue);
    }
    let em2 = edge_map(&w.faces);
    for comp in &comps {
        if component_is_closed(&w.faces, &em2, comp) && signed_volume(&w.points, &w.faces, comp) < 0.0 {
            for &f in comp {
                w.faces[f].reverse();
            }
            res.flipped_components += 1;
        }
    }
    res
}

/// Per-triangle neighbours through each edge `(v[i], v[i+1])`; `None` at
/// boundaries and non-manifold edges.
pub fn tri_adjacency(tris: &[[u32; 3]]) -> Vec<[Option<u32>; 3]> {
    let mut m: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        for i in 0..3 {
            m.entry(edge_key(t[i], t[(i + 1) % 3]).0).or_default().push(ti as u32);
        }
    }
    tris.iter()
        .enumerate()
        .map(|(ti, t)| {
            let mut out = [None; 3];
            for i in 0..3 {
                let v = &m[&edge_key(t[i], t[(i + 1) % 3]).0];
                if v.len() == 2 {
                    out[i] = v.iter().copied().find(|&x| x as usize != ti);
                }
            }
            out
        })
        .collect()
}

/// Boundary loops (directed as the single adjacent face traverses them).
pub fn boundary_loops(faces: &[Vec<u32>]) -> Vec<Vec<u32>> {
    let em = edge_map(faces);
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    for f in faces {
        for i in 0..f.len() {
            let (a, b) = (f[i], f[(i + 1) % f.len()]);
            if em[&edge_key(a, b).0].len() == 1 {
                next.entry(a).or_default().push(b);
            }
        }
    }
    let mut starts: Vec<u32> = next.keys().copied().collect();
    starts.sort_unstable();
    let mut loops = Vec::new();
    for s in starts {
        while let Some(list) = next.get_mut(&s) {
            if list.is_empty() {
                break;
            }
            let mut ring = vec![s];
            let mut cur = s;
            while let Some(nx) = next.get_mut(&cur).and_then(|l| l.pop()) {
                if nx == s {
                    break;
                }
                ring.push(nx);
                cur = nx;
                if ring.len() > faces.len() * 4 + 8 {
                    break;
                }
            }
            if ring.len() >= 3 {
                loops.push(ring);
            }
        }
    }
    loops
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxg() -> Geometry {
        Geometry::make_box(DVec3::new(10.0, 20.0, 30.0))
    }

    #[test]
    fn box_is_closed_with_exact_volume() {
        let w = weld(&boxg(), 1e-6);
        let s = stats(&w);
        assert_eq!((s.vertices, s.edges, s.faces), (8, 12, 6));
        assert!(s.closed);
        assert_eq!(s.euler_char, 2);
        assert!((s.volume - 6000.0).abs() < 1e-9);
    }

    #[test]
    fn reversed_box_is_repaired() {
        let mut g = boxg();
        for p in &mut g.prims {
            p.vertices.reverse();
        }
        let mut w = weld(&g, 1e-6);
        assert!((stats(&w).volume + 6000.0).abs() < 1e-9);
        let r = orient(&mut w);
        assert_eq!(r.flipped_components, 1);
        assert!((stats(&w).volume - 6000.0).abs() < 1e-9);
    }

    #[test]
    fn mixed_winding_is_propagated() {
        let mut g = boxg();
        g.prims[2].vertices.reverse();
        g.prims[4].vertices.reverse();
        let mut w = weld(&g, 1e-6);
        assert!(stats(&w).inconsistent_edges > 0);
        let r = orient(&mut w);
        assert!(r.propagated_flips > 0);
        let s = stats(&w);
        assert!(s.closed && (s.volume - 6000.0).abs() < 1e-9);
    }

    #[test]
    fn missing_face_gives_four_boundary_edges() {
        let mut g = boxg();
        g.prims.remove(1);
        let w = weld(&g, 1e-6);
        let s = stats(&w);
        assert_eq!(s.boundary_edges, 4);
        assert!(!s.closed);
        let loops = boundary_loops(&w.faces);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].len(), 4);
    }

    #[test]
    fn weld_merges_duplicates() {
        let mut g = Geometry::new();
        for _ in 0..2 {
            for p in Geometry::make_box(DVec3::ONE).points {
                g.points.push(p);
            }
        }
        g.prims.push(vernon_core::Prim::polygon(vec![0, 1, 2, 3]));
        g.prims.push(vernon_core::Prim::polygon(vec![8, 9, 10, 11]));
        let w = weld(&g, 1e-6);
        assert_eq!(w.points.len(), 8);
        assert_eq!(w.faces.len(), 2);
    }
}

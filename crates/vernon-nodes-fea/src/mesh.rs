//! Tetrahedral solid meshes stored as a [`Geometry`]: points are the nodes,
//! each primitive is one element with 4 (C3D4) or 10 (C3D10) vertices in the
//! solver's node order. Deck node id = point index + 1, element id = prim
//! index + 1.

use std::collections::HashMap;

use glam::DVec3;
use vernon_core::{AttribArray, Geometry, Prim};

/// Local corner indices of the four faces (solver face labels S1..S4).
pub const FACES: [[usize; 3]; 4] = [[0, 1, 2], [0, 3, 1], [1, 3, 2], [2, 3, 0]];
/// Corner pairs of the six mid-edge nodes 5..10 of a quadratic tetrahedron.
pub const EDGES10: [(usize, usize); 6] = [(0, 1), (1, 2), (2, 0), (0, 3), (1, 3), (2, 3)];

/// Signed volume of a tetrahedron (positive for the solver's node order).
pub fn tet_volume(a: DVec3, b: DVec3, c: DVec3, d: DVec3) -> f64 {
    (b - a).cross(c - a).dot(d - a) / 6.0
}

/// Check that `g` is a pure C3D4/C3D10 mesh and return the vertex count.
pub fn element_size(g: &Geometry) -> Result<usize, String> {
    let first = g.prims.first().ok_or("the solid mesh has no elements")?;
    let n = first.vertices.len();
    if n != 4 && n != 10 {
        return Err(format!("elements must have 4 or 10 nodes, found {n}"));
    }
    if g.prims.iter().any(|p| p.vertices.len() != n || !p.closed) {
        return Err("mixed element types are not supported".into());
    }
    g.validate().map_err(|e| e.to_string())?;
    Ok(n)
}

pub fn corners(g: &Geometry, e: usize) -> [u32; 4] {
    let v = &g.prims[e].vertices;
    [v[0], v[1], v[2], v[3]]
}

pub fn corner_points(g: &Geometry, e: usize) -> [DVec3; 4] {
    let c = corners(g, e);
    [g.points[c[0] as usize], g.points[c[1] as usize], g.points[c[2] as usize], g.points[c[3] as usize]]
}

pub fn element_volume(g: &Geometry, e: usize) -> f64 {
    let p = corner_points(g, e);
    tet_volume(p[0], p[1], p[2], p[3])
}

/// Total volume (sum of absolute element volumes) in mm3.
pub fn mesh_volume(g: &Geometry) -> f64 {
    (0..g.prims.len()).map(|e| element_volume(g, e).abs()).sum()
}

pub fn element_centroid(g: &Geometry, e: usize) -> DVec3 {
    let p = corner_points(g, e);
    (p[0] + p[1] + p[2] + p[3]) * 0.25
}

/// Swap nodes 2 and 3 of every linear tetrahedron with negative volume.
/// Returns the number of elements fixed.
pub fn orient_positive(g: &mut Geometry) -> usize {
    let mut fixed = 0;
    for e in 0..g.prims.len() {
        if g.prims[e].vertices.len() == 4 && element_volume(g, e) < 0.0 {
            g.prims[e].vertices.swap(1, 2);
            fixed += 1;
        }
    }
    fixed
}

/// Add mid-edge nodes (straight-sided) to a linear tetrahedral mesh. Edges
/// are deduplicated by their sorted node pair. Elements get positive volume.
pub fn tet4_to_tet10(g: &Geometry) -> Result<Geometry, String> {
    if element_size(g)? != 4 {
        return Err("mesh is not a linear tetrahedral mesh".into());
    }
    let mut out = g.clone();
    orient_positive(&mut out);
    let mut mids: HashMap<(u32, u32), u32> = HashMap::new();
    for e in 0..out.prims.len() {
        let v = out.prims[e].vertices.clone();
        let mut full = v.clone();
        for (a, b) in EDGES10 {
            let key = (v[a].min(v[b]), v[a].max(v[b]));
            let id = *mids.entry(key).or_insert_with(|| {
                let p = (out.points[v[a] as usize] + out.points[v[b] as usize]) * 0.5;
                out.points.push(p);
                (out.points.len() - 1) as u32
            });
            full.push(id);
        }
        out.prims[e].vertices = full;
    }
    let n = out.points.len();
    out.point_attribs.resize_all(n);
    Ok(out)
}

/// Reorder a Gmsh second-order tetrahedron into the solver's order (the
/// last two mid-edge nodes are swapped).
pub fn gmsh_tet10_to_solver(v: &[u32; 10]) -> [u32; 10] {
    [v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[9], v[8]]
}

/// One boundary face of the solid mesh.
#[derive(Debug, Clone)]
pub struct BFace {
    pub elem: usize,
    /// Solver face label 1..4.
    pub face: u8,
    pub nodes: [u32; 3],
    pub area: f64,
    /// Outward unit normal.
    pub normal: DVec3,
    pub centroid: DVec3,
}

pub fn boundary_faces(g: &Geometry) -> Vec<BFace> {
    let mut seen: HashMap<[u32; 3], (usize, usize)> = HashMap::new();
    let mut count: HashMap<[u32; 3], u32> = HashMap::new();
    for e in 0..g.prims.len() {
        let c = corners(g, e);
        for (fi, f) in FACES.iter().enumerate() {
            let mut key = [c[f[0]], c[f[1]], c[f[2]]];
            key.sort_unstable();
            *count.entry(key).or_insert(0) += 1;
            seen.entry(key).or_insert((e, fi));
        }
    }
    let mut out: Vec<BFace> = count
        .iter()
        .filter(|(_, &n)| n == 1)
        .map(|(k, _)| {
            let (e, fi) = seen[k];
            let c = corners(g, e);
            let f = FACES[fi];
            let nodes = [c[f[0]], c[f[1]], c[f[2]]];
            let p = nodes.map(|i| g.points[i as usize]);
            let cr = (p[1] - p[0]).cross(p[2] - p[0]);
            let opposite = g.points[c[(0..4).find(|i| !f.contains(i)).unwrap_or(3)] as usize];
            let centroid = (p[0] + p[1] + p[2]) / 3.0;
            let mut normal = cr.normalize_or_zero();
            if normal.dot(opposite - centroid) > 0.0 {
                normal = -normal;
            }
            BFace { elem: e, face: fi as u8 + 1, nodes, area: 0.5 * cr.length(), normal, centroid }
        })
        .collect();
    out.sort_by_key(|f| (f.elem, f.face));
    out
}

/// Display surface: the boundary triangles with outward winding. Shares the
/// point list (and point attributes) of the solid mesh.
pub fn boundary_surface(g: &Geometry) -> Geometry {
    let faces = boundary_faces(g);
    let mut s = Geometry::new();
    s.points = g.points.clone();
    s.point_attribs = g.point_attribs.clone();
    let mut elem = Vec::with_capacity(faces.len());
    let mut face = Vec::with_capacity(faces.len());
    for f in &faces {
        let [a, b, c] = f.nodes;
        let p = [g.points[a as usize], g.points[b as usize], g.points[c as usize]];
        let tri = if (p[1] - p[0]).cross(p[2] - p[0]).dot(f.normal) >= 0.0 { vec![a, b, c] } else { vec![a, c, b] };
        s.prims.push(Prim::polygon(tri));
        elem.push(f.elem as i64);
        face.push(f.face as i64);
    }
    s.prim_attribs.set("elem", AttribArray::Int(elem));
    s.prim_attribs.set("face", AttribArray::Int(face));
    s.compute_normals();
    s
}

/// Quality numbers of one tetrahedron.
#[derive(Debug, Clone, Copy)]
pub struct TetQuality {
    pub volume: f64,
    pub min_dihedral_deg: f64,
    /// Longest edge divided by the smallest altitude (about 1.22 for a regular tetrahedron).
    pub aspect: f64,
    pub mean_edge: f64,
}

pub fn tet_quality(p: [DVec3; 4]) -> TetQuality {
    let volume = tet_volume(p[0], p[1], p[2], p[3]);
    let mut min_dih = f64::INFINITY;
    let mut longest = 0.0f64;
    let mut sum_edge = 0.0;
    for (a, b) in [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)] {
        let others: Vec<usize> = (0..4).filter(|&i| i != a && i != b).collect();
        let axis = (p[b] - p[a]).normalize_or_zero();
        let perp = |q: DVec3| {
            let r = q - p[a];
            r - axis * r.dot(axis)
        };
        let (u, w) = (perp(p[others[0]]), perp(p[others[1]]));
        let denom = u.length() * w.length();
        if denom > 0.0 {
            min_dih = min_dih.min((u.dot(w) / denom).clamp(-1.0, 1.0).acos().to_degrees());
        } else {
            min_dih = 0.0;
        }
        let l = (p[b] - p[a]).length();
        longest = longest.max(l);
        sum_edge += l;
    }
    let mut min_alt = f64::INFINITY;
    for (f, apex) in FACES.iter().zip([3usize, 2, 0, 1]) {
        let _ = apex;
        let area = 0.5 * (p[f[1]] - p[f[0]]).cross(p[f[2]] - p[f[0]]).length();
        if area > 0.0 {
            min_alt = min_alt.min(3.0 * volume.abs() / area);
        }
    }
    let aspect = if min_alt.is_finite() && min_alt > 0.0 { longest / min_alt } else { f64::INFINITY };
    TetQuality { volume, min_dihedral_deg: min_dih, aspect, mean_edge: sum_edge / 6.0 }
}

/// Distinct point indices of every boundary face, including mid-edge nodes
/// of quadratic elements.
pub fn boundary_node_set(g: &Geometry) -> Vec<u32> {
    let quad = g.prims.first().is_some_and(|p| p.vertices.len() == 10);
    let mut set: Vec<u32> = Vec::new();
    for f in boundary_faces(g) {
        set.extend(f.nodes);
        if quad {
            let v = &g.prims[f.elem].vertices;
            let loc = FACES[f.face as usize - 1];
            for (k, (a, b)) in EDGES10.iter().enumerate() {
                if loc.contains(a) && loc.contains(b) {
                    set.push(v[4 + k]);
                }
            }
        }
    }
    set.sort_unstable();
    set.dedup();
    set
}

/// Welded triangle list of a surface, with the number of open edges.
pub struct TriSurface {
    pub tris: Vec<[DVec3; 3]>,
    pub open_edges: usize,
    pub degenerate: usize,
}

pub fn tri_surface(g: &Geometry) -> TriSurface {
    let bb = g.bounds();
    let tol = (bb.size().length() * 1e-7).max(1e-12);
    let mut ids: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut weld = |p: DVec3| -> u32 {
        let k = ((p.x / tol).round() as i64, (p.y / tol).round() as i64, (p.z / tol).round() as i64);
        let n = ids.len() as u32;
        *ids.entry(k).or_insert(n)
    };
    let mut tris = Vec::new();
    let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
    let mut degenerate = 0;
    for (_, t) in g.triangles() {
        let p = t.map(|i| g.points[i as usize]);
        let w = [weld(p[0]), weld(p[1]), weld(p[2])];
        if w[0] == w[1] || w[1] == w[2] || w[0] == w[2] || (p[1] - p[0]).cross(p[2] - p[0]).length_squared() < 1e-24 {
            degenerate += 1;
            continue;
        }
        for (a, b) in [(w[0], w[1]), (w[1], w[2]), (w[2], w[0])] {
            let (key, dir) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
            *edges.entry(key).or_insert(0) += dir;
        }
        tris.push(p);
    }
    // A closed, consistently wound manifold has every edge used once in each direction.
    let mut counts: HashMap<(u32, u32), u32> = HashMap::new();
    for (_, t) in g.triangles() {
        let p = t.map(|i| g.points[i as usize]);
        let w = [weld(p[0]), weld(p[1]), weld(p[2])];
        if w[0] == w[1] || w[1] == w[2] || w[0] == w[2] {
            continue;
        }
        for (a, b) in [(w[0], w[1]), (w[1], w[2]), (w[2], w[0])] {
            *counts.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    let open_edges = counts.values().filter(|&&n| n != 2).count();
    TriSurface { tris, open_edges, degenerate }
}

/// Stair-stepped voxel tetrahedralisation of a closed surface (white-box
/// placeholder for a conformal mesher). Cells whose centre is inside the
/// surface are split into six conforming tetrahedra. Returns points and
/// linear tetrahedra with positive volume.
pub fn voxel_tets(tris: &[[DVec3; 3]], h: f64, max_cells: usize) -> Result<(Vec<DVec3>, Vec<[u32; 4]>), String> {
    if tris.is_empty() {
        return Err("the surface has no triangles".into());
    }
    let mut lo = DVec3::splat(f64::INFINITY);
    let mut hi = DVec3::splat(f64::NEG_INFINITY);
    for t in tris {
        for p in t {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
    }
    let size = hi - lo;
    if size.min_element() <= 0.0 {
        return Err("the surface is flat in one direction".into());
    }
    let mut h = h.max(1e-6);
    let dims = |h: f64| -> [usize; 3] { [0, 1, 2].map(|a| ((size[a] / h).ceil() as usize).max(1)) };
    while dims(h).iter().product::<usize>() > max_cells {
        h *= 1.1;
    }
    let [nx, ny, nz] = dims(h);
    let cell = DVec3::new(size.x / nx as f64, size.y / ny as f64, size.z / nz as f64);
    let jitter = DVec3::new(0.0, cell.y * 1.37e-3, cell.z * 0.61e-3);
    let mut columns: Vec<Vec<f64>> = vec![Vec::new(); ny * nz];
    for t in tris {
        let n = (t[1] - t[0]).cross(t[2] - t[0]);
        if n.x.abs() < 1e-14 * n.length().max(1e-300) {
            continue;
        }
        let (ymin, ymax) = (t.iter().map(|p| p.y).fold(f64::INFINITY, f64::min), t.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max));
        let (zmin, zmax) = (t.iter().map(|p| p.z).fold(f64::INFINITY, f64::min), t.iter().map(|p| p.z).fold(f64::NEG_INFINITY, f64::max));
        let j0 = (((ymin - lo.y) / cell.y - 0.5).floor().max(0.0)) as usize;
        let j1 = ((((ymax - lo.y) / cell.y) + 0.5).ceil() as usize).min(ny);
        let k0 = (((zmin - lo.z) / cell.z - 0.5).floor().max(0.0)) as usize;
        let k1 = ((((zmax - lo.z) / cell.z) + 0.5).ceil() as usize).min(nz);
        let area2 = n.x; // twice the signed area in the (y, z) plane
        for j in j0..j1 {
            let yc = lo.y + (j as f64 + 0.5) * cell.y + jitter.y;
            for k in k0..k1 {
                let zc = lo.z + (k as f64 + 0.5) * cell.z + jitter.z;
                let e = |a: DVec3, b: DVec3| (b.y - a.y) * (zc - a.z) - (b.z - a.z) * (yc - a.y);
                let (w0, w1, w2) = (e(t[1], t[2]), e(t[2], t[0]), e(t[0], t[1]));
                let inside = if area2 > 0.0 { w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 } else { w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0 };
                if inside {
                    let x = t[0].x - (n.y * (yc - t[0].y) + n.z * (zc - t[0].z)) / n.x;
                    columns[j * nz + k].push(x);
                }
            }
        }
    }
    let mut odd = 0usize;
    for c in &mut columns {
        c.sort_by(|a, b| a.total_cmp(b));
        if c.len() % 2 == 1 {
            odd += 1;
            c.pop();
        }
    }
    if odd * 200 > ny * nz {
        return Err(format!("the surface does not look closed ({odd} of {} ray columns crossed it an odd number of times)", ny * nz));
    }
    let node_dims = (nx + 1, ny + 1, nz + 1);
    let mut node_id = vec![u32::MAX; node_dims.0 * node_dims.1 * node_dims.2];
    let mut points: Vec<DVec3> = Vec::new();
    let mut tets: Vec<[u32; 4]> = Vec::new();
    let nidx = |i: usize, j: usize, k: usize| (i * node_dims.1 + j) * node_dims.2 + k;
    const PERMS: [[usize; 3]; 6] = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
    for j in 0..ny {
        for k in 0..nz {
            let col = &columns[j * nz + k];
            if col.is_empty() {
                continue;
            }
            for i in 0..nx {
                let xc = lo.x + (i as f64 + 0.5) * cell.x;
                let inside = col.chunks(2).any(|c| c.len() == 2 && xc > c[0] && xc < c[1]);
                if !inside {
                    continue;
                }
                let mut get = |di: usize, dj: usize, dk: usize| -> u32 {
                    let slot = &mut node_id[nidx(i + di, j + dj, k + dk)];
                    if *slot == u32::MAX {
                        *slot = points.len() as u32;
                        points.push(lo + DVec3::new((i + di) as f64 * cell.x, (j + dj) as f64 * cell.y, (k + dk) as f64 * cell.z));
                    }
                    *slot
                };
                for perm in PERMS {
                    let mut off = [0usize; 3];
                    let mut ids = [0u32; 4];
                    ids[0] = get(0, 0, 0);
                    for (s, &axis) in perm.iter().enumerate() {
                        off[axis] = 1;
                        ids[s + 1] = get(off[0], off[1], off[2]);
                    }
                    tets.push(ids);
                }
            }
        }
    }
    if tets.is_empty() {
        return Err("no cell centre lies inside the surface; reduce the edge length".into());
    }
    for t in &mut tets {
        if tet_volume(points[t[0] as usize], points[t[1] as usize], points[t[2] as usize], points[t[3] as usize]) < 0.0 {
            t.swap(1, 2);
        }
    }
    Ok((points, tets))
}

/// Build a linear tetrahedral `Geometry` from points and tets.
pub fn solid_from_tets(points: Vec<DVec3>, tets: &[[u32; 4]]) -> Geometry {
    let mut g = Geometry::from_points(points);
    g.prims = tets.iter().map(|t| Prim::polygon(t.to_vec())).collect();
    g.detail_attribs.set("elem_type", AttribArray::Str(vec!["C3D4".into()]));
    g
}

/// A box meshed with a regular grid of `n` cells per axis (six tets each),
/// centred on the origin. Used by tests and the white-box demos.
pub fn box_tets(size: DVec3, n: [usize; 3]) -> Geometry {
    let tris = box_tris(size);
    let h = (size.x / n[0] as f64).max(size.y / n[1] as f64).max(size.z / n[2] as f64);
    let (p, t) = voxel_tets(&tris, h * 0.999, usize::MAX).expect("box meshes");
    solid_from_tets(p, &t)
}

/// Twelve outward-wound triangles of a box centred on the origin.
pub fn box_tris(size: DVec3) -> Vec<[DVec3; 3]> {
    let g = Geometry::make_box(size);
    g.triangles().into_iter().map(|(_, t)| t.map(|i| g.points[i as usize])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g4_unit_tet_conversion() {
        let pts = vec![DVec3::ZERO, DVec3::new(10.0, 0.0, 0.0), DVec3::new(0.0, 10.0, 0.0), DVec3::new(0.0, 0.0, 10.0)];
        let g4 = solid_from_tets(pts, &[[0, 1, 2, 3]]);
        assert!((mesh_volume(&g4) - 166.6667).abs() < 1e-3);
        let g10 = tet4_to_tet10(&g4).unwrap();
        assert_eq!(g10.points.len(), 10);
        assert_eq!(g10.prims[0].vertices.len(), 10);
        assert!((mesh_volume(&g10) - 166.6667).abs() < 1e-3);
        let f = boundary_faces(&g10);
        assert_eq!(f.len(), 4);
        let f1 = f.iter().find(|f| f.face == 1).unwrap();
        assert!((f1.area - 50.0).abs() < 1e-9);
        // face 1 = nodes 1-2-3 (z = 0 plane), outward normal is -z
        assert!((f1.normal - DVec3::NEG_Z).length() < 1e-12);
        // mid-edge node 5 sits between corners 1 and 2
        let v = &g10.prims[0].vertices;
        assert!((g10.points[v[4] as usize] - DVec3::new(5.0, 0.0, 0.0)).length() < 1e-12);
        assert!((g10.points[v[9] as usize] - DVec3::new(0.0, 5.0, 5.0)).length() < 1e-12);
    }

    #[test]
    fn negative_tets_are_flipped_and_edges_are_shared() {
        let pts = vec![DVec3::ZERO, DVec3::X, DVec3::Y, DVec3::Z, DVec3::new(1.0, 1.0, 1.0)];
        let mut g = solid_from_tets(pts, &[[0, 2, 1, 3], [1, 2, 3, 4]]);
        assert!(orient_positive(&mut g) >= 1);
        assert!((0..2).all(|e| element_volume(&g, e) > 0.0));
        let g10 = tet4_to_tet10(&g).unwrap();
        // 5 corner nodes + 9 distinct edges (the shared face contributes 3 edges once)
        assert_eq!(g10.points.len(), 5 + 9);
    }

    #[test]
    fn gmsh_order_is_converted() {
        // corners 0..3, then mid-edge nodes in Gmsh order: 01 12 20 30 32 31
        let gm: [u32; 10] = [0, 1, 2, 3, 10, 11, 12, 13, 14, 15];
        let s = gmsh_tet10_to_solver(&gm);
        // solver order: 01 12 20 03 13 23
        assert_eq!(&s[4..], &[10, 11, 12, 13, 15, 14]);
    }

    #[test]
    fn voxel_mesh_fills_a_box_exactly() {
        let g = box_tets(DVec3::new(40.0, 20.0, 10.0), [8, 4, 2]);
        assert!((mesh_volume(&g) - 8000.0).abs() < 1e-6);
        assert_eq!(g.prims.len(), 8 * 4 * 2 * 6);
        let faces = boundary_faces(&g);
        let area: f64 = faces.iter().map(|f| f.area).sum();
        assert!((area - 2.0 * (800.0 + 400.0 + 200.0)).abs() < 1e-6);
        assert!(faces.iter().all(|f| f.normal.length() > 0.999));
        let q = tet_quality(corner_points(&g, 0));
        assert!(q.min_dihedral_deg > 44.0 && q.volume > 0.0);
        let s = boundary_surface(&g);
        s.validate().unwrap();
        assert_eq!(s.prims.len(), faces.len());
    }

    #[test]
    fn open_surface_is_detected() {
        let mut g = Geometry::make_box(DVec3::splat(10.0));
        g.prims.pop();
        let s = tri_surface(&g);
        assert!(s.open_edges > 0);
        assert_eq!(tri_surface(&Geometry::make_box(DVec3::splat(10.0))).open_edges, 0);
    }

    #[test]
    fn regular_tet_quality() {
        let a = 2.0f64.sqrt();
        let p = [DVec3::new(1.0, 1.0, 1.0), DVec3::new(1.0, -1.0, -1.0), DVec3::new(-1.0, 1.0, -1.0), DVec3::new(-1.0, -1.0, 1.0)];
        let q = tet_quality(p);
        assert!((q.min_dihedral_deg - 70.5288).abs() < 1e-3);
        assert!((q.aspect - 1.2247).abs() < 1e-3);
        assert!((q.mean_edge - 2.0 * a).abs() < 1e-9);
    }
}

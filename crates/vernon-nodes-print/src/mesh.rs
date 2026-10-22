//! Mesh bookkeeping shared by the print-stage nodes: parts, volumes, edge
//! statistics, extraction and merging. Everything here is pure Rust on top of
//! `vernon_core::Geometry`.

use std::collections::{BTreeSet, HashMap};

use glam::DVec3;
use vernon_core::{Aabb, AttribArray, Geometry};

/// Name of the prim Int attribute that numbers the printable parts.
pub const PART: &str = "part";
/// Prim Int attribute: id (plane index + 1) of the cap a face belongs to, 0 for body faces.
pub const CAP: &str = "cap";

/// Part id of every primitive (all zeros when the attribute is missing).
pub fn part_ids(geo: &Geometry) -> Vec<i64> {
    match geo.prim_attribs.ints(PART) {
        Ok(v) => v.to_vec(),
        Err(_) => vec![0; geo.prims.len()],
    }
}

/// Sorted distinct part ids that own at least one closed primitive.
pub fn unique_parts(geo: &Geometry) -> Vec<i64> {
    let ids = part_ids(geo);
    let set: BTreeSet<i64> = geo
        .prims
        .iter()
        .enumerate()
        .filter(|(_, p)| p.closed)
        .map(|(i, _)| ids[i])
        .collect();
    set.into_iter().collect()
}

pub fn set_int_prim_attr(geo: &mut Geometry, name: &str, values: Vec<i64>) {
    debug_assert_eq!(values.len(), geo.prims.len());
    geo.prim_attribs.set(name, AttribArray::Int(values));
}

/// New geometry made of the listed primitives with unused points dropped.
/// Point and primitive attributes follow; groups are not carried over.
pub fn extract_prims(geo: &Geometry, prims: &[usize]) -> Geometry {
    let mut remap: HashMap<u32, u32> = HashMap::new();
    let mut point_src: Vec<usize> = Vec::new();
    let mut out = Geometry::new();
    for &pi in prims {
        let src = &geo.prims[pi];
        let verts: Vec<u32> = src
            .vertices
            .iter()
            .map(|&v| {
                *remap.entry(v).or_insert_with(|| {
                    point_src.push(v as usize);
                    (point_src.len() - 1) as u32
                })
            })
            .collect();
        out.prims.push(vernon_core::Prim { vertices: verts, closed: src.closed });
    }
    out.points = point_src.iter().map(|&i| geo.points[i]).collect();
    out.point_attribs = geo.point_attribs.gather_all(&point_src);
    out.prim_attribs = geo.prim_attribs.gather_all(prims);
    out
}

pub fn extract_part(geo: &Geometry, part: i64) -> Geometry {
    let ids = part_ids(geo);
    let prims: Vec<usize> = (0..geo.prims.len()).filter(|&i| ids[i] == part).collect();
    extract_prims(geo, &prims)
}

/// Merge several geometries into one (attributes zero-padded where missing).
pub fn merge_all(parts: &[Geometry]) -> Geometry {
    let mut out = Geometry::new();
    for p in parts {
        out.merge(p);
    }
    out
}

/// Signed volume by the divergence theorem (positive for outward-wound shells).
pub fn signed_volume(geo: &Geometry) -> f64 {
    let mut v = 0.0;
    for (_, [a, b, c]) in geo.triangles() {
        let (pa, pb, pc) = (geo.points[a as usize], geo.points[b as usize], geo.points[c as usize]);
        v += pa.dot(pb.cross(pc));
    }
    v / 6.0
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EdgeStats {
    pub edges: usize,
    /// Edges used by exactly one face.
    pub open: usize,
    /// Edges used by more than two faces.
    pub non_manifold: usize,
}

fn edge_key(a: u32, b: u32) -> (u32, u32) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

pub fn edge_stats(geo: &Geometry) -> EdgeStats {
    let mut counts: HashMap<(u32, u32), u32> = HashMap::new();
    for p in geo.prims.iter().filter(|p| p.closed && p.vertices.len() >= 3) {
        let n = p.vertices.len();
        for i in 0..n {
            let (a, b) = (p.vertices[i], p.vertices[(i + 1) % n]);
            if a != b {
                *counts.entry(edge_key(a, b)).or_insert(0) += 1;
            }
        }
    }
    let mut s = EdgeStats { edges: counts.len(), ..Default::default() };
    for &c in counts.values() {
        if c == 1 {
            s.open += 1;
        } else if c > 2 {
            s.non_manifold += 1;
        }
    }
    s
}

/// Open-edge count per part (parts are separate shells).
pub fn open_edges_by_part(geo: &Geometry) -> usize {
    unique_parts(geo)
        .into_iter()
        .map(|p| edge_stats(&extract_part(geo, p)).open)
        .sum()
}

/// Bounds of the points used by the given primitives.
pub fn prim_bounds(geo: &Geometry, prims: &[usize]) -> Aabb {
    let mut b = Aabb::empty();
    for &pi in prims {
        for &v in &geo.prims[pi].vertices {
            b.grow(geo.points[v as usize]);
        }
    }
    b
}

/// Total area of the closed primitives.
pub fn area(geo: &Geometry) -> f64 {
    geo.surface_area()
}

/// Reverse the winding of every closed primitive.
pub fn flip_all(geo: &mut Geometry) {
    for p in &mut geo.prims {
        if p.closed {
            p.vertices.reverse();
        }
    }
}

/// Orthonormal frame (u, v) of a plane with unit normal `n`, with `u x v = n`.
pub fn plane_frame(n: DVec3) -> (DVec3, DVec3) {
    let helper = if n.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
    let u = (helper - n * helper.dot(n)).normalize();
    let v = n.cross(u);
    (u, v)
}

/// Moller-Trumbore ray/triangle test; returns the ray parameter of the hit.
pub fn ray_triangle(o: DVec3, d: DVec3, a: DVec3, b: DVec3, c: DVec3) -> Option<f64> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-14 {
        return None;
    }
    let inv = 1.0 / det;
    let t0 = o - a;
    let u = t0.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = t0.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some(t)
}

#[cfg(test)]
pub mod testutil {
    use super::*;
    use vernon_core::Geometry;

    /// Triangulated box (outward winding), min corner at `min`.
    pub fn tri_box(min: DVec3, size: DVec3) -> Geometry {
        let mut g = Geometry::make_box(size);
        for p in &mut g.points {
            *p += min + size * 0.5;
        }
        let quads = std::mem::take(&mut g.prims);
        for q in quads {
            let v = &q.vertices;
            g.prims.push(vernon_core::Prim::polygon(vec![v[0], v[1], v[2]]));
            g.prims.push(vernon_core::Prim::polygon(vec![v[0], v[2], v[3]]));
        }
        g
    }

    /// UV sphere with outward winding; poles are single points.
    pub fn uv_sphere(r: f64, nu: usize, nv: usize) -> Geometry {
        let mut pts = vec![DVec3::new(0.0, 0.0, r)];
        for j in 1..nv {
            let th = std::f64::consts::PI * j as f64 / nv as f64;
            for i in 0..nu {
                let ph = std::f64::consts::TAU * i as f64 / nu as f64;
                pts.push(DVec3::new(r * th.sin() * ph.cos(), r * th.sin() * ph.sin(), r * th.cos()));
            }
        }
        pts.push(DVec3::new(0.0, 0.0, -r));
        let south = (pts.len() - 1) as u32;
        let ring = |j: usize, i: usize| (1 + (j - 1) * nu + (i % nu)) as u32;
        let mut faces = Vec::new();
        for i in 0..nu {
            faces.push(vec![0, ring(1, i), ring(1, i + 1)]);
        }
        for j in 1..nv - 1 {
            for i in 0..nu {
                faces.push(vec![ring(j, i), ring(j + 1, i), ring(j + 1, i + 1)]);
                faces.push(vec![ring(j, i), ring(j + 1, i + 1), ring(j, i + 1)]);
            }
        }
        for i in 0..nu {
            faces.push(vec![south, ring(nv - 1, i + 1), ring(nv - 1, i)]);
        }
        Geometry::from_polygons(pts, faces)
    }
}

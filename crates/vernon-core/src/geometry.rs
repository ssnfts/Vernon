use std::collections::BTreeMap;

use glam::{DMat4, DVec3};
use serde::{Deserialize, Serialize};

use crate::{AttribTable, GeoError, GeoResult};

/// A primitive: an ordered list of point indices (its vertices).
/// `closed = true` is a polygon face; `closed = false` is a polyline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prim {
    pub vertices: Vec<u32>,
    pub closed: bool,
}

impl Prim {
    pub fn polygon(vertices: Vec<u32>) -> Self {
        Self { vertices, closed: true }
    }

    pub fn polyline(vertices: Vec<u32>) -> Self {
        Self { vertices, closed: false }
    }
}

/// Axis-aligned bounding box.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: DVec3,
    pub max: DVec3,
}

impl Aabb {
    pub fn empty() -> Self {
        Self {
            min: DVec3::splat(f64::INFINITY),
            max: DVec3::splat(f64::NEG_INFINITY),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }

    pub fn grow(&mut self, p: DVec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    pub fn size(&self) -> DVec3 {
        if self.is_empty() {
            DVec3::ZERO
        } else {
            self.max - self.min
        }
    }

    pub fn center(&self) -> DVec3 {
        (self.min + self.max) * 0.5
    }
}

/// Generic geometry container. See the crate docs for the model.
///
/// Standard attribute names (use these so nodes interoperate):
/// - point `N` (Vec3): normal
/// - point `Cd` (Vec3): colour 0..1
/// - point `uv` (Vec3): texture coordinate, z unused
/// - point `dist` (Float): signed deviation in mm
/// - prim `N` (Vec3): face normal
/// - prim `part` (Int): split-panel id
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Geometry {
    pub points: Vec<DVec3>,
    pub prims: Vec<Prim>,
    pub point_attribs: AttribTable,
    pub prim_attribs: AttribTable,
    /// One value per attribute (arrays of length 1).
    pub detail_attribs: AttribTable,
    /// Named point groups: sorted point indices.
    pub point_groups: BTreeMap<String, Vec<u32>>,
    /// Named primitive groups: sorted prim indices.
    pub prim_groups: BTreeMap<String, Vec<u32>>,
}

impl Geometry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_points(points: Vec<DVec3>) -> Self {
        Self {
            points,
            ..Default::default()
        }
    }

    pub fn from_polygons(points: Vec<DVec3>, faces: Vec<Vec<u32>>) -> Self {
        Self {
            points,
            prims: faces.into_iter().map(Prim::polygon).collect(),
            ..Default::default()
        }
    }

    pub fn point_count(&self) -> usize {
        self.points.len()
    }

    pub fn prim_count(&self) -> usize {
        self.prims.len()
    }

    pub fn is_point_cloud(&self) -> bool {
        self.prims.is_empty() && !self.points.is_empty()
    }

    pub fn add_point(&mut self, p: DVec3) -> u32 {
        self.points.push(p);
        let n = self.points.len();
        self.point_attribs.resize_all(n);
        (n - 1) as u32
    }

    pub fn add_prim(&mut self, prim: Prim) -> u32 {
        self.prims.push(prim);
        let n = self.prims.len();
        self.prim_attribs.resize_all(n);
        (n - 1) as u32
    }

    pub fn bounds(&self) -> Aabb {
        let mut b = Aabb::empty();
        for &p in &self.points {
            b.grow(p);
        }
        b
    }

    pub fn transform(&mut self, m: DMat4) {
        for p in &mut self.points {
            *p = m.transform_point3(*p);
        }
        let normal_m = m.inverse().transpose();
        for table in [&mut self.point_attribs, &mut self.prim_attribs] {
            if let Some(crate::AttribArray::Vec3(ns)) = table.get_mut("N") {
                for n in ns.iter_mut() {
                    *n = normal_m.transform_vector3(*n).normalize_or_zero();
                }
            }
        }
    }

    /// Fan-triangulated triangles of every closed primitive with >= 3 vertices.
    /// Returns (prim index, [a, b, c]) tuples.
    pub fn triangles(&self) -> Vec<(usize, [u32; 3])> {
        let mut out = Vec::new();
        for (pi, prim) in self.prims.iter().enumerate() {
            if !prim.closed || prim.vertices.len() < 3 {
                continue;
            }
            let v = &prim.vertices;
            for i in 1..v.len() - 1 {
                out.push((pi, [v[0], v[i], v[i + 1]]));
            }
        }
        out
    }

    /// Newell-method normal of a closed primitive.
    pub fn prim_normal(&self, prim: usize) -> DVec3 {
        let v = &self.prims[prim].vertices;
        let mut n = DVec3::ZERO;
        for i in 0..v.len() {
            let a = self.points[v[i] as usize];
            let b = self.points[v[(i + 1) % v.len()] as usize];
            n += DVec3::new(
                (a.y - b.y) * (a.z + b.z),
                (a.z - b.z) * (a.x + b.x),
                (a.x - b.x) * (a.y + b.y),
            );
        }
        n.normalize_or_zero()
    }

    /// Area of a closed planar-ish primitive (half the Newell vector length).
    pub fn prim_area(&self, prim: usize) -> f64 {
        let v = &self.prims[prim].vertices;
        if v.len() < 3 {
            return 0.0;
        }
        let o = self.points[v[0] as usize];
        let mut sum = DVec3::ZERO;
        for i in 1..v.len() - 1 {
            let a = self.points[v[i] as usize] - o;
            let b = self.points[v[i + 1] as usize] - o;
            sum += a.cross(b);
        }
        sum.length() * 0.5
    }

    pub fn surface_area(&self) -> f64 {
        (0..self.prims.len())
            .filter(|&i| self.prims[i].closed)
            .map(|i| self.prim_area(i))
            .sum()
    }

    /// Compute area-weighted point normals into point attribute `N` and face
    /// normals into prim attribute `N`.
    pub fn compute_normals(&mut self) {
        let mut pn = vec![DVec3::ZERO; self.points.len()];
        let mut fnorm = vec![DVec3::ZERO; self.prims.len()];
        for (pi, prim) in self.prims.iter().enumerate() {
            if !prim.closed || prim.vertices.len() < 3 {
                continue;
            }
            let n = self.prim_normal(pi);
            fnorm[pi] = n;
            let w = self.prim_area(pi).max(1e-12);
            for &v in &prim.vertices {
                pn[v as usize] += n * w;
            }
        }
        for n in &mut pn {
            *n = n.normalize_or_zero();
        }
        self.point_attribs.set("N", crate::AttribArray::Vec3(pn));
        self.prim_attribs.set("N", crate::AttribArray::Vec3(fnorm));
    }

    /// Append another geometry, offsetting its point indices. Attributes
    /// missing on either side are zero-filled.
    pub fn merge(&mut self, other: &Geometry) {
        let p0 = self.points.len();
        let f0 = self.prims.len();
        merge_tables(&mut self.point_attribs, p0, &other.point_attribs, other.points.len());
        merge_tables(&mut self.prim_attribs, f0, &other.prim_attribs, other.prims.len());
        self.points.extend_from_slice(&other.points);
        self.prims.extend(other.prims.iter().map(|p| Prim {
            vertices: p.vertices.iter().map(|&v| v + p0 as u32).collect(),
            closed: p.closed,
        }));
        for (name, idx) in &other.point_groups {
            self.point_groups
                .entry(name.clone())
                .or_default()
                .extend(idx.iter().map(|&i| i + p0 as u32));
        }
        for (name, idx) in &other.prim_groups {
            self.prim_groups
                .entry(name.clone())
                .or_default()
                .extend(idx.iter().map(|&i| i + f0 as u32));
        }
    }

    /// Check attribute lengths and point references.
    pub fn validate(&self) -> GeoResult<()> {
        let np = self.points.len();
        for (name, col) in self.point_attribs.iter() {
            if col.len() != np {
                return Err(GeoError::AttribLen {
                    name: name.to_string(),
                    expected: np,
                    found: col.len(),
                });
            }
        }
        for (name, col) in self.prim_attribs.iter() {
            if col.len() != self.prims.len() {
                return Err(GeoError::AttribLen {
                    name: name.to_string(),
                    expected: self.prims.len(),
                    found: col.len(),
                });
            }
        }
        for (pi, prim) in self.prims.iter().enumerate() {
            for &v in &prim.vertices {
                if v as usize >= np {
                    return Err(GeoError::BadPointRef {
                        prim: pi,
                        point: v,
                        count: np,
                    });
                }
            }
        }
        Ok(())
    }

    /// A unit-free axis-aligned box centred at the origin, as polygons.
    pub fn make_box(size: DVec3) -> Self {
        let h = size * 0.5;
        let points = vec![
            DVec3::new(-h.x, -h.y, -h.z),
            DVec3::new(h.x, -h.y, -h.z),
            DVec3::new(h.x, h.y, -h.z),
            DVec3::new(-h.x, h.y, -h.z),
            DVec3::new(-h.x, -h.y, h.z),
            DVec3::new(h.x, -h.y, h.z),
            DVec3::new(h.x, h.y, h.z),
            DVec3::new(-h.x, h.y, h.z),
        ];
        let faces = vec![
            vec![0, 3, 2, 1],
            vec![4, 5, 6, 7],
            vec![0, 1, 5, 4],
            vec![1, 2, 6, 5],
            vec![2, 3, 7, 6],
            vec![3, 0, 4, 7],
        ];
        Self::from_polygons(points, faces)
    }
}

fn merge_tables(dst: &mut AttribTable, dst_len: usize, src: &AttribTable, src_len: usize) {
    for (name, col) in src.iter() {
        if !dst.contains(name) {
            dst.ensure(name, col.ty(), dst_len).ok();
        }
    }
    let names: Vec<String> = dst.names().map(str::to_string).collect();
    for name in names {
        let d = dst.get_mut(&name).expect("column exists");
        match src.get(&name) {
            Some(s) => d.append(s, src_len),
            None => d.resize(dst_len + src_len),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AttribArray;

    #[test]
    fn box_area_and_normals() {
        let mut g = Geometry::make_box(DVec3::new(10.0, 20.0, 30.0));
        assert!((g.surface_area() - 2.0 * (200.0 + 300.0 + 600.0)).abs() < 1e-9);
        g.compute_normals();
        g.validate().unwrap();
        let fn_ = g.prim_attribs.vec3s("N").unwrap();
        // bottom face points down (-Z)
        assert!((fn_[0] - DVec3::NEG_Z).length() < 1e-9);
    }

    #[test]
    fn merge_offsets_and_pads() {
        let mut a = Geometry::make_box(DVec3::ONE);
        a.point_attribs
            .set("w", AttribArray::Float(vec![1.0; a.point_count()]));
        let b = Geometry::make_box(DVec3::ONE);
        a.merge(&b);
        a.validate().unwrap();
        assert_eq!(a.point_count(), 16);
        assert_eq!(a.prims[6].vertices[0], 8);
        assert_eq!(a.point_attribs.floats("w").unwrap()[15], 0.0);
    }
}

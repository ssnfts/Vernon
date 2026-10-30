//! CPU-side conversion of a [`Geometry`] into GPU-ready vertex/index arrays.
//! Pure and testable; the GPU upload lives in `renderer.rs`.

use bytemuck::{Pod, Zeroable};
use glam::DVec3;
use vernon_core::{Aabb, AttribArray, Geometry};

/// Colour (linear RGB) used for shaded faces when the geometry has no `Cd`.
pub const DEFAULT_MESH_COLOR: [f32; 3] = [0.5, 0.5, 0.5];


/// Vertex of the shaded-triangle buffer.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    /// Linear RGB.
    pub color: [f32; 3],
}

/// Vertex of the point buffer, shared by edges, polylines, loose points and
/// the grid.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct PointVertex {
    pub pos: [f32; 3],
    /// Linear RGB (only meaningful when the source had point `Cd`).
    pub color: [f32; 3],
}

/// GPU-ready arrays for one geometry.
#[derive(Debug, Clone)]
pub struct MeshBuffers {
    /// Shaded triangle vertices.
    pub tri_vertices: Vec<MeshVertex>,
    /// Triangle list indices into `tri_vertices`.
    pub tri_indices: Vec<u32>,
    /// One vertex per geometry point.
    pub point_vertices: Vec<PointVertex>,
    /// Unique polygon edges as a line list into `point_vertices`.
    pub edge_indices: Vec<u32>,
    /// Open polyline segments as a line list into `point_vertices`.
    pub curve_indices: Vec<u32>,
    /// Points not referenced by any primitive (a point cloud is all loose).
    pub loose_points: Vec<u32>,
    /// True when the geometry carried point `Cd`, so lines/points should
    /// use vertex colours instead of the wire colour.
    pub has_point_color: bool,
    /// True when triangles share one vertex per point (smooth `N`);
    /// false when every polygon has its own vertices (flat shading).
    pub smooth: bool,
    /// Bounds of all points (mm).
    pub bounds: Aabb,
}

impl Default for MeshBuffers {
    fn default() -> Self {
        Self {
            tri_vertices: Vec::new(),
            tri_indices: Vec::new(),
            point_vertices: Vec::new(),
            edge_indices: Vec::new(),
            curve_indices: Vec::new(),
            loose_points: Vec::new(),
            has_point_color: false,
            smooth: false,
            bounds: Aabb::empty(),
        }
    }
}

impl MeshBuffers {
    pub fn triangle_count(&self) -> usize {
        self.tri_indices.len() / 3
    }

    pub fn edge_count(&self) -> usize {
        self.edge_indices.len() / 2
    }

    pub fn curve_segment_count(&self) -> usize {
        self.curve_indices.len() / 2
    }

    /// Build buffers from a geometry.
    ///
    /// - Closed prims with >= 3 vertices are triangulated (ear clipping,
    ///   falling back to a fan) for shading.
    /// - Normals: point `N` if present (smooth), else prim `N`, else the
    ///   polygon's Newell normal (flat).
    /// - Colours: point `Cd`, else prim `Cd` (flat), else
    ///   [`DEFAULT_MESH_COLOR`].
    /// - Edges of closed prims are de-duplicated; open prims become
    ///   polyline segments.
    /// - Prims referencing missing points are skipped, never panic.
    pub fn from_geometry(geo: &Geometry) -> Self {
        let np = geo.points.len();
        let point_n = vec3_attrib(geo.point_attribs.get("N"), np);
        let point_cd = vec3_attrib(geo.point_attribs.get("Cd"), np);
        let prim_n = vec3_attrib(geo.prim_attribs.get("N"), geo.prims.len());
        let prim_cd = vec3_attrib(geo.prim_attribs.get("Cd"), geo.prims.len());
        let smooth = point_n.is_some() && (point_cd.is_some() || prim_cd.is_none());

        let mut out = MeshBuffers {
            has_point_color: point_cd.is_some(),
            smooth,
            bounds: geo.bounds(),
            ..Default::default()
        };

        out.point_vertices = geo
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| PointVertex {
                pos: f3(*p),
                color: point_cd.map_or([0.0; 3], |c| color3(c[i])),
            })
            .collect();

        if smooth {
            let ns = point_n.expect("smooth implies point N");
            out.tri_vertices = geo
                .points
                .iter()
                .enumerate()
                .map(|(i, p)| MeshVertex {
                    pos: f3(*p),
                    normal: f3(ns[i]),
                    color: point_cd.map_or(DEFAULT_MESH_COLOR, |c| color3(c[i])),
                })
                .collect();
        }

        let mut used = vec![false; np];
        let mut edge_keys: Vec<u64> = Vec::new();
        let mut local = Vec::new();
        let mut poly = Vec::new();

        for (pi, prim) in geo.prims.iter().enumerate() {
            let v = &prim.vertices;
            if v.is_empty() || v.iter().any(|&i| i as usize >= np) {
                continue;
            }
            for &i in v {
                used[i as usize] = true;
            }
            if !prim.closed {
                for w in v.windows(2) {
                    if w[0] != w[1] {
                        out.curve_indices.extend_from_slice(&[w[0], w[1]]);
                    }
                }
                continue;
            }
            // Closed prim: edges, including the closing edge.
            if v.len() >= 2 {
                let n = v.len();
                let count = if n == 2 { 1 } else { n };
                for k in 0..count {
                    let (a, b) = (v[k], v[(k + 1) % n]);
                    if a != b {
                        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
                        edge_keys.push(((lo as u64) << 32) | hi as u64);
                    }
                }
            }
            if v.len() < 3 {
                continue;
            }
            poly.clear();
            poly.extend(v.iter().map(|&i| geo.points[i as usize]));
            local.clear();
            triangulate_polygon(&poly, &mut local);

            if smooth {
                for t in &local {
                    out.tri_indices.extend(t.iter().map(|&k| v[k]));
                }
            } else {
                let face_n = prim_n
                    .map(|n| n[pi])
                    .filter(|n| n.length_squared() > 0.0)
                    .unwrap_or_else(|| newell_normal(&poly));
                let face_c = prim_cd.map_or(DEFAULT_MESH_COLOR, |c| color3(c[pi]));
                let base = out.tri_vertices.len() as u32;
                for &i in v {
                    let i = i as usize;
                    out.tri_vertices.push(MeshVertex {
                        pos: f3(geo.points[i]),
                        normal: f3(point_n.map_or(face_n, |n| n[i])),
                        color: point_cd.map_or(face_c, |c| color3(c[i])),
                    });
                }
                for t in &local {
                    out.tri_indices.extend(t.iter().map(|&k| base + k as u32));
                }
            }
        }

        edge_keys.sort_unstable();
        edge_keys.dedup();
        out.edge_indices.reserve(edge_keys.len() * 2);
        for k in edge_keys {
            out.edge_indices.push((k >> 32) as u32);
            out.edge_indices.push(k as u32);
        }
        out.loose_points = (0..np as u32).filter(|&i| !used[i as usize]).collect();
        out
    }
}

fn vec3_attrib(a: Option<&AttribArray>, len: usize) -> Option<&[DVec3]> {
    match a {
        Some(AttribArray::Vec3(v)) if v.len() == len && len > 0 => Some(v.as_slice()),
        _ => None,
    }
}

fn f3(v: DVec3) -> [f32; 3] {
    [v.x as f32, v.y as f32, v.z as f32]
}

fn color3(c: DVec3) -> [f32; 3] {
    let c = c.clamp(DVec3::ZERO, DVec3::ONE);
    let c = if c.is_finite() { c } else { DVec3::ZERO };
    f3(c)
}

/// Newell normal of a polygon (unit, or zero when degenerate).
pub fn newell_normal(poly: &[DVec3]) -> DVec3 {
    let mut n = DVec3::ZERO;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        n += DVec3::new(
            (a.y - b.y) * (a.z + b.z),
            (a.z - b.z) * (a.x + b.x),
            (a.x - b.x) * (a.y + b.y),
        );
    }
    n.normalize_or_zero()
}

/// Triangulate a (possibly concave, roughly planar) polygon into `out` as
/// local vertex indices, keeping the polygon's winding. Delegates to the shared
/// triangulator in `vernon-core`, so the viewport matches the triangle writers.
pub fn triangulate_polygon(poly: &[DVec3], out: &mut Vec<[usize; 3]>) {
    let ring: Vec<u32> = (0..poly.len() as u32).collect();
    let mut tris = Vec::new();
    vernon_core::triangulate::triangulate_ring(poly, &ring, &mut tris);
    out.extend(tris.iter().map(|t| t.map(|i| i as usize)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use vernon_core::Prim;

    fn tri_area(b: &MeshBuffers, t: usize) -> f64 {
        let p = |k: usize| {
            let v = b.tri_vertices[b.tri_indices[3 * t + k] as usize].pos;
            DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64)
        };
        (p(1) - p(0)).cross(p(2) - p(0)).length() * 0.5
    }

    #[test]
    fn box_counts_flat() {
        let g = Geometry::make_box(DVec3::new(10.0, 20.0, 30.0));
        let b = MeshBuffers::from_geometry(&g);
        assert!(!b.smooth);
        assert_eq!(b.triangle_count(), 12);
        assert_eq!(b.tri_vertices.len(), 24); // 6 quads, own vertices each
        assert_eq!(b.edge_count(), 12);
        assert_eq!(b.curve_segment_count(), 0);
        assert!(b.loose_points.is_empty());
        assert_eq!(b.point_vertices.len(), 8);
        assert!(!b.has_point_color);
        let total: f64 = (0..b.triangle_count()).map(|t| tri_area(&b, t)).sum();
        assert!((total - g.surface_area()).abs() < 1e-3);
        // Flat normals: the first face (bottom) points down.
        assert_eq!(b.tri_vertices[0].normal, [0.0, 0.0, -1.0]);
        assert_eq!(b.tri_vertices[0].color, DEFAULT_MESH_COLOR);
        // Every edge index is a valid point.
        assert!(b.edge_indices.iter().all(|&i| (i as usize) < 8));
    }

    #[test]
    fn box_with_normals_and_colour_is_smooth() {
        let mut g = Geometry::make_box(DVec3::splat(2.0));
        g.compute_normals();
        let cd = vec![DVec3::new(1.0, 0.0, 0.0); 8];
        g.point_attribs.set("Cd", AttribArray::Vec3(cd));
        let b = MeshBuffers::from_geometry(&g);
        assert!(b.smooth && b.has_point_color);
        assert_eq!(b.tri_vertices.len(), 8);
        assert_eq!(b.triangle_count(), 12);
        assert_eq!(b.edge_count(), 12);
        assert_eq!(b.tri_vertices[3].color, [1.0, 0.0, 0.0]);
        assert_eq!(b.point_vertices[3].color, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn concave_l_hexagon_renders_four_non_overlapping_triangles() {
        // An L shape with a reflex corner at index 3: a fan from vertex 0
        // would put triangles outside the outline.
        let pts: Vec<DVec3> = [(0., 0.), (4., 0.), (4., 2.), (2., 2.), (2., 4.), (0., 4.)]
            .iter()
            .map(|&(x, y)| DVec3::new(x, y, 0.0))
            .collect();
        let g = Geometry::from_polygons(pts, vec![(0..6).collect()]);
        let b = MeshBuffers::from_geometry(&g);
        assert_eq!(b.triangle_count(), 4);
        let total: f64 = (0..b.triangle_count()).map(|t| tri_area(&b, t)).sum();
        assert!((total - 12.0).abs() < 1e-9, "area {total}");
        assert!((total - g.surface_area()).abs() < 1e-9);
    }

    #[test]
    fn ngon_counts() {
        for n in [5usize, 6, 12, 300] {
            let pts: Vec<DVec3> = (0..n)
                .map(|i| {
                    let a = i as f64 / n as f64 * std::f64::consts::TAU;
                    DVec3::new(a.cos() * 50.0, a.sin() * 50.0, 7.0)
                })
                .collect();
            let g = Geometry::from_polygons(pts, vec![(0..n as u32).collect()]);
            let b = MeshBuffers::from_geometry(&g);
            assert_eq!(b.triangle_count(), n - 2, "n = {n}");
            assert_eq!(b.edge_count(), n, "n = {n}");
            assert_eq!(b.tri_vertices.len(), n);
            let total: f64 = (0..b.triangle_count()).map(|t| tri_area(&b, t)).sum();
            assert!((total - g.surface_area()).abs() < 1e-3 * g.surface_area());
        }
    }

    #[test]
    fn concave_polygon_is_ear_clipped() {
        // L-shape in the XZ plane (normal along -Y), 6 vertices, reflex corner.
        let pts = vec![
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(20.0, 0.0, 0.0),
            DVec3::new(20.0, 0.0, 10.0),
            DVec3::new(10.0, 0.0, 10.0),
            DVec3::new(10.0, 0.0, 20.0),
            DVec3::new(0.0, 0.0, 20.0),
        ];
        let g = Geometry::from_polygons(pts, vec![vec![0, 1, 2, 3, 4, 5]]);
        let b = MeshBuffers::from_geometry(&g);
        assert_eq!(b.triangle_count(), 4);
        let total: f64 = (0..4).map(|t| tri_area(&b, t)).sum();
        // A fan from vertex 0 would cover area outside the L; ear clipping
        // covers exactly 300 mm^2.
        assert!((total - 300.0).abs() < 1e-6, "area {total}");
        // Winding preserved: every triangle normal agrees with the polygon.
        let pn = g.prim_normal(0);
        for t in 0..4 {
            let p = |k: usize| {
                let v = b.tri_vertices[b.tri_indices[3 * t + k] as usize].pos;
                DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64)
            };
            let n = (p(1) - p(0)).cross(p(2) - p(0));
            assert!(n.dot(pn) > 0.0);
        }
    }

    #[test]
    fn polylines_points_and_bad_refs() {
        let mut g = Geometry::from_points((0..7).map(|i| DVec3::new(i as f64, 0.0, 0.0)).collect());
        g.prims.push(Prim::polyline(vec![0, 1, 2, 3, 4]));
        g.prims.push(Prim::polygon(vec![0, 1, 99])); // bad ref: skipped
        let b = MeshBuffers::from_geometry(&g);
        assert_eq!(b.curve_segment_count(), 4);
        assert_eq!(b.triangle_count(), 0);
        assert_eq!(b.edge_count(), 0);
        assert_eq!(b.loose_points, vec![5, 6]);

        let cloud = Geometry::from_points(vec![DVec3::ZERO, DVec3::ONE, DVec3::X]);
        let b = MeshBuffers::from_geometry(&cloud);
        assert_eq!(b.loose_points, vec![0, 1, 2]);
        assert_eq!(b.point_vertices.len(), 3);
        assert_eq!(b.bounds.max, DVec3::ONE);

        let empty = MeshBuffers::from_geometry(&Geometry::new());
        assert_eq!(empty.triangle_count(), 0);
        assert!(empty.bounds.is_empty());
    }

    #[test]
    fn shared_edges_are_deduplicated() {
        // Two triangles sharing an edge: 5 unique edges, 2 triangles.
        let pts = vec![DVec3::ZERO, DVec3::X, DVec3::Y, DVec3::new(1.0, 1.0, 0.0)];
        let g = Geometry::from_polygons(pts, vec![vec![0, 1, 2], vec![1, 3, 2]]);
        let b = MeshBuffers::from_geometry(&g);
        assert_eq!(b.triangle_count(), 2);
        assert_eq!(b.edge_count(), 5);
    }
}

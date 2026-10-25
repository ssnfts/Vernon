//! Polygon clipping and reflection for the quad remesher's symmetry wedge.

use glam::DVec3;
use vernon_core::Geometry;

use crate::topology::weld;
use crate::util::triangulate_ring;

fn selected_axes(axes: &str) -> Vec<usize> {
    ['x', 'y', 'z']
        .into_iter()
        .enumerate()
        .filter_map(|(i, c)| axes.contains(c).then_some(i))
        .collect()
}

fn valid(geo: &Geometry, center: DVec3) -> bool {
    center.is_finite()
        && geo.points.iter().all(|p| p.is_finite())
        && geo
            .prims
            .iter()
            .all(|p| p.vertices.iter().all(|&v| (v as usize) < geo.points.len()))
}

fn tolerance(geo: &Geometry, center: DVec3) -> f64 {
    let magnitude = geo
        .points
        .iter()
        .map(|p| p.abs().max_element())
        .fold(center.abs().max_element(), f64::max);
    (geo.bounds().size().length() * 1e-11)
        .max(magnitude * 1e-13)
        .max(1e-12)
}

fn snap(mut p: DVec3, axes: &[usize], center: DVec3, tol: f64) -> DVec3 {
    for &axis in axes {
        if (p[axis] - center[axis]).abs() <= tol {
            p[axis] = center[axis];
        }
    }
    p
}

fn area_vector(ring: &[DVec3]) -> DVec3 {
    let Some(&origin) = ring.first() else {
        return DVec3::ZERO;
    };
    (1..ring.len().saturating_sub(1))
        .map(|i| (ring[i] - origin).cross(ring[i + 1] - origin))
        .sum()
}

fn append_ring(geo: &mut Geometry, ring: &[DVec3], tol: f64) {
    if ring.len() < 3 || area_vector(ring).length() <= tol * tol {
        return;
    }
    let start = geo.points.len() as u32;
    geo.points.extend_from_slice(ring);
    geo.prims.push(vernon_core::Prim::polygon(
        (start..start + ring.len() as u32).collect(),
    ));
}

/// Shared seam vertices, no zero-area polygons, and no unused cut-away points.
fn finish(geo: Geometry, tol: f64) -> Geometry {
    let welded = weld(&geo, tol);
    let faces: Vec<Vec<u32>> = welded
        .faces
        .into_iter()
        .filter(|f| {
            let ring: Vec<DVec3> = f.iter().map(|&i| welded.points[i as usize]).collect();
            area_vector(&ring).length() > tol * tol
        })
        .collect();
    let mut used = vec![false; welded.points.len()];
    for f in &faces {
        for &v in f {
            used[v as usize] = true;
        }
    }
    let mut points = Vec::new();
    let mut map = vec![0u32; used.len()];
    for (i, &p) in welded.points.iter().enumerate() {
        if used[i] {
            map[i] = points.len() as u32;
            points.push(p);
        }
    }
    let faces = faces
        .into_iter()
        .map(|f| f.into_iter().map(|v| map[v as usize]).collect())
        .collect();
    let mut out = Geometry::from_polygons(points, faces);
    out.compute_normals();
    out
}

fn clip_plane(ring: &[DVec3], axis: usize, coordinate: f64) -> Vec<DVec3> {
    let Some(&last) = ring.last() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut a = last;
    for &b in ring {
        let da = a[axis] - coordinate;
        let db = b[axis] - coordinate;
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            let mut p = a * (1.0 - t) + b * t;
            p[axis] = coordinate;
            out.push(p);
        }
        if db >= 0.0 {
            out.push(b);
        }
        a = b;
    }
    out
}

/// A concave polygon can split into several components when clipped. A
/// non-planar polygon also needs its triangulation preserved during a cut.
fn needs_triangles(ring: &[DVec3], tol: f64) -> bool {
    let n = area_vector(ring).normalize_or_zero();
    for i in 0..ring.len() {
        let (a, b, c) = (
            ring[i],
            ring[(i + 1) % ring.len()],
            ring[(i + 2) % ring.len()],
        );
        if (b - a).cross(c - b).dot(n) < -tol * tol || (a - ring[0]).dot(n).abs() > tol {
            return true;
        }
    }
    false
}

/// Retain the positive halfspace of each selected axis through `center`.
/// New cut edges are left open; reflection joins them without internal caps.
/// Closed polygon surfaces are retained; attributes are rebuilt as normals.
pub fn cut_positive(geo: &Geometry, axes: &str, center: DVec3) -> Geometry {
    if !valid(geo, center) {
        return finish(Geometry::new(), 1e-12);
    }
    let axes = selected_axes(axes);
    if axes.is_empty() {
        let mut out = geo.clone();
        out.compute_normals();
        return out;
    }
    let tol = tolerance(geo, center);
    let mut out = Geometry::new();
    for prim in geo
        .prims
        .iter()
        .filter(|p| p.closed && p.vertices.len() >= 3)
    {
        let ring: Vec<DVec3> = prim
            .vertices
            .iter()
            .map(|&v| snap(geo.points[v as usize], &axes, center, tol))
            .collect();
        if axes
            .iter()
            .any(|&axis| ring.iter().all(|p| p[axis] < center[axis]))
        {
            continue;
        }
        if axes
            .iter()
            .all(|&axis| ring.iter().all(|p| p[axis] >= center[axis]))
        {
            append_ring(&mut out, &ring, tol);
            continue;
        }
        let pieces = if needs_triangles(&ring, tol) {
            let indices: Vec<u32> = (0..ring.len() as u32).collect();
            triangulate_ring(&ring, &indices)
                .into_iter()
                .map(|t| t.into_iter().map(|v| ring[v as usize]).collect())
                .collect::<Vec<Vec<DVec3>>>()
        } else {
            vec![ring]
        };
        for mut piece in pieces {
            for &axis in &axes {
                piece = clip_plane(&piece, axis, center[axis]);
            }
            append_ring(&mut out, &piece, tol);
        }
    }
    finish(out, tol)
}

/// Replicate a positive wedge into the selected halfspaces. Odd reflection
/// parity reverses polygon winding, and seam vertices are welded afterwards.
pub fn reflect(geo: &Geometry, axes: &str, center: DVec3) -> Geometry {
    if !valid(geo, center) {
        return finish(Geometry::new(), 1e-12);
    }
    let axes = selected_axes(axes);
    if axes.is_empty() {
        let mut out = geo.clone();
        out.compute_normals();
        return out;
    }
    let tol = tolerance(geo, center);
    let mut out = Geometry::new();
    for mask in 0usize..(1 << axes.len()) {
        for prim in geo
            .prims
            .iter()
            .filter(|p| p.closed && p.vertices.len() >= 3)
        {
            let mut ring: Vec<DVec3> = prim
                .vertices
                .iter()
                .map(|&v| snap(geo.points[v as usize], &axes, center, tol))
                .collect();
            // A face entirely in a mirror plane already exists in the copy
            // whose corresponding mask bit is zero.
            if axes.iter().enumerate().any(|(bit, &axis)| {
                mask & (1 << bit) != 0 && ring.iter().all(|p| p[axis] == center[axis])
            }) {
                continue;
            }
            for (bit, &axis) in axes.iter().enumerate() {
                if mask & (1 << bit) != 0 {
                    for p in &mut ring {
                        p[axis] = center[axis] - (p[axis] - center[axis]);
                    }
                }
            }
            if mask.count_ones() % 2 == 1 {
                ring.reverse();
            }
            append_ring(&mut out, &ring, tol);
        }
    }
    finish(out, tol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::{stats, weld};

    fn symmetry_pairs(geo: &Geometry, axes: &str, center: DVec3) {
        for (axis, letter) in ['x', 'y', 'z'].into_iter().enumerate() {
            if axes.contains(letter) {
                for p in &geo.points {
                    let mut q = *p;
                    q[axis] = 2.0 * center[axis] - q[axis];
                    assert!(
                        geo.points.iter().any(|v| v.distance(q) < 1e-9),
                        "missing reflected point {q:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn cube_half_and_octant_reassemble_closed_with_outward_winding() {
        let center = DVec3::new(13.0, -7.0, 4.0);
        let mut cube = Geometry::make_box(DVec3::splat(10.0));
        for p in &mut cube.points {
            *p += center;
        }
        for axes in ["x", "xy", "xyz"] {
            let part = cut_positive(&cube, axes, center);
            for (axis, letter) in ['x', 'y', 'z'].into_iter().enumerate() {
                if axes.contains(letter) {
                    assert!(part.points.iter().all(|p| p[axis] >= center[axis]));
                }
            }
            let out = reflect(&part, axes, center);
            out.validate().unwrap();
            let topology = stats(&weld(&out, 1e-9));
            assert!(topology.closed, "{axes}: {topology:?}");
            assert_eq!(topology.euler_char, 2);
            assert_eq!(topology.nonmanifold_edges, 0);
            assert!((topology.volume - 1000.0).abs() < 1e-8);
            assert!((out.surface_area() - 600.0).abs() < 1e-8);
            symmetry_pairs(&out, axes, center);
            assert!(out.point_attribs.vec3s("N").is_ok());
        }
    }

    #[test]
    fn even_curved_panel_keeps_one_welded_seam() {
        let mut points = Vec::new();
        let mut faces = Vec::new();
        for j in 0..=4 {
            for i in 0..=6 {
                let x = i as f64 - 3.0;
                let y = j as f64 - 2.0;
                points.push(DVec3::new(x, y, 0.2 * x * x + 0.1 * y * y));
            }
        }
        for j in 0..4u32 {
            for i in 0..6u32 {
                let a = j * 7 + i;
                faces.push(vec![a, a + 1, a + 8, a + 7]);
            }
        }
        let panel = Geometry::from_polygons(points, faces);
        let out = reflect(&cut_positive(&panel, "x", DVec3::ZERO), "x", DVec3::ZERO);
        assert_eq!(out.prims.len(), 24);
        assert_eq!(out.points.len(), 35);
        let topology = stats(&weld(&out, 1e-9));
        assert_eq!(topology.boundary_edges, 20);
        assert_eq!(topology.nonmanifold_edges, 0);
        assert_eq!(topology.inconsistent_edges, 0);
        assert_eq!(topology.euler_char, 1);
        assert!((out.surface_area() - panel.surface_area()).abs() < 1e-10);
        symmetry_pairs(&out, "x", DVec3::ZERO);
    }

    #[test]
    fn oblique_plane_cut_interpolates_on_the_source_plane() {
        let point = |x: f64, y: f64| DVec3::new(x, y, 0.5 * x + 0.25 * y + 3.0);
        let plane = Geometry::from_polygons(
            vec![
                point(-2.0, -1.0),
                point(2.0, -1.0),
                point(2.0, 1.0),
                point(-2.0, 1.0),
            ],
            vec![vec![0, 1, 2, 3]],
        );
        let out = cut_positive(&plane, "x", DVec3::new(0.5, 0.0, 0.0));
        assert_eq!(out.prims.len(), 1);
        assert_eq!(out.points.len(), 4);
        assert!(out.points.iter().all(|p| p.x >= 0.5));
        assert_eq!(out.points.iter().filter(|p| p.x == 0.5).count(), 2);
        assert!(out
            .points
            .iter()
            .all(|p| (p.z - (0.5 * p.x + 0.25 * p.y + 3.0)).abs() < 1e-12));
        assert!((out.surface_area() / plane.surface_area() - 0.375).abs() < 1e-12);
        assert!(out.prim_normal(0).dot(plane.prim_normal(0)) > 1.0 - 1e-12);
    }

    #[test]
    fn faces_on_a_mirror_plane_are_not_duplicated() {
        let plane = Geometry::from_polygons(
            vec![
                DVec3::new(0.0, -1.0, -1.0),
                DVec3::new(0.0, 1.0, -1.0),
                DVec3::new(0.0, 1.0, 1.0),
                DVec3::new(0.0, -1.0, 1.0),
            ],
            vec![vec![0, 1, 2, 3]],
        );
        let out = reflect(&plane, "x", DVec3::ZERO);
        assert_eq!(out.prims.len(), 1);
        assert_eq!(out.points.len(), 4);
        let quarter = cut_positive(&plane, "xyz", DVec3::ZERO);
        let all = reflect(&quarter, "xyz", DVec3::ZERO);
        assert_eq!(all.prims.len(), 4);
        assert!((all.surface_area() - 4.0).abs() < 1e-12);
        assert_eq!(stats(&weld(&all, 1e-9)).nonmanifold_edges, 0);
    }

    #[test]
    fn clipping_a_concave_face_keeps_disconnected_pieces_separate() {
        // A U-shaped face: cutting off the bottom must leave two arms.
        let xy = [
            (-3.0, -2.0),
            (3.0, -2.0),
            (3.0, 2.0),
            (1.0, 2.0),
            (1.0, -1.0),
            (-1.0, -1.0),
            (-1.0, 2.0),
            (-3.0, 2.0),
        ];
        let mesh = Geometry::from_polygons(
            xy.into_iter().map(|(x, y)| DVec3::new(x, y, 0.0)).collect(),
            vec![(0..8).collect()],
        );
        let out = cut_positive(&mesh, "y", DVec3::ZERO);
        assert!((out.surface_area() - 8.0).abs() < 1e-10);
        assert_eq!(stats(&weld(&out, 1e-9)).components, 2);
        assert!(out.points.iter().all(|p| p.y >= 0.0));
    }

    #[test]
    fn empty_none_and_degenerate_geometry_are_handled() {
        for axes in ["none", "xyz"] {
            let empty = reflect(
                &cut_positive(&Geometry::new(), axes, DVec3::ZERO),
                axes,
                DVec3::ZERO,
            );
            empty.validate().unwrap();
            assert!(empty.prims.is_empty());
            assert!(empty.points.is_empty());
        }
        let cube = Geometry::make_box(DVec3::splat(2.0));
        let out = reflect(
            &cut_positive(&cube, "none", DVec3::ZERO),
            "none",
            DVec3::ZERO,
        );
        assert_eq!(out.points, cube.points);
        assert_eq!(out.prims, cube.prims);
        assert!(out.point_attribs.vec3s("N").is_ok());
        let line =
            Geometry::from_polygons(vec![-DVec3::X, DVec3::ZERO, DVec3::X], vec![vec![0, 1, 2]]);
        assert!(cut_positive(&line, "x", DVec3::ZERO).prims.is_empty());
        assert!(reflect(&line, "x", DVec3::ZERO).prims.is_empty());
    }

    #[test]
    fn adjacent_cut_triangles_share_intersections_after_reflection() {
        let cube = Geometry::make_box(DVec3::splat(2.0));
        let triangles = Geometry::from_polygons(
            cube.points.clone(),
            cube.triangles()
                .into_iter()
                .map(|(_, t)| t.to_vec())
                .collect(),
        );
        let half = cut_positive(&triangles, "x", DVec3::ZERO);
        let out = reflect(&half, "x", DVec3::ZERO);
        let topology = stats(&weld(&out, 1e-9));
        assert!(topology.closed, "{topology:?}");
        assert_eq!(topology.euler_char, 2);
        assert!((topology.volume - 8.0).abs() < 1e-10);
        assert_eq!(out, reflect(&half, "x", DVec3::ZERO));
    }

    #[test]
    fn invalid_indices_and_non_finite_input_do_not_escape() {
        let mut bad = Geometry::make_box(DVec3::splat(2.0));
        bad.points[0].x = f64::NAN;
        for out in [
            cut_positive(&bad, "x", DVec3::ZERO),
            reflect(&bad, "x", DVec3::ZERO),
        ] {
            assert!(out.points.is_empty());
            out.validate().unwrap();
        }
        let mut bad = Geometry::make_box(DVec3::splat(2.0));
        bad.prims[0].vertices.push(999);
        assert!(cut_positive(&bad, "x", DVec3::ZERO).points.is_empty());
        assert!(reflect(&bad, "x", DVec3::ZERO).points.is_empty());
        assert!(
            cut_positive(&Geometry::new(), "x", DVec3::splat(f64::INFINITY))
                .points
                .is_empty()
        );
    }
}

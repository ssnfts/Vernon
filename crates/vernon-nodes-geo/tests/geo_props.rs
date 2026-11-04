//! Property tests of transforms, primitives and measurements in
//! `vernon-nodes-geo`, public API only, with a seeded xorshift generator.
//!
//! Closed forms used (elementary geometry; no external source needed):
//! * box: area 2(ab+bc+ca), volume abc;
//! * regular n-gon of circumradius R: area n/2 R^2 sin(2 pi/n), perimeter 2 n R sin(pi/n);
//! * prism / frustum of such polygons: volume H/3 (A1 + A2 + sqrt(A1 A2)) (prismatoid);
//! * a polyhedron inscribed in a sphere of radius R whose faces have angular
//!   circumradius at most `a` (seen from the centre) has
//!   A_sphere cos^2 a <= A <= A_sphere and V_sphere cos^3 a <= V <= V_sphere
//!   (radial projection of a face at distance h = R cos a: solid angle
//!   dA cos(psi)/rho^2 with h <= rho <= R; volume as a sum of pyramids on the faces).
//!
//! NOTES: `file`, `noise`, `select`, `topology` and the node `cook` entry
//! points are not exercised here (they need a `CookContext`); `car_panel` and
//! `make_tube` with unequal radii are only checked through the prismatoid
//! volume.

use glam::{DMat4, DVec3};
use std::collections::HashMap;
use vernon_core::Geometry;
use vernon_nodes_geo::measure::measure;
use vernon_nodes_geo::primitives::{make_box, make_grid, make_sphere, make_tube, GridPlane};
use vernon_nodes_geo::xform::{apply_transform, euler_matrix, mirror, transform_matrix};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
    fn vec(&mut self, lo: f64, hi: f64) -> DVec3 {
        DVec3::new(self.range(lo, hi), self.range(lo, hi), self.range(lo, hi))
    }
}

const ORDERS: [&str; 6] = ["xyz", "xzy", "yxz", "yzx", "zxy", "zyx"];

fn cloud(rng: &mut Rng, n: usize) -> Geometry {
    let mut g = Geometry::new();
    for _ in 0..n {
        g.points.push(rng.vec(-100.0, 100.0));
    }
    g
}

fn rel(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * a.abs().max(b.abs()).max(1e-12)
}

fn v_close(a: DVec3, b: DVec3, tol: f64) -> bool {
    (a - b).length() <= tol
}

/// Independent fan triangulation of every polygon: (a, b, c) positions.
fn fan(g: &Geometry) -> Vec<[DVec3; 3]> {
    let mut out = Vec::new();
    for p in &g.prims {
        for k in 1..p.vertices.len().saturating_sub(1) {
            out.push([
                g.points[p.vertices[0] as usize],
                g.points[p.vertices[k] as usize],
                g.points[p.vertices[k + 1] as usize],
            ]);
        }
    }
    out
}

fn area_of(g: &Geometry) -> f64 {
    fan(g).iter().map(|[a, b, c]| 0.5 * (*b - *a).cross(*c - *a).length()).sum()
}

fn volume_of(g: &Geometry) -> f64 {
    fan(g).iter().map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0).sum()
}

/// Undirected edge -> number of polygon sides using it.
fn edges(g: &Geometry) -> HashMap<(u32, u32), usize> {
    let mut m = HashMap::new();
    for p in &g.prims {
        let n = p.vertices.len();
        for i in 0..n {
            let (a, b) = (p.vertices[i], p.vertices[(i + 1) % n]);
            *m.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    m
}

// ------------------------------------------------------------- transforms

/// Property: a transform followed by its inverse restores every point (1e-9),
/// for random translation, rotation order, non-uniform scale and pivot.
#[test]
fn geo_props_transform_then_inverse_restores_points() {
    let mut rng = Rng(0xC0FF_EE00_1234);
    for k in 0..200 {
        let g0 = cloud(&mut rng, 20);
        let m = transform_matrix(
            rng.vec(-50.0, 50.0),
            rng.vec(-180.0, 180.0),
            ORDERS[k % 6],
            DVec3::new(rng.range(0.3, 3.0), rng.range(0.3, 3.0), rng.range(0.3, 3.0)),
            rng.vec(-20.0, 20.0),
        );
        let mut g = g0.clone();
        apply_transform(&mut g, m, false);
        apply_transform(&mut g, m.inverse(), false);
        for (a, b) in g0.points.iter().zip(&g.points) {
            assert!(v_close(*a, *b, 1e-9), "round trip off by {}", (*a - *b).length());
        }
    }
}

/// Property: euler_matrix is a proper rotation (orthonormal, det +1) for all
/// orders, and the order matters: rotate X then Z differs from Z then X.
#[test]
fn geo_props_euler_matrix_is_a_rotation_and_order_matters() {
    let mut rng = Rng(5);
    for k in 0..120 {
        let m = euler_matrix(rng.vec(-360.0, 360.0), ORDERS[k % 6]);
        assert!(rel(m.determinant(), 1.0, 1e-12));
        let (c0, c1, c2) = (m.col(0).truncate(), m.col(1).truncate(), m.col(2).truncate());
        assert!(rel(c0.length(), 1.0, 1e-12) && rel(c1.length(), 1.0, 1e-12) && rel(c2.length(), 1.0, 1e-12));
        assert!(c0.dot(c1).abs() < 1e-12 && c1.dot(c2).abs() < 1e-12 && c0.dot(c2).abs() < 1e-12);
        assert_eq!(m.col(3).truncate(), DVec3::ZERO);
    }
    // R_x(90): y -> z. R_z(90): x -> y. Then (0,1,0) under "xz" is x first:
    // (0,1,0) -> (0,0,1) -> (0,0,1); under "zx": (0,1,0) -> (-1,0,0) -> (-1,0,0).
    let r = DVec3::new(90.0, 0.0, 90.0);
    let y = DVec3::Y;
    assert!(v_close(euler_matrix(r, "xz").transform_point3(y), DVec3::Z, 1e-12));
    assert!(v_close(euler_matrix(r, "zx").transform_point3(y), DVec3::new(-1.0, 0.0, 0.0), 1e-12));
    // a full turn is the identity
    let full = euler_matrix(DVec3::splat(360.0), "xyz");
    assert!(full.abs_diff_eq(DMat4::IDENTITY, 1e-12));
}

/// Property: a rotation (about any pivot, plus translation) preserves all
/// pairwise distances, surface area, enclosed volume and the box's centroid
/// shift is exactly the rotated-and-translated centroid.
#[test]
fn geo_props_rotation_preserves_distances_area_and_volume() {
    let mut rng = Rng(6);
    for k in 0..40 {
        let g0 = make_sphere(rng.range(5.0, 60.0), 10, 16, rng.vec(-30.0, 30.0));
        let b0 = make_box(DVec3::new(rng.range(1.0, 50.0), rng.range(1.0, 50.0), rng.range(1.0, 50.0)), rng.vec(-30.0, 30.0), 2);
        for g0 in [g0, b0] {
            let t = rng.vec(-40.0, 40.0);
            let m = transform_matrix(t, rng.vec(-180.0, 180.0), ORDERS[k % 6], DVec3::ONE, rng.vec(-10.0, 10.0));
            let mut g = g0.clone();
            apply_transform(&mut g, m, true);
            let n = g0.points.len();
            for _ in 0..30 {
                let (i, j) = ((rng.next() % n as u64) as usize, (rng.next() % n as u64) as usize);
                assert!(rel((g.points[i] - g.points[j]).length(), (g0.points[i] - g0.points[j]).length(), 1e-12));
            }
            let (m0, m1) = (measure(&g0), measure(&g));
            assert!(rel(m1.area, m0.area, 1e-12), "area {} vs {}", m1.area, m0.area);
            assert!(rel(m1.volume, m0.volume, 1e-11), "volume {} vs {}", m1.volume, m0.volume);
            assert!(v_close(m1.centroid, m.transform_point3(m0.centroid), 1e-9));
            assert_eq!(m1.watertight(), m0.watertight());
        }
    }
}

/// Property: uniform scaling by s multiplies area by s^2 and volume by s^3
/// (and non-uniform scale multiplies volume by sx sy sz).
#[test]
fn geo_props_scaling_laws() {
    let mut rng = Rng(7);
    for _ in 0..60 {
        let s = rng.range(0.1, 5.0);
        let g0 = make_sphere(20.0, 8, 12, rng.vec(-10.0, 10.0));
        let m0 = measure(&g0);
        let mut g = g0.clone();
        apply_transform(&mut g, transform_matrix(DVec3::ZERO, DVec3::ZERO, "xyz", DVec3::splat(s), rng.vec(-20.0, 20.0)), true);
        let m1 = measure(&g);
        assert!(rel(m1.area, m0.area * s * s, 1e-11));
        assert!(rel(m1.volume, m0.volume * s * s * s, 1e-11));
        let sc = DVec3::new(rng.range(0.2, 4.0), rng.range(0.2, 4.0), rng.range(0.2, 4.0));
        let mut h = g0.clone();
        apply_transform(&mut h, transform_matrix(DVec3::ZERO, DVec3::ZERO, "xyz", sc, DVec3::ZERO), true);
        assert!(rel(measure(&h).volume, m0.volume * sc.x * sc.y * sc.z, 1e-11));
    }
}

/// Property: a mirroring matrix (det < 0) keeps the enclosed volume positive
/// when `fix_winding` is set and flips its sign when it is not.
#[test]
fn geo_props_mirroring_winding_fix() {
    let g0 = make_box(DVec3::new(10.0, 20.0, 30.0), DVec3::new(5.0, 5.0, 5.0), 1);
    let m = transform_matrix(DVec3::ZERO, DVec3::ZERO, "xyz", DVec3::new(-1.0, 1.0, 1.0), DVec3::ZERO);
    assert!(m.determinant() < 0.0);
    let (mut fixed, mut raw) = (g0.clone(), g0.clone());
    apply_transform(&mut fixed, m, true);
    apply_transform(&mut raw, m, false);
    assert!(rel(measure(&fixed).volume, 6000.0, 1e-12));
    assert!(rel(measure(&raw).volume, -6000.0, 1e-12));
}

/// Property: `mirror` reflects across the plane axis = origin; volume and
/// area are preserved, the bounding box is reflected, and keeping the
/// original (with a gap from the plane) doubles volume and area.
#[test]
fn geo_props_mirror_reflects_and_preserves_measures() {
    let g = make_box(DVec3::new(10.0, 6.0, 4.0), DVec3::new(20.0, 1.0, 2.0), 2);
    let m0 = measure(&g);
    for axis in 0..3 {
        let origin = 3.0;
        let (r, _) = mirror(&g, axis, origin, false, false, 1e-6);
        let m1 = measure(&r);
        assert!(rel(m1.volume, m0.volume, 1e-12) && rel(m1.area, m0.area, 1e-12));
        let (lo, hi) = (m0.bbox_min[axis], m0.bbox_max[axis]);
        assert!((m1.bbox_min[axis] - (2.0 * origin - hi)).abs() < 1e-9);
        assert!((m1.bbox_max[axis] - (2.0 * origin - lo)).abs() < 1e-9);
        let (both, _) = mirror(&g, axis, origin, true, false, 1e-6);
        let mb = measure(&both);
        assert!(rel(mb.volume, 2.0 * m0.volume, 1e-12) && rel(mb.area, 2.0 * m0.area, 1e-12));
        assert!(mb.watertight());
    }
}

// ------------------------------------------------------------- primitives

/// Property: a box has the requested bounding box, area 2(ab+bc+ca), volume abc,
/// 6n^2+2 welded points, 12 n^2 triangles, and is watertight, for every
/// subdivision count.
#[test]
fn geo_props_box_matches_closed_forms() {
    let mut rng = Rng(8);
    for n in [1usize, 2, 3, 5] {
        for _ in 0..10 {
            let s = DVec3::new(rng.range(1.0, 80.0), rng.range(1.0, 80.0), rng.range(1.0, 80.0));
            let c = rng.vec(-40.0, 40.0);
            let g = make_box(s, c, n);
            let m = measure(&g);
            assert!(v_close(m.bbox_min, c - s / 2.0, 1e-9) && v_close(m.bbox_max, c + s / 2.0, 1e-9));
            assert!(v_close(m.size(), s, 1e-9));
            assert!(rel(m.area, 2.0 * (s.x * s.y + s.y * s.z + s.z * s.x), 1e-12));
            assert!(rel(m.volume, s.x * s.y * s.z, 1e-12));
            assert!(v_close(m.centroid, c, 1e-9));
            assert!(m.watertight());
            assert_eq!(m.points, 6 * n * n + 2);
            assert_eq!(m.prims, 6 * n * n);
            assert_eq!(m.triangles, 12 * n * n);
            assert!(edges(&g).values().all(|&k| k == 2));
        }
    }
}

/// Property: a UV sphere is closed (Euler characteristic 2), spans exactly
/// +-R on z (poles) and, with an even number of rows and cols divisible by 4,
/// on x and y; its area and volume stay inside the inscribed-polyhedron
/// bounds derived from its own worst-case face angle.
#[test]
fn geo_props_sphere_is_closed_and_within_the_inscribed_bounds() {
    for (rows, cols) in [(8usize, 12usize), (16, 24), (24, 48)] {
        let (r, c) = (37.5, DVec3::new(4.0, -9.0, 11.0));
        let g = make_sphere(r, rows, cols, c);
        let m = measure(&g);
        assert!(m.watertight());
        // Euler characteristic V - E + F = 2
        let e = edges(&g);
        assert_eq!(g.points.len() as i64 - e.len() as i64 + g.prims.len() as i64, 2);
        assert_eq!(g.points.len(), (rows - 1) * cols + 2);
        assert!(v_close(m.bbox_min, c - DVec3::splat(r), 1e-9) && v_close(m.bbox_max, c + DVec3::splat(r), 1e-9));
        assert!(g.points.iter().all(|p| ((*p - c).length() - r).abs() < 1e-9));
        // worst angular circumradius a: plane distance h = R cos a of each face
        let mut cos_a_min: f64 = 1.0;
        for p in &g.prims {
            let (a, b, d) = (g.points[p.vertices[0] as usize], g.points[p.vertices[1] as usize], g.points[p.vertices[2] as usize]);
            let n = (b - a).cross(d - a).normalize();
            cos_a_min = cos_a_min.min((n.dot(a - c)).abs() / r);
        }
        let (a_s, v_s) = (4.0 * std::f64::consts::PI * r * r, 4.0 / 3.0 * std::f64::consts::PI * r.powi(3));
        assert!(m.area < a_s && m.area >= a_s * cos_a_min.powi(2), "area {} in [{}, {}]", m.area, a_s * cos_a_min.powi(2), a_s);
        assert!(m.volume < v_s && m.volume >= v_s * cos_a_min.powi(3), "volume {} in [{}, {}]", m.volume, v_s * cos_a_min.powi(3), v_s);
    }
    // refining brings the polyhedron closer to the sphere
    let v = |rows, cols| measure(&make_sphere(10.0, rows, cols, DVec3::ZERO)).volume;
    assert!(v(8, 12) < v(16, 24) && v(16, 24) < v(32, 48));
}

/// Property: a capped cylinder (equal radii) is an exact regular-prism: area
/// n R H 2 sin(pi/n) + 2 (n/2) R^2 sin(2pi/n), volume (n/2) R^2 sin(2pi/n) H, a
/// bounding box of the right size on each axis choice, and watertight.
#[test]
fn geo_props_cylinder_matches_the_regular_prism_closed_form() {
    let (r, h, n) = (20.0f64, 60.0f64, 24usize);
    let ngon = 0.5 * n as f64 * r * r * (2.0 * std::f64::consts::PI / n as f64).sin();
    let lateral = n as f64 * 2.0 * r * (std::f64::consts::PI / n as f64).sin() * h;
    for rows in [1usize, 3] {
        for axis in 0..3 {
            let c = DVec3::new(1.0, -2.0, 3.0);
            let g = make_tube(r, r, h, rows, n, true, axis, c);
            let m = measure(&g);
            assert!(m.watertight(), "axis {axis} rows {rows}");
            assert!(rel(m.area, lateral + 2.0 * ngon, 1e-12), "area {} vs {}", m.area, lateral + 2.0 * ngon);
            assert!(rel(m.volume, ngon * h, 1e-12));
            let mut want = DVec3::splat(2.0 * r);
            want[axis] = h;
            assert!(v_close(m.size(), want, 1e-9), "size {:?} vs {want:?}", m.size());
            assert!(v_close(0.5 * (m.bbox_min + m.bbox_max), c, 1e-9));
            // the tube is within the circumscribed cylinder volume pi R^2 H
            assert!(m.volume < std::f64::consts::PI * r * r * h);
        }
    }
    // open tube: two boundary loops of n edges each
    let open = measure(&make_tube(r, r, h, 2, n, false, 2, DVec3::ZERO));
    assert_eq!(open.open_edges, 2 * n);
    assert!(!open.watertight());
}

/// Property: a capped frustum (radii r0 -> r1) is a prismatoid, volume
/// H/3 (A0 + A1 + sqrt(A0 A1)) with A = (n/2) r^2 sin(2pi/n).
#[test]
fn geo_props_frustum_volume_is_the_prismatoid_formula() {
    let (r0, r1, h, n) = (30.0f64, 12.0f64, 45.0f64, 18usize);
    let a = |r: f64| 0.5 * n as f64 * r * r * (2.0 * std::f64::consts::PI / n as f64).sin();
    let want = h / 3.0 * (a(r0) + a(r1) + (a(r0) * a(r1)).sqrt());
    for rows in [1usize, 4] {
        let m = measure(&make_tube(r0, r1, h, rows, n, true, 2, DVec3::ZERO));
        assert!(rel(m.volume, want, 1e-11), "volume {} vs {want}", m.volume);
        assert!(m.watertight());
    }
}

/// Property: a grid is flat with area size_u * size_v, the flux volume above, the
/// right bounding box in each plane, and 2 (cols + rows) boundary edges.
#[test]
fn geo_props_grid_is_flat_with_the_requested_area() {
    let c = DVec3::new(3.0, -4.0, 5.0);
    for (plane, flat_axis) in [(GridPlane::Xy, 2usize), (GridPlane::Yz, 0), (GridPlane::Zx, 1)] {
        let g = make_grid(40.0, 25.0, 8, 5, plane, c);
        let m = measure(&g);
        assert!(rel(m.area, 1000.0, 1e-12));
        // "volume" of an open sheet is the divergence-theorem flux, area * (c . n) / 3
        // with n the +axis normal u x v, i.e. nonzero when the sheet is off the origin
        assert!(rel(m.volume, 1000.0 * c[flat_axis] / 3.0, 1e-12), "volume {}", m.volume);
        assert!(m.size()[flat_axis].abs() < 1e-12);
        assert!(v_close(0.5 * (m.bbox_min + m.bbox_max), c, 1e-9));
        assert_eq!(m.points, 9 * 6);
        assert_eq!(m.triangles, 2 * 8 * 5);
        assert_eq!(m.open_edges, 2 * (8 + 5));
        assert!(!m.watertight());
    }
}

// ---------------------------------------------------------------- measure

/// Property: `measure` agrees with area, volume, centroid and bounds computed
/// independently in this file (own fan triangulation), on random closed and
/// open shapes after random rigid + scale transforms.
#[test]
fn geo_props_measure_agrees_with_independent_computation() {
    let mut rng = Rng(9);
    for k in 0..40 {
        let base = match k % 3 {
            0 => make_sphere(rng.range(5.0, 40.0), 6 + k % 5, 8 + k % 7, rng.vec(-20.0, 20.0)),
            1 => make_box(rng.vec(2.0, 40.0), rng.vec(-20.0, 20.0), 1 + k % 3),
            _ => make_grid(rng.range(5.0, 50.0), rng.range(5.0, 50.0), 1 + k % 6, 1 + k % 4, GridPlane::Xy, rng.vec(-10.0, 10.0)),
        };
        let mut g = base;
        let sc = DVec3::new(rng.range(0.5, 2.0), rng.range(0.5, 2.0), rng.range(0.5, 2.0));
        apply_transform(&mut g, transform_matrix(rng.vec(-10.0, 10.0), rng.vec(-90.0, 90.0), ORDERS[k % 6], sc, DVec3::ZERO), true);
        let m = measure(&g);
        assert!(rel(m.area, area_of(&g), 1e-11));
        assert!(rel(m.volume, volume_of(&g), 1e-9) || (m.volume - volume_of(&g)).abs() < 1e-9);
        let mut w = DVec3::ZERO;
        for [a, b, c] in fan(&g) {
            w += (a + b + c) / 3.0 * (0.5 * (b - a).cross(c - a).length());
        }
        assert!(v_close(m.centroid, w / area_of(&g), 1e-9));
        let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
        for p in &g.points {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        assert!(v_close(m.bbox_min, lo, 1e-12) && v_close(m.bbox_max, hi, 1e-12));
        assert!(v_close(m.size(), hi - lo, 1e-12));
        assert_eq!(m.points, g.points.len());
        assert_eq!(m.prims, g.prims.len());
        assert_eq!(m.triangles, fan(&g).len());
        let e = edges(&g);
        assert_eq!(m.open_edges, e.values().filter(|&&c| c == 1).count());
        assert_eq!(m.non_manifold_edges, e.values().filter(|&&c| c > 2).count());
    }
}

/// Property: translating a closed mesh leaves volume and area unchanged and
/// shifts the centroid and bounding box by exactly the translation.
#[test]
fn geo_props_measure_translation_invariance() {
    let mut rng = Rng(10);
    for _ in 0..30 {
        let g0 = make_sphere(15.0, 10, 16, rng.vec(-10.0, 10.0));
        let t = rng.vec(-100.0, 100.0);
        let mut g = g0.clone();
        apply_transform(&mut g, DMat4::from_translation(t), true);
        let (a, b) = (measure(&g0), measure(&g));
        assert!(rel(b.volume, a.volume, 1e-10) && rel(b.area, a.area, 1e-12));
        assert!(v_close(b.centroid, a.centroid + t, 1e-9));
        assert!(v_close(b.bbox_min, a.bbox_min + t, 1e-9));
        assert!(v_close(b.bbox_max, a.bbox_max + t, 1e-9));
    }
}

/// Property: empty and point-cloud input: no points gives zero size and is
/// not watertight; a bare point cloud has no area and its centroid is the mean.
#[test]
fn geo_props_measure_degenerate_inputs() {
    let e = measure(&Geometry::new());
    assert_eq!((e.points, e.prims, e.triangles), (0, 0, 0));
    assert_eq!(e.size(), DVec3::ZERO);
    assert!(!e.watertight());
    let mut rng = Rng(11);
    let g = cloud(&mut rng, 50);
    let m = measure(&g);
    assert_eq!(m.area, 0.0);
    assert_eq!(m.triangles, 0);
    assert!(!m.watertight());
    let mean = g.points.iter().copied().sum::<DVec3>() / 50.0;
    assert!(v_close(m.centroid, mean, 1e-9));
}

//! Tests for the numerical building blocks: k-d tree, linear algebra and the
//! triangle-mesh distance structure (golden numbers from the spec).

use glam::{DMat3, DVec3};
use vernon_core::Geometry;

use crate::kdtree::KdTree;
use crate::linalg::*;
use crate::trimesh::*;
use crate::util::Rng;

fn cloud(n: usize, seed: u64) -> Vec<DVec3> {
    let mut r = Rng::new(seed);
    (0..n)
        .map(|_| DVec3::new(r.next_f64() * 100.0, r.next_f64() * 50.0, r.next_f64() * 20.0))
        .collect()
}

#[test]
fn kd_nearest_matches_brute_force() {
    let pts = cloud(3000, 1);
    let tree = KdTree::new(&pts);
    for q in cloud(200, 2) {
        let (i, d2) = tree.nearest(q).unwrap();
        let (bi, bd2) = pts
            .iter()
            .enumerate()
            .map(|(i, p)| (i, (*p - q).length_squared()))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        assert!((d2 - bd2).abs() < 1e-9);
        assert_eq!(i as usize, bi);
    }
}

#[test]
fn kd_knn_and_radius_match_brute_force() {
    let pts = cloud(2000, 3);
    let tree = KdTree::new(&pts);
    let mut out = Vec::new();
    let mut rad = Vec::new();
    for q in cloud(50, 4) {
        tree.knn(q, 9, &mut out);
        let mut brute: Vec<f64> = pts.iter().map(|p| (*p - q).length_squared()).collect();
        brute.sort_by(f64::total_cmp);
        assert_eq!(out.len(), 9);
        for (k, e) in out.iter().enumerate() {
            assert!((e.0 - brute[k]).abs() < 1e-9);
        }
        tree.radius(q, 6.0, &mut rad);
        let want = pts.iter().filter(|p| (**p - q).length() <= 6.0).count();
        assert_eq!(rad.len(), want);
        assert_eq!(tree.radius_count(q, 6.0, usize::MAX), want);
    }
}

#[test]
fn kd_handles_duplicates_and_empty() {
    let pts = vec![DVec3::ONE; 100];
    let tree = KdTree::new(&pts);
    let mut out = Vec::new();
    tree.knn(DVec3::ONE, 5, &mut out);
    assert_eq!(out.len(), 5);
    assert!(KdTree::new(&[]).nearest(DVec3::ZERO).is_none());
    assert!(tree.nearest_within(DVec3::splat(50.0), 1.0).is_none());
}

fn base_points() -> Vec<DVec3> {
    vec![
        DVec3::new(0.0, 0.0, 0.0),
        DVec3::new(100.0, 0.0, 0.0),
        DVec3::new(0.0, 50.0, 0.0),
        DVec3::new(0.0, 0.0, 25.0),
    ]
}

#[test]
fn golden_1_closed_form_noiseless() {
    let p = base_points();
    let r = DMat3::from_rotation_z(10f64.to_radians());
    let t = DVec3::new(5.0, -3.0, 2.0);
    let q: Vec<DVec3> = p.iter().map(|x| r * *x + t).collect();
    let horn = fit_rigid(&p, &q, None, false).unwrap();
    let svd = fit_rigid_svd(&p, &q, None).unwrap();
    for f in [horn, svd] {
        assert!(f.rms < 1e-9, "rms {}", f.rms);
        assert!((f.t - t).length() < 1e-9);
        for c in 0..3 {
            assert!((f.r.col(c) - r.col(c)).length() < 1e-9);
        }
    }
    for c in 0..3 {
        assert!((horn.r.col(c) - svd.r.col(c)).length() < 1e-9);
    }
}

#[test]
fn golden_2_degenerate_inputs() {
    let p = base_points();
    assert!(matches!(fit_rigid(&p[..2], &p[..2], None, false), Err(FitError::TooFew(2))));
    let line = [DVec3::ZERO, DVec3::X, DVec3::X * 2.0];
    assert!(matches!(fit_rigid(&line, &line, None, false), Err(FitError::Degenerate)));
    assert!(matches!(fit_rigid_svd(&line, &line, None), Err(FitError::Degenerate)));
    assert!(matches!(fit_rigid(&p, &p[..3], None, false), Err(FitError::Mismatch(4, 3))));
}

#[test]
fn golden_3_scale() {
    let p = base_points();
    let q: Vec<DVec3> = p.iter().map(|x| *x * 1.02).collect();
    let s = fit_rigid(&p, &q, None, true).unwrap();
    assert!((s.s - 1.02).abs() < 1e-6);
    assert!(s.rms < 1e-9);
    let fixed = fit_rigid(&p, &q, None, false).unwrap();
    assert!(fixed.rms > s.rms + 0.1);
}

#[test]
fn reflection_is_not_returned() {
    let p = base_points();
    let q: Vec<DVec3> = p.iter().map(|x| DVec3::new(-x.x, x.y, x.z)).collect();
    let f = fit_rigid(&p, &q, None, false).unwrap();
    assert!((f.r.determinant() - 1.0).abs() < 1e-9);
    let g = fit_rigid_svd(&p, &q, None).unwrap();
    assert!((g.r.determinant() - 1.0).abs() < 1e-9);
}

#[test]
fn eigen_and_svd_reconstruct() {
    let m = DMat3::from_cols(DVec3::new(4.0, 1.0, 0.5), DVec3::new(1.0, 3.0, 0.2), DVec3::new(0.5, 0.2, 1.0));
    let (vals, vecs) = eigen3(&m);
    assert!(vals[0] >= vals[1] && vals[1] >= vals[2]);
    for k in 0..3 {
        assert!((m * vecs[k] - vecs[k] * vals[k]).length() < 1e-9);
    }
    let a = DMat3::from_cols(DVec3::new(1.0, 2.0, 3.0), DVec3::new(0.0, 1.0, 4.0), DVec3::new(5.0, 6.0, 0.0));
    let (u, s, v) = svd3(a);
    let rec = u * DMat3::from_diagonal(s) * v.transpose();
    for c in 0..3 {
        assert!((rec.col(c) - a.col(c)).length() < 1e-9);
    }
}

#[test]
fn pinv_reports_free_direction() {
    let mut a: Mat = [[0.0; MAXN]; MAXN];
    a[0][0] = 4.0;
    a[1][1] = 1.0;
    let mut b = [0.0; MAXN];
    b[0] = 8.0;
    b[1] = 3.0;
    let s = solve_pinv(3, &a, &b, 1e8);
    assert!((s.x[0] - 2.0).abs() < 1e-12 && (s.x[1] - 3.0).abs() < 1e-12 && s.x[2].abs() < 1e-12);
    assert_eq!(s.free.len(), 1);
    assert!(s.free[0][2].abs() > 0.99);
    assert!(s.cond >= 1e30);
}

fn plate() -> TriMesh {
    let pos = vec![
        DVec3::new(-100.0, -100.0, 0.0),
        DVec3::new(100.0, -100.0, 0.0),
        DVec3::new(100.0, 100.0, 0.0),
        DVec3::new(-100.0, 100.0, 0.0),
    ];
    TriMesh::new(pos, vec![[0, 1, 2], [0, 2, 3]], vec![0, 0])
}

fn cube(a: f64) -> TriMesh {
    TriMesh::from_geometry(&Geometry::make_box(DVec3::splat(a))).unwrap()
}

#[test]
fn golden_9_signed_plate() {
    let m = plate();
    let s = |p: DVec3| m.closest(p, f64::INFINITY, SignMode::Pseudo).unwrap().signed;
    assert!((s(DVec3::new(10.0, 10.0, 0.3)) - 0.3).abs() < 1e-12);
    assert!((s(DVec3::new(-20.0, 5.0, -0.4)) + 0.4).abs() < 1e-12);
    assert!(s(DVec3::new(50.0, 50.0, 0.0)).abs() < 1e-12);
    assert_eq!(m.topo.open_edges, 4);
}

#[test]
fn golden_10_cube_edge_sign() {
    let m = cube(100.0);
    assert!(m.topo.is_closed(), "{:?}", m.topo);
    assert!((m.topo.signed_volume - 1.0e6).abs() < 1e-3);
    let h = m.closest(DVec3::new(60.0, 60.0, 0.0), f64::INFINITY, SignMode::Pseudo).unwrap();
    assert!((h.signed - 14.142136).abs() < 1e-5, "{}", h.signed);
    assert!(matches!(h.feature, Feature::Edge(_)));
    let c = m.closest(DVec3::ZERO, f64::INFINITY, SignMode::Pseudo).unwrap();
    assert!((c.signed + 50.0).abs() < 1e-9);
    let v = m.closest(DVec3::new(60.0, 60.0, 60.0), f64::INFINITY, SignMode::Pseudo).unwrap();
    assert!(v.signed > 0.0);
    assert!(matches!(v.feature, Feature::Vertex(_)));
}

#[test]
fn max_distance_cap_and_unsigned() {
    let m = cube(100.0);
    assert!(m.closest(DVec3::new(200.0, 0.0, 0.0), 10.0 * 10.0, SignMode::None).is_none());
    assert!((m.distance(DVec3::new(70.0, 0.0, 0.0)) - 20.0).abs() < 1e-9);
}


#[test]
fn bvh_matches_brute_force_on_a_sphere() {
    let (pos, tris) = crate::testkit::uv_sphere(50.0, 24, 12);
    let n = tris.len();
    let m = TriMesh::new(pos, tris, vec![0; n]);
    assert!(m.topo.is_closed(), "{:?}", m.topo);
    assert!(m.topo.signed_volume > 0.0);
    let mut rng = Rng::new(5);
    for _ in 0..300 {
        let q = DVec3::new(rng.next_f64() - 0.5, rng.next_f64() - 0.5, rng.next_f64() - 0.5) * 160.0;
        let h = m.closest(q, f64::INFINITY, SignMode::Pseudo).unwrap();
        let brute = (0..m.tris.len() as u32)
            .map(|t| {
                let [a, b, c] = m.tri_points(t);
                (q - closest_point_triangle(q, a, b, c).0).length()
            })
            .fold(f64::INFINITY, f64::min);
        assert!((h.dist - brute).abs() < 1e-9);
        if (q.length() - 50.0).abs() > 3.0 {
            assert_eq!(h.signed > 0.0, q.length() > 50.0);
        }
    }
}

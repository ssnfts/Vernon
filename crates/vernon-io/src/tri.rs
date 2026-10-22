//! Triangulation of polygons for the triangle-only writers (STL, 3MF).
//!
//! `Geometry::triangles()` fans every polygon from its first vertex, which
//! flips or overlaps triangles on concave quads and n-gons. Here quads pick
//! the diagonal whose triangles agree with the polygon's Newell normal, and
//! larger concave polygons are ear-clipped in the plane of that normal.
//! Triangles keep the polygon's winding.

use vernon_core::{DVec3, Geometry};

/// Polygons larger than this are fanned: ear clipping is cubic.
const MAX_EAR_CLIP: usize = 2_000;

/// Newell normal of a ring of points (length is twice the area).
fn newell(points: &[DVec3], ring: &[u32]) -> DVec3 {
    let mut n = DVec3::ZERO;
    let mut prev = points[ring[ring.len() - 1] as usize];
    for &i in ring {
        let cur = points[i as usize];
        n += prev.cross(cur);
        prev = cur;
    }
    n
}

/// Cosine-like agreement of a triangle with `n` (unit); 0 for degenerate.
fn agreement(points: &[DVec3], t: [u32; 3], n: DVec3) -> f64 {
    let [a, b, c] = t.map(|i| points[i as usize]);
    (b - a).cross(c - a).normalize_or_zero().dot(n)
}

/// Triangulate one closed polygon, appending to `out`. Consecutive duplicate
/// indices (and a repeated closing vertex) are dropped first.
pub(crate) fn triangulate_ring(points: &[DVec3], verts: &[u32], out: &mut Vec<[u32; 3]>) {
    let mut ring: Vec<u32> = Vec::with_capacity(verts.len());
    for &v in verts {
        if ring.last() != Some(&v) {
            ring.push(v);
        }
    }
    while ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    match ring.len() {
        0..=2 => return,
        3 => {
            if ring[0] != ring[2] && ring[0] != ring[1] && ring[1] != ring[2] {
                out.push([ring[0], ring[1], ring[2]]);
            }
            return;
        }
        _ => {}
    }
    let nrm = newell(points, &ring);
    let area2 = nrm.length();
    let fan = |out: &mut Vec<[u32; 3]>| {
        for k in 1..ring.len() - 1 {
            out.push([ring[0], ring[k], ring[k + 1]]);
        }
    };
    if area2 <= 1e-300 {
        return fan(out);
    }
    let nu = nrm / area2;
    if ring.len() == 4 {
        let (a, b, c, d) = (ring[0], ring[1], ring[2], ring[3]);
        let score = |t1: [u32; 3], t2: [u32; 3]| agreement(points, t1, nu).min(agreement(points, t2, nu));
        let s02 = score([a, b, c], [a, c, d]);
        let s13 = score([b, c, d], [b, d, a]);
        if s13 > s02 + 1e-12 {
            out.extend([[b, c, d], [b, d, a]]);
        } else {
            out.extend([[a, b, c], [a, c, d]]);
        }
        return;
    }
    if ring.len() > MAX_EAR_CLIP || is_convex(points, &ring, nu) {
        return fan(out);
    }
    ear_clip(points, &ring, nu, out);
}

/// All turns agree with the normal (collinear vertices allowed).
fn is_convex(points: &[DVec3], ring: &[u32], nu: DVec3) -> bool {
    let n = ring.len();
    let scale = ring.iter().map(|&i| points[i as usize].abs().max_element()).fold(0.0, f64::max);
    let eps = 1e-12 * scale * scale;
    (0..n).all(|k| {
        let a = points[ring[k] as usize];
        let b = points[ring[(k + 1) % n] as usize];
        let c = points[ring[(k + 2) % n] as usize];
        (b - a).cross(c - b).dot(nu) >= -eps
    })
}

fn ear_clip(points: &[DVec3], ring: &[u32], nu: DVec3, out: &mut Vec<[u32; 3]>) {
    // Orthonormal frame (u, v, nu) so the polygon is counter-clockwise in (u, v).
    let helper = if nu.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
    let u = nu.cross(helper).normalize();
    let v = nu.cross(u);
    let pts: Vec<(f64, f64)> = ring
        .iter()
        .map(|&i| {
            let p = points[i as usize];
            (p.dot(u), p.dot(v))
        })
        .collect();
    let extent = pts.iter().fold(0.0f64, |m, p| m.max(p.0.abs()).max(p.1.abs()));
    let eps = 1e-12 * extent * extent;
    let cross = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
    let mut idx: Vec<usize> = (0..ring.len()).collect();
    while idx.len() > 3 {
        let m = idx.len();
        let mut best: Option<usize> = None;
        let mut fallback = (0usize, f64::NEG_INFINITY);
        for k in 0..m {
            let (ia, ib, ic) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            let (a, b, c) = (pts[ia], pts[ib], pts[ic]);
            let turn = cross(a, b, c);
            if turn > fallback.1 {
                fallback = (k, turn);
            }
            if turn <= eps {
                continue;
            }
            let blocked = idx.iter().any(|&j| {
                if j == ia || j == ib || j == ic {
                    return false;
                }
                let p = pts[j];
                if [a, b, c].iter().any(|q| (q.0 - p.0).abs() <= 1e-12 * extent && (q.1 - p.1).abs() <= 1e-12 * extent) {
                    return false; // coincident with a corner
                }
                cross(a, b, p) >= -eps && cross(b, c, p) >= -eps && cross(c, a, p) >= -eps
            });
            if !blocked {
                best = Some(k);
                break;
            }
        }
        // No ear (degenerate input): clip the sharpest convex corner anyway.
        let k = best.unwrap_or(fallback.0);
        let (ia, ib, ic) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
        out.push([ring[ia], ring[ib], ring[ic]]);
        idx.remove(k);
    }
    out.push([ring[idx[0]], ring[idx[1]], ring[idx[2]]]);
}

/// Triangulate every closed polygon of `geo`; each triangle is tagged with the
/// index of the primitive it came from.
pub(crate) fn triangulate(geo: &Geometry) -> Vec<(usize, [u32; 3])> {
    let mut out = Vec::with_capacity(geo.prims.len());
    let mut tmp = Vec::new();
    for (pi, prim) in geo.prims.iter().enumerate() {
        if !prim.closed {
            continue;
        }
        tmp.clear();
        triangulate_ring(&geo.points, &prim.vertices, &mut tmp);
        out.extend(tmp.iter().map(|&t| (pi, t)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(points: &[DVec3], tris: &[[u32; 3]]) -> f64 {
        tris.iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| points[i as usize]);
                (b - a).cross(c - a).length() / 2.0
            })
            .sum()
    }

    fn min_agreement(points: &[DVec3], ring: &[u32], tris: &[[u32; 3]]) -> f64 {
        let n = newell(points, ring).normalize();
        tris.iter().map(|&t| agreement(points, t, n)).fold(1.0, f64::min)
    }

    fn pts2(v: &[(f64, f64)]) -> Vec<DVec3> {
        v.iter().map(|&(x, y)| DVec3::new(x, y, 0.0)).collect()
    }

    #[test]
    fn triangle_and_convex_quad_match_the_fan() {
        let p = pts2(&[(0., 0.), (2., 0.), (2., 2.), (0., 2.)]);
        let mut out = Vec::new();
        triangulate_ring(&p, &[0, 1, 2, 3], &mut out);
        assert_eq!(out, vec![[0, 1, 2], [0, 2, 3]]);
        out.clear();
        triangulate_ring(&p, &[0, 1, 2], &mut out);
        assert_eq!(out, vec![[0, 1, 2]]);
        out.clear();
        triangulate_ring(&p, &[0, 1, 1, 0], &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn dart_quad_uses_the_interior_diagonal() {
        // Reflex vertex at index 1: the 0-2 diagonal lies outside.
        let p = pts2(&[(4., 0.), (1., 1.), (0., 4.), (0., 0.)]);
        let ring = [0, 1, 2, 3];
        let mut out = Vec::new();
        triangulate_ring(&p, &ring, &mut out);
        assert_eq!(out.len(), 2);
        assert!(min_agreement(&p, &ring, &out) > 0.999, "{out:?}");
        let g = Geometry::from_polygons(p.clone(), vec![ring.to_vec()]);
        assert!((area(&p, &out) - g.prim_area(0)).abs() < 1e-9);
        // Same dart with the reflex vertex at index 0 keeps the 0-2 fan.
        let p = pts2(&[(1., 1.), (0., 4.), (0., 0.), (4., 0.)]);
        out.clear();
        triangulate_ring(&p, &ring, &mut out);
        assert!(min_agreement(&p, &ring, &out) > 0.999);
        assert!((area(&p, &out) - 4.0).abs() < 1e-9);
    }

    #[test]
    fn concave_ngon_is_ear_clipped_without_overlap() {
        // An L shape (6 vertices, reflex at index 3) and a comb with collinear points.
        let l = pts2(&[(0., 0.), (4., 0.), (4., 2.), (2., 2.), (2., 4.), (0., 4.)]);
        let ring: Vec<u32> = (0..6).collect();
        let mut out = Vec::new();
        triangulate_ring(&l, &ring, &mut out);
        assert_eq!(out.len(), 4);
        assert!((area(&l, &out) - 12.0).abs() < 1e-9);
        assert!(min_agreement(&l, &ring, &out) > 0.999);

        let comb = pts2(&[
            (0., 0.), (1., 0.), (2., 0.), (3., 0.), (3., 3.), (2., 3.), (2., 1.), (1., 1.), (1., 3.), (0., 3.),
        ]);
        let ring: Vec<u32> = (0..10).collect();
        out.clear();
        triangulate_ring(&comb, &ring, &mut out);
        assert_eq!(out.len(), 8);
        let g = Geometry::from_polygons(comb.clone(), vec![ring.clone()]);
        assert!((area(&comb, &out) - g.prim_area(0)).abs() < 1e-9);
        assert!(min_agreement(&comb, &ring, &out) > 0.999, "{out:?}");
    }

    #[test]
    fn ear_clipping_works_off_axis_and_for_clockwise_rings() {
        // The L shape rotated into an oblique plane, and reversed.
        let l = pts2(&[(0., 0.), (4., 0.), (4., 2.), (2., 2.), (2., 4.), (0., 4.)]);
        let rot = vernon_core::DMat4::from_rotation_x(0.7) * vernon_core::DMat4::from_rotation_y(-0.4);
        let p: Vec<DVec3> = l.iter().map(|&v| rot.transform_point3(v)).collect();
        for ring in [[0, 1, 2, 3, 4, 5], [5, 4, 3, 2, 1, 0]] {
            let mut out = Vec::new();
            triangulate_ring(&p, &ring, &mut out);
            assert_eq!(out.len(), 4);
            assert!((area(&p, &out) - 12.0).abs() < 1e-9);
            assert!(min_agreement(&p, &ring, &out) > 0.999);
        }
    }

    #[test]
    fn degenerate_rings_do_not_panic() {
        let p = pts2(&[(0., 0.), (1., 0.), (2., 0.), (3., 0.), (4., 0.)]);
        let mut out = Vec::new();
        triangulate_ring(&p, &[0, 1, 2, 3, 4], &mut out);
        assert_eq!(out.len(), 3);
        let same = vec![DVec3::ONE; 6];
        out.clear();
        triangulate_ring(&same, &[0, 1, 2, 3, 4, 5], &mut out);
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn convex_ngon_fans_and_open_prims_are_skipped() {
        let mut g = Geometry::from_polygons(
            pts2(&[(0., 0.), (2., 0.), (3., 1.), (2., 2.), (0., 2.)]),
            vec![vec![0, 1, 2, 3, 4]],
        );
        g.add_prim(vernon_core::Prim::polyline(vec![0, 1]));
        let t = triangulate(&g);
        assert_eq!(t.len(), 3);
        assert!(t.iter().all(|(pi, _)| *pi == 0));
    }
}

//! Property tests of `KdTree` and the triangle-mesh distance query against
//! brute force, with a seeded xorshift generator (no extra crates).
//!
//! NOTES
//! * `distance.rs` only exposes node registration and cook functions
//!   publicly; its distance maths lives in `trimesh::TriMesh`, which is what
//!   the distance-to-mesh tests below exercise.
//! * `KdTree::radius` squares `r`, so a negative `r` behaves like `|r|`. This
//!   is not documented either way, so no test asserts it (possible doubt).
//! * `radius_count`'s contract at the cap ("stopping early") does not say
//!   whether the result may exceed `cap`; the test accepts any value >= cap
//!   when the true count reaches the cap, and the exact count below it.

use glam::DVec3;
use vernon_nodes_clean::kdtree::KdTree;
use vernon_nodes_clean::trimesh::{SignMode, TriMesh};

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
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Point-set families: random, with exact duplicates, collinear, all identical,
/// and an integer lattice (exact arithmetic, many distance ties).
fn point_sets(rng: &mut Rng, n: usize) -> Vec<(&'static str, Vec<DVec3>)> {
    let random: Vec<DVec3> = (0..n).map(|_| rng.vec(-50.0, 50.0)).collect();
    let mut dup = random.clone();
    for i in 0..n {
        if rng.below(3) == 0 {
            dup[i] = dup[rng.below(n)];
        }
    }
    let dir = DVec3::new(1.0, 2.0, -0.5).normalize();
    let line: Vec<DVec3> = (0..n).map(|_| dir * rng.range(-40.0, 40.0) + DVec3::new(3.0, 1.0, 2.0)).collect();
    let same = vec![DVec3::new(1.5, -2.5, 4.0); n];
    let lattice: Vec<DVec3> = (0..n).map(|_| DVec3::new(rng.below(6) as f64, rng.below(6) as f64, rng.below(6) as f64)).collect();
    vec![("random", random), ("duplicates", dup), ("collinear", line), ("identical", same), ("lattice", lattice)]
}

fn d2(a: DVec3, b: DVec3) -> f64 {
    let d = a - b;
    d.x * d.x + d.y * d.y + d.z * d.z
}

fn queries(rng: &mut Rng, pts: &[DVec3]) -> Vec<DVec3> {
    let mut q: Vec<DVec3> = (0..12).map(|_| rng.vec(-60.0, 60.0)).collect();
    q.extend((0..4).map(|_| pts[rng.below(pts.len())])); // exact hits
    q.push(DVec3::ZERO);
    q
}

const SIZES: [usize; 4] = [1, 2, 10, 1000];

/// Property: nearest returns a point at the brute-force minimum distance (the
/// index may be any of the tied points) and `nearest_within` agrees with the
/// strict `< max_d2` cut.
#[test]
fn kdtree_props_nearest_matches_brute_force() {
    let mut rng = Rng(0xDEAD_BEEF_1234_5678);
    for &n in &SIZES {
        for (name, pts) in point_sets(&mut rng, n) {
            let tree = KdTree::new(&pts);
            assert_eq!(tree.len(), n);
            for q in queries(&mut rng, &pts) {
                let best = pts.iter().map(|p| d2(*p, q)).fold(f64::INFINITY, f64::min);
                let (i, got) = tree.nearest(q).expect("non-empty tree");
                assert!((got - best).abs() <= 1e-9 * best.max(1.0), "{name} n={n}: {got} vs {best}");
                assert!((d2(pts[i as usize], q) - got).abs() <= 1e-9 * best.max(1.0), "{name}: index/distance mismatch");
                // strict upper cut
                assert!(tree.nearest_within(q, best).is_none(), "{name}: cut at best must exclude it");
                let w = tree.nearest_within(q, best * 1.0001 + 1e-9).expect("within");
                assert!((w.1 - best).abs() <= 1e-9 * best.max(1.0));
            }
        }
    }
}

/// Property: knn returns min(k, n) hits, sorted by ascending squared distance,
/// with distinct valid indices whose distances equal the brute-force k smallest.
#[test]
fn kdtree_props_knn_order_count_and_distances() {
    let mut rng = Rng(0xABCD_EF01);
    let mut out = Vec::new();
    for &n in &SIZES {
        for (name, pts) in point_sets(&mut rng, n) {
            let tree = KdTree::new(&pts);
            for q in queries(&mut rng, &pts) {
                let mut all: Vec<f64> = pts.iter().map(|p| d2(*p, q)).collect();
                all.sort_by(f64::total_cmp);
                for k in [0usize, 1, 3, 7, n, n + 5] {
                    tree.knn(q, k, &mut out);
                    assert_eq!(out.len(), k.min(n), "{name} n={n} k={k}");
                    let mut seen = std::collections::HashSet::new();
                    for (r, &(dd, idx)) in out.iter().enumerate() {
                        assert!((idx as usize) < n && seen.insert(idx), "{name}: bad or repeated index");
                        assert!((d2(pts[idx as usize], q) - dd).abs() <= 1e-9 * dd.max(1.0));
                        assert!((dd - all[r]).abs() <= 1e-9 * all[r].max(1.0), "{name} n={n} k={k} rank {r}");
                        if r > 0 {
                            assert!(out[r - 1].0 <= dd, "{name}: not sorted");
                        }
                    }
                    // when the k-th and (k+1)-th distances differ the index set is unique
                    if k > 0 && k < n && all[k - 1] < all[k] - 1e-9 {
                        let brute: std::collections::HashSet<u32> = {
                            let mut idx: Vec<u32> = (0..n as u32).collect();
                            idx.sort_by(|&a, &b| d2(pts[a as usize], q).total_cmp(&d2(pts[b as usize], q)));
                            idx[..k].iter().copied().collect()
                        };
                        assert_eq!(seen, brute, "{name} n={n} k={k}");
                    }
                }
            }
        }
    }
}

/// Property: radius search returns exactly the set of points with
/// distance <= r (boundary inclusive, on the exact integer lattice too), and
/// `radius_count` equals that set size below the cap.
#[test]
fn kdtree_props_radius_set_equals_brute_force() {
    let mut rng = Rng(0x1357_9BDF);
    let mut out = Vec::new();
    for &n in &SIZES {
        for (name, pts) in point_sets(&mut rng, n) {
            let tree = KdTree::new(&pts);
            for q in queries(&mut rng, &pts) {
                for r in [0.0, 0.5, 2.0, 7.5, 30.0, 500.0] {
                    tree.radius(q, r, &mut out);
                    let mut got = out.clone();
                    got.sort_unstable();
                    let want: Vec<u32> = (0..n as u32).filter(|&i| d2(pts[i as usize], q) <= r * r).collect();
                    assert_eq!(got, want, "{name} n={n} r={r}");
                    let cnt = tree.radius_count(q, r, usize::MAX);
                    assert_eq!(cnt, want.len(), "{name} count r={r}");
                    let cap = 3;
                    let capped = tree.radius_count(q, r, cap);
                    if want.len() >= cap {
                        assert!(capped >= cap && capped <= want.len(), "{name}: cap {capped} vs {}", want.len());
                    } else {
                        assert_eq!(capped, want.len());
                    }
                }
            }
        }
    }
    // exact inclusivity on integers: points at distance exactly 2 are inside r = 2
    let pts: Vec<DVec3> = (-3..=3).map(|x| DVec3::new(x as f64, 0.0, 0.0)).collect();
    let tree = KdTree::new(&pts);
    tree.radius(DVec3::ZERO, 2.0, &mut out);
    assert_eq!(out.len(), 5);
}

/// Property: results do not depend on the order the points are supplied in:
/// distances are identical and, mapped back through the permutation, the
/// radius sets are equal.
#[test]
fn kdtree_props_results_unchanged_when_input_is_shuffled() {
    let mut rng = Rng(0x2468_ACE0);
    let mut out_a = Vec::new();
    let mut out_b = Vec::new();
    for &n in &[2usize, 10, 1000] {
        for (name, pts) in point_sets(&mut rng, n) {
            // perm[j] = original index of the point placed at position j
            let mut perm: Vec<usize> = (0..n).collect();
            for i in (1..n).rev() {
                perm.swap(i, rng.below(i + 1));
            }
            let shuffled: Vec<DVec3> = perm.iter().map(|&i| pts[i]).collect();
            let (ta, tb) = (KdTree::new(&pts), KdTree::new(&shuffled));
            for q in queries(&mut rng, &pts) {
                let (_, da) = ta.nearest(q).unwrap();
                let (_, db) = tb.nearest(q).unwrap();
                assert_eq!(da, db, "{name} nearest distance");
                ta.knn(q, 6, &mut out_a);
                tb.knn(q, 6, &mut out_b);
                let (la, lb): (Vec<f64>, Vec<f64>) = (out_a.iter().map(|x| x.0).collect(), out_b.iter().map(|x| x.0).collect());
                assert_eq!(la, lb, "{name} knn distances");
                let mut ra = Vec::new();
                let mut rb = Vec::new();
                ta.radius(q, 12.0, &mut ra);
                tb.radius(q, 12.0, &mut rb);
                let mut ra: Vec<usize> = ra.iter().map(|&i| i as usize).collect();
                let mut rb: Vec<usize> = rb.iter().map(|&j| perm[j as usize]).collect();
                ra.sort_unstable();
                rb.sort_unstable();
                assert_eq!(ra, rb, "{name} radius set");
            }
        }
    }
}

/// Property: an empty tree answers every query with "nothing".
#[test]
fn kdtree_props_empty_tree() {
    let tree = KdTree::new(&[]);
    assert!(tree.is_empty());
    assert_eq!(tree.len(), 0);
    let q = DVec3::new(1.0, 2.0, 3.0);
    assert!(tree.nearest(q).is_none());
    assert!(tree.nearest_within(q, f64::INFINITY).is_none());
    let mut k = vec![(1.0, 1u32)];
    tree.knn(q, 5, &mut k);
    assert!(k.is_empty(), "knn must clear the output");
    let mut r = vec![9u32];
    tree.radius(q, 10.0, &mut r);
    assert!(r.is_empty(), "radius must clear the output");
    assert_eq!(tree.radius_count(q, 10.0, 100), 0);
}

// ------------------------------------------------- distance to triangle mesh

fn seg_d2(p: DVec3, a: DVec3, b: DVec3) -> f64 {
    let ab = b - a;
    let l2 = ab.length_squared();
    let t = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + ab * t)).length_squared()
}

/// Brute-force point-triangle distance: perpendicular foot when it falls
/// inside, otherwise the nearest of the three edges.
fn tri_dist(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> f64 {
    let n = (b - a).cross(c - a);
    let n2 = n.length_squared();
    let edges = seg_d2(p, a, b).min(seg_d2(p, b, c)).min(seg_d2(p, c, a));
    if n2 < 1e-24 {
        return edges.sqrt();
    }
    let h = (p - a).dot(n) / n2.sqrt();
    let foot = p - n / n2.sqrt() * h;
    let inside = (b - a).cross(foot - a).dot(n) >= 0.0 && (c - b).cross(foot - b).dot(n) >= 0.0 && (a - c).cross(foot - c).dot(n) >= 0.0;
    if inside {
        h.abs()
    } else {
        edges.sqrt()
    }
}

fn soup(rng: &mut Rng, nt: usize) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let mut pos = Vec::new();
    let mut tris = Vec::new();
    for t in 0..nt {
        let base = rng.vec(-30.0, 30.0);
        for _ in 0..3 {
            pos.push(base + rng.vec(-12.0, 12.0));
        }
        let i = (3 * t) as u32;
        tris.push([i, i + 1, i + 2]);
    }
    (pos, tris)
}

/// Property: TriMesh::distance equals the brute-force minimum point-triangle
/// distance on random triangle soups of 1, 2, 10 and 300 triangles; `closest`
/// returns a point on the mesh at that distance and honours the squared cap.
#[test]
fn kdtree_props_distance_to_triangle_mesh_matches_brute_force() {
    let mut rng = Rng(0x0F0F_1234_ABCD);
    for &nt in &[1usize, 2, 10, 300] {
        let (pos, tris) = soup(&mut rng, nt);
        let mesh = TriMesh::new(pos.clone(), tris.clone(), vec![0; nt]);
        for _ in 0..60 {
            let q = rng.vec(-60.0, 60.0);
            let want = tris.iter().map(|t| tri_dist(q, pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize])).fold(f64::INFINITY, f64::min);
            let got = mesh.distance(q);
            assert!((got - want).abs() <= 1e-9 * want.max(1.0), "nt={nt}: {got} vs {want}");
            let hit = mesh.closest(q, f64::INFINITY, SignMode::None).unwrap();
            assert!((hit.dist - got).abs() < 1e-12);
            assert!(((hit.point - q).length() - got).abs() <= 1e-9 * got.max(1.0));
            // the reported point lies on the reported triangle
            let t = tris[hit.tri as usize];
            let on = tri_dist(hit.point, pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
            assert!(on < 1e-9, "closest point is {on} mm off its triangle");
            // squared cap is a strict upper bound
            // (a relative margin absorbs the last-bit difference between the two distance formulas)
            assert!(mesh.closest(q, (want * (1.0 - 1e-6)).powi(2), SignMode::None).is_none());
            assert!(mesh.closest(q, (want * (1.0 + 1e-6) + 1e-9).powi(2), SignMode::None).is_some());
        }
        // a vertex of the mesh is at distance zero
        assert!(mesh.distance(pos[0]) < 1e-12);
    }
}

/// Property: on a closed, outward-wound box the signed distance equals the
/// analytic box value (positive outside, negative inside) for both sign modes.
#[test]
fn kdtree_props_signed_distance_to_a_box_is_analytic() {
    let h = DVec3::new(10.0, 6.0, 4.0); // half extents
    let corner = |i: usize| DVec3::new(if i & 1 == 0 { -h.x } else { h.x }, if i & 2 == 0 { -h.y } else { h.y }, if i & 4 == 0 { -h.z } else { h.z });
    let pos: Vec<DVec3> = (0..8).map(corner).collect();
    // index bits: x = 1, y = 2, z = 4; quads wound counter-clockwise from outside
    let quads: [[u32; 4]; 6] = [[1, 3, 7, 5], [0, 4, 6, 2], [2, 6, 7, 3], [0, 1, 5, 4], [4, 5, 7, 6], [0, 2, 3, 1]];
    let tris: Vec<[u32; 3]> = quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect();
    let mesh = TriMesh::new(pos, tris, vec![0; 12]);
    let mut rng = Rng(77);
    for _ in 0..400 {
        let q = rng.vec(-20.0, 20.0);
        let d = q.abs() - h;
        let outside = d.max(DVec3::ZERO).length();
        let inside = d.x.max(d.y).max(d.z).min(0.0);
        let want = if d.x.max(d.y).max(d.z) > 0.0 { outside } else { inside };
        for mode in [SignMode::Face, SignMode::Pseudo] {
            let hit = mesh.closest(q, f64::INFINITY, mode).unwrap();
            assert!((hit.signed - want).abs() < 1e-9, "{mode:?} at {q:?}: {} vs {want}", hit.signed);
            assert!((hit.dist - want.abs()).abs() < 1e-9);
        }
    }
}

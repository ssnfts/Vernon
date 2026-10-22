//! 2D polygon helpers: areas, containment, distances and ear-clipping
//! triangulation of polygons with holes (hole bridging as in the public
//! ear-clipping literature; no third-party code).

use glam::DVec2;

/// Signed shoelace area; positive for counter-clockwise rings.
pub fn signed_area(p: &[DVec2]) -> f64 {
    let n = p.len();
    if n < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    for i in 0..n {
        let a = p[i];
        let b = p[(i + 1) % n];
        s += a.x * b.y - b.x * a.y;
    }
    s * 0.5
}

/// Even-odd ray-casting containment test.
pub fn point_in_polygon(pt: DVec2, poly: &[DVec2]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > pt.y) != (b.y > pt.y) && pt.x < (b.x - a.x) * (pt.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Even-odd containment over a set of rings (outers and holes together).
pub fn inside_rings(pt: DVec2, rings: &[Vec<DVec2>]) -> bool {
    rings.iter().filter(|r| point_in_polygon(pt, r)).count() % 2 == 1
}

pub fn dist_to_segment(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    let ab = b - a;
    let l2 = ab.length_squared();
    if l2 < 1e-300 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// Smallest distance from `p` to the boundary of any ring.
pub fn dist_to_rings(p: DVec2, rings: &[Vec<DVec2>]) -> f64 {
    let mut best = f64::INFINITY;
    for r in rings {
        let n = r.len();
        for i in 0..n {
            best = best.min(dist_to_segment(p, r[i], r[(i + 1) % n]));
        }
    }
    best
}

fn cross(o: DVec2, a: DVec2, b: DVec2) -> f64 {
    (a - o).perp_dot(b - o)
}

fn in_triangle_incl(p: DVec2, a: DVec2, b: DVec2, c: DVec2) -> bool {
    let e = 1e-12;
    let d1 = cross(a, b, p);
    let d2 = cross(b, c, p);
    let d3 = cross(c, a, p);
    let has_neg = d1 < -e || d2 < -e || d3 < -e;
    let has_pos = d1 > e || d2 > e || d3 > e;
    !(has_neg && has_pos)
}

/// Triangulate one outer ring (any orientation) with holes (any orientation).
/// Indices in the result refer to the concatenation `outer ++ holes[0] ++ ...`.
/// Triangles are counter-clockwise.
pub fn triangulate(outer: &[DVec2], holes: &[Vec<DVec2>]) -> Vec<[usize; 3]> {
    let mut pts: Vec<DVec2> = outer.to_vec();
    let mut hole_idx: Vec<Vec<usize>> = Vec::new();
    for h in holes {
        let s = pts.len();
        pts.extend_from_slice(h);
        hole_idx.push((s..pts.len()).collect());
    }
    if outer.len() < 3 {
        return Vec::new();
    }
    let mut poly: Vec<usize> = (0..outer.len()).collect();
    if signed_area(outer) < 0.0 {
        poly.reverse();
    }
    for h in &mut hole_idx {
        let hp: Vec<DVec2> = h.iter().map(|&i| pts[i]).collect();
        if signed_area(&hp) > 0.0 {
            h.reverse();
        }
    }
    // Rightmost hole first.
    let max_x = |h: &Vec<usize>| h.iter().map(|&i| pts[i].x).fold(f64::NEG_INFINITY, f64::max);
    hole_idx.retain(|h| h.len() >= 3);
    hole_idx.sort_by(|a, b| max_x(b).partial_cmp(&max_x(a)).unwrap_or(std::cmp::Ordering::Equal));
    for h in &hole_idx {
        bridge_hole(&mut poly, h, &pts);
    }
    ear_clip(&poly, &pts)
}

/// True when `m` lies inside the interior wedge at `q` of a counter-clockwise polygon.
fn locally_inside(prev: DVec2, q: DVec2, next: DVec2, m: DVec2) -> bool {
    if cross(prev, q, next) >= 0.0 {
        cross(prev, q, m) >= 0.0 && cross(q, next, m) >= 0.0
    } else {
        cross(prev, q, m) >= 0.0 || cross(q, next, m) >= 0.0
    }
}

fn bridge_hole(poly: &mut Vec<usize>, hole: &[usize], pts: &[DVec2]) {
    // Hole vertex with the largest x.
    let (mpos, &m) = hole
        .iter()
        .enumerate()
        .max_by(|a, b| pts[*a.1].x.partial_cmp(&pts[*b.1].x).unwrap_or(std::cmp::Ordering::Equal))
        .expect("hole not empty");
    let mp = pts[m];
    let len = poly.len();
    let mut best_x = f64::INFINITY;
    let mut best_edge = None;
    for i in 0..len {
        let a = pts[poly[i]];
        let b = pts[poly[(i + 1) % len]];
        if a.y == b.y {
            continue;
        }
        let (lo, hi) = if a.y < b.y { (a.y, b.y) } else { (b.y, a.y) };
        if mp.y < lo || mp.y > hi {
            continue;
        }
        let t = (mp.y - a.y) / (b.y - a.y);
        let x = a.x + t * (b.x - a.x);
        if x >= mp.x && x < best_x {
            best_x = x;
            best_edge = Some(i);
        }
    }
    let bridge_pos = match best_edge {
        None => {
            // Degenerate (hole outside the outer ring): connect to the nearest vertex.
            (0..len)
                .min_by(|&i, &j| {
                    (pts[poly[i]] - mp)
                        .length_squared()
                        .partial_cmp(&(pts[poly[j]] - mp).length_squared())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap_or(0)
        }
        Some(e) => {
            let e2 = (e + 1) % len;
            let ip = DVec2::new(best_x, mp.y);
            let p_pos = if pts[poly[e]].x > pts[poly[e2]].x { e } else { e2 };
            let pp = pts[poly[p_pos]];
            if (pp - ip).length_squared() < 1e-24 {
                p_pos
            } else {
                // Any reflex vertex inside triangle (M, I, P) blocks the view; take
                // the one with the smallest angle to the ray.
                let mut best = p_pos;
                let mut best_tan = {
                    let d = pp - mp;
                    d.y.abs() / d.x.max(1e-300)
                };
                let mut best_d2 = (pp - mp).length_squared();
                for k in 0..len {
                    if k == p_pos {
                        continue;
                    }
                    let q = pts[poly[k]];
                    if q.x < mp.x || (q.x == mp.x && q.y == mp.y) {
                        continue;
                    }
                    if !in_triangle_incl(q, mp, ip, pp) {
                        continue;
                    }
                    let prev = pts[poly[(k + len - 1) % len]];
                    let next = pts[poly[(k + 1) % len]];
                    if !locally_inside(prev, q, next, mp) {
                        continue;
                    }
                    let d = q - mp;
                    let tan = d.y.abs() / d.x.max(1e-300);
                    let d2 = d.length_squared();
                    if tan < best_tan || (tan == best_tan && d2 < best_d2) {
                        best = k;
                        best_tan = tan;
                        best_d2 = d2;
                    }
                }
                best
            }
        }
    };
    // A vertex that already carries a bridge appears several times; splice at
    // the occurrence whose interior wedge faces the hole.
    let target = pts[poly[bridge_pos]];
    let bridge_pos = (0..len)
        .filter(|&k| pts[poly[k]] == target)
        .find(|&k| locally_inside(pts[poly[(k + len - 1) % len]], target, pts[poly[(k + 1) % len]], mp))
        .unwrap_or(bridge_pos);
    let mut merged: Vec<usize> = Vec::with_capacity(poly.len() + hole.len() + 2);
    merged.extend_from_slice(&poly[..=bridge_pos]);
    for k in 0..hole.len() {
        merged.push(hole[(mpos + k) % hole.len()]);
    }
    merged.push(hole[mpos]);
    merged.push(poly[bridge_pos]);
    merged.extend_from_slice(&poly[bridge_pos + 1..]);
    *poly = merged;
}

fn ear_clip(poly: &[usize], pts: &[DVec2]) -> Vec<[usize; 3]> {
    let n = poly.len();
    let mut out = Vec::with_capacity(n.saturating_sub(2));
    if n < 3 {
        return out;
    }
    let mut prev: Vec<usize> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1) % n).collect();
    let mut alive = n;
    let p = |i: usize| pts[poly[i]];
    let scale = {
        let mut lo = DVec2::splat(f64::INFINITY);
        let mut hi = DVec2::splat(f64::NEG_INFINITY);
        for &i in poly {
            lo = lo.min(pts[i]);
            hi = hi.max(pts[i]);
        }
        (hi - lo).max_element().max(1e-12)
    };
    let tiny = 1e-13 * scale * scale;
    let mut cur = 0usize;
    let mut mode = 0;
    while alive > 3 {
        let mut found = None;
        let mut i = cur;
        for _ in 0..alive {
            let (a, c) = (prev[i], next[i]);
            let cr = cross(p(a), p(i), p(c));
            let convex = match mode {
                0 | 2 => cr > tiny,
                _ => cr.abs() <= tiny,
            };
            if convex {
                let mut ok = true;
                if mode < 2 {
                    let (pa, pb, pc) = (p(a), p(i), p(c));
                    let mut j = next[c];
                    while j != a {
                        let q = p(j);
                        if q != pa && q != pb && q != pc {
                            let reflex = cross(p(prev[j]), q, p(next[j])) <= tiny;
                            if reflex && in_triangle_incl(q, pa, pb, pc) {
                                ok = false;
                                break;
                            }
                        }
                        j = next[j];
                    }
                }
                if ok {
                    found = Some(i);
                    break;
                }
            }
            i = next[i];
        }
        match found {
            Some(i) => {
                let (a, c) = (prev[i], next[i]);
                out.push([poly[a], poly[i], poly[c]]);
                next[a] = c;
                prev[c] = a;
                alive -= 1;
                cur = c;
                mode = 0;
            }
            None => {
                mode += 1;
                if mode > 2 {
                    break;
                }
            }
        }
    }
    if alive == 3 {
        let a = cur;
        let (b, c) = (next[a], next[next[a]]);
        if cross(p(a), p(b), p(c)).abs() > tiny {
            out.push([poly[a], poly[b], poly[c]]);
        }
    }
    out
}

/// Result of [`triangulate_rings`].
pub struct RingTriangulation {
    /// Counter-clockwise triangles, indices into the flattened rings.
    pub tris: Vec<[usize; 3]>,
    /// Net area: outers minus holes.
    pub area: f64,
    /// Nesting depth of each ring (0 = outer).
    pub depth: Vec<usize>,
}

/// Classify rings by nesting (even depth = outer, odd = hole) and triangulate
/// every outer with its direct holes. Indices refer to the flattened rings.
pub fn triangulate_rings(rings: &[Vec<DVec2>]) -> RingTriangulation {
    let n = rings.len();
    let areas: Vec<f64> = rings.iter().map(|r| signed_area(r).abs()).collect();
    let mut parent: Vec<Option<usize>> = vec![None; n];
    let mut depth = vec![0usize; n];
    for i in 0..n {
        let probe = rings[i].first().copied().unwrap_or(DVec2::ZERO);
        let mut best: Option<usize> = None;
        for j in 0..n {
            if i != j && areas[j] > areas[i] && point_in_polygon(probe, &rings[j]) {
                depth[i] += 1;
                if best.map_or(true, |b| areas[j] < areas[b]) {
                    best = Some(j);
                }
            }
        }
        parent[i] = best;
    }
    let mut offsets = Vec::with_capacity(n);
    let mut acc = 0;
    for r in rings {
        offsets.push(acc);
        acc += r.len();
    }
    let mut tris = Vec::new();
    let mut area = 0.0;
    for i in 0..n {
        if depth[i] % 2 != 0 {
            area -= areas[i];
            continue;
        }
        area += areas[i];
        let holes: Vec<usize> = (0..n).filter(|&j| depth[j] % 2 == 1 && parent[j] == Some(i)).collect();
        let hole_rings: Vec<Vec<DVec2>> = holes.iter().map(|&j| rings[j].clone()).collect();
        let local = triangulate(&rings[i], &hole_rings);
        // Map local indices (outer ++ holes) back to flattened indices.
        let mut map: Vec<usize> = (0..rings[i].len()).map(|k| offsets[i] + k).collect();
        for &j in &holes {
            map.extend((0..rings[j].len()).map(|k| offsets[j] + k));
        }
        for t in local {
            tris.push([map[t[0]], map[t[1]], map[t[2]]]);
        }
    }
    RingTriangulation { tris, area, depth }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri_area(pts: &[DVec2], t: [usize; 3]) -> f64 {
        cross(pts[t[0]], pts[t[1]], pts[t[2]]) * 0.5
    }

    #[test]
    fn square_with_hole_area() {
        let outer = vec![DVec2::new(0.0, 0.0), DVec2::new(10.0, 0.0), DVec2::new(10.0, 10.0), DVec2::new(0.0, 10.0)];
        let hole = vec![DVec2::new(3.0, 3.0), DVec2::new(3.0, 7.0), DVec2::new(7.0, 7.0), DVec2::new(7.0, 3.0)];
        let mut pts = outer.clone();
        pts.extend(hole.iter().copied());
        let tris = triangulate(&outer, &[hole]);
        let a: f64 = tris.iter().map(|&t| tri_area(&pts, t)).sum();
        assert!((a - 84.0).abs() < 1e-9, "area {a}");
        assert!(tris.iter().all(|&t| tri_area(&pts, t) > 0.0));
    }

    #[test]
    fn concave_with_collinear_vertices() {
        let outer = vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(5.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(10.0, 10.0),
            DVec2::new(5.0, 3.0),
            DVec2::new(0.0, 10.0),
        ];
        let tris = triangulate(&outer, &[]);
        let a: f64 = tris.iter().map(|&t| tri_area(&outer, t)).sum();
        assert!((a - signed_area(&outer)).abs() < 1e-9);
        // every vertex is used
        for v in 0..outer.len() {
            assert!(tris.iter().any(|t| t.contains(&v)), "vertex {v} unused");
        }
    }

    #[test]
    fn square_with_many_circular_holes() {
        let outer = vec![DVec2::new(0.0, 0.0), DVec2::new(100.0, 0.0), DVec2::new(100.0, 100.0), DVec2::new(0.0, 100.0)];
        let mut rings = vec![outer];
        for cx in [10.0, 50.0, 90.0] {
            for cy in [10.0, 50.0, 90.0] {
                let r: Vec<DVec2> = (0..64)
                    .map(|i| {
                        let a = std::f64::consts::TAU * i as f64 / 64.0;
                        DVec2::new(cx + 4.0 * a.cos(), cy + 4.0 * a.sin())
                    })
                    .collect();
                rings.push(r);
            }
        }
        let flat: Vec<DVec2> = rings.iter().flatten().copied().collect();
        let t = triangulate_rings(&rings);
        let sum: f64 = t.tris.iter().map(|&x| tri_area(&flat, x)).sum();
        assert!((sum - t.area).abs() < 1e-6, "triangles {sum} vs net {}", t.area);
        assert!(t.tris.iter().all(|&x| tri_area(&flat, x) > 0.0));
    }

    #[test]
    fn nested_rings_classified() {
        let sq = |x0: f64, x1: f64| {
            vec![DVec2::new(x0, x0), DVec2::new(x1, x0), DVec2::new(x1, x1), DVec2::new(x0, x1)]
        };
        let rings = vec![sq(0.0, 10.0), sq(2.0, 8.0), sq(4.0, 6.0)];
        let r = triangulate_rings(&rings);
        assert_eq!(r.depth, vec![0, 1, 2]);
        assert!((r.area - (100.0 - 36.0 + 4.0)).abs() < 1e-9);
    }
}

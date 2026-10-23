//! A small bounding-volume hierarchy with a two-sided any-hit ray query,
//! used for shadow and sky-visibility rays.

use glam::DVec3;

#[derive(Clone, Copy, Debug)]
struct Node {
    min: DVec3,
    max: DVec3,
    /// Leaf: first triangle slot. Internal: index of the left child.
    start: u32,
    /// Leaf: triangle count (> 0). Internal: 0.
    count: u32,
    /// Internal: index of the right child.
    right: u32,
}

pub struct Bvh {
    tris: Vec<[DVec3; 3]>,
    nodes: Vec<Node>,
}

const LEAF: usize = 4;

impl Bvh {
    pub fn build(tris: Vec<[DVec3; 3]>) -> Self {
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let mut nodes = Vec::new();
        if !tris.is_empty() {
            let cent: Vec<DVec3> = tris.iter().map(|t| (t[0] + t[1] + t[2]) / 3.0).collect();
            build_rec(&tris, &cent, &mut order, 0, tris.len(), &mut nodes);
        }
        let tris = order.iter().map(|&i| tris[i as usize]).collect();
        Bvh { tris, nodes }
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// True if the segment `o + t d`, `t` in `(tmin, tmax)`, hits any
    /// triangle (both faces count).
    pub fn any_hit(&self, o: DVec3, d: DVec3, tmin: f64, tmax: f64) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        let inv = DVec3::new(
            if d.x != 0.0 { 1.0 / d.x } else { f64::INFINITY },
            if d.y != 0.0 { 1.0 / d.y } else { f64::INFINITY },
            if d.z != 0.0 { 1.0 / d.z } else { f64::INFINITY },
        );
        let mut stack = [0u32; 64];
        let mut sp = 1usize;
        while sp > 0 {
            sp -= 1;
            let n = &self.nodes[stack[sp] as usize];
            if !slab(n, o, inv, tmin, tmax) {
                continue;
            }
            if n.count > 0 {
                for t in &self.tris[n.start as usize..(n.start + n.count) as usize] {
                    if tri_hit(t, o, d, tmin, tmax) {
                        return true;
                    }
                }
            } else if sp + 2 <= stack.len() {
                stack[sp] = n.start;
                stack[sp + 1] = n.right;
                sp += 2;
            }
        }
        false
    }
}

fn build_rec(
    tris: &[[DVec3; 3]],
    cent: &[DVec3],
    order: &mut [u32],
    lo: usize,
    hi: usize,
    nodes: &mut Vec<Node>,
) -> u32 {
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    let mut cmin = min;
    let mut cmax = max;
    for &i in &order[lo..hi] {
        for v in &tris[i as usize] {
            min = min.min(*v);
            max = max.max(*v);
        }
        cmin = cmin.min(cent[i as usize]);
        cmax = cmax.max(cent[i as usize]);
    }
    let idx = nodes.len() as u32;
    nodes.push(Node {
        min,
        max,
        start: lo as u32,
        count: (hi - lo) as u32,
        right: 0,
    });
    if hi - lo <= LEAF {
        return idx;
    }
    let ext = cmax - cmin;
    let axis = if ext.x >= ext.y && ext.x >= ext.z {
        0
    } else if ext.y >= ext.z {
        1
    } else {
        2
    };
    if ext[axis] <= 0.0 {
        return idx;
    }
    order[lo..hi].sort_by(|&a, &b| {
        cent[a as usize][axis]
            .total_cmp(&cent[b as usize][axis])
            .then(a.cmp(&b))
    });
    let mid = (lo + hi) / 2;
    let left = build_rec(tris, cent, order, lo, mid, nodes);
    let right = build_rec(tris, cent, order, mid, hi, nodes);
    let n = &mut nodes[idx as usize];
    n.start = left;
    n.count = 0;
    n.right = right;
    idx
}

fn slab(n: &Node, o: DVec3, inv: DVec3, tmin: f64, tmax: f64) -> bool {
    let mut t0 = tmin;
    let mut t1 = tmax;
    for a in 0..3 {
        let (lo, hi, oa, ia) = (n.min[a], n.max[a], o[a], inv[a]);
        if ia.is_infinite() {
            if oa < lo || oa > hi {
                return false;
            }
            continue;
        }
        let (mut ta, mut tb) = ((lo - oa) * ia, (hi - oa) * ia);
        if ta > tb {
            std::mem::swap(&mut ta, &mut tb);
        }
        t0 = t0.max(ta);
        t1 = t1.min(tb);
        if t0 > t1 {
            return false;
        }
    }
    true
}

/// Moller-Trumbore, two-sided.
fn tri_hit(t: &[DVec3; 3], o: DVec3, d: DVec3, tmin: f64, tmax: f64) -> bool {
    let e1 = t[1] - t[0];
    let e2 = t[2] - t[0];
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-18 {
        return false;
    }
    let inv = 1.0 / det;
    let s = o - t[0];
    let u = s.dot(p) * inv;
    if !(-1e-12..=1.0 + 1e-12).contains(&u) {
        return false;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < -1e-12 || u + v > 1.0 + 1e-12 {
        return false;
    }
    let dist = e2.dot(q) * inv;
    dist > tmin && dist < tmax
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_and_misses() {
        let big = [
            DVec3::new(-100.0, -100.0, 5.0),
            DVec3::new(100.0, -100.0, 5.0),
            DVec3::new(0.0, 100.0, 5.0),
        ];
        let b = Bvh::build(vec![big]);
        assert!(b.any_hit(DVec3::ZERO, DVec3::Z, 0.0, f64::INFINITY));
        assert!(!b.any_hit(DVec3::ZERO, -DVec3::Z, 0.0, f64::INFINITY));
        assert!(!b.any_hit(DVec3::ZERO, DVec3::Z, 0.0, 4.0));
        assert!(!b.any_hit(DVec3::new(500.0, 0.0, 0.0), DVec3::Z, 0.0, f64::INFINITY));
    }

    #[test]
    fn many_triangles_match_brute_force() {
        let mut tris = Vec::new();
        for i in 0..200 {
            let x = (i % 20) as f64 * 3.0;
            let y = (i / 20) as f64 * 3.0;
            tris.push([
                DVec3::new(x, y, 2.0),
                DVec3::new(x + 2.0, y, 2.0),
                DVec3::new(x, y + 2.0, 2.0),
            ]);
        }
        let b = Bvh::build(tris.clone());
        for k in 0..50 {
            let o = DVec3::new(k as f64 * 1.3, k as f64 * 0.7, 0.0);
            let brute = tris
                .iter()
                .any(|t| tri_hit(t, o, DVec3::Z, 0.0, f64::INFINITY));
            assert_eq!(b.any_hit(o, DVec3::Z, 0.0, f64::INFINITY), brute);
        }
    }
}

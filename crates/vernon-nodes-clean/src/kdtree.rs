//! Exact 3-D k-d tree: nearest, k nearest and radius queries. Built with
//! median splits on the widest axis; queries are allocation-free apart from
//! the caller's output buffers, so they parallelise cleanly with rayon.

use glam::DVec3;

const LEAF: usize = 8;

#[derive(Clone, Copy)]
struct Item {
    p: [f64; 3],
    i: u32,
}

#[derive(Clone, Copy)]
struct Node {
    lo: u32,
    hi: u32,
    split: f64,
    axis: u8, // 3 = leaf
    left: u32,
    right: u32,
}

pub struct KdTree {
    items: Vec<Item>,
    nodes: Vec<Node>,
}

fn d2(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    let (x, y, z) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    x * x + y * y + z * z
}

fn build(items: &mut [Item], lo: usize, nodes: &mut Vec<Node>) -> u32 {
    let n = items.len();
    let id = nodes.len() as u32;
    if n <= LEAF {
        nodes.push(Node {
            lo: lo as u32,
            hi: (lo + n) as u32,
            split: 0.0,
            axis: 3,
            left: 0,
            right: 0,
        });
        return id;
    }
    let mut mn = [f64::INFINITY; 3];
    let mut mx = [f64::NEG_INFINITY; 3];
    for it in items.iter() {
        for a in 0..3 {
            mn[a] = mn[a].min(it.p[a]);
            mx[a] = mx[a].max(it.p[a]);
        }
    }
    let mut axis = 0;
    for a in 1..3 {
        if mx[a] - mn[a] > mx[axis] - mn[axis] {
            axis = a;
        }
    }
    let mid = n / 2;
    items.select_nth_unstable_by(mid, |a, b| a.p[axis].total_cmp(&b.p[axis]));
    let split = items[mid].p[axis];
    nodes.push(Node {
        lo: 0,
        hi: 0,
        split,
        axis: axis as u8,
        left: 0,
        right: 0,
    });
    let (l, r) = items.split_at_mut(mid);
    let left = build(l, lo, nodes);
    let right = build(r, lo + mid, nodes);
    nodes[id as usize].left = left;
    nodes[id as usize].right = right;
    id
}

impl KdTree {
    pub fn new(points: &[DVec3]) -> Self {
        let mut items: Vec<Item> = points
            .iter()
            .enumerate()
            .map(|(i, p)| Item {
                p: [p.x, p.y, p.z],
                i: i as u32,
            })
            .collect();
        let mut nodes = Vec::with_capacity(items.len() / LEAF * 2 + 1);
        if !items.is_empty() {
            build(&mut items, 0, &mut nodes);
        }
        Self { items, nodes }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Nearest point: (original index, squared distance).
    pub fn nearest(&self, q: DVec3) -> Option<(u32, f64)> {
        self.nearest_within(q, f64::INFINITY)
    }

    /// Nearest point with squared distance strictly below `max_d2`.
    pub fn nearest_within(&self, q: DVec3, max_d2: f64) -> Option<(u32, f64)> {
        if self.nodes.is_empty() {
            return None;
        }
        let qa = [q.x, q.y, q.z];
        let mut best = (u32::MAX, max_d2);
        self.nn_rec(0, &qa, &mut best);
        if best.0 == u32::MAX {
            None
        } else {
            Some(best)
        }
    }

    fn nn_rec(&self, node: u32, q: &[f64; 3], best: &mut (u32, f64)) {
        let n = self.nodes[node as usize];
        if n.axis == 3 {
            for it in &self.items[n.lo as usize..n.hi as usize] {
                let dd = d2(&it.p, q);
                if dd < best.1 {
                    *best = (it.i, dd);
                }
            }
            return;
        }
        let diff = q[n.axis as usize] - n.split;
        let (near, far) = if diff < 0.0 { (n.left, n.right) } else { (n.right, n.left) };
        self.nn_rec(near, q, best);
        if diff * diff < best.1 {
            self.nn_rec(far, q, best);
        }
    }

    /// The `k` nearest points as (squared distance, original index), sorted
    /// ascending. `out` is cleared first.
    pub fn knn(&self, q: DVec3, k: usize, out: &mut Vec<(f64, u32)>) {
        out.clear();
        if self.nodes.is_empty() || k == 0 {
            return;
        }
        let qa = [q.x, q.y, q.z];
        self.knn_rec(0, &qa, k, out);
    }

    fn knn_rec(&self, node: u32, q: &[f64; 3], k: usize, out: &mut Vec<(f64, u32)>) {
        let n = self.nodes[node as usize];
        if n.axis == 3 {
            for it in &self.items[n.lo as usize..n.hi as usize] {
                let dd = d2(&it.p, q);
                if out.len() < k || dd < out[k - 1].0 {
                    let pos = out.partition_point(|e| e.0 <= dd);
                    out.insert(pos, (dd, it.i));
                    if out.len() > k {
                        out.pop();
                    }
                }
            }
            return;
        }
        let diff = q[n.axis as usize] - n.split;
        let (near, far) = if diff < 0.0 { (n.left, n.right) } else { (n.right, n.left) };
        self.knn_rec(near, q, k, out);
        if out.len() < k || diff * diff < out[k - 1].0 {
            self.knn_rec(far, q, k, out);
        }
    }

    /// All points within `r` (inclusive) of `q`; original indices appended to
    /// `out` (cleared first), unordered.
    pub fn radius(&self, q: DVec3, r: f64, out: &mut Vec<u32>) {
        out.clear();
        if self.nodes.is_empty() {
            return;
        }
        let qa = [q.x, q.y, q.z];
        self.radius_rec(0, &qa, r * r, out);
    }

    fn radius_rec(&self, node: u32, q: &[f64; 3], r2: f64, out: &mut Vec<u32>) {
        let n = self.nodes[node as usize];
        if n.axis == 3 {
            for it in &self.items[n.lo as usize..n.hi as usize] {
                if d2(&it.p, q) <= r2 {
                    out.push(it.i);
                }
            }
            return;
        }
        let diff = q[n.axis as usize] - n.split;
        let (near, far) = if diff < 0.0 { (n.left, n.right) } else { (n.right, n.left) };
        self.radius_rec(near, q, r2, out);
        if diff * diff <= r2 {
            self.radius_rec(far, q, r2, out);
        }
    }

    /// Number of points within `r` of `q`, stopping early at `cap`.
    pub fn radius_count(&self, q: DVec3, r: f64, cap: usize) -> usize {
        if self.nodes.is_empty() {
            return 0;
        }
        let qa = [q.x, q.y, q.z];
        let mut c = 0;
        self.count_rec(0, &qa, r * r, cap, &mut c);
        c
    }

    fn count_rec(&self, node: u32, q: &[f64; 3], r2: f64, cap: usize, c: &mut usize) {
        if *c >= cap {
            return;
        }
        let n = self.nodes[node as usize];
        if n.axis == 3 {
            for it in &self.items[n.lo as usize..n.hi as usize] {
                if d2(&it.p, q) <= r2 {
                    *c += 1;
                }
            }
            return;
        }
        let diff = q[n.axis as usize] - n.split;
        let (near, far) = if diff < 0.0 { (n.left, n.right) } else { (n.right, n.left) };
        self.count_rec(near, q, r2, cap, c);
        if diff * diff <= r2 {
            self.count_rec(far, q, r2, cap, c);
        }
    }
}

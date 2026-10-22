//! Triangle mesh acceleration structure: AABB tree with nearest-first
//! closest-point queries and angle-weighted pseudo-normals for a robust
//! inside/outside sign (Baerentzen and Aanaes 2005), plus topology checks.

use std::collections::HashMap;

use glam::DVec3;
use vernon_core::Geometry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Face,
    /// 0 = a-b, 1 = b-c, 2 = c-a
    Edge(u8),
    /// 0 = a, 1 = b, 2 = c
    Vertex(u8),
}

/// Closest point on triangle (a, b, c) to `p` by Voronoi-region
/// classification (Ericson, Real-Time Collision Detection, 5.1.5).
pub fn closest_point_triangle(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> (DVec3, Feature) {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, Feature::Vertex(0));
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return (b, Feature::Vertex(1));
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (a + ab * v, Feature::Edge(0));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return (c, Feature::Vertex(2));
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (a + ac * w, Feature::Edge(2));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (b + (c - b) * w, Feature::Edge(1));
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (a + ab * v + ac * w, Feature::Face)
}

#[derive(Debug, Clone, Copy)]
pub struct Hit {
    pub tri: u32,
    pub point: DVec3,
    /// Unsigned distance.
    pub dist: f64,
    /// Signed distance (positive on the outward-normal side).
    pub signed: f64,
    pub feature: Feature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignMode {
    /// Report unsigned distance only.
    None,
    /// Sign from the face normal of the closest triangle (naive).
    Face,
    /// Sign from the angle-weighted pseudo-normal of the closest feature.
    Pseudo,
}

#[derive(Debug, Clone, Default)]
pub struct Topology {
    pub open_edges: usize,
    pub nonmanifold_edges: usize,
    pub inconsistent_edges: usize,
    pub degenerate_tris: usize,
    pub signed_volume: f64,
    pub components: usize,
}

impl Topology {
    /// Closed, 2-manifold and consistently oriented.
    pub fn is_closed(&self) -> bool {
        self.open_edges == 0 && self.nonmanifold_edges == 0 && self.inconsistent_edges == 0
    }
}

#[derive(Debug, Clone, Copy)]
struct BvhNode {
    min: [f64; 3],
    max: [f64; 3],
    /// inner: left child; leaf: first slot in `order`
    a: u32,
    /// inner: right child; leaf: triangle count
    b: u32,
    leaf: bool,
}

pub struct TriMesh {
    pub pos: Vec<DVec3>,
    pub tris: Vec<[u32; 3]>,
    pub src_prim: Vec<u32>,
    pub tri_n: Vec<DVec3>,
    vnorm: Vec<DVec3>,
    weld: Vec<u32>,
    edge_n: Vec<[DVec3; 3]>,
    nodes: Vec<BvhNode>,
    order: Vec<u32>,
    pub topo: Topology,
}

fn box_d2(n: &BvhNode, p: DVec3) -> f64 {
    let mut d = 0.0;
    for a in 0..3 {
        let x = p[a];
        let e = (n.min[a] - x).max(0.0).max(x - n.max[a]);
        d += e * e;
    }
    d
}

fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        parent[x as usize] = parent[parent[x as usize] as usize];
        x = parent[x as usize];
    }
    x
}

impl TriMesh {
    /// Triangulate the closed primitives of `g` (fan) and index them.
    pub fn from_geometry(g: &Geometry) -> Option<TriMesh> {
        let t = g.triangles();
        if t.is_empty() {
            return None;
        }
        let tris = t.iter().map(|x| x.1).collect();
        let src = t.iter().map(|x| x.0 as u32).collect();
        Some(Self::new(g.points.clone(), tris, src))
    }

    pub fn new(pos: Vec<DVec3>, tris: Vec<[u32; 3]>, src_prim: Vec<u32>) -> TriMesh {
        // Weld coincident points for topology (exact bit equality).
        let mut weld = vec![0u32; pos.len()];
        let mut map: HashMap<[u64; 3], u32> = HashMap::with_capacity(pos.len());
        for (i, p) in pos.iter().enumerate() {
            let key = [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()];
            let next = map.len() as u32;
            weld[i] = *map.entry(key).or_insert(next);
        }
        let nw = map.len();
        drop(map);

        let nt = tris.len();
        let mut tri_n = vec![DVec3::ZERO; nt];
        let mut degenerate = vec![false; nt];
        let mut topo = Topology::default();
        let mut vnorm = vec![DVec3::ZERO; nw];
        let mut edges: HashMap<(u32, u32), (DVec3, u32, u32)> = HashMap::with_capacity(nt * 2);
        let mut vol = 0.0;
        for (ti, t) in tris.iter().enumerate() {
            let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
            let cr = (b - a).cross(c - a);
            let wv = [weld[t[0] as usize], weld[t[1] as usize], weld[t[2] as usize]];
            if cr.length_squared() < 1e-30 || wv[0] == wv[1] || wv[1] == wv[2] || wv[0] == wv[2] {
                degenerate[ti] = true;
                topo.degenerate_tris += 1;
                continue;
            }
            let n = cr.normalize();
            tri_n[ti] = n;
            vol += a.dot(b.cross(c)) / 6.0;
            let pts = [a, b, c];
            for k in 0..3 {
                let e1 = (pts[(k + 1) % 3] - pts[k]).normalize_or_zero();
                let e2 = (pts[(k + 2) % 3] - pts[k]).normalize_or_zero();
                let ang = e1.dot(e2).clamp(-1.0, 1.0).acos();
                vnorm[wv[k] as usize] += n * ang;
                let (u, v) = (wv[k], wv[(k + 1) % 3]);
                let (key, fwd) = if u < v { ((u, v), 1) } else { ((v, u), 0) };
                let e = edges.entry(key).or_insert((DVec3::ZERO, 0, 0));
                e.0 += n;
                e.1 += 1;
                e.2 += fwd;
            }
        }
        topo.signed_volume = vol;
        for (_, (_, cnt, fwd)) in edges.iter() {
            match *cnt {
                1 => topo.open_edges += 1,
                2 => {
                    if *fwd != 1 {
                        topo.inconsistent_edges += 1;
                    }
                }
                _ => topo.nonmanifold_edges += 1,
            }
        }
        let mut edge_n = vec![[DVec3::ZERO; 3]; nt];
        for (ti, t) in tris.iter().enumerate() {
            if degenerate[ti] {
                continue;
            }
            let wv = [weld[t[0] as usize], weld[t[1] as usize], weld[t[2] as usize]];
            for k in 0..3 {
                let (u, v) = (wv[k], wv[(k + 1) % 3]);
                let key = if u < v { (u, v) } else { (v, u) };
                if let Some(e) = edges.get(&key) {
                    edge_n[ti][k] = e.0.normalize_or_zero();
                }
            }
        }
        for n in &mut vnorm {
            *n = n.normalize_or_zero();
        }
        // connected components over welded vertices
        let mut parent: Vec<u32> = (0..nw as u32).collect();
        let mut used = vec![false; nw];
        for (ti, t) in tris.iter().enumerate() {
            if degenerate[ti] {
                continue;
            }
            let w0 = weld[t[0] as usize];
            for k in 0..3 {
                used[weld[t[k] as usize] as usize] = true;
                let (ra, rb) = (find(&mut parent, w0), find(&mut parent, weld[t[k] as usize]));
                if ra != rb {
                    parent[ra as usize] = rb;
                }
            }
        }
        let mut roots = std::collections::HashSet::new();
        for v in 0..nw as u32 {
            if used[v as usize] {
                roots.insert(find(&mut parent, v));
            }
        }
        topo.components = roots.len();

        // BVH over non-degenerate triangles
        let mut order: Vec<u32> = (0..nt as u32).filter(|&i| !degenerate[i as usize]).collect();
        let mut tmin = vec![[0.0; 3]; nt];
        let mut tmax = vec![[0.0; 3]; nt];
        let mut cen = vec![[0.0; 3]; nt];
        for &ti in &order {
            let t = tris[ti as usize];
            let pts = [pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]];
            let mn = pts[0].min(pts[1]).min(pts[2]);
            let mx = pts[0].max(pts[1]).max(pts[2]);
            tmin[ti as usize] = mn.to_array();
            tmax[ti as usize] = mx.to_array();
            cen[ti as usize] = ((mn + mx) * 0.5).to_array();
        }
        let mut nodes = Vec::with_capacity(order.len() / 2 + 1);
        if !order.is_empty() {
            let len = order.len();
            build_bvh(&mut order, 0, len, &tmin, &tmax, &cen, &mut nodes);
        }
        TriMesh {
            pos,
            tris,
            src_prim,
            tri_n,
            vnorm,
            weld,
            edge_n,
            nodes,
            order,
            topo,
        }
    }

    pub fn tri_points(&self, t: u32) -> [DVec3; 3] {
        let v = self.tris[t as usize];
        [self.pos[v[0] as usize], self.pos[v[1] as usize], self.pos[v[2] as usize]]
    }

    pub fn tri_area(&self, t: u32) -> f64 {
        let [a, b, c] = self.tri_points(t);
        (b - a).cross(c - a).length() * 0.5
    }

    /// Closest point on the mesh to `p`, restricted to squared distance
    /// below `max_d2` (use `f64::INFINITY` for no cap).
    pub fn closest(&self, p: DVec3, max_d2: f64, mode: SignMode) -> Option<Hit> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best_d2 = max_d2;
        let mut best: Option<(u32, DVec3, Feature)> = None;
        let mut stack = [0u32; 128];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let node = &self.nodes[stack[sp] as usize];
            if box_d2(node, p) >= best_d2 {
                continue;
            }
            if node.leaf {
                for k in node.a..node.a + node.b {
                    let t = self.order[k as usize];
                    let [a, b, c] = self.tri_points(t);
                    let (cp, f) = closest_point_triangle(p, a, b, c);
                    let dd = (p - cp).length_squared();
                    if dd < best_d2 {
                        best_d2 = dd;
                        best = Some((t, cp, f));
                    }
                }
            } else {
                let (l, r) = (node.a, node.b);
                let dl = box_d2(&self.nodes[l as usize], p);
                let dr = box_d2(&self.nodes[r as usize], p);
                let (near, far, dn, df) = if dl <= dr { (l, r, dl, dr) } else { (r, l, dr, dl) };
                if df < best_d2 {
                    stack[sp] = far;
                    sp += 1;
                }
                if dn < best_d2 {
                    stack[sp] = near;
                    sp += 1;
                }
            }
        }
        let (tri, point, feature) = best?;
        let dist = best_d2.sqrt();
        let signed = match mode {
            SignMode::None => dist,
            _ => {
                let fnorm = self.tri_n[tri as usize];
                let n = if mode == SignMode::Face {
                    fnorm
                } else {
                    let n = match feature {
                        Feature::Face => fnorm,
                        Feature::Edge(e) => self.edge_n[tri as usize][e as usize],
                        Feature::Vertex(v) => {
                            let vi = self.tris[tri as usize][v as usize];
                            self.vnorm[self.weld[vi as usize] as usize]
                        }
                    };
                    if n.length_squared() < 1e-24 {
                        fnorm
                    } else {
                        n
                    }
                };
                if n.dot(p - point) >= 0.0 {
                    dist
                } else {
                    -dist
                }
            }
        };
        Some(Hit {
            tri,
            point,
            dist,
            signed,
            feature,
        })
    }

    /// Unsigned distance to the mesh.
    pub fn distance(&self, p: DVec3) -> f64 {
        self.closest(p, f64::INFINITY, SignMode::None).map_or(f64::INFINITY, |h| h.dist)
    }
}

fn build_bvh(
    order: &mut [u32],
    lo: usize,
    hi: usize,
    tmin: &[[f64; 3]],
    tmax: &[[f64; 3]],
    cen: &[[f64; 3]],
    nodes: &mut Vec<BvhNode>,
) -> u32 {
    let id = nodes.len() as u32;
    let mut mn = [f64::INFINITY; 3];
    let mut mx = [f64::NEG_INFINITY; 3];
    let mut cmn = [f64::INFINITY; 3];
    let mut cmx = [f64::NEG_INFINITY; 3];
    for &t in &order[lo..hi] {
        for a in 0..3 {
            mn[a] = mn[a].min(tmin[t as usize][a]);
            mx[a] = mx[a].max(tmax[t as usize][a]);
            cmn[a] = cmn[a].min(cen[t as usize][a]);
            cmx[a] = cmx[a].max(cen[t as usize][a]);
        }
    }
    let n = hi - lo;
    if n <= 4 {
        nodes.push(BvhNode {
            min: mn,
            max: mx,
            a: lo as u32,
            b: n as u32,
            leaf: true,
        });
        return id;
    }
    let mut axis = 0;
    for a in 1..3 {
        if cmx[a] - cmn[a] > cmx[axis] - cmn[axis] {
            axis = a;
        }
    }
    let mid = lo + n / 2;
    order[lo..hi].select_nth_unstable_by(n / 2, |&x, &y| cen[x as usize][axis].total_cmp(&cen[y as usize][axis]));
    nodes.push(BvhNode {
        min: mn,
        max: mx,
        a: 0,
        b: 0,
        leaf: false,
    });
    let l = build_bvh(order, lo, mid, tmin, tmax, cen, nodes);
    let r = build_bvh(order, mid, hi, tmin, tmax, cen, nodes);
    nodes[id as usize].a = l;
    nodes[id as usize].b = r;
    id
}

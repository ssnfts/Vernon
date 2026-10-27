//! Subdivision cage to NURBS patches (`surface.subd_to_nurbs`),
//! the patch-versus-limit deviation check (`surface.subd_deviation`) and the
//! measured continuity conversion (`surface.mesh_to_g2_surface`).
//!
//! Regular quads (all four corners regular, interior or on a straight crease /
//! boundary, or a pinned mesh corner) are exactly bicubic B-spline patches of
//! the Catmull-Clark limit surface. Rectangular runs of them are packed into
//! one larger clamped patch. Quads next to an extraordinary vertex get a
//! bicubic Bezier patch fitted to limit samples, sharing fitted edge curves
//! with their neighbours so the network stays watertight (G0).

use std::collections::{HashMap, HashSet};

use glam::DVec3;
use vernon_core::{AttribArray, Geometry, Metric, Report, Severity};
use vernon_graph::{
    CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec,
};

use crate::bvh::TriBvh;
use crate::continuity::{measure, SeamRef};
use crate::deviation::dev_stats;
use crate::linalg::solve_dense;
use crate::nurbs::{basis_funs, find_span, NurbsSurface, PatchSet};
use crate::patchops::find_seams;
use crate::subd::{Adj, Subd, VKind};
use crate::topology::edge_key;
use crate::util::{
    check_finite, json_err, json_input, max_abs_coord, node_info, positive, SPEC_MESH,
};

/// Outcome of a conversion.
#[derive(Debug, Clone)]
pub struct Conversion {
    pub patches: Vec<NurbsSurface>,
    pub cage_faces: usize,
    pub regular_faces: usize,
    pub irregular_faces: usize,
    pub extraordinary_vertices: usize,
    /// Largest and RMS distance from patches to the limit surface (sampled).
    pub max_dev: f64,
    pub rms_dev: f64,
    pub samples: usize,
    /// True when the cage was not all quads and was refined once first.
    pub refined_first: bool,
}

struct PatchPlan {
    /// Cage faces covered, as (face, column a, row b) inside the rectangle.
    faces: Vec<(usize, usize, usize)>,
    w: usize,
    h: usize,
    regular: bool,
}

fn bezier_basis(t: f64) -> [f64; 4] {
    let s = 1.0 - t;
    [s * s * s, 3.0 * t * s * s, 3.0 * t * t * s, t * t * t]
}

fn uniform_basis(t: f64) -> [f64; 4] {
    [
        (1.0 - t).powi(3) / 6.0,
        (3.0 * t.powi(3) - 6.0 * t * t + 4.0) / 6.0,
        (-3.0 * t.powi(3) + 3.0 * t * t + 3.0 * t + 1.0) / 6.0,
        t.powi(3) / 6.0,
    ]
}

fn clamped_knots(n: usize) -> Vec<f64> {
    // n = spans + 3 control points, cubic, knots normalised to [0, 1]
    let spans = n - 3;
    let mut k = vec![0.0; 4];
    for i in 1..spans {
        k.push(i as f64 / spans as f64);
    }
    k.extend([1.0; 4]);
    k
}

/// Exact clamped cubic B-spline equivalent of the uniform B-spline patch with
/// control net `ctrl[i][j]` (i along u, `ctrl.len() = w + 3`, `ctrl[0].len() = h + 3`).
fn uniform_to_clamped(ctrl: &[Vec<DVec3>]) -> NurbsSurface {
    let (nu, nv) = (ctrl.len(), ctrl[0].len());
    let (w, h) = (nu - 3, nv - 3);
    let (ku, kv) = (clamped_knots(nu), clamped_knots(nv));
    let greville = |k: &[f64], n: usize| -> Vec<f64> {
        (0..n)
            .map(|i| (k[i + 1] + k[i + 2] + k[i + 3]) / 3.0)
            .collect()
    };
    let (gu, gv) = (greville(&ku, nu), greville(&kv, nv));
    let eval = |u: f64, v: f64| -> DVec3 {
        let (us, vs) = (u * w as f64, v * h as f64);
        let (i0, j0) = (
            (us.floor() as usize).min(w - 1),
            (vs.floor() as usize).min(h - 1),
        );
        let (bu, bv) = (uniform_basis(us - i0 as f64), uniform_basis(vs - j0 as f64));
        let mut s = DVec3::ZERO;
        for a in 0..4 {
            for b in 0..4 {
                s += ctrl[i0 + a][j0 + b] * (bu[a] * bv[b]);
            }
        }
        s
    };
    let colloc = |k: &[f64], g: &[f64], n: usize| -> Vec<f64> {
        let mut m = vec![0.0; n * n];
        for (r, &t) in g.iter().enumerate() {
            let sp = find_span(n, 3, t, k);
            let b = basis_funs(sp, t, 3, k);
            for (a, ba) in b.iter().enumerate() {
                m[r * n + sp - 3 + a] = *ba;
            }
        }
        m
    };
    let (cu, cv) = (colloc(&ku, &gu, nu), colloc(&kv, &gv, nv));
    let mut samples = vec![vec![DVec3::ZERO; nv]; nu];
    for i in 0..nu {
        for j in 0..nv {
            samples[i][j] = eval(gu[i], gv[j]);
        }
    }
    // solve along u for each j, then along v for each i
    let mut mid = vec![vec![DVec3::ZERO; nv]; nu];
    for j in 0..nv {
        for c in 0..3 {
            let rhs: Vec<f64> = (0..nu).map(|i| samples[i][j][c]).collect();
            let sol = solve_dense(&cu, nu, &rhs).expect("collocation matrix is nonsingular");
            for i in 0..nu {
                mid[i][j][c] = sol[i];
            }
        }
    }
    let mut cps = vec![DVec3::ZERO; nu * nv];
    for i in 0..nu {
        for c in 0..3 {
            let rhs: Vec<f64> = (0..nv).map(|j| mid[i][j][c]).collect();
            let sol = solve_dense(&cv, nv, &rhs).expect("collocation matrix is nonsingular");
            for j in 0..nv {
                cps[i * nv + j][c] = sol[j];
            }
        }
    }
    NurbsSurface::new(3, 3, ku, kv, nu, nv, cps)
}

struct Walker<'a> {
    sub: &'a Subd,
    adj: &'a Adj,
    kinds: Vec<VKind>,
}

impl<'a> Walker<'a> {
    fn new(sub: &'a Subd, adj: &'a Adj) -> Self {
        let kinds = (0..sub.points.len() as u32)
            .map(|v| sub.vertex_kind(adj, v))
            .collect();
        Walker { sub, adj, kinds }
    }

    fn regular_vertex(&self, v: u32) -> bool {
        let (nf, ne) = (
            self.adj.vert_faces[v as usize].len(),
            self.adj.vert_edges[v as usize].len(),
        );
        match self.kinds[v as usize] {
            VKind::Smooth(4) => {
                nf == 4
                    && ne == 4
                    && self.adj.vert_edges[v as usize]
                        .iter()
                        .all(|&e| !self.sub.is_sharp(self.adj, e as usize))
            }
            VKind::Crease(..) => nf == 2 && ne == 3,
            VKind::Fixed => nf == 1 && ne == 2,
            _ => false,
        }
    }

    fn regular_face(&self, f: usize) -> bool {
        self.sub.faces[f].len() == 4 && self.sub.faces[f].iter().all(|&v| self.regular_vertex(v))
    }

    fn next_around(&self, cur: u32, from: u32, via: Option<usize>) -> Option<(u32, usize)> {
        for &f in &self.adj.vert_faces[cur as usize] {
            let f = f as usize;
            if Some(f) == via {
                continue;
            }
            let ring = &self.sub.faces[f];
            let pos = ring.iter().position(|&x| x == cur)?;
            let (p, n) = (ring[(pos + 3) % 4], ring[(pos + 1) % 4]);
            if p == from {
                return Some((n, f));
            }
            if n == from {
                return Some((p, f));
            }
        }
        None
    }

    /// The vertex that continues the straight line `prev -> cur` (None at a
    /// boundary: the caller extrapolates a phantom point).
    fn ext(&self, prev: u32, cur: u32) -> Option<u32> {
        match self.kinds[cur as usize] {
            VKind::Smooth(4) => {
                let (x, f1) = self.next_around(cur, prev, None)?;
                let (y, _) = self.next_around(cur, x, Some(f1))?;
                Some(y)
            }
            VKind::Crease(a, b) => {
                if prev == a {
                    Some(b)
                } else if prev == b {
                    Some(a)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn face_with(&self, a: u32, b: u32, c: u32) -> Option<usize> {
        self.adj.vert_faces[a as usize]
            .iter()
            .map(|&f| f as usize)
            .find(|&f| {
                let r = &self.sub.faces[f];
                r.contains(&b) && r.contains(&c)
            })
    }

    /// Frame of the face across the edge `(x, y)` (directed as the current
    /// face runs it), with its ring rotated so that the shared edge is
    /// reported as `shared = [first, second]`.
    fn across(&self, f: usize, x: u32, y: u32) -> Option<usize> {
        let e = *self.adj.edge_id.get(&edge_key(x, y).0)?;
        let fs = &self.adj.edge_faces[e as usize];
        if fs.len() != 2 {
            return None;
        }
        fs.iter().map(|&g| g as usize).find(|&g| g != f)
    }
}

fn ring_from(ring: &[u32], first: u32, second: u32) -> Option<[u32; 4]> {
    let k = ring.iter().position(|&x| x == first)?;
    if ring[(k + 1) % 4] != second {
        return None;
    }
    Some([
        ring[k],
        ring[(k + 1) % 4],
        ring[(k + 2) % 4],
        ring[(k + 3) % 4],
    ])
}

/// Control net (w+3) x (h+3) of the uniform B-spline for the vertex grid `v`.
fn extended_net(wk: &Walker, pts: &[DVec3], v: &[Vec<u32>]) -> Vec<Vec<DVec3>> {
    let (w, h) = (v.len() - 1, v[0].len() - 1);
    let (nu, nv) = (w + 3, h + 3);
    let mut id: Vec<Vec<Option<u32>>> = vec![vec![None; nv]; nu];
    let mut pos = vec![vec![DVec3::ZERO; nv]; nu];
    for i in 0..=w {
        for j in 0..=h {
            id[i + 1][j + 1] = Some(v[i][j]);
            pos[i + 1][j + 1] = pts[v[i][j] as usize];
        }
    }
    let mut put = |gi: usize,
                   gj: usize,
                   e: Option<u32>,
                   prev_pos: DVec3,
                   cur_pos: DVec3,
                   id: &mut Vec<Vec<Option<u32>>>| {
        id[gi][gj] = e;
        pos[gi][gj] = match e {
            Some(x) => pts[x as usize],
            None => cur_pos * 2.0 - prev_pos,
        };
    };
    for j in 0..=h {
        put(
            0,
            j + 1,
            wk.ext(v[1][j], v[0][j]),
            pts[v[1][j] as usize],
            pts[v[0][j] as usize],
            &mut id,
        );
        put(
            w + 2,
            j + 1,
            wk.ext(v[w - 1][j], v[w][j]),
            pts[v[w - 1][j] as usize],
            pts[v[w][j] as usize],
            &mut id,
        );
    }
    for i in 0..=w {
        put(
            i + 1,
            0,
            wk.ext(v[i][1], v[i][0]),
            pts[v[i][1] as usize],
            pts[v[i][0] as usize],
            &mut id,
        );
        put(
            i + 1,
            h + 2,
            wk.ext(v[i][h - 1], v[i][h]),
            pts[v[i][h - 1] as usize],
            pts[v[i][h] as usize],
            &mut id,
        );
    }
    for &(ki, kj) in &[(0usize, 0usize), (w + 2, 0), (0, h + 2), (w + 2, h + 2)] {
        let si: isize = if ki == 0 { 1 } else { -1 };
        let sj: isize = if kj == 0 { 1 } else { -1 };
        let at = |a: isize, b: isize| ((ki as isize + a) as usize, (kj as isize + b) as usize);
        let (ci, cj) = at(si, sj);
        let (ai, aj) = at(0, sj);
        let (bi, bj) = at(si, 0);
        let (a2i, a2j) = at(0, 2 * sj);
        let (b2i, b2j) = at(2 * si, 0);
        let (cid, aid, bid) = (id[ci][cj], id[ai][aj], id[bi][bj]);
        let real = match (cid, aid, bid) {
            (Some(c), Some(a), Some(b)) => wk.face_with(c, a, b).and_then(|f| {
                wk.sub.faces[f]
                    .iter()
                    .copied()
                    .find(|&x| x != c && x != a && x != b)
            }),
            _ => None,
        };
        let value = match real {
            Some(x) => pts[x as usize],
            None => {
                if aid.is_some() && bid.is_none() {
                    pos[ai][aj] * 2.0 - pos[a2i][a2j]
                } else {
                    pos[bi][bj] * 2.0 - pos[b2i][b2j]
                }
            }
        };
        id[ki][kj] = real;
        pos[ki][kj] = value;
    }
    pos
}

type Frame = [u32; 4];

/// Greedy rectangular packing of regular faces. Returns the plans and, for
/// every face, the frame (ring rotation) used for its lattice.
fn plan_patches(wk: &Walker, packed: bool) -> (Vec<PatchPlan>, Vec<Frame>, Vec<Vec<Vec<u32>>>) {
    let nf = wk.sub.faces.len();
    let mut frames: Vec<Frame> = wk
        .sub
        .faces
        .iter()
        .map(|r| {
            if r.len() == 4 {
                [r[0], r[1], r[2], r[3]]
            } else {
                [0; 4]
            }
        })
        .collect();
    let regular: Vec<bool> = (0..nf).map(|f| wk.regular_face(f)).collect();
    let mut assigned = vec![false; nf];
    let mut plans = Vec::new();
    let mut vgrids: Vec<Vec<Vec<u32>>> = Vec::new();
    for seed in 0..nf {
        if assigned[seed] {
            continue;
        }
        if !regular[seed] {
            assigned[seed] = true;
            plans.push(PatchPlan {
                faces: vec![(seed, 0, 0)],
                w: 1,
                h: 1,
                regular: false,
            });
            vgrids.push(Vec::new());
            continue;
        }
        let f0 = frames[seed];
        let mut grid: Vec<Vec<(usize, Frame)>> = vec![vec![(seed, f0)]];
        let mut taken: HashSet<usize> = HashSet::from([seed]);
        if packed {
            // extend along u
            loop {
                let (last_f, last) = *grid.last().expect("column").first().expect("row");
                let Some(g) = wk.across(last_f, last[1], last[2]) else {
                    break;
                };
                if !regular[g] || assigned[g] || taken.contains(&g) {
                    break;
                }
                let Some(fr) = ring_from_rev(&wk.sub.faces[g], last[2], last[1]) else {
                    break;
                };
                taken.insert(g);
                grid.push(vec![(g, fr)]);
                if grid.len() >= 64 {
                    break;
                }
            }
            // extend along v, whole rows at a time
            'rows: loop {
                let mut row: Vec<(usize, Frame)> = Vec::new();
                for col in &grid {
                    let (cf, fr) = *col.last().expect("column");
                    let Some(g) = wk.across(cf, fr[3], fr[2]) else {
                        break 'rows;
                    };
                    if !regular[g]
                        || assigned[g]
                        || taken.contains(&g)
                        || row.iter().any(|r| r.0 == g)
                    {
                        break 'rows;
                    }
                    let Some(nf_) = ring_from(&wk.sub.faces[g], fr[3], fr[2]) else {
                        break 'rows;
                    };
                    if let Some(prev) = row.last() {
                        if prev.1[1] != nf_[0] || prev.1[2] != nf_[3] {
                            break 'rows;
                        }
                    }
                    row.push((g, nf_));
                }
                for (ci, item) in row.into_iter().enumerate() {
                    taken.insert(item.0);
                    grid[ci].push(item);
                }
                if grid[0].len() >= 64 {
                    break;
                }
            }
        }
        let (w, h) = (grid.len(), grid[0].len());
        let mut faces = Vec::new();
        for (a, col) in grid.iter().enumerate() {
            for (b, &(f, fr)) in col.iter().enumerate() {
                assigned[f] = true;
                frames[f] = fr;
                faces.push((f, a, b));
            }
        }
        let mut v = vec![vec![0u32; h + 1]; w + 1];
        for a in 0..w {
            for b in 0..h {
                let fr = grid[a][b].1;
                v[a][b] = fr[0];
                if a == w - 1 {
                    v[a + 1][b] = fr[1];
                }
                if b == h - 1 {
                    v[a][b + 1] = fr[3];
                }
                if a == w - 1 && b == h - 1 {
                    v[a + 1][b + 1] = fr[2];
                }
            }
        }
        plans.push(PatchPlan {
            faces,
            w,
            h,
            regular: true,
        });
        vgrids.push(v);
    }
    (plans, frames, vgrids)
}

/// Ring rotated so that the directed edge `second -> first`... returns
/// `[first_of_new, ..]` such that the ring contains `a -> b` and the frame
/// starts at `b` (so the frame runs `b, next, next, a`).
fn ring_from_rev(ring: &[u32], a: u32, b: u32) -> Option<Frame> {
    let k = ring.iter().position(|&x| x == a)?;
    if ring[(k + 1) % 4] != b {
        return None;
    }
    // ring: a, b, c, d  ->  frame starting at b: b, c, d, a
    Some([
        ring[(k + 1) % 4],
        ring[(k + 2) % 4],
        ring[(k + 3) % 4],
        ring[k],
    ])
}

struct Lattice {
    /// Per face: (2^l + 1)^2 refined-vertex ids, index `i * m + j`.
    grids: Vec<Vec<u32>>,
    m: usize,
    refined: Subd,
}

fn build_lattice(cage: &Subd, level: usize, frames: &[Frame]) -> Lattice {
    let nf = cage.faces.len();
    let mut cur = cage.clone();
    let mut m = 2usize;
    // index i * m + j: (0,0) = c0, (0,1) = c3, (1,0) = c1, (1,1) = c2
    let mut grids: Vec<Vec<u32>> = frames
        .iter()
        .map(|f| vec![f[0], f[3], f[1], f[2]])
        .collect();
    for _ in 0..level {
        let adj = cur.adjacency();
        let (nv, ne) = (cur.points.len(), adj.edges.len());
        let fmap: HashMap<[u32; 4], u32> = cur
            .faces
            .iter()
            .enumerate()
            .map(|(fi, r)| {
                let mut k = [r[0], r[1], r[2], r[3]];
                k.sort_unstable();
                (k, fi as u32)
            })
            .collect();
        let nm = 2 * m - 1;
        let mut ng: Vec<Vec<u32>> = Vec::with_capacity(nf);
        for g in grids.iter().take(nf) {
            let mut n = vec![0u32; nm * nm];
            for i in 0..m {
                for j in 0..m {
                    n[2 * i * nm + 2 * j] = g[i * m + j];
                    if i + 1 < m {
                        let e = adj.edge_id[&edge_key(g[i * m + j], g[(i + 1) * m + j]).0];
                        n[(2 * i + 1) * nm + 2 * j] = (nv as u32) + e;
                    }
                    if j + 1 < m {
                        let e = adj.edge_id[&edge_key(g[i * m + j], g[i * m + j + 1]).0];
                        n[2 * i * nm + 2 * j + 1] = (nv as u32) + e;
                    }
                    if i + 1 < m && j + 1 < m {
                        let mut key = [
                            g[i * m + j],
                            g[(i + 1) * m + j],
                            g[(i + 1) * m + j + 1],
                            g[i * m + j + 1],
                        ];
                        key.sort_unstable();
                        n[(2 * i + 1) * nm + 2 * j + 1] = (nv + ne) as u32 + fmap[&key];
                    }
                }
            }
            ng.push(n);
        }
        grids = ng;
        cur = cur.refine();
        m = nm;
    }
    Lattice {
        grids,
        m,
        refined: cur,
    }
}

fn solve_small(a: &[f64], n: usize, rhs: &[DVec3]) -> Vec<DVec3> {
    let mut out = vec![DVec3::ZERO; n];
    for c in 0..3 {
        let b: Vec<f64> = rhs.iter().map(|v| v[c]).collect();
        let sol = solve_dense(a, n, &b).unwrap_or_else(|| vec![0.0; n]);
        for i in 0..n {
            out[i][c] = sol[i];
        }
    }
    out
}

/// Fit a cubic Bezier curve to samples taken at uniform parameter steps, end
/// points fixed. Uniform steps match the dyadic subdivision parameter, so the
/// fit is exact on edges whose limit curve is a cubic.
fn fit_edge(q: &[DVec3]) -> [DVec3; 4] {
    let n = q.len();
    let (p0, p3) = (q[0], q[n - 1]);
    if n < 4 {
        return [p0, p0 + (p3 - p0) / 3.0, p0 + (p3 - p0) * (2.0 / 3.0), p3];
    }
    let (mut a, mut rhs) = ([0.0; 4], [DVec3::ZERO; 2]);
    for (k, qk) in q.iter().enumerate().take(n - 1).skip(1) {
        let b = bezier_basis(k as f64 / (n - 1) as f64);
        let r = *qk - p0 * b[0] - p3 * b[3];
        a[0] += b[1] * b[1];
        a[1] += b[1] * b[2];
        a[3] += b[2] * b[2];
        rhs[0] += r * b[1];
        rhs[1] += r * b[2];
    }
    a[2] = a[1];
    let sol = solve_small(&a, 2, &rhs);
    [p0, sol[0], sol[1], p3]
}

/// Check direct callers as well as node inputs before adjacency indexes vertices.
fn validate_cage(cage: &Subd) -> Result<(), String> {
    if cage.faces.is_empty() || cage.points.iter().any(|p| !p.is_finite()) {
        return Err("a finite cage with usable faces is required".into());
    }
    for f in &cage.faces {
        if f.len() < 3
            || f.iter().any(|&v| v as usize >= cage.points.len())
            || f.iter().copied().collect::<HashSet<_>>().len() != f.len()
        {
            return Err("cage faces require distinct, valid vertex indices".into());
        }
        let origin = cage.points[f[0] as usize];
        let area = (1..f.len() - 1).fold(DVec3::ZERO, |sum, i| {
            sum + (cage.points[f[i] as usize] - origin)
                .cross(cage.points[f[i + 1] as usize] - origin)
        });
        if !area.is_finite() || area.length_squared() <= 1e-24 {
            return Err("cage faces must have nonzero finite area".into());
        }
    }
    if cage.adjacency().edge_faces.iter().any(|fs| fs.len() > 2) {
        return Err("nonmanifold cage edges are unsupported".into());
    }
    Ok(())
}

/// Complete expected joins for unpacked quad patches, in face order. Side
/// directions follow increasing patch parameter, rather than the face winding.
fn expected_seams(cage: &Subd) -> (Vec<SeamRef>, usize) {
    let mut edges: HashMap<(u32, u32), Vec<(usize, usize, u32)>> = HashMap::new();
    for (f, r) in cage.faces.iter().enumerate() {
        for (side, (x, y)) in [(r[0], r[1]), (r[1], r[2]), (r[3], r[2]), (r[0], r[3])]
            .into_iter()
            .enumerate()
        {
            edges
                .entry(edge_key(x, y).0)
                .or_default()
                .push((f, side, x));
        }
    }
    let mut seams = Vec::new();
    let mut free = 0;
    for fs in edges.values() {
        if fs.len() == 2 {
            let (a, side_a, x) = fs[0];
            let (b, side_b, y) = fs[1];
            seams.push(SeamRef {
                a,
                side_a,
                b,
                side_b,
                reversed: x != y,
            });
        } else {
            free += fs.len();
        }
    }
    seams.sort_by_key(|s| (s.a, s.side_a, s.b, s.side_b));
    (seams, free)
}

/// Index an edge row and its first inward row in a bicubic Bezier net.
fn seam_control(side: usize, k: usize, inward: bool) -> usize {
    let d = usize::from(inward);
    match side {
        0 => k * 4 + d,
        1 => (3 - d) * 4 + k,
        2 => k * 4 + 3 - d,
        _ => d * 4 + k,
    }
}

/// Local C1 candidates against exact regular neighbors. Only fitted interior
/// coefficients move; shared curves and regular Catmull-Clark patches are fixed.
/// Conflicting corner-row constraints are averaged, then the caller measures
/// whether the candidate actually improves the entire network.
fn reconcile_regular_neighbors(
    cage: &Subd,
    conv: &mut Conversion,
    seams: &[SeamRef],
    fit_level: usize,
) -> bool {
    let adj = cage.adjacency();
    let wk = Walker::new(cage, &adj);
    let mut targets = vec![[DVec3::ZERO; 16]; conv.patches.len()];
    let mut counts = vec![[0usize; 16]; conv.patches.len()];
    for seam in seams {
        let (a, sa, b, sb) = if wk.regular_face(seam.a) && !wk.regular_face(seam.b) {
            (seam.a, seam.side_a, seam.b, seam.side_b)
        } else if wk.regular_face(seam.b) && !wk.regular_face(seam.a) {
            (seam.b, seam.side_b, seam.a, seam.side_a)
        } else {
            continue;
        };
        for k in 1..3 {
            let kb = if seam.reversed { 3 - k } else { k };
            let edge = conv.patches[a].control_points[seam_control(sa, k, false)];
            let inside = conv.patches[a].control_points[seam_control(sa, k, true)];
            let index = seam_control(sb, kb, true);
            targets[b][index] += edge * 2.0 - inside;
            counts[b][index] += 1;
        }
    }
    let mut changed = false;
    for (p, patch) in conv.patches.iter_mut().enumerate() {
        for k in 0..16 {
            if counts[p][k] > 0 {
                patch.control_points[k] = targets[p][k] / counts[p][k] as f64;
                changed = true;
            }
        }
    }
    if changed {
        // Re-evaluate the same limit lattice after editing, never retain the
        // pre-edit approximation residual as a claim about the new surface.
        let (_, frames, _) = plan_patches(&wk, false);
        let mut level = fit_level.clamp(1, 4) + 1;
        while level > 2 && cage.faces.len().saturating_mul(4usize.pow(level as u32)) > 3_000_000 {
            level -= 1;
        }
        let lat = build_lattice(cage, level, &frames);
        let limit = lat.refined.limit_positions();
        let mut sum = 0.0;
        conv.max_dev = 0.0;
        conv.samples = 0;
        for (f, patch) in conv.patches.iter().enumerate() {
            for i in 0..lat.m {
                for j in 0..lat.m {
                    let d = patch
                        .point(i as f64 / (lat.m - 1) as f64, j as f64 / (lat.m - 1) as f64)
                        .distance(limit[lat.grids[f][i * lat.m + j] as usize]);
                    conv.max_dev = conv.max_dev.max(d);
                    sum += d * d;
                    conv.samples += 1;
                }
            }
        }
        conv.rms_dev = (sum / conv.samples.max(1) as f64).sqrt();
    }
    changed
}

/// Convert a cage to NURBS patches.
pub fn convert(
    cage_in: &Subd,
    packed: bool,
    fit_level: usize,
    max_faces: usize,
) -> Result<Conversion, String> {
    validate_cage(cage_in)?;
    let (cage, refined_first) = if cage_in.all_quads() {
        (cage_in.clone(), false)
    } else {
        (cage_in.refine(), true)
    };
    let adj = cage.adjacency();
    let wk = Walker::new(&cage, &adj);
    let (plans, frames, vgrids) = plan_patches(&wk, packed);
    let nf = cage.faces.len();

    // lattice level: one finer than the fit samples, as far as the budget allows
    let mut level = fit_level.clamp(1, 5) + 1;
    while level > 2 && nf.saturating_mul(4usize.pow(level as u32)) > max_faces {
        level -= 1;
    }
    if nf.saturating_mul(4usize.pow(level as u32)) > max_faces {
        return Err(format!("{nf} cage faces are too many to sample at the minimum lattice within the limit of {max_faces}; reduce the cage or raise the limit"));
    }
    let lat = build_lattice(&cage, level, &frames);
    let limit = lat.refined.limit_positions();
    let m = lat.m;
    let stride = if level >= 3 { 2 } else { 1 };
    let g = |f: usize, i: usize, j: usize| limit[lat.grids[f][i * m + j] as usize];

    // fitted edge curves for edges next to irregular faces
    let mut edge_curves: HashMap<(u32, u32), [DVec3; 4]> = HashMap::new();
    for plan in plans.iter() {
        if plan.regular {
            continue;
        }
        let f = plan.faces[0].0;
        let fr = frames[f];
        // sides: bottom c0->c1 (j=0, i along), right c1->c2, top c3->c2, left c0->c3
        let sample = |side: usize, k: usize| match side {
            0 => g(f, k, 0),
            1 => g(f, m - 1, k),
            2 => g(f, k, m - 1),
            _ => g(f, 0, k),
        };
        let ends = [
            (fr[0], fr[1]),
            (fr[1], fr[2]),
            (fr[3], fr[2]),
            (fr[0], fr[3]),
        ];
        for (side, &(x, y)) in ends.iter().enumerate() {
            let key = edge_key(x, y).0;
            edge_curves.entry(key).or_insert_with(|| {
                let q: Vec<DVec3> = (0..m).step_by(stride).map(|k| sample(side, k)).collect();
                let mut b = fit_edge(&q);
                if x > y {
                    b.reverse();
                }
                b
            });
        }
    }

    let mut surfaces: Vec<NurbsSurface> = Vec::new();
    let mut regular_faces = 0;
    let mut irregular_faces = 0;
    let mut dev_max: f64 = 0.0;
    let (mut dev_sum2, mut dev_n) = (0.0, 0usize);
    for (pi, plan) in plans.iter().enumerate() {
        if plan.regular {
            let net = extended_net(&wk, &cage.points, &vgrids[pi]);
            let surf = uniform_to_clamped(&net);
            for &(f, a, b) in &plan.faces {
                for i in 0..m {
                    for j in 0..m {
                        let u = (a as f64 + i as f64 / (m - 1) as f64) / plan.w as f64;
                        let v = (b as f64 + j as f64 / (m - 1) as f64) / plan.h as f64;
                        let d = (surf.point(u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)) - g(f, i, j))
                            .length();
                        dev_max = dev_max.max(d);
                        dev_sum2 += d * d;
                        dev_n += 1;
                    }
                }
            }
            regular_faces += plan.faces.len();
            surfaces.push(surf);
        } else {
            let f = plan.faces[0].0;
            let fr = frames[f];
            let curve = |x: u32, y: u32| -> [DVec3; 4] {
                let mut c = edge_curves[&edge_key(x, y).0];
                if x > y {
                    c.reverse();
                }
                c
            };
            let (bot, right, top, left) = (
                curve(fr[0], fr[1]),
                curve(fr[1], fr[2]),
                curve(fr[3], fr[2]),
                curve(fr[0], fr[3]),
            );
            let mut b = [[DVec3::ZERO; 4]; 4];
            for k in 0..4 {
                b[k][0] = bot[k];
                b[k][3] = top[k];
                b[0][k] = left[k];
                b[3][k] = right[k];
            }
            // limit values pin the corners exactly
            b[0][0] = g(f, 0, 0);
            b[3][0] = g(f, m - 1, 0);
            b[3][3] = g(f, m - 1, m - 1);
            b[0][3] = g(f, 0, m - 1);
            // interior control points by least squares
            let idx = [(1usize, 1usize), (1, 2), (2, 1), (2, 2)];
            let mut a = [0.0; 16];
            let mut rhs = [DVec3::ZERO; 4];
            for i in (0..m).step_by(stride) {
                for j in (0..m).step_by(stride) {
                    let (bu, bv) = (
                        bezier_basis(i as f64 / (m - 1) as f64),
                        bezier_basis(j as f64 / (m - 1) as f64),
                    );
                    let mut r = g(f, i, j);
                    for p in 0..4 {
                        for q in 0..4 {
                            if !(1..=2).contains(&p) || !(1..=2).contains(&q) {
                                r -= b[p][q] * (bu[p] * bv[q]);
                            }
                        }
                    }
                    let w: Vec<f64> = idx.iter().map(|&(p, q)| bu[p] * bv[q]).collect();
                    for s in 0..4 {
                        rhs[s] += r * w[s];
                        for t in 0..4 {
                            a[s * 4 + t] += w[s] * w[t];
                        }
                    }
                }
            }
            let sol = solve_small(&a, 4, &rhs);
            for (s, &(p, q)) in idx.iter().enumerate() {
                b[p][q] = sol[s];
            }
            let cps: Vec<DVec3> = (0..4)
                .flat_map(|i| (0..4).map(move |j| (i, j)))
                .map(|(i, j)| b[i][j])
                .collect();
            let surf = NurbsSurface::clamped_uniform(3, 3, 4, 4, cps);
            for i in 0..m {
                for j in 0..m {
                    let d = (surf.point(i as f64 / (m - 1) as f64, j as f64 / (m - 1) as f64)
                        - g(f, i, j))
                    .length();
                    dev_max = dev_max.max(d);
                    dev_sum2 += d * d;
                    dev_n += 1;
                }
            }
            irregular_faces += 1;
            surfaces.push(surf);
        }
    }
    Ok(Conversion {
        patches: surfaces,
        cage_faces: nf,
        regular_faces,
        irregular_faces,
        extraordinary_vertices: cage.extraordinary_count(&adj),
        max_dev: dev_max,
        rms_dev: (dev_sum2 / dev_n.max(1) as f64).sqrt(),
        samples: dev_n,
        refined_first,
    })
}

fn patch_report(
    title: &str,
    conv: &Conversion,
    ps: &PatchSet,
    max_dev_limit: f64,
    diag: f64,
) -> Report {
    let mut report = Report::new(title);
    let seams = find_seams(ps, 1e-4 * diag.max(1.0), 8);
    report.metric(
        "patch_count",
        Metric::new(conv.patches.len() as f64, "count"),
    );
    report.metric(
        "control_points",
        Metric::new(ps.control_point_count() as f64, "count"),
    );
    report.metric("cage_faces", Metric::new(conv.cage_faces as f64, "count"));
    report.metric(
        "regular_faces",
        Metric::new(conv.regular_faces as f64, "count"),
    );
    report.metric(
        "extraordinary_patches",
        Metric::new(conv.irregular_faces as f64, "count"),
    );
    report.metric(
        "extraordinary_vertices",
        Metric::new(conv.extraordinary_vertices as f64, "count"),
    );
    report.metric(
        "dev_max_mm",
        Metric::new(conv.max_dev, "mm").with_limit(max_dev_limit, true),
    );
    report.metric("dev_rms_mm", Metric::new(conv.rms_dev, "mm"));
    report.metric("dev_samples", Metric::new(conv.samples as f64, "count"));
    report.metric("seam_gap_max_mm", Metric::new(seams.max_gap(), "mm"));
    report.metric("g1_max_deg", Metric::new(seams.max_g1_deg(), "deg"));
    report.metric(
        "free_boundary_edges",
        Metric::new(seams.free_edges.len() as f64, "count"),
    );
    if conv.refined_first {
        report.note(
            Severity::Info,
            "the cage had non-quad faces and was refined once before conversion",
        );
    }
    report
}

/// Most faces `surface.subd_to_nurbs` fits after refining the cage.
pub const MAX_REFINED_FACES: f64 = 50_000.0;

/// `surface.subd_to_nurbs`
pub struct SubdToNurbsNode;

impl NodeOp for SubdToNurbsNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.subd_to_nurbs", "Subdivision to NURBS", SPEC_MESH)
            .input(PortSpec::new("cage", DataKind::Geometry))
            .output(PortSpec::new("patches", DataKind::Json))
            .output(PortSpec::new("preview", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("faces", "Faces", "packed", &["packed", "unpacked"]).help("Packed merges rectangular runs of regular quads into one larger patch; unpacked makes one patch per quad."))
            .param(ParamSpec::choice("ev_continuity", "Extraordinary vertex continuity", "g0", &["g0", "g1", "g1x", "g1xx", "g2"]).help("Continuity wanted around extraordinary vertices. Only position continuity is enforced for now; the achieved normal angle is reported."))
            .param(ParamSpec::int("refine_levels", "Refine levels", 0, 0, 3).help("Subdivide the cage this many times first; this pushes extraordinary vertices into smaller patches."))
            .param(ParamSpec::int("fit_level", "Fit resolution", 2, 1, 4).help("Subdivision depth of the limit samples used to fit patches near extraordinary vertices."))
            .param(ParamSpec::float("max_deviation_mm", "Max deviation", 0.5, 0.0001, 100.0).unit("mm").help("Limit on the largest distance between the patches and the limit surface."))
            .param(ParamSpec::int("max_faces", "Sample budget", 3_000_000, 1000, 100_000_000).help("Largest number of sampled subdivision faces."))
            .param(ParamSpec::float("preview_tol_mm", "Preview tolerance", 0.2, 0.001, 10.0).unit("mm").help("Chord tolerance of the preview mesh."))
            .param(ParamSpec::float("weld_tol_mm", "Weld tolerance", 1e-6, 0.0, 10.0).unit("mm").help("Vertices closer than this are merged first."))
            .help("Converts a subdivision cage into bicubic NURBS patches. Regular regions are exact; patches next to extraordinary vertices are fitted to the limit surface and share their edge curves with neighbours. Reports patch counts, the deviation from the limit surface and the largest normal angle across seams.")
            .emulates("Rhino ToNURBS")
            .emulates("SubD to NURBS conversion")
            .implemented()
    }

    fn cook(
        &self,
        ctx: &CookContext,
        inputs: &[Option<Data>],
        params: &Params,
    ) -> CookResult<Vec<Data>> {
        let cage = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &cage)?;
        let tol = params
            .float("weld_tol_mm")?
            .max(1.2e-7 * max_abs_coord(&cage.points));
        let mut sub = Subd::from_geometry(&cage, tol);
        if sub.faces.is_empty() {
            return Err(ctx.fail("the cage has no usable faces"));
        }
        let levels = params.int("refine_levels")?.clamp(0, 3);
        // Each level makes four faces of one; refuse a cage the fit would take minutes on.
        let faces = sub.faces.len() as f64 * 4f64.powi(levels as i32);
        if faces > MAX_REFINED_FACES {
            return Err(ctx.fail(format!("{levels} refine levels would make about {faces:.0} faces (limit {MAX_REFINED_FACES:.0}); lower the refine levels or simplify the cage")));
        }
        for _ in 0..levels {
            sub = sub.refine();
        }
        let max_faces = params.float("max_faces")? as usize;
        let conv = convert(
            &sub,
            params.str("faces")? == "packed",
            params.int("fit_level")?.clamp(1, 4) as usize,
            max_faces,
        )
        .map_err(|e| ctx.fail(e))?;
        let ps = PatchSet::new(conv.patches.clone());
        let diag = crate::util::bbox_diagonal(&cage);
        let lim = positive(ctx, "max_deviation_mm", params.float("max_deviation_mm")?)?;
        let mut report = patch_report("Subdivision to NURBS", &conv, &ps, lim, diag);
        let ev = params.str("ev_continuity")?;
        if ev != "g0" && conv.irregular_faces > 0 {
            report.note(Severity::Warning, format!("{ev} continuity around extraordinary vertices is not enforced yet; patches there are position-continuous and the measured normal angle is in g1_max_deg"));
        }
        let mut ps = ps;
        ps.closed = report.metrics["free_boundary_edges"].value == 0.0;
        let preview = ps.tessellate(positive(
            ctx,
            "preview_tol_mm",
            params.float("preview_tol_mm")?,
        )?);
        Ok(vec![
            Data::json(ps.to_json()),
            Data::geometry(preview),
            Data::report(report),
        ])
    }
}

/// `surface.subd_deviation`
pub struct SubdDeviationNode;

impl NodeOp for SubdDeviationNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.subd_deviation", "Subdivision vs NURBS Deviation", SPEC_MESH)
            .input(PortSpec::new("cage", DataKind::Geometry))
            .input(PortSpec::new("patches", DataKind::Json))
            .output(PortSpec::new("deviation", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::int("levels", "Limit levels", 3, 1, 5).help("Subdivision depth of the reference limit mesh."))
            .param(ParamSpec::int("samples_per_patch", "Samples per patch", 256, 16, 100_000).help("Surface points tested on every patch."))
            .param(ParamSpec::float("tol_mm", "Tolerance", 0.5, 0.0001, 100.0).unit("mm").help("Limit applied to the 95th percentile deviation."))
            .param(ParamSpec::float("weld_tol_mm", "Weld tolerance", 1e-6, 0.0, 10.0).unit("mm").help("Vertices closer than this are merged first."))
            .help("Compares NURBS patches with the limit surface of a subdivision cage. Patch sample points are measured against a finely subdivided limit mesh; the report holds the deviation percentiles and the largest normal angle across patch seams.")
            .emulates("Rhino ToNURBS quality check")
            .implemented()
    }

    fn cook(
        &self,
        ctx: &CookContext,
        inputs: &[Option<Data>],
        params: &Params,
    ) -> CookResult<Vec<Data>> {
        let cage = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &cage)?;
        let pj = json_input(inputs, 1).ok_or_else(|| ctx.fail("connect a patch set"))?;
        let ps = PatchSet::from_json(&pj).map_err(|e| json_err(ctx, "patch set input", e))?;
        let tol = positive(ctx, "tol_mm", params.float("tol_mm")?)?;
        let sub = Subd::from_geometry(
            &cage,
            params
                .float("weld_tol_mm")?
                .max(1.2e-7 * max_abs_coord(&cage.points)),
        );
        if sub.faces.is_empty() {
            return Err(ctx.fail("the cage has no usable faces"));
        }
        let levels = params.int("levels")?.clamp(1, 5) as usize;
        let predicted = sub
            .refined_face_count()
            .saturating_mul(4usize.pow((levels - 1) as u32));
        if predicted > 4_000_000 {
            return Err(ctx.fail(format!(
                "{levels} levels would create {predicted} faces; lower the level count"
            )));
        }
        let fine = sub.refine_levels(levels);
        let lim = fine.limit_positions();
        let mut reference = fine.to_geometry();
        reference.points[..lim.len()].copy_from_slice(&lim);
        let bvh = TriBvh::from_geometry(&reference, Some(1e-9));
        let per = params.int("samples_per_patch")?.clamp(16, 100_000) as usize;
        let side = (per as f64).sqrt().ceil() as usize;
        let mut pts = Vec::new();
        for p in &ps.patches {
            let (ua, ub) = p.surface.domain_u();
            let (va, vb) = p.surface.domain_v();
            let us: Vec<f64> = (0..side)
                .map(|i| ua + (ub - ua) * i as f64 / (side - 1) as f64)
                .collect();
            let vs: Vec<f64> = (0..side)
                .map(|j| va + (vb - va) * j as f64 / (side - 1) as f64)
                .collect();
            pts.extend(p.surface.eval_grid(&us, &vs));
        }
        let hits = bvh.closest_many(&pts);
        let d: Vec<f64> = hits.iter().map(|h| h.map_or(0.0, |h| h.signed)).collect();
        let st = dev_stats(&d, tol);
        let diag = crate::util::bbox_diagonal(&cage);
        let seams = find_seams(&ps, 1e-4 * diag.max(1.0), 8);
        let mut report = Report::new("Subdivision vs NURBS deviation");
        report.metric("dev_mean_mm", Metric::new(st.mean, "mm"));
        report.metric("dev_rms_mm", Metric::new(st.rms, "mm"));
        report.metric("dev_max_mm", Metric::new(st.max_abs, "mm"));
        report.metric("dev_p50_mm", Metric::new(st.p50, "mm"));
        report.metric(
            "dev_p95_mm",
            Metric::new(st.p95, "mm").with_limit(tol, true),
        );
        report.metric("dev_p99_mm", Metric::new(st.p99, "mm"));
        report.metric("g1_max_deg", Metric::new(seams.max_g1_deg(), "deg"));
        report.metric("seam_gap_max_mm", Metric::new(seams.max_gap(), "mm"));
        report.metric("sample_count", Metric::new(st.count as f64, "count"));
        let mut out = Geometry::from_points(pts);
        out.point_attribs.set("dist", AttribArray::Float(d.clone()));
        out.point_attribs.set(
            "Cd",
            AttribArray::Vec3(
                d.iter()
                    .map(|&x| crate::deviation::abs_ramp(x, tol))
                    .collect(),
            ),
        );
        Ok(vec![Data::geometry(out), Data::report(report)])
    }
}

/// `surface.mesh_to_g2_surface`
pub struct MeshToG2SurfaceNode;

impl NodeOp for MeshToG2SurfaceNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.mesh_to_g2_surface", "G2 Mesh to Surface", crate::util::SPEC_RE)
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("patches", DataKind::Json))
            .output(PortSpec::new("preview", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("continuity", "Continuity", "g2", &["g1", "g2"]).help("Continuity wanted between patches."))
            .param(ParamSpec::int("max_degree", "Max degree", 6, 3, 6).help("Highest patch degree the construction may use."))
            .param(ParamSpec::float("max_deviation_mm", "Max deviation", 0.3, 0.0001, 100.0).unit("mm").help("Limit on the distance between the patches and the limit surface of the quad mesh."))
            .param(ParamSpec::int("fit_level", "Fit resolution", 3, 1, 4).help("Subdivision depth of the samples used near extraordinary vertices."))
            .param(ParamSpec::float("preview_tol_mm", "Preview tolerance", 0.2, 0.001, 10.0).unit("mm").help("Chord tolerance of the preview mesh."))
            .help("Converts a mesh through its subdivision limit surface. Regular regions are exact bicubics; extraordinary regions are fitted. Reports sampled position, tangent-plane and curvature continuity over every topological join, including partial G2 coverage.")
            .emulates("Plasticity PolySplines")
            .emulates("SurfLab G2 spline constructions")
            .implemented()
    }

    fn cook(
        &self,
        ctx: &CookContext,
        inputs: &[Option<Data>],
        params: &Params,
    ) -> CookResult<Vec<Data>> {
        let mesh = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &mesh)?;
        mesh.validate()
            .map_err(|e| ctx.fail(format!("invalid mesh: {e}")))?;
        let original =
            Subd::from_geometry(&mesh, 1e-6f64.max(1.2e-7 * max_abs_coord(&mesh.points)));
        validate_cage(&original).map_err(|e| ctx.fail(e))?;
        let refined_first = !original.all_quads();
        if refined_first && original.refined_face_count() > MAX_REFINED_FACES as usize {
            return Err(ctx.fail("refining non-quad input exceeds the cage face budget"));
        }
        let sub = if refined_first {
            original.refine()
        } else {
            original
        };
        let fit = params.int("fit_level")?.clamp(1, 4) as usize;
        let max_degree = params.int("max_degree")?;
        if !(3..=6).contains(&max_degree) {
            return Err(ctx.fail("max_degree must be between 3 and 6"));
        }
        let lim = positive(ctx, "max_deviation_mm", params.float("max_deviation_mm")?)?;
        let mut conv = convert(&sub, false, fit, 3_000_000).map_err(|e| ctx.fail(e))?;
        conv.refined_first = refined_first;
        let (expected, free) = expected_seams(&sub);
        let mut ps = PatchSet::new(conv.patches.clone());
        let mut stats = measure(&ps, &expected, 17);
        let mut candidate = conv.clone();
        let mut accepted = false;
        if reconcile_regular_neighbors(&sub, &mut candidate, &expected, fit) {
            let candidate_ps = PatchSet::new(candidate.patches.clone());
            let trial = measure(&candidate_ps, &expected, 17);
            if trial.invalid_samples <= stats.invalid_samples
                && trial.g1_fraction >= stats.g1_fraction
                && (params.str("continuity")? != "g2" || trial.g2_fraction >= stats.g2_fraction)
                && trial.max_normal_deg < stats.max_normal_deg
                && candidate.max_dev <= lim
            {
                conv = candidate;
                ps = candidate_ps;
                stats = trial;
                accepted = true;
            }
        }
        ps.closed = free == 0;
        let mut report = Report::new("Measured mesh to surface");
        for (key, value, unit) in [
            ("patch_count", conv.patches.len() as f64, "count"),
            ("control_points", ps.control_point_count() as f64, "count"),
            ("cage_faces", conv.cage_faces as f64, "count"),
            ("regular_faces", conv.regular_faces as f64, "count"),
            (
                "extraordinary_patches",
                conv.irregular_faces as f64,
                "count",
            ),
            (
                "extraordinary_vertices",
                conv.extraordinary_vertices as f64,
                "count",
            ),
            ("dev_rms_mm", conv.rms_dev, "mm"),
            ("dev_samples", conv.samples as f64, "count"),
            ("seam_gap_max_mm", stats.max_gap_mm, "mm"),
            ("g1_max_deg", stats.max_normal_deg, "deg"),
            (
                "curvature_jump_max_inv_mm",
                stats.max_curvature_jump_inv_mm,
                "1/mm",
            ),
            ("g1_fraction", stats.g1_fraction, "fraction"),
            ("g2_fraction", stats.g2_fraction, "fraction"),
            ("expected_seams", stats.expected_seams as f64, "count"),
            (
                "invalid_continuity_samples",
                stats.invalid_samples as f64,
                "count",
            ),
            ("free_boundary_edges", free as f64, "count"),
            ("g1_reconciliation_applied", f64::from(accepted), "bool"),
            ("max_degree_used", 3.0, "degree"),
        ] {
            report.metric(key, Metric::new(value, unit));
        }
        report.metric(
            "dev_max_mm",
            Metric::new(conv.max_dev, "mm").with_limit(lim, true),
        );
        report.metric("g1_target_deg", Metric::new(1.0, "deg"));
        report.metric(
            "invalid_continuity_samples",
            Metric::new(stats.invalid_samples as f64, "count").with_limit(0.0, true),
        );
        let requested = if params.str("continuity")? == "g2" {
            "g2_fraction"
        } else {
            "g1_fraction"
        };
        let fraction = if params.str("continuity")? == "g2" {
            stats.g2_fraction
        } else {
            stats.g1_fraction
        };
        report.metric(
            requested,
            Metric::new(fraction, "fraction").with_limit(1.0, false),
        );
        if fraction < 1.0 || expected.is_empty() {
            report.note(Severity::Warning, "requested continuity is only partially achieved; inspect g1_fraction, g2_fraction and invalid_continuity_samples. Extraordinary regions use fitted bicubics, without a guaranteed curvature-continuous cap");
        }
        if refined_first {
            report.note(
                Severity::Info,
                "non-quad input was refined once before fitting",
            );
        }
        let preview = ps.tessellate(positive(
            ctx,
            "preview_tol_mm",
            params.float("preview_tol_mm")?,
        )?);
        Ok(vec![
            Data::json(ps.to_json()),
            Data::geometry(preview),
            Data::report(report),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{b, cook, f, geo_of, grid, i, report_of, s, some};

    fn pinned_grid(nx: usize, ny: usize) -> Subd {
        let g = grid(nx, ny, nx as f64 * 10.0, ny as f64 * 10.0, |x, y| {
            6.0 * (x / 25.0).sin() + 4.0 * (y / 18.0).cos()
        });
        let mut sub = Subd::from_geometry(&g, 1e-9);
        let (lo, hi) = (
            DVec3::ZERO,
            DVec3::new(nx as f64 * 10.0, ny as f64 * 10.0, 0.0),
        );
        for (k, p) in sub.points.iter().enumerate() {
            if ((p.x - lo.x).abs() < 1e-9 || (p.x - hi.x).abs() < 1e-9)
                && ((p.y - lo.y).abs() < 1e-9 || (p.y - hi.y).abs() < 1e-9)
            {
                sub.corners.insert(k as u32);
            }
        }
        sub
    }

    #[test]
    fn periodic_regular_torus_measures_every_topology_join() {
        let (nu, nv) = (8, 6);
        let points = (0..nu)
            .flat_map(|i| {
                (0..nv).map(move |j| {
                    let u = std::f64::consts::TAU * i as f64 / nu as f64;
                    let v = std::f64::consts::TAU * j as f64 / nv as f64;
                    DVec3::new(
                        (30.0 + 10.0 * v.cos()) * u.cos(),
                        (30.0 + 10.0 * v.cos()) * u.sin(),
                        10.0 * v.sin(),
                    )
                })
            })
            .collect();
        let faces = (0..nu)
            .flat_map(|i| {
                (0..nv).map(move |j| {
                    vec![
                        (i * nv + j) as u32,
                        (((i + 1) % nu) * nv + j) as u32,
                        (((i + 1) % nu) * nv + (j + 1) % nv) as u32,
                        (i * nv + (j + 1) % nv) as u32,
                    ]
                })
            })
            .collect();
        let cage = Subd {
            points,
            faces,
            ..Default::default()
        };
        let conv = convert(&cage, false, 2, 3_000_000).unwrap();
        let (seams, free) = expected_seams(&cage);
        assert_eq!(seams.len(), 2 * nu * nv);
        assert_eq!(free, 0);
        let mut ps = PatchSet::new(conv.patches);
        let stats = measure(&ps, &seams, 17);
        assert_eq!(stats.g2_fraction, 1.0, "{stats:?}");
        for p in &mut ps.patches[0].surface.control_points {
            p.z += 1.0;
        }
        let stats = measure(&ps, &seams, 17);
        assert_eq!(stats.expected_seams, 2 * nu * nv);
        assert!(stats.max_gap_mm > 0.99);
        assert!(stats.g2_fraction < 1.0);
    }

    #[test]
    fn malformed_cages_fail_before_lattice_construction() {
        let cage = Subd {
            points: (0..4).map(|i| DVec3::X * i as f64).collect(),
            faces: vec![vec![0, 1, 2, 3]],
            ..Default::default()
        };
        assert!(convert(&cage, false, 2, 1_000_000).is_err());
        let mut cage = pinned_grid(2, 2);
        cage.faces[0][0] = u32::MAX;
        assert!(convert(&cage, false, 2, 1_000_000).is_err());
    }

    #[test]
    fn valence_five_cap_has_measured_partial_g2() {
        let n = 5;
        let mut points = vec![DVec3::new(0.0, 0.0, 8.0)];
        for i in 0..2 * n {
            let t = std::f64::consts::TAU * i as f64 / (2 * n) as f64;
            let radius = if i % 2 == 0 { 20.0 } else { 30.0 };
            points.push(DVec3::new(radius * t.cos(), radius * t.sin(), 0.0));
        }
        let faces = (0..n)
            .map(|i| {
                vec![
                    0,
                    1 + (2 * i) as u32,
                    2 + (2 * i) as u32,
                    1 + (2 * ((i + 1) % n)) as u32,
                ]
            })
            .collect();
        let sub = Subd {
            points,
            faces,
            ..Default::default()
        }
        .refine_levels(2);
        let out = cook(
            "surface.mesh_to_g2_surface",
            &[some(&sub.to_geometry())],
            &[("max_degree", i(3)), ("fit_level", i(2))],
        )
        .unwrap();
        let r = report_of(&out[2]);
        assert_eq!(r.metrics["expected_seams"].value, 140.0);
        assert!(r.metrics["g2_fraction"].value > 0.0);
        assert!(r.metrics["g2_fraction"].value < 1.0);
        println!(
            "valence5 g2={} g1={} dev={} curvature_jump={}",
            r.metrics["g2_fraction"].value,
            r.metrics["g1_fraction"].value,
            r.metrics["dev_max_mm"].value,
            r.metrics["curvature_jump_max_inv_mm"].value
        );
        assert_eq!(r.metrics["invalid_continuity_samples"].value, 0.0);
        assert_eq!(r.metrics["max_degree_used"].value, 3.0);
    }

    #[test]
    fn reconciliation_recomputes_deviation_after_control_edits() {
        let cube =
            Subd::from_geometry(&Geometry::make_box(DVec3::splat(100.0)), 1e-9).refine_levels(2);
        let mut c = convert(&cube, false, 2, 3_000_000).unwrap();
        let before = c.clone();
        let (seams, _) = expected_seams(&cube);
        assert!(reconcile_regular_neighbors(&cube, &mut c, &seams, 2));
        assert!(c
            .patches
            .iter()
            .zip(&before.patches)
            .any(|(a, b)| a.control_points != b.control_points));
        let old_stats = measure(&PatchSet::new(before.patches.clone()), &seams, 17);
        let new_stats = measure(&PatchSet::new(c.patches.clone()), &seams, 17);
        assert_eq!(old_stats.expected_seams, new_stats.expected_seams);
        assert_eq!(new_stats.invalid_samples, 0);
        let adj = cube.adjacency();
        let (_, frames, _) = plan_patches(&Walker::new(&cube, &adj), false);
        let lat = build_lattice(&cube, 3, &frames);
        let limit = lat.refined.limit_positions();
        let mut worst: f64 = 0.0;
        for (f, patch) in c.patches.iter().enumerate() {
            for i in 0..lat.m {
                for j in 0..lat.m {
                    worst = worst.max(
                        patch
                            .point(i as f64 / (lat.m - 1) as f64, j as f64 / (lat.m - 1) as f64)
                            .distance(limit[lat.grids[f][i * lat.m + j] as usize]),
                    );
                }
            }
        }
        assert!((c.max_dev - worst).abs() < 1e-12);
        for f in 0..cube.faces.len() {
            if Walker::new(&cube, &adj).regular_face(f) {
                assert_eq!(
                    c.patches[f].control_points,
                    before.patches[f].control_points
                );
            }
        }
    }

    #[test]
    fn g2_reports_topology_seams_and_partial_coverage() {
        let sub = pinned_grid(4, 3);
        let out = cook(
            "surface.mesh_to_g2_surface",
            &[some(&sub.to_geometry())],
            &[],
        )
        .unwrap();
        let r = report_of(&out[2]);
        assert_eq!(r.metrics["expected_seams"].value, 17.0);
        assert_eq!(r.metrics["g2_fraction"].value, 1.0);
        assert!(r.metrics["curvature_jump_max_inv_mm"].value < 1e-8);
        let cube =
            Subd::from_geometry(&Geometry::make_box(DVec3::splat(100.0)), 1e-9).refine_levels(2);
        let out = cook(
            "surface.mesh_to_g2_surface",
            &[some(&cube.to_geometry())],
            &[],
        )
        .unwrap();
        let r = report_of(&out[2]);
        assert_eq!(r.metrics["expected_seams"].value, 192.0);
        assert!(r.metrics["g2_fraction"].value > 0.0);
        assert!(r.metrics["g2_fraction"].value < 1.0);
        assert!(r.metrics["dev_max_mm"].value > 0.0);
        assert!(
            !r.passes(),
            "partial G2 must fail the requested continuity gate"
        );
        println!(
            "cube g2={} g1={} dev={} curvature_jump={}",
            r.metrics["g2_fraction"].value,
            r.metrics["g1_fraction"].value,
            r.metrics["dev_max_mm"].value,
            r.metrics["curvature_jump_max_inv_mm"].value
        );
    }

    #[test]
    fn packed_regular_grid_is_one_exact_patch() {
        // spec N1: 4 x 3 regular grid, packed -> 1 surface, 7 x 6 control points
        let sub = pinned_grid(4, 3);
        let diag = 50.0;
        let c = convert(&sub, true, 2, 1_000_000).unwrap();
        assert_eq!(c.patches.len(), 1);
        let p = &c.patches[0];
        assert_eq!((p.degree_u, p.degree_v, p.nu, p.nv), (3, 3, 7, 6));
        assert_eq!(p.control_points.len(), 42);
        assert!(c.max_dev <= 1e-9 * diag, "dev {}", c.max_dev);
        assert_eq!(c.irregular_faces, 0);
        let un = convert(&sub, false, 2, 1_000_000).unwrap();
        assert_eq!(un.patches.len(), 12);
        assert!(un.patches.iter().all(|p| p.control_points.len() == 16));
        assert!(un.max_dev <= 1e-9 * diag, "dev {}", un.max_dev);
        PatchSet::new(un.patches).to_json();
    }

    #[test]
    fn patch_equals_the_deeply_refined_limit() {
        let sub = pinned_grid(5, 5);
        let c = convert(&sub, true, 2, 1_000_000).unwrap();
        let fine = sub.refine_levels(5);
        let lim = fine.limit_positions();
        let ps = PatchSet::new(c.patches);
        let mut g = fine.to_geometry();
        g.points[..lim.len()].copy_from_slice(&lim);
        let bvh = TriBvh::from_geometry(&g, Some(1e-9));
        let s0 = &ps.patches[0].surface;
        let mut worst: f64 = 0.0;
        for k in 0..30 {
            let (u, v) = (k as f64 / 29.0, ((k * 7) % 30) as f64 / 29.0);
            worst = worst.max(bvh.closest(s0.point(u, v)).unwrap().dist);
        }
        assert!(worst < 5e-3, "worst {worst}");
    }

    #[test]
    fn cube_cage_with_extraordinary_vertices_stays_watertight() {
        // spec N2: each cube face subdivided 4 x 4 -> valence-3 vertices at 8 corners
        let cube =
            Subd::from_geometry(&Geometry::make_box(DVec3::splat(100.0)), 1e-9).refine_levels(2);
        let c = convert(&cube, false, 2, 3_000_000).unwrap();
        assert_eq!(c.patches.len(), 96);
        assert_eq!(c.extraordinary_vertices, 8);
        assert_eq!(c.irregular_faces, 24, "three faces at each of 8 corners");
        let diag = 100.0 * 3f64.sqrt();
        assert!(
            c.max_dev <= 1e-3 * diag,
            "dev {} vs {}",
            c.max_dev,
            1e-3 * diag
        );
        let ps = PatchSet::new(c.patches);
        let seams = find_seams(&ps, 1e-6, 8);
        assert!(seams.is_closed(), "free edges {}", seams.free_edges.len());
        assert!(seams.max_gap() < 1e-6, "gap {}", seams.max_gap());
        println!(
            "cube g1 {:.3} deg, dev {:.5}",
            seams.max_g1_deg(),
            c.max_dev
        );
    }

    #[test]
    fn packed_cube_merges_regular_runs() {
        let cube =
            Subd::from_geometry(&Geometry::make_box(DVec3::splat(100.0)), 1e-9).refine_levels(3);
        let c = convert(&cube, true, 2, 3_000_000).unwrap();
        assert!(c.patches.len() < 384, "{} patches", c.patches.len());
        assert!(c.max_dev < 0.2, "dev {}", c.max_dev);
        let seams = find_seams(&PatchSet::new(c.patches), 1e-6, 8);
        assert!(seams.max_gap() < 1e-6);
    }

    #[test]
    fn open_grid_through_the_nodes_gives_one_packed_patch() {
        let g = grid(4, 3, 40.0, 30.0, |x, y| 5.0 * (x / 20.0).sin() + 0.05 * y);
        let cage = cook("surface.mesh_to_subd", &[some(&g)], &[]).unwrap();
        let rep = report_of(&cage[1]);
        assert_eq!(rep.metrics["corner_vertices"].value, 4.0);
        let out = cook("surface.subd_to_nurbs", &[Some(cage[0].clone())], &[]).unwrap();
        let r = report_of(&out[2]);
        assert_eq!(r.metrics["patch_count"].value, 1.0);
        assert_eq!(r.metrics["control_points"].value, 42.0);
        assert!(r.metrics["dev_max_mm"].value < 1e-6);
        assert!(r.passes());
    }

    #[test]
    fn nodes_run_end_to_end() {
        let cube = Geometry::make_box(DVec3::splat(100.0));
        let out = cook(
            "surface.mesh_to_subd",
            &[some(&cube)],
            &[("creases", b(false))],
        )
        .unwrap();
        let cage = geo_of(&out[0]);
        let out = cook(
            "surface.subd_to_nurbs",
            &[some(&cage)],
            &[("refine_levels", i(2)), ("faces", s("unpacked"))],
        )
        .unwrap();
        let rep = report_of(&out[2]);
        assert_eq!(rep.metrics["patch_count"].value, 96.0);
        assert_eq!(rep.metrics["free_boundary_edges"].value, 0.0);
        let patches = out[0].clone();
        let dev = cook(
            "surface.subd_deviation",
            &[some(&cage), Some(patches)],
            &[("tol_mm", f(1.0)), ("levels", i(3))],
        )
        .unwrap();
        let drep = report_of(&dev[1]);
        assert!(drep.passes(), "p95 {}", drep.metrics["dev_p95_mm"].value);
        let refined = cook(
            "surface.subd_refine",
            &[some(&cage)],
            &[("levels", i(2)), ("output", s("level"))],
        )
        .unwrap();
        let cage2 = geo_of(&refined[0]);
        let g2 = cook("surface.mesh_to_g2_surface", &[some(&cage2)], &[]).unwrap();
        assert!(report_of(&g2[2]).metrics.contains_key("g2_fraction"));
    }
}

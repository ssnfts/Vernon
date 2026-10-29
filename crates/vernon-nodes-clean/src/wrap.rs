//! Watertight wrap: distance field on a sparse narrow band, outside
//! flood fill, iso-surface extraction by marching tetrahedra, then optional
//! uniform remeshing, smoothing and polygon reduction.
//!
//! Closed, consistently oriented meshes give an exact signed field (sign
//! from the closest-feature pseudo-normal). Open meshes, triangle soups and
//! point clouds use an unsigned field: everything within a small radius of
//! the input is blocked, the outside is flooded from the grid corner and
//! whatever is not reached counts as inside, which seals gaps narrower than
//! twice that radius.

use std::collections::HashMap;

use glam::DVec3;
use rayon::prelude::*;
use vernon_core::{AttribArray, Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::decimate::{qem_decimate, QemOptions};
use crate::kdtree::KdTree;
use crate::meshedit::{edge_stats, signed_volume, triangles_to_geometry, weld_triangles};
use crate::meshfix::{close_small_holes, smooth_welded};
use crate::remesh::{isotropic_remesh, RemeshOptions};
use crate::trimesh::{SignMode, TriMesh};
use crate::util::{add, ensure_metric, pf, pi, SPEC_MESH};

pub fn register(reg: &mut Registry) {
    add(reg, wrap_info, wrap_cook);
    add(reg, alpha_info, alpha_cook);
}

const TS: usize = 8;
const TN: usize = TS * TS * TS;

// -------------------------------------------------------------------- bits

struct Bits(Vec<u64>);

impl Bits {
    fn new(n: usize) -> Self {
        Bits(vec![0; n.div_ceil(64)])
    }
    #[inline]
    fn get(&self, i: usize) -> bool {
        (self.0[i >> 6] >> (i & 63)) & 1 == 1
    }
    #[inline]
    fn set(&mut self, i: usize) {
        self.0[i >> 6] |= 1u64 << (i & 63);
    }
}

// -------------------------------------------------------------------- grid

#[derive(Clone, Copy)]
struct Grid {
    origin: DVec3,
    h: f64,
    n: [usize; 3],
    t: [usize; 3],
}

impl Grid {
    #[inline]
    fn id(&self, i: usize, j: usize, k: usize) -> usize {
        (k * self.n[1] + j) * self.n[0] + i
    }
    #[inline]
    fn ijk(&self, id: usize) -> (usize, usize, usize) {
        let i = id % self.n[0];
        let r = id / self.n[0];
        (i, r % self.n[1], r / self.n[1])
    }
    #[inline]
    fn pos(&self, i: usize, j: usize, k: usize) -> DVec3 {
        self.origin + DVec3::new(i as f64, j as f64, k as f64) * self.h
    }
    #[inline]
    fn tile_of(&self, i: usize, j: usize, k: usize) -> usize {
        ((k / TS) * self.t[1] + j / TS) * self.t[0] + i / TS
    }
}

// ------------------------------------------------------------------ source

enum Src<'a> {
    Mesh { mesh: &'a TriMesh, signed: bool, flip: bool },
    Points { tree: &'a KdTree },
}

impl Src<'_> {
    /// Unsigned distance below `r2.sqrt()` and, for signed meshes, whether
    /// the query lies inside.
    fn query(&self, p: DVec3, r2: f64) -> Option<(f64, bool)> {
        match self {
            Src::Mesh { mesh, signed, flip } => {
                let mode = if *signed { SignMode::Pseudo } else { SignMode::None };
                let h = mesh.closest(p, r2, mode)?;
                let inside = *signed && if *flip { h.signed > 0.0 } else { h.signed < 0.0 };
                Some((h.dist, inside))
            }
            Src::Points { tree } => tree.nearest_within(p, r2).map(|(_, d2)| (d2.sqrt(), false)),
        }
    }
}

// -------------------------------------------------------------------- pool

struct Pool {
    tile_idx: Vec<u32>,
    tiles: Vec<[usize; 3]>,
    dist: Vec<f32>,
    inside: Vec<u8>,
}

impl Pool {
    #[inline]
    fn slot(&self, g: &Grid, i: usize, j: usize, k: usize) -> Option<usize> {
        let t = self.tile_idx[g.tile_of(i, j, k)];
        if t == u32::MAX {
            None
        } else {
            Some(t as usize * TN + ((k & 7) * TS + (j & 7)) * TS + (i & 7))
        }
    }
}

struct FieldCtx<'a> {
    grid: Grid,
    pool: &'a Pool,
    signed: bool,
    rho: f64,
    offset: f64,
    far: f32,
    reached: &'a Bits,
    comp_inside: &'a Bits,
}

impl FieldCtx<'_> {
    /// Field value; negative inside the wrapped body.
    fn s_at(&self, i: usize, j: usize, k: usize) -> f32 {
        let id = self.grid.id(i, j, k);
        let slot = self.pool.slot(&self.grid, i, j, k);
        let d = slot.map_or(f32::INFINITY, |s| self.pool.dist[s]);
        let v = if self.signed {
            if d.is_finite() {
                let inside = self.pool.inside[slot.unwrap()] == 1;
                if inside {
                    -d - self.offset as f32
                } else {
                    d - self.offset as f32
                }
            } else if self.comp_inside.get(id) {
                -self.far
            } else {
                self.far
            }
        } else if d.is_finite() {
            let rho = self.rho as f32;
            if self.reached.get(id) {
                d - rho
            } else {
                -(d - rho).abs()
            }
        } else if self.reached.get(id) {
            self.far
        } else {
            -self.far
        };
        if v == 0.0 {
            1e-6 * self.grid.h as f32
        } else {
            v
        }
    }
}

fn flood(grid: &Grid, start: usize, visited: &mut Bits, barrier: &dyn Fn(usize, usize, usize) -> bool, mut on_visit: impl FnMut(usize)) {
    let mut stack: Vec<u32> = vec![start as u32];
    visited.set(start);
    while let Some(id) = stack.pop() {
        let id = id as usize;
        on_visit(id);
        let (i, j, k) = grid.ijk(id);
        let mut try_push = |ni: usize, nj: usize, nk: usize| {
            let nid = grid.id(ni, nj, nk);
            if !visited.get(nid) && !barrier(ni, nj, nk) {
                visited.set(nid);
                stack.push(nid as u32);
            }
        };
        if i > 0 {
            try_push(i - 1, j, k);
        }
        if i + 1 < grid.n[0] {
            try_push(i + 1, j, k);
        }
        if j > 0 {
            try_push(i, j - 1, k);
        }
        if j + 1 < grid.n[1] {
            try_push(i, j + 1, k);
        }
        if k > 0 {
            try_push(i, j, k - 1);
        }
        if k + 1 < grid.n[2] {
            try_push(i, j, k + 1);
        }
    }
}

// -------------------------------------------------------------- extraction

#[derive(Clone, Copy)]
struct EdgeRef {
    a: u32,
    b: u32,
    t: f32,
}

type TriRec = [EdgeRef; 3];

/// Corner indices (bit 0 = x, bit 1 = y, bit 2 = z) of the six tetrahedra
/// that share the cube's main diagonal.
const TETS: [[usize; 4]; 6] = [[0, 1, 3, 7], [0, 1, 5, 7], [0, 2, 3, 7], [0, 2, 6, 7], [0, 4, 5, 7], [0, 4, 6, 7]];

fn tet_triangles(nodes: [(u32, DVec3, f32); 4], out: &mut Vec<TriRec>) {
    let ins: Vec<usize> = (0..4).filter(|&i| nodes[i].2 < 0.0).collect();
    let outs: Vec<usize> = (0..4).filter(|&i| nodes[i].2 >= 0.0).collect();
    if ins.is_empty() || outs.is_empty() {
        return;
    }
    // x is the inside node, y the outside node of the crossed edge
    let edge = |x: usize, y: usize| -> (EdgeRef, DVec3) {
        let (na, nb) = (nodes[x], nodes[y]);
        let t = na.2 / (na.2 - nb.2);
        let p = na.1 + (nb.1 - na.1) * t as f64;
        let er = if na.0 < nb.0 { EdgeRef { a: na.0, b: nb.0, t } } else { EdgeRef { a: nb.0, b: na.0, t: 1.0 - t } };
        (er, p)
    };
    let centroid = |idx: &[usize]| idx.iter().map(|&i| nodes[i].1).sum::<DVec3>() / idx.len() as f64;
    let mut emit = |e: [(EdgeRef, DVec3); 3], dir: DVec3| {
        let n = (e[1].1 - e[0].1).cross(e[2].1 - e[0].1);
        if n.dot(dir) >= 0.0 {
            out.push([e[0].0, e[1].0, e[2].0]);
        } else {
            out.push([e[0].0, e[2].0, e[1].0]);
        }
    };
    match (ins.len(), outs.len()) {
        (1, 3) => {
            let l = ins[0];
            let dir = centroid(&outs) - nodes[l].1;
            emit([edge(l, outs[0]), edge(l, outs[1]), edge(l, outs[2])], dir);
        }
        (3, 1) => {
            let l = outs[0];
            let dir = nodes[l].1 - centroid(&ins);
            emit([edge(ins[0], l), edge(ins[1], l), edge(ins[2], l)], dir);
        }
        (2, 2) => {
            let (a, b, c, d) = (ins[0], ins[1], outs[0], outs[1]);
            let dir = centroid(&outs) - centroid(&ins);
            let (ac, ad, bd, bc) = (edge(a, c), edge(a, d), edge(b, d), edge(b, c));
            emit([ac, ad, bd], dir);
            emit([ac, bd, bc], dir);
        }
        _ => {}
    }
}

// ------------------------------------------------------------------ driver

/// Parameters of the wrap pipeline.
#[derive(Clone, Debug)]
pub struct WrapParams {
    /// Voxel size; 0 = bounding-box diagonal / 100.
    pub voxel: f64,
    pub offset: f64,
    pub smooth_iterations: usize,
    pub shrink_compensate: bool,
    pub polygon_optimization: f64,
    pub remesh_iterations: usize,
    pub inflate_points: bool,
    pub fill_holes: bool,
    pub max_cells: usize,
}

pub struct WrapOutput {
    pub pos: Vec<DVec3>,
    pub tris: Vec<[u32; 3]>,
    pub voxel: f64,
    pub rho: f64,
    pub dims: [usize; 3],
    pub band_nodes: usize,
    pub signed: bool,
    pub from_points: bool,
    pub notes: Vec<(Severity, String)>,
    /// Expected distance of the surface from the input.
    pub expected_dist: f64,
}

fn components(n_verts: usize, tris: &[[u32; 3]]) -> usize {
    let mut parent: Vec<u32> = (0..n_verts as u32).collect();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    let mut used = vec![false; n_verts];
    for t in tris {
        for k in 0..3 {
            used[t[k] as usize] = true;
            let (a, b) = (find(&mut parent, t[0]), find(&mut parent, t[k]));
            if a != b {
                parent[a as usize] = b;
            }
        }
    }
    let mut roots = std::collections::HashSet::new();
    for v in 0..n_verts as u32 {
        if used[v as usize] {
            roots.insert(find(&mut parent, v));
        }
    }
    roots.len()
}

pub fn wrap_geometry(g: &Geometry, prm: &WrapParams) -> Result<WrapOutput, String> {
    let mut notes: Vec<(Severity, String)> = Vec::new();
    let w = weld_triangles(g, 0.0);
    let have_faces = !w.tris.is_empty();
    let from_points = prm.inflate_points || !have_faces;
    if !have_faces && !g.points.is_empty() {
        notes.push((Severity::Info, "input has no faces; every point is inflated into a ball".to_string()));
    } else if from_points && have_faces {
        notes.push((Severity::Info, "faces are ignored; every vertex is inflated into a ball".to_string()));
    }
    if g.points.is_empty() {
        return Err("input is empty".to_string());
    }

    let pos = w.pos.clone();
    let mut tris = w.tris.clone();
    if !from_points && prm.fill_holes {
        let (added, filled) = close_small_holes(&pos, &tris, 600);
        if filled > 0 {
            notes.push((Severity::Info, format!("closed {filled} hole(s) of the input before wrapping")));
        }
        tris.extend(added);
    }
    let tree;
    let mesh;
    let src;
    let bbox_pts: &[DVec3];
    if from_points {
        tree = KdTree::new(&g.points);
        src = Src::Points { tree: &tree };
        bbox_pts = &g.points;
        mesh = None;
    } else {
        let m = TriMesh::new(pos.clone(), tris.clone(), vec![0; tris.len()]);
        let signed = m.topo.is_closed() && m.topo.signed_volume.abs() > 1e-12;
        let flip = m.topo.signed_volume < 0.0;
        mesh = Some(m);
        src = Src::Mesh { mesh: mesh.as_ref().unwrap(), signed, flip };
        bbox_pts = &pos;
    }
    let signed = matches!(src, Src::Mesh { signed: true, .. });
    let mut bb = vernon_core::Aabb::empty();
    for p in bbox_pts {
        bb.grow(*p);
    }
    let diag = bb.size().length();
    let h = if prm.voxel > 0.0 { prm.voxel } else { diag / 100.0 };
    if h <= 0.0 || !h.is_finite() {
        return Err("cannot derive a voxel size from the input; set the edge length".to_string());
    }
    let rho = prm.offset.max(0.6 * h);
    if !signed && prm.offset < 0.0 {
        notes.push((Severity::Info, "negative offsets apply only to closed, consistently oriented meshes; using the minimum wrap radius".to_string()));
    }
    let band = if signed { prm.offset.abs() } else { rho } + 2.0 * h;
    let pad = band + 2.0 * h;
    let origin = bb.min - DVec3::splat(pad);
    let size = bb.size() + DVec3::splat(2.0 * pad);
    let n = [(size.x / h).ceil() as usize + 1, (size.y / h).ceil() as usize + 1, (size.z / h).ceil() as usize + 1];
    let total = n[0].saturating_mul(n[1]).saturating_mul(n[2]);
    if total > prm.max_cells {
        return Err(format!(
            "the grid would have {total} nodes (limit {}); raise the edge length or the grid limit",
            prm.max_cells
        ));
    }
    let grid = Grid { origin, h, n, t: [n[0].div_ceil(TS), n[1].div_ceil(TS), n[2].div_ceil(TS)] };

    // candidate tiles: every tile within the band of some primitive
    let mut mark = vec![false; grid.t[0] * grid.t[1] * grid.t[2]];
    let mut mark_box = |lo: DVec3, hi: DVec3| {
        let clamp = |v: f64, axis: usize| -> usize {
            let x = ((v - origin[axis]) / h).floor();
            (x.max(0.0) as usize).min(n[axis] - 1) / TS
        };
        let (a, b) = ((clamp(lo.x - band, 0), clamp(lo.y - band, 1), clamp(lo.z - band, 2)), (clamp(hi.x + band, 0), clamp(hi.y + band, 1), clamp(hi.z + band, 2)));
        for tk in a.2..=b.2 {
            for tj in a.1..=b.1 {
                for ti in a.0..=b.0 {
                    mark[(tk * grid.t[1] + tj) * grid.t[0] + ti] = true;
                }
            }
        }
    };
    if from_points {
        for p in &g.points {
            mark_box(*p, *p);
        }
    } else {
        for t in &tris {
            let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
            mark_box(a.min(b).min(c), a.max(b).max(c));
        }
    }
    let mut tile_idx = vec![u32::MAX; mark.len()];
    let mut tiles: Vec<[usize; 3]> = Vec::new();
    for tk in 0..grid.t[2] {
        for tj in 0..grid.t[1] {
            for ti in 0..grid.t[0] {
                let id = (tk * grid.t[1] + tj) * grid.t[0] + ti;
                if mark[id] {
                    tile_idx[id] = tiles.len() as u32;
                    tiles.push([ti, tj, tk]);
                }
            }
        }
    }
    let mut dist = vec![f32::INFINITY; tiles.len() * TN];
    let mut inside = vec![0u8; tiles.len() * TN];
    let r2 = band * band;
    dist.par_chunks_mut(TN).zip(inside.par_chunks_mut(TN)).enumerate().for_each(|(ti, (dc, sc))| {
        let tc = tiles[ti];
        for lk in 0..TS {
            for lj in 0..TS {
                for li in 0..TS {
                    let (i, j, k) = (tc[0] * TS + li, tc[1] * TS + lj, tc[2] * TS + lk);
                    if i >= n[0] || j >= n[1] || k >= n[2] {
                        continue;
                    }
                    if let Some((d, ins)) = src.query(grid.pos(i, j, k), r2) {
                        let o = (lk * TS + lj) * TS + li;
                        dc[o] = d as f32;
                        sc[o] = ins as u8;
                    }
                }
            }
        }
    });
    let pool = Pool { tile_idx, tiles, dist, inside };

    // classification of the free space
    let mut reached = Bits::new(total);
    let mut comp_inside = Bits::new(total);
    {
        let barrier = |i: usize, j: usize, k: usize| -> bool {
            match pool.slot(&grid, i, j, k) {
                None => false,
                Some(s) => {
                    let d = pool.dist[s];
                    if signed {
                        d.is_finite()
                    } else {
                        d as f64 <= rho
                    }
                }
            }
        };
        if barrier(0, 0, 0) {
            return Err("internal error: the grid corner is not in free space".to_string());
        }
        flood(&grid, 0, &mut reached, &barrier, |_| {});
        if signed {
            let mesh = mesh.as_ref().unwrap();
            let flip = mesh.topo.signed_volume < 0.0;
            let mut seen = Bits::new(total);
            for id in 0..total {
                if reached.get(id) || seen.get(id) {
                    continue;
                }
                let (i, j, k) = grid.ijk(id);
                if barrier(i, j, k) {
                    continue;
                }
                let p = grid.pos(i, j, k);
                let inside_solid = mesh.closest(p, f64::INFINITY, SignMode::Pseudo).is_some_and(|h| if flip { h.signed > 0.0 } else { h.signed < 0.0 });
                flood(&grid, id, &mut seen, &barrier, |v| {
                    if inside_solid {
                        comp_inside.set(v);
                    }
                });
            }
        }
    }
    let far = (band + h) as f32;
    let fc = FieldCtx { grid, pool: &pool, signed, rho, offset: prm.offset, far, reached: &reached, comp_inside: &comp_inside };

    // marching tetrahedra over the candidate tiles
    let recs: Vec<Vec<TriRec>> = pool
        .tiles
        .par_iter()
        .map(|tc| {
            let mut out: Vec<TriRec> = Vec::new();
            for lk in 0..TS {
                let k = tc[2] * TS + lk;
                if k + 1 >= n[2] {
                    break;
                }
                for lj in 0..TS {
                    let j = tc[1] * TS + lj;
                    if j + 1 >= n[1] {
                        break;
                    }
                    for li in 0..TS {
                        let i = tc[0] * TS + li;
                        if i + 1 >= n[0] {
                            break;
                        }
                        let mut s = [0f32; 8];
                        let (mut neg, mut nonneg) = (false, false);
                        for (c, sv) in s.iter_mut().enumerate() {
                            *sv = fc.s_at(i + (c & 1), j + ((c >> 1) & 1), k + (c >> 2));
                            if *sv < 0.0 {
                                neg = true;
                            } else {
                                nonneg = true;
                            }
                        }
                        if !(neg && nonneg) {
                            continue;
                        }
                        let corner = |c: usize| -> (u32, DVec3, f32) {
                            let (ci, cj, ck) = (i + (c & 1), j + ((c >> 1) & 1), k + (c >> 2));
                            (grid.id(ci, cj, ck) as u32, grid.pos(ci, cj, ck), s[c])
                        };
                        for tet in TETS {
                            tet_triangles([corner(tet[0]), corner(tet[1]), corner(tet[2]), corner(tet[3])], &mut out);
                        }
                    }
                }
            }
            out
        })
        .collect();

    let mut vmap: HashMap<(u32, u32), u32> = HashMap::new();
    let mut opos: Vec<DVec3> = Vec::new();
    let mut otris: Vec<[u32; 3]> = Vec::new();
    for rec in recs.iter().flatten() {
        let mut idx = [0u32; 3];
        for k in 0..3 {
            let e = rec[k];
            idx[k] = *vmap.entry((e.a, e.b)).or_insert_with(|| {
                let (ai, aj, ak) = grid.ijk(e.a as usize);
                let (bi, bj, bk) = grid.ijk(e.b as usize);
                let (pa, pb) = (grid.pos(ai, aj, ak), grid.pos(bi, bj, bk));
                opos.push(pa + (pb - pa) * e.t as f64);
                (opos.len() - 1) as u32
            });
        }
        otris.push(idx);
    }
    drop(recs);
    drop(vmap);
    if otris.is_empty() {
        return Err("the wrap produced no surface; check the offset and edge length".to_string());
    }
    let band_nodes = pool.tiles.len() * TN;

    // post-processing
    if prm.remesh_iterations > 0 {
        let (p, t) = isotropic_remesh(opos, otris, &RemeshOptions { target: h, iterations: prm.remesh_iterations, cos_crease: -1.0, project: true });
        opos = p;
        otris = t;
    }
    if prm.smooth_iterations > 0 {
        let fixed = vec![false; opos.len()];
        opos = smooth_welded(&opos, &otris, &fixed, prm.smooth_iterations, prm.shrink_compensate, 0.5, -0.53);
    }
    if prm.polygon_optimization > 0.0 {
        let target = ((otris.len() as f64) * (1.0 - 0.9 * prm.polygon_optimization / 100.0)).round() as usize;
        let c = bb.center();
        let centred: Vec<DVec3> = opos.iter().map(|p| *p - c).collect();
        let out = qem_decimate(
            centred,
            otris,
            &QemOptions {
                target_faces: target.max(4),
                max_cost: (0.25 * h) * (0.25 * h),
                boundary_weight: 100.0,
                crease: Some((30f64.to_radians(), 20.0)),
                min_normal_cos: 0.2,
            },
        );
        let (p, t, _) = out.mesh.compact();
        opos = p.into_iter().map(|q| q + c).collect();
        otris = t;
    }
    Ok(WrapOutput {
        pos: opos,
        tris: otris,
        voxel: h,
        rho,
        dims: n,
        band_nodes,
        signed,
        from_points,
        notes,
        expected_dist: if signed { prm.offset.abs() } else { rho },
    })
}

// -------------------------------------------------------------------- nodes

fn wrap_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.watertight_wrap", "Watertight Wrap", Stage::Clean)
        .input(PortSpec::new("geometry", DataKind::Geometry))
        .input(PortSpec::optional("extra", DataKind::Geometry))
        .output(PortSpec::new("geometry", DataKind::Geometry))
        .output(PortSpec::new("report", DataKind::Report))
        .param(
            ParamSpec::float("target_edge_length", "Edge length", 0.0, 0.0, 10000.0)
                .unit("mm")
                .help("Grid cell size and wanted edge length of the result. 0 = one hundredth of the input size."),
        )
        .param(
            ParamSpec::float("offset", "Offset", 0.0, -1000.0, 1000.0)
                .unit("mm")
                .help("Distance of the shell from the input. Positive inflates, negative shrinks (closed meshes only)."),
        )
        .param(ParamSpec::int("smooth_iterations", "Smoothing", 0, 0, 1000).help("Smoothing passes on the result. More passes remove more faces and deflate the shell."))
        .param(ParamSpec::bool("shrink_compensate", "Compensate shrink", false).help("Alternate shrink and inflate smoothing passes so the shell keeps its size."))
        .param(ParamSpec::float("polygon_optimization", "Polygon optimization", 0.0, 0.0, 100.0).unit("%").help("Collapse edges in flat areas to cut the face count; hard edges are kept."))
        .param(ParamSpec::int("remesh_iterations", "Remesh rounds", 2, 0, 20).help("Rounds of uniform remeshing toward the edge length. 0 keeps the raw grid surface."))
        .param(ParamSpec::bool("inflate_points", "Inflate vertices", false).help("Build the shell from the vertices and points only, ignoring faces. Always on when the input has no faces."))
        .param(ParamSpec::bool("fill_input_holes", "Fill input holes", false).help("Close holes in the input mesh first. May also close intentional gaps."))
        .param(ParamSpec::bool("transfer_colors", "Transfer colours", false).help("Copy vertex colours (Cd) from the nearest input point."))
        .param(ParamSpec::int("max_cells", "Grid limit", 40_000_000, 1000, 400_000_000).help("Largest number of grid nodes allowed; larger requests fail instead of using the memory."))
        .help("Wraps meshes, scan fragments and point clouds into one closed shell. Gaps narrower than about twice the minimum wrap radius are sealed.")
        .emulates("shrink-wrap: distance-field wrap with iso-surface extraction, uniform remeshing and edge-collapse optimisation")
        .spec(SPEC_MESH)
        .implemented()
}

fn alpha_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.alpha_wrap", "Conservative Wrap", Stage::Clean)
        .input(PortSpec::new("geometry", DataKind::Geometry))
        .output(PortSpec::new("geometry", DataKind::Geometry))
        .output(PortSpec::new("report", DataKind::Report))
        .param(
            ParamSpec::float("alpha", "Alpha", 0.0, 0.0, 10000.0)
                .unit("mm")
                .help("Smallest gap the wrap may cross. 0 = one twentieth of the input size."),
        )
        .param(
            ParamSpec::float("offset", "Offset", 0.0, 0.0, 1000.0)
                .unit("mm")
                .help("Minimum distance from the input. The certified vertex band extends to offset plus alpha. 0 = alpha / 30."),
        )
        .help("Builds a conservative closed wrap of a mesh, triangle soup or point cloud. Checks every output vertex against the requested distance band and verifies closed edges and vertex links. Alpha controls gap bridging; some larger gaps may also close. Excessive or unresolved requests return an error.")
        .emulates("guarded voxel offset wrapping with Hermite dual contouring and constrained projection")
        .spec(SPEC_MESH)
        .implemented()
}

fn read_wrap_inputs(inp: &[Option<Data>]) -> CookResult<Geometry> {
    let mut g = (*inputs::geometry(inp, 0)?).clone();
    if let Some(e) = inputs::opt_geometry(inp, 1) {
        g.merge(&e);
    }
    Ok(g)
}

fn wrap_report(title: &str, g: &Geometry, out: &WrapOutput, geo: &Geometry) -> Report {
    let mut rep = Report::new(title);
    let st = edge_stats(&out.tris);
    let comps = components(out.pos.len(), &out.tris);
    rep.metric("faces", Metric::new(out.tris.len() as f64, "faces"));
    rep.metric("vertices", Metric::new(out.pos.len() as f64, "points"));
    rep.metric("open_edges", Metric::new(st.open_edges as f64, "edges").with_limit(0.0, true));
    rep.metric("nonmanifold_edges", Metric::new(st.nonmanifold_edges as f64, "edges").with_limit(0.0, true));
    rep.metric("components", Metric::new(comps as f64, "shells"));
    rep.metric("volume", Metric::new(signed_volume(&out.pos, &out.tris), "mm3"));
    rep.metric("surface_area", Metric::new(geo.surface_area(), "mm2"));
    rep.metric("voxel_size", Metric::new(out.voxel, "mm"));
    rep.metric("minimum_wrap_radius", Metric::new(out.rho, "mm"));
    rep.metric("grid_nodes", Metric::new((out.dims[0] * out.dims[1] * out.dims[2]) as f64, "nodes"));
    rep.metric("band_nodes", Metric::new(out.band_nodes as f64, "nodes"));
    if !out.signed && !out.from_points {
        rep.note(Severity::Info, format!("input is not a closed oriented mesh; unsigned field used, gaps up to {:.3} mm are sealed", 2.0 * out.rho));
    }
    for (sev, text) in &out.notes {
        rep.note(*sev, text.clone());
    }
    if comps > 1 {
        rep.note(Severity::Warning, format!("the result has {comps} separate shells"));
    }
    if st.open_edges > 0 || st.nonmanifold_edges > 0 {
        rep.note(Severity::Error, "the result is not a closed manifold surface");
    }
    // how far the surface really is from the input
    if !out.pos.is_empty() {
        let step = (out.pos.len() / 100_000).max(1);
        let sample: Vec<usize> = (0..out.pos.len()).step_by(step).collect();
        let w = weld_triangles(g, 0.0);
        let errs: Vec<f64> = if out.from_points {
            let tree = KdTree::new(&g.points);
            sample.par_iter().map(|&i| tree.nearest(out.pos[i]).map_or(0.0, |(_, d2)| (d2.sqrt() - out.expected_dist).abs())).collect()
        } else {
            let m = TriMesh::new(w.pos.clone(), w.tris.clone(), vec![0; w.tris.len()]);
            sample.par_iter().map(|&i| m.closest(out.pos[i], f64::INFINITY, SignMode::None).map_or(0.0, |h| (h.dist - out.expected_dist).abs())).collect()
        };
        let max = errs.iter().cloned().fold(0.0, f64::max);
        let mean = errs.iter().sum::<f64>() / errs.len().max(1) as f64;
        rep.metric("offset_error_max", Metric::new(max, "mm"));
        rep.metric("offset_error_mean", Metric::new(mean, "mm"));
    }
    rep
}

fn build_output(g: &Geometry, out: &WrapOutput, transfer_colors: bool) -> Geometry {
    let mut geo = triangles_to_geometry(out.pos.clone(), &out.tris);
    geo.detail_attribs = g.detail_attribs.clone();
    geo.compute_normals();
    if transfer_colors {
        if let Ok(cd) = g.point_attribs.vec3s("Cd") {
            if cd.len() == g.points.len() && !g.points.is_empty() {
                let tree = KdTree::new(&g.points);
                let col: Vec<DVec3> = out.pos.iter().map(|p| tree.nearest(*p).map_or(DVec3::ONE, |(i, _)| cd[i as usize])).collect();
                geo.point_attribs.set("Cd", AttribArray::Vec3(col));
            }
        }
    }
    geo
}

fn wrap_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = read_wrap_inputs(inp)?;
    ensure_metric(ctx, &g)?;
    let prm = WrapParams {
        voxel: pf(p, "target_edge_length", 0.0, 10000.0)?,
        offset: pf(p, "offset", -1000.0, 1000.0)?,
        smooth_iterations: pi(p, "smooth_iterations", 0, 1000)? as usize,
        shrink_compensate: p.bool("shrink_compensate")?,
        polygon_optimization: pf(p, "polygon_optimization", 0.0, 100.0)?,
        remesh_iterations: pi(p, "remesh_iterations", 0, 20)? as usize,
        inflate_points: p.bool("inflate_points")?,
        fill_holes: p.bool("fill_input_holes")?,
        max_cells: pi(p, "max_cells", 1000, 400_000_000)? as usize,
    };
    let out = wrap_geometry(&g, &prm).map_err(|e| ctx.fail(e))?;
    let geo = build_output(&g, &out, p.bool("transfer_colors")?);
    let rep = wrap_report("Watertight Wrap", &g, &out, &geo);
    Ok(vec![Data::geometry(geo), Data::report(rep)])
}

fn alpha_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    ensure_metric(ctx, &g)?;
    let params = crate::conservative::ConservativeParams {
        alpha: pf(p, "alpha", 0.0, 10000.0)?,
        offset: pf(p, "offset", 0.0, 1000.0)?,
        max_grid_nodes: 40_000_000,
    };
    let out = crate::conservative::conservative_wrap(&g, &params).map_err(|e| ctx.fail(e))?;
    let m = out.metrics;
    let vertices = out.pos.len();
    let faces = out.tris.len();
    let reference = out.pos[0];
    let volume = out.tris.iter().map(|t| {
        let [a,b,c] = t.map(|i| out.pos[i as usize] - reference);
        a.dot(b.cross(c)) / 6.0
    }).sum::<f64>();
    let mut geo = triangles_to_geometry(out.pos, &out.tris);
    geo.detail_attribs = g.detail_attribs.clone();
    geo.compute_normals();
    let mut rep = Report::new("Conservative Wrap");
    for (name, value, unit) in [
        ("faces", faces as f64, "faces"),
        ("vertices", vertices as f64, "points"),
        ("components", m.components as f64, "shells"),
        ("volume", volume, "mm3"),
        ("surface_area", geo.surface_area(), "mm2"),
        ("alpha", m.alpha, "mm"),
        ("offset", m.offset, "mm"),
        ("voxel_size", m.voxel_size, "mm"),
        ("minimum_wrap_radius", m.barrier_radius, "mm"),
        ("barrier_radius", m.barrier_radius, "mm"),
        ("grid_nodes", m.grid_nodes as f64, "nodes"),
        ("band_nodes", m.band_nodes as f64, "nodes"),
        ("source_triangles", m.source_triangles as f64, "count"),
        ("source_points", m.source_points as f64, "count"),
        ("checked_vertices", m.checked_vertices as f64, "points"),
        ("distance_min_mm", m.distance_min, "mm"),
        ("distance_max_mm", m.distance_max, "mm"),
        ("offset_error_max", m.distance_max - m.offset, "mm"),
        ("offset_error_mean", m.distance_mean - m.offset, "mm"),
        ("projection_moves", m.projection_moves as f64, "count"),
        ("refinement_retries", m.refinement_retries as f64, "count"),
    ] {
        rep.metric(name, Metric::new(value, unit));
    }
    for (name, value) in [
        ("open_edges", m.open_edges),
        ("nonmanifold_edges", m.nonmanifold_edges),
        ("inconsistent_edges", m.inconsistent_edges),
        ("nonmanifold_vertices", m.nonmanifold_vertices),
        ("degenerate_triangles", m.degenerate_triangles),
        ("below_offset_vertices", m.below_offset),
        ("above_upper_bound_vertices", m.above_upper_bound),
    ] {
        rep.metric(name, Metric::new(value as f64, "count").with_limit(0.0, true));
    }
    rep.note(Severity::Info, "Conservative voxel wrap with bounded dual contouring and projection. Alpha sets a gap scale; some larger openings may close. All output vertices were checked. This alternative does not certify global absence of self-intersections.");
    Ok(vec![Data::geometry(geo), Data::report(rep)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use crate::util::Rng;

    fn tris_of(g: &Geometry) -> Vec<[u32; 3]> {
        g.triangles().into_iter().map(|t| t.1).collect()
    }

    fn euler(g: &Geometry) -> i64 {
        let tris = tris_of(g);
        let mut e = std::collections::HashSet::new();
        for t in &tris {
            for k in 0..3 {
                e.insert((t[k].min(t[(k + 1) % 3]), t[k].max(t[(k + 1) % 3])));
            }
        }
        g.points.len() as i64 - e.len() as i64 + tris.len() as i64
    }

    #[test]
    fn closed_sphere_wraps_to_a_closed_sphere() {
        let out = run(wrap_info, wrap_cook, vec![geo(sphere_geo(50.0, 40, 41)), None], &[("target_edge_length", f(2.5))]).unwrap();
        let g = out_geo(&out[0]);
        let r = out_report(&out[1]);
        assert_eq!(r.metrics["open_edges"].value, 0.0);
        assert_eq!(r.metrics["nonmanifold_edges"].value, 0.0);
        assert_eq!(r.metrics["components"].value, 1.0);
        assert_eq!(euler(g), 2);
        for p in &g.points {
            assert!((p.length() - 50.0).abs() <= 2.5, "radius {}", p.length());
        }
        let vol = 4.0 / 3.0 * std::f64::consts::PI * 50.0f64.powi(3);
        assert!((r.metrics["volume"].value - vol).abs() / vol < 0.06, "volume {}", r.metrics["volume"].value);
        assert!(r.metrics["offset_error_max"].value <= 2.5);
        // outward winding
        for t in tris_of(g).iter().step_by(37) {
            let (a, b, c) = (g.points[t[0] as usize], g.points[t[1] as usize], g.points[t[2] as usize]);
            assert!((b - a).cross(c - a).dot(a + b + c) > 0.0);
        }
        assert!(g.validate().is_ok());
    }

    #[test]
    fn offset_inflates_and_shrinks() {
        for (off, expect) in [(5.0, 55.0), (-5.0, 45.0)] {
            let out = run(wrap_info, wrap_cook, vec![geo(sphere_geo(50.0, 40, 41)), None], &[("target_edge_length", f(2.5)), ("offset", f(off))]).unwrap();
            let g = out_geo(&out[0]);
            let mean_r = g.points.iter().map(|p| p.length()).sum::<f64>() / g.points.len() as f64;
            assert!((mean_r - expect).abs() < 1.0, "offset {off}: mean radius {mean_r}");
            assert_eq!(out_report(&out[1]).metrics["open_edges"].value, 0.0);
            assert_eq!(euler(g), 2);
        }
    }

    #[test]
    fn open_plate_becomes_a_thin_closed_shell() {
        let n = 10usize;
        let mut pts = vec![];
        for j in 0..=n {
            for i in 0..=n {
                pts.push(DVec3::new(i as f64 * 10.0, j as f64 * 10.0, 0.0));
            }
        }
        let id = |i: usize, j: usize| (j * (n + 1) + i) as u32;
        let mut faces = vec![];
        for j in 0..n {
            for i in 0..n {
                faces.push(vec![id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
                faces.push(vec![id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
            }
        }
        let plate = Geometry::from_polygons(pts, faces);
        let out = run(wrap_info, wrap_cook, vec![geo(plate), None], &[("target_edge_length", f(4.0))]).unwrap();
        let g = out_geo(&out[0]);
        let r = out_report(&out[1]);
        assert_eq!(r.metrics["open_edges"].value, 0.0);
        assert_eq!(r.metrics["components"].value, 1.0);
        assert_eq!(euler(g), 2);
        let b = g.bounds();
        let rho = r.metrics["minimum_wrap_radius"].value;
        assert!(b.max.z > 0.5 * rho && b.min.z < -0.5 * rho && b.max.z < 2.5 * rho, "z {} {} rho {rho}", b.min.z, b.max.z);
        assert!(r.metrics["volume"].value > 0.0);
        assert!(r.notes.iter().any(|n| n.1.contains("unsigned field")));
    }

    #[test]
    fn a_hole_narrower_than_the_wrap_radius_is_sealed() {
        let mut s = sphere_geo(50.0, 40, 41);
        // drop the polar cap fan: a hole of about 8 mm diameter
        s.prims.retain(|p| !p.vertices.contains(&0));
        let out = run(wrap_info, wrap_cook, vec![geo(s), None], &[("target_edge_length", f(6.0)), ("remesh_iterations", i(0))]).unwrap();
        let g = out_geo(&out[0]);
        let r = out_report(&out[1]);
        assert_eq!(r.metrics["open_edges"].value, 0.0);
        assert_eq!(r.metrics["components"].value, 1.0);
        assert_eq!(euler(g), 2);
        let vol = 4.0 / 3.0 * std::f64::consts::PI * 50.0f64.powi(3);
        assert!((r.metrics["volume"].value - vol).abs() / vol < 0.2);
    }

    #[test]
    fn fill_input_holes_makes_the_signed_path_available() {
        let mut s = sphere_geo(50.0, 40, 41);
        s.prims.retain(|p| !p.vertices.contains(&0));
        let out = run(wrap_info, wrap_cook, vec![geo(s), None], &[("target_edge_length", f(2.5)), ("fill_input_holes", b(true))]).unwrap();
        let r = out_report(&out[1]);
        assert!(r.notes.iter().any(|n| n.1.contains("closed 1 hole")));
        assert!(!r.notes.iter().any(|n| n.1.contains("unsigned field")));
        assert_eq!(r.metrics["open_edges"].value, 0.0);
        let vol = 4.0 / 3.0 * std::f64::consts::PI * 50.0f64.powi(3);
        assert!((r.metrics["volume"].value - vol).abs() / vol < 0.05);
    }

    #[test]
    fn point_cloud_is_inflated_into_one_shell() {
        let mut rng = Rng::new(3);
        let mut pts = vec![];
        while pts.len() < 3000 {
            let d = DVec3::new(rng.next_f64() * 2.0 - 1.0, rng.next_f64() * 2.0 - 1.0, rng.next_f64() * 2.0 - 1.0);
            if d.length() > 0.1 && d.length() <= 1.0 {
                pts.push(d.normalize() * 30.0);
            }
        }
        let out = run(wrap_info, wrap_cook, vec![geo(Geometry::from_points(pts)), None], &[("target_edge_length", f(2.0)), ("offset", f(3.0))]).unwrap();
        let g = out_geo(&out[0]);
        let r = out_report(&out[1]);
        assert_eq!(r.metrics["open_edges"].value, 0.0);
        assert_eq!(r.metrics["components"].value, 1.0);
        // outer surface only: no inner cavity, genus 0
        assert_eq!(euler(g), 2);
        let max_r = g.points.iter().map(|p| p.length()).fold(0.0, f64::max);
        assert!((max_r - 33.0).abs() < 2.0, "outer radius {max_r}");
        let min_r = g.points.iter().map(|p| p.length()).fold(f64::MAX, f64::min);
        assert!(min_r > 25.0, "inner radius {min_r}");
    }

    #[test]
    fn two_pieces_give_two_shells_and_a_warning() {
        let mut a = sphere_geo(10.0, 24, 25);
        let mut b2 = sphere_geo(10.0, 24, 25);
        b2.transform(glam::DMat4::from_translation(DVec3::new(60.0, 0.0, 0.0)));
        a.merge(&b2);
        let out = run(wrap_info, wrap_cook, vec![geo(a), None], &[("target_edge_length", f(2.0))]).unwrap();
        let r = out_report(&out[1]);
        assert_eq!(r.metrics["components"].value, 2.0);
        assert!(r.notes.iter().any(|n| n.0 == Severity::Warning));
        assert_eq!(r.metrics["open_edges"].value, 0.0);
    }

    #[test]
    fn polygon_optimization_and_smoothing_keep_it_closed() {
        let base = run(wrap_info, wrap_cook, vec![geo(sphere_geo(40.0, 40, 41)), None], &[("target_edge_length", f(2.0))]).unwrap();
        let opt = run(
            wrap_info,
            wrap_cook,
            vec![geo(sphere_geo(40.0, 40, 41)), None],
            &[("target_edge_length", f(2.0)), ("polygon_optimization", f(80.0)), ("smooth_iterations", i(3)), ("shrink_compensate", b(true))],
        )
        .unwrap();
        let (fb, fo) = (out_report(&base[1]).metrics["faces"].value, out_report(&opt[1]).metrics["faces"].value);
        assert!(fo < fb * 0.8, "{fo} vs {fb}");
        assert_eq!(out_report(&opt[1]).metrics["open_edges"].value, 0.0);
    }

    #[test]
    fn oversize_requests_and_empty_inputs_fail_cleanly() {
        let r = run(wrap_info, wrap_cook, vec![geo(sphere_geo(50.0, 20, 21)), None], &[("target_edge_length", f(0.5)), ("max_cells", i(100_000))]);
        assert!(r.is_err());
        assert!(run(wrap_info, wrap_cook, vec![geo(Geometry::new()), None], &[]).is_err());
    }

    #[test]
    fn colours_transfer_from_the_nearest_input_point() {
        let mut s = sphere_geo(20.0, 24, 25);
        let cd: Vec<DVec3> = s.points.iter().map(|p| if p.z > 0.0 { DVec3::new(1.0, 0.0, 0.0) } else { DVec3::new(0.0, 0.0, 1.0) }).collect();
        s.point_attribs.set("Cd", AttribArray::Vec3(cd));
        let out = run(wrap_info, wrap_cook, vec![geo(s), None], &[("target_edge_length", f(2.0)), ("transfer_colors", b(true))]).unwrap();
        let g = out_geo(&out[0]);
        let cd = g.point_attribs.vec3s("Cd").unwrap();
        for (p, c) in g.points.iter().zip(cd) {
            if p.z > 3.0 {
                assert_eq!(c.x, 1.0);
            } else if p.z < -3.0 {
                assert_eq!(c.z, 1.0);
            }
        }
    }

    #[test]
    fn wp6_alpha_wrap_is_native_and_reports_complete_certification() {
        assert!(!alpha_info().whitebox);
        let out = run(alpha_info, alpha_cook, vec![geo(sphere_geo(30.0, 24, 25))], &[]).unwrap();
        let r = out_report(&out[1]);
        assert!(!r.notes.iter().any(|n| n.1.starts_with("white-box placeholder")));
        for key in ["open_edges", "nonmanifold_edges", "inconsistent_edges", "nonmanifold_vertices",
            "degenerate_triangles", "below_offset_vertices", "above_upper_bound_vertices"] {
            assert_eq!(r.metrics[key].value, 0.0, "{key}");
            assert_eq!(r.metrics[key].passes(), Some(true));
        }
        let result = out_geo(&out[0]);
        result.validate().unwrap();
        assert_eq!(r.metrics["checked_vertices"].value, result.points.len() as f64);
        assert!(r.metrics["distance_min_mm"].value >= r.metrics["offset"].value);
        assert!(r.metrics["distance_max_mm"].value <= r.metrics["offset"].value + r.metrics["alpha"].value);
        assert!(r.metrics["projection_moves"].value > 0.0);
    }

    #[test]
    #[ignore = "performance check: run with --release -- --ignored"]
    fn large_grid_wrap_scales() {
        let g = sphere_geo(100.0, 200, 101);
        let t = std::time::Instant::now();
        let out = run(wrap_info, wrap_cook, vec![geo(g), None], &[("target_edge_length", f(1.0)), ("remesh_iterations", i(0))]).unwrap();
        let r = out_report(&out[1]);
        eprintln!("wrap: {} faces, {} grid nodes, {} band nodes in {:?}", r.metrics["faces"].value, r.metrics["grid_nodes"].value, r.metrics["band_nodes"].value, t.elapsed());
        assert_eq!(r.metrics["open_edges"].value, 0.0);
    }
}

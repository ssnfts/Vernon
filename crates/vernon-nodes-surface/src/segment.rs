//! Curvature / primitive based region segmentation of a triangle mesh
//! (bottom-up merging with plane, sphere and cylinder models), region
//! refinement and primitive fitting.
//!
//! Regions start as single triangles and merge greedily, cheapest merged
//! fit first, while one primitive explains the union within the tolerance
//! `tau` and the face normals stay within `theta` of the primitive normal.
//! Small leftover fragments are then absorbed into the neighbour they share
//! the longest border with, and every region is classified again.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap};

use glam::DVec3;
use serde_json::json;
use vernon_core::{AttribArray, Geometry, Metric, Report, Severity};
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::curvefit::fit_circle;
use crate::fit::TriMesh;
use crate::linalg::{scatter, solve_dense, sym_eigen3};
use crate::topology::{tri_adjacency, weld};
use crate::util::{check_finite, max_abs_coord, node_info, positive, triangulate_ring, SPEC_RE};

/// An analytic surface fitted to a region.
#[derive(Debug, Clone, PartialEq)]
pub enum Primitive {
    Plane { origin: DVec3, normal: DVec3 },
    Sphere { center: DVec3, radius: f64 },
    Cylinder { origin: DVec3, axis: DVec3, radius: f64 },
    Freeform,
}

impl Primitive {
    pub fn kind(&self) -> &'static str {
        match self {
            Primitive::Plane { .. } => "plane",
            Primitive::Sphere { .. } => "sphere",
            Primitive::Cylinder { .. } => "cylinder",
            Primitive::Freeform => "freeform",
        }
    }

    /// Distance from `p` to the surface.
    pub fn distance(&self, p: DVec3) -> f64 {
        match self {
            Primitive::Plane { origin, normal } => (p - *origin).dot(*normal).abs(),
            Primitive::Sphere { center, radius } => ((p - *center).length() - radius).abs(),
            Primitive::Cylinder { origin, axis, radius } => {
                let d = p - *origin;
                ((d - *axis * d.dot(*axis)).length() - radius).abs()
            }
            Primitive::Freeform => f64::INFINITY,
        }
    }

    /// Unit surface normal direction at `p` (sign arbitrary).
    pub fn normal_at(&self, p: DVec3) -> DVec3 {
        match self {
            Primitive::Plane { normal, .. } => *normal,
            Primitive::Sphere { center, .. } => (p - *center).normalize_or_zero(),
            Primitive::Cylinder { origin, axis, .. } => {
                let d = p - *origin;
                (d - *axis * d.dot(*axis)).normalize_or_zero()
            }
            Primitive::Freeform => DVec3::ZERO,
        }
    }
}

/// Tolerances of a segmentation run.
#[derive(Debug, Clone, Copy)]
pub struct SegParams {
    /// 0..100: higher splits finer.
    pub sensitivity: f64,
    /// Scan noise level used to scale the fit tolerance.
    pub noise_sigma: f64,
    /// Regions smaller than this share of the total area are absorbed.
    pub min_area_frac: f64,
    /// Meshes made from CAD facets: exact coplanarity and tight tolerances.
    pub cad_tessellation: bool,
}

impl SegParams {
    /// Normal-angle limit in degrees (35 at sensitivity 0, 3 at 100).
    pub fn theta_deg(&self) -> f64 {
        35.0 - 32.0 * (self.sensitivity / 100.0).clamp(0.0, 1.0)
    }

    /// RMS / distance tolerance in mm (4 sigma at 0, 1 sigma at 100).
    pub fn tau(&self) -> f64 {
        if self.cad_tessellation {
            return self.noise_sigma.max(1e-6) * 0.2;
        }
        self.noise_sigma.max(1e-6) * (4.0 - 3.0 * (self.sensitivity / 100.0).clamp(0.0, 1.0))
    }
}

/// A classified region.
#[derive(Debug, Clone)]
pub struct Region {
    pub tris: Vec<u32>,
    pub area: f64,
    pub prim: Primitive,
    pub rms: f64,
    pub max_dev: f64,
}

/// Result of a segmentation.
#[derive(Debug, Clone)]
pub struct Segmentation {
    /// Region index per triangle.
    pub labels: Vec<u32>,
    /// Regions ordered by decreasing area.
    pub regions: Vec<Region>,
    pub tau: f64,
    pub theta_deg: f64,
}

struct Geo {
    pts: Vec<DVec3>,
    tris: Vec<[u32; 3]>,
    normals: Vec<DVec3>,
    areas: Vec<f64>,
}

impl Geo {
    fn new(m: &TriMesh) -> Geo {
        let mut normals = Vec::with_capacity(m.tris.len());
        let mut areas = Vec::with_capacity(m.tris.len());
        for t in &m.tris {
            let (a, b, c) = (m.points[t[0] as usize], m.points[t[1] as usize], m.points[t[2] as usize]);
            let n = (b - a).cross(c - a);
            areas.push(0.5 * n.length());
            normals.push(n.normalize_or_zero());
        }
        Geo { pts: m.points.clone(), tris: m.tris.clone(), normals, areas }
    }
}

fn fit_plane(pts: &[DVec3]) -> Option<Primitive> {
    if pts.len() < 3 {
        return None;
    }
    let (c, m) = scatter(pts, None);
    let (_, v) = sym_eigen3(m);
    Some(Primitive::Plane { origin: c, normal: v[0] })
}

fn fit_sphere(pts: &[DVec3]) -> Option<Primitive> {
    if pts.len() < 5 {
        return None;
    }
    let c0 = pts.iter().fold(DVec3::ZERO, |s, p| s + *p) / pts.len() as f64;
    let (mut a, mut b) = ([0.0; 16], [0.0; 4]);
    for p in pts {
        let q = *p - c0;
        let row = [2.0 * q.x, 2.0 * q.y, 2.0 * q.z, 1.0];
        let rhs = q.length_squared();
        for i in 0..4 {
            b[i] += row[i] * rhs;
            for j in 0..4 {
                a[i * 4 + j] += row[i] * row[j];
            }
        }
    }
    let s = solve_dense(&a, 4, &b)?;
    let center = c0 + DVec3::new(s[0], s[1], s[2]);
    let r2 = s[3] + s[0] * s[0] + s[1] * s[1] + s[2] * s[2];
    if r2 <= 0.0 || !r2.is_finite() {
        return None;
    }
    Some(Primitive::Sphere { center, radius: r2.sqrt() })
}

fn fit_cylinder(pts: &[DVec3], normals: &[(DVec3, f64)]) -> Option<Primitive> {
    if pts.len() < 6 || normals.is_empty() {
        return None;
    }
    let mut m = [[0.0; 3]; 3];
    for (n, w) in normals {
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += w * n[i] * n[j];
            }
        }
    }
    let (ev, vecs) = sym_eigen3(m);
    // cylinder normals span a plane: the smallest eigenvalue is ~0, the other two are not
    if ev[2] <= 0.0 || ev[1] < 0.02 * ev[2] {
        return None;
    }
    let axis = vecs[0].normalize_or_zero();
    if axis.length_squared() < 0.5 {
        return None;
    }
    let (ex, ey) = {
        let h = if axis.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
        let ex = axis.cross(h).normalize();
        (ex, axis.cross(ex))
    };
    let proj: Vec<DVec3> = pts.iter().map(|p| ex * p.dot(ex) + ey * p.dot(ey)).collect();
    let cf = fit_circle(&proj)?;
    if !(cf.radius.is_finite() && cf.radius > 0.0) {
        return None;
    }
    // centre in the projection plane; the axis passes through it
    let cp = cf.center;
    let origin = ex * cp.dot(ex) + ey * cp.dot(ey);
    Some(Primitive::Cylinder { origin, axis, radius: cf.radius })
}

fn subsample<T: Copy>(v: &[T], cap: usize) -> Vec<T> {
    if v.len() <= cap {
        return v.to_vec();
    }
    let stride = v.len().div_ceil(cap);
    v.iter().step_by(stride).copied().collect()
}

/// Residual statistics of `prim` over the vertices and face normals of a region.
fn residuals(g: &Geo, prim: &Primitive, tris: &[u32], verts: &[DVec3]) -> (f64, f64, f64) {
    let mut s2 = 0.0;
    let mut mx: f64 = 0.0;
    for p in verts {
        let d = prim.distance(*p);
        s2 += d * d;
        mx = mx.max(d);
    }
    let rms = (s2 / verts.len().max(1) as f64).sqrt();
    let mut worst_cos: f64 = 1.0;
    for &t in tris {
        let tri = g.tris[t as usize];
        let c = (g.pts[tri[0] as usize] + g.pts[tri[1] as usize] + g.pts[tri[2] as usize]) / 3.0;
        let n = prim.normal_at(c);
        if n.length_squared() > 0.0 {
            worst_cos = worst_cos.min(n.dot(g.normals[t as usize]).abs());
        }
    }
    (rms, mx, worst_cos.clamp(-1.0, 1.0).acos().to_degrees())
}

struct FitOutcome {
    prim: Primitive,
    rms: f64,
}

/// Cheapest primitive that explains the region, if any.
fn fit_region(g: &Geo, tris: &[u32], verts: &[u32], tau: f64, theta: f64, allow_curved: bool) -> Option<FitOutcome> {
    let pts_all: Vec<DVec3> = verts.iter().map(|&v| g.pts[v as usize]).collect();
    let pts = subsample(&pts_all, 400);
    let tris_s = subsample(tris, 400);
    let try_one = |prim: Primitive| -> Option<FitOutcome> {
        let (rms, mx, ang) = residuals(g, &prim, &tris_s, &pts);
        if rms <= tau && mx <= 3.0 * tau && ang <= theta {
            Some(FitOutcome { prim, rms })
        } else {
            None
        }
    };
    if let Some(p) = fit_plane(&pts).and_then(try_one) {
        return Some(p);
    }
    if !allow_curved || tris.len() < 6 || verts.len() < 10 {
        return None;
    }
    let normals: Vec<(DVec3, f64)> = tris_s.iter().map(|&t| (g.normals[t as usize], g.areas[t as usize])).collect();
    let mut best: Option<FitOutcome> = None;
    for cand in [fit_cylinder(&pts, &normals), fit_sphere(&pts)].into_iter().flatten() {
        if let Primitive::Sphere { radius, .. } = &cand {
            if *radius > 1e4 {
                continue;
            }
        }
        if let Some(o) = try_one(cand) {
            if best.as_ref().is_none_or(|b| o.rms < b.rms) {
                best = Some(o);
            }
        }
    }
    best
}

#[derive(Clone)]
struct Reg {
    tris: Vec<u32>,
    verts: Vec<u32>,
    area: f64,
    nbr: BTreeMap<usize, usize>,
    alive: bool,
    version: u32,
}

#[derive(PartialEq)]
struct Cand {
    cost: f64,
    size: usize,
    a: usize,
    b: usize,
    va: u32,
    vb: u32,
}

impl Eq for Cand {}
impl PartialOrd for Cand {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Cand {
    // BinaryHeap is a max-heap: reverse so the cheapest candidate pops first
    fn cmp(&self, o: &Self) -> Ordering {
        o.cost.total_cmp(&self.cost).then(o.size.cmp(&self.size)).then(o.a.cmp(&self.a)).then(o.b.cmp(&self.b))
    }
}

fn union_sorted(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            Ordering::Less => {
                out.push(a[i]);
                i += 1;
            }
            Ordering::Greater => {
                out.push(b[j]);
                j += 1;
            }
            Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out.extend_from_slice(&a[i..]);
    out.extend_from_slice(&b[j..]);
    out
}

/// One greedy merge pass: adjacent regions merge, cheapest merged fit first,
/// while a single model explains the union within `tau` and `theta`.
fn merge_pass(g: &Geo, regs: &mut [Reg], tau: f64, theta: f64, allow_curved: bool) {
    let evaluate = |regs: &[Reg], a: usize, b: usize| -> Option<f64> {
        let verts = union_sorted(&regs[a].verts, &regs[b].verts);
        let mut tris = regs[a].tris.clone();
        tris.extend_from_slice(&regs[b].tris);
        fit_region(g, &tris, &verts, tau, theta, allow_curved).map(|o| o.rms)
    };
    let mut heap: BinaryHeap<Cand> = BinaryHeap::new();
    for a in 0..regs.len() {
        if !regs[a].alive {
            continue;
        }
        let nb: Vec<usize> = regs[a].nbr.keys().copied().collect();
        for b in nb {
            if a < b {
                if let Some(c) = evaluate(regs, a, b) {
                    heap.push(Cand { cost: c, size: regs[a].tris.len() + regs[b].tris.len(), a, b, va: regs[a].version, vb: regs[b].version });
                }
            }
        }
    }
    while let Some(c) = heap.pop() {
        if !regs[c.a].alive || !regs[c.b].alive || regs[c.a].version != c.va || regs[c.b].version != c.vb {
            continue;
        }
        merge_regions(regs, c.a, c.b);
        let a = c.a;
        let nb: Vec<usize> = regs[a].nbr.keys().copied().collect();
        for b in nb {
            if let Some(cost) = evaluate(regs, a, b) {
                let (lo, hi) = (a.min(b), a.max(b));
                heap.push(Cand { cost, size: regs[a].tris.len() + regs[b].tris.len(), a: lo, b: hi, va: regs[lo].version, vb: regs[hi].version });
            }
        }
    }
}

/// Segment a triangle mesh into regions.
pub fn segment(mesh: &TriMesh, p: &SegParams) -> Segmentation {
    let g = Geo::new(mesh);
    let nt = g.tris.len();
    let tau = p.tau();
    let theta = p.theta_deg();
    let diag = {
        let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
        for q in &g.pts {
            lo = lo.min(*q);
            hi = hi.max(*q);
        }
        if g.pts.is_empty() { 0.0 } else { (hi - lo).length() }
    };
    let mut regs: Vec<Reg> = (0..nt)
        .map(|t| {
            let mut v = g.tris[t].to_vec();
            v.sort_unstable();
            Reg { tris: vec![t as u32], verts: v, area: g.areas[t], nbr: BTreeMap::new(), alive: true, version: 0 }
        })
        .collect();
    let adj = tri_adjacency(&g.tris);
    for (t, a) in adj.iter().enumerate() {
        for n in a.iter().flatten() {
            *regs[t].nbr.entry(*n as usize).or_insert(0) += 1;
        }
    }
    let total_area: f64 = g.areas.iter().sum();
    // pass 1: locally flat seed clusters, so the curved models get well-posed data
    let tau_seed = p.noise_sigma.max(1e-6) * 2.35;
    let (tau1, theta1) = ((3.0 * tau_seed).max(0.002 * diag), theta.min(8.0));
    merge_pass(&g, &mut regs, tau1, theta1, false);
    // pass 2: real tolerance, all models
    merge_pass(&g, &mut regs, tau, theta, true);
    // absorb fragments
    let min_area = p.min_area_frac * total_area;
    loop {
        let frag = (0..regs.len()).filter(|&i| regs[i].alive && regs[i].area < min_area && !regs[i].nbr.is_empty()).min_by(|&x, &y| regs[x].area.total_cmp(&regs[y].area).then(x.cmp(&y)));
        let Some(f) = frag else { break };
        let (&best, _) = regs[f].nbr.iter().max_by(|x, y| x.1.cmp(y.1).then(regs[*x.0].area.total_cmp(&regs[*y.0].area)).then(y.0.cmp(x.0))).expect("has neighbours");
        merge_regions(&mut regs, best.min(f), best.max(f));
    }
    // classify
    let mut alive: Vec<usize> = (0..regs.len()).filter(|&i| regs[i].alive).collect();
    alive.sort_by(|&x, &y| regs[y].area.total_cmp(&regs[x].area).then(regs[x].tris[0].cmp(&regs[y].tris[0])));
    let mut labels = vec![0u32; nt];
    let mut regions = Vec::with_capacity(alive.len());
    for (ri, &i) in alive.iter().enumerate() {
        for &t in &regs[i].tris {
            labels[t as usize] = ri as u32;
        }
        regions.push(classify(&g, &regs[i].tris, &regs[i].verts, tau, theta));
    }
    Segmentation { labels, regions, tau, theta_deg: theta }
}

fn merge_regions(regs: &mut [Reg], a: usize, b: usize) {
    let rb = std::mem::replace(&mut regs[b], Reg { tris: vec![], verts: vec![], area: 0.0, nbr: BTreeMap::new(), alive: false, version: 0 });
    let verts = union_sorted(&regs[a].verts, &rb.verts);
    regs[a].verts = verts;
    regs[a].tris.extend(rb.tris);
    regs[a].area += rb.area;
    regs[a].version += 1;
    regs[a].nbr.remove(&b);
    for (n, cnt) in rb.nbr {
        if n == a {
            continue;
        }
        *regs[a].nbr.entry(n).or_insert(0) += cnt;
        let other = &mut regs[n].nbr;
        let moved = other.remove(&b).unwrap_or(0);
        *other.entry(a).or_insert(0) += moved;
    }
}

fn classify(g: &Geo, tris: &[u32], verts: &[u32], tau: f64, theta: f64) -> Region {
    let area: f64 = tris.iter().map(|&t| g.areas[t as usize]).sum();
    // classification uses a looser angle limit than merging: it only labels
    match fit_region(g, tris, verts, tau, theta.max(20.0), true) {
        Some(o) => {
            // refit on the full vertex set for the final numbers
            let pts: Vec<DVec3> = verts.iter().map(|&v| g.pts[v as usize]).collect();
            let normals: Vec<(DVec3, f64)> = tris.iter().map(|&t| (g.normals[t as usize], g.areas[t as usize])).collect();
            let refit = match &o.prim {
                Primitive::Plane { .. } => fit_plane(&pts),
                Primitive::Sphere { .. } => fit_sphere(&pts),
                Primitive::Cylinder { .. } => fit_cylinder(&pts, &normals),
                Primitive::Freeform => None,
            }
            .unwrap_or(o.prim.clone());
            let (rms, mx, _) = residuals(g, &refit, &[], &pts);
            Region { tris: tris.to_vec(), area, prim: refit, rms, max_dev: mx }
        }
        None => {
            let pts: Vec<DVec3> = verts.iter().map(|&v| g.pts[v as usize]).collect();
            let (rms, mx) = match fit_plane(&pts) {
                Some(p) => {
                    let (r, m, _) = residuals(g, &p, &[], &pts);
                    (r, m)
                }
                None => (0.0, 0.0),
            };
            Region { tris: tris.to_vec(), area, prim: Primitive::Freeform, rms, max_dev: mx }
        }
    }
}

/// Re-label with explicit region groups and re-classify.
pub fn classify_labels(mesh: &TriMesh, labels: &[i64], p: &SegParams) -> Segmentation {
    let g = Geo::new(mesh);
    let mut ids: Vec<i64> = labels.iter().copied().filter(|&l| l >= 0).collect();
    ids.sort_unstable();
    ids.dedup();
    let (tau, theta) = (p.tau(), p.theta_deg());
    let mut members: Vec<(i64, Vec<u32>)> = ids.iter().map(|&i| (i, Vec::new())).collect();
    for (t, &l) in labels.iter().enumerate() {
        if l >= 0 {
            let k = ids.binary_search(&l).expect("known id");
            members[k].1.push(t as u32);
        }
    }
    let mut regions: Vec<Region> = members
        .iter()
        .map(|(_, tris)| {
            let mut verts: Vec<u32> = tris.iter().flat_map(|&t| g.tris[t as usize]).collect();
            verts.sort_unstable();
            verts.dedup();
            classify(&g, tris, &verts, tau, theta)
        })
        .collect();
    let mut out_labels = vec![u32::MAX; labels.len()];
    for (k, (_, tris)) in members.iter().enumerate() {
        for &t in tris {
            out_labels[t as usize] = k as u32;
        }
    }
    // keep region order = ascending original id so callers can map back
    regions.shrink_to_fit();
    Segmentation { labels: out_labels, regions, tau, theta_deg: theta }
}

/// Merge the smallest regions into the neighbour sharing the longest border
/// until at most `max_count` regions remain.
pub fn limit_regions(mesh: &TriMesh, seg: &Segmentation, max_count: usize, p: &SegParams) -> Segmentation {
    if seg.regions.len() <= max_count.max(1) {
        return seg.clone();
    }
    let adj = tri_adjacency(&mesh.tris);
    let mut labels: Vec<i64> = seg.labels.iter().map(|&l| i64::from(l)).collect();
    let mut area: BTreeMap<i64, f64> = BTreeMap::new();
    let g = Geo::new(mesh);
    for (t, &l) in labels.iter().enumerate() {
        *area.entry(l).or_insert(0.0) += g.areas[t];
    }
    while area.len() > max_count.max(1) {
        let (&small, _) = area.iter().min_by(|a, b| a.1.total_cmp(b.1).then(a.0.cmp(b.0))).expect("regions");
        let mut border: BTreeMap<i64, usize> = BTreeMap::new();
        for (t, &l) in labels.iter().enumerate() {
            if l != small {
                continue;
            }
            for n in adj[t].iter().flatten() {
                let nl = labels[*n as usize];
                if nl != small {
                    *border.entry(nl).or_insert(0) += 1;
                }
            }
        }
        let target = border.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(k, _)| *k);
        let Some(target) = target else {
            area.remove(&small);
            continue;
        };
        for l in labels.iter_mut() {
            if *l == small {
                *l = target;
            }
        }
        let a = area.remove(&small).unwrap_or(0.0);
        *area.entry(target).or_insert(0.0) += a;
    }
    classify_labels(mesh, &labels, p)
}

/// Welded triangle mesh of a geometry together with the source prim of every triangle.
pub struct SegMesh {
    pub mesh: TriMesh,
    pub tri_prim: Vec<usize>,
}

pub fn seg_mesh(geo: &Geometry) -> SegMesh {
    let w = weld(geo, 1e-6f64.max(1.2e-7 * max_abs_coord(&geo.points)));
    let mut tris = Vec::new();
    let mut tri_prim = Vec::new();
    for (fi, f) in w.faces.iter().enumerate() {
        for t in triangulate_ring(&w.points, f) {
            tris.push(t);
            tri_prim.push(w.src_prim[fi]);
        }
    }
    SegMesh { mesh: TriMesh { points: w.points, tris }, tri_prim }
}

fn palette(i: usize) -> DVec3 {
    let h = (i as f64 * 0.618_033_988_75).fract();
    let (s, v) = (0.55, 0.95);
    let k = h * 6.0;
    let (c, x) = (v * s, v * s * (1.0 - (k % 2.0 - 1.0).abs()));
    let m = v - c;
    let rgb = match k as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    DVec3::new(rgb.0 + m, rgb.1 + m, rgb.2 + m)
}

/// Write region ids / types / colours back onto the prims of `geo`.
pub fn write_regions(geo: &mut Geometry, sm: &SegMesh, labels: &[u32], regions: &[Region], ids: &[i64]) {
    let np = geo.prims.len();
    let mut rid = vec![-1i64; np];
    let mut rty = vec![String::new(); np];
    let mut cd = vec![DVec3::splat(0.5); np];
    for (t, &l) in labels.iter().enumerate() {
        let p = sm.tri_prim[t];
        if l != u32::MAX && rid[p] == -1 {
            let id = ids[l as usize];
            rid[p] = id;
            rty[p] = regions[l as usize].prim.kind().to_string();
            cd[p] = palette(id as usize);
        }
    }
    geo.prim_attribs.set("region_id", AttribArray::Int(rid));
    geo.prim_attribs.set("region_type", AttribArray::Str(rty));
    geo.prim_attribs.set("Cd", AttribArray::Vec3(cd));
}

fn seg_params(ctx: &CookContext, params: &Params) -> CookResult<SegParams> {
    Ok(SegParams {
        sensitivity: params.float("sensitivity")?.clamp(0.0, 100.0),
        noise_sigma: positive(ctx, "noise_sigma_mm", params.float("noise_sigma_mm")?)?,
        min_area_frac: params.float("min_region_area_pct")?.clamp(0.0, 10.0) / 100.0,
        cad_tessellation: params.bool("cad_tessellation")?,
    })
}

fn seg_report(title: &str, seg: &Segmentation, total_area: f64) -> Report {
    let mut r = Report::new(title);
    r.metric("region_count", Metric::new(seg.regions.len() as f64, "count"));
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut freeform_area = 0.0;
    let mut worst_rms: f64 = 0.0;
    for reg in &seg.regions {
        *counts.entry(reg.prim.kind()).or_default() += 1;
        if reg.prim == Primitive::Freeform {
            freeform_area += reg.area;
        } else {
            worst_rms = worst_rms.max(reg.rms);
        }
    }
    for kind in ["plane", "sphere", "cylinder", "freeform"] {
        r.metric(format!("count_{kind}"), Metric::new(*counts.get(kind).unwrap_or(&0) as f64, "count"));
    }
    r.metric("freeform_area_pct", Metric::new(100.0 * freeform_area / total_area.max(1e-300), "%"));
    r.metric("primitive_rms_max_mm", Metric::new(worst_rms, "mm"));
    r.metric("tau_mm", Metric::new(seg.tau, "mm"));
    r.metric("normal_limit_deg", Metric::new(seg.theta_deg, "deg"));
    r
}

fn seg_ports(info: NodeTypeInfo) -> NodeTypeInfo {
    info.param(ParamSpec::float("sensitivity", "Sensitivity", 55.0, 0.0, 100.0).help("Higher values split the mesh into more, smaller regions."))
        .param(ParamSpec::float("noise_sigma_mm", "Noise level", 0.05, 0.0005, 5.0).unit("mm").help("Scan noise used to scale the fit tolerance."))
        .param(ParamSpec::float("min_region_area_pct", "Minimum region area", 0.05, 0.0, 10.0).unit("%").help("Regions smaller than this share of the total area are absorbed by a neighbour."))
        .param(ParamSpec::bool("cad_tessellation", "Tessellated CAD mesh", false).help("Use the tight tolerance suited to meshes whose facets lie exactly on the original surfaces."))
}

/// `surface.segment_regions`
pub struct SegmentRegionsNode;

impl NodeOp for SegmentRegionsNode {
    fn info(&self) -> NodeTypeInfo {
        seg_ports(
            node_info("surface.segment_regions", "Segment Regions", SPEC_RE)
                .input(PortSpec::new("mesh", DataKind::Geometry))
                .output(PortSpec::new("regions", DataKind::Geometry))
                .output(PortSpec::new("report", DataKind::Report)),
        )
        .help("Splits a mesh into regions that each follow one plane, sphere or cylinder, or are freeform. Writes the primitive attributes 'region_id', 'region_type' and a display colour. Cones and tori are not recognised yet and fall into freeform regions.")
        .emulates("Design X Auto Segment")
        .emulates("QUICKSURFACE automatic mesh segmentation")
        .emulates("Variational shape approximation with region merging")
        .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let geo = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &geo)?;
        let sp = seg_params(ctx, params)?;
        let sm = seg_mesh(&geo);
        if sm.mesh.tris.is_empty() {
            return Err(ctx.fail("the input has no faces to segment"));
        }
        if sm.mesh.tris.len() > 400_000 {
            return Err(ctx.fail(format!("{} triangles is too many to segment; reduce the mesh first", sm.mesh.tris.len())));
        }
        let seg = segment(&sm.mesh, &sp);
        let ids: Vec<i64> = (0..seg.regions.len() as i64).collect();
        let mut out = (*geo).clone();
        write_regions(&mut out, &sm, &seg.labels, &seg.regions, &ids);
        let report = seg_report("Region segmentation", &seg, sm.mesh.area());
        Ok(vec![Data::geometry(out), Data::report(report)])
    }
}

/// Region ids per triangle from the prim attribute `region_id`.
pub(crate) fn labels_from_geo(geo: &Geometry, sm: &SegMesh) -> Option<Vec<i64>> {
    let ids = geo.prim_attribs.ints("region_id").ok()?;
    sm.tri_prim.iter().map(|&p| ids.get(p).copied()).collect()
}

/// `surface.refine_regions`
pub struct RefineRegionsNode;

/// A projected cut removes crossings in the original face dual graph. Keeping
/// all triangles of a polygon together avoids silently losing a split on export.
fn polyline_split(geo: &Geometry, sm: &SegMesh, labels: &mut [i64], targets: &[i64], curve: &Geometry) -> Result<(), String> {
    if curve.prims.len() != 1 || curve.prims[0].closed {
        return Err("provide one open polyline primitive; branches and polygon primitives are unsupported (repeat the first vertex to close a polyline)".into());
    }
    let path = &curve.prims[0].vertices;
    if path.len() < 2 || path.iter().any(|&i| i as usize >= curve.points.len()) {
        return Err("polyline needs at least two valid point indices".into());
    }
    if path.len().saturating_mul(sm.mesh.tris.len()) > 20_000_000 {
        return Err("projected cut exceeds the work limit; simplify the polyline or mesh".into());
    }
    let adj = tri_adjacency(&sm.mesh.tris);
    let mut face_edges = std::collections::BTreeSet::new();
    for (t, ns) in adj.iter().enumerate() {
        for &n in ns.iter().flatten() {
            let (a, b) = (sm.tri_prim[t], sm.tri_prim[n as usize]);
            if a != b { face_edges.insert((a.min(b), a.max(b))); }
        }
    }
    let mut face_labels = vec![-1; geo.prims.len()];
    for (t, &id) in labels.iter().enumerate() { face_labels[sm.tri_prim[t]] = id; }
    let centers: Vec<DVec3> = geo.prims.iter().map(|p| p.vertices.iter().map(|&i| geo.points[i as usize]).sum::<DVec3>() / p.vertices.len().max(1) as f64).collect();
    let mut next = labels.iter().copied().max().unwrap_or(-1).checked_add(1).ok_or("region id overflow")?;
    let mut cuts = 0;
    for &id in targets {
        let faces: Vec<usize> = face_labels.iter().enumerate().filter_map(|(i, &l)| (l == id).then_some(i)).collect();
        let pts: Vec<DVec3> = faces.iter().flat_map(|&i| geo.prims[i].vertices.iter().map(|&v| geo.points[v as usize])).collect();
        let (origin, m) = scatter(&pts, None);
        let (_, basis) = sym_eigen3(m);
        let project = |p: DVec3| { let q = p - origin; glam::DVec2::new(q.dot(basis[1]), q.dot(basis[2])) };
        let segments: Vec<_> = path.windows(2).map(|w| (project(curve.points[w[0] as usize]), project(curve.points[w[1] as usize]))).collect();
        if segments.iter().all(|(a,b)| a.distance_squared(*b) < 1e-20) { return Err("polyline has no length in the region projection".into()); }
        let mut graph = vec![Vec::new(); geo.prims.len()];
        let mut uncut = graph.clone();
        for &(a,b) in &face_edges {
            if face_labels[a] != id || face_labels[b] != id { continue; }
            uncut[a].push(b); uncut[b].push(a);
            let (p,q) = (project(centers[a]), project(centers[b]));
            let crosses = segments.iter().any(|&(r,s)| {
                let (d,e) = (q-p,s-r);
                let den = d.perp_dot(e);
                if den.abs() < 1e-14 { return false; }
                let t = (r-p).perp_dot(e)/den;
                let u = (r-p).perp_dot(d)/den;
                (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)
            });
            if !crosses { graph[a].push(b); graph[b].push(a); }
        }
        let components = |graph: &[Vec<usize>]| {
            let mut seen = vec![false; geo.prims.len()];
            let mut out = Vec::new();
            for &seed in &faces {
                if seen[seed] { continue; }
                seen[seed] = true;
                let mut members = vec![seed]; let mut cursor = 0;
                while cursor < members.len() {
                    for &n in &graph[members[cursor]] { if !seen[n] { seen[n] = true; members.push(n); } }
                    cursor += 1;
                }
                out.push(members);
            }
            out
        };
        let original = components(&uncut);
        let split = components(&graph);
        if split.len() <= original.len() { continue; }
        cuts += split.len() - original.len();
        // Retain the old id in one piece of each pre-existing component.
        let mut retained = std::collections::BTreeSet::new();
        for members in split {
            let parent = original.iter().position(|c| c.contains(&members[0])).expect("component parent");
            if retained.insert(parent) { continue; }
            for f in members { face_labels[f] = next; }
            next = next.checked_add(1).ok_or("region id overflow")?;
        }
    }
    if cuts == 0 { return Err("projected polyline does not separate any selected region; extend it to the region boundary or supply a closed loop".into()); }
    for (t,l) in labels.iter_mut().enumerate() { *l = face_labels[sm.tri_prim[t]]; }
    Ok(())
}

impl NodeOp for RefineRegionsNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.refine_regions", "Refine Regions", SPEC_RE)
            .input(PortSpec::new("regions", DataKind::Geometry))
            .input(PortSpec::optional("polyline", DataKind::Geometry))
            .output(PortSpec::new("regions", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("op", "Operation", "resegment", &["resegment", "merge", "split_by_polyline"]).help("Re-run segmentation inside regions, merge regions, or cut a region along a polyline."))
            .param(ParamSpec::string("group", "Regions", "").help("Comma separated region ids the operation acts on; empty means all regions for re-segmentation."))
            .param(ParamSpec::float("sensitivity", "Sensitivity", 80.0, 0.0, 100.0).help("Sensitivity used when re-segmenting."))
            .param(ParamSpec::float("noise_sigma_mm", "Noise level", 0.05, 0.0005, 5.0).unit("mm").help("Scan noise used to scale the fit tolerance."))
            .param(ParamSpec::float("min_region_area_pct", "Minimum region area", 0.05, 0.0, 10.0).unit("%").help("Smallest region kept when re-segmenting."))
            .param(ParamSpec::bool("cad_tessellation", "Tessellated CAD mesh", false).help("Use the tight tolerance suited to meshes whose facets lie exactly on the original surfaces."))
            .help("Edits a segmentation: re-run it at higher sensitivity inside chosen regions, merge several regions into one, or cut original faces along a polyline projected into each region's best-fit plane. Use a single open polyline, repeating its first vertex for a closed loop. Folded regions with overlapping planar projections must be separated first. Mesh topology is preserved.")
            .emulates("Design X Resegment / Merge / Split")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let geo = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &geo)?;
        let sm = seg_mesh(&geo);
        let Some(mut labels) = labels_from_geo(&geo, &sm) else {
            return Err(ctx.fail("the input has no 'region_id' primitive attribute; run Segment Regions first"));
        };
        if sm.mesh.tris.is_empty() || sm.mesh.tris.len() > 400_000 {
            return Err(ctx.fail("region refinement requires between 1 and 400000 triangles"));
        }
        if labels.iter().copied().max() == Some(i64::MAX) {
            return Err(ctx.fail("region ids leave no space for new region labels"));
        }
        let text = params.str("group")?.trim();
        let mut group: Vec<i64> = if text.is_empty() { vec![] } else {
            text.split(',').map(|s| s.trim().parse::<i64>().map_err(|_| ctx.fail("region group must contain comma separated nonnegative integer ids"))).collect::<Result<_,_>>()?
        };
        group.sort_unstable(); group.dedup();
        for &id in &group {
            if id < 0 || !labels.contains(&id) { return Err(ctx.fail(format!("region {id} does not exist"))); }
        }
        let sp = seg_params(ctx, params)?;
        let op = params.str("op")?.to_string();
        let mut notes: Vec<(vernon_core::Severity, String)> = Vec::new();
        match op.as_str() {
            "merge" => {
                if group.len() < 2 {
                    return Err(ctx.fail("list at least two region ids to merge"));
                }
                let first = group[0];
                for l in labels.iter_mut() {
                    if group.contains(l) {
                        *l = first;
                    }
                }
            }
            "resegment" => {
                let mut next = labels.iter().copied().max().unwrap_or(-1) + 1;
                let targets: Vec<i64> = if group.is_empty() {
                    let mut v: Vec<i64> = labels.iter().copied().filter(|&l| l >= 0).collect();
                    v.sort_unstable();
                    v.dedup();
                    v
                } else {
                    group.clone()
                };
                for id in targets {
                    let tri_ids: Vec<usize> = (0..labels.len()).filter(|&t| labels[t] == id).collect();
                    if tri_ids.is_empty() {
                        notes.push((vernon_core::Severity::Warning, format!("region {id} does not exist")));
                        continue;
                    }
                    let sub_mesh = sm.mesh.sub(&tri_ids);
                    let sub = segment(&sub_mesh, &sp);
                    for (k, &t) in tri_ids.iter().enumerate() {
                        let l = sub.labels[k];
                        labels[t] = if l == 0 { id } else { next + l as i64 - 1 };
                    }
                    next += (sub.regions.len() as i64 - 1).max(0);
                }
            }
            "split_by_polyline" => {
                let curve = vernon_graph::node::inputs::geometry(inputs, 1)?;
                check_finite(ctx, &curve)?;
                let targets: Vec<i64> = if group.is_empty() { labels.iter().copied().filter(|&l| l >= 0).collect::<std::collections::BTreeSet<_>>().into_iter().collect() } else { group.clone() };
                polyline_split(&geo, &sm, &mut labels, &targets, &curve).map_err(|e| ctx.fail(e))?;
            }
            _ => return Err(ctx.fail("unknown region refinement operation")),
        }
        // compact ids in ascending order and reclassify
        let seg = classify_labels(&sm.mesh, &labels, &sp);
        let mut ids: Vec<i64> = labels.iter().copied().filter(|&l| l >= 0).collect();
        ids.sort_unstable();
        ids.dedup();
        let mut out = (*geo).clone();
        write_regions(&mut out, &sm, &seg.labels, &seg.regions, &ids);
        let mut report = seg_report("Refined regions", &seg, sm.mesh.area());
        for (s, t) in notes {
            report.note(s, t);
        }
        Ok(vec![Data::geometry(out), Data::report(report)])
    }
}

/// `surface.fit_primitives`
pub struct FitPrimitivesNode;

impl NodeOp for FitPrimitivesNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.fit_primitives", "Fit Primitives", SPEC_RE)
            .input(PortSpec::new("regions", DataKind::Geometry))
            .output(PortSpec::new("faces", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("tol_mm", "Tolerance", 0.05, 0.001, 5.0).unit("mm").help("Largest accepted distance between a region and its primitive."))
            .param(ParamSpec::float("max_hole_radius_mm", "Largest hole radius", 30.0, 0.1, 500.0).unit("mm").help("Concave cylinders up to this radius are reported as holes."))
            .param(ParamSpec::float("sensitivity", "Sensitivity", 55.0, 0.0, 100.0).help("Used only when the input has no region attributes yet."))
            .help("Fits a plane, sphere or cylinder to every region of a segmented mesh and lists the analytic faces with their residuals. Concave cylinders below the hole radius limit are flagged as holes. Regions without a good fit are listed as freeform.")
            .emulates("primitive extraction in reverse-engineering tools")
            .emulates("RANSAC shape detection")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let geo = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &geo)?;
        let tol = positive(ctx, "tol_mm", params.float("tol_mm")?)?;
        let hole_cap = positive(ctx, "max_hole_radius_mm", params.float("max_hole_radius_mm")?)?;
        let sm = seg_mesh(&geo);
        if sm.mesh.tris.is_empty() {
            return Err(ctx.fail("the input has no faces"));
        }
        let sens = params.float("sensitivity")?.clamp(0.0, 100.0);
        let sp = SegParams { sensitivity: sens, noise_sigma: tol / (4.0 - 3.0 * sens / 100.0), min_area_frac: 0.0005, cad_tessellation: false };
        let mut report = Report::new("Primitive fit");
        let (seg, ids) = match labels_from_geo(&geo, &sm) {
            Some(l) => {
                let mut ids: Vec<i64> = l.iter().copied().filter(|&x| x >= 0).collect();
                ids.sort_unstable();
                ids.dedup();
                // a user tolerance overrides the segmentation tolerance
                (classify_labels(&sm.mesh, &l, &sp), ids)
            }
            None => {
                report.note(Severity::Info, "no region attributes found; segmented with the given sensitivity");
                let s = segment(&sm.mesh, &sp);
                let ids = (0..s.regions.len() as i64).collect();
                (s, ids)
            }
        };
        let g = Geo::new(&sm.mesh);
        let mut faces = Vec::new();
        let (mut holes, mut prims, mut worst) = (0usize, 0usize, 0.0f64);
        for (k, reg) in seg.regions.iter().enumerate() {
            let mut entry = json!({"region_id": ids[k], "type": reg.prim.kind(), "area_mm2": reg.area, "triangles": reg.tris.len(), "rms_mm": reg.rms, "max_mm": reg.max_dev});
            match &reg.prim {
                Primitive::Plane { origin, normal } => {
                    entry["origin"] = json!(origin.to_array());
                    entry["normal"] = json!(normal.to_array());
                }
                Primitive::Sphere { center, radius } => {
                    entry["center"] = json!(center.to_array());
                    entry["radius"] = json!(radius);
                }
                Primitive::Cylinder { origin, axis, radius } => {
                    // concave when the face normals point at the axis
                    let (mut inward, mut total) = (0.0, 0.0);
                    for &t in &reg.tris {
                        let tri = g.tris[t as usize];
                        let c = (g.pts[tri[0] as usize] + g.pts[tri[1] as usize] + g.pts[tri[2] as usize]) / 3.0;
                        let d = c - *origin;
                        let radial = (d - *axis * d.dot(*axis)).normalize_or_zero();
                        let w = g.areas[t as usize];
                        total += w;
                        if g.normals[t as usize].dot(radial) < 0.0 {
                            inward += w;
                        }
                    }
                    let is_hole = *radius <= hole_cap && inward > 0.5 * total;
                    entry["origin"] = json!(origin.to_array());
                    entry["axis"] = json!(axis.to_array());
                    entry["radius"] = json!(radius);
                    entry["hole"] = json!(is_hole);
                    if is_hole {
                        holes += 1;
                    }
                }
                Primitive::Freeform => {}
            }
            if reg.prim != Primitive::Freeform {
                prims += 1;
                worst = worst.max(reg.rms);
            }
            faces.push(entry);
        }
        report.metric("region_count", Metric::new(seg.regions.len() as f64, "count"));
        report.metric("primitive_count", Metric::new(prims as f64, "count"));
        report.metric("hole_count", Metric::new(holes as f64, "count"));
        report.metric("freeform_count", Metric::new((seg.regions.len() - prims) as f64, "count"));
        report.metric("fit_rms_max_mm", Metric::new(worst, "mm").with_limit(tol, true));
        let doc = json!({"schema": "analytic_faces", "version": 1, "units": "mm", "faces": faces});
        Ok(vec![Data::json(doc), Data::report(report)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{cook, f, geo_of, icosphere, report_of, s, some, tube, cylinder, filleted_box, b};

    fn types(g: &Geometry) -> BTreeMap<String, usize> {
        let mut seen = BTreeMap::new();
        let ids = g.prim_attribs.ints("region_id").unwrap();
        let ty = g.prim_attribs.strs("region_type").unwrap();
        let mut done = std::collections::HashSet::new();
        for (i, id) in ids.iter().enumerate() {
            if done.insert(*id) {
                *seen.entry(ty[i].clone()).or_insert(0) += 1;
            }
        }
        seen
    }

    #[test]
    fn cylinder_gives_two_planes_and_one_cylinder() {
        // spec case 1: r = 25, h = 100, 112 segments
        let g = cylinder(25.0, 100.0, 112);
        let out = cook("surface.segment_regions", &[some(&g)], &[]).unwrap();
        let seg = geo_of(&out[0]);
        let t = types(&seg);
        assert_eq!(t.get("plane"), Some(&2), "{t:?}");
        assert_eq!(t.get("cylinder"), Some(&1), "{t:?}");
        assert_eq!(report_of(&out[1]).metrics["region_count"].value, 3.0);
        let prims = cook("surface.fit_primitives", &[Some(out[0].clone())], &[]).unwrap();
        let doc = prims[0].as_json().unwrap();
        let cyl = doc["faces"].as_array().unwrap().iter().find(|f| f["type"] == "cylinder").unwrap();
        assert!((cyl["radius"].as_f64().unwrap() - 25.0).abs() < 0.01, "{}", cyl["radius"]);
        let axis: Vec<f64> = cyl["axis"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        let ang = DVec3::new(axis[0], axis[1], axis[2]).dot(DVec3::Z).abs().clamp(0.0, 1.0).acos().to_degrees();
        assert!(ang < 0.05, "axis error {ang}");
    }

    #[test]
    fn icosphere_is_one_sphere() {
        // spec case 4
        let g = icosphere(50.0, 4);
        let out = cook("surface.segment_regions", &[some(&g)], &[]).unwrap();
        let seg = geo_of(&out[0]);
        assert_eq!(types(&seg).get("sphere"), Some(&1), "{:?}", types(&seg));
        let prims = cook("surface.fit_primitives", &[Some(out[0].clone())], &[]).unwrap();
        let r = prims[0].as_json().unwrap()["faces"][0]["radius"].as_f64().unwrap();
        assert!((r - 50.0).abs() < 0.001, "radius {r}");
    }

    #[test]
    fn filleted_box_gives_26_regions() {
        // spec case 2: planes + edge fillets + corner spheres
        let g = filleted_box(40.0, 5.0, 8);
        let out = cook("surface.segment_regions", &[some(&g)], &[]).unwrap();
        let seg = geo_of(&out[0]);
        let t = types(&seg);
        assert_eq!(t.get("plane"), Some(&6), "{t:?}");
        assert_eq!(t.get("cylinder"), Some(&12), "{t:?}");
        assert_eq!(t.get("sphere"), Some(&8), "{t:?}");
    }

    #[test]
    fn cad_mode_fits_exact_radii() {
        let g = cylinder(25.0, 100.0, 112);
        let out = cook("surface.segment_regions", &[some(&g)], &[("cad_tessellation", b(true))]).unwrap();
        assert_eq!(report_of(&out[1]).metrics["region_count"].value, 3.0);
        let prims = cook("surface.fit_primitives", &[Some(out[0].clone())], &[]).unwrap();
        let doc = prims[0].as_json().unwrap();
        let cyl = doc["faces"].as_array().unwrap().iter().find(|f| f["type"] == "cylinder").unwrap();
        assert!((cyl["radius"].as_f64().unwrap() - 25.0).abs() < 1e-6, "{}", cyl["radius"]);
    }

    #[test]
    fn through_hole_is_detected() {
        // spec case 3: 64-segment hole r = 5 in a plate-like part
        let g = tube(25.0, 5.0, 10.0, 64);
        let out = cook("surface.segment_regions", &[some(&g)], &[]).unwrap();
        let prims = cook("surface.fit_primitives", &[Some(out[0].clone())], &[]).unwrap();
        let rep = report_of(&prims[1]);
        assert_eq!(rep.metrics["hole_count"].value, 1.0);
        let doc = prims[0].as_json().unwrap();
        let hole = doc["faces"].as_array().unwrap().iter().find(|f| f["hole"] == true).unwrap();
        assert!((hole["radius"].as_f64().unwrap() - 5.0).abs() < 0.01);
        let axis: Vec<f64> = hole["axis"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert!(DVec3::new(axis[0], axis[1], axis[2]).dot(DVec3::Z).abs() > 0.999998);
    }

    #[test]
    fn refine_merge_and_resegment_edit_the_labels() {
        let g = filleted_box(40.0, 5.0, 8);
        let seg = cook("surface.segment_regions", &[some(&g)], &[("cad_tessellation", b(true))]).unwrap();
        let before = report_of(&seg[1]).metrics["region_count"].value;
        let merged = cook("surface.refine_regions", &[Some(seg[0].clone())], &[("op", s("merge")), ("group", s("0,1"))]).unwrap();
        assert_eq!(report_of(&merged[1]).metrics["region_count"].value, before - 1.0);
        let re = cook("surface.refine_regions", &[Some(seg[0].clone())], &[("op", s("resegment")), ("group", s("0")), ("sensitivity", f(95.0))]).unwrap();
        assert!(report_of(&re[1]).metrics["region_count"].value >= before);
        assert!(cook("surface.refine_regions", &[Some(seg[0].clone())], &[("op", s("split_by_polyline"))]).is_err());
        // no attribute: clear error
        assert!(cook("surface.refine_regions", &[some(&g)], &[]).is_err());
    }

    fn curved_grid() -> Geometry {
        let mut g = Geometry::new();
        for y in 0..9 { for x in 0..9 {
            g.points.push(DVec3::new(x as f64, y as f64, 0.02 * (x * x + y * y) as f64));
        }}
        for y in 0..8 { for x in 0..8 {
            let a = y * 9 + x;
            g.prims.push(vernon_core::Prim::polygon(vec![a, a + 1, a + 10, a + 9]));
        }}
        g.prim_attribs.set("region_id", AttribArray::Int(vec![7; 64]));
        g.prim_attribs.set("part", AttribArray::Int(vec![42; 64]));
        g
    }

    #[test]
    fn projected_polyline_splits_curved_faces_and_preserves_attributes() {
        let g = curved_grid();
        let mut line = Geometry::new();
        line.points = vec![DVec3::new(2.0, -1.0, 0.0), DVec3::new(2.0, 4.0, 0.4), DVec3::new(6.0, 9.0, 2.0)];
        line.prims.push(vernon_core::Prim::polyline(vec![0, 1, 2]));
        let args = [("op", s("split_by_polyline")), ("group", s("7"))];
        let a = cook("surface.refine_regions", &[some(&g), some(&line)], &args).unwrap();
        let b = cook("surface.refine_regions", &[some(&g), some(&line)], &args).unwrap();
        let out = geo_of(&a[0]);
        assert_eq!(out.prim_attribs.ints("region_id").unwrap(), geo_of(&b[0]).prim_attribs.ints("region_id").unwrap());
        assert_eq!(report_of(&a[1]).metrics["region_count"].value, 2.0);
        assert_eq!(out.prims, g.prims);
        assert_eq!(out.points, g.points);
        assert_eq!(out.prim_attribs.ints("part").unwrap(), &[42; 64]);
        let ids = out.prim_attribs.ints("region_id").unwrap();
        assert_eq!(ids[0], ids[56]);
        assert_ne!(ids[0], ids[7]);
        assert_ne!(ids[2], ids[58], "the bent cut must follow its supplied path");
        line.points = vec![DVec3::splat(100.0), DVec3::splat(110.0)];
        line.prims[0].vertices = vec![0, 1];
        assert!(cook("surface.refine_regions", &[some(&g), some(&line)], &args).is_err());
    }

    #[test]
    fn refine_rejects_invalid_region_selections() {
        let g = curved_grid();
        for group in ["7", "7,7", "7,99", "bad,7", "-1,7"] {
            assert!(cook("surface.refine_regions", &[some(&g)], &[("op", s("merge")), ("group", s(group))]).is_err(), "{group}");
        }
        assert!(cook("surface.refine_regions", &[some(&g)], &[("group", s("99"))]).is_err());
    }

    #[test]
    fn closed_cut_targets_only_selected_region_and_rejects_branches() {
        let mut g = curved_grid();
        let mut ids = vec![7; 64]; ids[0] = 9;
        g.prim_attribs.set("region_id", AttribArray::Int(ids));
        let mut line = Geometry::new();
        line.points = vec![DVec3::new(2.0,2.0,0.2), DVec3::new(6.0,2.0,0.8), DVec3::new(6.0,6.0,1.4), DVec3::new(2.0,6.0,0.8)];
        line.prims.push(vernon_core::Prim::polyline(vec![0,1,2,3,0]));
        let args = [("op", s("split_by_polyline")), ("group", s("7"))];
        let out = cook("surface.refine_regions", &[some(&g), some(&line)], &args).unwrap();
        assert_eq!(geo_of(&out[0]).prim_attribs.ints("region_id").unwrap()[0], 9);
        assert!(report_of(&out[1]).metrics["region_count"].value >= 3.0);
        line.prims.push(vernon_core::Prim::polyline(vec![0,2]));
        let err = cook("surface.refine_regions", &[some(&g), some(&line)], &args).unwrap_err();
        assert!(err.to_string().contains("branches"));
    }

    #[test]
    fn curvature_resegment_is_deterministic_and_separates_disconnected_faces() {
        let mut g = curved_grid();
        let a = g.points.len() as u32;
        g.points.extend([DVec3::new(20.0,0.0,0.0),DVec3::new(21.0,0.0,0.0),DVec3::new(20.0,1.0,0.0)]);
        g.prims.push(vernon_core::Prim::polygon(vec![a,a+1,a+2]));
        g.prim_attribs.set("region_id", AttribArray::Int(vec![7;65]));
        let args = [("sensitivity", f(100.0)), ("noise_sigma_mm", f(0.0005)), ("min_region_area_pct", f(0.0))];
        let out = cook("surface.refine_regions", &[some(&g)], &args).unwrap();
        let again = cook("surface.refine_regions", &[some(&g)], &args).unwrap();
        let a = geo_of(&out[0]); let b = geo_of(&again[0]);
        let ids = a.prim_attribs.ints("region_id").unwrap();
        assert_eq!(ids, b.prim_attribs.ints("region_id").unwrap());
        assert!(ids[..64].iter().all(|&i| i != ids[64]));
        assert!(report_of(&out[1]).metrics["region_count"].value > 2.0, "curvature must refine the connected panel too");
    }

    #[test]
    fn tolerance_maps_follow_the_spec() {
        let p = SegParams { sensitivity: 55.0, noise_sigma: 0.05, min_area_frac: 0.0005, cad_tessellation: false };
        assert!((p.theta_deg() - 17.4).abs() < 1e-9);
        assert!((p.tau() - 0.05 * 2.35).abs() < 1e-12);
    }
}

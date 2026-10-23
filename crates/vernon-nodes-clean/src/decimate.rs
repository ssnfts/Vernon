//! Polygon reduction by quadric-error edge collapse (Garland and Heckbert,
//! 1997) with boundary and crease penalty planes.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use glam::{DMat3, DVec3};
use rayon::prelude::*;
use vernon_core::{Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::meshedit::{edge_stats, triangles_to_geometry, weld_triangles, EditMesh};
use crate::trimesh::{SignMode, TriMesh};
use crate::util::{add, ensure_metric, pf, pi, SPEC_MESH};

pub fn register(reg: &mut Registry) {
    add(reg, decimate_info, decimate_cook);
    add(reg, adaptive_info, adaptive_cook);
}

// ----------------------------------------------------------------- quadric

/// Symmetric 4x4 error quadric, upper triangle row by row.
#[derive(Clone, Copy, Default, Debug)]
pub struct Quadric(pub [f64; 10]);

impl Quadric {
    pub fn plane(n: DVec3, d: f64, w: f64) -> Self {
        let (a, b, c) = (n.x, n.y, n.z);
        let mut q = [a * a, a * b, a * c, a * d, b * b, b * c, b * d, c * c, c * d, d * d];
        for x in &mut q {
            *x *= w;
        }
        Quadric(q)
    }

    pub fn add(&mut self, o: &Quadric) {
        for i in 0..10 {
            self.0[i] += o.0[i];
        }
    }

    pub fn eval(&self, p: DVec3) -> f64 {
        let q = &self.0;
        q[0] * p.x * p.x
            + 2.0 * q[1] * p.x * p.y
            + 2.0 * q[2] * p.x * p.z
            + 2.0 * q[3] * p.x
            + q[4] * p.y * p.y
            + 2.0 * q[5] * p.y * p.z
            + 2.0 * q[6] * p.y
            + q[7] * p.z * p.z
            + 2.0 * q[8] * p.z
            + q[9]
    }

    /// Minimiser of the quadric when the 3x3 block is well conditioned.
    pub fn optimum(&self) -> Option<DVec3> {
        let q = &self.0;
        let a = DMat3::from_cols(
            DVec3::new(q[0], q[1], q[2]),
            DVec3::new(q[1], q[4], q[5]),
            DVec3::new(q[2], q[5], q[7]),
        );
        let tr = q[0] + q[4] + q[7];
        let det = a.determinant();
        if tr <= 0.0 || det.abs() <= 1e-9 * tr * tr * tr {
            return None;
        }
        Some(-(a.inverse() * DVec3::new(q[3], q[6], q[8])))
    }
}

#[derive(Clone, Copy)]
struct Cand {
    cost: f64,
    u: u32,
    v: u32,
    keep: u32,
    p: DVec3,
    vu: u32,
    vv: u32,
}

impl PartialEq for Cand {
    fn eq(&self, o: &Self) -> bool {
        self.cost == o.cost
    }
}
impl Eq for Cand {}
impl PartialOrd for Cand {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Cand {
    fn cmp(&self, o: &Self) -> Ordering {
        // BinaryHeap is a max-heap: reverse so the smallest cost pops first
        o.cost.total_cmp(&self.cost)
    }
}

/// Options of the collapse loop.
#[derive(Clone, Debug)]
pub struct QemOptions {
    pub target_faces: usize,
    /// Stop when the cheapest valid collapse costs more than this (summed
    /// squared plane distances, mm^2). `f64::INFINITY` = off.
    pub max_cost: f64,
    pub boundary_weight: f64,
    /// (crease angle in radians, weight) for sharp-edge penalty planes.
    pub crease: Option<(f64, f64)>,
    /// Smallest cosine between old and new triangle normals.
    pub min_normal_cos: f64,
}

pub struct QemOutput {
    pub mesh: EditMesh,
    pub collapses: usize,
    pub last_cost: f64,
    pub stopped_by_error: bool,
}

struct State {
    m: EditMesh,
    q: Vec<Quadric>,
    bnd: Vec<bool>,
    locked: Vec<bool>,
    ver: Vec<u32>,
}

impl State {
    fn eval_edge(&self, a: u32, b: u32) -> Option<Cand> {
        let (u, v) = if a < b { (a, b) } else { (b, a) };
        let (pu, pv) = (self.m.pos[u as usize], self.m.pos[v as usize]);
        let (lu, lv) = (self.locked[u as usize], self.locked[v as usize]);
        if lu && lv {
            return None;
        }
        let mut q = self.q[u as usize];
        q.add(&self.q[v as usize]);
        let (bu, bv) = (self.bnd[u as usize], self.bnd[v as usize]);
        let mut cands: Vec<(u32, DVec3)> = Vec::with_capacity(4);
        if lu {
            cands.push((u, pu));
        } else if lv {
            cands.push((v, pv));
        } else if bu && bv {
            if self.m.edge_tris(u, v).len() != 1 {
                return None; // would pinch two boundary parts together
            }
            cands.extend([(u, pu), (v, pv), (u, (pu + pv) * 0.5)]);
        } else if bu {
            cands.push((u, pu));
        } else if bv {
            cands.push((v, pv));
        } else {
            let mid = (pu + pv) * 0.5;
            if let Some(o) = q.optimum() {
                if (o - mid).length() <= 3.0 * (pv - pu).length().max(1e-12) {
                    cands.push((u, o));
                }
            }
            cands.extend([(u, pu), (v, pv), (u, mid)]);
        }
        let mut best: Option<(f64, u32, DVec3)> = None;
        for (keep, p) in cands {
            let c = q.eval(p).max(0.0);
            match best {
                Some(b) if b.0 <= c => {}
                _ => best = Some((c, keep, p)),
            }
        }
        best.map(|(cost, keep, p)| Cand {
            cost,
            u,
            v,
            keep,
            p,
            vu: self.ver[u as usize],
            vv: self.ver[v as usize],
        })
    }
}

/// Collapse edges of `pos`/`tris` until the face target or error limit is
/// reached.
pub fn qem_decimate(pos: Vec<DVec3>, tris: Vec<[u32; 3]>, opt: &QemOptions) -> QemOutput {
    let nv = pos.len();
    let m = EditMesh::new(pos, tris);
    let mut st = State { q: vec![Quadric::default(); nv], bnd: vec![false; nv], locked: vec![false; nv], ver: vec![0; nv], m };

    // vertex quadrics from face planes
    for t in 0..st.m.tris.len() {
        let n = st.m.raw_normal(t as u32);
        let len = n.length();
        if len < 1e-18 {
            continue;
        }
        let n = n / len;
        let tri = st.m.tris[t];
        let d = -n.dot(st.m.pos[tri[0] as usize]);
        let qd = Quadric::plane(n, d, 1.0);
        for v in tri {
            st.q[v as usize].add(&qd);
        }
    }

    // edge incidence -> boundary, crease and non-manifold handling
    let mut ed: Vec<(u32, u32, u32)> = Vec::with_capacity(st.m.tris.len() * 3);
    for (t, tri) in st.m.tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            ed.push((a.min(b), a.max(b), t as u32));
        }
    }
    ed.sort_unstable();
    let cos_crease = opt.crease.map(|(a, _)| a.cos());
    let mut edges: Vec<(u32, u32)> = Vec::new();
    let mut i = 0;
    while i < ed.len() {
        let mut j = i;
        while j < ed.len() && ed[j].0 == ed[i].0 && ed[j].1 == ed[i].1 {
            j += 1;
        }
        let (u, v) = (ed[i].0, ed[i].1);
        edges.push((u, v));
        let cnt = j - i;
        let (pu, pv) = (st.m.pos[u as usize], st.m.pos[v as usize]);
        let dir = (pv - pu).normalize_or_zero();
        let penalty = |t: u32, w: f64, st: &mut State| {
            let n = st.m.raw_normal(t).normalize_or_zero();
            let pn = dir.cross(n).normalize_or_zero();
            if pn == DVec3::ZERO {
                return;
            }
            let qd = Quadric::plane(pn, -pn.dot(pu), w);
            st.q[u as usize].add(&qd);
            st.q[v as usize].add(&qd);
        };
        match cnt {
            1 => {
                st.bnd[u as usize] = true;
                st.bnd[v as usize] = true;
                penalty(ed[i].2, opt.boundary_weight, &mut st);
            }
            2 => {
                if let (Some(c), Some((_, w))) = (cos_crease, opt.crease) {
                    let (a, b) = (st.m.raw_normal(ed[i].2).normalize_or_zero(), st.m.raw_normal(ed[i + 1].2).normalize_or_zero());
                    if a.dot(b) < c {
                        penalty(ed[i].2, w, &mut st);
                        penalty(ed[i + 1].2, w, &mut st);
                    }
                }
            }
            _ => {
                st.locked[u as usize] = true;
                st.locked[v as usize] = true;
            }
        }
        i = j;
    }

    let cands: Vec<Cand> = edges.par_iter().filter_map(|&(u, v)| st.eval_edge(u, v)).collect();
    let mut heap: BinaryHeap<Cand> = cands.into_iter().collect();

    let mut collapses = 0usize;
    let mut last_cost = 0.0;
    let mut stopped_by_error = false;
    while st.m.n_alive > opt.target_faces {
        let Some(c) = heap.pop() else { break };
        let (u, v) = (c.u as usize, c.v as usize);
        if st.m.vdead[u] || st.m.vdead[v] || st.ver[u] != c.vu || st.ver[v] != c.vv {
            continue;
        }
        if c.cost > opt.max_cost {
            stopped_by_error = true;
            break;
        }
        if !st.m.link_ok(c.u, c.v) || st.m.collapse_flips(c.u, c.v, c.p, opt.min_normal_cos) {
            continue;
        }
        let (keep, remove) = if c.keep == c.u { (c.u, c.v) } else { (c.v, c.u) };
        let qr = st.q[remove as usize];
        st.q[keep as usize].add(&qr);
        st.bnd[keep as usize] |= st.bnd[remove as usize];
        st.locked[keep as usize] |= st.locked[remove as usize];
        st.m.collapse(keep, remove, c.p);
        st.ver[keep as usize] += 1;
        collapses += 1;
        last_cost = c.cost;
        for w in st.m.neighbors(keep) {
            if let Some(nc) = st.eval_edge(keep, w) {
                heap.push(nc);
            }
        }
    }
    QemOutput { mesh: st.m, collapses, last_cost, stopped_by_error }
}

// -------------------------------------------------------------------- nodes

fn mesh_in() -> PortSpec {
    PortSpec::new("mesh", DataKind::Geometry)
}
fn geo_out() -> PortSpec {
    PortSpec::new("geometry", DataKind::Geometry)
}
fn rep_out() -> PortSpec {
    PortSpec::new("report", DataKind::Report)
}

fn common_params(info: NodeTypeInfo) -> NodeTypeInfo {
    info.param(
        ParamSpec::float("boundary_weight", "Border weight", 100.0, 0.0, 10000.0)
            .help("Strength of the penalty that keeps open borders in place. 0 lets borders erode."),
    )
    .param(
        ParamSpec::float("weld_tolerance", "Weld distance", 0.0, 0.0, 10.0)
            .unit("mm")
            .help("Points closer than this are merged before reducing. 0 merges exact duplicates only."),
    )
    .param(ParamSpec::bool("measure", "Measure deviation", true).help("Measure the distance from the reduced mesh back to the input and report it."))
}

fn decimate_info() -> NodeTypeInfo {
    common_params(
        NodeTypeInfo::new("clean.decimate", "Reduce Polygons", Stage::Clean)
            .input(mesh_in())
            .output(geo_out())
            .output(rep_out())
            .param(
                ParamSpec::choice("mode", "Target", "ratio", &["ratio", "face_count", "max_error"])
                    .help("ratio: keep a fraction of the faces. face_count: keep a fixed number. max_error: collapse until the error limit is reached."),
            )
            .param(ParamSpec::float("ratio", "Keep ratio", 0.5, 0.001, 1.0).help("Fraction of faces to keep in ratio mode."))
            .param(ParamSpec::int("face_count", "Face count", 10000, 4, 100_000_000).help("Target number of faces in face_count mode."))
            .param(
                ParamSpec::float("max_error", "Error limit", 0.0, 0.0, 1000.0)
                    .unit("mm")
                    .help("Stop when the cheapest collapse would exceed this plane-distance error. 0 = off (required above 0 in max_error mode)."),
            ),
    )
    .help("Reduces the triangle count by collapsing the edges that change the shape least (quadric error metric). Open borders are protected by a penalty.")
    .emulates("quadric-error edge-collapse polygon reduction (Garland and Heckbert 1997), as in mesh reduction tools")
    .spec(SPEC_MESH)
    .implemented()
}

fn adaptive_info() -> NodeTypeInfo {
    common_params(
        NodeTypeInfo::new("clean.decimate_adaptive", "Adaptive Polygon Reduction", Stage::Clean)
            .input(mesh_in())
            .output(geo_out())
            .output(rep_out())
            .param(ParamSpec::float("reduction", "Reduction", 50.0, 0.0, 99.9).unit("%").help("Percentage of faces to remove."))
            .param(
                ParamSpec::float("crease_angle_deg", "Hard-edge angle", 30.0, 1.0, 179.0)
                    .unit("deg")
                    .help("Edges whose faces meet at a sharper angle than this are protected."),
            )
            .param(ParamSpec::float("crease_weight", "Hard-edge weight", 20.0, 0.0, 10000.0).help("Strength of the hard-edge penalty."))
            .param(
                ParamSpec::float("max_error", "Error limit", 0.0, 0.0, 1000.0)
                    .unit("mm")
                    .help("Stop early when the cheapest collapse would exceed this plane-distance error. 0 = off."),
            ),
    )
    .help("Polygon reduction that keeps hard edges: flat regions lose most faces, curved and sharp regions keep them.")
    .emulates("polygon optimization of a watertight-wrap tool: adaptive vertex collapse that preserves hard edges")
    .spec(SPEC_MESH)
    .implemented()
}

fn decimate_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    let mode = p.str("mode")?.to_string();
    let ratio = pf(p, "ratio", 0.001, 1.0)?;
    let faces = pi(p, "face_count", 4, 100_000_000)? as usize;
    let max_error = pf(p, "max_error", 0.0, 1000.0)?;
    if mode == "max_error" && max_error <= 0.0 {
        return Err(ctx.fail("max_error mode needs an error limit above 0"));
    }
    let bw = pf(p, "boundary_weight", 0.0, 10000.0)?;
    let tol = pf(p, "weld_tolerance", 0.0, 10.0)?;
    let measure = p.bool("measure")?;
    let n_in = g.triangles().len();
    let target = match mode.as_str() {
        "face_count" => faces,
        "max_error" => 4,
        _ => ((n_in as f64) * ratio).round() as usize,
    };
    let opt = QemOptions {
        target_faces: target.max(4),
        max_cost: if max_error > 0.0 { max_error * max_error } else { f64::INFINITY },
        boundary_weight: bw,
        crease: None,
        min_normal_cos: 0.2,
    };
    reduce(ctx, &g, opt, tol, measure, "Reduce Polygons")
}

fn adaptive_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    let red = pf(p, "reduction", 0.0, 99.9)?;
    let ang = pf(p, "crease_angle_deg", 1.0, 179.0)?;
    let cw = pf(p, "crease_weight", 0.0, 10000.0)?;
    let max_error = pf(p, "max_error", 0.0, 1000.0)?;
    let bw = pf(p, "boundary_weight", 0.0, 10000.0)?;
    let tol = pf(p, "weld_tolerance", 0.0, 10.0)?;
    let measure = p.bool("measure")?;
    let n_in = g.triangles().len();
    let target = ((n_in as f64) * (1.0 - red / 100.0)).round() as usize;
    let opt = QemOptions {
        target_faces: target.max(4),
        max_cost: if max_error > 0.0 { max_error * max_error } else { f64::INFINITY },
        boundary_weight: bw,
        crease: Some((ang.to_radians(), cw)),
        min_normal_cos: 0.2,
    };
    reduce(ctx, &g, opt, tol, measure, "Adaptive Polygon Reduction")
}

fn reduce(ctx: &CookContext, g: &Geometry, opt: QemOptions, tol: f64, measure: bool, title: &str) -> CookResult<Vec<Data>> {
    ensure_metric(ctx, g)?;
    let w = weld_triangles(g, tol);
    if w.tris.is_empty() {
        return Err(ctx.fail("input has no triangles; connect a mesh"));
    }
    let n_in = w.tris.len();
    let before = edge_stats(&w.tris);
    // centre the coordinates so large offsets do not eat precision
    let c = g.bounds().center();
    let pos: Vec<DVec3> = w.pos.iter().map(|p| *p - c).collect();
    let out = qem_decimate(pos, w.tris.clone(), &opt);
    let (mut npos, ntris, old) = out.mesh.compact();
    for p in &mut npos {
        *p += c;
    }
    let after = edge_stats(&ntris);

    let mut geo = triangles_to_geometry(npos.clone(), &ntris);
    let idx: Vec<usize> = old.iter().map(|&o| w.first[o as usize]).collect();
    geo.point_attribs = g.point_attribs.gather_all(&idx);
    geo.detail_attribs = g.detail_attribs.clone();
    geo.compute_normals();

    let mut rep = Report::new(title);
    rep.metric("input_faces", Metric::new(n_in as f64, "faces"));
    rep.metric("output_faces", Metric::new(ntris.len() as f64, "faces"));
    rep.metric("reduction", Metric::new(100.0 * (1.0 - ntris.len() as f64 / n_in as f64), "%"));
    rep.metric("collapses", Metric::new(out.collapses as f64, "edges"));
    rep.metric("open_edges_in", Metric::new(before.open_edges as f64, "edges"));
    rep.metric("open_edges_out", Metric::new(after.open_edges as f64, "edges"));
    if out.stopped_by_error {
        rep.note(Severity::Info, "stopped at the error limit before reaching the face target");
    } else if ntris.len() > opt.target_faces + opt.target_faces / 20 + 2 {
        rep.note(Severity::Warning, format!("stopped at {} faces; further collapses would fold or pinch the mesh", ntris.len()));
    }
    if after.open_edges > before.open_edges {
        rep.note(Severity::Warning, "reduction opened new holes");
    }
    if measure {
        let reference = TriMesh::new(w.pos.clone(), w.tris.clone(), vec![0; n_in]);
        let step = (npos.len() / 200_000).max(1);
        let ds: Vec<f64> = (0..npos.len())
            .step_by(step)
            .collect::<Vec<_>>()
            .par_iter()
            .map(|&i| reference.closest(npos[i], f64::INFINITY, SignMode::None).map_or(0.0, |h| h.dist))
            .collect();
        let max = ds.iter().cloned().fold(0.0, f64::max);
        let mean = ds.iter().sum::<f64>() / ds.len().max(1) as f64;
        rep.metric("max_deviation", Metric::new(max, "mm"));
        rep.metric("mean_deviation", Metric::new(mean, "mm"));
    }
    Ok(vec![Data::geometry(geo), Data::report(rep)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;

    fn out_tris(g: &Geometry) -> Vec<[u32; 3]> {
        g.triangles().into_iter().map(|t| t.1).collect()
    }

    #[test]
    fn halves_a_sphere_and_stays_closed_and_outward() {
        let g = sphere_geo(50.0, 50, 51);
        let n0 = g.triangles().len();
        let out = run(decimate_info, decimate_cook, vec![geo(g)], &[("ratio", f(0.5))]).unwrap();
        let o = out_geo(&out[0]);
        let tris = out_tris(o);
        let target = n0 as f64 * 0.5;
        assert!((tris.len() as f64 - target).abs() / target < 0.02, "{} vs {}", tris.len(), target);
        let st = edge_stats(&tris);
        assert_eq!((st.open_edges, st.nonmanifold_edges, st.inconsistent_edges), (0, 0, 0));
        // V - E + F = 2
        let mut edges = std::collections::HashSet::new();
        for t in &tris {
            for k in 0..3 {
                edges.insert((t[k].min(t[(k + 1) % 3]), t[k].max(t[(k + 1) % 3])));
            }
        }
        assert_eq!(o.points.len() as i64 - edges.len() as i64 + tris.len() as i64, 2);
        // every face still looks outward
        for t in &tris {
            let (a, b, c) = (o.points[t[0] as usize], o.points[t[1] as usize], o.points[t[2] as usize]);
            let n = (b - a).cross(c - a);
            assert!(n.dot((a + b + c) / 3.0) > 0.0);
        }
        let r = out_report(&out[1]);
        assert!(r.metrics["max_deviation"].value < 1.0, "{}", r.metrics["max_deviation"].value);
        assert_eq!(r.metrics["open_edges_out"].value, 0.0);
        assert!(o.validate().is_ok());
    }

    #[test]
    fn open_border_is_preserved() {
        // flat 20x20 quad grid, 1 mm cells
        let n = 20usize;
        let mut pts = vec![];
        for j in 0..=n {
            for i in 0..=n {
                pts.push(DVec3::new(i as f64, j as f64, 0.0));
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
        let g = Geometry::from_polygons(pts, faces);
        let out = run(decimate_info, decimate_cook, vec![geo(g)], &[("ratio", f(0.1))]).unwrap();
        let o = out_geo(&out[0]);
        let b = o.bounds();
        assert!((b.min.x).abs() < 1e-9 && (b.max.x - 20.0).abs() < 1e-9 && (b.max.y - 20.0).abs() < 1e-9);
        assert!(o.points.iter().all(|p| p.z.abs() < 1e-9));
        assert!(o.triangles().len() <= 80);
        // the area is unchanged: the border did not erode
        assert!((o.surface_area() - 400.0).abs() < 1e-6);
    }

    #[test]
    fn error_limit_stops_early() {
        let g = sphere_geo(50.0, 40, 41);
        let n0 = g.triangles().len();
        let out = run(decimate_info, decimate_cook, vec![geo(g)], &[("mode", s("max_error")), ("max_error", f(0.05))]).unwrap();
        let o = out_geo(&out[0]);
        assert!(o.triangles().len() < n0);
        assert!(o.triangles().len() > 100);
        let r = out_report(&out[1]);
        assert!(r.metrics["max_deviation"].value < 0.5);
        assert!(run(decimate_info, decimate_cook, vec![geo(sphere_geo(10.0, 8, 9))], &[("mode", s("max_error"))]).is_err());
    }

    #[test]
    fn adaptive_keeps_a_sharp_ridge() {
        // 40x40 sheet folded along x = 20 by 90 degrees
        let n = 40usize;
        let mut pts = vec![];
        for j in 0..=n {
            for i in 0..=n {
                let x = i as f64;
                pts.push(DVec3::new(x, j as f64, -(x - 20.0).abs()));
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
        let g = Geometry::from_polygons(pts, faces);
        let out = run(adaptive_info, adaptive_cook, vec![geo(g)], &[("reduction", f(90.0))]).unwrap();
        let o = out_geo(&out[0]);
        assert!(o.triangles().len() <= 330);
        // every output point still lies on the folded surface
        for p in &o.points {
            assert!((p.z + (p.x - 20.0).abs()).abs() < 1e-6, "{p:?}");
        }
        assert!(out_report(&out[1]).metrics["max_deviation"].value < 1e-6);
    }

    #[test]
    fn point_cloud_is_a_clean_error() {
        let g = Geometry::from_points(vec![DVec3::ZERO, DVec3::X]);
        assert!(run(decimate_info, decimate_cook, vec![geo(g)], &[]).is_err());
    }

    #[test]
    fn quadric_optimum_of_three_planes_is_the_corner() {
        let mut q = Quadric::plane(DVec3::X, -1.0, 1.0);
        q.add(&Quadric::plane(DVec3::Y, -2.0, 1.0));
        q.add(&Quadric::plane(DVec3::Z, -3.0, 1.0));
        let o = q.optimum().unwrap();
        assert!((o - DVec3::new(1.0, 2.0, 3.0)).length() < 1e-12);
        assert!(q.eval(o).abs() < 1e-12);
        // one plane alone is singular
        assert!(Quadric::plane(DVec3::X, 0.0, 1.0).optimum().is_none());
    }

    #[test]
    #[ignore = "performance check: run with --release -- --ignored"]
    fn quarter_million_triangles_in_a_blink() {
        let g = sphere_geo(100.0, 500, 251);
        let n0 = g.triangles().len();
        let t = std::time::Instant::now();
        let out = run(decimate_info, decimate_cook, vec![geo(g)], &[("ratio", f(0.1)), ("measure", crate::testkit::b(false))]).unwrap();
        eprintln!("{} -> {} triangles in {:?}", n0, out_geo(&out[0]).triangles().len(), t.elapsed());
        assert!(out_geo(&out[0]).triangles().len() < n0 / 8);
    }
}

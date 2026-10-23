//! Isotropic triangle remeshing (Botsch and Kobbelt 2004): split long
//! edges, collapse short ones, flip toward valence six, relax tangentially
//! and re-project onto the input surface.

use glam::DVec3;
use rayon::prelude::*;
use vernon_core::{Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::meshedit::{edge_stats, triangles_to_geometry, weld_triangles, EditMesh, VClass};
use crate::trimesh::{SignMode, TriMesh};
use crate::util::{add, ensure_metric, pf, pi, SPEC_MESH};

pub fn register(reg: &mut Registry) {
    add(reg, remesh_info, remesh_cook);
}

#[derive(Clone, Debug)]
pub struct RemeshOptions {
    pub target: f64,
    pub iterations: usize,
    /// Cosine of the crease angle; `-1.0` disables feature protection.
    pub cos_crease: f64,
    pub project: bool,
}

fn all_edges(m: &EditMesh) -> Vec<(u32, u32)> {
    let mut e: Vec<(u32, u32)> = Vec::with_capacity(m.n_alive * 2);
    for (t, tri) in m.tris.iter().enumerate() {
        if !m.alive[t] {
            continue;
        }
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            e.push((a.min(b), a.max(b)));
        }
    }
    e.sort_unstable();
    e.dedup();
    e
}

fn elen(m: &EditMesh, u: u32, v: u32) -> f64 {
    (m.pos[u as usize] - m.pos[v as usize]).length()
}

fn split_long(m: &mut EditMesh, hi: f64) -> usize {
    let mut total = 0;
    for _ in 0..8 {
        let mut long: Vec<(f64, u32, u32)> = all_edges(m)
            .into_iter()
            .filter_map(|(u, v)| {
                let l = elen(m, u, v);
                (l > hi).then_some((l, u, v))
            })
            .collect();
        if long.is_empty() {
            break;
        }
        long.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut n = 0;
        for (_, u, v) in long {
            if elen(m, u, v) <= hi {
                continue;
            }
            let mid = (m.pos[u as usize] + m.pos[v as usize]) * 0.5;
            if m.split_edge(u, v, mid).is_some() {
                n += 1;
            }
        }
        total += n;
        if n == 0 {
            break;
        }
    }
    total
}

/// A boundary vertex may be removed only where its border runs straight, so
/// corners and curved outlines survive.
fn border_is_straight(m: &EditMesh, v: u32) -> bool {
    let nb: Vec<u32> = m.neighbor_counts(v).into_iter().filter(|&(_, c)| c == 1).map(|(x, _)| x).collect();
    if nb.len() != 2 {
        return false;
    }
    let p = m.pos[v as usize];
    let (d1, d2) = ((p - m.pos[nb[0] as usize]).normalize_or_zero(), (m.pos[nb[1] as usize] - p).normalize_or_zero());
    d1.dot(d2) > 0.94
}

fn collapse_short(m: &mut EditMesh, lo: f64, hi: f64, cos_c: f64) -> usize {
    let mut short: Vec<(f64, u32, u32)> = all_edges(m)
        .into_iter()
        .filter_map(|(u, v)| {
            let l = elen(m, u, v);
            (l < lo).then_some((l, u, v))
        })
        .collect();
    short.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut n = 0;
    for (_, u, v) in short {
        if m.vdead[u as usize] || m.vdead[v as usize] || elen(m, u, v) >= lo {
            continue;
        }
        let ts = m.edge_tris(u, v);
        if ts.is_empty() || ts.len() > 2 || !m.link_ok(u, v) {
            continue;
        }
        let (cu, cv) = (m.classify(u, cos_c), m.classify(v, cos_c));
        let (keep, remove, p) = match (cu, cv) {
            (VClass::Free, VClass::Free) => (u, v, (m.pos[u as usize] + m.pos[v as usize]) * 0.5),
            (VClass::Free, _) => (v, u, m.pos[v as usize]),
            (_, VClass::Free) => (u, v, m.pos[u as usize]),
            (a, b) => {
                // both pinned: only slide along a border edge
                if a == VClass::Boundary && b == VClass::Boundary && ts.len() == 1 && border_is_straight(m, v) {
                    (u, v, m.pos[u as usize])
                } else {
                    continue;
                }
            }
        };
        let too_long = m
            .neighbors(u)
            .into_iter()
            .chain(m.neighbors(v))
            .filter(|&w| w != u && w != v)
            .any(|w| (m.pos[w as usize] - p).length() > hi);
        if too_long || m.collapse_flips(u, v, p, 0.1) {
            continue;
        }
        m.collapse(keep, remove, p);
        n += 1;
    }
    n
}

fn flip_pass(m: &mut EditMesh, cos_c: f64) -> usize {
    let mut flips = 0;
    for (u, v) in all_edges(m) {
        let ts = m.edge_tris(u, v);
        if ts.len() != 2 || m.is_fixed_edge(u, v, cos_c) {
            continue;
        }
        // t1 is the triangle that runs u -> v
        let runs_uv = |t: u32| {
            let tri = m.tris[t as usize];
            let i = tri.iter().position(|&x| x == u).unwrap();
            tri[(i + 1) % 3] == v
        };
        let (t1, t2) = if runs_uv(ts[0]) { (ts[0], ts[1]) } else { (ts[1], ts[0]) };
        let a = m.tris[t1 as usize].iter().copied().find(|&x| x != u && x != v).unwrap();
        let b = m.tris[t2 as usize].iter().copied().find(|&x| x != u && x != v).unwrap();
        let target = |x: u32| if m.classify(x, -1.0) == VClass::Boundary { 4i64 } else { 6 };
        let val = |x: u32| m.valence(x) as i64;
        let before: i64 = [u, v, a, b].iter().map(|&x| (val(x) - target(x)).pow(2)).sum();
        let after: i64 = [(u, -1), (v, -1), (a, 1), (b, 1)].iter().map(|&(x, d)| (val(x) + d - target(x)).pow(2)).sum();
        if after >= before {
            continue;
        }
        let n_old = m.raw_normal(t1) + m.raw_normal(t2);
        let (pu, pv, pa, pb) = (m.pos[u as usize], m.pos[v as usize], m.pos[a as usize], m.pos[b as usize]);
        // flip_edge replaces them by (a, u, b) and (a, b, v)
        let (n1, n2) = ((pu - pa).cross(pb - pa), (pb - pa).cross(pv - pa));
        let ok = |n: DVec3| n.dot(n_old) > 0.1 * n.length() * n_old.length();
        if !ok(n1) || !ok(n2) {
            continue;
        }
        if m.flip_edge(u, v) {
            flips += 1;
        }
    }
    flips
}

fn relax(m: &mut EditMesh, reference: Option<&TriMesh>, cos_c: f64) {
    let mut moves: Vec<(u32, DVec3)> = Vec::new();
    for v in 0..m.pos.len() as u32 {
        if m.vdead[v as usize] || m.vt[v as usize].is_empty() || m.classify(v, cos_c) != VClass::Free {
            continue;
        }
        let nb = m.neighbors(v);
        let mean = nb.iter().map(|&x| m.pos[x as usize]).sum::<DVec3>() / nb.len() as f64;
        let n = m.vertex_normal(v);
        let d = mean - m.pos[v as usize];
        let dt = d - n * n.dot(d);
        moves.push((v, m.pos[v as usize] + dt * 0.5));
    }
    if let Some(r) = reference {
        moves.par_iter_mut().for_each(|(_, p)| {
            if let Some(h) = r.closest(*p, f64::INFINITY, SignMode::None) {
                *p = h.point;
            }
        });
    }
    for (v, p) in moves {
        m.pos[v as usize] = p;
    }
}

/// Remesh to a uniform edge length. Returns positions and triangles.
pub fn isotropic_remesh(pos: Vec<DVec3>, tris: Vec<[u32; 3]>, opt: &RemeshOptions) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let reference = opt.project.then(|| TriMesh::new(pos.clone(), tris.clone(), vec![0; tris.len()]));
    let mut m = EditMesh::new(pos, tris);
    let (lo, hi) = (0.8 * opt.target, 4.0 / 3.0 * opt.target);
    for _ in 0..opt.iterations {
        split_long(&mut m, hi);
        collapse_short(&mut m, lo, hi, opt.cos_crease);
        for _ in 0..3 {
            if flip_pass(&mut m, opt.cos_crease) == 0 {
                break;
            }
        }
        relax(&mut m, reference.as_ref(), opt.cos_crease);
    }
    let (p, t, _) = m.compact();
    (p, t)
}

fn remesh_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.isotropic_remesh", "Uniform Triangle Remesh", Stage::Clean)
        .input(PortSpec::new("mesh", DataKind::Geometry))
        .output(PortSpec::new("geometry", DataKind::Geometry))
        .output(PortSpec::new("report", DataKind::Report))
        .param(
            ParamSpec::float("target_edge_length", "Edge length", 0.0, 0.0, 10000.0)
                .unit("mm")
                .help("Wanted edge length of the result. 0 = the mean edge length of the input."),
        )
        .param(ParamSpec::int("iterations", "Iterations", 6, 1, 50).help("Number of split, collapse, flip and relax rounds."))
        .param(ParamSpec::bool("keep_features", "Keep hard edges", true).help("Leave points on sharp edges in place and do not flip or collapse across them."))
        .param(ParamSpec::float("crease_angle_deg", "Hard-edge angle", 30.0, 1.0, 179.0).unit("deg").help("Edges whose faces meet at a sharper angle than this count as hard."))
        .param(ParamSpec::bool("project", "Stay on input surface", true).help("Pull relaxed points back onto the input surface after every round."))
        .param(
            ParamSpec::float("weld_tolerance", "Weld distance", 0.0, 0.0, 10.0)
                .unit("mm")
                .help("Points closer than this are merged first. 0 merges exact duplicates only."),
        )
        .help("Rebuilds a triangle mesh with nearly equal edge lengths and regular valence, staying close to the input surface.")
        .emulates("isotropic remeshing inside a watertight shrink-wrap tool (Botsch and Kobbelt 2004)")
        .spec(SPEC_MESH)
        .implemented()
}

fn remesh_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    ensure_metric(ctx, &g)?;
    let mut target = pf(p, "target_edge_length", 0.0, 10000.0)?;
    let iterations = pi(p, "iterations", 1, 50)? as usize;
    let keep = p.bool("keep_features")?;
    let ang = pf(p, "crease_angle_deg", 1.0, 179.0)?;
    let project = p.bool("project")?;
    let tol = pf(p, "weld_tolerance", 0.0, 10.0)?;
    let w = weld_triangles(&g, tol);
    if w.tris.is_empty() {
        return Err(ctx.fail("input has no triangles; connect a mesh"));
    }
    let mut lens = 0.0;
    for t in &w.tris {
        for k in 0..3 {
            lens += (w.pos[t[k] as usize] - w.pos[t[(k + 1) % 3] as usize]).length();
        }
    }
    let mean_in = lens / (3 * w.tris.len()) as f64;
    if target <= 0.0 {
        target = mean_in;
    }
    if target <= 0.0 || !target.is_finite() {
        return Err(ctx.fail("cannot derive an edge length from the input; set one"));
    }
    let opt = RemeshOptions { target, iterations, cos_crease: if keep { ang.to_radians().cos() } else { -1.0 }, project };
    let before = edge_stats(&w.tris);
    let n_in = w.tris.len();
    let (pos, tris) = isotropic_remesh(w.pos.clone(), w.tris.clone(), &opt);
    let after = edge_stats(&tris);
    let mut out: Geometry = triangles_to_geometry(pos, &tris);
    out.detail_attribs = g.detail_attribs.clone();
    out.compute_normals();

    // quality numbers
    let mut e = std::collections::HashSet::new();
    for t in &tris {
        for k in 0..3 {
            e.insert((t[k].min(t[(k + 1) % 3]), t[k].max(t[(k + 1) % 3])));
        }
    }
    let ls: Vec<f64> = e.iter().map(|&(a, b)| (out.points[a as usize] - out.points[b as usize]).length()).collect();
    let mean = ls.iter().sum::<f64>() / ls.len().max(1) as f64;
    let var = ls.iter().map(|l| (l - mean) * (l - mean)).sum::<f64>() / ls.len().max(1) as f64;
    let mut rep = Report::new("Uniform Triangle Remesh");
    rep.metric("input_faces", Metric::new(n_in as f64, "faces"));
    rep.metric("output_faces", Metric::new(tris.len() as f64, "faces"));
    rep.metric("target_edge_length", Metric::new(target, "mm"));
    rep.metric("mean_edge_length", Metric::new(mean, "mm"));
    rep.metric("edge_length_cv", Metric::new(if mean > 0.0 { var.sqrt() / mean } else { 0.0 }, "ratio"));
    rep.metric("open_edges_in", Metric::new(before.open_edges as f64, "edges"));
    rep.metric("open_edges_out", Metric::new(after.open_edges as f64, "edges"));
    if after.open_edges > before.open_edges {
        rep.note(Severity::Warning, "remeshing opened new holes; check the input for non-manifold edges");
    }
    if project {
        let reference = TriMesh::new(w.pos.clone(), w.tris.clone(), vec![0; n_in]);
        let step = (out.points.len() / 100_000).max(1);
        let idx: Vec<usize> = (0..out.points.len()).step_by(step).collect();
        let max_dev = idx
            .par_iter()
            .map(|&i| reference.closest(out.points[i], f64::INFINITY, SignMode::None).map_or(0.0, |h| h.dist))
            .reduce(|| 0.0, f64::max);
        rep.metric("max_deviation", Metric::new(max_dev, "mm"));
    }
    Ok(vec![Data::geometry(out), Data::report(rep)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;

    #[test]
    fn sphere_becomes_uniform_and_stays_on_the_surface() {
        let g = sphere_geo(50.0, 60, 61);
        let out = run(remesh_info, remesh_cook, vec![geo(g)], &[("target_edge_length", f(8.0))]).unwrap();
        let o = out_geo(&out[0]);
        let r = out_report(&out[1]);
        let mean = r.metrics["mean_edge_length"].value;
        assert!((mean - 8.0).abs() < 1.2, "mean edge {mean}");
        assert!(r.metrics["edge_length_cv"].value < 0.25, "cv {}", r.metrics["edge_length_cv"].value);
        assert_eq!(r.metrics["open_edges_out"].value, 0.0);
        let tris: Vec<_> = o.triangles().iter().map(|t| t.1).collect();
        let st = edge_stats(&tris);
        assert_eq!((st.open_edges, st.nonmanifold_edges, st.inconsistent_edges), (0, 0, 0));
        for p in &o.points {
            assert!((p.length() - 50.0).abs() < 1.0, "radius {}", p.length());
        }
        assert!(r.metrics["max_deviation"].value < 1.0);
        // far fewer faces than the 60 x 61 input
        assert!(o.triangles().len() < 6000 && o.triangles().len() > 200);
    }

    #[test]
    fn plate_keeps_its_outline() {
        let n = 10usize;
        let mut pts = vec![];
        for j in 0..=n {
            for i in 0..=n {
                pts.push(DVec3::new(i as f64 * 5.0, j as f64 * 5.0, 0.0));
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
        let out = run(remesh_info, remesh_cook, vec![geo(g)], &[("target_edge_length", f(10.0))]).unwrap();
        let o = out_geo(&out[0]);
        let b = o.bounds();
        assert!(b.min.x.abs() < 1e-9 && (b.max.x - 50.0).abs() < 1e-9 && (b.max.y - 50.0).abs() < 1e-9);
        assert!((o.surface_area() - 2500.0).abs() < 1e-6);
        assert!(o.points.iter().all(|p| p.z.abs() < 1e-9));
        assert!(o.triangles().len() < 200);
    }

    #[test]
    fn auto_edge_length_and_errors() {
        let out = run(remesh_info, remesh_cook, vec![geo(sphere_geo(20.0, 16, 17))], &[("iterations", i(2))]).unwrap();
        assert!(out_report(&out[1]).metrics["target_edge_length"].value > 1.0);
        assert!(run(remesh_info, remesh_cook, vec![geo(Geometry::from_points(vec![DVec3::ZERO]))], &[]).is_err());
    }
}

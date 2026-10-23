//! Mesh repair nodes: boundary hole filling (minimum-weight triangulation
//! with optional refinement and fairing) and Laplacian / Taubin smoothing.

use std::collections::{HashMap, HashSet};

use glam::DVec3;
use vernon_core::{AttribArray, Geometry, Metric, Prim, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::meshedit::{edge_flags, edge_stats, signed_volume, weld_triangles, EditMesh};
use crate::util::{add, ensure_metric, pf, pi, SPEC_MESH};

pub fn register(reg: &mut Registry) {
    add(reg, fill_info, fill_cook);
    add(reg, smooth_info, smooth_cook);
}

fn mesh_in() -> PortSpec {
    PortSpec::new("mesh", DataKind::Geometry)
}
fn geo_out() -> PortSpec {
    PortSpec::new("geometry", DataKind::Geometry)
}
fn rep_out() -> PortSpec {
    PortSpec::new("report", DataKind::Report)
}

// ------------------------------------------------------------ hole filling

/// Boundary loops of an oriented triangle list, each as the vertex
/// sequence along the boundary half-edges (the mesh lies to the left).
pub fn boundary_loops(tris: &[[u32; 3]]) -> Vec<Vec<u32>> {
    let mut half: HashMap<(u32, u32), u32> = HashMap::with_capacity(tris.len() * 3);
    for t in tris {
        for k in 0..3 {
            *half.entry((t[k], t[(k + 1) % 3])).or_insert(0) += 1;
        }
    }
    let mut from: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&(a, b), &c) in &half {
        if c == 1 && !half.contains_key(&(b, a)) {
            from.entry(a).or_default().push(b);
        }
    }
    for v in from.values_mut() {
        v.sort_unstable();
    }
    let mut starts: Vec<u32> = from.keys().copied().collect();
    starts.sort_unstable();
    let mut loops = Vec::new();
    for s in starts {
        while let Some(first) = from.get_mut(&s).and_then(|v| v.pop()) {
            let mut lp = vec![s];
            let mut cur = first;
            let mut closed = false;
            while lp.len() <= tris.len() * 3 {
                if cur == s {
                    closed = true;
                    break;
                }
                lp.push(cur);
                match from.get_mut(&cur).and_then(|v| v.pop()) {
                    Some(n) => cur = n,
                    None => break,
                }
            }
            if closed && lp.len() >= 3 {
                loops.push(lp);
            }
        }
    }
    loops
}

fn angle_between(a: DVec3, b: DVec3) -> f64 {
    a.dot(b).clamp(-1.0, 1.0).acos()
}

/// Minimum-weight triangulation of a hole polygon (Liepa 2003 style): the
/// weight of a triangle is its area scaled by how much its normal departs
/// from the mesh triangle across each border edge it touches.
fn triangulate_hole(p: &[DVec3], ids: &[u32], border_n: &[DVec3], forbidden: &HashSet<(u32, u32)>) -> Option<Vec<[usize; 3]>> {
    let n = p.len();
    let allowed = |a: usize, b: usize| -> bool {
        if b == a + 1 || (a == 0 && b == n - 1) {
            return true;
        }
        let (x, y) = (ids[a].min(ids[b]), ids[a].max(ids[b]));
        !forbidden.contains(&(x, y))
    };
    let tri_cost = |i: usize, k: usize, j: usize| -> f64 {
        let cr = (p[k] - p[i]).cross(p[j] - p[i]);
        let area = cr.length() * 0.5;
        let nrm = cr.normalize_or_zero();
        let mut f = 1.0;
        let mut pen = |bn: DVec3| {
            if bn != DVec3::ZERO && nrm != DVec3::ZERO {
                let th = angle_between(nrm, bn);
                f += th * th;
            }
        };
        if k == i + 1 {
            pen(border_n[i]);
        }
        if j == k + 1 {
            pen(border_n[k]);
        }
        if i == 0 && j == n - 1 {
            pen(border_n[n - 1]);
        }
        area * f + 1e-12
    };
    let mut w = vec![f64::INFINITY; n * n];
    let mut sp = vec![u32::MAX; n * n];
    for i in 0..n - 1 {
        w[i * n + i + 1] = 0.0;
    }
    for len in 2..n {
        for i in 0..n - len {
            let j = i + len;
            let mut best = f64::INFINITY;
            let mut bk = u32::MAX;
            for k in i + 1..j {
                if (k - i >= 2 && !allowed(i, k)) || (j - k >= 2 && !allowed(k, j)) {
                    continue;
                }
                let (a, b) = (w[i * n + k], w[k * n + j]);
                if !a.is_finite() || !b.is_finite() {
                    continue;
                }
                let c = a + b + tri_cost(i, k, j);
                if c < best {
                    best = c;
                    bk = k as u32;
                }
            }
            w[i * n + j] = best;
            sp[i * n + j] = bk;
        }
    }
    if !w[n - 1].is_finite() {
        return None;
    }
    let mut out = Vec::with_capacity(n - 2);
    let mut stack = vec![(0usize, n - 1)];
    while let Some((i, j)) = stack.pop() {
        if j <= i + 1 {
            continue;
        }
        let k = sp[i * n + j] as usize;
        out.push([i, k, j]);
        stack.push((i, k));
        stack.push((k, j));
    }
    Some(out)
}

fn fill_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.fill_boundary_holes", "Fill Holes", Stage::Clean)
        .input(mesh_in())
        .output(geo_out())
        .output(rep_out())
        .param(
            ParamSpec::int("max_hole_edges", "Max hole edges", 300, 3, 600)
                .help("Holes whose border has more edges than this are left open."),
        )
        .param(
            ParamSpec::float("max_hole_diameter", "Max hole size", 0.0, 0.0, 100000.0)
                .unit("mm")
                .help("Holes wider than this (bounding-box diagonal of the border) are left open. 0 = off."),
        )
        .param(ParamSpec::bool("refine", "Refine patch", true).help("Add vertices inside large holes so the patch matches the surrounding triangle size."))
        .param(
            ParamSpec::float("target_edge", "Patch edge length", 0.0, 0.0, 10000.0)
                .unit("mm")
                .help("Edge length of the refined patch. 0 = use the surrounding edge length."),
        )
        .param(ParamSpec::int("fair_iterations", "Fairing iterations", 20, 0, 500).help("Smoothing passes that relax the new vertices toward their neighbours, with the hole border fixed."))
        .param(
            ParamSpec::float("weld_tolerance", "Weld distance", 0.0, 0.0, 10.0)
                .unit("mm")
                .help("Points closer than this are merged first so seams do not look like holes. 0 merges exact duplicates only."),
        )
        .help("Closes the open borders of a triangle mesh with a minimum-weight triangulation, optionally refined and relaxed. Output is a triangle mesh.")
        .emulates("fill-holes option of a watertight shrink-wrap tool; minimum-weight hole triangulation (Liepa 2003)")
        .spec(SPEC_MESH)
        .implemented()
}

fn fill_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    ensure_metric(ctx, &g)?;
    let max_edges = pi(p, "max_hole_edges", 3, 600)? as usize;
    let max_diam = pf(p, "max_hole_diameter", 0.0, 100000.0)?;
    let refine = p.bool("refine")?;
    let target_edge = pf(p, "target_edge", 0.0, 10000.0)?;
    let fair_iters = pi(p, "fair_iterations", 0, 500)? as usize;
    let tol = pf(p, "weld_tolerance", 0.0, 10.0)?;

    let w = weld_triangles(&g, tol);
    if w.tris.is_empty() {
        return Err(ctx.fail("input has no triangles; connect a mesh"));
    }
    let loops = boundary_loops(&w.tris);
    let before = edge_stats(&w.tris);
    let mut mesh_edges: HashSet<(u32, u32)> = HashSet::new();
    let mut half: HashMap<(u32, u32), u32> = HashMap::new();
    for (ti, t) in w.tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            mesh_edges.insert((a.min(b), a.max(b)));
            half.insert((a, b), ti as u32);
        }
    }
    let tri_normal = |t: u32| {
        let v = w.tris[t as usize];
        let (a, b, c) = (w.pos[v[0] as usize], w.pos[v[1] as usize], w.pos[v[2] as usize]);
        (b - a).cross(c - a).normalize_or_zero()
    };

    let n0_tris = w.tris.len();
    let mut m = EditMesh::new(w.pos.clone(), w.tris.clone());
    let n0_verts = m.pos.len();
    let mut filled = 0usize;
    let mut skipped_size = 0usize;
    let mut failed = 0usize;
    // average incident edge length per original vertex, the patch size field
    let mut sigma: Vec<f64> = (0..n0_verts as u32)
        .map(|v| {
            let nb = m.neighbors(v);
            if nb.is_empty() {
                0.0
            } else {
                nb.iter().map(|&x| (m.pos[x as usize] - m.pos[v as usize]).length()).sum::<f64>() / nb.len() as f64
            }
        })
        .collect();

    for lp in &loops {
        let n = lp.len();
        let pts: Vec<DVec3> = lp.iter().map(|&v| w.pos[v as usize]).collect();
        let diam = {
            let mut b = vernon_core::Aabb::empty();
            for q in &pts {
                b.grow(*q);
            }
            b.size().length()
        };
        if n > max_edges || (max_diam > 0.0 && diam > max_diam) {
            skipped_size += 1;
            continue;
        }
        // polygon = reversed loop so the fill winds opposite to the mesh
        let r_ids: Vec<u32> = lp.iter().rev().copied().collect();
        let r_pts: Vec<DVec3> = r_ids.iter().map(|&v| w.pos[v as usize]).collect();
        let border_n: Vec<DVec3> = (0..n)
            .map(|a| half.get(&(r_ids[(a + 1) % n], r_ids[a])).map_or(DVec3::ZERO, |&t| tri_normal(t)))
            .collect();
        match triangulate_hole(&r_pts, &r_ids, &border_n, &mesh_edges) {
            Some(tris) => {
                for t in tris {
                    m.add_tri(r_ids[t[0]], r_ids[t[1]], r_ids[t[2]]);
                }
                for k in 0..n {
                    mesh_edges.insert((r_ids[k].min(r_ids[(k + 1) % n]), r_ids[k].max(r_ids[(k + 1) % n])));
                }
                filled += 1;
            }
            None => failed += 1,
        }
    }

    // refinement of the patch (triangles with id >= n0_tris)
    if refine && filled > 0 {
        let alpha = std::f64::consts::SQRT_2;
        for _pass in 0..30 {
            let mut splits = 0usize;
            let ids: Vec<u32> = (n0_tris as u32..m.tris.len() as u32).filter(|&t| m.alive[t as usize]).collect();
            for t in ids {
                if !m.alive[t as usize] {
                    continue;
                }
                let tri = m.tris[t as usize];
                let pv = [m.pos[tri[0] as usize], m.pos[tri[1] as usize], m.pos[tri[2] as usize]];
                let c = (pv[0] + pv[1] + pv[2]) / 3.0;
                let sig_of = |v: u32| if target_edge > 0.0 { target_edge } else { sigma.get(v as usize).copied().unwrap_or(0.0) };
                let sc = if target_edge > 0.0 { target_edge } else { (sig_of(tri[0]) + sig_of(tri[1]) + sig_of(tri[2])) / 3.0 };
                if sc <= 0.0 {
                    continue;
                }
                let split = (0..3).all(|k| {
                    let d = alpha * (pv[k] - c).length();
                    d > sc && d > sig_of(tri[k])
                });
                if split {
                    let nv = m.split_tri(t, c);
                    if sigma.len() <= nv as usize {
                        sigma.resize(nv as usize + 1, sc);
                    }
                    sigma[nv as usize] = sc;
                    splits += 1;
                }
            }
            // Delaunay-style relaxation of interior patch edges
            for _ in 0..10 {
                let mut flips = 0usize;
                let ids: Vec<u32> = (n0_tris as u32..m.tris.len() as u32).filter(|&t| m.alive[t as usize]).collect();
                for t in ids {
                    if !m.alive[t as usize] {
                        continue;
                    }
                    let tri = m.tris[t as usize];
                    for k in 0..3 {
                        let (u, v) = (tri[k], tri[(k + 1) % 3]);
                        let ts = m.edge_tris(u, v);
                        if ts.len() != 2 || ts.iter().any(|&x| (x as usize) < n0_tris) {
                            continue;
                        }
                        let a = tri[(k + 2) % 3];
                        let other = if ts[0] == t { ts[1] } else { ts[0] };
                        let b = m.tris[other as usize].iter().copied().find(|&x| x != u && x != v).unwrap();
                        let ang = |apex: u32| {
                            let (e1, e2) = ((m.pos[u as usize] - m.pos[apex as usize]).normalize_or_zero(), (m.pos[v as usize] - m.pos[apex as usize]).normalize_or_zero());
                            angle_between(e1, e2)
                        };
                        if ang(a) + ang(b) <= std::f64::consts::PI + 1e-9 {
                            continue;
                        }
                        // flipped triangles must keep the local orientation
                        let n_old = m.raw_normal(t) + m.raw_normal(other);
                        let (pa, pb, pu, pvv) = (m.pos[a as usize], m.pos[b as usize], m.pos[u as usize], m.pos[v as usize]);
                        let n1 = (pu - pa).cross(pb - pa);
                        let n2 = (pb - pa).cross(pvv - pa);
                        if n1.dot(n_old) <= 0.0 || n2.dot(n_old) <= 0.0 {
                            continue;
                        }
                        if m.flip_edge(u, v) {
                            flips += 1;
                            break;
                        }
                    }
                }
                if flips == 0 {
                    break;
                }
            }
            if splits == 0 {
                break;
            }
        }
    }
    // fairing of the new vertices (uniform relaxation, border fixed)
    if fair_iters > 0 && m.pos.len() > n0_verts {
        for _ in 0..fair_iters {
            let mut next: Vec<(u32, DVec3)> = Vec::new();
            for v in n0_verts as u32..m.pos.len() as u32 {
                if m.vdead[v as usize] || m.vt[v as usize].is_empty() {
                    continue;
                }
                let nb = m.neighbors(v);
                let mean = nb.iter().map(|&x| m.pos[x as usize]).sum::<DVec3>() / nb.len() as f64;
                next.push((v, m.pos[v as usize].lerp(mean, 0.5)));
            }
            for (v, q) in next {
                m.pos[v as usize] = q;
            }
        }
    }

    let (npos, ntris, old) = m.compact();
    let after = edge_stats(&ntris);
    // output: welded positions, original point attributes, new points zero-filled
    let mut out = Geometry {
        points: npos,
        prims: ntris.iter().map(|t| Prim::polygon(t.to_vec())).collect(),
        ..Default::default()
    };
    let idx: Vec<usize> = old.iter().map(|&o| if (o as usize) < n0_verts { w.first[o as usize] } else { usize::MAX }).collect();
    let src_idx: Vec<usize> = idx.iter().map(|&i| if i == usize::MAX { 0 } else { i }).collect();
    out.point_attribs = g.point_attribs.gather_all(&src_idx);
    // zero the attributes of new points
    let new_rows: Vec<usize> = idx.iter().enumerate().filter(|(_, &i)| i == usize::MAX).map(|(r, _)| r).collect();
    if !new_rows.is_empty() {
        let names: Vec<String> = out.point_attribs.names().map(String::from).collect();
        for name in names {
            if name == "N" {
                continue;
            }
            if let Some(arr) = out.point_attribs.get_mut(&name) {
                zero_rows(arr, &new_rows);
            }
        }
    }
    out.detail_attribs = g.detail_attribs.clone();
    out.compute_normals();

    let area_added: f64 = (n0_tris..ntris.len().max(n0_tris))
        .filter(|&i| i < ntris.len())
        .map(|i| {
            let t = ntris[i];
            let (a, b, c) = (out.points[t[0] as usize], out.points[t[1] as usize], out.points[t[2] as usize]);
            (b - a).cross(c - a).length() * 0.5
        })
        .sum();
    let mut rep = Report::new("Fill Holes");
    rep.metric("holes_found", Metric::new(loops.len() as f64, "holes"));
    rep.metric("holes_filled", Metric::new(filled as f64, "holes"));
    rep.metric("holes_skipped", Metric::new((skipped_size + failed) as f64, "holes"));
    rep.metric("faces_added", Metric::new(ntris.len() as f64 - n0_tris as f64, "faces"));
    rep.metric("vertices_added", Metric::new((out.points.len() as f64) - w.pos.len() as f64, "points"));
    rep.metric("patch_area", Metric::new(area_added, "mm2"));
    rep.metric("open_edges_in", Metric::new(before.open_edges as f64, "edges"));
    rep.metric("open_edges_out", Metric::new(after.open_edges as f64, "edges").with_limit(0.0, true));
    if skipped_size > 0 {
        rep.note(Severity::Warning, format!("{skipped_size} hole(s) larger than the size limits were left open"));
    }
    if failed > 0 {
        rep.note(Severity::Warning, format!("{failed} hole(s) could not be triangulated without creating duplicate edges"));
    }
    if loops.is_empty() {
        rep.note(Severity::Info, "the mesh has no open borders");
    }
    if after.nonmanifold_edges > 0 {
        rep.note(Severity::Warning, "the result still has non-manifold edges");
    }
    Ok(vec![Data::geometry(out), Data::report(rep)])
}

fn zero_rows(arr: &mut AttribArray, rows: &[usize]) {
    match arr {
        AttribArray::Float(v) => rows.iter().for_each(|&r| v[r] = 0.0),
        AttribArray::Int(v) => rows.iter().for_each(|&r| v[r] = 0),
        AttribArray::Vec3(v) => rows.iter().for_each(|&r| v[r] = DVec3::ZERO),
        AttribArray::Str(v) => rows.iter().for_each(|&r| v[r] = String::new()),
    }
}

// --------------------------------------------------------------- smoothing

/// Laplacian (or Taubin lambda|mu) smoothing of an indexed triangle mesh.
/// Points flagged in `fixed`, and points without neighbours, stay put.
pub fn smooth_welded(pos0: &[DVec3], tris: &[[u32; 3]], fixed: &[bool], iters: usize, taubin: bool, lambda: f64, mu: f64) -> Vec<DVec3> {
    let nw = pos0.len();
    let mut nbrs: Vec<Vec<u32>> = vec![Vec::new(); nw];
    for t in tris {
        for k in 0..3 {
            nbrs[t[k] as usize].push(t[(k + 1) % 3]);
            nbrs[t[k] as usize].push(t[(k + 2) % 3]);
        }
    }
    for l in &mut nbrs {
        l.sort_unstable();
        l.dedup();
    }
    let mut pos = pos0.to_vec();
    let mut next = pos.clone();
    let mut pass = |pos: &mut Vec<DVec3>, step: f64| {
        for i in 0..nw {
            if fixed[i] || nbrs[i].is_empty() {
                next[i] = pos[i];
                continue;
            }
            let mean = nbrs[i].iter().map(|&j| pos[j as usize]).sum::<DVec3>() / nbrs[i].len() as f64;
            next[i] = pos[i] + (mean - pos[i]) * step;
        }
        std::mem::swap(pos, &mut next);
    };
    for _ in 0..iters {
        pass(&mut pos, lambda);
        if taubin {
            pass(&mut pos, mu);
        }
    }
    pos
}

/// Close the boundary loops of `tris` with up to `max_edges` edges using
/// the minimum-weight triangulation. Returns the added triangles and the
/// number of loops filled.
pub fn close_small_holes(pos: &[DVec3], tris: &[[u32; 3]], max_edges: usize) -> (Vec<[u32; 3]>, usize) {
    let loops = boundary_loops(tris);
    if loops.is_empty() {
        return (Vec::new(), 0);
    }
    let mut mesh_edges: HashSet<(u32, u32)> = HashSet::new();
    let mut half: HashMap<(u32, u32), u32> = HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            mesh_edges.insert((a.min(b), a.max(b)));
            half.insert((a, b), ti as u32);
        }
    }
    let normal = |t: u32| {
        let v = tris[t as usize];
        (pos[v[1] as usize] - pos[v[0] as usize]).cross(pos[v[2] as usize] - pos[v[0] as usize]).normalize_or_zero()
    };
    let mut added = Vec::new();
    let mut filled = 0;
    for lp in &loops {
        let n = lp.len();
        if n > max_edges {
            continue;
        }
        let r_ids: Vec<u32> = lp.iter().rev().copied().collect();
        let r_pts: Vec<DVec3> = r_ids.iter().map(|&v| pos[v as usize]).collect();
        let border_n: Vec<DVec3> = (0..n).map(|a| half.get(&(r_ids[(a + 1) % n], r_ids[a])).map_or(DVec3::ZERO, |&t| normal(t))).collect();
        if let Some(ts) = triangulate_hole(&r_pts, &r_ids, &border_n, &mesh_edges) {
            for t in ts {
                added.push([r_ids[t[0]], r_ids[t[1]], r_ids[t[2]]]);
            }
            for k in 0..n {
                mesh_edges.insert((r_ids[k].min(r_ids[(k + 1) % n]), r_ids[k].max(r_ids[(k + 1) % n])));
            }
            filled += 1;
        }
    }
    (added, filled)
}

fn smooth_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.smooth_mesh", "Smooth Mesh", Stage::Clean)
        .input(mesh_in())
        .output(geo_out())
        .output(rep_out())
        .param(ParamSpec::int("iterations", "Iterations", 10, 0, 1000).help("Number of smoothing passes. 0 returns the input unchanged."))
        .param(
            ParamSpec::choice("method", "Method", "taubin", &["laplace", "taubin"])
                .help("laplace: move each point toward the mean of its neighbours (shrinks the shape). taubin: alternate shrink and inflate passes to limit shrinkage."),
        )
        .param(ParamSpec::float("lambda", "Shrink step", 0.5, 0.01, 1.0).help("Fraction of the way each point moves toward its neighbours' mean."))
        .param(ParamSpec::float("mu", "Inflate step", -0.53, -1.0, -0.01).help("Taubin inflate step; its magnitude should be slightly larger than the shrink step."))
        .param(ParamSpec::bool("lock_boundary", "Lock borders", true).help("Keep points on open borders where they are."))
        .param(ParamSpec::bool("lock_hard_edges", "Lock hard edges", false).help("Keep points on sharp edges where they are."))
        .param(ParamSpec::float("crease_angle_deg", "Hard-edge angle", 30.0, 1.0, 179.0).unit("deg").help("Edges whose faces meet at a sharper angle than this count as hard."))
        .param(
            ParamSpec::float("weld_tolerance", "Weld distance", 0.0, 0.0, 10.0)
                .unit("mm")
                .help("Points closer than this move together. 0 treats exact duplicates as one point."),
        )
        .help("Relaxes noise on a mesh by moving points toward the average of their neighbours. Topology and attributes are kept.")
        .emulates("smoothing iterations of a watertight shrink-wrap tool; uniform Laplacian and Taubin lambda|mu smoothing (Taubin 1995)")
        .spec(SPEC_MESH)
        .implemented()
}

fn smooth_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    ensure_metric(ctx, &g)?;
    let iters = pi(p, "iterations", 0, 1000)? as usize;
    let taubin = p.str("method")? == "taubin";
    let lambda = pf(p, "lambda", 0.01, 1.0)?;
    let mu = pf(p, "mu", -1.0, -0.01)?;
    let lock_b = p.bool("lock_boundary")?;
    let lock_h = p.bool("lock_hard_edges")?;
    let ang = pf(p, "crease_angle_deg", 1.0, 179.0)?;
    let tol = pf(p, "weld_tolerance", 0.0, 10.0)?;
    let w = weld_triangles(&g, tol);
    if w.tris.is_empty() {
        return Err(ctx.fail("input has no triangles; connect a mesh"));
    }
    let nw = w.pos.len();
    let (boundary, feature) = edge_flags(&w.pos, &w.tris, ang.to_radians().cos());
    let fixed: Vec<bool> = (0..nw).map(|i| (lock_b && boundary[i]) || (lock_h && feature[i])).collect();
    let closed = edge_stats(&w.tris).open_edges == 0;
    let vol0 = if closed { signed_volume(&w.pos, &w.tris) } else { 0.0 };
    let pos = smooth_welded(&w.pos, &w.tris, &fixed, iters, taubin, lambda, mu);
    let mut out = (*g).clone();
    let mut max_d: f64 = 0.0;
    let mut sum_d = 0.0;
    for (i, q) in out.points.iter_mut().enumerate() {
        let id = w.map[i] as usize;
        max_d = max_d.max((pos[id] - w.pos[id]).length());
        sum_d += (pos[id] - w.pos[id]).length();
        *q = pos[id];
    }
    if out.point_attribs.contains("N") {
        out.compute_normals();
    }
    let mut rep = Report::new("Smooth Mesh");
    rep.metric("iterations", Metric::new(iters as f64, "passes"));
    rep.metric("max_displacement", Metric::new(max_d, "mm"));
    rep.metric("mean_displacement", Metric::new(sum_d / out.points.len().max(1) as f64, "mm"));
    if closed && vol0.abs() > 1e-12 {
        let vol1 = signed_volume(&pos, &w.tris);
        rep.metric("volume_change", Metric::new(100.0 * (vol1 - vol0) / vol0, "%"));
    } else if !closed {
        rep.note(Severity::Info, "mesh is open; volume change not reported");
    }
    Ok(vec![Data::geometry(out), Data::report(rep)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;

    fn grid(n: usize, hole: Option<(usize, usize, usize, usize)>) -> Geometry {
        // n x n quads on z = 0, two triangles each; quads inside `hole` omitted
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
                if let Some((i0, j0, i1, j1)) = hole {
                    if i >= i0 && i < i1 && j >= j0 && j < j1 {
                        continue;
                    }
                }
                faces.push(vec![id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
                faces.push(vec![id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
            }
        }
        Geometry::from_polygons(pts, faces)
    }

    #[test]
    fn loops_of_a_plate_with_a_hole() {
        let g = grid(10, Some((3, 3, 6, 6)));
        let w = weld_triangles(&g, 0.0);
        let loops = boundary_loops(&w.tris);
        assert_eq!(loops.len(), 2); // outer border + hole
        let mut lens: Vec<usize> = loops.iter().map(Vec::len).collect();
        lens.sort();
        assert_eq!(lens, vec![12, 40]);
    }

    #[test]
    fn planar_hole_is_restored_exactly() {
        let g = grid(10, Some((3, 3, 6, 6)));
        assert!((g.surface_area() - 91.0).abs() < 1e-9);
        // outer border (40 edges) is left because max size is small
        let out = run(fill_info, fill_cook, vec![geo(g)], &[("max_hole_edges", i(20)), ("refine", b(false))]).unwrap();
        let o = out_geo(&out[0]);
        assert!((o.surface_area() - 100.0).abs() < 1e-9);
        assert!(o.points.iter().all(|p| p.z.abs() < 1e-12));
        let r = out_report(&out[1]);
        assert_eq!(r.metrics["holes_found"].value, 2.0);
        assert_eq!(r.metrics["holes_filled"].value, 1.0);
        assert_eq!(r.metrics["holes_skipped"].value, 1.0);
        assert_eq!(r.metrics["faces_added"].value, 10.0); // 12-gon -> 10 triangles
        assert!(r.notes.iter().any(|n| n.0 == Severity::Warning));
        assert!(o.validate().is_ok());
        // winding matches the plate: all normals +z
        let nrm = o.prim_normal(o.prims.len() - 1);
        assert!(nrm.z > 0.99);
    }

    #[test]
    fn refinement_adds_vertices_and_keeps_the_plane_flat() {
        let g = grid(14, Some((3, 3, 11, 11)));
        let out = run(fill_info, fill_cook, vec![geo(g)], &[("max_hole_edges", i(40))]).unwrap();
        let o = out_geo(&out[0]);
        let r = out_report(&out[1]);
        assert!(r.metrics["vertices_added"].value > 0.0);
        assert!(o.points.iter().all(|p| p.z.abs() < 1e-9));
        assert!((r.metrics["patch_area"].value - 64.0).abs() < 1e-6);
        let st = edge_stats(&o.triangles().iter().map(|t| t.1).collect::<Vec<_>>());
        assert_eq!(st.nonmanifold_edges, 0);
        assert_eq!(st.inconsistent_edges, 0);
        // only the outer border remains open
        assert_eq!(r.metrics["open_edges_out"].value, 56.0);
    }

    #[test]
    fn sphere_cap_hole_closes_to_a_watertight_solid() {
        let mut g = sphere_geo(50.0, 40, 41);
        // remove the 40 triangles of the north-pole fan
        let keep: Vec<_> = g.prims.iter().enumerate().filter(|(_, p)| !p.vertices.contains(&0)).map(|(i, _)| i).collect();
        let prims: Vec<_> = keep.iter().map(|&i| g.prims[i].clone()).collect();
        g.prims = prims;
        let out = run(fill_info, fill_cook, vec![geo(g)], &[]).unwrap();
        let o = out_geo(&out[0]);
        let tris: Vec<_> = o.triangles().iter().map(|t| t.1).collect();
        let st = edge_stats(&tris);
        assert_eq!((st.open_edges, st.nonmanifold_edges, st.inconsistent_edges), (0, 0, 0));
        let r = out_report(&out[1]);
        assert_eq!(r.metrics["holes_filled"].value, 1.0);
        let vol = 4.0 / 3.0 * std::f64::consts::PI * 50.0f64.powi(3);
        let v = signed_volume(&o.points, &tris);
        assert!((v - vol).abs() / vol < 0.03, "volume {v} vs {vol}");
    }

    #[test]
    fn closed_mesh_has_nothing_to_fill() {
        let out = run(fill_info, fill_cook, vec![geo(sphere_geo(10.0, 12, 13))], &[]).unwrap();
        let r = out_report(&out[1]);
        assert_eq!(r.metrics["holes_found"].value, 0.0);
        assert_eq!(out_geo(&out[0]).triangles().len(), sphere_geo(10.0, 12, 13).triangles().len());
    }

    #[test]
    fn smoothing_flattens_noise_and_locks_borders() {
        let mut g = grid(20, None);
        let mut rng = crate::util::Rng::new(5);
        for p in &mut g.points {
            p.z = (rng.next_f64() - 0.5) * 0.2;
        }
        let interior = |i: usize| { let (x, y) = (i % 21, i / 21); x > 0 && y > 0 && x < 20 && y < 20 };
        let rough0: f64 = g.points.iter().enumerate().filter(|(i, _)| interior(*i)).map(|(_, p)| p.z * p.z).sum::<f64>();
        let before = g.points.clone();
        let out = run(smooth_info, smooth_cook, vec![geo(g)], &[("iterations", i(20)), ("method", s("laplace"))]).unwrap();
        let o = out_geo(&out[0]);
        let rough1: f64 = o.points.iter().enumerate().filter(|(i, _)| interior(*i)).map(|(_, p)| p.z * p.z).sum::<f64>();
        assert!(rough1 < rough0 * 0.2);
        // border points did not move
        for (i, p) in o.points.iter().enumerate() {
            let (x, y) = (i % 21, i / 21);
            if x == 0 || y == 0 || x == 20 || y == 20 {
                assert_eq!(*p, before[i]);
            }
        }
        assert!(out_report(&out[1]).metrics["max_displacement"].value > 0.0);
    }

    #[test]
    fn taubin_shrinks_a_sphere_less_than_laplace() {
        let g = sphere_geo(50.0, 40, 41);
        let lap = run(smooth_info, smooth_cook, vec![geo(g.clone())], &[("iterations", i(30)), ("method", s("laplace"))]).unwrap();
        let tau = run(smooth_info, smooth_cook, vec![geo(g)], &[("iterations", i(30)), ("method", s("taubin"))]).unwrap();
        let (vl, vt) = (out_report(&lap[1]).metrics["volume_change"].value, out_report(&tau[1]).metrics["volume_change"].value);
        assert!(vl < -5.0, "laplace {vl}");
        assert!(vt > vl && vt.abs() < vl.abs() * 0.5, "taubin {vt} laplace {vl}");
        // zero iterations is the identity
        let id = run(smooth_info, smooth_cook, vec![geo(sphere_geo(5.0, 8, 9))], &[("iterations", i(0))]).unwrap();
        assert_eq!(out_report(&id[1]).metrics["max_displacement"].value, 0.0);
    }

    #[test]
    fn hard_edges_can_be_locked() {
        // box edge stays put when locked
        let g = Geometry::make_box(DVec3::splat(10.0));
        let out = run(smooth_info, smooth_cook, vec![geo(g.clone())], &[("lock_hard_edges", b(true))]).unwrap();
        let o = out_geo(&out[0]);
        for (a, b) in o.points.iter().zip(&g.points) {
            assert_eq!(a, b);
        }
    }
}

//! Quad mesh metrics (`surface.quad_mesh_report`) and the quad remesh
//! placeholder (`surface.quad_remesh`).

use std::collections::{BTreeMap, HashMap};

use glam::DVec3;
use vernon_core::{Geometry, Metric, Report, Severity};
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::bvh::TriBvh;
use crate::deviation::{dev_stats, signed_distances};
use crate::subd::Subd;
use crate::topology::{edge_map, orient, tri_adjacency, weld};
use crate::util::{check_finite, max_abs_coord, node_info, placeholder_note, positive, SPEC_MESH};

/// Metrics of a polygon mesh.
#[derive(Debug, Clone, Default)]
pub struct QuadStats {
    pub faces: usize,
    pub quads: usize,
    pub tris: usize,
    pub others: usize,
    pub vertices: usize,
    pub edges: usize,
    pub euler_char: i64,
    pub boundary_edges: usize,
    /// valence -> vertex count (interior vertices only).
    pub valence: BTreeMap<usize, usize>,
    /// Sum over interior vertices of `4 - valence`.
    pub index_sum: i64,
    pub quad_fraction: f64,
    pub mean_edge: f64,
    pub min_edge: f64,
    pub max_edge: f64,
    /// Smallest scaled Jacobian over all quad corners (-1..1).
    pub min_scaled_jacobian: f64,
    /// Share of quads whose worst corner Jacobian exceeds 0.3.
    pub good_quad_pct: f64,
}

/// Scaled Jacobian at the corners of a quad: the sine of the corner angle,
/// signed against the quad normal.
pub fn quad_corner_jacobians(p: &[DVec3; 4]) -> [f64; 4] {
    let mut n = DVec3::ZERO;
    for i in 0..4 {
        let (a, b) = (p[i], p[(i + 1) % 4]);
        n += DVec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    let n = n.normalize_or_zero();
    let mut out = [0.0; 4];
    for i in 0..4 {
        let e1 = (p[(i + 1) % 4] - p[i]).normalize_or_zero();
        let e2 = (p[(i + 3) % 4] - p[i]).normalize_or_zero();
        out[i] = e1.cross(e2).dot(n);
    }
    out
}

pub fn quad_stats(geo: &Geometry, weld_tol: f64) -> QuadStats {
    let w = weld(geo, weld_tol);
    let mut s = QuadStats { faces: w.faces.len(), vertices: w.points.len(), ..Default::default() };
    let em = edge_map(&w.faces);
    s.edges = em.len();
    s.boundary_edges = em.values().filter(|f| f.len() == 1).count();
    s.euler_char = s.vertices as i64 - s.edges as i64 + s.faces as i64;
    let mut incident = vec![0usize; w.points.len()];
    let mut on_boundary = vec![false; w.points.len()];
    for (&(a, b), f) in &em {
        incident[a as usize] += 1;
        incident[b as usize] += 1;
        if f.len() != 2 {
            on_boundary[a as usize] = true;
            on_boundary[b as usize] = true;
        }
    }
    for v in 0..w.points.len() {
        if !on_boundary[v] && incident[v] > 0 {
            *s.valence.entry(incident[v]).or_default() += 1;
            s.index_sum += 4 - incident[v] as i64;
        }
    }
    // sorted lengths: the sum must not depend on hash-map iteration order
    let mut lens: Vec<f64> = em.keys().map(|&(a, b)| (w.points[a as usize] - w.points[b as usize]).length()).collect();
    lens.sort_by(|a, b| a.total_cmp(b));
    s.mean_edge = if lens.is_empty() { 0.0 } else { lens.iter().sum::<f64>() / lens.len() as f64 };
    s.min_edge = lens.first().copied().unwrap_or(0.0);
    s.max_edge = lens.last().copied().unwrap_or(0.0);
    let (mut jmin, mut good) = (1.0f64, 0usize);
    for f in &w.faces {
        match f.len() {
            4 => {
                s.quads += 1;
                let p = [w.points[f[0] as usize], w.points[f[1] as usize], w.points[f[2] as usize], w.points[f[3] as usize]];
                let j = quad_corner_jacobians(&p);
                let worst = j.iter().cloned().fold(1.0, f64::min);
                jmin = jmin.min(worst);
                if worst > 0.3 {
                    good += 1;
                }
            }
            3 => s.tris += 1,
            _ => s.others += 1,
        }
    }
    s.quad_fraction = if s.faces > 0 { s.quads as f64 / s.faces as f64 } else { 0.0 };
    s.min_scaled_jacobian = if s.quads > 0 { jmin } else { 0.0 };
    s.good_quad_pct = if s.quads > 0 { 100.0 * good as f64 / s.quads as f64 } else { 0.0 };
    s
}

/// `surface.quad_mesh_report`
pub struct QuadMeshReportNode;

impl NodeOp for QuadMeshReportNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.quad_mesh_report", "Quad Mesh Report", SPEC_MESH)
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .input(PortSpec::optional("reference", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("tol_mm", "Deviation tolerance", 0.5, 0.001, 50.0).unit("mm").help("Limit on the 95th percentile deviation from the reference mesh."))
            .param(ParamSpec::float("weld_tol_mm", "Weld tolerance", 1e-6, 0.0, 10.0).unit("mm").help("Vertices closer than this are merged before counting."))
            .help("Measures a polygon mesh: face and quad counts, edge lengths, vertex valence histogram, the index sum that equals 8 for a closed all-quad sphere-like mesh, corner quality, and optionally the deviation of the vertices from a reference mesh.")
            .emulates("QuadRemesh preview readout")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let mesh = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &mesh)?;
        let tol = positive(ctx, "tol_mm", params.float("tol_mm")?)?;
        let wt = params.float("weld_tol_mm")?.max(1.2e-7 * max_abs_coord(&mesh.points));
        let s = quad_stats(&mesh, wt);
        if s.faces == 0 {
            return Err(ctx.fail("the mesh has no faces"));
        }
        let mut r = Report::new("Quad mesh report");
        r.metric("faces", Metric::new(s.faces as f64, "count"));
        r.metric("quads", Metric::new(s.quads as f64, "count"));
        r.metric("triangles", Metric::new(s.tris as f64, "count"));
        r.metric("quad_fraction", Metric::new(s.quad_fraction, ""));
        r.metric("vertices", Metric::new(s.vertices as f64, "count"));
        r.metric("euler_char", Metric::new(s.euler_char as f64, ""));
        r.metric("boundary_edges", Metric::new(s.boundary_edges as f64, "count"));
        r.metric("mean_edge_mm", Metric::new(s.mean_edge, "mm"));
        r.metric("edge_ratio", Metric::new(if s.min_edge > 0.0 { s.max_edge / s.min_edge } else { 0.0 }, ""));
        r.metric("index_sum", Metric::new(s.index_sum as f64, ""));
        for v in [3usize, 4, 5] {
            r.metric(format!("valence_{v}"), Metric::new(*s.valence.get(&v).unwrap_or(&0) as f64, "count"));
        }
        r.metric("valence_6_plus", Metric::new(s.valence.range(6..).map(|(_, c)| *c).sum::<usize>() as f64, "count"));
        r.metric("min_scaled_jacobian", Metric::new(s.min_scaled_jacobian, ""));
        r.metric("good_quad_pct", Metric::new(s.good_quad_pct, "%"));
        if let Some(reference) = vernon_graph::node::inputs::opt_geometry(inputs, 1) {
            check_finite(ctx, &reference)?;
            let bvh = TriBvh::from_geometry(&reference, Some(wt));
            if bvh.is_empty() {
                return Err(ctx.fail("the reference mesh has no triangles"));
            }
            let d = signed_distances(&bvh, &mesh.points);
            let st = dev_stats(&d, tol);
            r.metric("dev_p50_mm", Metric::new(st.p50, "mm"));
            r.metric("dev_p95_mm", Metric::new(st.p95, "mm").with_limit(tol, true));
            r.metric("dev_p99_mm", Metric::new(st.p99, "mm"));
            r.metric("dev_max_mm", Metric::new(st.max_abs, "mm"));
        }
        Ok(vec![Data::report(r)])
    }
}

// ---------------------------------------------------------------------------
// remesh placeholder
// ---------------------------------------------------------------------------

/// Merge adjacent triangles into well-shaped quads: triangles with the fewest
/// remaining partners are matched first, taking the best-shaped partner.
fn pair_triangles(points: &[DVec3], tris: &[[u32; 3]], hard_cos: f64) -> (Vec<Vec<u32>>, usize) {
    let adj = tri_adjacency(tris);
    let normal = |t: &[u32; 3]| (points[t[1] as usize] - points[t[0] as usize]).cross(points[t[2] as usize] - points[t[0] as usize]).normalize_or_zero();
    let mut cand: Vec<Vec<(f64, usize, [u32; 4])>> = vec![Vec::new(); tris.len()];
    for (a, nb) in adj.iter().enumerate() {
        for (k, n) in nb.iter().enumerate() {
            let Some(b) = n else { continue };
            let b = *b as usize;
            if b <= a {
                continue;
            }
            let ta = tris[a];
            let (u, v) = (ta[k], ta[(k + 1) % 3]);
            let r = ta[(k + 2) % 3];
            let Some(s) = tris[b].iter().copied().find(|x| *x != u && *x != v) else { continue };
            if normal(&ta).dot(normal(&tris[b])) < hard_cos {
                continue;
            }
            let ring = [u, s, v, r];
            let p = [points[ring[0] as usize], points[ring[1] as usize], points[ring[2] as usize], points[ring[3] as usize]];
            let worst = quad_corner_jacobians(&p).iter().cloned().fold(1.0, f64::min);
            if worst < 0.2 {
                continue;
            }
            cand[a].push((worst, b, ring));
            cand[b].push((worst, a, ring));
        }
    }
    let mut matched = vec![false; tris.len()];
    let free_count = |t: usize, matched: &[bool]| cand[t].iter().filter(|c| !matched[c.1]).count();
    let mut heap: std::collections::BinaryHeap<std::cmp::Reverse<(usize, usize)>> = (0..tris.len()).map(|t| std::cmp::Reverse((cand[t].len(), t))).collect();
    let mut faces = Vec::new();
    while let Some(std::cmp::Reverse((d, t))) = heap.pop() {
        if matched[t] || d != free_count(t, &matched) || d == 0 {
            continue;
        }
        let best = cand[t].iter().filter(|c| !matched[c.1]).max_by(|x, y| x.0.total_cmp(&y.0).then(y.1.cmp(&x.1))).copied();
        let Some((_, o, ring)) = best else { continue };
        matched[t] = true;
        matched[o] = true;
        faces.push(ring.to_vec());
        for src in [t, o] {
            for c in &cand[src] {
                if !matched[c.1] {
                    heap.push(std::cmp::Reverse((free_count(c.1, &matched), c.1)));
                }
            }
        }
    }
    let mut left = 0;
    for (t, &m) in matched.iter().enumerate() {
        if !m {
            faces.push(tris[t].to_vec());
            left += 1;
        }
    }
    (faces, left)
}

/// Vertex at step `s` of `n` along the original edge `a -> b`, shared by every
/// face that uses the edge.
fn edge_vertex(pts: &mut Vec<DVec3>, cache: &mut HashMap<(u32, u32, usize), u32>, a: u32, b: u32, s: usize, n: usize) -> u32 {
    if s == 0 {
        return a;
    }
    if s == n {
        return b;
    }
    let (lo, hi, k) = if a < b { (a, b, s) } else { (b, a, n - s) };
    *cache.entry((lo, hi, k)).or_insert_with(|| {
        let p = pts[lo as usize].lerp(pts[hi as usize], k as f64 / n as f64);
        pts.push(p);
        (pts.len() - 1) as u32
    })
}

/// Fill a quad whose four border vertex rows are known with an `m x m` grid
/// (bilinear interior). Rows: bottom c0 -> c1, right c1 -> c2, top c3 -> c2, left c0 -> c3.
fn grid_quad(pts: &mut Vec<DVec3>, out: &mut Vec<Vec<u32>>, bottom: &[u32], right: &[u32], top: &[u32], left: &[u32], m: usize) {
    let mut g = vec![vec![0u32; m + 1]; m + 1];
    for i in 0..=m {
        g[i][0] = bottom[i];
        g[i][m] = top[i];
        g[0][i] = left[i];
        g[m][i] = right[i];
    }
    let (p00, p10, p01, p11) = (pts[g[0][0] as usize], pts[g[m][0] as usize], pts[g[0][m] as usize], pts[g[m][m] as usize]);
    for i in 1..m {
        for j in 1..m {
            let (u, v) = (i as f64 / m as f64, j as f64 / m as f64);
            pts.push(p00 * ((1.0 - u) * (1.0 - v)) + p10 * (u * (1.0 - v)) + p01 * ((1.0 - u) * v) + p11 * (u * v));
            g[i][j] = (pts.len() - 1) as u32;
        }
    }
    for i in 0..m {
        for j in 0..m {
            out.push(vec![g[i][j], g[i + 1][j], g[i + 1][j + 1], g[i][j + 1]]);
        }
    }
}

/// Split every quad into `n x n` and every triangle into three `(n/2)^2`
/// grids, sharing the vertices on common edges. `n` must be even when
/// triangles exist.
fn split_faces(points: &[DVec3], faces: &[Vec<u32>], n: usize) -> (Vec<DVec3>, Vec<Vec<u32>>) {
    let mut pts: Vec<DVec3> = points.to_vec();
    let mut edges: HashMap<(u32, u32, usize), u32> = HashMap::new();
    let mut spokes: HashMap<(u32, u32, usize), u32> = HashMap::new();
    let mut out = Vec::new();
    for f in faces {
        match f.len() {
            4 => {
                let side = |pts: &mut Vec<DVec3>, edges: &mut HashMap<(u32, u32, usize), u32>, a: u32, b: u32| (0..=n).map(|s| edge_vertex(pts, edges, a, b, s, n)).collect::<Vec<u32>>();
                let bottom = side(&mut pts, &mut edges, f[0], f[1]);
                let right = side(&mut pts, &mut edges, f[1], f[2]);
                let top = side(&mut pts, &mut edges, f[3], f[2]);
                let left = side(&mut pts, &mut edges, f[0], f[3]);
                grid_quad(&mut pts, &mut out, &bottom, &right, &top, &left, n);
            }
            3 => {
                let m = n / 2;
                pts.push((points[f[0] as usize] + points[f[1] as usize] + points[f[2] as usize]) / 3.0);
                let centre = (pts.len() - 1) as u32;
                for k in 0..3 {
                    let (a, b, c) = (f[k], f[(k + 1) % 3], f[(k + 2) % 3]);
                    let mid_ab = edge_vertex(&mut pts, &mut edges, a, b, m, n);
                    let mid_ca = edge_vertex(&mut pts, &mut edges, c, a, m, n);
                    let bottom: Vec<u32> = (0..=m).map(|s| edge_vertex(&mut pts, &mut edges, a, b, s, n)).collect();
                    let left: Vec<u32> = (0..=m).map(|s| edge_vertex(&mut pts, &mut edges, a, c, s, n)).collect();
                    let mut spoke = |pts: &mut Vec<DVec3>, from: u32| -> Vec<u32> {
                        (0..=m)
                            .map(|s| {
                                if s == 0 {
                                    return from;
                                }
                                if s == m {
                                    return centre;
                                }
                                *spokes.entry((from, centre, s)).or_insert_with(|| {
                                    let p = pts[from as usize].lerp(pts[centre as usize], s as f64 / m as f64);
                                    pts.push(p);
                                    (pts.len() - 1) as u32
                                })
                            })
                            .collect()
                    };
                    let right = spoke(&mut pts, mid_ab);
                    let top = spoke(&mut pts, mid_ca);
                    grid_quad(&mut pts, &mut out, &bottom, &right, &top, &left, m);
                }
            }
            _ => out.push(f.clone()),
        }
    }
    (pts, out)
}

/// Most quads `surface.quad_remesh` makes; beyond it a cook would run for
/// minutes on the cook thread.
pub const MAX_QUADS: f64 = 2_000_000.0;

/// `surface.quad_remesh`
pub struct QuadRemeshNode;

impl NodeOp for QuadRemeshNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.quad_remesh", "Quad Remesh", SPEC_MESH)
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .input(PortSpec::optional("guides", DataKind::Geometry))
            .output(PortSpec::new("quads", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("size_mode", "Size mode", "count", &["count", "edge"]).help("Aim for a quad count or for an edge length."))
            .param(ParamSpec::int("target_quads", "Target quads", 1000, 4, 10_000_000).help("Approximate number of quads."))
            .param(ParamSpec::float("target_edge", "Target edge length", 5.0, 0.01, 10_000.0).unit("mm").help("Approximate quad edge length."))
            .param(ParamSpec::int("adaptive", "Adaptive size", 0, 0, 100).help("More detail where the surface curves; 0 is uniform."))
            .param(ParamSpec::bool("detect_hard_edges", "Detect hard edges", true).help("Keep edges sharper than the hard edge angle as mesh edges."))
            .param(ParamSpec::float("hard_edge_angle_deg", "Hard edge angle", 30.0, 0.0, 180.0).unit("deg").help("Faces turning by more than this meet at a hard edge."))
            .param(ParamSpec::choice("symmetry", "Symmetry", "none", &["none", "x", "y", "z", "xy", "xz", "yz", "xyz"]).help("Mirror planes through the bounding box centre."))
            .param(ParamSpec::choice("guide_influence", "Guide influence", "none", &["none", "approximate", "edge_ring", "edge_loop"]).help("How strongly guide curves steer the edge flow."))
            .param(ParamSpec::choice("use_surface_edges", "Use surface edges", "smart", &["off", "smart", "strict"]).help("Treat borders between source faces as edges."))
            .param(ParamSpec::bool("convert_to_subd", "Convert to subdivision cage", false).help("Output the quads as a subdivision cage with hard edges as creases."))
            .help("Rebuilds a triangle mesh as a quad mesh near a target size. The current version pairs input triangles into quads and splits them evenly; it does not yet follow surface curvature directions.")
            .emulates("Rhino QuadRemesh")
            .emulates("Instant Meshes")
            .emulates("QuadriFlow")
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let mesh = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &mesh)?;
        let mut w = weld(&mesh, 1e-6f64.max(1.2e-7 * max_abs_coord(&mesh.points)));
        if w.faces.is_empty() {
            return Err(ctx.fail("the mesh has no usable faces"));
        }
        orient(&mut w);
        let area: f64 = Geometry::from_polygons(w.points.clone(), w.faces.clone()).surface_area();
        let target = if params.str("size_mode")? == "edge" {
            let l = positive(ctx, "target_edge", params.float("target_edge")?)?;
            (area / (l * l)).max(1.0)
        } else {
            params.int("target_quads")?.max(1) as f64
        };
        if target > MAX_QUADS {
            return Err(ctx.fail(format!("about {target:.0} quads asked for (limit {MAX_QUADS:.0}); lower the count or raise the edge length")));
        }
        let hard_cos = if params.bool("detect_hard_edges")? { params.float("hard_edge_angle_deg")?.to_radians().cos() } else { -1.0 };
        let tris: Vec<[u32; 3]> = w.faces.iter().flat_map(|f| crate::util::triangulate_ring(&w.points, f)).collect();
        let (paired, left) = pair_triangles(&w.points, &tris, hard_cos);
        // split count: quads give n^2 faces, triangles 3 (n/2)^2
        let q0 = (paired.len() - left) as f64;
        let t0 = left as f64;
        let faces_at = |n: usize| q0 * (n * n) as f64 + t0 * 3.0 * ((n / 2) * (n / 2)) as f64;
        let mut best_n = if left > 0 { 2 } else { 1 };
        let mut best_err = f64::INFINITY;
        let step = if left > 0 { 2 } else { 1 };
        let mut n = best_n;
        while n <= 256 {
            let err = (faces_at(n) - target).abs() / target;
            if err < best_err {
                best_err = err;
                best_n = n;
            }
            if faces_at(n) > 2.0 * target {
                break;
            }
            n += step;
        }
        let (pts, faces) = split_faces(&w.points, &paired, best_n);
        let mut geo = Geometry::from_polygons(pts, faces);
        geo.compute_normals();
        let mut report = Report::new("Quad remesh");
        let st = quad_stats(&geo, 1e-9);
        report.metric("quad_count", Metric::new(st.quads as f64, "count"));
        report.metric("face_count", Metric::new(st.faces as f64, "count"));
        report.metric("target_faces", Metric::new(target, "count"));
        report.metric("quad_fraction", Metric::new(st.quad_fraction, ""));
        report.metric("mean_edge_mm", Metric::new(st.mean_edge, "mm"));
        report.metric("index_sum", Metric::new(st.index_sum as f64, ""));
        report.metric("min_scaled_jacobian", Metric::new(st.min_scaled_jacobian, ""));
        report.metric("input_triangles_left", Metric::new(left as f64, "count"));
        let off_target = (st.faces as f64 - target).abs() / target;
        if off_target > 0.15 {
            report.note(Severity::Warning, format!("the face count {} is {:.0}% away from the target {}: this version can only refine, not coarsen, the input", st.faces, 100.0 * off_target, target as usize));
        }
        if params.str("symmetry")? != "none" {
            report.note(Severity::Warning, "symmetry is not applied by this version");
        }
        if vernon_graph::node::inputs::opt_geometry(inputs, 1).is_some() || params.str("guide_influence")? != "none" {
            report.note(Severity::Warning, "guide curves are not used by this version");
        }
        placeholder_note(&mut report, "field-aligned quad extraction (a smoothed 4-direction orientation field and a position field, then quad extraction and singularity cleanup); this version pairs input triangles into quads and splits them uniformly");
        let out = if params.bool("convert_to_subd")? {
            let mut sub = Subd::from_geometry(&geo, 1e-9);
            let adj = sub.adjacency();
            let cos_limit = params.float("hard_edge_angle_deg")?.to_radians().cos();
            for (e, fs) in adj.edge_faces.iter().enumerate() {
                if fs.len() == 2 && params.bool("detect_hard_edges")? {
                    let nrm = |f: usize| {
                        let r = &sub.faces[f];
                        (sub.points[r[1] as usize] - sub.points[r[0] as usize]).cross(sub.points[r[2] as usize] - sub.points[r[0] as usize]).normalize_or_zero()
                    };
                    if nrm(fs[0] as usize).dot(nrm(fs[1] as usize)) < cos_limit {
                        sub.creases.insert(adj.edges[e]);
                    }
                }
            }
            sub.to_geometry()
        } else {
            geo
        };
        Ok(vec![Data::geometry(out), Data::report(report)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{b, cook, f, geo_of, i, report_of, s, some, sphere, triangulated};

    #[test]
    fn closed_all_quad_meshes_have_index_sum_eight() {
        let bx = Geometry::make_box(DVec3::splat(10.0));
        let st = quad_stats(&bx, 1e-9);
        assert_eq!((st.faces, st.quads, st.vertices, st.edges, st.euler_char), (6, 6, 8, 12, 2));
        assert_eq!(st.index_sum, 8);
        assert_eq!(st.valence.get(&3), Some(&8));
        assert!((st.min_scaled_jacobian - 1.0).abs() < 1e-12);
        // 4V - 2E = 8 - 8g
        assert_eq!(4 * st.vertices as i64 - 2 * st.edges as i64, 8);
        let t = quad_stats(&triangulated(&bx), 1e-9);
        assert_eq!(t.quads, 0);
        assert!(t.quad_fraction < 1e-12);
    }

    #[test]
    fn dart_quads_have_negative_jacobian() {
        let p = [DVec3::new(0.0, 0.0, 0.0), DVec3::new(4.0, 0.0, 0.0), DVec3::new(1.0, 1.0, 0.0), DVec3::new(0.0, 4.0, 0.0)];
        let j = quad_corner_jacobians(&p);
        assert!(j.iter().cloned().fold(1.0, f64::min) < 0.0);
    }

    #[test]
    fn report_node_compares_with_a_reference() {
        let bx = Geometry::make_box(DVec3::splat(10.0));
        let mut off = bx.clone();
        for p in &mut off.points {
            *p *= 1.0 + 0.2 / 5.0 / 3f64.sqrt();
        }
        let out = cook("surface.quad_mesh_report", &[some(&off), some(&bx)], &[("tol_mm", f(0.5))]).unwrap();
        let r = report_of(&out[0]);
        assert_eq!(r.metrics["index_sum"].value, 8.0);
        assert!(r.metrics["dev_p95_mm"].value > 0.0 && r.metrics["dev_p95_mm"].value < 0.5, "{}", r.metrics["dev_p95_mm"].value);
        assert!(r.passes());
    }

    #[test]
    fn cube_remesh_hits_the_regular_grid_golden() {
        // spec Q2: cube, N = 600 -> 6 x 10 x 10 faces, V = 602, 8 valence-3 vertices
        let cube = triangulated(&Geometry::make_box(DVec3::splat(100.0)));
        let out = cook("surface.quad_remesh", &[some(&cube), None], &[("target_quads", i(600))]).unwrap();
        let g = geo_of(&out[0]);
        let st = quad_stats(&g, 1e-9);
        assert_eq!(st.faces, 600, "{st:?}");
        assert_eq!(st.vertices, 602);
        assert_eq!(st.edges, 1200);
        assert_eq!(st.valence.get(&3), Some(&8));
        assert!((st.quad_fraction - 1.0).abs() < 1e-12);
        assert_eq!(st.index_sum, 8);
        let r = report_of(&out[1]);
        assert!(r.notes.iter().any(|(_, t)| t.starts_with("white-box placeholder")));
        // every corner and edge of the cube survives as mesh geometry
        assert!(g.points.iter().any(|p| (p.abs() - DVec3::splat(50.0)).abs().max_element() < 1e-9));
    }

    #[test]
    fn sphere_remesh_is_all_quads_with_index_sum_eight() {
        let sph = sphere(50.0, 16, 8);
        let out = cook("surface.quad_remesh", &[some(&sph), None], &[("target_quads", i(1000))]).unwrap();
        let r = report_of(&out[1]);
        let faces = r.metrics["face_count"].value;
        assert!((faces - 1000.0).abs() / 1000.0 < 0.15, "faces {faces}");
        assert!((r.metrics["quad_fraction"].value - 1.0).abs() < 1e-12);
        assert_eq!(r.metrics["index_sum"].value, 8.0);
        // edge-length mode
        let by_edge = cook("surface.quad_remesh", &[some(&sph), None], &[("size_mode", s("edge")), ("target_edge", f(8.0))]).unwrap();
        assert!(report_of(&by_edge[1]).metrics["face_count"].value > 100.0);
    }

    #[test]
    fn hard_edges_become_creases_in_the_cage_output() {
        let cube = triangulated(&Geometry::make_box(DVec3::splat(100.0)));
        let out = cook("surface.quad_remesh", &[some(&cube), None], &[("target_quads", i(96)), ("convert_to_subd", b(true))]).unwrap();
        let g = geo_of(&out[0]);
        let creases = g.prims.iter().filter(|p| !p.closed).count();
        assert_eq!(creases, 12 * 4, "12 cube edges, 4 segments each");
        assert!(cook("surface.quad_remesh", &[some(&Geometry::new()), None], &[]).is_err());
    }

    #[test]
    fn runaway_quad_counts_are_refused() {
        let cube = triangulated(&Geometry::make_box(DVec3::splat(100.0)));
        let t0 = std::time::Instant::now();
        let err = cook("surface.quad_remesh", &[some(&cube), None], &[("target_quads", i(5_000_000))]).unwrap_err().to_string();
        assert!(err.contains("limit"), "{err}");
        let err = cook("surface.quad_remesh", &[some(&cube), None], &[("size_mode", s("edge")), ("target_edge", f(0.01))]).unwrap_err().to_string();
        assert!(err.contains("limit"), "{err}");
        assert!(t0.elapsed().as_secs() < 5);
    }
}

//! Freeform patch nodes: `surface.fit_patch`, `surface.auto_surface`,
//! `surface.curve_network_patch` and `surface.loft_sections`.

use glam::DVec3;
use vernon_core::{Metric, Report, Severity};
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::curvefit::fit_with_knots;
use crate::fit::{fit_adaptive_surface, is_height_field, param_boundary, param_conformal, param_projection, sample_mesh, FitSample, SurfaceFit, TriMesh};
use crate::linalg::solve_dense;
use crate::nurbs::{basis_funs, clamped_uniform_knots, find_span, CurveSet, NurbsCurve, NurbsSurface, PatchSet};
use crate::segment::{labels_from_geo, limit_regions, segment, seg_mesh, Primitive, SegParams};
use crate::topology::boundary_loops;
use crate::util::{check_finite, json_err, json_input, node_info, percentile, placeholder_note, positive, SPEC_RE};

/// Parameterise a mesh patch for fitting. Returns (uv, aspect, mode used).
pub fn parameterise(m: &TriMesh, mode: &str) -> Result<(Vec<[f64; 2]>, f64, &'static str), String> {
    let faces: Vec<Vec<u32>> = m.tris.iter().map(|t| t.to_vec()).collect();
    let open = !boundary_loops(&faces).is_empty();
    let chosen = match mode {
        "projection" => "projection",
        "boundary" => "boundary",
        "conformal" => "conformal",
        _ => {
            if is_height_field(m) {
                "projection"
            } else if open {
                "boundary"
            } else {
                "conformal"
            }
        }
    };
    match chosen {
        "projection" => {
            let (uv, a) = param_projection(&m.points);
            Ok((uv, a, "projection"))
        }
        "boundary" => param_boundary(m).map(|(uv, a)| (uv, a, "boundary")).ok_or_else(|| "the region has no boundary loop to map onto a square".to_string()),
        _ => {
            if !open {
                return Err("the region is closed (it has no boundary); split it into regions first".to_string());
            }
            param_conformal(m).map(|(uv, a)| (uv, a, "conformal")).ok_or_else(|| "the conformal parameterisation failed".to_string())
        }
    }
}

/// Cut a closed region into two parts by a plane through its centre,
/// perpendicular to its longest extent.
pub fn split_closed(m: &TriMesh) -> Vec<TriMesh> {
    let (_, ax) = crate::fit::plane_frame(&m.points);
    let c = m.points.iter().fold(DVec3::ZERO, |s, p| s + *p) / m.points.len().max(1) as f64;
    let side = |t: &[u32; 3]| {
        let ct = (m.points[t[0] as usize] + m.points[t[1] as usize] + m.points[t[2] as usize]) / 3.0;
        (ct - c).dot(ax[0]) >= 0.0
    };
    let (a, b): (Vec<usize>, Vec<usize>) = (0..m.tris.len()).partition(|&i| side(&m.tris[i]));
    [a, b].iter().filter(|v| !v.is_empty()).map(|v| m.sub(v)).collect()
}

/// Fit one patch to a mesh region.
#[allow(clippy::too_many_arguments)]
pub fn fit_region_patch(m: &TriMesh, mode: &str, degree: usize, init_cp: usize, max_cp: usize, smooth: f64, tol: f64, extra_per_tri: usize) -> Result<(SurfaceFit, &'static str), String> {
    if m.tris.is_empty() {
        return Err("the region has no triangles".into());
    }
    let (uv, aspect, used) = parameterise(m, mode)?;
    // coarse regions have fewer vertices than unknowns: add points inside the triangles
    let extra = if m.points.len() < 3 * init_cp * init_cp { extra_per_tri.max(8) } else { extra_per_tri };
    let mut samples = sample_mesh(m, &uv, extra, 60_000);
    let fit = fit_adaptive_surface(&mut samples, degree, init_cp, max_cp, aspect, tol, smooth, 3).ok_or_else(|| "too little data to fit a surface".to_string())?;
    Ok((fit, used))
}

fn fit_params(info: NodeTypeInfo) -> NodeTypeInfo {
    info.param(ParamSpec::int("degree", "Degree", 3, 2, 5).help("Polynomial degree in both directions."))
        .param(ParamSpec::int("init_cp", "Initial control points", 6, 4, 32).help("Control points per direction to start with."))
        .param(ParamSpec::int("max_cp", "Max control points", 48, 8, 128).help("Upper bound of control points per direction."))
        .param(ParamSpec::float("smoothness", "Smoothness", 1e-3, 0.0, 1.0).help("Weight of the thin-plate fairness term."))
        .param(ParamSpec::float("tol_mm", "Tolerance", 0.1, 0.001, 5.0).unit("mm").help("Largest accepted distance between the surface and the data."))
        .param(ParamSpec::int("extra_samples", "Extra samples per face", 0, 0, 16).help("Additional sample points inside every triangle."))
}

struct FitSettings {
    degree: usize,
    init_cp: usize,
    max_cp: usize,
    smooth: f64,
    tol: f64,
    extra: usize,
}

fn fit_settings(ctx: &CookContext, params: &Params) -> CookResult<FitSettings> {
    Ok(FitSettings {
        degree: params.int("degree")?.clamp(2, 5) as usize,
        init_cp: params.int("init_cp")?.clamp(4, 32) as usize,
        max_cp: params.int("max_cp")?.clamp(8, 128) as usize,
        smooth: params.float("smoothness")?.clamp(0.0, 1.0),
        tol: positive(ctx, "tol_mm", params.float("tol_mm")?)?,
        extra: params.int("extra_samples")?.clamp(0, 16) as usize,
    })
}

fn fit_metrics(report: &mut Report, f: &SurfaceFit, tol: f64) {
    report.metric("fit_max_mm", Metric::new(f.max_dev, "mm").with_limit(tol, true));
    report.metric("fit_rms_mm", Metric::new(f.rms_dev, "mm"));
    report.metric("fit_p95_mm", Metric::new(f.p95_dev, "mm").with_limit(tol, true));
    report.metric("control_points_u", Metric::new(f.surface.nu as f64, "count"));
    report.metric("control_points_v", Metric::new(f.surface.nv as f64, "count"));
    report.metric("refinement_rounds", Metric::new(f.iterations as f64, "count"));
}

/// `surface.fit_patch`
pub struct FitPatchNode;

impl NodeOp for FitPatchNode {
    fn info(&self) -> NodeTypeInfo {
        fit_params(
            node_info("surface.fit_patch", "Fit Freeform Patch", SPEC_RE)
                .input(PortSpec::new("mesh", DataKind::Geometry))
                .output(PortSpec::new("patches", DataKind::Json))
                .output(PortSpec::new("preview", DataKind::Geometry))
                .output(PortSpec::new("report", DataKind::Report)),
        )
        .param(ParamSpec::int("region", "Region", -1, -1, 100_000).help("Region id to fit; -1 fits the whole mesh."))
        .param(ParamSpec::choice("param", "Parameterisation", "auto", &["auto", "projection", "boundary", "conformal"]).help("How mesh points get surface coordinates: projection onto the best-fit plane, a harmonic map of the border onto a square, or a conformal map."))
        .help("Fits one bicubic-style tensor B-spline patch to a mesh or to one region of a segmented mesh by least squares with a fairness term. Parameters are corrected by foot-point projection and the net is refined until the tolerance is met or the control point limit is reached. The report carries the measured distance of the data to the surface.")
        .emulates("QUICKSURFACE freeform patch fit")
        .emulates("Design X Auto Surface patch fit")
        .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let geo = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &geo)?;
        let fs = fit_settings(ctx, params)?;
        let sm = seg_mesh(&geo);
        let region = params.int("region")?;
        let mesh = if region >= 0 {
            let labels = labels_from_geo(&geo, &sm).ok_or_else(|| ctx.fail("a region id was given but the mesh has no 'region_id' primitive attribute"))?;
            let ids: Vec<usize> = (0..labels.len()).filter(|&t| labels[t] == region).collect();
            if ids.is_empty() {
                return Err(ctx.fail(format!("region {region} does not exist")));
            }
            sm.mesh.sub(&ids)
        } else {
            sm.mesh.clone()
        };
        let (fit, used) = fit_region_patch(&mesh, params.str("param")?, fs.degree, fs.init_cp, fs.max_cp, fs.smooth, fs.tol, fs.extra).map_err(|e| ctx.fail(e))?;
        let mut ps = PatchSet::new(vec![fit.surface.clone()]);
        if region >= 0 {
            ps.patches[0].region_id = Some(region);
        }
        let mut report = Report::new("Freeform patch fit");
        fit_metrics(&mut report, &fit, fs.tol);
        report.note(Severity::Info, format!("parameterisation: {used}"));
        if !fit.reached_tol {
            report.note(Severity::Warning, format!("the tolerance of {} mm was not reached with {} x {} control points", fs.tol, fit.surface.nu, fit.surface.nv));
        }
        let preview = ps.tessellate((fs.tol / 2.0).max(0.01));
        Ok(vec![Data::json(ps.to_json()), Data::geometry(preview), Data::report(report)])
    }
}

/// `surface.auto_surface`
pub struct AutoSurfaceNode;

impl NodeOp for AutoSurfaceNode {
    fn info(&self) -> NodeTypeInfo {
        fit_params(
            node_info("surface.auto_surface", "Auto Surface", SPEC_RE)
                .input(PortSpec::new("mesh", DataKind::Geometry))
                .output(PortSpec::new("patches", DataKind::Json))
                .output(PortSpec::new("preview", DataKind::Geometry))
                .output(PortSpec::new("report", DataKind::Report)),
        )
        .param(ParamSpec::choice("detail", "Detail", "medium", &["coarse", "medium", "fine"]).help("How small regions may get: coarse keeps only large regions."))
        .param(ParamSpec::int("target_patches", "Target patches", 0, 0, 5000).help("Merge smallest regions until at most this many remain; 0 lets the detail level decide."))
        .param(ParamSpec::choice("continuity", "Continuity", "g0", &["g0", "g1"]).help("Continuity wanted between neighbouring patches."))
        .param(ParamSpec::float("sensitivity", "Segmentation sensitivity", 55.0, 0.0, 100.0).help("Sensitivity of the region segmentation."))
        .param(ParamSpec::float("noise_sigma_mm", "Noise level", 0.05, 0.0005, 5.0).unit("mm").help("Scan noise used to scale the region fit tolerance."))
        .help("One-click surfacing: segments the mesh into regions, lays one patch over every region, fits each with a least-squares tensor B-spline and reports the measured deviation of the mesh from the patches. The patch layout is region based (see the report note).")
        .emulates("QUICKSURFACE auto surfacing")
        .emulates("Design X Auto Surface")
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let geo = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &geo)?;
        let mut fs = fit_settings(ctx, params)?;
        fs.tol = fs.tol.max(0.001);
        let sm = seg_mesh(&geo);
        if sm.mesh.tris.is_empty() {
            return Err(ctx.fail("the input has no faces"));
        }
        let min_area = match params.str("detail")? {
            "coarse" => 0.05,
            "fine" => 0.002,
            _ => 0.01,
        };
        let sp = SegParams { sensitivity: params.float("sensitivity")?.clamp(0.0, 100.0), noise_sigma: positive(ctx, "noise_sigma_mm", params.float("noise_sigma_mm")?)?, min_area_frac: min_area, cad_tessellation: false };
        let mut seg = segment(&sm.mesh, &sp);
        let target = params.int("target_patches")?.max(0) as usize;
        if target > 0 {
            seg = limit_regions(&sm.mesh, &seg, target, &sp);
        }
        let mut surfaces = Vec::new();
        let mut region_ids = Vec::new();
        let mut report = Report::new("Auto surface");
        let (mut worst, mut sum2, mut n_s, mut p95s) = (0.0f64, 0.0, 0usize, Vec::<f64>::new());
        let mut skipped = 0usize;
        for (ri, reg) in seg.regions.iter().enumerate() {
            let ids: Vec<usize> = reg.tris.iter().map(|&t| t as usize).collect();
            let whole = sm.mesh.sub(&ids);
            // planes need only the minimum net
            let init = if matches!(reg.prim, Primitive::Plane { .. }) { fs.degree + 1 } else { fs.init_cp };
            let mut pieces = vec![whole.clone()];
            let faces: Vec<Vec<u32>> = whole.tris.iter().map(|t| t.to_vec()).collect();
            if boundary_loops(&faces).is_empty() {
                // a closed region cannot be mapped to a square: cut it in two along its longest axis
                pieces = split_closed(&whole);
                report.note(Severity::Info, format!("region {ri} is closed and was cut into {} parts", pieces.len()));
            }
            for (pi, sub) in pieces.iter().enumerate() {
                match fit_region_patch(sub, "auto", fs.degree, init, fs.max_cp, fs.smooth, fs.tol, fs.extra) {
                    Ok((fit, _)) => {
                        worst = worst.max(fit.max_dev);
                        sum2 += fit.rms_dev * fit.rms_dev * sub.points.len() as f64;
                        n_s += sub.points.len();
                        p95s.push(fit.p95_dev);
                        surfaces.push(fit.surface);
                        region_ids.push(ri as i64);
                    }
                    Err(e) => {
                        skipped += 1;
                        report.note(Severity::Warning, format!("region {ri} part {pi} skipped: {e}"));
                    }
                }
            }
        }
        if surfaces.is_empty() {
            return Err(ctx.fail("no region could be fitted; the mesh may be closed with a single region"));
        }
        let mut ps = PatchSet::new(surfaces);
        for (p, id) in ps.patches.iter_mut().zip(&region_ids) {
            p.region_id = Some(*id);
        }
        p95s.sort_by(|a, b| a.total_cmp(b));
        report.metric("region_count", Metric::new(seg.regions.len() as f64, "count"));
        report.metric("patch_count", Metric::new(ps.patches.len() as f64, "count"));
        report.metric("skipped_regions", Metric::new(skipped as f64, "count"));
        report.metric("control_points", Metric::new(ps.control_point_count() as f64, "count"));
        report.metric("dev_max_mm", Metric::new(worst, "mm").with_limit(fs.tol, true));
        report.metric("dev_rms_mm", Metric::new((sum2 / n_s.max(1) as f64).sqrt(), "mm"));
        report.metric("dev_p95_mm", Metric::new(percentile(&p95s, 100.0), "mm").with_limit(fs.tol, true));
        placeholder_note(&mut report, "patch layout is one untrimmed tensor patch per region; shared boundary curves, trimming and tangent continuity between patches are not built yet");
        if params.str("continuity")? == "g1" {
            report.note(Severity::Warning, "g1 continuity between patches is not enforced");
        }
        let preview = ps.tessellate((fs.tol / 2.0).max(0.02));
        Ok(vec![Data::json(ps.to_json()), Data::geometry(preview), Data::report(report)])
    }
}

// ---------------------------------------------------------------------------
// curve network (bilinearly blended patch)
// ---------------------------------------------------------------------------

/// A curve reparameterised by normalised arc length.
struct ArcCurve<'a> {
    curve: &'a NurbsCurve,
    reversed: bool,
    table: Vec<(f64, f64)>, // (arc length fraction, curve parameter)
}

impl<'a> ArcCurve<'a> {
    fn new(curve: &'a NurbsCurve, reversed: bool) -> Self {
        let (a, b) = curve.domain();
        let n = 512;
        let mut table = Vec::with_capacity(n + 1);
        let mut s = 0.0;
        let mut prev = curve.point(a);
        table.push((0.0, a));
        for i in 1..=n {
            let u = a + (b - a) * i as f64 / n as f64;
            let p = curve.point(u);
            s += (p - prev).length();
            prev = p;
            table.push((s, u));
        }
        let total = s.max(1e-300);
        for e in &mut table {
            e.0 /= total;
        }
        ArcCurve { curve, reversed, table }
    }

    fn at(&self, t: f64) -> DVec3 {
        let t = if self.reversed { 1.0 - t } else { t }.clamp(0.0, 1.0);
        let k = self.table.partition_point(|e| e.0 < t).clamp(1, self.table.len() - 1);
        let (s0, u0) = self.table[k - 1];
        let (s1, u1) = self.table[k];
        let f = if s1 > s0 { (t - s0) / (s1 - s0) } else { 0.0 };
        self.curve.point(u0 + (u1 - u0) * f)
    }
}

fn chain_curves(curves: &[&NurbsCurve], tol: f64) -> Option<Vec<(usize, bool)>> {
    let ends: Vec<(DVec3, DVec3)> = curves
        .iter()
        .map(|c| {
            let (a, b) = c.domain();
            (c.point(a), c.point(b))
        })
        .collect();
    let mut used = vec![false; curves.len()];
    let mut order = vec![(0usize, false)];
    used[0] = true;
    let mut tail = ends[0].1;
    for _ in 1..curves.len() {
        let mut found = None;
        for (i, e) in ends.iter().enumerate() {
            if used[i] {
                continue;
            }
            if (e.0 - tail).length() <= tol {
                found = Some((i, false, e.1));
                break;
            }
            if (e.1 - tail).length() <= tol {
                found = Some((i, true, e.0));
                break;
            }
        }
        let (i, rev, next) = found?;
        used[i] = true;
        order.push((i, rev));
        tail = next;
    }
    if (tail - ends[0].0).length() <= tol {
        Some(order)
    } else {
        None
    }
}

/// `surface.curve_network_patch`
pub struct CurveNetworkPatchNode;

impl NodeOp for CurveNetworkPatchNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.curve_network_patch", "Curve Network Patch", SPEC_RE)
            .input(PortSpec::new("curves", DataKind::Json))
            .input(PortSpec::optional("mesh", DataKind::Geometry))
            .output(PortSpec::new("patches", DataKind::Json))
            .output(PortSpec::new("preview", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("tangency", "Tangency", "g0", &["g0", "g1"]).help("Continuity wanted along the border curves."))
            .param(ParamSpec::int("grid_density", "Grid density", 8, 4, 32).help("Control points per direction of the resulting patch."))
            .param(ParamSpec::float("tol_mm", "Tolerance", 0.1, 0.001, 10.0).unit("mm").help("Largest accepted distance between the patch and its border curves."))
            .param(ParamSpec::string("curve_ids", "Curve ids", "").help("Comma separated ids of the four border curves; empty uses all curves."))
            .help("Builds one patch from four border curves that form a closed loop: a bilinearly blended surface is sampled and fitted by least squares with extra weight on the borders. Reports how far the patch border lies from the input curves. Tangency to neighbouring surfaces is not enforced yet.")
            .emulates("Design X Exact Surfacing")
            .emulates("curve network surface")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let cj = json_input(inputs, 0).ok_or_else(|| ctx.fail("connect a curve set"))?;
        let set = CurveSet::from_json(&cj).map_err(|e| json_err(ctx, "curve set input", e))?;
        let want: Vec<usize> = params.str("curve_ids")?.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        let curves: Vec<&NurbsCurve> = set.curves.iter().filter(|c| want.is_empty() || want.contains(&c.id)).map(|c| &c.nurbs).collect();
        if curves.len() != 4 {
            return Err(ctx.fail(format!("four border curves are needed, found {}", curves.len())));
        }
        let tol = positive(ctx, "tol_mm", params.float("tol_mm")?)?;
        let scale = curves.iter().map(|c| c.length()).fold(0.0, f64::max);
        let join_tol = (1e-3 * scale).max(1e-6);
        let order = chain_curves(&curves, join_tol).ok_or_else(|| ctx.fail("the four curves do not form a closed loop (end points must meet)"))?;
        let arcs: Vec<ArcCurve> = order.iter().map(|&(i, rev)| ArcCurve::new(curves[i], rev)).collect();
        // bottom: arcs[0] (P00 -> P10), right: arcs[1] (P10 -> P11), top: arcs[2] runs P11 -> P01, left: arcs[3] runs P01 -> P00
        let b = |u: f64| arcs[0].at(u);
        let r = |v: f64| arcs[1].at(v);
        let t = |u: f64| arcs[2].at(1.0 - u);
        let l = |v: f64| arcs[3].at(1.0 - v);
        let (p00, p10, p11, p01) = (b(0.0), b(1.0), t(1.0), t(0.0));
        let coons = |u: f64, v: f64| -> DVec3 {
            b(u) * (1.0 - v) + t(u) * v + l(v) * (1.0 - u) + r(v) * u - (p00 * ((1.0 - u) * (1.0 - v)) + p10 * (u * (1.0 - v)) + p01 * ((1.0 - u) * v) + p11 * (u * v))
        };
        let g = params.int("grid_density")?.clamp(4, 32) as usize;
        let n = 4 * g + 1;
        let mut samples = Vec::with_capacity(n * n + 4 * n * 4);
        for i in 0..n {
            for j in 0..n {
                let (u, v) = (i as f64 / (n - 1) as f64, j as f64 / (n - 1) as f64);
                samples.push(FitSample { u, v, q: coons(u, v) });
                if i == 0 || j == 0 || i == n - 1 || j == n - 1 {
                    for _ in 0..4 {
                        samples.push(FitSample { u, v, q: coons(u, v) });
                    }
                }
            }
        }
        let fit = fit_adaptive_surface(&mut samples, 3, g, g.max(8), 1.0, tol, 1e-6, 2).ok_or_else(|| ctx.fail("the fit failed"))?;
        // border check: distance of the input curves from the surface
        let mut border: f64 = 0.0;
        for c in &curves {
            for q in c.sample(64) {
                let mut best = f64::INFINITY;
                for &(u0, v0) in &[(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 0.5), (0.5, 0.0), (0.5, 1.0), (0.0, 0.5), (1.0, 0.5)] {
                    best = best.min(fit.surface.foot_point(q, u0, v0, 30).2);
                }
                border = border.max(best);
            }
        }
        let mut report = Report::new("Curve network patch");
        report.metric("fit_max_mm", Metric::new(fit.max_dev, "mm").with_limit(tol, true));
        report.metric("fit_rms_mm", Metric::new(fit.rms_dev, "mm"));
        report.metric("border_deviation_mm", Metric::new(border, "mm").with_limit(tol, true));
        report.metric("control_points_u", Metric::new(fit.surface.nu as f64, "count"));
        report.metric("control_points_v", Metric::new(fit.surface.nv as f64, "count"));
        if params.str("tangency")? == "g1" {
            placeholder_note(&mut report, "tangent continuity to neighbouring surfaces (cross-boundary derivative constraints); the patch is position-continuous only");
        }
        let ps = PatchSet::new(vec![fit.surface]);
        let preview = ps.tessellate((tol / 2.0).max(0.02));
        Ok(vec![Data::json(ps.to_json()), Data::geometry(preview), Data::report(report)])
    }
}

// ---------------------------------------------------------------------------
// loft
// ---------------------------------------------------------------------------

/// Points at equal arc-length steps along `curve` (`m + 1` points, both ends included).
pub fn resample(curve: &NurbsCurve, m: usize) -> Vec<DVec3> {
    let arc = ArcCurve::new(curve, false);
    (0..=m).map(|i| arc.at(i as f64 / m as f64)).collect()
}

fn align_closed(prev: &[DVec3], cur: &[DVec3]) -> Vec<DVec3> {
    let m = cur.len() - 1;
    let mut best = (f64::INFINITY, 0usize, false);
    for rev in [false, true] {
        for s in 0..m {
            let mut cost = 0.0;
            for i in 0..m {
                let k = if rev { (s + m - i % m) % m } else { (s + i) % m };
                cost += (cur[k] - prev[i]).length_squared();
            }
            if cost < best.0 {
                best = (cost, s, rev);
            }
        }
    }
    let (_, s, rev) = best;
    let mut out: Vec<DVec3> = (0..m).map(|i| cur[if rev { (s + m - i % m) % m } else { (s + i) % m }]).collect();
    out.push(out[0]);
    out
}

fn align_open(prev: &[DVec3], cur: &[DVec3]) -> Vec<DVec3> {
    let n = cur.len() - 1;
    let fwd = (cur[0] - prev[0]).length_squared() + (cur[n] - prev[prev.len() - 1]).length_squared();
    let bwd = (cur[n] - prev[0]).length_squared() + (cur[0] - prev[prev.len() - 1]).length_squared();
    if bwd < fwd {
        cur.iter().rev().copied().collect()
    } else {
        cur.to_vec()
    }
}

/// Global interpolation of `n` rows with degree `q`; returns (knots, control rows).
fn interpolate_rows(rows: &[Vec<DVec3>], q: usize) -> Option<(Vec<f64>, Vec<Vec<DVec3>>)> {
    let n = rows.len();
    let ncols = rows[0].len();
    // averaged chord-length parameters
    let mut vbar = vec![0.0; n];
    for j in 0..ncols {
        let mut cum = vec![0.0; n];
        for i in 1..n {
            cum[i] = cum[i - 1] + (rows[i][j] - rows[i - 1][j]).length();
        }
        let tot = cum[n - 1];
        for i in 0..n {
            vbar[i] += if tot > 1e-12 { cum[i] / tot } else { i as f64 / (n - 1) as f64 };
        }
    }
    for v in &mut vbar {
        *v /= ncols as f64;
    }
    vbar[0] = 0.0;
    vbar[n - 1] = 1.0;
    let mut knots = vec![0.0; q + 1];
    for j in 1..n - q {
        knots.push(vbar[j..j + q].iter().sum::<f64>() / q as f64);
    }
    knots.extend(std::iter::repeat_n(1.0, q + 1));
    let mut a = vec![0.0; n * n];
    for (k, &v) in vbar.iter().enumerate() {
        let sp = find_span(n, q, v, &knots);
        let b = basis_funs(sp, v, q, &knots);
        for (i, bi) in b.iter().enumerate() {
            a[k * n + sp - q + i] = *bi;
        }
    }
    let mut out = vec![vec![DVec3::ZERO; ncols]; n];
    for j in 0..ncols {
        for c in 0..3 {
            let rhs: Vec<f64> = (0..n).map(|i| rows[i][j][c]).collect();
            let sol = solve_dense(&a, n, &rhs)?;
            for i in 0..n {
                out[i][j][c] = sol[i];
            }
        }
    }
    Some((knots, out))
}

/// `surface.loft_sections`
pub struct LoftSectionsNode;

impl NodeOp for LoftSectionsNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.loft_sections", "Loft Sections", SPEC_RE)
            .input(PortSpec::new("curves", DataKind::Json))
            .output(PortSpec::new("patches", DataKind::Json))
            .output(PortSpec::new("preview", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::int("degree_v", "Degree between sections", 3, 1, 3).help("1 is ruled between sections; 2 and 3 are smooth interpolations through every section."))
            .param(ParamSpec::int("control_points", "Control points per section", 24, 4, 200).help("Control points along each section curve."))
            .param(ParamSpec::int("loop_id", "Loop", 0, 0, 100).help("Which outline of a section to use when a section has several."))
            .param(ParamSpec::float("tol_mm", "Tolerance", 0.1, 0.001, 10.0).unit("mm").help("Largest accepted distance between the surface and the section curves."))
            .param(ParamSpec::float("preview_tol_mm", "Preview tolerance", 0.2, 0.001, 10.0).unit("mm").help("Chord tolerance of the preview mesh."))
            .help("Skins a surface through a stack of section curves. Sections are ordered by their section id, matched in start point and direction, fitted with a common knot vector and interpolated across the stack. The report measures how far the surface lies from every input curve.")
            .emulates("Design X loft")
            .emulates("Fusion loft through sections")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let cj = json_input(inputs, 0).ok_or_else(|| ctx.fail("connect a curve set"))?;
        let set = CurveSet::from_json(&cj).map_err(|e| json_err(ctx, "curve set input", e))?;
        let loop_id = params.int("loop_id")?;
        let tol = positive(ctx, "tol_mm", params.float("tol_mm")?)?;
        let ncp = params.int("control_points")?.clamp(4, 200) as usize;
        // one curve per section
        let mut by_section: std::collections::BTreeMap<i64, &crate::nurbs::CurveEntry> = std::collections::BTreeMap::new();
        for c in &set.curves {
            let e = by_section.entry(c.section_id).or_insert(c);
            if c.loop_id == loop_id && e.loop_id != loop_id {
                *e = c;
            }
        }
        let mut entries: Vec<&crate::nurbs::CurveEntry> = by_section.values().copied().collect();
        if entries.len() < set.curves.len().min(2) && set.curves.len() >= 2 && by_section.len() == 1 {
            // no usable section ids: order the curves along their principal direction
            entries = set.curves.iter().collect();
            let cents: Vec<DVec3> = entries.iter().map(|e| e.nurbs.sample(16).iter().fold(DVec3::ZERO, |s, p| s + *p) / 16.0).collect();
            let (_, ax) = crate::fit::plane_frame(&cents);
            let c0 = cents.iter().fold(DVec3::ZERO, |s, p| s + *p) / cents.len() as f64;
            let mut idx: Vec<usize> = (0..entries.len()).collect();
            idx.sort_by(|&a, &b| (cents[a] - c0).dot(ax[0]).total_cmp(&(cents[b] - c0).dot(ax[0])));
            entries = idx.into_iter().map(|i| entries[i]).collect();
        }
        if entries.len() < 2 {
            return Err(ctx.fail("a loft needs at least two section curves"));
        }
        let m = (8 * ncp).max(96);
        let mut rows_pts: Vec<Vec<DVec3>> = Vec::new();
        for e in entries.iter() {
            let raw = resample(&e.nurbs, m);
            let aligned = match rows_pts.last() {
                None => raw,
                Some(prev) => {
                    if e.closed {
                        align_closed(prev, &raw)
                    } else {
                        align_open(prev, &raw)
                    }
                }
            };
            rows_pts.push(aligned);
        }
        let knots_u = clamped_uniform_knots(ncp, 3);
        let params_u: Vec<f64> = (0..=m).map(|i| i as f64 / m as f64).collect();
        let mut rows = Vec::with_capacity(rows_pts.len());
        for p in &rows_pts {
            let c = fit_with_knots(p, &params_u, 3, &knots_u, None, None).ok_or_else(|| ctx.fail("a section could not be fitted"))?;
            rows.push(c.control_points);
        }
        let q = (params.int("degree_v")?.clamp(1, 3) as usize).min(rows.len() - 1);
        let (knots_v, net) = interpolate_rows(&rows, q).ok_or_else(|| ctx.fail("the section interpolation system is singular"))?;
        let nv = net.len();
        let mut cps = Vec::with_capacity(ncp * nv);
        for j in 0..ncp {
            for row in net.iter() {
                cps.push(row[j]);
            }
        }
        let surf = NurbsSurface::new(3, q, knots_u, knots_v, ncp, nv, cps);
        surf.validate().map_err(|e| ctx.fail(format!("lofted surface is invalid: {e}")))?;
        // deviation of every section curve from the surface
        let mut dev_max: f64 = 0.0;
        let (v_start, v_end) = surf.domain_v();
        for i in 0..entries.len() {
            let v0 = v_start + (v_end - v_start) * i as f64 / (entries.len() - 1) as f64;
            for (k, p) in rows_pts[i].iter().enumerate().step_by((m / 48).max(1)) {
                let (_, _, d) = surf.foot_point(*p, k as f64 / m as f64, v0, 20);
                dev_max = dev_max.max(d);
            }
        }
        let mut report = Report::new("Loft");
        report.metric("section_count", Metric::new(entries.len() as f64, "count"));
        report.metric("degree_v", Metric::new(q as f64, ""));
        report.metric("control_points", Metric::new((ncp * nv) as f64, "count"));
        report.metric("section_deviation_mm", Metric::new(dev_max, "mm").with_limit(tol, true));
        if entries.iter().any(|e| e.closed) {
            report.note(Severity::Info, "closed sections: the seam sits at the start point of the first section");
        }
        let mut ps = PatchSet::new(vec![surf]);
        ps.closed = false;
        let preview = ps.tessellate(positive(ctx, "preview_tol_mm", params.float("preview_tol_mm")?)?);
        Ok(vec![Data::json(ps.to_json()), Data::geometry(preview), Data::report(report)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::CurveEntry;
    use crate::testutil::{b, cook, cylinder, f, filleted_box, grid, i, report_of, s, some, v3};
    use std::f64::consts::PI;

    fn line_curve(id: usize, a: DVec3, bb: DVec3) -> CurveEntry {
        CurveEntry { id, section_id: 0, loop_id: id as i64, kind: "line".into(), closed: false, max_dev_mm: 0.0, nurbs: NurbsCurve::clamped_uniform(vec![a, bb], 1), params: serde_json::Value::Null }
    }

    #[test]
    fn fit_patch_on_the_sine_sheet() {
        let g = grid(60, 60, 100.0, 100.0, |x, _| 5.0 * (PI * x / 50.0).sin());
        let out = cook("surface.fit_patch", &[some(&g)], &[("tol_mm", f(0.05)), ("init_cp", i(8)), ("max_cp", i(24))]).unwrap();
        let r = report_of(&out[2]);
        assert!(r.passes(), "{:?}", r.metrics);
        assert!(r.metrics["fit_max_mm"].value <= 0.05);
        let ps = PatchSet::from_json(out[0].as_json().unwrap()).unwrap();
        assert_eq!(ps.patches.len(), 1);
    }

    #[test]
    fn fit_patch_handles_a_non_height_field_strip() {
        // half cylinder, 180 degree wrap: not a height field, needs a boundary or conformal map
        let (nu, nv) = (40usize, 12usize);
        let r = 30.0;
        let mut g = vernon_core::Geometry::new();
        for a in 0..=nu {
            for b2 in 0..=nv {
                let ang = PI * a as f64 / nu as f64;
                g.points.push(DVec3::new(r * ang.cos(), r * ang.sin(), 40.0 * b2 as f64 / nv as f64));
            }
        }
        let id = |a: usize, b2: usize| (a * (nv + 1) + b2) as u32;
        for a in 0..nu {
            for b2 in 0..nv {
                g.prims.push(vernon_core::Prim::polygon(vec![id(a, b2), id(a + 1, b2), id(a + 1, b2 + 1), id(a, b2 + 1)]));
            }
        }
        let out = cook("surface.fit_patch", &[some(&g)], &[("tol_mm", f(0.2)), ("param", s("boundary"))]).unwrap();
        let rep = report_of(&out[2]);
        assert!(rep.metrics["fit_max_mm"].value <= 0.2, "{:?}", rep.metrics["fit_max_mm"]);
        // auto picks a non-projection map here
        let auto = cook("surface.fit_patch", &[some(&g)], &[("tol_mm", f(0.2))]).unwrap();
        assert!(report_of(&auto[2]).notes.iter().any(|(_, t)| t.contains("boundary") || t.contains("conformal")));
    }

    #[test]
    fn closed_input_is_a_clear_error() {
        let g = crate::testutil::sphere(10.0, 16, 8);
        assert!(cook("surface.fit_patch", &[some(&g)], &[("param", s("conformal"))]).is_err());
    }

    #[test]
    fn auto_surface_lays_one_patch_per_region() {
        let g = filleted_box(40.0, 5.0, 6);
        let out = cook("surface.auto_surface", &[some(&g)], &[("tol_mm", f(0.3))]).unwrap();
        let r = report_of(&out[2]);
        assert_eq!(r.metrics["patch_count"].value + r.metrics["skipped_regions"].value, r.metrics["region_count"].value);
        assert!(r.metrics["patch_count"].value >= 6.0, "{:?}", r.metrics);
        let fine = cook("surface.auto_surface", &[some(&g)], &[("tol_mm", f(0.3)), ("detail", s("fine"))]).unwrap();
        assert!(report_of(&fine[2]).metrics["region_count"].value >= r.metrics["region_count"].value);
        assert!(r.notes.iter().any(|(_, t)| t.starts_with("white-box placeholder")));
        // fitted region by region, the worst data distance stays small
        assert!(r.metrics["dev_max_mm"].value < 1.0, "{}", r.metrics["dev_max_mm"].value);
        let few = cook("surface.auto_surface", &[some(&g)], &[("target_patches", i(8))]).unwrap();
        assert!(report_of(&few[2]).metrics["region_count"].value <= 8.0);
    }

    #[test]
    fn coons_patch_of_a_skew_quad_is_bilinear() {
        let (a, bb, c, d) = (DVec3::new(0.0, 0.0, 0.0), DVec3::new(100.0, 0.0, 10.0), DVec3::new(100.0, 80.0, 0.0), DVec3::new(0.0, 80.0, 20.0));
        let set = CurveSet::new(vec![line_curve(0, a, bb), line_curve(1, bb, c), line_curve(2, d, c), line_curve(3, a, d)]);
        let out = cook("surface.curve_network_patch", &[Some(Data::json(set.to_json())), None], &[("tol_mm", f(0.01))]).unwrap();
        let rep = report_of(&out[2]);
        assert!(rep.metrics["border_deviation_mm"].value < 0.01, "{:?}", rep.metrics);
        let ps = PatchSet::from_json(out[0].as_json().unwrap()).unwrap();
        let mid = ps.patches[0].surface.point(0.5, 0.5);
        assert!((mid - (a + bb + c + d) / 4.0).length() < 0.01, "{mid:?}");
        // three curves are rejected
        let bad = CurveSet::new(vec![line_curve(0, a, bb), line_curve(1, bb, c), line_curve(2, c, a)]);
        assert!(cook("surface.curve_network_patch", &[Some(Data::json(bad.to_json())), None], &[]).is_err());
    }

    #[test]
    fn loft_through_cylinder_sections() {
        let cyl = cylinder(25.0, 100.0, 96);
        let secs = cook("surface.section_curves", &[some(&cyl)], &[("mode", s("planes_along_axis")), ("normal", v3(0.0, 0.0, 1.0)), ("count", i(5)), ("spacing_mm", f(20.0)), ("from_bounds", b(true))]).unwrap();
        let fitted = cook("surface.fit_section_curves", &[Some(secs[0].clone())], &[("curve_type", s("closed_spline")), ("spline_fit_tol_mm", f(0.02))]).unwrap();
        let out = cook("surface.loft_sections", &[Some(fitted[0].clone())], &[("tol_mm", f(0.1))]).unwrap();
        let rep = report_of(&out[2]);
        assert_eq!(rep.metrics["section_count"].value, 5.0);
        assert!(rep.metrics["section_deviation_mm"].value <= 0.1, "{:?}", rep.metrics["section_deviation_mm"]);
        // the lofted skin of a straight cylinder keeps the radius between sections
        let ps = PatchSet::from_json(out[0].as_json().unwrap()).unwrap();
        let sfc = &ps.patches[0].surface;
        for k in 0..20 {
            let p = sfc.point(k as f64 / 19.0, 0.37);
            let rad = (p.x * p.x + p.y * p.y).sqrt();
            assert!((rad - 25.0).abs() < 0.1, "radius {rad}");
        }
        // ruled loft passes through every section
        let ruled = cook("surface.loft_sections", &[Some(fitted[0].clone())], &[("degree_v", i(1))]).unwrap();
        assert!(report_of(&ruled[2]).metrics["section_deviation_mm"].value <= 0.1);
        assert_eq!(report_of(&ruled[2]).metrics["degree_v"].value, 1.0);
    }

    #[test]
    fn loft_needs_two_sections() {
        let set = CurveSet::new(vec![line_curve(0, DVec3::ZERO, DVec3::X)]);
        assert!(cook("surface.loft_sections", &[Some(Data::json(set.to_json()))], &[]).is_err());
    }
}

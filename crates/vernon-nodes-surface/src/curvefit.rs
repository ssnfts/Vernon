//! Least-squares B-spline curve fitting with an error bound, line / arc /
//! circle fits, and the `surface.fit_section_curves` node.

use glam::DVec3;
use serde_json::json;
use vernon_core::{AttribArray, Metric, Report, Severity};
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::linalg::{scatter, solve_dense, solve_spd, sym_eigen3};
use crate::nurbs::{arc_nurbs, basis_funs, find_span, CurveEntry, CurveSet, NurbsCurve};
use crate::util::{check_finite, node_info, polylines_geometry, positive, SPEC_RE};

/// Chord-length (or centripetal) parameters normalised to `[0, 1]`.
pub fn chord_params(pts: &[DVec3], centripetal: bool) -> Vec<f64> {
    let mut u = vec![0.0; pts.len()];
    for i in 1..pts.len() {
        let d = (pts[i] - pts[i - 1]).length();
        u[i] = u[i - 1] + if centripetal { d.sqrt() } else { d };
    }
    let total = *u.last().unwrap_or(&0.0);
    if total <= 0.0 {
        return (0..pts.len()).map(|i| i as f64 / (pts.len().max(2) - 1) as f64).collect();
    }
    for v in &mut u {
        *v /= total;
    }
    if let Some(l) = u.last_mut() {
        *l = 1.0;
    }
    u
}

/// Knot vector for `n` control points by averaging the parameters (NURBS book 9.69).
pub fn averaged_knots(params: &[f64], degree: usize, n: usize) -> Vec<f64> {
    let m = params.len() - 1;
    let mut knots = vec![0.0; degree + 1];
    if n > degree + 1 {
        let d = (m + 1) as f64 / (n - degree) as f64;
        for j in 1..n - degree {
            let i = (j as f64 * d) as usize;
            let a = j as f64 * d - i as f64;
            let lo = params[(i.max(1)) - 1];
            let hi = params[i.min(m)];
            knots.push((1.0 - a) * lo + a * hi);
        }
    }
    knots.extend(std::iter::repeat_n(1.0, degree + 1));
    knots
}

/// Least-squares fit over a given clamped knot vector with both end points
/// interpolated. Optional end tangent directions are enforced by projecting
/// the second / second-last control point onto them.
pub fn fit_with_knots(
    pts: &[DVec3],
    params: &[f64],
    degree: usize,
    knots: &[f64],
    tan0: Option<DVec3>,
    tan1: Option<DVec3>,
) -> Option<NurbsCurve> {
    let n = knots.len() - degree - 1;
    let m = pts.len() - 1;
    if n < degree + 1 || m < 1 {
        return None;
    }
    let (q0, qm) = (pts[0], pts[m]);
    let free = n.saturating_sub(2);
    let mut cps = vec![DVec3::ZERO; n];
    cps[0] = q0;
    cps[n - 1] = qm;
    if free > 0 {
        let mut a = vec![0.0; free * free];
        let mut rhs = vec![DVec3::ZERO; free];
        for k in 1..m {
            let u = params[k];
            let span = find_span(n, degree, u, knots);
            let b = basis_funs(span, u, degree, knots);
            let mut r = pts[k];
            let mut entries: Vec<(usize, f64)> = Vec::with_capacity(degree + 1);
            for (j, bj) in b.iter().enumerate() {
                let i = span - degree + j;
                if i == 0 {
                    r -= q0 * *bj;
                } else if i == n - 1 {
                    r -= qm * *bj;
                } else {
                    entries.push((i - 1, *bj));
                }
            }
            for &(ia, ba) in &entries {
                rhs[ia] += r * ba;
                for &(ib, bb) in &entries {
                    a[ia * free + ib] += ba * bb;
                }
            }
        }
        // tiny pull towards the data polyline at each Greville abscissa keeps
        // spans without data well posed
        let lambda = 1e-9 * (0..free).map(|i| a[i * free + i]).fold(0.0, f64::max).max(1e-12);
        for i in 0..free {
            let g = (1..=degree).map(|k| knots[i + 1 + k]).sum::<f64>() / degree as f64;
            let j = params.partition_point(|&p| p < g).clamp(1, m);
            let t = ((g - params[j - 1]) / (params[j] - params[j - 1]).max(1e-300)).clamp(0.0, 1.0);
            let prior = pts[j - 1] + (pts[j] - pts[j - 1]) * t;
            a[i * free + i] += lambda;
            rhs[i] += prior * lambda;
        }
        let cols: Vec<Vec<f64>> = (0..3).map(|c| rhs.iter().map(|v| v[c]).collect()).collect();
        let sol = solve_spd(&a, free, &cols)?;
        for i in 0..free {
            cps[i + 1] = DVec3::new(sol[0][i], sol[1][i], sol[2][i]);
        }
    }
    if n >= 4 {
        if let Some(t) = tan0.filter(|t| t.length_squared() > 0.0) {
            let t = t.normalize();
            cps[1] = cps[0] + t * (cps[1] - cps[0]).dot(t).max(0.0);
        }
        if let Some(t) = tan1.filter(|t| t.length_squared() > 0.0) {
            let t = t.normalize();
            cps[n - 2] = cps[n - 1] - t * (cps[n - 1] - cps[n - 2]).dot(t).max(0.0);
        }
    }
    Some(NurbsCurve { degree, knots: knots.to_vec(), control_points: cps, weights: None })
}

/// Distance from `p` to segment `a`-`b`.
pub fn point_segment_dist(p: DVec3, a: DVec3, b: DVec3) -> f64 {
    let ab = b - a;
    let l2 = ab.length_squared();
    if l2 < 1e-300 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// Deviation of `curve` from the data: the largest and RMS foot-point distance
/// of the data points to the curve. The chord sagitta of the data polyline
/// itself is a property of the data and is not counted against the curve.
pub fn deviation(curve: &NurbsCurve, pts: &[DVec3], _closed_loop: bool) -> (f64, f64) {
    let mut max_d: f64 = 0.0;
    let mut sum2 = 0.0;
    for &q in pts {
        let (_, _, d) = curve.project(q);
        max_d = max_d.max(d);
        sum2 += d * d;
    }
    (max_d, (sum2 / pts.len().max(1) as f64).sqrt())
}

/// Result of an adaptive fit.
#[derive(Debug, Clone)]
pub struct CurveFit {
    pub curve: NurbsCurve,
    pub max_dev: f64,
    pub rms: f64,
}

/// Fit with knot insertion at the worst-residual parameter until the maximum
/// deviation is within `tol` (or `max_ctrl` is reached). The insertion
/// sequence depends only on the data, so a smaller `tol` never yields fewer
/// control points.
pub fn fit_adaptive(
    pts: &[DVec3],
    degree: usize,
    tol: f64,
    max_ctrl: usize,
    closed_loop: bool,
    tan0: Option<DVec3>,
    tan1: Option<DVec3>,
) -> Option<CurveFit> {
    let m = pts.len() - 1;
    if m < 1 {
        return None;
    }
    let degree = degree.clamp(1, 5).min(m);
    let params = chord_params(pts, false);
    // foot-point parameters, used only to decide where to insert knots
    let mut foot = params.clone();
    let mut knots = {
        let mut k = vec![0.0; degree + 1];
        k.extend(std::iter::repeat_n(1.0, degree + 1));
        k
    };
    let max_ctrl = max_ctrl.max(degree + 1).min(m + 1);
    let mut best: Option<NurbsCurve> = None;
    for _ in 0..max_ctrl * 2 {
        let curve = fit_with_knots(pts, &params, degree, &knots, tan0, tan1)?;
        let mut resid = vec![0.0; pts.len()];
        let mut worst = (0usize, -1.0);
        for (k, &q) in pts.iter().enumerate() {
            let (u, d) = curve.project_from(q, foot[k]);
            resid[k] = d;
            foot[k] = u;
            if d > worst.1 {
                worst = (k, d);
            }
        }
        let n_ctrl = knots.len() - degree - 1;
        let done = worst.1 <= tol * 0.9 && deviation(&curve, pts, closed_loop).0 <= tol;
        best = Some(curve);
        if done || n_ctrl >= max_ctrl {
            break;
        }
        // insert in the span with the largest residual that still holds enough
        // data on both sides, at the median data parameter
        let (vals, _) = crate::nurbs::compress_knots(&knots);
        let nspans = vals.len() - 1;
        let mut span_err = vec![0.0f64; nspans];
        let mut span_pts: Vec<Vec<usize>> = vec![Vec::new(); nspans];
        for k in 0..pts.len() {
            let s = vals.windows(2).position(|w| params[k] < w[1]).unwrap_or(nspans - 1);
            span_err[s] = span_err[s].max(resid[k]);
            span_pts[s].push(k);
        }
        let mut order: Vec<usize> = (0..nspans).filter(|&s| span_pts[s].len() >= 3).collect();
        order.sort_by(|&a, &b| span_err[b].total_cmp(&span_err[a]).then(a.cmp(&b)));
        let Some(&s) = order.first() else { break };
        let mid_k = span_pts[s][span_pts[s].len() / 2];
        let u_new = params[mid_k];
        if u_new <= vals[s] + 1e-12 || u_new >= vals[s + 1] - 1e-12 {
            break;
        }
        let pos = knots.partition_point(|&k| k <= u_new);
        knots.insert(pos, u_new);
    }
    let curve = best?;
    let (max_dev, rms) = deviation(&curve, pts, closed_loop);
    Some(CurveFit { curve, max_dev, rms })
}

/// Best-fit line (total least squares) as a degree-1 curve between the
/// projections of the first and last data points.
pub fn fit_line(pts: &[DVec3]) -> (NurbsCurve, f64) {
    let (c, m) = scatter(pts, None);
    let (_, vecs) = sym_eigen3(m);
    let dir = vecs[2];
    let proj = |p: DVec3| c + dir * (p - c).dot(dir);
    let (a, b) = (proj(pts[0]), proj(*pts.last().expect("non-empty")));
    let max_dev = pts.iter().map(|&p| (p - proj(p)).length()).fold(0.0, f64::max);
    (NurbsCurve { degree: 1, knots: vec![0.0, 0.0, 1.0, 1.0], control_points: vec![a, b], weights: None }, max_dev)
}

/// Result of a circle / arc fit.
#[derive(Debug, Clone)]
pub struct CircleFit {
    pub center: DVec3,
    pub normal: DVec3,
    pub radius: f64,
    pub x_axis: DVec3,
    pub y_axis: DVec3,
    pub start_angle: f64,
    /// Signed angular sweep along the data order (negative = clockwise about `normal`).
    pub sweep: f64,
    pub max_dev: f64,
    pub rms: f64,
}

/// Fit a circle (Kasa algebraic fit refined by Gauss-Newton on the geometric
/// distance) in the best-fit plane of `pts`.
pub fn fit_circle(pts: &[DVec3]) -> Option<CircleFit> {
    if pts.len() < 3 {
        return None;
    }
    let (c0, m) = scatter(pts, None);
    let (vals, vecs) = sym_eigen3(m);
    if vals[1] <= 1e-18 * vals[2].max(1e-300) {
        return None; // collinear
    }
    let normal = vecs[0];
    let (x_axis, y_axis) = (vecs[2], vecs[1]);
    let xy: Vec<(f64, f64)> = pts.iter().map(|&p| ((p - c0).dot(x_axis), (p - c0).dot(y_axis))).collect();
    // Kasa: x^2 + y^2 + D x + E y + F = 0
    let mut a = [0.0; 9];
    let mut rhs = [0.0; 3];
    for &(x, y) in &xy {
        let row = [x, y, 1.0];
        let t = -(x * x + y * y);
        for i in 0..3 {
            rhs[i] += row[i] * t;
            for j in 0..3 {
                a[i * 3 + j] += row[i] * row[j];
            }
        }
    }
    let sol = solve_dense(&a, 3, &rhs)?;
    let (mut cx, mut cy) = (-sol[0] / 2.0, -sol[1] / 2.0);
    let r2 = cx * cx + cy * cy - sol[2];
    if r2 <= 0.0 {
        return None;
    }
    let mut r = r2.sqrt();
    for _ in 0..8 {
        let mut jtj = [0.0; 9];
        let mut jtr = [0.0; 3];
        for &(x, y) in &xy {
            let (dx, dy) = (x - cx, y - cy);
            let d = (dx * dx + dy * dy).sqrt().max(1e-300);
            let res = d - r;
            let j = [-dx / d, -dy / d, -1.0];
            for i in 0..3 {
                jtr[i] += j[i] * res;
                for k in 0..3 {
                    jtj[i * 3 + k] += j[i] * j[k];
                }
            }
        }
        let Some(step) = solve_dense(&jtj, 3, &jtr) else { break };
        cx -= step[0];
        cy -= step[1];
        r -= step[2];
        if step.iter().map(|s| s.abs()).fold(0.0, f64::max) < 1e-13 * r.abs().max(1.0) {
            break;
        }
    }
    if !(r.is_finite() && r > 0.0) {
        return None;
    }
    let center = c0 + x_axis * cx + y_axis * cy;
    let mut sweep = 0.0;
    let ang = |&(x, y): &(f64, f64)| (y - cy).atan2(x - cx);
    let start_angle = ang(&xy[0]);
    for w in xy.windows(2) {
        let mut d = ang(&w[1]) - ang(&w[0]);
        while d > std::f64::consts::PI {
            d -= std::f64::consts::TAU;
        }
        while d < -std::f64::consts::PI {
            d += std::f64::consts::TAU;
        }
        sweep += d;
    }
    let (mut max_dev, mut sum2) = (0.0f64, 0.0);
    for &(x, y) in &xy {
        let d = (((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r).abs();
        max_dev = max_dev.max(d);
        sum2 += d * d;
    }
    Some(CircleFit { center, normal, radius: r, x_axis, y_axis, start_angle, sweep, max_dev, rms: (sum2 / xy.len() as f64).sqrt() })
}

impl CircleFit {
    /// NURBS arc along the data direction covering `sweep` (or the full circle).
    pub fn to_nurbs(&self, full_circle: bool) -> NurbsCurve {
        let sweep = if full_circle { std::f64::consts::TAU * self.sweep.signum() } else { self.sweep };
        if sweep >= 0.0 {
            arc_nurbs(self.center, self.x_axis, self.y_axis, self.radius, self.start_angle, self.start_angle + sweep)
        } else {
            // clockwise: use the mirrored y axis so the sweep is positive
            arc_nurbs(self.center, self.x_axis, -self.y_axis, self.radius, -self.start_angle, -self.start_angle - sweep)
        }
    }
}

/// Split a closed loop between the vertices nearest to `a` and `b` and keep
/// the shorter or longer arc.
pub fn pick_arc(pts: &[DVec3], a: DVec3, b: DVec3, short: bool) -> Vec<DVec3> {
    let n = pts.len();
    let nearest = |q: DVec3| (0..n).min_by(|&i, &j| (pts[i] - q).length_squared().total_cmp(&(pts[j] - q).length_squared())).unwrap_or(0);
    let (ia, ib) = (nearest(a), nearest(b));
    if ia == ib {
        return pts.to_vec();
    }
    let walk = |from: usize, to: usize| -> Vec<DVec3> {
        let mut out = vec![pts[from]];
        let mut i = from;
        while i != to {
            i = (i + 1) % n;
            out.push(pts[i]);
        }
        out
    };
    let len = |v: &[DVec3]| v.windows(2).map(|w| (w[1] - w[0]).length()).sum::<f64>();
    let (p1, p2) = (walk(ia, ib), walk(ib, ia));
    let (l1, l2) = (len(&p1), len(&p2));
    if (l1 <= l2) == short {
        p1
    } else {
        p2
    }
}

/// `surface.fit_section_curves`
pub struct FitSectionCurvesNode;

impl NodeOp for FitSectionCurvesNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.fit_section_curves", "Fit Section Curves", SPEC_RE)
            .input(PortSpec::new("sections", DataKind::Geometry))
            .output(PortSpec::new("curves", DataKind::Json))
            .output(PortSpec::new("preview", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("curve_type", "Curve type", "auto", &["auto", "line", "arc", "spline", "closed_spline", "circle", "ellipse"]).help("Type of curve to fit; auto picks the simplest type within the deviation cap."))
            .param(ParamSpec::choice("path", "Path", "short", &["short", "long"]).help("Which arc of a closed outline to keep when two pick points are used."))
            .param(ParamSpec::bool("use_picks", "Use pick points", false).help("Cut closed outlines between the two pick points."))
            .param(ParamSpec::vec3("pick_a", "Pick A", DVec3::ZERO).unit("mm").help("First pick point."))
            .param(ParamSpec::vec3("pick_b", "Pick B", DVec3::ZERO).unit("mm").help("Second pick point."))
            .param(ParamSpec::float("spline_fit_tol_mm", "Spline fit tolerance", 0.05, 0.001, 5.0).unit("mm").help("Lower values fit tighter and use more control points."))
            .param(ParamSpec::float("max_deviation_mm", "Max curve deviation", 0.1, 0.001, 10.0).unit("mm").help("Largest allowed distance between a fitted curve and its outline."))
            .param(ParamSpec::int("degree", "Spline degree", 3, 1, 5).help("Polynomial degree of fitted splines."))
            .param(ParamSpec::int("max_control_points", "Max control points", 128, 8, 512).help("Upper bound on spline control points."))
            .param(ParamSpec::bool("match_tangent_start", "Match tangent at start", false).help("Force the start tangent direction."))
            .param(ParamSpec::bool("match_tangent_end", "Match tangent at end", false).help("Force the end tangent direction."))
            .param(ParamSpec::vec3("tangent_start", "Start tangent", DVec3::ZERO).help("Direction used when start tangent matching is on."))
            .param(ParamSpec::vec3("tangent_end", "End tangent", DVec3::ZERO).help("Direction used when end tangent matching is on."))
            .help("Fits lines, arcs, circles or least-squares splines to section outlines. Every fit reports its measured deviation from the outline; splines add control points until the tolerance is met.")
            .emulates("Fusion Fit Curves to Mesh Section")
            .emulates("Design X mesh sketch entity fit")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let geo = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &geo)?;
        let ctype = params.str("curve_type")?.to_string();
        let spline_tol = positive(ctx, "spline_fit_tol_mm", params.float("spline_fit_tol_mm")?)?;
        let cap = positive(ctx, "max_deviation_mm", params.float("max_deviation_mm")?)?;
        let degree = params.int("degree")?.clamp(1, 5) as usize;
        let max_ctrl = params.int("max_control_points")?.clamp(8, 512) as usize;
        let tan0 = if params.bool("match_tangent_start")? { Some(params.vec3("tangent_start")?) } else { None };
        let tan1 = if params.bool("match_tangent_end")? { Some(params.vec3("tangent_end")?) } else { None };
        let use_picks = params.bool("use_picks")?;
        let short = params.str("path")? == "short";
        let (pick_a, pick_b) = (params.vec3("pick_a")?, params.vec3("pick_b")?);

        let sec_ids = geo.prim_attribs.ints("section_id").ok();
        let loop_ids = geo.prim_attribs.ints("loop_id").ok();
        let closed_attr = geo.prim_attribs.ints("loop_closed").ok();

        let mut report = Report::new("Fit section curves");
        let mut entries: Vec<CurveEntry> = Vec::new();
        let mut max_dev_all: f64 = 0.0;
        let mut rms_sum = 0.0;
        let mut ctrl_total = 0usize;
        let mut counts = std::collections::BTreeMap::<String, usize>::new();
        let mut ellipse_warned = false;
        for (pi, prim) in geo.prims.iter().enumerate() {
            if prim.vertices.len() < 2 {
                continue;
            }
            let mut pts: Vec<DVec3> = prim.vertices.iter().map(|&v| geo.points[v as usize]).collect();
            let repeated = prim.vertices.first() == prim.vertices.last() || pts.first() == pts.last();
            let mut closed = closed_attr.map_or(repeated, |c| c[pi] != 0) || (prim.closed && prim.vertices.len() > 2);
            if closed && pts.first() == pts.last() {
                pts.pop();
            }
            if pts.len() < 2 {
                continue;
            }
            if closed && use_picks && pts.len() > 3 {
                pts = pick_arc(&pts, pick_a, pick_b, short);
                closed = false;
            }
            let (kind, curve, max_dev, rms, extra) = fit_one(&pts, closed, &ctype, spline_tol, cap, degree, max_ctrl, tan0, tan1, &mut ellipse_warned, &mut report);
            if max_dev > cap + 1e-12 {
                report.note(Severity::Warning, format!("outline {pi}: fitted {kind} deviates {max_dev:.4} mm, above the {cap} mm cap"));
            }
            max_dev_all = max_dev_all.max(max_dev);
            rms_sum += rms;
            ctrl_total += curve.control_points.len();
            *counts.entry(kind.clone()).or_default() += 1;
            let is_closed_curve = closed && (kind == "circle" || kind == "closed_spline");
            entries.push(CurveEntry {
                id: entries.len(),
                section_id: sec_ids.map_or(0, |s| s[pi]),
                loop_id: loop_ids.map_or(pi as i64, |s| s[pi]),
                kind,
                closed: is_closed_curve,
                max_dev_mm: max_dev,
                nurbs: curve,
                params: extra,
            });
        }
        if entries.is_empty() {
            return Err(ctx.fail("no polylines with at least two points to fit"));
        }
        let lines: Vec<(Vec<DVec3>, bool)> = entries.iter().map(|e| (e.nurbs.sample_spans(8).into_iter().map(|(_, p)| p).collect(), false)).collect();
        let mut preview = polylines_geometry(&lines);
        preview.prim_attribs.set("section_id", AttribArray::Int(entries.iter().map(|e| e.section_id).collect()));
        let n = entries.len();
        report.metric("curve_count", Metric::new(n as f64, "count"));
        report.metric("fit_max_mm", Metric::new(max_dev_all, "mm").with_limit(cap, true));
        report.metric("fit_rms_mm", Metric::new(rms_sum / n as f64, "mm"));
        report.metric("control_points", Metric::new(ctrl_total as f64, "count"));
        for (k, v) in counts {
            report.metric(format!("count_{k}"), Metric::new(v as f64, "count"));
        }
        let set = CurveSet::new(entries);
        Ok(vec![Data::json(set.to_json()), Data::geometry(preview), Data::report(report)])
    }
}

type FitOne = (String, NurbsCurve, f64, f64, serde_json::Value);

#[allow(clippy::too_many_arguments)]
fn fit_one(
    pts: &[DVec3],
    closed: bool,
    ctype: &str,
    spline_tol: f64,
    cap: f64,
    degree: usize,
    max_ctrl: usize,
    tan0: Option<DVec3>,
    tan1: Option<DVec3>,
    ellipse_warned: &mut bool,
    report: &mut Report,
) -> FitOne {
    let spline = |pts: &[DVec3], closed: bool, name: &str| -> FitOne {
        let mut data = pts.to_vec();
        if closed {
            data.push(pts[0]);
        }
        let fit = fit_adaptive(&data, degree, spline_tol, max_ctrl, closed, tan0, tan1)
            .unwrap_or_else(|| {
                let (c, d) = fit_line(&data);
                CurveFit { curve: c, max_dev: d, rms: d }
            });
        (name.to_string(), fit.curve, fit.max_dev, fit.rms, json!({"spline_fit_tol_mm": spline_tol}))
    };
    let line = |pts: &[DVec3]| -> FitOne {
        let (c, d) = fit_line(pts);
        ("line".to_string(), c, d, d, json!({}))
    };
    let circle = |pts: &[DVec3], full: bool| -> Option<FitOne> {
        let cf = fit_circle(pts)?;
        let curve = cf.to_nurbs(full);
        let extra = json!({"center": cf.center.to_array(), "normal": cf.normal.to_array(), "radius": cf.radius, "sweep_rad": cf.sweep});
        Some((if full { "circle" } else { "arc" }.to_string(), curve, cf.max_dev, cf.rms, extra))
    };
    match ctype {
        "line" => line(pts),
        "circle" => circle(pts, true).unwrap_or_else(|| spline(pts, closed, if closed { "closed_spline" } else { "spline" })),
        "arc" => circle(pts, false).unwrap_or_else(|| spline(pts, closed, "spline")),
        "spline" => spline(pts, false, "spline"),
        "closed_spline" => spline(pts, true, "closed_spline"),
        "ellipse" => {
            if !*ellipse_warned {
                crate::util::placeholder_note(report, "ellipse fitting is not implemented; a closed spline is fitted instead");
                *ellipse_warned = true;
            }
            spline(pts, true, "closed_spline")
        }
        _ => {
            // auto: simplest type within the deviation cap
            if !closed {
                let l = line(pts);
                if l.2 <= cap {
                    return l;
                }
                if let Some(c) = circle(pts, false) {
                    if c.2 <= cap && pts.len() >= 4 {
                        return c;
                    }
                }
                spline(pts, false, "spline")
            } else {
                if let Some(c) = circle(pts, true) {
                    if c.2 <= cap && pts.len() >= 5 {
                        return c;
                    }
                }
                spline(pts, true, "closed_spline")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::Rng;

    fn circle_pts(r: f64, n: usize, a0: f64, a1: f64, noise: f64, seed: u64) -> Vec<DVec3> {
        let mut rng = Rng::new(seed);
        (0..n)
            .map(|i| {
                let a = a0 + (a1 - a0) * i as f64 / (n - 1) as f64;
                DVec3::new(r * a.cos() + noise * rng.gauss(), r * a.sin() + noise * rng.gauss(), noise * rng.gauss())
            })
            .collect()
    }

    #[test]
    fn arc_recovers_radius() {
        // golden: 90 degree arc, r = 30, sigma 0.02
        let pts = circle_pts(30.0, 200, 0.0, std::f64::consts::FRAC_PI_2, 0.02, 11);
        let cf = fit_circle(&pts).unwrap();
        assert!((cf.radius - 30.0).abs() < 0.02, "r={}", cf.radius);
        assert!(cf.max_dev < 0.1);
        assert!((cf.sweep - std::f64::consts::FRAC_PI_2).abs() < 0.01);
        let arc = cf.to_nurbs(false);
        arc.validate().unwrap();
        assert!((arc.point(0.0) - pts[0]).length() < 0.1);
    }

    #[test]
    fn clockwise_arc_and_full_circle() {
        let pts = circle_pts(20.0, 90, 0.0, -std::f64::consts::PI, 0.0, 1);
        let cf = fit_circle(&pts).unwrap();
        assert!(cf.sweep < 0.0);
        let arc = cf.to_nurbs(false);
        for p in &pts {
            assert!(arc.project(*p).2 < 1e-6);
        }
        let ring = circle_pts(20.0, 100, 0.0, std::f64::consts::TAU * 0.99, 0.0, 1);
        let cf = fit_circle(&ring).unwrap();
        let full = cf.to_nurbs(true);
        assert!((full.point(0.37) - cf.center).length() > 19.99);
    }

    #[test]
    fn line_fit_is_exact_on_collinear_points() {
        let pts: Vec<DVec3> = (0..10).map(|i| DVec3::new(i as f64, 2.0 * i as f64, 1.0)).collect();
        let (c, d) = fit_line(&pts);
        assert!(d < 1e-12);
        assert!((c.point(1.0) - pts[9]).length() < 1e-12);
    }

    #[test]
    fn spline_control_points_are_monotone_in_tolerance() {
        let pts: Vec<DVec3> = (0..=300).map(|i| {
            let x = i as f64 * 0.5;
            DVec3::new(x, 8.0 * (x * 0.12).sin() + 2.0 * (x * 0.5).sin(), 0.0)
        }).collect();
        let mut prev = 0usize;
        for tol in [0.2, 0.05, 0.01] {
            let fit = fit_adaptive(&pts, 3, tol, 256, false, None, None).unwrap();
            assert!(fit.max_dev <= tol * 1.05 + 1e-9, "tol {tol}: dev {}", fit.max_dev);
            assert!(fit.curve.control_points.len() >= prev, "monotone");
            prev = fit.curve.control_points.len();
            fit.curve.validate().unwrap();
        }
        assert!(prev > 10);
    }

    #[test]
    fn tangent_matching_aligns_start_direction() {
        let pts: Vec<DVec3> = (0..=60).map(|i| DVec3::new(i as f64, (i as f64 * 0.1).sin() * 4.0, 0.0)).collect();
        let t = DVec3::new(1.0, 0.0, 0.0);
        let fit = fit_adaptive(&pts, 3, 0.5, 64, false, Some(t), None).unwrap();
        let d = fit.curve.derivative(0.0).normalize();
        assert!(d.dot(t) > 0.999999, "{d:?}");
    }

    #[test]
    fn pick_arc_returns_short_or_long() {
        let ring = circle_pts(10.0, 40, 0.0, std::f64::consts::TAU * 39.0 / 40.0, 0.0, 1);
        let a = ring[0];
        let b = ring[10];
        let s = pick_arc(&ring, a, b, true);
        let l = pick_arc(&ring, a, b, false);
        assert_eq!(s.len(), 11);
        assert_eq!(l.len(), 31);
    }
}

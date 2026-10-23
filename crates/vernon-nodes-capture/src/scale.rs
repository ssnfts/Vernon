//! Scale correction and point-based alignment: known-distance scaling,
//! scale-bar solving and similarity alignment from marker pairs.

use std::collections::HashMap;

use glam::DVec3;
use serde_json::{json, Value};
use vernon_core::{Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{
    CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec,
};

use crate::linalg::{fit_similarity, Sim3};
use crate::tables::{bars_from_json, parse_bar_list, BarSpec};
use crate::util::{self, base, set_metric_flag};

// ---------------------------------------------------------------------------
// Scale bars
// ---------------------------------------------------------------------------

/// How bars are weighted in the scale fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weighting {
    InverseVariance,
    Equal,
}

/// One measured bar: distance in the scan, true length and its uncertainty.
#[derive(Debug, Clone, Copy)]
pub struct Bar {
    /// Distance between the two targets as measured in the scan, mm.
    pub measured: f64,
    /// True length after thermal correction, mm.
    pub length: f64,
    /// Standard uncertainty of the true length, mm.
    pub sigma: f64,
}

/// Length of a bar at the reference temperature, corrected for expansion:
/// `L = L0 * (1 + alpha * (T - T_ref))`.
pub fn thermal_length(l0: f64, alpha: f64, temp_c: f64, ref_temp_c: f64) -> f64 {
    l0 * (1.0 + alpha * (temp_c - ref_temp_c))
}

/// Weighted least-squares scale and its standard uncertainty over `idx`.
/// Minimises `sum w (s m - L)^2`; `u(s)` propagates the length uncertainties.
pub fn fit_scale(bars: &[Bar], idx: &[usize], weighting: Weighting) -> (f64, f64) {
    let w = |b: &Bar| match weighting {
        Weighting::InverseVariance => 1.0 / (b.sigma * b.sigma),
        Weighting::Equal => 1.0,
    };
    let mut num = 0.0;
    let mut den = 0.0;
    for &i in idx {
        let b = &bars[i];
        num += w(b) * b.measured * b.length;
        den += w(b) * b.measured * b.measured;
    }
    let mut var = 0.0;
    for &i in idx {
        let b = &bars[i];
        let c = w(b) * b.measured * b.sigma / den;
        var += c * c;
    }
    (num / den, var.sqrt())
}

/// Result of [`solve_scale`].
#[derive(Debug, Clone)]
pub struct ScaleSolution {
    pub scale: f64,
    /// Standard uncertainty of the scale (dimensionless).
    pub uncertainty: f64,
    pub used: Vec<bool>,
    /// `s m - L` for every bar with the final scale, mm.
    pub residual_mm: Vec<f64>,
    /// Residual relative to the bar length, ppm.
    pub residual_ppm: Vec<f64>,
    /// Max minus min of the per-bar scale `L / m` over the used bars, ppm.
    pub span_ppm: f64,
    pub notes: Vec<(Severity, String)>,
}

/// Solve the scale from bars with leave-one-out outlier rejection.
///
/// A bar is rejected when the scale from the *other* bars predicts its length
/// worse than `reject_tol_mm`; the worst offender (by relative error) goes
/// first and the test repeats. Rejection needs at least three active bars so
/// that the remaining ones can out-vote the suspect, and never goes below
/// `max(min_bars, 2)` bars.
pub fn solve_scale(
    bars: &[Bar],
    weighting: Weighting,
    reject_tol_mm: f64,
    min_bars: usize,
) -> Result<ScaleSolution, String> {
    if bars.is_empty() {
        return Err("no scale bars given".into());
    }
    for (i, b) in bars.iter().enumerate() {
        if !(b.measured.is_finite() && b.measured > 1e-9) {
            return Err(format!("bar {i}: measured length {} is not positive", b.measured));
        }
        if !(b.length.is_finite() && b.length > 0.0) {
            return Err(format!("bar {i}: true length {} is not positive", b.length));
        }
        if !(b.sigma.is_finite() && b.sigma > 0.0) {
            return Err(format!("bar {i}: sigma {} must be positive", b.sigma));
        }
    }
    let mut used = vec![true; bars.len()];
    let mut notes = Vec::new();
    loop {
        let active: Vec<usize> = (0..bars.len()).filter(|&i| used[i]).collect();
        if active.len() < 3 || active.len() - 1 < min_bars.max(2) {
            break;
        }
        let mut worst: Option<(usize, f64)> = None;
        for &i in &active {
            let others: Vec<usize> = active.iter().copied().filter(|&j| j != i).collect();
            let (s, _) = fit_scale(bars, &others, weighting);
            let r = s * bars[i].measured - bars[i].length;
            if r.abs() > reject_tol_mm {
                let rel = (r / bars[i].length).abs();
                if worst.is_none_or(|(_, w)| rel > w) {
                    worst = Some((i, rel));
                }
            }
        }
        match worst {
            Some((i, rel)) => {
                used[i] = false;
                notes.push((
                    Severity::Warning,
                    format!("bar {i} rejected: the other bars predict its length {:.0} ppm away", rel * 1e6),
                ));
            }
            None => break,
        }
    }
    let active: Vec<usize> = (0..bars.len()).filter(|&i| used[i]).collect();
    let (scale, uncertainty) = fit_scale(bars, &active, weighting);
    if !(scale.is_finite() && scale > 0.0) {
        return Err("fitted scale is not positive".into());
    }
    let residual_mm: Vec<f64> = bars.iter().map(|b| scale * b.measured - b.length).collect();
    let residual_ppm: Vec<f64> = residual_mm
        .iter()
        .zip(bars)
        .map(|(r, b)| r / b.length * 1e6)
        .collect();
    let per_bar: Vec<f64> = active.iter().map(|&i| bars[i].length / bars[i].measured).collect();
    let span_ppm = (per_bar.iter().copied().fold(f64::MIN, f64::max)
        - per_bar.iter().copied().fold(f64::MAX, f64::min))
        * 1e6;
    if active.len() < 3 {
        notes.push((
            Severity::Warning,
            format!("only {} usable bar(s); three or more allow a bad bar to be detected", active.len()),
        ));
    }
    if active.len() < min_bars {
        notes.push((
            Severity::Error,
            format!("{} usable bar(s), but at least {min_bars} are required", active.len()),
        ));
    }
    if active.len() == 2 && span_ppm > 1000.0 * reject_tol_mm.max(0.01) {
        notes.push((
            Severity::Warning,
            format!("the two bars disagree by {span_ppm:.0} ppm and there is no third bar to tell which is wrong"),
        ));
    }
    Ok(ScaleSolution {
        scale,
        uncertainty,
        used,
        residual_mm,
        residual_ppm,
        span_ppm,
        notes,
    })
}

// ---------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------

/// Uniform scale from one known distance.
pub struct ScaleFromDistance;

impl NodeOp for ScaleFromDistance {
    fn info(&self) -> NodeTypeInfo {
        base("capture.scale_from_distance", "Scale From Distance")
            .input(PortSpec::new("geometry", DataKind::Geometry))
            .input(PortSpec::optional("picks", DataKind::Geometry))
            .output(PortSpec::new("geometry", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::vec3("p0", "Point A", DVec3::ZERO).unit("mm").help("First reference point, used when no picks cloud is connected."))
            .param(ParamSpec::vec3("p1", "Point B", DVec3::new(1000.0, 0.0, 0.0)).unit("mm").help("Second reference point."))
            .param(ParamSpec::int("index_a", "Pick index A", 0, 0, 1_000_000).help("Index of the first point in the picks cloud."))
            .param(ParamSpec::int("index_b", "Pick index B", 1, 0, 1_000_000).help("Index of the second point in the picks cloud."))
            .param(ParamSpec::float("true_length", "True length", 1000.0, 1e-6, 1e6).unit("mm").help("Real distance between the two points."))
            .param(ParamSpec::float("uncertainty", "Length uncertainty", 0.5, 0.0, 1e4).unit("mm").help("Standard uncertainty of the true length."))
            .param(ParamSpec::choice("pivot", "Pivot", "origin", &["origin", "centroid", "point_a"]).help("Point the scaling is applied about."))
            .help("Scales a geometry uniformly so that the distance between two picked points equals a known length, and marks it metric. Reports the scale uncertainty carried to the whole part.")
            .emulates("rescale tool in phone scanning apps: pick two points, enter the real distance")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let src = inputs::geometry(inputs, 0)?;
        let (a, b) = match inputs::opt_geometry(inputs, 1) {
            Some(picks) => {
                let ia = params.int("index_a")?.max(0) as usize;
                let ib = params.int("index_b")?.max(0) as usize;
                match (picks.points.get(ia), picks.points.get(ib)) {
                    (Some(a), Some(b)) => (*a, *b),
                    _ => {
                        return Err(ctx.fail(format!(
                            "picks cloud has {} points; indices {ia} and {ib} are out of range",
                            picks.points.len()
                        )))
                    }
                }
            }
            None => (params.vec3("p0")?, params.vec3("p1")?),
        };
        let true_length = params.float("true_length")?;
        util::require_positive(ctx, "true_length", true_length)?;
        let sigma = params.float("uncertainty")?;
        util::require_non_negative(ctx, "uncertainty", sigma)?;
        let measured = (b - a).length();
        if measured < 1e-9 {
            return Err(ctx.fail("the two reference points coincide; pick two distinct points"));
        }
        let s = true_length / measured;
        let pivot = match params.str("pivot")? {
            "origin" => DVec3::ZERO,
            "centroid" => util::centroid(&src.points),
            "point_a" => a,
            other => return Err(util::bad_choice(ctx, "pivot", other)),
        };
        let mut geo = (*src).clone();
        util::scale_about(&mut geo, s, pivot);
        set_metric_flag(&mut geo, true);
        let u_ppm = sigma / true_length * 1e6;
        let mut rep = Report::new("Scale From Distance");
        rep.metric("scale_factor", Metric::new(s, "-"));
        rep.metric("scale_ppm", Metric::new((s - 1.0) * 1e6, "ppm"));
        rep.metric("measured_length", Metric::new(measured, "mm"));
        rep.metric("true_length", Metric::new(true_length, "mm"));
        rep.metric("scale_uncertainty_ppm", Metric::new(u_ppm, "ppm"));
        rep.metric(
            "span_effect",
            Metric::new(util::longest_edge(&geo) * u_ppm * 1e-6, "mm"),
        );
        if (s - 1.0).abs() > 0.5 {
            rep.note(
                Severity::Warning,
                format!("scale factor {s:.4} is far from 1; check the picked points and the units"),
            );
        }
        if u_ppm > 100.0 {
            rep.note(
                Severity::Warning,
                format!("a single distance with {u_ppm:.0} ppm uncertainty limits the whole part; use calibrated bars for fitment work"),
            );
        }
        Ok(vec![Data::geometry(geo), Data::report(rep)])
    }
}

/// Solve scale from several bars with thermal correction and outlier rejection.
pub struct ScaleBarSolve;

impl ScaleBarSolve {
    fn read_bars(ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<BarSpec>> {
        let specs = match util::json_input(inputs, 2) {
            Some(v) => bars_from_json(&v).map_err(|m| ctx.fail(format!("bars input: {m}")))?,
            None => parse_bar_list(params.str("bars")?).map_err(|m| ctx.fail(format!("bars parameter: {m}")))?,
        };
        if specs.is_empty() {
            return Err(ctx.fail("no scale bars given: connect a bars table or fill the 'bars' parameter"));
        }
        Ok(specs)
    }
}

impl NodeOp for ScaleBarSolve {
    fn info(&self) -> NodeTypeInfo {
        base("capture.scale_bar_solve", "Solve Scale Bars")
            .input(PortSpec::new("markers", DataKind::Geometry))
            .input(PortSpec::optional("scan", DataKind::Geometry))
            .input(PortSpec::optional("bars", DataKind::Json))
            .output(PortSpec::new("geometry", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("table", DataKind::Json))
            .param(ParamSpec::string("bars", "Bars", "").help("Rows 'id_a, id_b, length [, sigma, alpha, temp_c]', separated by semicolons or new lines; ignored when a bars table is connected."))
            .param(ParamSpec::float("default_sigma", "Default sigma", 0.02, 1e-6, 100.0).unit("mm").help("Length uncertainty for rows without one."))
            .param(ParamSpec::float("ref_temp", "Reference temperature", 20.0, -50.0, 100.0).unit("°C").help("Temperature at which the scale bar length is certified. Readings at another temperature are corrected for thermal expansion."))
            .param(ParamSpec::float("reject_tol", "Reject tolerance", 0.05, 0.001, 5.0).unit("mm").help("A bar is dropped when the other bars predict its length worse than this."))
            .param(ParamSpec::int("min_bars", "Minimum bars", 1, 1, 100).help("Fewest usable scale bars required. With fewer the result is reported as an error."))
            .param(ParamSpec::choice("weighting", "Weighting", "inv_var", &["inv_var", "equal"]).help("How the bars are combined: inverse-variance gives better-measured bars more weight, equal treats them alike."))
            .param(ParamSpec::float("reference_length", "Reference span", 4500.0, 1.0, 1e5).unit("mm").help("Span for which the scale uncertainty is converted to millimetres."))
            .help("Fits one uniform scale to several scale bars (weighted least squares, thermal correction, leave-one-out rejection of bad bars), applies it to the scan, and reports the scale, per-bar residuals and the uncertainty over a car-length span.")
            .emulates("scale-bar adjustment used in close-range photogrammetry metrology")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let markers = inputs::geometry(inputs, 0)?;
        let target = inputs::opt_geometry(inputs, 1);
        let mut text_params = params.clone();
        if let Ok(t) = params.str("bars") {
            text_params.set("bars", vernon_graph::ParamValue::Str(t.replace(';', "\n")));
        }
        let specs = Self::read_bars(ctx, inputs, &text_params)?;
        let ref_temp = params.float("ref_temp")?;
        let default_sigma = params.float("default_sigma")?;
        util::require_positive(ctx, "default_sigma", default_sigma)?;
        let tol = params.float("reject_tol")?;
        util::require_positive(ctx, "reject_tol", tol)?;
        let weighting = match params.str("weighting")? {
            "inv_var" => Weighting::InverseVariance,
            "equal" => Weighting::Equal,
            other => return Err(util::bad_choice(ctx, "weighting", other)),
        };
        let min_bars = params.int("min_bars")?.max(1) as usize;

        let ids = util::point_ids(&markers);
        let mut by_id: HashMap<&str, usize> = HashMap::new();
        for (i, id) in ids.iter().enumerate() {
            if by_id.insert(id.as_str(), i).is_some() {
                return Err(ctx.fail(format!("marker id '{id}' appears twice in the marker cloud")));
            }
        }
        let mut bars = Vec::with_capacity(specs.len());
        for s in &specs {
            let find = |id: &str| {
                by_id
                    .get(id)
                    .copied()
                    .ok_or_else(|| ctx.fail(format!("bar refers to marker '{id}', which is not in the marker cloud")))
            };
            let (ia, ib) = (find(&s.a)?, find(&s.b)?);
            let l = thermal_length(
                s.length,
                s.alpha.unwrap_or(0.0),
                s.temp_c.unwrap_or(ref_temp),
                ref_temp,
            );
            bars.push(Bar {
                measured: (markers.points[ia] - markers.points[ib]).length(),
                length: l,
                sigma: s.sigma.unwrap_or(default_sigma),
            });
        }
        let sol = solve_scale(&bars, weighting, tol, min_bars).map_err(|m| ctx.fail(m))?;

        let mut geo = (*target.unwrap_or_else(|| markers.clone())).clone();
        util::scale_about(&mut geo, sol.scale, DVec3::ZERO);
        set_metric_flag(&mut geo, true);

        let mut rep = Report::new("Solve Scale Bars");
        let used = sol.used.iter().filter(|u| **u).count();
        let rel_u = sol.uncertainty / sol.scale;
        let span = params.float("reference_length")?;
        rep.metric("scale_factor", Metric::new(sol.scale, "-"));
        rep.metric("scale_ppm", Metric::new((sol.scale - 1.0) * 1e6, "ppm"));
        rep.metric("scale_uncertainty_ppm", Metric::new(rel_u * 1e6, "ppm"));
        rep.metric("span_effect", Metric::new(span * rel_u, "mm"));
        rep.metric("bar_span_ppm", Metric::new(sol.span_ppm, "ppm"));
        rep.metric("bars_used", Metric::new(used as f64, "bars"));
        rep.metric("bars_rejected", Metric::new((bars.len() - used) as f64, "bars"));
        for (sev, n) in &sol.notes {
            rep.note(*sev, n.clone());
        }
        let table: Vec<Value> = specs
            .iter()
            .zip(&bars)
            .enumerate()
            .map(|(i, (s, b))| {
                json!({
                    "a": s.a, "b": s.b,
                    "measured_mm": b.measured,
                    "true_length_mm": b.length,
                    "sigma_mm": b.sigma,
                    "residual_mm": sol.residual_mm[i],
                    "residual_ppm": sol.residual_ppm[i],
                    "used": sol.used[i],
                })
            })
            .collect();
        Ok(vec![Data::geometry(geo), Data::report(rep), Data::json(Value::Array(table))])
    }
}

// ---------------------------------------------------------------------------
// Similarity alignment
// ---------------------------------------------------------------------------

/// Result of a (robust) similarity fit.
#[derive(Debug, Clone)]
pub struct AlignFit {
    pub sim: Sim3,
    pub inliers: Vec<bool>,
    pub rms: f64,
    pub max_residual: f64,
}

fn residuals(sim: &Sim3, src: &[DVec3], dst: &[DVec3]) -> Vec<f64> {
    src.iter().zip(dst).map(|(s, d)| (sim.apply(*s) - *d).length()).collect()
}

/// Plain least squares over all pairs, or RANSAC over 3-point subsets with
/// an inlier gate and a refit on the inliers.
pub fn align_points(
    src: &[DVec3],
    dst: &[DVec3],
    with_scale: bool,
    ransac: Option<(f64, usize, u64)>,
) -> Result<AlignFit, String> {
    let finish = |sim: Sim3, inliers: Vec<bool>| {
        let r = residuals(&sim, src, dst);
        let used: Vec<f64> = r.iter().zip(&inliers).filter(|(_, i)| **i).map(|(v, _)| *v).collect();
        let rms = (used.iter().map(|v| v * v).sum::<f64>() / used.len().max(1) as f64).sqrt();
        let max_residual = used.iter().copied().fold(0.0, f64::max);
        AlignFit {
            sim,
            inliers,
            rms,
            max_residual,
        }
    };
    let Some((gate, iterations, seed)) = ransac else {
        let sim = fit_similarity(src, dst, with_scale)?;
        return Ok(finish(sim, vec![true; src.len()]));
    };
    let n = src.len();
    if n < 3 {
        return Err("need at least 3 point pairs".into());
    }
    let mut rng = util::Rng::new(seed);
    let mut best: Option<(usize, f64, Vec<bool>)> = None;
    for _ in 0..iterations.max(1) {
        let (a, b, c) = (rng.below(n), rng.below(n), rng.below(n));
        if a == b || b == c || a == c {
            continue;
        }
        let s3 = [src[a], src[b], src[c]];
        let d3 = [dst[a], dst[b], dst[c]];
        let Ok(sim) = fit_similarity(&s3, &d3, with_scale) else {
            continue;
        };
        let r = residuals(&sim, src, dst);
        let flags: Vec<bool> = r.iter().map(|v| *v <= gate).collect();
        let count = flags.iter().filter(|f| **f).count();
        let sum: f64 = r.iter().zip(&flags).filter(|(_, f)| **f).map(|(v, _)| v * v).sum();
        if best.as_ref().is_none_or(|(c0, s0, _)| count > *c0 || (count == *c0 && sum < *s0)) {
            best = Some((count, sum, flags));
        }
        if count == n {
            break;
        }
    }
    let (count, _, flags) = best.ok_or("RANSAC found no valid 3-point sample (points may be collinear)")?;
    if count < 3 {
        return Err(format!("RANSAC found only {count} points within {gate} mm; raise the inlier gate"));
    }
    let s_in: Vec<DVec3> = src.iter().zip(&flags).filter(|(_, f)| **f).map(|(p, _)| *p).collect();
    let d_in: Vec<DVec3> = dst.iter().zip(&flags).filter(|(_, f)| **f).map(|(p, _)| *p).collect();
    let sim = fit_similarity(&s_in, &d_in, with_scale)?;
    let r = residuals(&sim, src, dst);
    let flags: Vec<bool> = r.iter().map(|v| *v <= gate).collect();
    Ok(finish(sim, flags))
}

/// Two marker lists paired one to one, with the shared names.
pub type PairedPoints = (Vec<DVec3>, Vec<DVec3>, Vec<String>);

/// Pair two marker clouds by `id` (when both have ids) or by index.
pub fn pair_points(a: &Geometry, b: &Geometry) -> Result<PairedPoints, String> {
    if util::has_ids(a) && util::has_ids(b) {
        let ia = util::point_ids(a);
        let ib = util::point_ids(b);
        let map: HashMap<&str, usize> = ib.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect();
        let mut sp = Vec::new();
        let mut dp = Vec::new();
        let mut names = Vec::new();
        for (i, id) in ia.iter().enumerate() {
            if let Some(&j) = map.get(id.as_str()) {
                sp.push(a.points[i]);
                dp.push(b.points[j]);
                names.push(id.clone());
            }
        }
        if sp.is_empty() {
            return Err("the two marker clouds share no ids".into());
        }
        Ok((sp, dp, names))
    } else {
        let n = a.points.len().min(b.points.len());
        Ok((
            a.points[..n].to_vec(),
            b.points[..n].to_vec(),
            (0..n).map(|i| i.to_string()).collect(),
        ))
    }
}

/// Rigid or similarity alignment from corresponding marker points.
pub struct SimilarityAlign;

impl NodeOp for SimilarityAlign {
    fn info(&self) -> NodeTypeInfo {
        base("capture.similarity_align", "Align By Points")
            .input(PortSpec::new("source", DataKind::Geometry))
            .input(PortSpec::new("target", DataKind::Geometry))
            .input(PortSpec::optional("scan", DataKind::Geometry))
            .output(PortSpec::new("geometry", DataKind::Geometry))
            .output(PortSpec::new("transform", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::bool("allow_scale", "Allow scale", false).help("Fit a uniform scale as well as rotation and translation."))
            .param(ParamSpec::bool("ransac", "Robust (RANSAC)", false).help("Fit on random 3-point subsets so that gross outliers are ignored."))
            .param(ParamSpec::float("inlier", "Inlier distance", 1.0, 0.01, 50.0).unit("mm").help("Point pairs farther apart than this after the fit are treated as outliers."))
            .param(ParamSpec::int("iterations", "Iterations", 1000, 1, 1_000_000).help("Number of random pair subsets tried when outlier rejection is on."))
            .param(ParamSpec::int("seed", "Random seed", 1, 0, 1_000_000).help("Random seed. The same seed gives the same result."))
            .help("Finds the rotation, translation and optional scale that bring corresponding source markers onto target markers (ids are matched when both clouds have them, otherwise by index) and applies it to the scan. Needs three or more non-collinear markers.")
            .emulates("marker-based scan alignment and known-point registration")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let source = inputs::geometry(inputs, 0)?;
        let target = inputs::geometry(inputs, 1)?;
        let scan = inputs::opt_geometry(inputs, 2);
        let (sp, dp, names) = pair_points(&source, &target).map_err(|m| ctx.fail(m))?;
        let gate = params.float("inlier")?;
        util::require_positive(ctx, "inlier", gate)?;
        let ransac = params
            .bool("ransac")?
            .then(|| (gate, params.int("iterations").unwrap_or(1000).max(1) as usize, params.int("seed").unwrap_or(1).max(0) as u64));
        let with_scale = params.bool("allow_scale")?;
        let fit = align_points(&sp, &dp, with_scale, ransac).map_err(|m| ctx.fail(m))?;
        let m = fit.sim.to_mat4();
        let mut geo = (*scan.unwrap_or_else(|| source.clone())).clone();
        geo.transform(m);
        if let Some(t) = util::metric_flag(&target) {
            set_metric_flag(&mut geo, t);
        }
        let outliers: Vec<&String> = names.iter().zip(&fit.inliers).filter(|(_, i)| !**i).map(|(n, _)| n).collect();
        let mut rep = Report::new("Align By Points");
        rep.metric("pairs", Metric::new(sp.len() as f64, "pairs"));
        rep.metric("rms", Metric::new(fit.rms, "mm"));
        rep.metric("max_residual", Metric::new(fit.max_residual, "mm"));
        rep.metric("outliers", Metric::new(outliers.len() as f64, "pairs"));
        rep.metric("rotation_angle", Metric::new(fit.sim.angle_deg(), "deg"));
        rep.metric("translation", Metric::new(fit.sim.trans.length(), "mm"));
        if with_scale {
            rep.metric("scale_factor", Metric::new(fit.sim.scale, "-"));
        }
        if !outliers.is_empty() {
            rep.note(
                Severity::Warning,
                format!(
                    "{} marker pair(s) excluded as outliers: {}",
                    outliers.len(),
                    outliers.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                ),
            );
        }
        if sp.len() == 3 {
            rep.note(Severity::Info, "exactly three pairs: the fit has no redundancy to check marker errors");
        }
        let tj = json!({
            "transform": util::mat_to_json(&m),
            "scale": fit.sim.scale,
            "rms_mm": fit.rms,
        });
        Ok(vec![Data::geometry(geo), Data::json(tj), Data::report(rep)])
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DQuat;

    fn bar(m: f64, l: f64) -> Bar {
        Bar {
            measured: m,
            length: l,
            sigma: 0.02,
        }
    }

    #[test]
    fn golden_7_single_bar() {
        let sol = solve_scale(&[bar(1000.5, 1000.0)], Weighting::Equal, 0.05, 1).unwrap();
        assert!((sol.scale - 0.999_500_25).abs() < 1e-7);
        // box 4500 mm stretched by 1.0005 returns to 4500
        assert!((4500.0 * 1.0005 * sol.scale - 4500.0).abs() < 1e-3);
    }

    #[test]
    fn golden_8_three_bars() {
        let bars = [bar(500.25, 500.0), bar(1000.5, 1000.0), bar(2001.0, 2000.0)];
        let sol = solve_scale(&bars, Weighting::Equal, 0.05, 1).unwrap();
        assert!((sol.scale - 5_252_625.0 / 5_255_251.312_5).abs() < 1e-12);
        assert!((sol.scale - 0.999_500_25).abs() < 1e-7);
        assert!(sol.residual_mm.iter().all(|r| r.abs() < 1e-3));
        assert!(sol.used.iter().all(|u| *u));
    }

    #[test]
    fn golden_9_bad_bar_rejected() {
        let good = [bar(500.25, 500.0), bar(1000.5, 1000.0), bar(2001.0, 2000.0)];
        let mut all = good.to_vec();
        all.push(bar(1002.0, 1000.0));
        let sol = solve_scale(&all, Weighting::Equal, 0.05, 1).unwrap();
        let reference = solve_scale(&good, Weighting::Equal, 0.05, 1).unwrap();
        assert_eq!(sol.used, [true, true, true, false]);
        assert!((sol.scale - reference.scale).abs() < 1e-6);
        assert!(sol.residual_mm[3].abs() > 1.0);
    }

    #[test]
    fn golden_10_thermal_length() {
        assert!((thermal_length(1000.0, 23e-6, 30.0, 20.0) - 1000.230).abs() < 1e-9);
        assert!((thermal_length(1000.0, 1.2e-6, 30.0, 20.0) - 1000.012).abs() < 1e-9);
    }

    #[test]
    fn weighting_prefers_the_tighter_bar() {
        let bars = [
            Bar { measured: 1000.0, length: 1000.0, sigma: 0.01 },
            Bar { measured: 1000.0, length: 1000.4, sigma: 1.0 },
        ];
        let (s_w, _) = fit_scale(&bars, &[0, 1], Weighting::InverseVariance);
        let (s_e, _) = fit_scale(&bars, &[0, 1], Weighting::Equal);
        assert!((s_w - 1.0).abs() < 1e-4);
        assert!((s_e - 1.0002).abs() < 1e-6);
        let (_, u) = fit_scale(&bars, &[0, 1], Weighting::InverseVariance);
        assert!((u - 1.0 / (1000.0_f64.powi(2) * (1.0 / 0.01_f64.powi(2) + 1.0)).sqrt()).abs() < 1e-12);
    }

    #[test]
    fn bad_bar_input_is_an_error() {
        assert!(solve_scale(&[], Weighting::Equal, 0.05, 1).is_err());
        assert!(solve_scale(&[bar(0.0, 10.0)], Weighting::Equal, 0.05, 1).is_err());
        assert!(solve_scale(&[bar(10.0, -1.0)], Weighting::Equal, 0.05, 1).is_err());
    }

    fn markers() -> Vec<DVec3> {
        vec![
            DVec3::new(0.1, 0.2, 0.0),
            DVec3::new(2.0, 0.5, 0.3),
            DVec3::new(1.0, 3.0, 0.1),
            DVec3::new(4.0, 1.0, 0.9),
            DVec3::new(3.0, 3.5, 1.2),
            DVec3::new(0.5, 2.0, 0.7),
        ]
    }

    #[test]
    fn golden_11_umeyama_and_ransac() {
        let truth = Sim3 {
            scale: 1000.0,
            rot: DQuat::from_rotation_z(30f64.to_radians()),
            trans: DVec3::new(120.0, -40.0, 15.0),
        };
        let src = markers();
        let mut dst: Vec<DVec3> = src.iter().map(|p| truth.apply(*p)).collect();
        let clean = align_points(&src, &dst, true, None).unwrap();
        assert!(clean.rms < 1e-9);
        assert!((clean.sim.scale - 1000.0).abs() < 1e-9);
        assert!((clean.sim.trans - truth.trans).length() < 1e-9);
        // one gross outlier, 500 mm off
        dst[2] += DVec3::new(500.0, 0.0, 0.0);
        let plain = align_points(&src, &dst, true, None).unwrap();
        assert!(plain.rms > 50.0, "plain LS rms {}", plain.rms);
        let robust = align_points(&src, &dst, true, Some((1.0, 1000, 5))).unwrap();
        assert_eq!(robust.inliers.iter().filter(|i| !**i).count(), 1);
        assert!(!robust.inliers[2]);
        assert!((robust.sim.scale - 1000.0).abs() < 1e-6);
        assert!((robust.sim.trans - truth.trans).length() < 1e-6);
        assert!(robust.rms < 1e-6);
    }
}

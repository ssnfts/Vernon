//! Deviation reporting: statistics with percentile gating, histogram with
//! CSV export, colour map and a tolerance gate. These nodes turn the `dist`
//! point attribute into the numbers the next stage checks against.

use std::fmt::Write as _;

use glam::DVec3;
use serde_json::{json, Value};
use vernon_core::{AttribArray, Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::stats::{self, bands, fraction_within, histogram, moments, nearest_rank, range_t, ramp, weighted_fraction_within, weighted_rank, Scale};
use crate::util::{add, json_in, parse_list, pf, pi, SPEC_ALIGN};

pub const SCHEMA: &str = "deviation_report.v1";

pub fn register(reg: &mut Registry) {
    add(reg, stats_info, stats_cook);
    add(reg, hist_info, hist_cook);
    add(reg, colors_info, colors_cook);
    add(reg, gate_info, gate_cook);
}

fn geo_in() -> PortSpec {
    PortSpec::new("geometry", DataKind::Geometry)
}

// ------------------------------------------------------------------ input

/// Valid deviation samples read from a geometry.
struct Dev {
    vals: Vec<f64>,
    weights: Option<Vec<f64>>,
    n_invalid: usize,
}

fn vertex_areas(g: &Geometry) -> Option<Vec<f64>> {
    if g.prims.is_empty() {
        return None;
    }
    let mut a = vec![0.0; g.points.len()];
    for (_, t) in g.triangles() {
        let (p, q, r) = (g.points[t[0] as usize], g.points[t[1] as usize], g.points[t[2] as usize]);
        let area = (q - p).cross(r - p).length() * 0.5 / 3.0;
        for v in t {
            a[v as usize] += area;
        }
    }
    if a.iter().sum::<f64>() > 0.0 {
        Some(a)
    } else {
        None
    }
}

fn collect_dev(ctx: &CookContext, g: &Geometry, attrib: &str, by_area: bool) -> CookResult<Dev> {
    let d = g
        .point_attribs
        .floats(attrib)
        .map_err(|_| ctx.fail(format!("the input has no float point attribute '{attrib}'; run a distance node first")))?;
    let valid = g
        .point_attribs
        .ints("dist_valid")
        .ok()
        .filter(|v| v.len() == d.len() && attrib == "dist");
    let areas = if by_area { vertex_areas(g) } else { None };
    let (mut vals, mut w) = (Vec::with_capacity(d.len()), Vec::new());
    let mut invalid = 0;
    for (i, &x) in d.iter().enumerate() {
        let ok = x.is_finite() && valid.is_none_or(|v| v[i] == 1);
        if ok {
            vals.push(x);
            if let Some(a) = &areas {
                w.push(a[i]);
            }
        } else {
            invalid += 1;
        }
    }
    if vals.is_empty() {
        return Err(ctx.fail("the input has no valid deviation values"));
    }
    Ok(Dev {
        vals,
        weights: if areas.is_some() { Some(w) } else { None },
        n_invalid: invalid,
    })
}

// ------------------------------------------------------------------ stats

fn stats_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.deviation_stats", "Deviation Statistics", Stage::Clean)
        .input(geo_in())
        .output(PortSpec::new("report", DataKind::Report))
        .output(PortSpec::new("stats", DataKind::Json))
        .param(ParamSpec::string("attrib", "Attribute", "dist").help("Float point attribute holding the signed deviation in mm."))
        .param(ParamSpec::float("tolerance", "Tolerance", 0.5, 0.001, 100.0).unit("mm").help("Deviation band checked by the verdict."))
        .param(ParamSpec::float("target_fraction", "Target share", 0.95, 0.5, 1.0).help("Share of points that must be within the tolerance, e.g. 0.95 for '95 % within'."))
        .param(ParamSpec::choice("weighting", "Weighting", "point", &["point", "area"]).help("point: every sample counts equally. area: samples are weighted by the surface area around them (meshes)."))
        .param(ParamSpec::string("percentiles", "Percentiles", "50,90,95,99").help("Comma-separated percentiles of the absolute deviation (nearest rank)."))
        .param(ParamSpec::string("band_edges", "Band edges", "").unit("mm").help("Optional ascending list of band limits, e.g. '-0.5,-0.25,0,0.25,0.5'; counts points in each band."))
        .help("Mean, spread, percentiles, tolerance share and bands of the deviation. The report carries the numbers the next stage gates on (percentile against tolerance, share within tolerance); the stats output feeds the histogram and gate nodes.")
        .emulates("deviation statistics, percentile and tolerance-band reporting")
        .spec(SPEC_ALIGN)
        .implemented()
}

fn pname(q: f64) -> String {
    if (q - q.round()).abs() < 1e-9 {
        format!("p{}_abs", q.round() as i64)
    } else {
        format!("p{}_abs", format!("{q}").replace('.', "_"))
    }
}

fn parse_percentiles(ctx: &CookContext, s: &str) -> CookResult<Vec<f64>> {
    let mut v = parse_list(s);
    if v.is_empty() {
        v = vec![50.0, 90.0, 95.0, 99.0];
    }
    if v.iter().any(|q| !(*q > 0.0 && *q <= 100.0)) {
        return Err(ctx.fail("percentiles must be in the range (0, 100]"));
    }
    Ok(v)
}

fn stats_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    let attrib = p.str("attrib")?.to_string();
    let tol = pf(p, "tolerance", 0.001, 100.0)?;
    let target = pf(p, "target_fraction", 0.5, 1.0)?;
    let by_area = p.str("weighting")? == "area";
    let mut qs = parse_percentiles(ctx, p.str("percentiles")?)?;
    let tq = target * 100.0;
    if !qs.iter().any(|q| (q - tq).abs() < 1e-9) {
        qs.push(tq);
    }
    qs.sort_by(f64::total_cmp);
    let edges = parse_list(p.str("band_edges")?);
    if edges.windows(2).any(|w| w[1] <= w[0]) {
        return Err(ctx.fail("band edges must be strictly ascending"));
    }
    let dev = collect_dev(ctx, &g, &attrib, by_area)?;
    let weights = dev.weights.as_deref();
    let m = moments(&dev.vals, weights);
    let abs: Vec<f64> = dev.vals.iter().map(|v| v.abs()).collect();
    let weighted = weights.is_some();
    // sorted absolute values (with weights if any)
    let mut abs_sorted = abs.clone();
    abs_sorted.sort_by(f64::total_cmp);
    let pairs: Vec<(f64, f64)> = if let Some(w) = weights {
        let mut v: Vec<(f64, f64)> = abs.iter().cloned().zip(w.iter().cloned()).collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v
    } else {
        Vec::new()
    };
    let pct_of = |q: f64| -> f64 {
        if weighted {
            weighted_rank(&pairs, q / 100.0)
        } else {
            nearest_rank(&abs_sorted, q / 100.0)
        }
    };
    let within = |t: f64| -> f64 {
        match weights {
            Some(w) => weighted_fraction_within(&dev.vals, w, t),
            None => fraction_within(&abs_sorted, t),
        }
    };
    let frac = within(tol);
    let pass = frac >= target - 1e-12;

    let mut rep = Report::new("Deviation Statistics");
    rep.note(Severity::Info, vernon_core::stats::CONVENTION);
    rep.metric("valid_points", Metric::new(dev.vals.len() as f64, "points"));
    rep.metric("invalid_points", Metric::new(dev.n_invalid as f64, "points"));
    rep.metric("mean_dev", Metric::new(m.mean, "mm"));
    rep.metric("std_dev", Metric::new(m.std, "mm"));
    rep.metric("rms_dev", Metric::new(m.rms, "mm"));
    rep.metric("mean_abs_dev", Metric::new(m.mean_abs, "mm"));
    rep.metric("max_abs_dev", Metric::new(m.max_abs, "mm"));
    let mut perc_json = serde_json::Map::new();
    for &q in &qs {
        let v = pct_of(q);
        let mut met = Metric::new(v, "mm");
        if (q - tq).abs() < 1e-9 {
            met = met.with_limit(tol, true);
        }
        rep.metric(pname(q), met);
        perc_json.insert(pname(q).trim_end_matches("_abs").to_string(), json!(v));
    }
    rep.metric("fraction_within", Metric::new(100.0 * frac, "%").with_limit(100.0 * target, false));
    rep.metric("tolerance", Metric::new(tol, "mm"));

    let mut tols: Vec<f64> = vec![tol * 0.5, tol, tol * 2.0];
    tols.dedup();
    let within_json: Vec<Value> = tols.iter().map(|&t| json!({"tol": t, "fraction": within(t)})).collect();
    let (band_list, below, above) = bands(&dev.vals, &edges);
    let bands_json: Vec<Value> = band_list
        .iter()
        .map(|b| json!({"lo": b.lo, "hi": b.hi, "count": b.count, "percent": b.percent}))
        .collect();
    if !edges.is_empty() {
        rep.metric("below_bands", Metric::new(below as f64, "points"));
        rep.metric("above_bands", Metric::new(above as f64, "points"));
    }
    // dense quantile table for the gate node
    let mut grid: Vec<f64> = (1..=100).map(|i| i as f64).collect();
    grid.extend([99.5, 99.9, 99.99]);
    grid.sort_by(f64::total_cmp);
    let table: Vec<Value> = grid.iter().map(|&q| json!([q, pct_of(q)])).collect();

    let mut stats = json!({
        "schema": SCHEMA,
        "attrib": attrib,
        "weighting": if weighted { "area" } else { "point" },
        "n_valid": dev.vals.len(),
        "n_invalid": dev.n_invalid,
        "mean": m.mean, "std": m.std, "rms": m.rms, "min": m.min, "max": m.max,
        "mean_abs": m.mean_abs, "max_abs": m.max_abs,
        "abs_percentiles": Value::Object(perc_json),
        "abs_quantiles": table,
        "within": within_json,
        "bands": bands_json,
        "below": below,
        "above": above,
        "verdict": {"tolerance": tol, "target_fraction": target, "fraction_within": frac, "pass": pass},
    });
    if let Ok(b) = g.detail_attribs.floats("icp_rms_before") {
        let after = g.detail_attribs.floats("icp_rms_after").map(|a| a[0]).unwrap_or(f64::NAN);
        let iters = g.detail_attribs.ints("icp_iterations").map(|a| a[0]).unwrap_or(0);
        stats["alignment"] = json!({"rms_before": b[0], "rms_after": after, "iterations": iters});
        rep.metric("alignment_rms_after", Metric::new(after, "mm"));
    }
    if dev.n_invalid > 0 {
        rep.note(Severity::Info, format!("{} invalid sample(s) were excluded", dev.n_invalid));
    }
    if pass {
        rep.note(Severity::Info, format!("{:.1} % of points are within {tol} mm (target {:.1} %)", 100.0 * frac, 100.0 * target));
        if m.max_abs > 3.0 * tol {
            rep.note(Severity::Info, format!("the worst deviation is {:.3} mm, above {:.3} mm: a tail can hide under a good share", m.max_abs, 3.0 * tol));
        }
    } else {
        rep.note(
            Severity::Warning,
            format!("only {:.1} % of points are within {tol} mm (target {:.1} %)", 100.0 * frac, 100.0 * target),
        );
    }
    if m.mean.abs() > 0.5 * tol {
        rep.note(Severity::Info, format!("the mean deviation {:.3} mm is a systematic offset; check the alignment", m.mean));
    }
    Ok(vec![Data::report(rep), Data::json(stats)])
}

// -------------------------------------------------------------- histogram

fn hist_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.deviation_histogram", "Deviation Histogram", Stage::Clean)
        .input(geo_in())
        .output(PortSpec::new("histogram", DataKind::Json))
        .output(PortSpec::new("csv", DataKind::File))
        .param(ParamSpec::string("attrib", "Attribute", "dist").help("Float point attribute holding the deviation in mm."))
        .param(ParamSpec::int("bins", "Bins", 100, 4, 2000).help("Number of equal-width bins."))
        .param(ParamSpec::choice("range_mode", "Range", "symmetric", &["symmetric", "auto", "manual"]).help("symmetric: minus to plus the limit. auto: data minimum to maximum. manual: the two values below."))
        .param(ParamSpec::float("limit", "Symmetric limit", 0.5, 0.001, 1000.0).unit("mm").help("Half-width in symmetric mode."))
        .param(ParamSpec::float("range_min", "Manual minimum", -0.5, -100000.0, 100000.0).unit("mm").help("Lower end of the range in manual mode."))
        .param(ParamSpec::float("range_max", "Manual maximum", 0.5, -100000.0, 100000.0).unit("mm").help("Upper end of the range in manual mode."))
        .param(ParamSpec::string("csv_name", "CSV file name", "deviation_histogram.csv").help("Written into the node's work folder."))
        .help("Histogram of the deviation with counts, percentages and cumulative percentages per bin, as JSON and as a CSV file. Values outside the range are counted separately.")
        .emulates("deviation histogram with cumulative readout")
        .spec(SPEC_ALIGN)
        .implemented()
}

fn hist_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    let attrib = p.str("attrib")?.to_string();
    let bins = pi(p, "bins", 4, 2000)? as usize;
    let dev = collect_dev(ctx, &g, &attrib, false)?;
    let (mut lo, mut hi) = match p.str("range_mode")? {
        "auto" => {
            let m = moments(&dev.vals, None);
            (m.min, m.max)
        }
        "manual" => (pf(p, "range_min", -100000.0, 100000.0)?, pf(p, "range_max", -100000.0, 100000.0)?),
        _ => {
            let l = pf(p, "limit", 0.001, 1000.0)?;
            (-l, l)
        }
    };
    if lo >= hi {
        if p.str("range_mode")? == "manual" {
            return Err(ctx.fail("manual range needs minimum < maximum"));
        }
        lo -= 0.5;
        hi += 0.5;
    }
    let (counts, below, above) = histogram(&dev.vals, lo, hi, bins);
    let n = dev.vals.len() as f64;
    let w = (hi - lo) / bins as f64;
    let edges: Vec<f64> = (0..=bins).map(|i| lo + w * i as f64).collect();
    let mut cum = Vec::with_capacity(bins);
    let mut run = below;
    for &c in &counts {
        run += c;
        cum.push(100.0 * run as f64 / n);
    }
    let name = std::path::Path::new(p.str("csv_name")?)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ctx.fail("CSV file name is empty"))?
        .to_string();
    let mut csv = String::from("bin_lo,bin_hi,count,percent,cumulative_percent\n");
    for i in 0..bins {
        let _ = writeln!(csv, "{},{},{},{},{}", edges[i], edges[i + 1], counts[i], 100.0 * counts[i] as f64 / n, cum[i]);
    }
    std::fs::create_dir_all(&ctx.work_dir)?;
    let path = ctx.work_dir.join(name);
    std::fs::write(&path, csv)?;
    let j = json!({
        "schema": "deviation_histogram.v1",
        "attrib": attrib,
        "lo": lo, "hi": hi, "bins": bins,
        "edges": edges, "counts": counts, "cumulative_percent": cum,
        "below": below, "above": above, "n_valid": dev.vals.len(), "n_invalid": dev.n_invalid,
    });
    Ok(vec![Data::json(j), Data::File(path)])
}

// ----------------------------------------------------------------- colours

fn colors_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.deviation_colors", "Deviation Colors", Stage::Clean)
        .input(geo_in())
        .output(geo_in_out())
        .param(ParamSpec::string("attrib", "Attribute", "dist").help("Float point attribute holding the deviation in mm."))
        .param(ParamSpec::choice("range_mode", "Range", "symmetric", &["symmetric", "auto", "manual"]).help("symmetric: minus to plus the limit, zero in the middle. auto: data minimum to maximum. manual: the two values below."))
        .param(ParamSpec::float("limit", "Symmetric limit", 0.5, 0.001, 1000.0).unit("mm").help("Half-width of the range in symmetric mode."))
        .param(ParamSpec::float("range_min", "Manual minimum", -0.5, -100000.0, 100000.0).unit("mm").help("Lower end of the range in manual mode."))
        .param(ParamSpec::float("range_max", "Manual maximum", 0.5, -100000.0, 100000.0).unit("mm").help("Upper end of the range in manual mode."))
        .param(ParamSpec::choice("scale", "Colour scale", "diverging", &["diverging", "spectrum", "grey"]).help("diverging: blue, cyan, green, yellow, red with zero on green."))
        .param(ParamSpec::int("steps", "Steps", 0, 0, 32).help("0 = smooth. Otherwise the range is split into this many flat colour bands."))
        .param(ParamSpec::vec3("invalid_color", "Invalid colour", DVec3::splat(0.5)).help("Colour of points without a valid deviation."))
        .help("Writes the point colour 'Cd' from the deviation: values beyond the range saturate at the end colours, invalid points get the invalid colour.")
        .emulates("colour deviation map with symmetric or manual range and stepped legend")
        .spec(SPEC_ALIGN)
        .implemented()
}

fn geo_in_out() -> PortSpec {
    PortSpec::new("geometry", DataKind::Geometry)
}

fn colors_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    let attrib = p.str("attrib")?.to_string();
    let d = g
        .point_attribs
        .floats(&attrib)
        .map_err(|_| ctx.fail(format!("the input has no float point attribute '{attrib}'; run a distance node first")))?;
    let valid = g.point_attribs.ints("dist_valid").ok().filter(|v| v.len() == d.len() && attrib == "dist");
    let scale = Scale::parse(p.str("scale")?).unwrap_or(Scale::Diverging);
    let steps = pi(p, "steps", 0, 32)? as usize;
    let inv = p.vec3("invalid_color")?;
    let ok = |i: usize| d[i].is_finite() && valid.is_none_or(|v| v[i] == 1);
    let (lo, hi) = match p.str("range_mode")? {
        "auto" => {
            let vals: Vec<f64> = (0..d.len()).filter(|&i| ok(i)).map(|i| d[i]).collect();
            if vals.is_empty() {
                return Err(ctx.fail("the input has no valid deviation values"));
            }
            let m = moments(&vals, None);
            (m.min, m.max)
        }
        "manual" => {
            let (a, b) = (pf(p, "range_min", -100000.0, 100000.0)?, pf(p, "range_max", -100000.0, 100000.0)?);
            if a >= b {
                return Err(ctx.fail("manual range needs minimum < maximum"));
            }
            (a, b)
        }
        _ => {
            let l = pf(p, "limit", 0.001, 1000.0)?;
            (-l, l)
        }
    };
    let cd: Vec<DVec3> = (0..d.len())
        .map(|i| {
            if ok(i) {
                ramp(scale, range_t(d[i], lo, hi, steps))
            } else {
                inv
            }
        })
        .collect();
    let mut out = (*g).clone();
    out.point_attribs.set("Cd", AttribArray::Vec3(cd));
    Ok(vec![Data::geometry(out)])
}

// -------------------------------------------------------------------- gate

fn gate_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.tolerance_gate", "Tolerance Gate", Stage::Clean)
        .input(PortSpec::new("stats", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::float("tolerance", "Tolerance", 0.5, 0.001, 100.0).unit("mm").help("The absolute deviation band to check."))
        .param(ParamSpec::float("target_fraction", "Target share", 0.95, 0.5, 1.0).help("Share of points that must lie within the tolerance."))
        .param(ParamSpec::bool("fail_on_miss", "Fail on miss", false).help("Off: a miss is a warning. On: a miss is an error that stops downstream gating."))
        .help("Checks a deviation statistics record against a tolerance and target share, e.g. '95 % within 0.5 mm', and reports pass or fail for the next stage.")
        .emulates("tolerance pass/fail check on a deviation report")
        .spec(SPEC_ALIGN)
        .implemented()
}

fn gate_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let s = json_in(inp, 0, "stats")?;
    if s.get("schema").and_then(|v| v.as_str()) != Some(SCHEMA) {
        return Err(CookError::Other(format!("input is not a '{SCHEMA}' record")));
    }
    let tol = pf(p, "tolerance", 0.001, 100.0)?;
    let target = pf(p, "target_fraction", 0.5, 1.0)?;
    let fail_on_miss = p.bool("fail_on_miss")?;
    let table = s
        .get("abs_quantiles")
        .and_then(|t| t.as_array())
        .ok_or_else(|| ctx.fail("the statistics record has no quantile table"))?;
    let rows: Vec<(f64, f64)> = table
        .iter()
        .filter_map(|r| Some((r.get(0)?.as_f64()?, r.get(1)?.as_f64()?)))
        .collect();
    if rows.is_empty() {
        return Err(ctx.fail("the statistics record has an empty quantile table"));
    }
    // the first tabulated quantile at or above the target is the conservative reading
    let tq = target * 100.0;
    let row = rows.iter().find(|r| r.0 >= tq - 1e-9).copied().unwrap_or(rows[rows.len() - 1]);
    let pass = row.1 <= tol + 1e-12;
    // largest tabulated share known to lie within the tolerance
    let achieved = rows.iter().filter(|r| r.1 <= tol + 1e-12).map(|r| r.0).fold(0.0, f64::max);
    let max_abs = s.get("max_abs").and_then(|v| v.as_f64()).unwrap_or(f64::NAN);
    let mean = s.get("mean").and_then(|v| v.as_f64()).unwrap_or(f64::NAN);
    let n_invalid = s.get("n_invalid").and_then(|v| v.as_u64()).unwrap_or(0);

    let mut rep = Report::new("Tolerance Gate");
    rep.metric("tolerance", Metric::new(tol, "mm"));
    rep.metric("target_share", Metric::new(tq, "%"));
    rep.metric("p_target_abs", Metric::new(row.1, "mm").with_limit(tol, true));
    rep.metric("share_within_at_least", Metric::new(achieved, "%"));
    rep.metric("pass", Metric::new(if pass { 1.0 } else { 0.0 }, "bool").with_limit(1.0, false));
    if max_abs.is_finite() {
        rep.metric("max_abs_dev", Metric::new(max_abs, "mm"));
    }
    if pass {
        rep.note(Severity::Info, format!("pass: the {:.1}th percentile of |deviation| is {:.4} mm, within {tol} mm", row.0, row.1));
        if max_abs.is_finite() && max_abs > 3.0 * tol {
            rep.note(Severity::Info, format!("worst deviation {max_abs:.3} mm lies well outside the band; inspect the tail"));
        }
    } else {
        let sev = if fail_on_miss { Severity::Error } else { Severity::Warning };
        rep.note(
            sev,
            format!(
                "fail: the {:.1}th percentile of |deviation| is {:.4} mm, above {tol} mm (at least {achieved:.1} % are within the band, {tq:.1} % required)",
                row.0, row.1
            ),
        );
    }
    if mean.is_finite() && mean.abs() > 0.5 * tol {
        rep.note(Severity::Warning, format!("mean deviation {mean:.3} mm is a systematic offset; re-check the alignment before trusting the share"));
    }
    if n_invalid > 0 {
        rep.note(Severity::Info, format!("{n_invalid} sample(s) had no valid deviation and were not counted"));
    }
    let _ = stats::Scale::Grey;
    Ok(vec![Data::report(rep)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;

    /// Geometry whose `dist` attribute holds exactly `vals`.
    fn dev_geo(vals: Vec<f64>) -> Geometry {
        let n = vals.len();
        let mut g = Geometry::from_points((0..n).map(|i| DVec3::new(i as f64, 0.0, 0.0)).collect());
        g.point_attribs.set("dist", AttribArray::Float(vals));
        g
    }

    #[test]
    fn golden_11_report_and_gate() {
        let vals: Vec<f64> = (1..=1000).map(|k| k as f64 / 1000.0).collect();
        let out = run(stats_info, stats_cook, vec![geo(dev_geo(vals))], &[]).unwrap();
        let r = out_report(&out[0]);
        assert!((r.metrics["p50_abs"].value - 0.5).abs() < 1e-12);
        assert!((r.metrics["p95_abs"].value - 0.95).abs() < 1e-12);
        assert!((r.metrics["mean_abs_dev"].value - 0.5005).abs() < 1e-12);
        assert!((r.metrics["rms_dev"].value - 0.577783).abs() < 1e-6);
        assert!((r.metrics["fraction_within"].value - 50.0).abs() < 1e-9);
        // the 95 %-within-0.5 mm gate fails, both in the report and in the gate node
        assert_eq!(r.metrics["p95_abs"].passes(), Some(false));
        assert_eq!(r.metrics["fraction_within"].passes(), Some(false));
        assert!(!r.passes());
        let stats = out_json(&out[1]).clone();
        assert_eq!(stats["schema"], SCHEMA);
        assert_eq!(stats["verdict"]["pass"], false);
        let gate = run(gate_info, gate_cook, vec![json(stats.clone())], &[]).unwrap();
        let gr = out_report(&gate[0]);
        assert_eq!(gr.metrics["pass"].value, 0.0);
        assert!(gr.notes.iter().any(|n| n.0 == Severity::Warning && n.1.contains("fail")));
        let strict = run(gate_info, gate_cook, vec![json(stats.clone())], &[("fail_on_miss", b(true))]).unwrap();
        assert!(!out_report(&strict[0]).passes());
        // a looser tolerance passes
        let loose = run(gate_info, gate_cook, vec![json(stats)], &[("tolerance", f(0.96))]).unwrap();
        assert_eq!(out_report(&loose[0]).metrics["pass"].value, 1.0);
        assert!(out_report(&loose[0]).passes());
    }

    #[test]
    fn golden_12_inclusive_boundary() {
        let mut v = vec![0.2; 9500];
        v.extend(vec![0.8; 500]);
        let out = run(stats_info, stats_cook, vec![geo(dev_geo(v))], &[]).unwrap();
        let r = out_report(&out[0]);
        assert!((r.metrics["mean_abs_dev"].value - 0.23).abs() < 1e-12);
        assert!((r.metrics["rms_dev"].value - 0.26458).abs() < 1e-4);
        assert!((r.metrics["p95_abs"].value - 0.2).abs() < 1e-12);
        assert!((r.metrics["p99_abs"].value - 0.8).abs() < 1e-12);
        assert!(r.passes());
        assert_eq!(out_json(&out[1])["verdict"]["pass"], true);
        let g = run(gate_info, gate_cook, vec![json(out_json(&out[1]).clone())], &[]).unwrap();
        assert_eq!(out_report(&g[0]).metrics["pass"].value, 1.0);
        // 9 499 / 501 fails
        let mut w = vec![0.2; 9499];
        w.extend(vec![0.8; 501]);
        let out = run(stats_info, stats_cook, vec![geo(dev_geo(w))], &[]).unwrap();
        assert!(!out_report(&out[0]).passes());
    }

    #[test]
    fn signed_values_use_absolute_percentiles_and_bands() {
        let out = run(
            stats_info,
            stats_cook,
            vec![geo(dev_geo(vec![-0.4, -0.1, 0.0, 0.1, 0.3, 0.9]))],
            &[("band_edges", s("-0.5,0,0.5")), ("tolerance", f(0.5)), ("target_fraction", f(0.8))],
        )
        .unwrap();
        let st = out_json(&out[1]);
        assert_eq!(st["bands"][0]["count"], 2);
        assert_eq!(st["bands"][1]["count"], 3);
        assert_eq!(st["above"], 1);
        assert_eq!(st["below"], 0);
        assert!((out_report(&out[0]).metrics["mean_dev"].value - (-0.4 - 0.1 + 0.1 + 0.3 + 0.9) / 6.0).abs() < 1e-12);
        assert!(out_report(&out[0]).metrics.contains_key("p80_abs"));
    }

    #[test]
    fn invalid_samples_are_excluded_and_counted() {
        let mut g = dev_geo(vec![0.1, 0.2, 9.0, f64::NAN]);
        g.point_attribs.set("dist_valid", AttribArray::Int(vec![1, 1, 0, 1]));
        let out = run(stats_info, stats_cook, vec![geo(g)], &[]).unwrap();
        let r = out_report(&out[0]);
        assert_eq!(r.metrics["valid_points"].value, 2.0);
        assert_eq!(r.metrics["invalid_points"].value, 2.0);
        assert!((r.metrics["max_abs_dev"].value - 0.2).abs() < 1e-12);
    }

    #[test]
    fn area_weighting_changes_the_mean() {
        // two triangles sharing an edge; the big one has deviation 1, the small 0
        let mut g = Geometry::from_polygons(
            vec![
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(10.0, 0.0, 0.0),
                DVec3::new(0.0, 10.0, 0.0),
                DVec3::new(10.0, 10.0, 0.0),
                DVec3::new(11.0, 10.0, 0.0),
                DVec3::new(10.0, 11.0, 0.0),
            ],
            vec![vec![0, 1, 2], vec![3, 4, 5]],
        );
        g.point_attribs.set("dist", AttribArray::Float(vec![1.0, 1.0, 1.0, 0.0, 0.0, 0.0]));
        let point = run(stats_info, stats_cook, vec![geo(g.clone())], &[]).unwrap();
        let area = run(stats_info, stats_cook, vec![geo(g)], &[("weighting", s("area"))]).unwrap();
        let mp = out_report(&point[0]).metrics["mean_dev"].value;
        let ma = out_report(&area[0]).metrics["mean_dev"].value;
        assert!((mp - 0.5).abs() < 1e-12);
        assert!((ma - 50.0 / 50.5).abs() < 1e-9, "{ma}");
    }

    #[test]
    fn alignment_block_is_carried_from_icp() {
        let mut g = dev_geo(vec![0.1, 0.2]);
        g.detail_attribs.set("icp_rms_before", AttribArray::Float(vec![3.0]));
        g.detail_attribs.set("icp_rms_after", AttribArray::Float(vec![0.05]));
        g.detail_attribs.set("icp_iterations", AttribArray::Int(vec![7]));
        let out = run(stats_info, stats_cook, vec![geo(g)], &[]).unwrap();
        let st = out_json(&out[1]);
        assert_eq!(st["alignment"]["iterations"], 7);
        assert_eq!(st["alignment"]["rms_before"], 3.0);
    }

    #[test]
    fn stats_errors_are_clean() {
        let g = Geometry::from_points(vec![DVec3::ZERO]);
        assert!(run(stats_info, stats_cook, vec![geo(g)], &[]).is_err());
        let g = dev_geo(vec![0.1, 0.2]);
        assert!(run(stats_info, stats_cook, vec![geo(g.clone())], &[("percentiles", s("0,101"))]).is_err());
        assert!(run(stats_info, stats_cook, vec![geo(g)], &[("band_edges", s("1,0"))]).is_err());
        assert!(run(gate_info, gate_cook, vec![json(json!({"schema": "other"}))], &[]).is_err());
    }

    #[test]
    fn golden_13_histogram_node_and_csv() {
        let vals: Vec<f64> = (1..=1000).map(|k| k as f64 / 1000.0).collect();
        let out = run(
            hist_info,
            hist_cook,
            vec![geo(dev_geo(vals))],
            &[("bins", i(10)), ("range_mode", s("manual")), ("range_min", f(0.0)), ("range_max", f(1.0)), ("csv_name", s("golden13.csv"))],
        )
        .unwrap();
        let h = out_json(&out[0]);
        let counts: Vec<u64> = h["counts"].as_array().unwrap().iter().map(|c| c.as_u64().unwrap()).collect();
        assert_eq!(counts, vec![99, 100, 100, 100, 100, 100, 100, 100, 100, 101]);
        let cum = h["cumulative_percent"].as_array().unwrap();
        assert!((cum[9].as_f64().unwrap() - 100.0).abs() < 1e-9);
        assert!((cum[0].as_f64().unwrap() - 9.9).abs() < 1e-9);
        let path = out[1].as_file().unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.starts_with("bin_lo,bin_hi,count"));
        assert_eq!(text.lines().count(), 11);
    }

    #[test]
    fn histogram_ranges_and_errors() {
        let g = dev_geo(vec![-0.2, 0.1, 0.4, 3.0]);
        let sym = run(hist_info, hist_cook, vec![geo(g.clone())], &[("bins", i(10))]).unwrap();
        assert_eq!(out_json(&sym[0])["above"], 1);
        let auto = run(hist_info, hist_cook, vec![geo(g.clone())], &[("bins", i(10)), ("range_mode", s("auto"))]).unwrap();
        assert_eq!(out_json(&auto[0])["above"], 0);
        let counts_total: u64 = out_json(&auto[0])["counts"].as_array().unwrap().iter().map(|c| c.as_u64().unwrap()).sum();
        assert_eq!(counts_total, 4);
        assert!(run(hist_info, hist_cook, vec![geo(g.clone())], &[("range_mode", s("manual")), ("range_min", f(1.0)), ("range_max", f(1.0))]).is_err());
        // the CSV name cannot escape the work folder
        let esc = run(hist_info, hist_cook, vec![geo(g)], &[("csv_name", s("../../evil.csv"))]).unwrap();
        assert!(esc[1].as_file().unwrap().ends_with("evil.csv"));
        assert!(esc[1].as_file().unwrap().starts_with(std::env::temp_dir()));
    }

    #[test]
    fn golden_14_colour_node() {
        let mut g = dev_geo(vec![-0.5, 0.0, 0.25, 2.0, 0.1]);
        g.point_attribs.set("dist_valid", AttribArray::Int(vec![1, 1, 1, 1, 0]));
        let out = run(colors_info, colors_cook, vec![geo(g)], &[]).unwrap();
        let cd = out_geo(&out[0]).point_attribs.vec3s("Cd").unwrap();
        assert_eq!(cd[0], ramp(Scale::Diverging, 0.0));
        assert_eq!(cd[1], ramp(Scale::Diverging, 0.5));
        assert_eq!(cd[2], ramp(Scale::Diverging, 0.75));
        assert_eq!(cd[3], ramp(Scale::Diverging, 1.0));
        assert_eq!(cd[4], DVec3::splat(0.5));
    }

    #[test]
    fn colour_steps_and_manual_range() {
        let g = dev_geo(vec![0.01, 0.2, 0.9]);
        let out = run(
            colors_info,
            colors_cook,
            vec![geo(g.clone())],
            &[("range_mode", s("manual")), ("range_min", f(0.0)), ("range_max", f(1.0)), ("steps", i(4))],
        )
        .unwrap();
        let cd = out_geo(&out[0]).point_attribs.vec3s("Cd").unwrap();
        assert_eq!(cd[0], cd[1]);
        assert_ne!(cd[1], cd[2]);
        assert!(run(colors_info, colors_cook, vec![geo(g)], &[("range_mode", s("manual")), ("range_min", f(1.0)), ("range_max", f(0.0))]).is_err());
    }

    #[test]
    fn full_chain_scan_to_gate() {
        // flat plate reference, scan 0.3 mm proud everywhere -> mean 0.3, all within 0.5
        let reference = Geometry::from_polygons(
            vec![
                DVec3::new(-100.0, -100.0, 0.0),
                DVec3::new(100.0, -100.0, 0.0),
                DVec3::new(100.0, 100.0, 0.0),
                DVec3::new(-100.0, 100.0, 0.0),
            ],
            vec![vec![0, 1, 2], vec![0, 2, 3]],
        );
        let scan = Geometry::from_points((0..400).map(|i| DVec3::new((i % 20) as f64 * 9.0 - 90.0, (i / 20) as f64 * 9.0 - 90.0, 0.3)).collect());
        let d = run(crate::distance::mesh_info, crate::distance::mesh_cook, vec![geo(scan), geo(reference)], &[]).unwrap();
        let st = run(stats_info, stats_cook, vec![d[0].clone().into()], &[]).unwrap();
        let r = out_report(&st[0]);
        assert!((r.metrics["mean_dev"].value - 0.3).abs() < 1e-9);
        assert!(r.passes());
        let gate = run(gate_info, gate_cook, vec![json(out_json(&st[1]).clone())], &[]).unwrap();
        assert!(out_report(&gate[0]).passes());
        // the 0.3 mm systematic offset is flagged against a 0.5 tolerance? 0.3 > 0.25 -> yes
        assert!(out_report(&gate[0]).notes.iter().any(|n| n.1.contains("systematic offset")));
    }
}

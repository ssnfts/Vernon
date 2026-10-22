//! Scanner error model and verification: scanner profiles, the volumetric
//! drift prediction `E(L) = a + b*L`, the combined error budget, the length
//! check against reference distances and the scan quality estimate.
//!
//! The scanner figures are claims to check, not measured truth (spec 2.1):
//! the nodes here only evaluate the published model form and compare it with
//! what the data says.

use std::collections::HashMap;

use glam::DVec3;
use serde_json::{json, Value};
use vernon_core::{Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{
    CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec,
};

use crate::spatial::{plane_fit, Grid};
use crate::tables::{bars_from_json, parse_bar_list, BarSpec};
use crate::util::{self, base};

// ---------------------------------------------------------------------------
// Model functions
// ---------------------------------------------------------------------------

/// Maximum-permissible-error style length accuracy `a + b*L` in mm, with the
/// constant term `a` in mm, the slope `b` in mm per metre and `L` in mm.
pub fn e_scanner(a_mm: f64, b_mm_per_m: f64, length_mm: f64) -> f64 {
    a_mm + b_mm_per_m * length_mm / 1000.0
}

/// Length error caused by the scale uncertainty of `n_bars` independent bars
/// of `bar_len_mm` with standard uncertainty `sigma_mm`, over `length_mm`.
pub fn e_scale(length_mm: f64, sigma_mm: f64, bar_len_mm: f64, n_bars: usize) -> f64 {
    length_mm * (sigma_mm / bar_len_mm) / (n_bars.max(1) as f64).sqrt()
}

/// Combine the three error terms: root-sum-square, or a plain sum for a
/// worst-case bound.
pub fn combine(terms: &[f64], rss: bool) -> f64 {
    if rss {
        terms.iter().map(|t| t * t).sum::<f64>().sqrt()
    } else {
        terms.iter().sum()
    }
}

// ---------------------------------------------------------------------------
// Scanner profile
// ---------------------------------------------------------------------------

/// A scanner's published accuracy figures.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub preset: String,
    /// Constant term of the length accuracy, mm.
    pub a_mm: f64,
    /// Length-proportional term, mm per metre.
    pub b_mm_per_m: f64,
    pub single_frame_mm: f64,
    pub point_distance_mm: f64,
    pub work_dist_mm: [f64; 2],
    pub surface_finish: String,
    /// Planning multiplier for glossy, undeveloped surfaces (1 = none).
    pub haircut: f64,
}

/// `(key, a, b, single frame, point distance, work distance min, max)`.
const PRESETS: &[(&str, f64, f64, f64, f64, f64, f64)] = &[
    ("tracked_handheld_laser", 0.025, 0.04, 0.02, 0.05, 200.0, 400.0),
    ("marker_assisted_laser", 0.04, 0.06, 0.05, 0.05, 170.0, 550.0),
    ("photogrammetry_assisted_laser", 0.02, 0.05, 0.04, 0.05, 200.0, 400.0),
    ("fast_infrared_mode", 0.1, 0.3, 0.1, 0.2, 160.0, 1500.0),
    ("entry_handheld_laser", 0.1, 0.3, 0.04, 0.1, 100.0, 400.0),
];

impl Profile {
    /// A named preset; `None` for `custom` or unknown names.
    pub fn preset(name: &str) -> Option<Profile> {
        PRESETS.iter().find(|p| p.0 == name).map(|p| Profile {
            preset: p.0.to_string(),
            a_mm: p.1,
            b_mm_per_m: p.2,
            single_frame_mm: p.3,
            point_distance_mm: p.4,
            work_dist_mm: [p.5, p.6],
            surface_finish: "matte".into(),
            haircut: 1.0,
        })
    }

    pub fn preset_names() -> Vec<&'static str> {
        PRESETS.iter().map(|p| p.0).collect()
    }

    /// Slope as parts per million.
    pub fn drift_ppm(&self) -> f64 {
        self.b_mm_per_m * 1000.0
    }

    /// Predicted length error over `length_mm` (the model, before any haircut).
    pub fn e_scanner(&self, length_mm: f64) -> f64 {
        e_scanner(self.a_mm, self.b_mm_per_m, length_mm)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "preset": self.preset,
            "a_mm": self.a_mm,
            "b_mm_per_m": self.b_mm_per_m,
            "single_frame_mm": self.single_frame_mm,
            "point_distance_mm": self.point_distance_mm,
            "work_distance_mm": self.work_dist_mm,
            "surface_finish": self.surface_finish,
            "haircut": self.haircut,
        })
    }

    pub fn from_json(v: &Value) -> Option<Profile> {
        let a_mm = v.get("a_mm")?.as_f64()?;
        let b_mm_per_m = v.get("b_mm_per_m")?.as_f64()?;
        if !(a_mm.is_finite() && b_mm_per_m.is_finite() && a_mm >= 0.0 && b_mm_per_m >= 0.0) {
            return None;
        }
        let wd = v.get("work_distance_mm").and_then(Value::as_array);
        let wd_at = |i: usize| wd.and_then(|a| a.get(i)).and_then(Value::as_f64).unwrap_or(0.0);
        Some(Profile {
            preset: v.get("preset").and_then(Value::as_str).unwrap_or("custom").to_string(),
            a_mm,
            b_mm_per_m,
            single_frame_mm: v.get("single_frame_mm").and_then(Value::as_f64).unwrap_or(0.0),
            point_distance_mm: v.get("point_distance_mm").and_then(Value::as_f64).unwrap_or(0.0),
            work_dist_mm: [wd_at(0), wd_at(1)],
            surface_finish: v.get("surface_finish").and_then(Value::as_str).unwrap_or("matte").to_string(),
            haircut: v.get("haircut").and_then(Value::as_f64).unwrap_or(1.0).max(1.0),
        })
    }
}

fn profile_input(inputs: &[Option<Data>], index: usize) -> Option<Profile> {
    util::json_input(inputs, index).and_then(|v| Profile::from_json(&v))
}

/// The profile to use: the connected one, otherwise built from `a`, `b`.
fn profile_or_params(ctx: &CookContext, inputs: &[Option<Data>], index: usize, params: &Params) -> CookResult<Profile> {
    if let Some(p) = profile_input(inputs, index) {
        return Ok(p);
    }
    if util::json_input(inputs, index).is_some() {
        return Err(ctx.fail("the connected profile has no valid 'a_mm' / 'b_mm_per_m' values"));
    }
    let a = params.float("a")?;
    let b = params.float("b")?;
    util::require_non_negative(ctx, "a", a)?;
    util::require_non_negative(ctx, "b", b)?;
    Ok(Profile {
        preset: "custom".into(),
        a_mm: a,
        b_mm_per_m: b,
        single_frame_mm: 0.0,
        point_distance_mm: 0.0,
        work_dist_mm: [0.0, 0.0],
        surface_finish: "matte".into(),
        haircut: 1.0,
    })
}

fn a_param() -> ParamSpec {
    ParamSpec::float("a", "Constant term a", 0.02, 0.0, 10.0)
        .unit("mm")
        .help("Constant part of the length accuracy a + b*L. Ignored when a profile is connected.")
}

fn b_param() -> ParamSpec {
    ParamSpec::float("b", "Slope b", 0.05, 0.0, 5.0)
        .unit("mm/m")
        .help("Length-proportional part of the length accuracy, mm per metre of measured length. Ignored when a profile is connected.")
}

/// Scanner accuracy profile source node.
pub struct ScannerProfile;

impl NodeOp for ScannerProfile {
    fn info(&self) -> NodeTypeInfo {
        let mut presets = vec!["custom"];
        presets.extend(Profile::preset_names());
        base("capture.scanner_profile", "Scanner Profile")
            .output(PortSpec::new("profile", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(
                ParamSpec::choice("preset", "Preset", "custom", &presets)
                    .help("Generic scanner class. A preset overrides the numeric fields below; all figures are published claims, not measurements."),
            )
            .param(a_param())
            .param(b_param())
            .param(ParamSpec::float("single_frame", "Single-frame accuracy", 0.04, 0.0, 10.0).unit("mm"))
            .param(ParamSpec::float("point_distance", "Point distance", 0.05, 0.0, 50.0).unit("mm").help("Spacing between measured points."))
            .param(ParamSpec::float("work_dist_min", "Working distance min", 200.0, 0.0, 10000.0).unit("mm"))
            .param(ParamSpec::float("work_dist_max", "Working distance max", 400.0, 0.0, 10000.0).unit("mm"))
            .param(
                ParamSpec::choice("surface_finish", "Surface finish", "matte", &["matte", "developer_spray", "gloss_paint"])
                    .help("Recorded on the profile only; glossy paint without a matting spray is usually worse than the lab figures."),
            )
            .param(
                ParamSpec::float("haircut", "Planning factor", 1.0, 1.0, 10.0)
                    .help("Multiplier on the predicted error for difficult surfaces (a planning heuristic of 3 to 5 is common for first-run gloss paint)."),
            )
            .help("Holds the published accuracy figures of a scanner: the constant and per-metre terms of the length accuracy, the single-frame accuracy, point spacing and working distance. Feeds the drift report, error budget and length check.")
            .emulates("handheld laser scanner specification sheet (a + b mm/m length accuracy)")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, _inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let name = params.str("preset")?;
        let mut p = if name == "custom" {
            let a = params.float("a")?;
            let b = params.float("b")?;
            util::require_non_negative(ctx, "a", a)?;
            util::require_non_negative(ctx, "b", b)?;
            let (wmin, wmax) = (params.float("work_dist_min")?, params.float("work_dist_max")?);
            if wmax < wmin {
                return Err(ctx.fail("working distance max is below the minimum"));
            }
            Profile {
                preset: "custom".into(),
                a_mm: a,
                b_mm_per_m: b,
                single_frame_mm: params.float("single_frame")?,
                point_distance_mm: params.float("point_distance")?,
                work_dist_mm: [wmin, wmax],
                surface_finish: String::new(),
                haircut: 1.0,
            }
        } else {
            Profile::preset(name).ok_or_else(|| util::bad_choice(ctx, "preset", name))?
        };
        p.surface_finish = params.str("surface_finish")?.to_string();
        p.haircut = params.float("haircut")?.max(1.0);
        let mut rep = Report::new("Scanner Profile");
        rep.metric("a", Metric::new(p.a_mm, "mm"));
        rep.metric("b", Metric::new(p.b_mm_per_m, "mm/m"));
        rep.metric("drift_ppm", Metric::new(p.drift_ppm(), "ppm"));
        rep.metric("single_frame", Metric::new(p.single_frame_mm, "mm"));
        rep.metric("point_distance", Metric::new(p.point_distance_mm, "mm"));
        rep.metric("haircut", Metric::new(p.haircut, "-"));
        rep.note(
            Severity::Info,
            "figures are published or user-entered claims; verify them with the length check on a measured reference",
        );
        Ok(vec![Data::json(p.to_json()), Data::report(rep)])
    }
}

// ---------------------------------------------------------------------------
// Drift report
// ---------------------------------------------------------------------------

/// Predicted volumetric drift `a + b*L` over a car length.
pub struct DriftReport;

impl NodeOp for DriftReport {
    fn info(&self) -> NodeTypeInfo {
        base("capture.drift_report", "Drift Report")
            .input(PortSpec::optional("profile", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("table", DataKind::Json))
            .param(ParamSpec::float("length", "Length", 4500.0, 100.0, 30000.0).unit("mm").help("Span the scan has to hold, e.g. the car length."))
            .param(a_param())
            .param(b_param())
            .param(ParamSpec::float("gate", "Gate", 1.0, 0.001, 100.0).unit("mm").help("Largest acceptable length error over the span."))
            .param(ParamSpec::float("haircut", "Planning factor", 1.0, 1.0, 10.0).help("Multiplier for difficult surfaces; the gate is checked against the planning value."))
            .help("Predicts the length error of a scan from the scanner's published figure a + b*L, shows how much of it is drift that grows with length, and checks it against a gate. A claim to verify, not a measurement.")
            .emulates("scanner volumetric accuracy figure (constant mm plus mm per metre)")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let profile = profile_or_params(ctx, inputs, 0, params)?;
        let length = params.float("length")?;
        let gate = params.float("gate")?;
        let haircut = params.float("haircut")?.max(profile.haircut);
        util::require_positive(ctx, "length", length)?;
        util::require_positive(ctx, "gate", gate)?;
        let drift = profile.b_mm_per_m * length / 1000.0;
        let e = profile.e_scanner(length);
        let planning = e * haircut;
        let mut rep = Report::new("Drift Report");
        rep.metric("e_scanner", Metric::new(e, "mm"));
        rep.metric("constant_term", Metric::new(profile.a_mm, "mm"));
        rep.metric("drift_term", Metric::new(drift, "mm"));
        rep.metric("drift_ppm", Metric::new(profile.drift_ppm(), "ppm"));
        rep.metric("drift_share", Metric::new(if e > 0.0 { drift / e * 100.0 } else { 0.0 }, "%"));
        rep.metric("e_planning", Metric::new(planning, "mm").with_limit(gate, true));
        if drift > profile.a_mm {
            rep.note(
                Severity::Info,
                format!("over {:.0} mm the length-proportional drift ({drift:.3} mm) dominates the constant term ({:.3} mm)", length, profile.a_mm),
            );
        }
        if planning > gate {
            rep.note(
                Severity::Warning,
                format!("predicted length error {planning:.3} mm exceeds the {gate:.3} mm gate; use markers, a tracker or a scale correction"),
            );
        }
        let mut lengths = vec![250.0, 500.0, 1000.0, 2000.0, 3000.0, 4500.0, 6000.0, length];
        lengths.sort_by(f64::total_cmp);
        lengths.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        let rows: Vec<Value> = lengths
            .iter()
            .map(|&l| {
                json!({
                    "length_mm": l,
                    "e_mm": profile.e_scanner(l),
                    "drift_mm": profile.b_mm_per_m * l / 1000.0,
                    "constant_mm": profile.a_mm,
                })
            })
            .collect();
        Ok(vec![Data::report(rep), Data::json(json!({ "profile": profile.to_json(), "rows": rows }))])
    }
}

// ---------------------------------------------------------------------------
// Error budget
// ---------------------------------------------------------------------------

/// Combined car-length error: scanner, scale bars and registration.
pub struct ErrorBudget;

fn metric_value(r: &Report, names: &[&str]) -> Option<f64> {
    names.iter().find_map(|n| r.metrics.get(*n)).map(|m| m.value)
}

impl NodeOp for ErrorBudget {
    fn info(&self) -> NodeTypeInfo {
        base("capture.error_budget", "Error Budget")
            .input(PortSpec::optional("profile", DataKind::Json))
            .input(PortSpec::optional("scale", DataKind::Report))
            .input(PortSpec::optional("registration", DataKind::Report))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("length", "Length", 4500.0, 100.0, 30000.0).unit("mm"))
            .param(a_param())
            .param(b_param())
            .param(ParamSpec::float("scale_bar_sigma", "Scale bar sigma", 0.01, 0.0, 100.0).unit("mm").help("Standard uncertainty of one scale bar length. Ignored when a scale report is connected."))
            .param(ParamSpec::float("scale_bar_len", "Scale bar length", 1000.0, 1.0, 20000.0).unit("mm"))
            .param(ParamSpec::int("scale_bars", "Independent bars", 1, 1, 100))
            .param(ParamSpec::float("reg_rms", "Registration RMS", 0.1, 0.0, 100.0).unit("mm").help("Inlier RMS of the multi-scan registration. Ignored when a registration report is connected."))
            .param(ParamSpec::float("k", "Coverage factor", 2.0, 1.0, 5.0).help("Multiplier on the registration RMS."))
            .param(ParamSpec::choice("combine", "Combine", "rss", &["rss", "linear"]).help("Root-sum-square, or a plain sum for a worst-case bound."))
            .param(ParamSpec::float("gate", "Gate", 1.0, 0.001, 100.0).unit("mm"))
            .help("Adds the scanner's length error, the scale-bar error and the registration error into one budget over the car length and checks it against a gate.")
            .emulates("measurement-uncertainty budget for a large scanned object")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let profile = profile_or_params(ctx, inputs, 0, params)?;
        let length = params.float("length")?;
        let gate = params.float("gate")?;
        let k = params.float("k")?;
        util::require_positive(ctx, "length", length)?;
        util::require_positive(ctx, "gate", gate)?;
        util::require_positive(ctx, "k", k)?;
        let sigma = params.float("scale_bar_sigma")?;
        let bar_len = params.float("scale_bar_len")?;
        util::require_non_negative(ctx, "scale_bar_sigma", sigma)?;
        util::require_positive(ctx, "scale_bar_len", bar_len)?;
        let n_bars = params.int("scale_bars")?.max(1) as usize;
        let rss = match params.str("combine")? {
            "rss" => true,
            "linear" => false,
            other => return Err(util::bad_choice(ctx, "combine", other)),
        };
        let mut rep = Report::new("Error Budget");
        let e_scan = profile.e_scanner(length) * profile.haircut;
        let (e_sc, scale_source) = match inputs::opt_report(inputs, 1)
            .and_then(|r| metric_value(&r, &["scale_uncertainty_ppm"]))
        {
            Some(ppm) => (length * ppm * 1e-6, "scale report"),
            None => (e_scale(length, sigma, bar_len, n_bars), "bar parameters"),
        };
        let (rms, reg_source) = match inputs::opt_report(inputs, 2)
            .and_then(|r| metric_value(&r, &["inlier_rmse", "rms"]))
        {
            Some(v) => (v, "registration report"),
            None => (params.float("reg_rms")?, "parameter"),
        };
        util::require_non_negative(ctx, "reg_rms", rms)?;
        let e_reg = k * rms;
        let total = combine(&[e_scan, e_sc, e_reg], rss);
        rep.metric("length", Metric::new(length, "mm"));
        rep.metric("e_scanner", Metric::new(e_scan, "mm"));
        rep.metric("e_scale", Metric::new(e_sc, "mm"));
        rep.metric("e_reg", Metric::new(e_reg, "mm"));
        rep.metric("e_total", Metric::new(total, "mm").with_limit(gate, true));
        rep.metric("margin", Metric::new(gate - total, "mm"));
        let terms = [("scanner", e_scan), ("scale bars", e_sc), ("registration", e_reg)];
        let (dominant, dv) = terms.iter().fold(("scanner", f64::MIN), |acc, t| if t.1 > acc.1 { *t } else { acc });
        rep.note(
            Severity::Info,
            format!("scale term from {scale_source}, registration term from {reg_source}; the {dominant} term dominates ({dv:.3} mm)"),
        );
        if total > gate {
            rep.note(
                Severity::Warning,
                format!("combined error {total:.3} mm exceeds the {gate:.3} mm gate; reduce the {dominant} term first"),
            );
        }
        Ok(vec![Data::report(rep)])
    }
}

// ---------------------------------------------------------------------------
// Length check
// ---------------------------------------------------------------------------

/// One checked marker pair.
#[derive(Debug, Clone, PartialEq)]
pub struct PairCheck {
    pub a: String,
    pub b: String,
    pub reference: f64,
    pub measured: f64,
    pub error: f64,
    pub limit: f64,
}

/// Outcome of comparing measured and reference lengths.
#[derive(Debug, Clone)]
pub struct LengthStats {
    pub b_hat_ppm: f64,
    pub max_abs_error: f64,
    pub error_ratio: f64,
    pub post_scale_residual: f64,
    pub longest: f64,
}

/// Through-origin scale regression and error ratios. `limit_of(L)` gives the
/// permitted error for a reference length.
pub fn check_lengths(pairs: &mut [PairCheck], limit_of: impl Fn(f64) -> f64) -> LengthStats {
    let mut sl2 = 0.0;
    let mut sle = 0.0;
    for p in pairs.iter_mut() {
        p.error = p.measured - p.reference;
        p.limit = limit_of(p.reference);
        sl2 += p.reference * p.reference;
        sle += p.reference * p.error;
    }
    let b_hat = if sl2 > 0.0 { sle / sl2 } else { 0.0 };
    let mut stats = LengthStats {
        b_hat_ppm: b_hat * 1e6,
        max_abs_error: 0.0,
        error_ratio: 0.0,
        post_scale_residual: 0.0,
        longest: 0.0,
    };
    for p in pairs.iter() {
        stats.max_abs_error = stats.max_abs_error.max(p.error.abs());
        stats.error_ratio = stats.error_ratio.max(p.error.abs() / p.limit.max(1e-12));
        stats.post_scale_residual = stats.post_scale_residual.max((p.error - b_hat * p.reference).abs());
        stats.longest = stats.longest.max(p.reference);
    }
    stats
}

/// Verify measured marker distances against reference lengths.
pub struct LengthCheck;

impl LengthCheck {
    fn reference_pairs(
        ctx: &CookContext,
        inputs: &[Option<Data>],
        params: &Params,
        measured: &HashMap<String, DVec3>,
    ) -> CookResult<(Vec<PairCheck>, usize)> {
        let specs: Vec<BarSpec> = if let Some(v) = util::json_input(inputs, 1) {
            bars_from_json(&v).map_err(|m| ctx.fail(format!("reference table: {m}")))?
        } else if let Some(refpts) = inputs::opt_geometry(inputs, 2) {
            let ids = util::point_ids(&refpts);
            let mut out = Vec::new();
            for i in 0..ids.len() {
                for j in (i + 1)..ids.len() {
                    out.push(BarSpec {
                        a: ids[i].clone(),
                        b: ids[j].clone(),
                        length: (refpts.points[i] - refpts.points[j]).length(),
                        sigma: None,
                        alpha: None,
                        temp_c: None,
                    });
                }
            }
            out
        } else {
            parse_bar_list(params.str("lengths")?).map_err(|m| ctx.fail(format!("lengths parameter: {m}")))?
        };
        if specs.is_empty() {
            return Err(ctx.fail(
                "no reference lengths: connect a reference table or reference markers, or fill the 'lengths' parameter",
            ));
        }
        let mut skipped = 0;
        let mut pairs = Vec::new();
        for s in specs {
            match (measured.get(&s.a), measured.get(&s.b)) {
                (Some(pa), Some(pb)) if s.length > 0.0 => pairs.push(PairCheck {
                    a: s.a,
                    b: s.b,
                    reference: s.length,
                    measured: (*pa - *pb).length(),
                    error: 0.0,
                    limit: 0.0,
                }),
                _ => skipped += 1,
            }
        }
        Ok((pairs, skipped))
    }
}

impl NodeOp for LengthCheck {
    fn info(&self) -> NodeTypeInfo {
        base("capture.length_check", "Length Check")
            .input(PortSpec::new("measured", DataKind::Geometry))
            .input(PortSpec::optional("reference", DataKind::Json))
            .input(PortSpec::optional("reference_points", DataKind::Geometry))
            .input(PortSpec::optional("profile", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("table", DataKind::Json))
            .param(ParamSpec::string("lengths", "Reference lengths", "").help("Rows 'id_a, id_b, length', separated by semicolons or new lines; used when no reference input is connected."))
            .param(ParamSpec::choice("gate_mode", "Gate", "fixed", &["fixed", "profile"]).help("'profile' checks every pair against a + b*L of the scanner profile; 'fixed' uses one gate for all."))
            .param(ParamSpec::float("gate", "Fixed gate", 1.0, 0.001, 100.0).unit("mm"))
            .param(a_param())
            .param(b_param())
            .param(ParamSpec::int("min_pairs", "Minimum pairs", 3, 1, 1000))
            .help("Compares marker distances measured in the scan with reference lengths. Reports the worst error, the through-origin scale error in ppm and the residual left after removing it, which separates a simple scale error from bending or drift.")
            .emulates("sphere-spacing style length-measurement-error test of a scanner")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let measured = inputs::geometry(inputs, 0)?;
        let ids = util::point_ids(&measured);
        let by_id: HashMap<String, DVec3> = ids.into_iter().zip(measured.points.iter().copied()).collect();
        let (mut pairs, skipped) = Self::reference_pairs(ctx, inputs, params, &by_id)?;
        let min_pairs = params.int("min_pairs")?.max(1) as usize;
        if pairs.len() < min_pairs {
            return Err(ctx.fail(format!(
                "only {} usable marker pairs found ({skipped} skipped for unknown ids), need at least {min_pairs}",
                pairs.len()
            )));
        }
        let gate = params.float("gate")?;
        util::require_positive(ctx, "gate", gate)?;
        let mode = params.str("gate_mode")?;
        let profile_mode = match mode {
            "fixed" => false,
            "profile" => true,
            other => return Err(util::bad_choice(ctx, "gate_mode", other)),
        };
        let profile = profile_or_params(ctx, inputs, 3, params)?;
        let stats = if profile_mode {
            check_lengths(&mut pairs, |l| profile.e_scanner(l))
        } else {
            check_lengths(&mut pairs, |_| gate)
        };
        let post_limit = if profile_mode { profile.a_mm.max(1e-6) } else { gate };
        let over = pairs.iter().filter(|p| p.error.abs() > p.limit).count();
        let mut rep = Report::new("Length Check");
        rep.metric("pairs", Metric::new(pairs.len() as f64, "pairs"));
        rep.metric("longest_pair", Metric::new(stats.longest, "mm"));
        let mut max_err = Metric::new(stats.max_abs_error, "mm");
        if !profile_mode {
            max_err = max_err.with_limit(gate, true);
        }
        rep.metric("max_abs_error", max_err);
        rep.metric("error_ratio", Metric::new(stats.error_ratio, "-").with_limit(1.0, true));
        rep.metric("pairs_over_limit", Metric::new(over as f64, "pairs").with_limit(0.0, true));
        rep.metric("b_hat_ppm", Metric::new(stats.b_hat_ppm, "ppm"));
        rep.metric(
            "post_scale_residual",
            Metric::new(stats.post_scale_residual, "mm").with_limit(post_limit, true),
        );
        if skipped > 0 {
            rep.note(Severity::Warning, format!("{skipped} reference rows skipped: unknown marker id or non-positive length"));
        }
        if profile_mode && stats.b_hat_ppm.abs() > profile.drift_ppm() {
            rep.note(
                Severity::Warning,
                format!(
                    "measured scale error {:.1} ppm exceeds the profile's {:.1} ppm; apply a scale correction before judging the scanner",
                    stats.b_hat_ppm,
                    profile.drift_ppm()
                ),
            );
        }
        if stats.error_ratio > 1.0 || stats.post_scale_residual > post_limit {
            rep.note(Severity::Warning, "length check failed: see error_ratio and post_scale_residual");
        }
        let rows: Vec<Value> = pairs
            .iter()
            .map(|p| {
                json!({
                    "a": p.a, "b": p.b, "reference_mm": p.reference, "measured_mm": p.measured,
                    "error_mm": p.error, "limit_mm": p.limit, "within": p.error.abs() <= p.limit,
                })
            })
            .collect();
        Ok(vec![
            Data::report(rep),
            Data::json(json!({ "b_hat_ppm": stats.b_hat_ppm, "rows": rows })),
        ])
    }
}

// ---------------------------------------------------------------------------
// Scan quality
// ---------------------------------------------------------------------------

/// Statistics of a scan's local sampling.
#[derive(Debug, Clone, PartialEq)]
pub struct QualityStats {
    pub points: usize,
    pub sampled: usize,
    pub fitted: usize,
    /// Median nearest-neighbour distance, mm.
    pub spacing: f64,
    /// Neighbourhood radius used for the local plane fits, mm.
    pub radius: f64,
    /// Points per mm^2 of surface.
    pub density: f64,
    /// RMS deviation from locally fitted planes, mm (dof-corrected).
    pub noise_rms: f64,
    pub noise_median: f64,
    /// Fraction of sampled points sitting at a boundary (hole or edge).
    pub boundary_fraction: f64,
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    let m = v.len() / 2;
    if v.len() % 2 == 1 {
        v[m]
    } else {
        0.5 * (v[m - 1] + v[m])
    }
}

fn sample_indices(n: usize, max: usize) -> Vec<usize> {
    if n <= max {
        (0..n).collect()
    } else {
        (0..max).map(|k| k * n / max).collect()
    }
}

/// Largest angular gap (radians) of the neighbours around `p` in the plane
/// with normal `n`.
fn max_angular_gap(points: &[DVec3], p: DVec3, n: DVec3, neigh: &[u32], me: u32) -> f64 {
    let helper = if n.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
    let u = n.cross(helper).normalize_or_zero();
    let v = n.cross(u);
    let mut ang: Vec<f64> = neigh
        .iter()
        .filter(|&&i| i != me)
        .map(|&i| {
            let d = points[i as usize] - p;
            d.dot(v).atan2(d.dot(u))
        })
        .collect();
    if ang.len() < 3 {
        return std::f64::consts::TAU;
    }
    ang.sort_by(f64::total_cmp);
    let mut gap = ang[0] + std::f64::consts::TAU - ang[ang.len() - 1];
    for w in ang.windows(2) {
        gap = gap.max(w[1] - w[0]);
    }
    gap
}

/// Estimate density, local-plane noise and boundary fraction of a point
/// cloud. `radius <= 0` picks the smallest radius that gives a comfortable
/// neighbour count. At most `max_samples` evenly strided points are examined.
pub fn scan_quality(
    points: &[DVec3],
    radius: f64,
    min_neighbors: usize,
    hole_gap: f64,
    max_samples: usize,
) -> Option<QualityStats> {
    let n = points.len();
    if n < 8 {
        return None;
    }
    let samples = sample_indices(n, max_samples.max(1));
    let diag = {
        let mut b = vernon_core::geometry::Aabb::empty();
        points.iter().for_each(|p| b.grow(*p));
        b.size().length()
    };
    // Median nearest-neighbour spacing from a first coarse grid.
    let cell0 = (diag / (n as f64).sqrt()).max(1e-9);
    let grid0 = Grid::build(points, cell0);
    let mut nn = Vec::new();
    let mut buf = Vec::new();
    for &i in samples.iter().step_by((samples.len() / 2000).max(1)) {
        let mut r = 2.0 * cell0;
        for _ in 0..8 {
            buf.clear();
            grid0.radius_query(points, points[i], r, &mut buf);
            let best = buf
                .iter()
                .filter(|&&j| j as usize != i)
                .map(|&j| (points[j as usize] - points[i]).length())
                .fold(f64::INFINITY, f64::min);
            if best.is_finite() {
                nn.push(best);
                break;
            }
            r *= 2.0;
        }
    }
    let spacing = median(&mut nn);
    if spacing <= 0.0 {
        return None;
    }
    // Neighbourhood radius.
    let wanted = (min_neighbors as f64 * 1.5).max(8.0);
    let mut r = if radius > 0.0 { radius } else { 3.0 * spacing };
    let hole = hole_gap.max(r);
    let grid = Grid::build(points, hole.max(r));
    if radius <= 0.0 {
        let probe: Vec<usize> = samples.iter().copied().step_by((samples.len() / 200).max(1)).collect();
        for _ in 0..12 {
            let mut counts: Vec<f64> = probe
                .iter()
                .map(|&i| {
                    buf.clear();
                    grid.radius_query(points, points[i], r, &mut buf);
                    buf.len() as f64
                })
                .collect();
            if median(&mut counts) >= wanted || r >= hole {
                break;
            }
            r *= 1.3;
        }
    }
    let mut noise2 = Vec::new();
    let mut dens = Vec::new();
    let mut boundary = 0usize;
    let mut fitted = 0usize;
    let mut hbuf = Vec::new();
    for &i in &samples {
        buf.clear();
        grid.radius_query(points, points[i], r, &mut buf);
        if buf.len() < min_neighbors.max(4) {
            continue;
        }
        let Some((normal, ev)) = plane_fit(points, &buf) else {
            continue;
        };
        let m = buf.len() as f64;
        noise2.push((ev[0] / (m - 3.0).max(1.0)).max(0.0));
        dens.push(m / (std::f64::consts::PI * r * r));
        fitted += 1;
        hbuf.clear();
        grid.radius_query(points, points[i], hole, &mut hbuf);
        if max_angular_gap(points, points[i], normal, &hbuf, i as u32) >= 150f64.to_radians() {
            boundary += 1;
        }
    }
    if fitted == 0 {
        return None;
    }
    let mean_n2 = noise2.iter().sum::<f64>() / fitted as f64;
    let mut sig: Vec<f64> = noise2.iter().map(|v| v.sqrt()).collect();
    Some(QualityStats {
        points: n,
        sampled: samples.len(),
        fitted,
        spacing,
        radius: r,
        density: median(&mut dens),
        noise_rms: mean_n2.sqrt(),
        noise_median: median(&mut sig),
        boundary_fraction: boundary as f64 / fitted as f64,
    })
}

/// Density, noise and boundary statistics of a scan.
pub struct ScanQuality;

impl NodeOp for ScanQuality {
    fn info(&self) -> NodeTypeInfo {
        base("capture.scan_quality", "Scan Quality")
            .input(PortSpec::new("geometry", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("radius", "Neighbourhood radius", 0.0, 0.0, 1000.0).unit("mm").help("Radius of the local plane fits; 0 chooses it from the point spacing."))
            .param(ParamSpec::int("min_neighbors", "Minimum neighbours", 12, 4, 500))
            .param(ParamSpec::float("hole_gap", "Hole gap", 5.0, 0.1, 1000.0).unit("mm").help("Radius used to look for one-sided neighbourhoods (holes and edges)."))
            .param(ParamSpec::int("max_samples", "Maximum samples", 20000, 100, 10_000_000).help("Points examined, evenly strided through the cloud."))
            .help("Measures point spacing, surface density, scanner noise (deviation from locally fitted planes) and the fraction of points on a boundary, plus the bounding box. The boundary fraction is an approximate hole indicator that also counts the outer edge.")
            .emulates("scan quality statistics: density, noise and hole indicators")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inputs, 0)?;
        let radius = params.float("radius")?;
        util::require_non_negative(ctx, "radius", radius)?;
        let hole = params.float("hole_gap")?;
        util::require_positive(ctx, "hole_gap", hole)?;
        let stats = scan_quality(
            &geo.points,
            radius,
            params.int("min_neighbors")?.max(4) as usize,
            hole,
            params.int("max_samples")?.max(1) as usize,
        );
        let mut rep = Report::new("Scan Quality");
        rep.metric("point_count", Metric::new(geo.point_count() as f64, "points"));
        util::bbox_metrics(&mut rep, &geo);
        match stats {
            Some(s) => {
                rep.metric("point_spacing", Metric::new(s.spacing, "mm"));
                rep.metric("neighborhood_radius", Metric::new(s.radius, "mm"));
                rep.metric("density", Metric::new(s.density, "1/mm2"));
                rep.metric("noise_rms", Metric::new(s.noise_rms, "mm"));
                rep.metric("noise_median", Metric::new(s.noise_median, "mm"));
                rep.metric("boundary_fraction", Metric::new(s.boundary_fraction * 100.0, "%"));
                rep.metric("fitted_points", Metric::new(s.fitted as f64, "points"));
                if (s.fitted as f64) < 0.5 * s.sampled as f64 {
                    rep.note(
                        Severity::Warning,
                        "fewer than half of the sampled points had enough neighbours; the cloud is sparse for this radius",
                    );
                }
                rep.note(
                    Severity::Info,
                    "boundary_fraction counts hole rims and the outer edge together; separating them is not done yet",
                );
            }
            None => {
                rep.note(
                    Severity::Warning,
                    "not enough points with enough neighbours to estimate density and noise; increase the radius or check the units",
                );
            }
        }
        if util::metric_flag(&geo) == Some(false) {
            rep.note(Severity::Warning, "geometry is marked non-metric: millimetre figures are not trustworthy until it is scaled");
        }
        Ok(vec![Data::report(rep)])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::Rng;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    fn ctx() -> CookContext {
        CookContext {
            node_name: "t".into(),
            work_dir: std::env::temp_dir(),
            frame: 0.0,
        }
    }

    fn params_for(op: &dyn NodeOp, set: &[(&str, vernon_graph::ParamValue)]) -> Params {
        let mut p = Params::from_specs(&op.info().params);
        for (k, v) in set {
            p.set(k, v.clone());
        }
        p
    }

    use vernon_graph::ParamValue as PV;

    #[test]
    fn spec_golden_1_to_4_scanner_error() {
        assert!(close(e_scanner(0.02, 0.05, 4500.0), 0.245, 1e-9));
        assert!(close(0.05 * 4.5, 0.225, 1e-12));
        assert!(close(e_scanner(0.04, 0.06, 4500.0), 0.310, 1e-9));
        assert!(close(e_scanner(0.1, 0.3, 4500.0), 1.450, 1e-9));
        assert!(close(e_scanner(0.1, 0.4, 4500.0), 1.900, 1e-9));
        assert!(close(e_scanner(0.025, 0.04, 4500.0), 0.205, 1e-9));
        assert!(close(e_scanner(0.02, 0.04, 4500.0), 0.200, 1e-9));
        // Paper check: 5 um + 5 um/m.
        assert!(close(e_scanner(0.005, 0.005, 4000.0), 0.025, 1e-9));
        assert!(close(e_scanner(0.005, 0.005, 13000.0), 0.070, 1e-9));
        assert!(close(0.012 / 3.0 * 13.0, 0.052, 1e-9));
    }

    #[test]
    fn spec_golden_5_6_scale_term_and_combination() {
        assert!(close(e_scale(4500.0, 0.5, 1000.0, 1), 2.25, 1e-9));
        assert!(close(e_scale(4500.0, 0.02, 1000.0, 1), 0.090, 1e-9));
        assert!(close(e_scale(4500.0, 0.02, 1000.0, 2), 0.0636396, 1e-6));
        let rss = combine(&[0.245, 0.090, 2.0 * 0.10], true);
        assert!(close(rss, 0.328, 5e-4), "{rss}");
        let lin = combine(&[0.245, 0.090, 0.2], false);
        assert!(close(lin, 0.535, 1e-9));
    }

    #[test]
    fn error_budget_verdict_flips_with_scale_bar_quality() {
        let op = ErrorBudget;
        let run = |sigma: f64| {
            let p = params_for(&op, &[("scale_bar_sigma", PV::Float(sigma))]);
            let out = op.cook(&ctx(), &[], &p).unwrap();
            out[0].as_report().unwrap().clone()
        };
        let good = run(0.02);
        let bad = run(0.5);
        assert!(good.passes(), "{good:?}");
        assert!(!bad.passes());
        assert!(close(good.metrics["e_scale"].value, 0.09, 1e-9));
        assert!(close(bad.metrics["e_scale"].value, 2.25, 1e-9));
        // The scale term can come from a scale report instead.
        let mut sc = Report::new("scale");
        sc.metric("scale_uncertainty_ppm", Metric::new(20.0, "ppm"));
        let out = op
            .cook(&ctx(), &[None, Some(Data::report(sc)), None], &params_for(&op, &[]))
            .unwrap();
        assert!(close(out[0].as_report().unwrap().metrics["e_scale"].value, 0.09, 1e-9));
    }

    #[test]
    fn drift_report_matches_profile_and_gate() {
        let op = DriftReport;
        let out = op
            .cook(&ctx(), &[], &params_for(&op, &[]))
            .unwrap();
        let rep = out[0].as_report().unwrap();
        assert!(close(rep.metrics["e_scanner"].value, 0.245, 1e-9));
        assert!(close(rep.metrics["drift_term"].value, 0.225, 1e-9));
        assert!(close(rep.metrics["drift_ppm"].value, 50.0, 1e-9));
        assert!(rep.passes());
        let ir = Profile::preset("fast_infrared_mode").unwrap();
        let out = op
            .cook(&ctx(), &[Some(Data::json(ir.to_json()))], &params_for(&op, &[]))
            .unwrap();
        let rep = out[0].as_report().unwrap();
        assert!(close(rep.metrics["e_scanner"].value, 1.45, 1e-9));
        assert!(!rep.passes());
        let rows = out[1].as_json().unwrap()["rows"].as_array().unwrap().len();
        assert!(rows >= 7);
    }

    #[test]
    fn profile_json_round_trip_and_presets() {
        for name in Profile::preset_names() {
            let p = Profile::preset(name).unwrap();
            assert_eq!(Profile::from_json(&p.to_json()).unwrap(), p);
        }
        assert!(Profile::preset("custom").is_none());
        assert!(Profile::from_json(&json!({"a_mm": -1.0, "b_mm_per_m": 0.1})).is_none());
        let op = ScannerProfile;
        let out = op
            .cook(&ctx(), &[], &params_for(&op, &[("preset", PV::Str("marker_assisted_laser".into()))]))
            .unwrap();
        let p = Profile::from_json(out[0].as_json().unwrap()).unwrap();
        assert!(close(p.a_mm, 0.04, 1e-12) && close(p.b_mm_per_m, 0.06, 1e-12));
    }

    /// Eight markers along a 4.5 m span, optionally stretched.
    fn marker_line(stretch_ppm: f64) -> (Geometry, Geometry) {
        let xs = [0.0, 300.0, 900.0, 1500.0, 2200.0, 3000.0, 3700.0, 4500.0];
        let mut reference = Geometry::from_points(xs.iter().map(|&x| DVec3::new(x, 0.0, 0.0)).collect());
        let mut measured = Geometry::from_points(
            xs.iter().map(|&x| DVec3::new(x * (1.0 + stretch_ppm * 1e-6), 0.0, 0.0)).collect(),
        );
        let ids: Vec<String> = (0..xs.len()).map(|i| format!("M{i}")).collect();
        for g in [&mut reference, &mut measured] {
            g.point_attribs.set("id", vernon_core::AttribArray::Str(ids.clone()));
        }
        (measured, reference)
    }

    #[test]
    fn spec_golden_12_length_check_scale_error_and_profile_gate() {
        let (measured, reference) = marker_line(60.0);
        let op = LengthCheck;
        let run = |profile: Profile| {
            let p = params_for(&op, &[("gate_mode", PV::Str("profile".into()))]);
            op.cook(
                &ctx(),
                &[
                    Some(Data::geometry(measured.clone())),
                    None,
                    Some(Data::geometry(reference.clone())),
                    Some(Data::json(profile.to_json())),
                ],
                &p,
            )
            .unwrap()
        };
        let mut weak = Profile::preset("photogrammetry_assisted_laser").unwrap();
        weak.a_mm = 0.02;
        weak.b_mm_per_m = 0.05;
        let out = run(weak);
        let rep = out[0].as_report().unwrap();
        assert!(close(rep.metrics["b_hat_ppm"].value, 60.0, 1e-6), "{:?}", rep.metrics["b_hat_ppm"]);
        assert!(close(rep.metrics["max_abs_error"].value, 0.27, 1e-6));
        assert!(rep.metrics["post_scale_residual"].value < 0.001);
        assert!(!rep.passes(), "b=0.05 limit 0.245 must fail");
        let mut strong = Profile::preset("marker_assisted_laser").unwrap();
        strong.a_mm = 0.04;
        strong.b_mm_per_m = 0.06;
        let out = run(strong);
        assert!(out[0].as_report().unwrap().passes(), "b=0.06 limit 0.31 must pass");
        assert_eq!(out[1].as_json().unwrap()["rows"].as_array().unwrap().len(), 28);
    }

    #[test]
    fn spec_golden_13_porsche_gate_fixed() {
        let mut rng = Rng::new(13);
        let xs: Vec<f64> = (0..8).map(|i| i as f64 * 4500.0 / 7.0).collect();
        let build = |drift: f64, rng: &mut Rng| {
            let mut reference = Geometry::from_points(xs.iter().map(|&x| DVec3::new(x, 0.0, 0.0)).collect());
            let mut measured = Geometry::from_points(
                xs.iter()
                    .map(|&x| {
                        DVec3::new(
                            x + drift * x / 4500.0 + (rng.next_f64() - 0.5) * 0.1,
                            (rng.next_f64() - 0.5) * 0.1,
                            (rng.next_f64() - 0.5) * 0.1,
                        )
                    })
                    .collect(),
            );
            let ids: Vec<String> = (0..8).map(|i| i.to_string()).collect();
            for g in [&mut reference, &mut measured] {
                g.point_attribs.set("id", vernon_core::AttribArray::Str(ids.clone()));
            }
            (measured, reference)
        };
        let op = LengthCheck;
        let run = |drift: f64, rng: &mut Rng| {
            let (m, r) = build(drift, rng);
            let out = op
                .cook(
                    &ctx(),
                    &[Some(Data::geometry(m)), None, Some(Data::geometry(r))],
                    &params_for(&op, &[]),
                )
                .unwrap();
            out[0].as_report().unwrap().clone()
        };
        assert!(run(0.5, &mut rng).passes());
        assert!(!run(1.2, &mut rng).passes());
    }

    #[test]
    fn length_check_needs_references_and_enough_pairs() {
        let (measured, _) = marker_line(0.0);
        let op = LengthCheck;
        assert!(op.cook(&ctx(), &[Some(Data::geometry(measured.clone()))], &params_for(&op, &[])).is_err());
        let p = params_for(&op, &[("lengths", PV::Str("M0, M1, 300".into()))]);
        assert!(op.cook(&ctx(), &[Some(Data::geometry(measured))], &p).is_err(), "one pair < min_pairs 3");
    }

    fn noisy_plane(n_side: usize, spacing: f64, sigma: f64, jitter: f64, seed: u64) -> Vec<DVec3> {
        let mut rng = Rng::new(seed);
        let mut pts = Vec::with_capacity(n_side * n_side);
        for i in 0..n_side {
            for j in 0..n_side {
                pts.push(DVec3::new(
                    i as f64 * spacing + (rng.next_f64() - 0.5) * jitter,
                    j as f64 * spacing + (rng.next_f64() - 0.5) * jitter,
                    sigma * rng.gaussian(),
                ));
            }
        }
        pts
    }

    #[test]
    fn spec_golden_18_noise_of_plane() {
        // 100k points, sigma 0.035 mm.
        let pts = noisy_plane(317, 1.0, 0.035, 0.6, 18);
        assert!(pts.len() > 100_000);
        let q = scan_quality(&pts, 0.0, 12, 5.0, 4000).unwrap();
        assert!((0.033..=0.037).contains(&q.noise_rms), "noise {}", q.noise_rms);
        assert!((q.density - 1.0).abs() < 0.2, "density {}", q.density);
        assert!((q.spacing - 0.8).abs() < 0.3, "spacing {}", q.spacing);
    }

    #[test]
    fn boundary_fraction_grows_when_a_hole_is_punched() {
        let full = noisy_plane(120, 1.0, 0.01, 0.2, 5);
        let holed: Vec<DVec3> = full
            .iter()
            .copied()
            .filter(|p| !((40.0..80.0).contains(&p.x) && (40.0..80.0).contains(&p.y)))
            .collect();
        let a = scan_quality(&full, 0.0, 12, 5.0, 20000).unwrap();
        let b = scan_quality(&holed, 0.0, 12, 5.0, 20000).unwrap();
        assert!(b.boundary_fraction > a.boundary_fraction + 0.02, "{} vs {}", b.boundary_fraction, a.boundary_fraction);
    }

    #[test]
    fn scan_quality_node_reports_and_handles_tiny_input() {
        let op = ScanQuality;
        let g = Geometry::from_points(noisy_plane(60, 1.0, 0.05, 0.2, 2));
        let out = op
            .cook(&ctx(), &[Some(Data::geometry(g))], &params_for(&op, &[]))
            .unwrap();
        let rep = out[0].as_report().unwrap();
        assert!(rep.metrics["noise_rms"].value > 0.03 && rep.metrics["noise_rms"].value < 0.08);
        assert!(rep.metrics.contains_key("bbox_diagonal"));
        let tiny = Geometry::from_points(vec![DVec3::ZERO, DVec3::X]);
        let out = op
            .cook(&ctx(), &[Some(Data::geometry(tiny))], &params_for(&op, &[]))
            .unwrap();
        assert!(out[0].as_report().unwrap().notes.iter().any(|(s, _)| *s == Severity::Warning));
        assert!(op.cook(&ctx(), &[None], &params_for(&op, &[])).is_err());
    }
}

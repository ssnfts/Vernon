//! Force-coefficient post-processing: reader for the solver's `.dat` histories,
//! window statistics, axle split, and the iteration-convergence gate.
//!
//! The reader discovers columns from the `#` header line (never by index) and
//! accepts the file-name variants different solver releases use.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use vernon_core::{Metric, Report, Severity};
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::air::air_in;
use crate::types::{ref_in, RefGeom, Series};
use crate::util::{fin, json_in, FnNode};

// ------------------------------------------------------------------ .dat reader

/// One parsed `.dat` file: header constants plus numeric columns.
#[derive(Debug, Clone, Default)]
pub struct Dat {
    pub meta: IndexMap<String, f64>,
    /// Column names without the leading time column.
    pub names: Vec<String>,
    pub time: Vec<f64>,
    /// One vector per entry of `names`.
    pub cols: Vec<Vec<f64>>,
}

impl Dat {
    #[cfg(test)]
    pub fn col(&self, name: &str) -> Option<&[f64]> {
        self.names.iter().position(|n| n == name).map(|i| self.cols[i].as_slice())
    }
}

/// `(total_x` -> `total_x`, `total_z)` -> `total_z`, but `Cl(f)` stays whole.
fn clean_token(t: &str) -> String {
    let mut s = t.trim_start_matches('(').to_string();
    while s.ends_with(')') && !s.contains('(') {
        s.pop();
    }
    s
}

fn parse_meta_line(body: &str, meta: &mut IndexMap<String, f64>) {
    let Some((key, val)) = body.split_once(':') else { return };
    let key = key.trim();
    if key.is_empty() || key.contains(char::is_whitespace) {
        return;
    }
    let vals: Vec<f64> = val
        .replace(['(', ')'], " ")
        .split_whitespace()
        .filter_map(|t| t.parse::<f64>().ok())
        .collect();
    match vals.as_slice() {
        [v] => {
            meta.insert(key.to_string(), *v);
        }
        [x, y, z] => {
            meta.insert(format!("{key}_x"), *x);
            meta.insert(format!("{key}_y"), *y);
            meta.insert(format!("{key}_z"), *z);
        }
        _ => {}
    }
}

/// Parse a solver history file. Rows whose width differs from the first data
/// row (a half-written last line of a running solver) are skipped; tokens that
/// are not numbers (solver names in the info file) become NaN.
pub fn parse_dat(text: &str) -> Dat {
    let mut meta = IndexMap::new();
    let mut header: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<f64>> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(body) = line.strip_prefix('#') {
            let body = body.trim();
            if body.contains(':') {
                parse_meta_line(body, &mut meta);
            } else if body.split_whitespace().next().is_some_and(|t| t.eq_ignore_ascii_case("time")) {
                header = body.split_whitespace().map(clean_token).filter(|t| !t.is_empty()).collect();
            }
            continue;
        }
        let vals: Vec<f64> = line
            .replace(['(', ')'], " ")
            .split_whitespace()
            .map(|t| t.parse::<f64>().unwrap_or(f64::NAN))
            .collect();
        if vals.first().is_some_and(|t| t.is_finite()) {
            if let Some(first) = rows.first() {
                if first.len() != vals.len() {
                    continue;
                }
            }
            rows.push(vals);
        }
    }
    let width = rows.first().map_or(0, Vec::len);
    let mut names: Vec<String> = header.into_iter().skip(1).collect();
    if names.len() + 1 != width {
        names = (1..width).map(|i| format!("c{i}")).collect();
    }
    let time = rows.iter().map(|r| r[0]).collect();
    let cols = (1..width).map(|j| rows.iter().map(|r| r[j]).collect()).collect();
    Dat { meta, names, time, cols }
}

/// Numeric sub-directories of `base`, sorted by time.
fn time_dirs(base: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(base) else { return vec![] };
    let mut v: Vec<(f64, PathBuf)> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let t = e.file_name().to_str()?.parse::<f64>().ok()?;
            e.path().is_dir().then(|| (t, e.path()))
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().map(|x| x.1).collect()
}

/// Join restart segments: a later segment replaces every earlier row at or
/// after its first time.
pub fn merge_restarts(parts: Vec<Dat>) -> Option<Dat> {
    let mut it = parts.into_iter();
    let mut out = it.next()?;
    for next in it {
        let Some(&t0) = next.time.first() else { continue };
        if next.names != out.names {
            continue;
        }
        let keep = out.time.iter().position(|&t| t >= t0).unwrap_or(out.time.len());
        out.time.truncate(keep);
        for c in &mut out.cols {
            c.truncate(keep);
        }
        out.time.extend(next.time);
        for (c, n) in out.cols.iter_mut().zip(next.cols) {
            c.extend(n);
        }
        out.meta.extend(next.meta);
    }
    Some(out)
}

/// Load `postProcessing/<object>/<time>/<file>` over all restart directories.
fn load_object(case: &Path, objects: &[&str], files: &[&str]) -> Option<Dat> {
    for obj in objects {
        let parts: Vec<Dat> = time_dirs(&case.join("postProcessing").join(obj))
            .iter()
            .filter_map(|dir| files.iter().map(|f| dir.join(f)).find(|p| p.is_file()))
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .map(|t| parse_dat(&t))
            .filter(|d| !d.time.is_empty())
            .collect();
        if let Some(d) = merge_restarts(parts) {
            return Some(d);
        }
    }
    None
}

/// Named columns on one time axis.
struct Src {
    time: Vec<f64>,
    cols: Vec<(String, Vec<f64>)>,
    meta: IndexMap<String, f64>,
}

fn coeff_src(d: Dat) -> Src {
    let cols = d.names.iter().cloned().zip(d.cols).collect();
    Src { time: d.time, cols, meta: d.meta }
}

/// Force or moment file: `total_x` -> `Fx`, `pressure_y` -> `Fy_pressure`...
fn vector_src(d: Dat, prefix: &str) -> Src {
    let mut cols = Vec::new();
    let mut named = false;
    for (name, col) in d.names.iter().zip(&d.cols) {
        if let Some((kind, axis)) = name.rsplit_once('_') {
            if matches!(axis, "x" | "y" | "z") && matches!(kind, "total" | "pressure" | "viscous" | "porous") {
                named = true;
                let key = if kind == "total" { format!("{prefix}{axis}") } else { format!("{prefix}{axis}_{kind}") };
                cols.push((key, col.clone()));
            }
        }
    }
    if !named && d.cols.len() >= 3 {
        // unknown header layout: the first three numbers are the totals
        for (axis, col) in ["x", "y", "z"].iter().zip(&d.cols) {
            cols.push((format!("{prefix}{axis}"), col.clone()));
        }
    }
    Src { time: d.time, cols, meta: d.meta }
}

fn canonical_field(f: &str) -> &str {
    match f {
        "Ux" | "Uy" | "Uz" => "U",
        o => o,
    }
}

/// Initial residuals per field (`res_p`, `res_U` = max over components...).
fn residual_src(d: Dat) -> Src {
    let solver_info = d.names.iter().any(|n| n.ends_with("_initial"));
    let mut fields: IndexMap<String, Vec<f64>> = IndexMap::new();
    for (name, col) in d.names.iter().zip(&d.cols) {
        let field = if solver_info {
            match name.strip_suffix("_initial") {
                Some(f) => f,
                None => continue,
            }
        } else {
            name.as_str()
        };
        let key = format!("res_{}", canonical_field(field));
        match fields.get_mut(&key) {
            Some(existing) => {
                for (e, v) in existing.iter_mut().zip(col) {
                    *e = e.max(*v);
                }
            }
            None => {
                fields.insert(key, col.clone());
            }
        }
    }
    Src { time: d.time, cols: fields.into_iter().collect(), meta: IndexMap::new() }
}

/// Read every available force, moment, coefficient and residual history of a
/// case into one series on their common time axis. An empty series (marked
/// `placeholder`) means the solver has not produced anything yet.
pub fn load_case_series(case: &Path) -> Series {
    let coeffs = load_object(case, &["forceCoeffs1", "forceCoeffs"], &["coefficient.dat", "forceCoeffs.dat"]);
    let force = load_object(case, &["forces1", "forces"], &["force.dat", "forces.dat"]);
    let moment = load_object(case, &["forces1", "forces"], &["moment.dat"]);
    let resid = load_object(case, &["solverInfo1", "solverInfo"], &["solverInfo.dat"])
        .or_else(|| load_object(case, &["residuals1", "residuals"], &["residuals.dat"]));
    let mut srcs: Vec<Src> = Vec::new();
    srcs.extend(coeffs.map(coeff_src));
    srcs.extend(force.map(|d| vector_src(d, "F")));
    srcs.extend(moment.map(|d| vector_src(d, "M")));
    srcs.extend(resid.map(residual_src));
    series_from_sources(srcs)
}

fn series_from_sources(mut srcs: Vec<Src>) -> Series {
    srcs.retain(|s| !s.time.is_empty());
    if srcs.is_empty() {
        return Series { placeholder: true, ..Series::default() };
    }
    let index: Vec<HashMap<u64, usize>> = srcs
        .iter()
        .map(|s| s.time.iter().enumerate().map(|(i, t)| (t.to_bits(), i)).collect())
        .collect();
    let mut common: Vec<f64> = srcs[0].time.clone();
    for idx in &index[1..] {
        common.retain(|t| idx.contains_key(&t.to_bits()));
    }
    if common.is_empty() {
        // sources on disjoint time axes: keep the first one alone
        srcs.truncate(1);
        common = srcs[0].time.clone();
    }
    let mut seen: HashSet<u64> = HashSet::new();
    common.retain(|t| seen.insert(t.to_bits()));
    let mut out = Series { time: common.clone(), ..Series::default() };
    for (src, idx) in srcs.iter().zip(&index) {
        out.meta.extend(src.meta.iter().map(|(k, v)| (k.clone(), *v)));
        for (name, col) in &src.cols {
            let vals: Vec<f64> = common.iter().filter_map(|t| idx.get(&t.to_bits()).map(|&i| col[i])).collect();
            if vals.len() == common.len() && vals.iter().all(|v| v.is_finite()) {
                out.columns.entry(name.clone()).or_insert(vals);
            }
        }
    }
    out
}

// ------------------------------------------------------------------ statistics and physics

/// Statistics over an averaging window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub n: usize,
    pub mean: f64,
    /// Sample standard deviation.
    pub sd: f64,
    /// Linear drift over the window as a percentage of |mean| (slope x n / |mean|).
    pub drift_pct: f64,
    pub p05: f64,
    pub p95: f64,
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    let pos = q * (sorted.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

/// Mean, standard deviation, drift and the 5-95% band. `None` for empty or
/// non-finite input.
pub fn window_stats(v: &[f64]) -> Option<Stats> {
    let n = v.len();
    if n == 0 || v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let nf = n as f64;
    let mean = v.iter().sum::<f64>() / nf;
    let var = if n > 1 { v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (nf - 1.0) } else { 0.0 };
    let centre = (nf - 1.0) / 2.0;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (i, x) in v.iter().enumerate() {
        let di = i as f64 - centre;
        sxy += di * (x - mean);
        sxx += di * di;
    }
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let drift_pct = if mean.abs() > 0.0 { slope * nf / mean.abs() * 100.0 } else { 0.0 };
    let mut sorted = v.to_vec();
    sorted.sort_by(f64::total_cmp);
    Some(Stats { n, mean, sd: var.sqrt(), drift_pct, p05: percentile(&sorted, 0.05), p95: percentile(&sorted, 0.95) })
}

/// Coefficient from a force: `F / (q Aref)`.
pub fn coefficient_from_force(force_n: f64, q_pa: f64, aref_m2: f64) -> f64 {
    force_n / (q_pa * aref_m2)
}

/// Split the vertical force over the axles from the pitch moment about
/// `x_c`: `Ff + Fr = Fz` and `Ff (xf - xc) + Fr (xr - xc) = -My`
/// (`My = -sum x_i F_i`). `None` when the axles coincide.
pub fn axle_split(fz: f64, my: f64, x_c: f64, x_f: f64, x_r: f64) -> Option<(f64, f64)> {
    let det = x_f - x_r;
    if det.abs() < 1e-9 {
        return None;
    }
    let ff = (-my - fz * (x_r - x_c)) / det;
    Some((ff, fz - ff))
}

/// Front share of the vertical load in percent; `None` when the axle forces
/// have opposite signs (some downforce, some lift) or the total is zero.
pub fn balance_front_pct(ff: f64, fr: f64) -> Option<f64> {
    let total = ff + fr;
    (ff * fr >= 0.0 && total.abs() > 1e-9).then(|| ff / total * 100.0)
}

/// Force-and-moment result of one run: the hand-off to convergence, mesh
/// study, A/B comparison and site loads. Coefficients refer to the FULL car.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ForceResult {
    /// True when no solver data existed; every number is then zero and meaningless.
    pub placeholder: bool,
    /// `forces` (derived from force/moment histories) or `coefficients`.
    pub source: String,
    pub u_ms: f64,
    pub rho: f64,
    pub q_pa: f64,
    pub a_front_m2: f64,
    /// Reference area the coefficients refer to (half of the full area for a half model).
    pub aref_m2: f64,
    pub half_model: bool,
    pub cd: f64,
    pub cd_sd: f64,
    pub cl: f64,
    pub cl_sd: f64,
    pub cda: f64,
    pub cda_sd: f64,
    pub cla: f64,
    pub cla_sd: f64,
    pub cd_drift_pct: f64,
    pub cd_p05: f64,
    pub cd_p95: f64,
    /// Full-car axle vertical forces, N (negative = downforce).
    pub front_force_n: Option<f64>,
    pub rear_force_n: Option<f64>,
    pub balance_front_pct: Option<f64>,
    pub balance_sd_pct: Option<f64>,
    pub window: usize,
    pub samples: usize,
    pub last_iter: f64,
}

impl ForceResult {
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn from_json(v: &Value) -> CookResult<Self> {
        serde_json::from_value(v.clone()).map_err(|e| CookError::Other(format!("not a force result: {e}")))
    }

    /// Value of a named quantity (`cd`, `cl`, `cda`, `cla`) and its iteration spread.
    pub fn quantity(&self, name: &str) -> Option<(f64, f64)> {
        match name {
            "cd" => Some((self.cd, self.cd_sd)),
            "cl" => Some((self.cl, self.cl_sd)),
            "cda" => Some((self.cda, self.cda_sd)),
            "cla" => Some((self.cla, self.cla_sd)),
            _ => None,
        }
    }
}

/// A force result input at `idx`; an absent input is a clean error.
pub(crate) fn force_in(inputs: &[Option<Data>], idx: usize, name: &str) -> CookResult<ForceResult> {
    match json_in(inputs, idx)? {
        Some(v) => ForceResult::from_json(&v),
        None => Err(CookError::MissingInput { index: idx, name: name.into() }),
    }
}

/// Settings for [`compute_forces`].
pub struct ForceSpec<'a> {
    pub rf: &'a RefGeom,
    pub half_model: bool,
    pub window: usize,
    pub rho: f64,
    /// Speed from a wired air state, preferred over everything else.
    pub u_wire: Option<f64>,
    pub u_param: f64,
    /// Centre of rotation override for the axle split.
    pub cofr_x: Option<f64>,
}

/// Result plus the notes the node should show.
pub struct Computed {
    pub result: ForceResult,
    pub notes: Vec<(Severity, String)>,
    /// |file Aref / expected Aref - 1| in percent (coefficient source only).
    pub aref_mismatch_pct: f64,
}

fn tail(v: &[f64], w: usize) -> &[f64] {
    &v[v.len() - w..]
}

/// Cd, Cl, CdA, ClA and axle balance over the last `window` iterations.
pub fn compute_forces(series: &Series, s: &ForceSpec) -> Result<Computed, String> {
    let n = series.len();
    if n == 0 || series.placeholder {
        return Err("no solver history".into());
    }
    let mut notes = Vec::new();
    let w = s.window.min(n).max(1);
    if w < s.window {
        notes.push((Severity::Warning, format!("history has {n} samples, shorter than the {} sample window; averaging all of it", s.window)));
    }
    let meta_u = series.meta.get("magUInf").copied().filter(|v| *v > 0.0);
    let u = s.u_wire.or(meta_u).unwrap_or(s.u_param);
    if let (Some(a), Some(b)) = (s.u_wire, meta_u) {
        if (a / b - 1.0).abs() > 0.01 {
            notes.push((Severity::Warning, format!("wired speed {a:.2} m/s differs from the speed in the solver history ({b:.2} m/s)")));
        }
    }
    let a_full = s.rf.a_front_m2;
    let aref = s.rf.aref_m2(s.half_model);
    if !(a_full > 0.0 && u > 0.0 && s.rho > 0.0) {
        return Err("reference area, speed and density must be positive".into());
    }
    let q = 0.5 * s.rho * u * u;
    let scale = if s.half_model { 2.0 } else { 1.0 };
    let mut aref_mismatch_pct = 0.0;
    let (source, cd_i, cl_i): (&str, Vec<f64>, Vec<f64>) = if let (Some(fx), Some(fz)) = (series.col("Fx"), series.col("Fz")) {
        (
            "forces",
            fx.iter().map(|f| coefficient_from_force(scale * f, q, a_full)).collect(),
            fz.iter().map(|f| coefficient_from_force(scale * f, q, a_full)).collect(),
        )
    } else if let (Some(cd), Some(cl)) = (series.col("Cd"), series.col("Cl")) {
        let mut ratio = 1.0;
        match series.meta.get("Aref") {
            Some(&a_file) if a_file > 0.0 => {
                ratio = a_file / aref;
                aref_mismatch_pct = (ratio - 1.0).abs() * 100.0;
                if aref_mismatch_pct > 1.0 {
                    notes.push((
                        Severity::Error,
                        format!(
                            "reference area in the solver history ({a_file:.4} m2) differs from the expected {aref:.4} m2 (half model: A_front / 2); coefficients were rescaled"
                        ),
                    ));
                } else {
                    ratio = 1.0;
                }
            }
            _ => notes.push((Severity::Warning, "reference area not recorded in the solver history; assuming it matches the vehicle reference".into())),
        }
        ("coefficients", cd.iter().map(|c| c * ratio).collect(), cl.iter().map(|c| c * ratio).collect())
    } else {
        return Err("history has neither Fx/Fz nor Cd/Cl columns".into());
    };

    let cd = window_stats(tail(&cd_i, w)).ok_or("non-finite drag history")?;
    let cl = window_stats(tail(&cl_i, w)).ok_or("non-finite lift history")?;
    let mut res = ForceResult {
        placeholder: false,
        source: source.into(),
        u_ms: u,
        rho: s.rho,
        q_pa: q,
        a_front_m2: a_full,
        aref_m2: aref,
        half_model: s.half_model,
        cd: cd.mean,
        cd_sd: cd.sd,
        cl: cl.mean,
        cl_sd: cl.sd,
        cda: cd.mean * a_full,
        cda_sd: cd.sd * a_full,
        cla: cl.mean * a_full,
        cla_sd: cl.sd * a_full,
        cd_drift_pct: cd.drift_pct,
        cd_p05: cd.p05,
        cd_p95: cd.p95,
        window: w,
        samples: n,
        last_iter: series.time[n - 1],
        ..ForceResult::default()
    };

    // axle split
    let x_c = s.cofr_x.or_else(|| series.meta.get("CofR_x").copied()).unwrap_or_else(|| s.rf.mid_wheelbase_x());
    if let (Some(fz), Some(my)) = (series.col("Fz"), series.col("My")) {
        let (mut ff_v, mut fr_v, mut bal_v) = (Vec::new(), Vec::new(), Vec::new());
        for (fz_i, my_i) in tail(fz, w).iter().zip(tail(my, w)) {
            if let Some((ff, fr)) = axle_split(scale * fz_i, scale * my_i, x_c, s.rf.x_front_axle_m, s.rf.x_rear_axle_m) {
                ff_v.push(ff);
                fr_v.push(fr);
                if let Some(b) = balance_front_pct(ff, fr) {
                    bal_v.push(b);
                }
            }
        }
        if let (Some(ff), Some(fr)) = (window_stats(&ff_v), window_stats(&fr_v)) {
            res.front_force_n = Some(ff.mean);
            res.rear_force_n = Some(fr.mean);
            match balance_front_pct(ff.mean, fr.mean) {
                Some(b) => {
                    res.balance_front_pct = Some(b);
                    res.balance_sd_pct = window_stats(&bal_v).map(|st| st.sd);
                }
                None => notes.push((Severity::Info, "front and rear vertical forces have opposite signs; balance is not defined, axle forces reported".into())),
            }
        }
    } else if let (Some(f), Some(r)) = (series.col("Cl(f)"), series.col("Cl(r)")) {
        if let (Some(fs), Some(rs)) = (window_stats(tail(f, w)), window_stats(tail(r, w))) {
            res.balance_front_pct = balance_front_pct(fs.mean, rs.mean);
            notes.push((Severity::Info, "axle split taken from the solver's front/rear lift coefficients".into()));
        }
    }
    Ok(Computed { result: res, notes, aref_mismatch_pct })
}

fn placeholder_result(s: &ForceSpec) -> ForceResult {
    ForceResult {
        placeholder: true,
        source: "none".into(),
        u_ms: s.u_wire.unwrap_or(s.u_param),
        rho: s.rho,
        a_front_m2: s.rf.a_front_m2,
        aref_m2: s.rf.aref_m2(s.half_model),
        half_model: s.half_model,
        ..ForceResult::default()
    }
}

// ------------------------------------------------------------------ cfd.force_coeffs

fn force_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.force_coeffs", "Force Coefficients", Stage::Cfd)
        .input(PortSpec::new("history", DataKind::Json))
        .input(PortSpec::optional("reference", DataKind::Json))
        .input(PortSpec::optional("air", DataKind::Json))
        .output(PortSpec::new("result", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::int("window", "Averaging window", 1000, 100, 2000).unit("iter").help("Last N iterations averaged; the spread over this window is the iteration uncertainty"))
        .param(ParamSpec::bool("half_model", "Half-car forces", true).help("Forces were integrated over the y >= 0 half only; the reference area is then half the full frontal area"))
        .param(ParamSpec::float("rho_inf", "Density of the run", 1.225, 0.9, 1.3).unit("kg/m3").help("Overridden by a wired air state"))
        .param(ParamSpec::float("u_ms", "Speed of the run", 30.0, 10.0, 100.0).unit("m/s").help("Overridden by a wired air state; otherwise the speed recorded in the history is used"))
        .param(ParamSpec::bool("custom_cofr", "Custom centre of rotation", false).help("Off: the centre of rotation recorded by the solver, else mid-wheelbase"))
        .param(ParamSpec::float("cofr_x_m", "Centre of rotation x", 0.0, -10.0, 20.0).unit("m"))
        .help("Drag and lift coefficients, CdA and ClA, front/rear axle split and aero balance with their iteration spread, from the solver's force histories. Checks the reference area so a half-car run can never be normalised by the full area.")
        .emulates("automotive wind-tunnel service: drag and lift coefficients")
        .emulates("cloud CFD service: force post-processing")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

fn force_report(c: &Computed) -> Report {
    let r = &c.result;
    let mut rep = Report::new("Force coefficients");
    rep.metric("data_available", Metric::new(1.0, "-").with_limit(1.0, false))
        .metric("Cd", Metric::new(r.cd, "-"))
        .metric("Cl", Metric::new(r.cl, "-"))
        .metric("CdA", Metric::new(r.cda, "m2"))
        .metric("ClA", Metric::new(r.cla, "m2"))
        .metric("CdA_sd", Metric::new(r.cda_sd, "m2"))
        .metric("Cd_drift_pct", Metric::new(r.cd_drift_pct, "%"))
        .metric("aref_mismatch_pct", Metric::new(c.aref_mismatch_pct, "%").with_limit(1.0, true))
        .metric("window", Metric::new(r.window as f64, "iter"));
    if let Some(b) = r.balance_front_pct {
        rep.metric("balance_front_pct", Metric::new(b, "%"));
    }
    if let Some(f) = r.front_force_n {
        rep.metric("front_force", Metric::new(f, "N"));
    }
    if let Some(f) = r.rear_force_n {
        rep.metric("rear_force", Metric::new(f, "N"));
    }
    for (sev, text) in &c.notes {
        rep.note(*sev, text.clone());
    }
    rep.note(Severity::Info, "downforce is less trustworthy than drag in steady RANS; trust only A/B deltas on a validated recipe");
    rep
}

fn force_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let series = match json_in(ins, 0)? {
        Some(v) => Series::from_json(&v)?,
        None => return Err(CookError::MissingInput { index: 0, name: "history".into() }),
    };
    let (rf, _) = ref_in(ins, 1)?;
    let air = air_in(ins, 2)?;
    let rho = air.rho.unwrap_or(p.float("rho_inf")?);
    let spec = ForceSpec {
        rf: &rf,
        half_model: p.bool("half_model")?,
        window: p.int("window")?.max(1) as usize,
        rho,
        u_wire: air.u,
        u_param: p.float("u_ms")?,
        cofr_x: p.bool("custom_cofr")?.then(|| p.float("cofr_x_m")).transpose()?,
    };
    match compute_forces(&series, &spec) {
        Ok(c) => {
            fin(ctx, "CdA", c.result.cda)?;
            fin(ctx, "ClA", c.result.cla)?;
            Ok(vec![Data::json(c.result.to_json()), Data::report(force_report(&c))])
        }
        Err(e) if series.is_empty() || series.placeholder => {
            let mut rep = Report::new("Force coefficients");
            rep.metric("data_available", Metric::new(0.0, "-").with_limit(1.0, false));
            rep.note(Severity::Warning, format!("{e}: connect a solved case or a recorded force history; the numbers on this wire are not results"));
            Ok(vec![Data::json(placeholder_result(&spec).to_json()), Data::report(rep)])
        }
        Err(e) => Err(ctx.fail(e)),
    }
}

// ------------------------------------------------------------------ cfd.run_convergence

/// Thresholds of the iteration gate.
#[derive(Debug, Clone)]
pub struct ConvSpec {
    pub window: usize,
    pub tol_pct: f64,
    pub drift_limit_pct: f64,
    pub sd_limit_pct: f64,
    pub residual_target: f64,
    pub require_residuals: bool,
}

/// Outcome of the iteration gate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Convergence {
    pub converged: bool,
    pub enough_iterations: bool,
    pub iterations: usize,
    pub window: usize,
    pub mean_prev: f64,
    pub mean_last: f64,
    pub window_diff_pct: f64,
    pub drift_pct: f64,
    pub sd_pct: f64,
    pub unsteady: bool,
    /// RMS initial residual over the last window, per field.
    pub residuals: IndexMap<String, f64>,
    pub residuals_ok: bool,
    pub residuals_checked: bool,
}

fn pick_column<'a>(series: &'a Series, column: &str) -> Result<&'a [f64], String> {
    let order: &[&str] = match column {
        "auto" => &["Cd", "Fx"],
        "Cd" => &["Cd"],
        "Cl" => &["Cl"],
        "Fx" => &["Fx"],
        "Fz" => &["Fz"],
        _ => &[],
    };
    order
        .iter()
        .find_map(|name| series.col(name))
        .ok_or_else(|| format!("history has no '{column}' column"))
}

/// Iteration-stationarity gate (spec 3.9): residuals below target, two
/// adjacent windows agree, drift small. Scatter only raises the `unsteady` flag.
pub fn iteration_gate(series: &Series, column: &str, c: &ConvSpec) -> Result<Convergence, String> {
    let col = pick_column(series, column)?;
    let n = col.len();
    let w = c.window.max(2);
    let enough = n >= 2 * w;
    let eff = if enough { w } else { n / 2 };
    if eff < 2 {
        return Err("history too short to compare two windows".into());
    }
    let w2 = window_stats(&col[n - eff..]).ok_or("non-finite history")?;
    let w1 = window_stats(&col[n - 2 * eff..n - eff]).ok_or("non-finite history")?;
    let mean_ref = w2.mean.abs().max(f64::MIN_POSITIVE);
    let diff = (w2.mean - w1.mean).abs() / mean_ref * 100.0;
    let drift = w1.drift_pct.abs().max(w2.drift_pct.abs());
    let sd_pct = w2.sd / mean_ref * 100.0;
    let mut residuals = IndexMap::new();
    for (name, r) in &series.columns {
        if let Some(field) = name.strip_prefix("res_") {
            let last = &r[r.len() - eff.min(r.len())..];
            let rms = (last.iter().map(|x| x * x).sum::<f64>() / last.len() as f64).sqrt();
            residuals.insert(field.to_string(), rms);
        }
    }
    let residuals_checked = !residuals.is_empty();
    let residuals_ok = if residuals_checked { residuals.values().all(|r| *r <= c.residual_target) } else { !c.require_residuals };
    let converged = enough && residuals_ok && diff <= c.tol_pct && drift <= c.drift_limit_pct;
    Ok(Convergence {
        converged,
        enough_iterations: enough,
        iterations: n,
        window: eff,
        mean_prev: w1.mean,
        mean_last: w2.mean,
        window_diff_pct: diff,
        drift_pct: drift,
        sd_pct,
        unsteady: sd_pct > c.sd_limit_pct,
        residuals,
        residuals_ok,
        residuals_checked,
    })
}

fn conv_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.run_convergence", "Run Convergence", Stage::Cfd)
        .input(PortSpec::new("history", DataKind::Json))
        .output(PortSpec::new("status", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::choice("column", "Monitored quantity", "auto", &["auto", "Cd", "Cl", "Fx", "Fz"]).help("auto: drag coefficient, else drag force"))
        .param(ParamSpec::int("window", "Window", 1000, 100, 2000).unit("iter"))
        .param(ParamSpec::float("tol_pct", "Window-to-window change", 0.2, 0.01, 2.0).unit("%").help("Mean of the last window against the one before"))
        .param(ParamSpec::float("drift_limit_pct", "Drift per window", 0.1, 0.01, 2.0).unit("%"))
        .param(ParamSpec::float("sd_limit_pct", "Scatter flag level", 0.5, 0.05, 5.0).unit("%").help("Above this the result is flagged unsteady: report the spread, not just the mean"))
        .param(ParamSpec::float("residual_target", "Residual target", 1e-5, 1e-6, 1e-3).help("RMS initial residual of every field over the last window"))
        .param(ParamSpec::bool("require_residuals", "Require residual history", true).help("Without residuals convergence cannot be certified"))
        .help("Iteration stationarity gate: residuals below target, the last two averaging windows agree, drift is small. Large scatter flags the run as unsteady instead of failing it.")
        .emulates("automotive wind-tunnel service: convergence check")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

fn conv_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let series = match json_in(ins, 0)? {
        Some(v) => Series::from_json(&v)?,
        None => return Err(CookError::MissingInput { index: 0, name: "history".into() }),
    };
    let spec = ConvSpec {
        window: p.int("window")?.max(2) as usize,
        tol_pct: p.float("tol_pct")?,
        drift_limit_pct: p.float("drift_limit_pct")?,
        sd_limit_pct: p.float("sd_limit_pct")?,
        residual_target: p.float("residual_target")?,
        require_residuals: p.bool("require_residuals")?,
    };
    let mut rep = Report::new("Run convergence");
    if series.is_empty() || series.placeholder {
        rep.metric("converged", Metric::new(0.0, "-").with_limit(1.0, false));
        rep.note(Severity::Warning, "no solver history yet: nothing to certify");
        let st = Convergence::default();
        return Ok(vec![Data::json(serde_json::to_value(st).unwrap_or(Value::Null)), Data::report(rep)]);
    }
    let g = iteration_gate(&series, p.str("column")?, &spec).map_err(|e| ctx.fail(e))?;
    rep.metric("converged", Metric::new(f64::from(g.converged), "-").with_limit(1.0, false))
        .metric("enough_iterations", Metric::new(f64::from(g.enough_iterations), "-").with_limit(1.0, false))
        .metric("window_diff_pct", Metric::new(g.window_diff_pct, "%").with_limit(spec.tol_pct, true))
        .metric("drift_pct", Metric::new(g.drift_pct, "%").with_limit(spec.drift_limit_pct, true))
        .metric("sd_pct", Metric::new(g.sd_pct, "%"))
        .metric("iterations", Metric::new(g.iterations as f64, "iter"));
    for (field, r) in &g.residuals {
        rep.metric(format!("res_{field}"), Metric::new(*r, "-").with_limit(spec.residual_target, true));
    }
    if !g.enough_iterations {
        rep.note(Severity::Warning, format!("only {} iterations: fewer than two windows of {}", g.iterations, spec.window));
    }
    if !g.residuals_checked {
        let sev = if spec.require_residuals { Severity::Error } else { Severity::Warning };
        rep.note(sev, "no residual history in the series; run the case with the solver-information monitor");
    }
    if g.unsteady {
        rep.note(Severity::Warning, "unsteady: scatter is above the flag level; report mean +/- spread");
    }
    let status = json!({
        "converged": g.converged, "unsteady": g.unsteady, "iterations": g.iterations, "window": g.window,
        "window_diff_pct": g.window_diff_pct, "drift_pct": g.drift_pct, "sd_pct": g.sd_pct,
        "mean_last": g.mean_last, "residuals_ok": g.residuals_ok,
    });
    Ok(vec![Data::json(status), Data::report(rep)])
}

pub(crate) fn register(reg: &mut Registry) {
    reg.add(FnNode::new(force_info, force_cook));
    reg.add(FnNode::new(conv_info, conv_cook));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::*;
    use vernon_graph::ParamValue;

    const COEFF: &str = "# Force coefficients\n# magUInf      : 30\n# lRef         : 4.6\n# Aref         : 1.08\n# CofR         : (2.313 0 0)\n# liftDir      : (0 0 1)\n#\n# Time          Cd              Cd(f)           Cd(r)           Cl              Cl(f)           Cl(r)\n1 0.31 0.15 0.16 -0.2 -0.1 -0.1\n2 0.32 0.16 0.16 -0.21 -0.11 -0.10\n3 0.32";

    #[test]
    fn parses_coefficient_header_by_name() {
        let d = parse_dat(COEFF);
        assert_eq!(d.names, ["Cd", "Cd(f)", "Cd(r)", "Cl", "Cl(f)", "Cl(r)"]);
        assert_eq!(d.time, [1.0, 2.0]);
        assert_eq!(d.col("Cl").unwrap(), [-0.2, -0.21]);
        assert_eq!(d.meta["Aref"], 1.08);
        assert_eq!(d.meta["CofR_x"], 2.313);
        assert_eq!(d.meta["magUInf"], 30.0);
    }

    #[test]
    fn parses_grouped_force_header() {
        let t = "# Force\n# CofR : (0 0 0)\n# Time  (total_x total_y total_z) (pressure_x pressure_y pressure_z)\n1 ((10 0 -5) (8 0 -6))\n2 ((11 0 -5.5) (9 0 -6.5))\n";
        let d = parse_dat(t);
        assert_eq!(d.names, ["total_x", "total_y", "total_z", "pressure_x", "pressure_y", "pressure_z"]);
        let s = vector_src(d, "F");
        let names: Vec<_> = s.cols.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(names, ["Fx", "Fy", "Fz", "Fx_pressure", "Fy_pressure", "Fz_pressure"]);
        assert_eq!(s.cols[2].1, [-5.0, -5.5]);
    }

    #[test]
    fn unknown_header_falls_back_to_first_three_columns() {
        let d = parse_dat("# Time forces(pressure viscous)\n1 10 0 -5 8 0 -6\n2 11 0 -6 9 0 -7\n");
        let s = vector_src(d, "M");
        assert_eq!(s.cols[0].0, "Mx");
        assert_eq!(s.cols[1].1, [0.0, 0.0]);
        assert_eq!(s.cols[2].1, [-5.0, -6.0]);
    }

    #[test]
    fn statistics_of_a_ramp() {
        let v: Vec<f64> = (0..100).map(|i| 1.0 + 0.001 * f64::from(i)).collect();
        let s = window_stats(&v).unwrap();
        assert!((s.mean - 1.0495).abs() < 1e-12);
        // slope 0.001 per sample over 100 samples = 0.1 on a 1.0495 mean
        assert!((s.drift_pct - 100.0 * 0.1 / 1.0495).abs() < 1e-9, "{}", s.drift_pct);
        assert!(s.p05 < s.mean && s.p95 > s.mean);
        assert!(window_stats(&[]).is_none());
        assert!(window_stats(&[1.0, f64::NAN]).is_none());
        assert_eq!(window_stats(&[2.0]).unwrap().sd, 0.0);
    }

    #[test]
    fn golden_axle_split_g16_g17() {
        let (ff, fr) = axle_split(-856.5, 193.7, 0.0, 1.35, -1.35).unwrap();
        assert!((ff + 500.0).abs() < 0.2 && (fr + 356.5).abs() < 0.2, "{ff} {fr}");
        let b = balance_front_pct(ff, fr).unwrap();
        assert!((b - 58.4).abs() < 0.1, "{b}");
        let (ff, fr) = axle_split(-856.5, 0.0, 0.0, 1.35, -1.35).unwrap();
        assert!((ff + 428.2).abs() < 0.1 && (fr + 428.2).abs() < 0.1);
        assert!(axle_split(-1.0, 0.0, 0.0, 1.0, 1.0).is_none());
        assert!(balance_front_pct(-100.0, 20.0).is_none());
    }

    #[test]
    fn golden_half_model_reference_area_g18() {
        // half-model force 548 N at q 1713 Pa, full frontal area 2.0 m2
        assert!((coefficient_from_force(548.0, 1713.0, 1.0) - 0.320).abs() < 0.001);
        assert!((coefficient_from_force(548.0, 1713.0, 2.0) - 0.160).abs() < 0.001);
        let mut rf = RefGeom::reference_car();
        rf.a_front_m2 = 2.0;
        // history normalised by the FULL area although only the half was integrated
        let mut s = Series { time: vec![1.0, 2.0, 3.0], ..Series::default() };
        s.columns.insert("Cd".into(), vec![0.160; 3]);
        s.columns.insert("Cl".into(), vec![-0.1; 3]);
        s.meta.insert("Aref".into(), 2.0);
        let spec = ForceSpec { rf: &rf, half_model: true, window: 100, rho: 1.225, u_wire: None, u_param: 30.0, cofr_x: None };
        let c = compute_forces(&s, &spec).unwrap();
        assert!((c.aref_mismatch_pct - 100.0).abs() < 1e-9);
        assert!(c.notes.iter().any(|(sev, _)| *sev == Severity::Error));
        assert!((c.result.cd - 0.320).abs() < 1e-9, "rescaled to the half-model area: {}", c.result.cd);
        assert!(!force_report(&c).passes(), "the wrong area must fail the gate");
        // correct area passes
        s.meta.insert("Aref".into(), 1.0);
        s.columns.insert("Cd".into(), vec![0.320; 3]);
        let c = compute_forces(&s, &spec).unwrap();
        assert!(force_report(&c).passes());
        assert!((c.result.cda - 0.64).abs() < 1e-9);
    }

    fn force_series(n: usize, fx: f64, fz: f64, my: f64) -> Series {
        let mut s = Series { time: (1..=n).map(|i| i as f64).collect(), ..Series::default() };
        s.columns.insert("Fx".into(), (0..n).map(|i| fx * (1.0 + 0.001 * ((i % 3) as f64 - 1.0))).collect());
        s.columns.insert("Fz".into(), vec![fz; n]);
        s.columns.insert("My".into(), vec![my; n]);
        s
    }

    #[test]
    fn forces_to_coefficients_and_balance() {
        let mut rf = RefGeom::reference_car();
        rf.x_front_axle_m = 1.35;
        rf.x_rear_axle_m = -1.35;
        rf.a_front_m2 = 2.0;
        // half-model forces; q = 1713 Pa
        let mut s = force_series(300, 548.0, -428.25, 96.85);
        s.meta.insert("CofR_x".into(), 0.0);
        let q = 0.5 * 1.11 * (200.0f64 / 3.6).powi(2);
        let spec = ForceSpec { rf: &rf, half_model: true, window: 100, rho: 1.11, u_wire: Some(200.0 / 3.6), u_param: 30.0, cofr_x: None };
        let c = compute_forces(&s, &spec).unwrap().result;
        assert!((c.q_pa - q).abs() < 1e-9);
        assert!((c.cd - 2.0 * 548.0 / (q * 2.0)).abs() < 1e-3, "{}", c.cd);
        assert!((c.cda - 0.64).abs() < 0.002, "{}", c.cda);
        assert!((c.cla + 0.5).abs() < 0.002, "{}", c.cla);
        assert!(c.cd_sd > 0.0 && c.cda_sd > 0.0);
        assert!((c.balance_front_pct.unwrap() - 58.4).abs() < 0.1);
        assert!((c.front_force_n.unwrap() + 500.0).abs() < 0.3);
        assert_eq!(c.window, 100);
        assert_eq!(c.source, "forces");
        // the same history as a full-car run gives half the force coefficients
        let full = compute_forces(&s, &ForceSpec { half_model: false, ..spec }).unwrap().result;
        assert!((full.cd * 2.0 - c.cd).abs() < 1e-9);
    }

    #[test]
    fn load_series_from_case_directory_with_restart() {
        let case = tmp("coeffs-load");
        let mk = |sub: &str, file: &str, text: &str| {
            let d = case.join("postProcessing").join(sub);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(file), text).unwrap();
        };
        mk("forceCoeffs1/0", "coefficient.dat", "# Aref : 1.08\n# magUInf : 30\n# Time Cd Cl\n1 0.30 -0.1\n2 0.31 -0.1\n3 0.32 -0.1\n4 0.33 -0.1\n");
        // restart from iteration 3 replaces the earlier rows
        mk("forceCoeffs1/3", "forceCoeffs.dat", "# Time Cd Cl\n3 0.40 -0.2\n4 0.41 -0.2\n5 0.42 -0.2\n");
        mk("forces1/0", "force.dat", "# CofR : (1.5 0 0)\n# Time (total_x total_y total_z)\n1 (5 0 -3)\n2 (5 0 -3)\n3 (5 0 -3)\n4 (5 0 -3)\n5 (5 0 -3)\n");
        mk("solverInfo1/0", "solverInfo.dat", "# Time Ux_solver Ux_initial Ux_final Uy_initial p_solver p_initial\n1 smoothSolver 1e-2 1e-4 5e-3 GAMG 1e-1\n2 smoothSolver 1e-3 1e-4 4e-3 GAMG 1e-2\n3 smoothSolver 1e-4 1e-4 3e-3 GAMG 1e-3\n4 smoothSolver 1e-5 1e-4 2e-3 GAMG 1e-4\n5 smoothSolver 1e-5 1e-4 1e-3 GAMG 1e-5\n");
        let s = load_case_series(&case);
        assert_eq!(s.time, [1.0, 2.0, 3.0, 4.0, 5.0], "common time axis of all sources");
        assert_eq!(s.col("Cd").unwrap(), [0.30, 0.31, 0.40, 0.41, 0.42], "restart replaces rows from its first time on");
        assert_eq!(s.col("Fz").unwrap(), [-3.0; 5]);
        // U residual = worst component; solver-name columns are dropped
        assert_eq!(s.col("res_U").unwrap(), [1e-2, 4e-3, 3e-3, 2e-3, 1e-3]);
        assert_eq!(s.col("res_p").unwrap(), [1e-1, 1e-2, 1e-3, 1e-4, 1e-5]);
        assert!(s.col("Ux_solver").is_none());
        assert_eq!(s.meta["CofR_x"], 1.5);
        assert_eq!(s.meta["magUInf"], 30.0);
        assert!(!s.placeholder);
        assert!(load_case_series(&tmp("coeffs-empty")).placeholder);
    }

    #[test]
    fn force_coeffs_node_flags_missing_history_without_inventing_numbers() {
        let ctx = ctx("fc-empty");
        let p = Params::from_specs(&force_info().params);
        let empty = Series { placeholder: true, ..Series::default() };
        let out = force_cook(&ctx, &[Some(Data::json(empty.to_json()))], &p).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(!r.passes());
        let res = ForceResult::from_json(out[0].as_json().unwrap()).unwrap();
        assert!(res.placeholder && res.cda == 0.0);
        assert!(force_cook(&ctx, &[], &p).is_err());
        // history without usable columns is a clean error, not a panic
        let mut odd = Series { time: vec![1.0], ..Series::default() };
        odd.columns.insert("foo".into(), vec![1.0]);
        assert!(force_cook(&ctx, &[Some(Data::json(odd.to_json()))], &p).is_err());
    }

    #[test]
    fn force_coeffs_node_cooks_a_history() {
        let ctx = ctx("fc-run");
        let mut p = Params::from_specs(&force_info().params);
        p.set("window", ParamValue::Int(100));
        p.set("half_model", ParamValue::Bool(false));
        let s = force_series(250, 700.0, -450.0, 0.0);
        let out = force_cook(&ctx, &[Some(Data::json(s.to_json()))], &p).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(r.passes(), "{r:?}");
        let res = ForceResult::from_json(out[0].as_json().unwrap()).unwrap();
        assert!(res.cda > 0.0 && res.cla < 0.0 && !res.placeholder);
        assert!(r.metrics.contains_key("balance_front_pct"));
    }

    fn conv_series(n: usize, drift: f64, resid: f64) -> Series {
        let mut s = Series { time: (1..=n).map(|i| i as f64).collect(), ..Series::default() };
        s.columns.insert("Cd".into(), (0..n).map(|i| 0.3 + drift * i as f64 + 1e-5 * ((i % 4) as f64 - 1.5)).collect());
        s.columns.insert("res_p".into(), vec![resid; n]);
        s.columns.insert("res_U".into(), vec![resid * 0.5; n]);
        s
    }

    fn conv_spec(window: usize) -> ConvSpec {
        ConvSpec { window, tol_pct: 0.2, drift_limit_pct: 0.1, sd_limit_pct: 0.5, residual_target: 1e-5, require_residuals: true }
    }

    #[test]
    fn iteration_gate_passes_a_flat_converged_run() {
        let g = iteration_gate(&conv_series(400, 0.0, 5e-6), "auto", &conv_spec(100)).unwrap();
        assert!(g.converged && g.enough_iterations && !g.unsteady, "{g:?}");
        assert!(g.residuals["p"] <= 1e-5);
    }

    #[test]
    fn iteration_gate_fails_drift_residuals_and_short_runs() {
        // 0.0001 per iteration on 0.3: 100 samples drift by about 3%
        let g = iteration_gate(&conv_series(400, 1e-4, 5e-6), "auto", &conv_spec(100)).unwrap();
        assert!(!g.converged && g.window_diff_pct > 0.2 && g.drift_pct > 0.1, "{g:?}");
        let g = iteration_gate(&conv_series(400, 0.0, 5e-5), "auto", &conv_spec(100)).unwrap();
        assert!(!g.converged && !g.residuals_ok);
        let g = iteration_gate(&conv_series(150, 0.0, 5e-6), "auto", &conv_spec(100)).unwrap();
        assert!(!g.converged && !g.enough_iterations);
        assert!(iteration_gate(&conv_series(400, 0.0, 5e-6), "Fz", &conv_spec(100)).is_err());
        assert!(iteration_gate(&conv_series(3, 0.0, 5e-6), "auto", &conv_spec(100)).is_err());
    }

    #[test]
    fn scatter_flags_unsteady_without_failing() {
        let mut s = conv_series(400, 0.0, 5e-6);
        let cd = s.columns.get_mut("Cd").unwrap();
        for (i, v) in cd.iter_mut().enumerate() {
            *v = 0.3 + if i % 2 == 0 { 0.003 } else { -0.003 };
        }
        let g = iteration_gate(&s, "auto", &conv_spec(100)).unwrap();
        assert!(g.unsteady && g.converged, "{g:?}");
    }

    #[test]
    fn missing_residuals_fail_unless_waived() {
        let mut s = conv_series(400, 0.0, 5e-6);
        s.columns.retain(|k, _| !k.starts_with("res_"));
        let g = iteration_gate(&s, "auto", &conv_spec(100)).unwrap();
        assert!(!g.converged && !g.residuals_checked);
        let waived = ConvSpec { require_residuals: false, ..conv_spec(100) };
        assert!(iteration_gate(&s, "auto", &waived).unwrap().converged);
    }

    #[test]
    fn convergence_node_reports_limits() {
        let ctx = ctx("conv");
        let mut p = Params::from_specs(&conv_info().params);
        p.set("window", ParamValue::Int(100));
        let out = conv_cook(&ctx, &[Some(Data::json(conv_series(400, 0.0, 5e-6).to_json()))], &p).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(r.passes(), "{r:?}");
        assert_eq!(out[0].as_json().unwrap()["converged"], true);
        let out = conv_cook(&ctx, &[Some(Data::json(conv_series(400, 1e-4, 5e-6).to_json()))], &p).unwrap();
        assert!(!out[1].as_report().unwrap().passes());
        let none = Series { placeholder: true, ..Series::default() };
        let out = conv_cook(&ctx, &[Some(Data::json(none.to_json()))], &p).unwrap();
        assert!(!out[1].as_report().unwrap().passes());
        assert!(conv_cook(&ctx, &[], &p).is_err());
    }
}

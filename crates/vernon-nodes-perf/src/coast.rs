//! Coastdown regression: closed-form model, log parsing, run filtering, per-run fits
//! (integrated nonlinear least squares or 1 s acceleration regression), opposite-direction
//! pairing, uncertainty and minimum detectable change. Spec sections 2.4 and 3.8.

use std::collections::HashMap;

use serde_json::{json, Value};
use vernon_core::Severity;

use crate::physics::{air_density, G, R_DRY};
use crate::util::{lin_solve, mat_inv, mean, median, sd, t975};

// ---------------------------------------------------------------- closed form

/// Speed after `t` seconds of coasting from `v0`: `M_e dv/dt = -(A + B v^2)`.
pub fn coast_speed(a: f64, b: f64, me: f64, v0: f64, t: f64) -> f64 {
    if b < 1e-12 {
        return (v0 - a * t / me).max(0.0);
    }
    if a < 1e-12 {
        return v0 / (1.0 + b * v0 * t / me);
    }
    let k = (a / b).sqrt();
    let w = (a * b).sqrt() / me;
    let arg = (v0 / k).atan() - w * t;
    if arg <= 0.0 {
        0.0
    } else {
        k * arg.tan()
    }
}

/// Time to slow from `v1` to `v2` (`v1 > v2`).
pub fn coast_time(a: f64, b: f64, me: f64, v1: f64, v2: f64) -> f64 {
    if b < 1e-12 {
        me * (v1 - v2) / a
    } else if a < 1e-12 {
        me / b * (1.0 / v2 - 1.0 / v1)
    } else {
        let k = (a / b).sqrt();
        me / (a * b).sqrt() * ((v1 / k).atan() - (v2 / k).atan())
    }
}

/// Distance covered slowing from `v1` to `v2`.
pub fn coast_distance(a: f64, b: f64, me: f64, v1: f64, v2: f64) -> f64 {
    me / (2.0 * b) * ((a + b * v1 * v1) / (a + b * v2 * v2)).ln()
}

/// Deceleration at speed `v`.
pub fn coast_decel(a: f64, b: f64, me: f64, v: f64) -> f64 {
    (a + b * v * v) / me
}

/// Sample a noise-free coastdown from `v_start` down to `v_end` at `hz`.
pub fn simulate_log(a: f64, b: f64, me: f64, v_start: f64, v_end: f64, hz: f64) -> (Vec<f64>, Vec<f64>) {
    let t_end = coast_time(a, b, me, v_start, v_end);
    let n = (t_end * hz).floor() as usize + 1;
    let t: Vec<f64> = (0..n).map(|i| i as f64 / hz).collect();
    let v = t.iter().map(|&x| coast_speed(a, b, me, v_start, x)).collect();
    (t, v)
}

// ---------------------------------------------------------------- data

/// One logged coastdown run.
#[derive(Debug, Clone)]
pub struct Run {
    pub id: i64,
    pub t: Vec<f64>,
    /// Ground speed, m/s.
    pub v: Vec<f64>,
    /// Mean head-wind component along the road (positive = head wind), m/s.
    pub wind_ms: f64,
    /// Mean grade as sin(theta), positive uphill.
    pub grade: f64,
    pub temp_c: Option<f64>,
    pub p_pa: Option<f64>,
    pub rh: f64,
    /// +1 / -1 travel direction, 0 unknown.
    pub dir: i32,
}

/// Parse a CSV with a header row into named numeric columns (non-numbers become NaN).
pub fn parse_columns_csv(text: &str) -> Result<HashMap<String, Vec<f64>>, String> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    let header = lines.next().ok_or("log file is empty")?;
    let names: Vec<String> = header.split(',').map(|s| s.trim().to_ascii_lowercase()).collect();
    let mut cols: Vec<Vec<f64>> = vec![Vec::new(); names.len()];
    for l in lines {
        let f: Vec<&str> = l.split(',').collect();
        for (c, col) in cols.iter_mut().enumerate() {
            col.push(f.get(c).and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(f64::NAN));
        }
    }
    Ok(names.into_iter().zip(cols).collect())
}

fn first_col<'a>(cols: &'a HashMap<String, Vec<f64>>, names: &[&str]) -> Option<(&'a Vec<f64>, usize)> {
    names.iter().enumerate().find_map(|(i, n)| cols.get(*n).map(|c| (c, i)))
}

/// Split named columns into runs (by a `run` column, else wherever time restarts).
pub fn runs_from_columns(cols: &HashMap<String, Vec<f64>>) -> Result<Vec<Run>, String> {
    let (t, _) = first_col(cols, &["t", "time_s", "time"]).ok_or("log needs a time column 't'")?;
    let (v, vi) = first_col(cols, &["v_ms", "v", "speed_ms", "v_kmh", "speed_kmh"])
        .ok_or("log needs a speed column (v_ms or v_kmh)")?;
    let kmh = vi >= 3;
    if v.len() != t.len() {
        return Err("log columns have different lengths".into());
    }
    let n = t.len();
    let run_col = first_col(cols, &["run", "run_id"]).map(|c| c.0);
    let opt = |names: &[&str]| first_col(cols, names).map(|c| c.0.clone());
    let wind = opt(&["wind_ms", "wind"]);
    let grade = opt(&["grade", "slope"]);
    let temp = opt(&["temp_c", "t_c", "temperature_c"]);
    let pres = opt(&["p_pa", "pressure_pa"]);
    let rh = opt(&["rh", "rel_humidity"]);
    let dir = opt(&["dir", "direction"]);
    let heading = opt(&["heading_deg", "heading"]);

    let mut bounds = vec![0usize];
    for i in 1..n {
        let split = match run_col {
            Some(r) => r[i] != r[i - 1],
            None => t[i] < t[i - 1],
        };
        if split {
            bounds.push(i);
        }
    }
    bounds.push(n);
    let mean_of = |c: &Option<Vec<f64>>, a: usize, b: usize| -> Option<f64> {
        let c = c.as_ref()?;
        let vals: Vec<f64> = c[a..b].iter().copied().filter(|x| x.is_finite()).collect();
        (!vals.is_empty()).then(|| mean(&vals))
    };
    let mut runs = Vec::new();
    let mut first_heading: Option<f64> = None;
    for (ri, w) in bounds.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        let rows: Vec<usize> = (a..b).filter(|&i| t[i].is_finite() && v[i].is_finite()).collect();
        if rows.len() < 2 {
            continue;
        }
        let d = if let Some(d) = mean_of(&dir, a, b) {
            if d > 0.0 {
                1
            } else if d < 0.0 {
                -1
            } else {
                0
            }
        } else if let Some(h) = mean_of(&heading, a, b) {
            let h0 = *first_heading.get_or_insert(h);
            if (h - h0).to_radians().cos() >= 0.0 {
                1
            } else {
                -1
            }
        } else {
            0
        };
        runs.push(Run {
            id: run_col.map(|r| r[a] as i64).unwrap_or(ri as i64),
            t: rows.iter().map(|&i| t[i]).collect(),
            v: rows.iter().map(|&i| if kmh { v[i] / 3.6 } else { v[i] }).collect(),
            wind_ms: mean_of(&wind, a, b).unwrap_or(0.0),
            grade: mean_of(&grade, a, b).unwrap_or(0.0),
            temp_c: mean_of(&temp, a, b),
            p_pa: mean_of(&pres, a, b),
            rh: mean_of(&rh, a, b).unwrap_or(0.0),
            dir: d,
        });
    }
    if runs.is_empty() {
        return Err("log holds no usable runs".into());
    }
    Ok(runs)
}

/// Runs from JSON: a column object, `{"runs": [column objects]}`, or an array of those.
pub fn runs_from_json(v: &Value) -> Result<Vec<Run>, String> {
    let list: Vec<&Value> = if let Some(r) = v.get("runs").and_then(Value::as_array) {
        r.iter().collect()
    } else if let Some(a) = v.as_array() {
        a.iter().collect()
    } else {
        vec![v]
    };
    let mut runs = Vec::new();
    for (ri, obj) in list.iter().enumerate() {
        let map = obj.as_object().ok_or("log: expected an object of columns")?;
        let len = map.get("t").and_then(Value::as_array).map(Vec::len).ok_or("log: missing array 't'")?;
        let mut cols: HashMap<String, Vec<f64>> = HashMap::new();
        for (k, val) in map {
            let col = if let Some(a) = val.as_array() {
                a.iter().map(|x| x.as_f64().unwrap_or(f64::NAN)).collect()
            } else if let Some(x) = val.as_f64() {
                vec![x; len]
            } else {
                continue;
            };
            cols.insert(k.to_ascii_lowercase(), col);
        }
        let mut sub = runs_from_columns(&cols)?;
        for (j, r) in sub.iter_mut().enumerate() {
            if list.len() > 1 && !cols.contains_key("run") {
                r.id = ri as i64;
            } else if j > 0 && !cols.contains_key("run") {
                r.id = (ri * 1000 + j) as i64;
            }
        }
        runs.append(&mut sub);
    }
    Ok(runs)
}

// ---------------------------------------------------------------- filters

/// Hampel filter: `false` marks samples further than `k` x MAD from the local median.
pub fn hampel_keep(t: &[f64], v: &[f64], half_window_s: f64, k: f64) -> Vec<bool> {
    let n = t.len();
    let mut keep = vec![true; n];
    let (mut lo, mut hi) = (0usize, 0usize);
    for i in 0..n {
        while t[i] - t[lo] > half_window_s {
            lo += 1;
        }
        while hi < n && t[hi] - t[i] <= half_window_s {
            hi += 1;
        }
        if hi - lo < 5 {
            continue;
        }
        let w = &v[lo..hi];
        let med = median(w);
        let mad = median(&w.iter().map(|x| (x - med).abs()).collect::<Vec<_>>());
        if mad > 0.0 && (v[i] - med).abs() > k * mad {
            keep[i] = false;
        }
    }
    keep
}

// ---------------------------------------------------------------- fitting

struct Linear {
    a: f64,
    b: f64,
    cov: Vec<Vec<f64>>,
}

/// Regress `-M_e dv/dt` (1 s local slopes) on `u^2`. With `a_fixed`, only the slope is fitted.
fn accel_regression(t: &[f64], u: &[f64], me: f64, a_fixed: Option<f64>) -> Option<Linear> {
    let n = t.len();
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    let (mut lo, mut hi) = (0usize, 0usize);
    for i in 0..n {
        if t[i] - 0.5 < t[0] - 1e-9 || t[i] + 0.5 > t[n - 1] + 1e-9 {
            continue;
        }
        while t[i] - t[lo] > 0.5 + 1e-9 {
            lo += 1;
        }
        while hi < n && t[hi] - t[i] <= 0.5 + 1e-9 {
            hi += 1;
        }
        if hi - lo < 3 {
            continue;
        }
        let tm = mean(&t[lo..hi]);
        let um = mean(&u[lo..hi]);
        let (mut num, mut den) = (0.0, 0.0);
        for j in lo..hi {
            num += (t[j] - tm) * (u[j] - um);
            den += (t[j] - tm).powi(2);
        }
        if den <= 0.0 {
            continue;
        }
        xs.push(u[i] * u[i]);
        ys.push(-me * num / den);
    }
    let rows = xs.len();
    if rows < 6 {
        return None;
    }
    if let Some(af) = a_fixed {
        let sxx: f64 = xs.iter().map(|x| x * x).sum();
        let sxy: f64 = xs.iter().zip(&ys).map(|(x, y)| x * (y - af)).sum();
        let b = sxy / sxx;
        let rss: f64 = xs.iter().zip(&ys).map(|(x, y)| (y - af - b * x).powi(2)).sum();
        let var = rss / (rows - 1) as f64 / sxx;
        return Some(Linear { a: af, b, cov: vec![vec![0.0, 0.0], vec![0.0, var]] });
    }
    let nf = rows as f64;
    let sx: f64 = xs.iter().sum();
    let sxx: f64 = xs.iter().map(|x| x * x).sum();
    let sy: f64 = ys.iter().sum();
    let sxy: f64 = xs.iter().zip(&ys).map(|(x, y)| x * y).sum();
    let det = nf * sxx - sx * sx;
    if det.abs() < 1e-12 * (nf * sxx).abs() {
        return None;
    }
    let b = (nf * sxy - sx * sy) / det;
    let a = (sy - b * sx) / nf;
    let rss: f64 = xs.iter().zip(&ys).map(|(x, y)| (y - a - b * x).powi(2)).sum();
    let s2 = rss / (nf - 2.0);
    let inv = [vec![sxx / det, -sx / det], vec![-sx / det, nf / det]];
    let cov = inv.iter().map(|r| r.iter().map(|x| x * s2).collect()).collect();
    Some(Linear { a, b, cov })
}

struct Lm {
    a: f64,
    b: f64,
    rss: f64,
    cov: Vec<Vec<f64>>,
}

/// Levenberg-Marquardt fit of the closed-form coastdown to `(t, u)`.
/// Parameters are `[A, B, v0]`, or `[B, v0]` when `a_fixed` is given.
fn lm_fit(t: &[f64], u: &[f64], me: f64, a0: f64, b0: f64, a_fixed: Option<f64>) -> Option<Lm> {
    let n = t.len();
    let fit_a = a_fixed.is_none();
    let np = if fit_a { 3 } else { 2 };
    let mut th: Vec<f64> = if fit_a { vec![a0, b0, u[0]] } else { vec![b0, u[0]] };
    let model = |th: &[f64], k: usize| -> f64 {
        let (a, b, v0) = if fit_a { (th[0], th[1], th[2]) } else { (a_fixed.unwrap_or(0.0), th[0], th[1]) };
        coast_speed(a.max(1e-9), b.max(1e-12), me, v0.max(1e-6), t[k] - t[0])
    };
    let resid = |th: &[f64]| -> Vec<f64> { (0..n).map(|k| model(th, k) - u[k]).collect() };
    let rss_of = |r: &[f64]| r.iter().map(|x| x * x).sum::<f64>();
    let jac = |th: &[f64]| -> Vec<Vec<f64>> {
        let mut cols = Vec::with_capacity(np);
        for p in 0..np {
            let h = 1e-6 * th[p].abs().max(1e-6);
            let mut tp = th.to_vec();
            let mut tm = th.to_vec();
            tp[p] += h;
            tm[p] -= h;
            let (rp, rm) = (resid(&tp), resid(&tm));
            cols.push(rp.iter().zip(&rm).map(|(a, b)| (a - b) / (2.0 * h)).collect::<Vec<f64>>());
        }
        cols
    };
    let mut r = resid(&th);
    let mut rss = rss_of(&r);
    let mut lambda = 1e-3;
    for _ in 0..200 {
        if rss < 1e-26 * n as f64 {
            break;
        }
        let j = jac(&th);
        let jtj: Vec<Vec<f64>> =
            (0..np).map(|p| (0..np).map(|q| j[p].iter().zip(&j[q]).map(|(a, b)| a * b).sum()).collect()).collect();
        let jtr: Vec<f64> = (0..np).map(|p| j[p].iter().zip(&r).map(|(a, b)| a * b).sum()).collect();
        let mut improved = false;
        for _ in 0..30 {
            let mut m = jtj.clone();
            for p in 0..np {
                m[p][p] += lambda * jtj[p][p].max(1e-30);
            }
            let rhs: Vec<f64> = jtr.iter().map(|x| -x).collect();
            let Some(step) = lin_solve(m, rhs) else {
                lambda *= 10.0;
                continue;
            };
            let cand: Vec<f64> = th.iter().zip(&step).map(|(a, b)| a + b).collect();
            if cand.iter().any(|x| !x.is_finite()) {
                lambda *= 10.0;
                continue;
            }
            let rc = resid(&cand);
            let rssc = rss_of(&rc);
            if rssc.is_finite() && rssc < rss {
                let gain = rss - rssc;
                th = cand;
                r = rc;
                rss = rssc;
                lambda = (lambda / 3.0).max(1e-12);
                improved = gain > 1e-14 * rss.max(1e-30);
                break;
            }
            lambda *= 4.0;
        }
        if !improved {
            break;
        }
    }
    let j = jac(&th);
    let jtj: Vec<Vec<f64>> =
        (0..np).map(|p| (0..np).map(|q| j[p].iter().zip(&j[q]).map(|(a, b)| a * b).sum()).collect()).collect();
    let s2 = rss / (n.saturating_sub(np)).max(1) as f64;
    let inv = mat_inv(&jtj)?;
    let mut cov = vec![vec![0.0; 3]; 3];
    let idx: Vec<usize> = if fit_a { vec![0, 1, 2] } else { vec![1, 2] };
    for (pi, &gi) in idx.iter().enumerate() {
        for (pj, &gj) in idx.iter().enumerate() {
            cov[gi][gj] = inv[pi][pj] * s2;
        }
    }
    let (a, b) = if fit_a { (th[0], th[1]) } else { (a_fixed.unwrap_or(0.0), th[0]) };
    Some(Lm { a, b, rss, cov })
}

/// Estimator choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Integrated,
    Accel,
}

impl Method {
    pub fn name(self) -> &'static str {
        match self {
            Method::Integrated => "integrated",
            Method::Accel => "accel",
        }
    }
}

/// Options of the regression.
#[derive(Debug, Clone)]
pub struct CoastOpts {
    pub mass_kg: f64,
    pub rot_mass_kg: f64,
    pub method: Method,
    pub v_hi_ms: f64,
    pub v_lo_ms: f64,
    pub min_runs: usize,
    pub wind_max_ms: f64,
    pub outlier_sd: f64,
    /// Rolling coefficient held fixed (0 = fit it).
    pub crr_fixed: f64,
    pub hampel: bool,
    pub rho_default: Option<f64>,
}

impl CoastOpts {
    pub fn new(mass_kg: f64) -> Self {
        Self {
            mass_kg,
            rot_mass_kg: 0.0,
            method: Method::Integrated,
            v_hi_ms: 120.0 / 3.6,
            v_lo_ms: 20.0 / 3.6,
            min_runs: 8,
            wind_max_ms: 2.7,
            outlier_sd: 2.0,
            crr_fixed: 0.0,
            hampel: true,
            rho_default: None,
        }
    }
}

/// Result for one run.
#[derive(Debug, Clone)]
pub struct RunFit {
    pub id: i64,
    pub dir: i32,
    pub samples: usize,
    pub rho: f64,
    pub a: f64,
    pub b: f64,
    pub cda: f64,
    pub crr: f64,
    pub corr_ab: f64,
    pub rmse_ms: f64,
    pub accepted: bool,
    pub reason: String,
}

fn rejected(run: &Run, reason: &str) -> RunFit {
    RunFit {
        id: run.id,
        dir: run.dir,
        samples: run.t.len(),
        rho: f64::NAN,
        a: f64::NAN,
        b: f64::NAN,
        cda: f64::NAN,
        crr: f64::NAN,
        corr_ab: f64::NAN,
        rmse_ms: f64::NAN,
        accepted: false,
        reason: reason.into(),
    }
}

/// Fit one run.
pub fn fit_run(run: &Run, o: &CoastOpts) -> RunFit {
    let me = o.mass_kg + o.rot_mass_kg;
    if o.wind_max_ms > 0.0 && run.wind_ms.abs() > o.wind_max_ms {
        return rejected(run, "wind above the limit");
    }
    let rho = match (run.temp_c, run.p_pa) {
        (Some(tc), Some(p)) => air_density(tc, p, run.rh, R_DRY),
        _ => match o.rho_default {
            Some(r) => r,
            None => return rejected(run, "no density (log has no temp_c/p_pa and no env input)"),
        },
    };
    let keep = if o.hampel { hampel_keep(&run.t, &run.v, 3.0, 4.4478) } else { vec![true; run.t.len()] };
    // from the speed peak onward, until the speed drops below the low window edge
    let i_peak = run.v.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i).unwrap_or(0);
    let mut t = Vec::new();
    let mut u = Vec::new();
    for (i, &vi) in run.v.iter().enumerate().skip(i_peak) {
        if vi < o.v_lo_ms {
            break;
        }
        if keep[i] && vi <= o.v_hi_ms {
            t.push(run.t[i]);
            u.push(vi + run.wind_ms);
        }
    }
    if t.len() < 20 {
        return rejected(run, "fewer than 20 samples in the speed window");
    }
    let mg = o.mass_kg * G;
    let a_fixed = (o.crr_fixed > 0.0).then_some(mg * (o.crr_fixed + run.grade));
    let Some(lin) = accel_regression(&t, &u, me, a_fixed) else {
        return rejected(run, "acceleration regression failed (window too short or speed range too narrow)");
    };
    let (a_fit, b_fit, cov, rmse) = match o.method {
        Method::Accel => (lin.a, lin.b, lin.cov.clone(), f64::NAN),
        Method::Integrated => {
            let a0 = lin.a.max(1.0);
            let b0 = lin.b.max(1e-3);
            match lm_fit(&t, &u, me, a0, b0, a_fixed) {
                Some(f) => (f.a, f.b, f.cov.clone(), (f.rss / t.len() as f64).sqrt()),
                None => return rejected(run, "least-squares fit did not converge"),
            }
        }
    };
    let corr = if a_fixed.is_some() {
        f64::NAN
    } else {
        let (va, vb, cab) = (cov[0][0], cov[1][1], cov[0][1]);
        if va > 0.0 && vb > 0.0 {
            cab / (va * vb).sqrt()
        } else {
            f64::NAN
        }
    };
    if !(b_fit > 0.0 && b_fit.is_finite()) {
        return rejected(run, "non-physical drag term (B <= 0)");
    }
    let f0 = a_fit - mg * run.grade;
    RunFit {
        id: run.id,
        dir: run.dir,
        samples: t.len(),
        rho,
        a: f0,
        b: b_fit,
        cda: 2.0 * b_fit / rho,
        crr: f0 / mg,
        corr_ab: corr,
        rmse_ms: rmse,
        accepted: true,
        reason: String::new(),
    }
}

/// Result of the whole regression.
#[derive(Debug, Clone)]
pub struct CoastFit {
    pub cda: f64,
    pub cda_se: f64,
    pub cda_sd: f64,
    pub cda_ci95: (f64, f64),
    pub crr: f64,
    pub crr_se: f64,
    pub corr_cda_crr: f64,
    pub rho_mean: f64,
    pub n_runs: usize,
    pub n_valid_runs: usize,
    pub n_used_runs: usize,
    pub n_pairs_used: usize,
    pub min_detectable_dcda: f64,
    pub method: Method,
    pub mass_kg: f64,
    pub rot_mass_kg: f64,
    pub crr_fixed: f64,
    pub runs: Vec<RunFit>,
    pub notes: Vec<(Severity, String)>,
}

impl CoastFit {
    pub fn to_json(&self) -> Value {
        let num = |x: f64| if x.is_finite() { json!(x) } else { Value::Null };
        json!({
            "kind": "coast_fit.v1",
            "cda_m2": self.cda,
            "cda_se": self.cda_se,
            "cda_sd": self.cda_sd,
            "cda_ci95": [self.cda_ci95.0, self.cda_ci95.1],
            "crr": self.crr,
            "crr_se": self.crr_se,
            "corr_cda_crr": num(self.corr_cda_crr),
            "rho_mean": self.rho_mean,
            "n_runs": self.n_runs,
            "n_valid_runs": self.n_valid_runs,
            "n_used_runs": self.n_used_runs,
            "n_pairs_used": self.n_pairs_used,
            "min_detectable_dcda_m2": self.min_detectable_dcda,
            "method": self.method.name(),
            "mass_kg": self.mass_kg,
            "rot_mass_kg": self.rot_mass_kg,
            "crr_fixed": self.crr_fixed,
            "runs": self.runs.iter().map(|r| json!({
                "id": r.id, "dir": r.dir, "samples": r.samples, "accepted": r.accepted, "reason": r.reason,
                "rho": num(r.rho), "cda_m2": num(r.cda), "crr": num(r.crr), "rmse_ms": num(r.rmse_ms),
            })).collect::<Vec<_>>(),
        })
    }
}

/// Fit every run, pair opposite directions, drop outliers and aggregate.
pub fn fit_coastdown(runs: &[Run], o: &CoastOpts) -> Result<CoastFit, String> {
    if !(o.mass_kg.is_finite() && o.mass_kg > 0.0) {
        return Err("mass must be positive".into());
    }
    if o.v_hi_ms <= o.v_lo_ms {
        return Err("speed window: high edge must exceed the low edge".into());
    }
    let fits: Vec<RunFit> = runs.iter().map(|r| fit_run(r, o)).collect();
    let valid: Vec<&RunFit> = fits.iter().filter(|f| f.accepted).collect();
    if valid.is_empty() {
        let why = fits.first().map(|f| f.reason.clone()).unwrap_or_default();
        return Err(format!("no run could be fitted ({why})"));
    }
    let mut notes: Vec<(Severity, String)> = Vec::new();
    for f in fits.iter().filter(|f| !f.accepted) {
        notes.push((Severity::Warning, format!("run {} rejected: {}", f.id, f.reason)));
    }
    // pair opposite directions in log order
    let pos: Vec<&RunFit> = valid.iter().copied().filter(|f| f.dir > 0).collect();
    let neg: Vec<&RunFit> = valid.iter().copied().filter(|f| f.dir < 0).collect();
    let unk: Vec<&RunFit> = valid.iter().copied().filter(|f| f.dir == 0).collect();
    let mut pairs: Vec<(f64, f64, f64, usize)> = Vec::new(); // cda, crr, corr, runs in pair
    for (p, n) in pos.iter().zip(&neg) {
        pairs.push((0.5 * (p.cda + n.cda), 0.5 * (p.crr + n.crr), 0.5 * (p.corr_ab + n.corr_ab), 2));
    }
    let leftover = pos.len().abs_diff(neg.len());
    if leftover > 0 && !pairs.is_empty() {
        notes.push((Severity::Info, format!("{leftover} unpaired run(s) ignored")));
    }
    if pairs.is_empty() {
        for f in valid.iter() {
            pairs.push((f.cda, f.crr, f.corr_ab, 1));
        }
        notes.push((
            Severity::Warning,
            if unk.len() == valid.len() {
                "no travel direction in the log: runs are not paired, so steady wind and grade are not cancelled".to_string()
            } else {
                "only one travel direction: runs are not paired, so steady wind and grade are not cancelled".to_string()
            },
        ));
    }
    // outlier filter on the pair CdA values
    let mut used: Vec<(f64, f64, f64, usize)> = pairs.clone();
    if pairs.len() >= 4 && o.outlier_sd > 0.0 {
        let c: Vec<f64> = pairs.iter().map(|p| p.0).collect();
        let (m, s) = (mean(&c), sd(&c));
        used = pairs.iter().copied().filter(|p| (p.0 - m).abs() <= o.outlier_sd * s).collect();
        if used.len() < pairs.len() {
            notes.push((
                Severity::Info,
                format!("{} pair(s) dropped beyond {} SD", pairs.len() - used.len(), o.outlier_sd),
            ));
        }
    }
    let n = used.len();
    let cdas: Vec<f64> = used.iter().map(|p| p.0).collect();
    let crrs: Vec<f64> = used.iter().map(|p| p.1).collect();
    let corr_vals: Vec<f64> = used.iter().map(|p| p.2).filter(|x| x.is_finite()).collect();
    let n_used_runs: usize = used.iter().map(|p| p.3).sum();
    let cda = mean(&cdas);
    let crr = mean(&crrs);
    let rho_mean = mean(&valid.iter().map(|f| f.rho).collect::<Vec<_>>());
    let (cda_sd, crr_sd) = if n >= 2 {
        (sd(&cdas), sd(&crrs))
    } else {
        // single pair: fall back to the run-to-run difference scale of the fit residual
        let f = valid[0];
        (0.05 * f.cda.abs(), 0.05 * f.crr.abs())
    };
    let cda_se = cda_sd / (n as f64).sqrt();
    let crr_se = crr_sd / (n as f64).sqrt();
    let tq = t975(n.saturating_sub(1));
    let ci = if n >= 2 { (cda - tq * cda_se, cda + tq * cda_se) } else { (cda - 1.96 * cda_se, cda + 1.96 * cda_se) };
    let min_detectable_dcda = 2.8 * cda_sd * (2.0 / n as f64).sqrt();
    if n < 2 {
        notes.push((Severity::Warning, "a single run/pair: the uncertainty is a placeholder (5 percent), repeat the runs".into()));
    }
    if o.rot_mass_kg == 0.0 {
        notes.push((
            Severity::Warning,
            "rotating mass is 0: a 3 percent error in effective mass is a 3 percent error in CdA".into(),
        ));
    }
    if valid.len() < o.min_runs {
        notes.push((Severity::Warning, format!("{} valid runs, {} required", valid.len(), o.min_runs)));
    }
    Ok(CoastFit {
        cda,
        cda_se,
        cda_sd,
        cda_ci95: ci,
        crr,
        crr_se,
        corr_cda_crr: if corr_vals.is_empty() { f64::NAN } else { mean(&corr_vals) },
        rho_mean,
        n_runs: runs.len(),
        n_valid_runs: valid.len(),
        n_used_runs,
        n_pairs_used: n,
        min_detectable_dcda,
        method: o.method,
        mass_kg: o.mass_kg,
        rot_mass_kg: o.rot_mass_kg,
        crr_fixed: o.crr_fixed,
        runs: fits,
        notes,
    })
}

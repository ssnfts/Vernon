//! Acceptance of a time-resolved (transient) force history.
//!
//! Pure functions, no I/O. A transient run produces a coefficient history over
//! physical time with a possibly varying step. The gate here is the v2 draft
//! of `docs/research/wp9-ddes-route-plan.md` section 3.1:
//!
//! * discard a wash-out after the switch from the steady start;
//! * average over at least a minimum number of convective time units (CTU),
//!   time-weighted, with a stop rule fixed in advance: the gate is evaluated
//!   at every check interval and the run stops at the first pass or at the cap;
//! * the window must be stationary (no trend, no drift between halves, each
//!   failing only when significant *and* larger than its size limit) and the
//!   mean known well enough (effective sample size, standard error as the
//!   larger of the autocorrelation and batch-means estimates);
//! * every time step must converge inside its outer loops (per-step rule read
//!   from the solver log), judged by a residual *drop*, never by an absolute
//!   residual, because absolute initial residuals shrink with the time step;
//! * a segment at half the time step must agree with the main window.
//!
//! Statistics reuse `convergence.rs` (Geyer ESS, normal quantile).

use serde::{Deserialize, Serialize};

use crate::convergence::{effective_sample_size, two_sided_z};

/// Tunables of the transient gate. Times are seconds; lengths are CTU.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransientParams {
    /// One convective time unit, L / U, in seconds.
    pub ctu_s: f64,
    /// Discarded after the switch from the steady start.
    pub washout_ctu: f64,
    /// Shortest averaging window.
    pub min_average_ctu: f64,
    /// The gate is evaluated at every multiple of this past the minimum.
    pub check_every_ctu: f64,
    /// Longest averaging window; reaching it without a pass is a fail.
    pub max_average_ctu: f64,
    /// Two-sided confidence of the trend and halves tests.
    pub confidence: f64,
    /// Largest accepted trend per CTU relative to `max(|mean|, std)`.
    pub max_rel_slope_per_ctu: f64,
    /// Largest accepted difference of the half means relative to `|mean|`.
    pub max_rel_half_diff: f64,
    /// Smallest accepted effective sample size; `None` reports it only.
    pub min_ess: Option<f64>,
    /// Largest accepted standard error of the mean relative to `|mean|`;
    /// `None` reports the interval instead of gating on it.
    pub max_rel_std_err: Option<f64>,
    /// Block length of the batch-means standard error.
    pub batch_ctu: f64,
}

impl TransientParams {
    /// The v2 draft values for a given convective time unit.
    pub fn v2(ctu_s: f64) -> Self {
        Self {
            ctu_s,
            washout_ctu: 2.0,
            min_average_ctu: 6.0,
            check_every_ctu: 1.0,
            max_average_ctu: 15.0,
            confidence: 0.99,
            max_rel_slope_per_ctu: 0.005,
            max_rel_half_diff: 0.002,
            min_ess: Some(50.0),
            max_rel_std_err: Some(0.005),
            batch_ctu: 1.0,
        }
    }

    /// The fast-tier rule (user decision 2026-11-12, "reported interval"):
    /// the window must be stationary and each step converged, and the mean is
    /// delivered with its interval; the effective sample size and the
    /// standard error are reported, not gated. The half-step segment stays
    /// the consistency check.
    pub fn fast_tier(ctu_s: f64) -> Self {
        Self { min_ess: None, max_rel_std_err: None, ..Self::v2(ctu_s) }
    }

    fn validate(&self) -> Result<(), String> {
        for (name, v) in [
            ("convective time unit", self.ctu_s),
            ("minimum averaging length", self.min_average_ctu),
            ("check interval", self.check_every_ctu),
            ("batch length", self.batch_ctu),
        ] {
            if !v.is_finite() || v <= 0.0 {
                return Err(format!("the {name} must be positive and finite"));
            }
        }
        if !self.washout_ctu.is_finite() || self.washout_ctu < 0.0 {
            return Err("the wash-out must be zero or positive".into());
        }
        if self.max_average_ctu < self.min_average_ctu {
            return Err("the averaging cap is shorter than the minimum".into());
        }
        if !(0.0 < self.confidence && self.confidence < 1.0) {
            return Err("the confidence must lie in (0, 1)".into());
        }
        Ok(())
    }
}

/// What the stop rule says after an assessment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// Not passed yet and the cap is not reached: run on.
    Continue,
    /// Every criterion holds: stop and report.
    Pass,
    /// The cap is reached without a pass.
    Fail,
}

/// Outcome of [`assess`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransientReport {
    pub verdict: Verdict,
    pub window_start_s: f64,
    pub window_end_s: f64,
    pub averaged_ctu: f64,
    /// Raw samples inside the window.
    pub samples: usize,
    /// Samples after resampling to a uniform step.
    pub resampled: usize,
    pub resample_dt_s: f64,
    /// Time-weighted mean of the window.
    pub mean: f64,
    pub std: f64,
    pub ess: f64,
    pub std_err_acf: f64,
    pub std_err_batch: f64,
    /// The larger of the two estimates.
    pub std_err: f64,
    pub batches: usize,
    pub rel_slope_per_ctu: f64,
    pub slope_significant: bool,
    pub rel_half_diff: f64,
    pub half_diff_significant: bool,
    pub reasons: Vec<String>,
}

fn empty_report(start: f64, end: f64, ctu_s: f64, samples: usize) -> TransientReport {
    TransientReport {
        verdict: Verdict::Continue,
        window_start_s: start,
        window_end_s: end,
        averaged_ctu: ((end - start) / ctu_s).max(0.0),
        samples,
        resampled: 0,
        resample_dt_s: f64::NAN,
        mean: f64::NAN,
        std: f64::NAN,
        ess: 0.0,
        std_err_acf: f64::NAN,
        std_err_batch: f64::NAN,
        std_err: f64::NAN,
        batches: 0,
        rel_slope_per_ctu: f64::NAN,
        slope_significant: false,
        rel_half_diff: f64::NAN,
        half_diff_significant: false,
        reasons: Vec::new(),
    }
}

fn mean_of(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

fn std_of(v: &[f64]) -> f64 {
    if v.len() < 2 {
        return 0.0;
    }
    let m = mean_of(v);
    (v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (v.len() - 1) as f64).sqrt()
}

/// Trapezoid time-weighted mean of `(t, v)` pairs sorted by time.
fn time_weighted_mean(t: &[f64], v: &[f64]) -> f64 {
    if t.len() < 2 {
        return v.first().copied().unwrap_or(f64::NAN);
    }
    let mut area = 0.0;
    for i in 1..t.len() {
        area += 0.5 * (v[i] + v[i - 1]) * (t[i] - t[i - 1]);
    }
    area / (t[t.len() - 1] - t[0])
}

/// Linear interpolation of `(t, v)` onto a uniform grid of step `dt`.
fn resample(t: &[f64], v: &[f64], dt: f64) -> Vec<f64> {
    let (t0, t1) = (t[0], t[t.len() - 1]);
    let n = ((t1 - t0) / dt).floor() as usize + 1;
    let mut out = Vec::with_capacity(n);
    let mut j = 0;
    for i in 0..n {
        let ti = (t0 + i as f64 * dt).min(t1);
        while j + 1 < t.len() - 1 && t[j + 1] < ti {
            j += 1;
        }
        let (ta, tb) = (t[j], t[j + 1]);
        let f = if tb > ta { ((ti - ta) / (tb - ta)).clamp(0.0, 1.0) } else { 0.0 };
        out.push(v[j] + f * (v[j + 1] - v[j]));
    }
    out
}

/// Assess a history `(time_s, value)` after a switch at `switch_time_s`.
pub fn assess(time: &[f64], value: &[f64], switch_time_s: f64, p: &TransientParams) -> Result<TransientReport, String> {
    p.validate()?;
    if time.len() != value.len() {
        return Err(format!("history has {} times and {} values", time.len(), value.len()));
    }
    if time.windows(2).any(|w| w[1] <= w[0]) || time.iter().any(|t| !t.is_finite()) {
        return Err("history times must be finite and strictly increasing".into());
    }
    let start = switch_time_s + p.washout_ctu * p.ctu_s;
    let end = time.last().copied().unwrap_or(start);
    let first = time.partition_point(|t| *t < start);
    let (t, v) = (&time[first..], &value[first..]);
    let mut r = empty_report(start, end.max(start), p.ctu_s, t.len());
    let mut reasons = Vec::new();
    if r.averaged_ctu < p.min_average_ctu {
        reasons.push(format!(
            "averaging: {:.2} of the required {:.2} CTU available after the wash-out",
            r.averaged_ctu, p.min_average_ctu
        ));
        r.reasons = reasons;
        return Ok(r);
    }
    if t.len() < 16 {
        reasons.push(format!("samples: only {} inside the window, need at least 16", t.len()));
        r.reasons = reasons;
        r.verdict = if r.averaged_ctu >= p.max_average_ctu { Verdict::Fail } else { Verdict::Continue };
        return Ok(r);
    }
    if v.iter().any(|x| !x.is_finite()) {
        return Err("history contains a non-finite value inside the window".into());
    }

    // Time-weighted mean on the raw samples; everything else on a uniform resample.
    r.mean = time_weighted_mean(t, v);
    let mut steps: Vec<f64> = t.windows(2).map(|w| w[1] - w[0]).collect();
    steps.sort_by(f64::total_cmp);
    let dt = steps[steps.len() / 2].max(f64::MIN_POSITIVE);
    let y = resample(t, v, dt);
    let n = y.len();
    r.resampled = n;
    r.resample_dt_s = dt;
    let z = two_sided_z(p.confidence);
    let std = std_of(&y);
    let ess = effective_sample_size(&y);
    r.std = std;
    r.ess = ess;
    r.std_err_acf = std / ess.sqrt();
    let scale = r.mean.abs().max(std).max(1e-300);

    // Batch means: blocks of `batch_ctu`, as a second estimate that does not
    // depend on where the autocorrelation sum is cut off.
    let per_block = ((p.batch_ctu * p.ctu_s) / dt).round().max(1.0) as usize;
    let blocks: Vec<f64> = y.chunks_exact(per_block).map(mean_of).collect();
    r.batches = blocks.len();
    r.std_err_batch = if blocks.len() >= 2 { std_of(&blocks) / (blocks.len() as f64).sqrt() } else { f64::NAN };
    r.std_err = if r.std_err_batch.is_finite() { r.std_err_acf.max(r.std_err_batch) } else { r.std_err_acf };

    // Trend: least squares on the resample; significance with the residual
    // autocorrelation folded into the slope's standard error.
    let x: Vec<f64> = (0..n).map(|i| i as f64 * dt).collect();
    let (xm, ym) = (mean_of(&x), mean_of(&y));
    let sxx: f64 = x.iter().map(|a| (a - xm) * (a - xm)).sum();
    let sxy: f64 = x.iter().zip(&y).map(|(a, b)| (a - xm) * (b - ym)).sum();
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let res: Vec<f64> = x.iter().zip(&y).map(|(a, b)| b - ym - slope * (a - xm)).collect();
    let ess_res = effective_sample_size(&res);
    let s2 = res.iter().map(|e| e * e).sum::<f64>() / (n as f64 - 2.0).max(1.0);
    let se_slope = (s2 / sxx.max(1e-300)).sqrt() * (n as f64 / ess_res).sqrt();
    r.rel_slope_per_ctu = (slope * p.ctu_s).abs() / scale;
    r.slope_significant = slope.abs() > z * se_slope;
    if r.slope_significant && r.rel_slope_per_ctu > p.max_rel_slope_per_ctu {
        reasons.push(format!(
            "slope: {:.3}% of scale per CTU is significant at {:.0}% and above the {:.3}% limit",
            100.0 * r.rel_slope_per_ctu,
            100.0 * p.confidence,
            100.0 * p.max_rel_slope_per_ctu
        ));
    }

    // Drift between the halves: significant and larger than the limit.
    let (h1, h2) = y.split_at(n / 2);
    let diff = (mean_of(h2) - mean_of(h1)).abs();
    let se_diff = 2.0 * std / ess.sqrt();
    r.rel_half_diff = diff / r.mean.abs().max(1e-300);
    r.half_diff_significant = diff > z * se_diff;
    if r.half_diff_significant && r.rel_half_diff > p.max_rel_half_diff {
        reasons.push(format!(
            "halves: the half means differ by {:.3}% of the mean, significant at {:.0}% and above the {:.3}% limit",
            100.0 * r.rel_half_diff,
            100.0 * p.confidence,
            100.0 * p.max_rel_half_diff
        ));
    }

    if let Some(min_ess) = p.min_ess {
        if ess < min_ess {
            reasons.push(format!("ess: effective sample size {ess:.1} < required {min_ess:.1}"));
        }
    }
    if let Some(limit) = p.max_rel_std_err {
        if r.mean.abs() > 1e-12 && r.std_err / r.mean.abs() > limit {
            reasons.push(format!(
                "std_err: {:.3}% of the mean (autocorrelation {:.3}%, batch means {:.3}%), limit {:.3}%",
                100.0 * r.std_err / r.mean.abs(),
                100.0 * r.std_err_acf / r.mean.abs(),
                100.0 * r.std_err_batch / r.mean.abs(),
                100.0 * limit
            ));
        }
    }

    r.verdict = if reasons.is_empty() {
        Verdict::Pass
    } else if r.averaged_ctu + 1e-9 >= p.max_average_ctu {
        reasons.push(format!("cap: {:.2} CTU averaged without a pass", r.averaged_ctu));
        Verdict::Fail
    } else {
        Verdict::Continue
    };
    r.reasons = reasons;
    Ok(r)
}

/// End time of the next segment to run, or `None` when the stop rule has decided.
pub fn next_segment_end_s(report: &TransientReport, switch_time_s: f64, p: &TransientParams) -> Option<f64> {
    if report.verdict != Verdict::Continue {
        return None;
    }
    let start = switch_time_s + p.washout_ctu * p.ctu_s;
    let next_ctu = if report.averaged_ctu + 1e-9 < p.min_average_ctu {
        p.min_average_ctu
    } else {
        // the next check boundary strictly after what has been averaged
        let k = ((report.averaged_ctu - p.min_average_ctu) / p.check_every_ctu + 1e-9).floor() + 1.0;
        p.min_average_ctu + k * p.check_every_ctu
    };
    Some(start + next_ctu.min(p.max_average_ctu) * p.ctu_s)
}

/// Does the half-step segment agree with the main window? Returns the
/// decision, the difference of the means and the allowed difference.
pub fn dt_half_agrees(main: &TransientReport, half: &TransientReport, confidence: f64) -> (bool, f64, f64) {
    let diff = (main.mean - half.mean).abs();
    let allowed = two_sided_z(confidence) * (main.std_err.powi(2) + half.std_err.powi(2)).sqrt();
    (diff.is_finite() && allowed.is_finite() && diff <= allowed, diff, allowed)
}

/// One time step as the solver logged it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StepRecord {
    pub time_s: f64,
    pub dt_s: Option<f64>,
    pub co_mean: Option<f64>,
    pub co_max: Option<f64>,
    pub outer_iterations: u32,
    /// Initial residual of the first p solve of the step.
    pub p_first: Option<f64>,
    /// Initial residual of the last p solve of the step (final corrector).
    pub p_final: Option<f64>,
    /// Largest component initial residual of U in the first / last outer loop.
    pub u_first: Option<f64>,
    pub u_final: Option<f64>,
    pub k_first: Option<f64>,
    pub k_final: Option<f64>,
    pub omega_first: Option<f64>,
    pub omega_final: Option<f64>,
}

/// Residuals of one outer loop, as the step accumulates them.
#[derive(Default)]
struct OuterLoop {
    u: Option<f64>,
    k: Option<f64>,
    omega: Option<f64>,
}

#[derive(Default)]
struct StepBuilder {
    record: StepRecord,
    loops: Vec<OuterLoop>,
}

impl StepBuilder {
    fn finish(mut self) -> StepRecord {
        self.record.outer_iterations = self.loops.len() as u32;
        if let (Some(first), Some(last)) = (self.loops.first(), self.loops.last()) {
            self.record.u_first = first.u;
            self.record.k_first = first.k;
            self.record.omega_first = first.omega;
            self.record.u_final = last.u;
            self.record.k_final = last.k;
            self.record.omega_final = last.omega;
        }
        self.record
    }
}

fn number_after(line: &str, key: &str) -> Option<f64> {
    let rest = line.split(key).nth(1)?;
    rest.trim().split(|c: char| c == ',' || c.is_whitespace()).next()?.parse().ok()
}

/// Parse a PIMPLE solver log into time steps.
pub fn parse_pimple_log(text: &str) -> Vec<StepRecord> {
    let mut steps = Vec::new();
    let mut current: Option<StepBuilder> = None;
    let mut pending_dt = None;
    let mut pending_co = (None, None);
    for raw in text.lines() {
        let line = raw.trim_end();
        if let Some(v) = line.strip_prefix("Time = ") {
            if let Ok(t) = v.trim().parse::<f64>() {
                if let Some(done) = current.take() {
                    steps.push(done.finish());
                }
                let mut b = StepBuilder::default();
                b.record.time_s = t;
                b.record.dt_s = pending_dt.take();
                b.record.co_mean = pending_co.0.take();
                b.record.co_max = pending_co.1.take();
                current = Some(b);
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("Courant Number mean: ") {
            pending_co = (number_after(&format!("mean: {rest}"), "mean: "), number_after(rest, "max: "));
            continue;
        }
        if let Some(v) = line.strip_prefix("deltaT = ") {
            pending_dt = v.trim().parse().ok();
            continue;
        }
        let Some(b) = current.as_mut() else { continue };
        if line.starts_with("PIMPLE: iteration ") || line.starts_with("PIMPLE: Iteration ") {
            b.loops.push(OuterLoop::default());
            continue;
        }
        if let Some(rest) = line.split("Solving for ").nth(1) {
            let field = rest.split(',').next().unwrap_or("").trim();
            let Some(initial) = number_after(rest, "Initial residual = ") else { continue };
            if field == "p" {
                if b.record.p_first.is_none() {
                    b.record.p_first = Some(initial);
                }
                b.record.p_final = Some(initial);
                continue;
            }
            if b.loops.is_empty() {
                b.loops.push(OuterLoop::default());
            }
            let l = b.loops.last_mut().unwrap();
            let slot = match field {
                "Ux" | "Uy" | "Uz" => &mut l.u,
                "k" => &mut l.k,
                "omega" => &mut l.omega,
                _ => continue,
            };
            *slot = Some(slot.map_or(initial, |prev| prev.max(initial)));
        }
    }
    if let Some(done) = current.take() {
        steps.push(done.finish());
    }
    // A step without its own deltaT line (fixed step) gets it from the clock.
    for i in 0..steps.len() {
        if steps[i].dt_s.is_none() && i > 0 {
            steps[i].dt_s = Some(steps[i].time_s - steps[i - 1].time_s);
        }
    }
    steps
}

/// The per-step convergence rule: every field's initial residual must drop
/// across the outer loops of the step. A drop is used for p as well, because
/// an absolute pressure residual scales with the time step (at a wake Courant
/// number near one it sits around 1e-3 even when each step is converged),
/// so an absolute limit would reward small steps for the wrong reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepRule {
    /// Required drop of p, U, k and omega from the first to the last outer
    /// loop, in orders of magnitude.
    pub min_drop_orders: f64,
    /// Share of steps that must meet each rule.
    pub min_pass_fraction: f64,
}

impl Default for StepRule {
    fn default() -> Self {
        Self { min_drop_orders: 1.0, min_pass_fraction: 0.99 }
    }
}

/// Outcome of [`step_convergence`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepReport {
    pub steps: usize,
    pub p_pass_fraction: f64,
    pub u_pass_fraction: f64,
    pub k_pass_fraction: f64,
    pub omega_pass_fraction: f64,
    pub p_final_median: f64,
    pub co_max_peak: f64,
    pub co_mean_median: f64,
    pub dt_min_s: f64,
    pub dt_max_s: f64,
    pub passed: bool,
    pub reasons: Vec<String>,
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn dropped(first: Option<f64>, last: Option<f64>, orders: f64) -> bool {
    match (first, last) {
        (Some(a), Some(b)) if a > 0.0 && b > 0.0 => (a / b).log10() >= orders,
        _ => false,
    }
}

/// Apply the per-step rule to the steps at or after `from_time_s`.
pub fn step_convergence(steps: &[StepRecord], from_time_s: f64, rule: &StepRule) -> Result<StepReport, String> {
    let counted: Vec<&StepRecord> = steps.iter().filter(|s| s.time_s >= from_time_s).collect();
    if counted.is_empty() {
        return Err(format!("no time steps at or after {from_time_s} s in the solver log"));
    }
    let n = counted.len() as f64;
    let fraction = |ok: &dyn Fn(&StepRecord) -> bool| counted.iter().filter(|s| ok(s)).count() as f64 / n;
    let p_pass = fraction(&|s| dropped(s.p_first, s.p_final, rule.min_drop_orders));
    let u_pass = fraction(&|s| dropped(s.u_first, s.u_final, rule.min_drop_orders));
    let k_pass = fraction(&|s| dropped(s.k_first, s.k_final, rule.min_drop_orders));
    let omega_pass = fraction(&|s| dropped(s.omega_first, s.omega_final, rule.min_drop_orders));
    let dts: Vec<f64> = counted.iter().filter_map(|s| s.dt_s).collect();
    let mut reasons = Vec::new();
    for (name, frac) in [("p drop", p_pass), ("U drop", u_pass), ("k drop", k_pass), ("omega drop", omega_pass)] {
        if frac < rule.min_pass_fraction {
            reasons.push(format!("{name}: met in {:.1}% of steps, required {:.1}%", 100.0 * frac, 100.0 * rule.min_pass_fraction));
        }
    }
    Ok(StepReport {
        steps: counted.len(),
        p_pass_fraction: p_pass,
        u_pass_fraction: u_pass,
        k_pass_fraction: k_pass,
        omega_pass_fraction: omega_pass,
        p_final_median: median(counted.iter().filter_map(|s| s.p_final).collect()),
        co_max_peak: counted.iter().filter_map(|s| s.co_max).fold(f64::NAN, f64::max),
        co_mean_median: median(counted.iter().filter_map(|s| s.co_mean).collect()),
        dt_min_s: dts.iter().copied().fold(f64::NAN, f64::min),
        dt_max_s: dts.iter().copied().fold(f64::NAN, f64::max),
        passed: reasons.is_empty(),
        reasons,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic uniform noise in [-0.5, 0.5) (xorshift64*).
    fn noise(n: usize, seed: u64) -> Vec<f64> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s ^= s >> 12;
                s ^= s << 25;
                s ^= s >> 27;
                (s.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64 - 0.5
            })
            .collect()
    }

    const CTU: f64 = 0.0261;
    const SWITCH: f64 = 10.0;

    /// A stationary, mildly autocorrelated signal around 0.30 with 0.5%
    /// scatter, sampled 200 times per CTU for `ctus` convective units.
    fn stationary(ctus: f64, seed: u64) -> (Vec<f64>, Vec<f64>) {
        let n = (ctus * 200.0) as usize;
        let e = noise(n, seed);
        let mut v = Vec::with_capacity(n);
        let mut x = 0.0;
        for ei in e {
            x = 0.6 * x + ei * 0.0015 * 2.0;
            v.push(0.30 + x);
        }
        let t: Vec<f64> = (0..n).map(|i| SWITCH + (i as f64 + 1.0) * CTU / 200.0).collect();
        (t, v)
    }

    #[test]
    fn transient_too_short_a_window_continues_and_says_how_far_it_got() {
        let (t, v) = stationary(5.0, 1);
        let r = assess(&t, &v, SWITCH, &TransientParams::v2(CTU)).unwrap();
        assert_eq!(r.verdict, Verdict::Continue);
        assert!((r.averaged_ctu - 3.0).abs() < 0.02, "{}", r.averaged_ctu);
        assert!(r.reasons.iter().any(|s| s.contains("averaging")), "{:?}", r.reasons);
        // The next segment ends when six CTU of averaging are available.
        let next = next_segment_end_s(&r, SWITCH, &TransientParams::v2(CTU)).unwrap();
        assert!((next - (SWITCH + 8.0 * CTU)).abs() < 1e-9, "{next}");
    }

    #[test]
    fn transient_stationary_window_passes_at_six_ctu() {
        let (t, v) = stationary(8.0, 7);
        let p = TransientParams::v2(CTU);
        let r = assess(&t, &v, SWITCH, &p).unwrap();
        assert_eq!(r.verdict, Verdict::Pass, "{:?}", r.reasons);
        assert!((r.averaged_ctu - 6.0).abs() < 0.02);
        assert!((r.mean - 0.30).abs() < 0.002, "{}", r.mean);
        assert!(r.ess >= 50.0, "ess {}", r.ess);
        assert!(r.std_err / r.mean <= 0.005);
        assert_eq!(r.batches, 6);
        assert!(r.std_err >= r.std_err_acf && r.std_err >= r.std_err_batch);
        assert_eq!(next_segment_end_s(&r, SWITCH, &p), None);
    }

    #[test]
    fn transient_drift_fails_at_the_cap_and_names_the_slope() {
        let (t, mut v) = stationary(17.5, 3);
        // 2% of the mean per CTU, far above the 0.5% limit.
        for (ti, vi) in t.iter().zip(v.iter_mut()) {
            *vi += 0.30 * 0.02 * (ti - SWITCH) / CTU;
        }
        let p = TransientParams::v2(CTU);
        let r = assess(&t, &v, SWITCH, &p).unwrap();
        assert_eq!(r.verdict, Verdict::Fail, "{:?}", r.reasons);
        assert!(r.slope_significant && r.rel_slope_per_ctu > 0.005);
        assert!(r.reasons.iter().any(|s| s.starts_with("slope")), "{:?}", r.reasons);
        // The same drift with less than the cap available asks for more time.
        let short: usize = (10.0 * 200.0) as usize;
        let r2 = assess(&t[..short], &v[..short], SWITCH, &p).unwrap();
        assert_eq!(r2.verdict, Verdict::Continue, "{:?}", r2.reasons);
        let next = next_segment_end_s(&r2, SWITCH, &p).unwrap();
        assert!((next - (SWITCH + 11.0 * CTU)).abs() < 1e-9, "{next}");
    }

    #[test]
    fn transient_quasi_periodic_signal_has_a_small_ess_and_a_larger_batch_error() {
        // Period 2 CTU, amplitude 1% of the mean, no noise: six whole periods.
        let n = (14.0 * 200.0) as usize;
        let t: Vec<f64> = (0..n).map(|i| SWITCH + (i as f64 + 1.0) * CTU / 200.0).collect();
        let v: Vec<f64> = t.iter().map(|ti| 0.30 * (1.0 + 0.01 * (std::f64::consts::TAU * (ti - SWITCH) / (2.0 * CTU)).sin())).collect();
        let p = TransientParams::v2(CTU);
        let r = assess(&t, &v, SWITCH, &p).unwrap();
        assert_ne!(r.verdict, Verdict::Pass, "{:?}", r.reasons);
        assert!(r.ess < 50.0, "ess {}", r.ess);
        assert!(r.reasons.iter().any(|s| s.starts_with("ess")), "{:?}", r.reasons);
        assert!(r.std_err_batch.is_finite());
    }

    #[test]
    fn transient_fast_tier_reports_the_interval_instead_of_gating_on_it() {
        // The same quasi-periodic signal: stationary over whole periods, but
        // too few independent samples for the v2 limits. The fast-tier rule
        // passes it at the minimum window and still reports ESS and the error.
        let n = (8.0 * 200.0) as usize;
        let t: Vec<f64> = (0..n).map(|i| SWITCH + (i as f64 + 1.0) * CTU / 200.0).collect();
        let v: Vec<f64> = t.iter().map(|ti| 0.30 * (1.0 + 0.01 * (std::f64::consts::TAU * (ti - SWITCH) / (2.0 * CTU)).sin())).collect();
        let r = assess(&t, &v, SWITCH, &TransientParams::fast_tier(CTU)).unwrap();
        assert_eq!(r.verdict, Verdict::Pass, "{:?}", r.reasons);
        assert!(r.ess < 50.0 && r.std_err.is_finite() && r.std_err > 0.0);
        assert!((r.mean - 0.30).abs() < 1e-3);
        // A drift still fails under the fast-tier rule.
        let (t, mut v) = stationary(17.5, 5);
        for (ti, vi) in t.iter().zip(v.iter_mut()) {
            *vi += 0.30 * 0.02 * (ti - SWITCH) / CTU;
        }
        let r = assess(&t, &v, SWITCH, &TransientParams::fast_tier(CTU)).unwrap();
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(r.reasons.iter().any(|s| s.starts_with("slope")));
    }

    #[test]
    fn transient_mean_is_time_weighted_not_sample_weighted() {
        // Equal durations at 0.4 and 0.2, but the high level is sampled ten
        // times as densely: the sample mean is near 0.38, the true mean 0.30.
        let p = TransientParams::v2(CTU);
        let mut t = Vec::new();
        let mut v = Vec::new();
        let t0 = SWITCH + 2.0 * CTU;
        let block = CTU;
        for b in 0..8 {
            let (level, per) = if b % 2 == 0 { (0.4, 100) } else { (0.2, 10) };
            for i in 0..per {
                t.push(t0 + b as f64 * block + (i as f64 + 1.0) * block / per as f64);
                v.push(level);
            }
        }
        let r = assess(&t, &v, SWITCH, &p).unwrap();
        assert!((r.mean - 0.30).abs() < 0.01, "time-weighted mean {}", r.mean);
        assert!(r.resampled > r.samples / 2);
    }

    #[test]
    fn transient_half_step_agreement_uses_the_combined_interval() {
        let (t, v) = stationary(8.0, 11);
        let p = TransientParams::v2(CTU);
        let main = assess(&t, &v, SWITCH, &p).unwrap();
        let mut half = main.clone();
        half.mean = main.mean + 0.5 * (main.std_err.powi(2) * 2.0).sqrt();
        let (ok, diff, allowed) = dt_half_agrees(&main, &half, 0.95);
        assert!(ok && diff < allowed, "{diff} {allowed}");
        half.mean = main.mean * 1.03;
        let (ok, diff, allowed) = dt_half_agrees(&main, &half, 0.95);
        assert!(!ok && diff > allowed);
    }

    const LOG: &str = "\
Courant Number mean: 0.05 max: 1.2
deltaT = 0.0004
Time = 10.0004

PIMPLE: iteration 1
smoothSolver:  Solving for Ux, Initial residual = 0.01, Final residual = 1e-08, No Iterations 3
smoothSolver:  Solving for Uy, Initial residual = 0.02, Final residual = 1e-08, No Iterations 3
smoothSolver:  Solving for Uz, Initial residual = 0.015, Final residual = 1e-08, No Iterations 3
GAMG:  Solving for p, Initial residual = 0.1, Final residual = 0.009, No Iterations 4
GAMG:  Solving for p, Initial residual = 0.02, Final residual = 1e-09, No Iterations 9
smoothSolver:  Solving for omega, Initial residual = 0.004, Final residual = 1e-08, No Iterations 2
smoothSolver:  Solving for k, Initial residual = 0.003, Final residual = 1e-08, No Iterations 2
PIMPLE: iteration 2
smoothSolver:  Solving for Ux, Initial residual = 0.0005, Final residual = 1e-08, No Iterations 3
smoothSolver:  Solving for Uy, Initial residual = 0.0009, Final residual = 1e-08, No Iterations 3
smoothSolver:  Solving for Uz, Initial residual = 0.0007, Final residual = 1e-08, No Iterations 3
GAMG:  Solving for p, Initial residual = 0.003, Final residual = 0.0002, No Iterations 4
GAMG:  Solving for p, Initial residual = 5e-05, Final residual = 1e-09, No Iterations 9
smoothSolver:  Solving for omega, Initial residual = 0.0002, Final residual = 1e-08, No Iterations 2
smoothSolver:  Solving for k, Initial residual = 0.0001, Final residual = 1e-08, No Iterations 2
ExecutionTime = 2.5 s  ClockTime = 3 s

Courant Number mean: 0.05 max: 1.3
deltaT = 0.0004
Time = 10.0008

PIMPLE: iteration 1
smoothSolver:  Solving for Ux, Initial residual = 0.01, Final residual = 1e-08, No Iterations 3
smoothSolver:  Solving for Uy, Initial residual = 0.01, Final residual = 1e-08, No Iterations 3
smoothSolver:  Solving for Uz, Initial residual = 0.01, Final residual = 1e-08, No Iterations 3
GAMG:  Solving for p, Initial residual = 0.1, Final residual = 0.009, No Iterations 4
GAMG:  Solving for p, Initial residual = 0.02, Final residual = 1e-09, No Iterations 9
smoothSolver:  Solving for omega, Initial residual = 0.004, Final residual = 1e-08, No Iterations 2
smoothSolver:  Solving for k, Initial residual = 0.003, Final residual = 1e-08, No Iterations 2
PIMPLE: iteration 2
smoothSolver:  Solving for Ux, Initial residual = 0.005, Final residual = 1e-08, No Iterations 3
smoothSolver:  Solving for Uy, Initial residual = 0.005, Final residual = 1e-08, No Iterations 3
smoothSolver:  Solving for Uz, Initial residual = 0.005, Final residual = 1e-08, No Iterations 3
GAMG:  Solving for p, Initial residual = 0.003, Final residual = 0.0002, No Iterations 4
GAMG:  Solving for p, Initial residual = 0.0005, Final residual = 1e-09, No Iterations 9
smoothSolver:  Solving for omega, Initial residual = 0.002, Final residual = 1e-08, No Iterations 2
smoothSolver:  Solving for k, Initial residual = 0.0001, Final residual = 1e-08, No Iterations 2
ExecutionTime = 5.0 s  ClockTime = 6 s
End
";

    #[test]
    fn transient_log_parser_reads_first_and_final_residuals_per_step() {
        let steps = parse_pimple_log(LOG);
        assert_eq!(steps.len(), 2);
        let s = &steps[0];
        assert!((s.time_s - 10.0004).abs() < 1e-12);
        assert_eq!(s.dt_s, Some(0.0004));
        assert_eq!((s.co_mean, s.co_max), (Some(0.05), Some(1.2)));
        assert_eq!(s.outer_iterations, 2);
        assert_eq!((s.p_first, s.p_final), (Some(0.1), Some(5e-05)));
        assert_eq!((s.u_first, s.u_final), (Some(0.02), Some(0.0009)));
        assert_eq!((s.k_first, s.k_final), (Some(0.003), Some(0.0001)));
        assert_eq!((s.omega_first, s.omega_final), (Some(0.004), Some(0.0002)));
    }

    #[test]
    fn transient_step_rule_counts_pass_fractions_and_fails_closed() {
        let steps = parse_pimple_log(LOG);
        let r = step_convergence(&steps, 0.0, &StepRule::default()).unwrap();
        assert_eq!(r.steps, 2);
        // Step 1 meets every rule; step 2 misses U (0.3 orders) and omega
        // (0.3 orders) but meets p (0.1 -> 5e-4, 2.3 orders) and k (1.5).
        assert!((r.p_pass_fraction - 1.0).abs() < 1e-12);
        assert!((r.u_pass_fraction - 0.5).abs() < 1e-12);
        assert!((r.k_pass_fraction - 1.0).abs() < 1e-12);
        assert!((r.omega_pass_fraction - 0.5).abs() < 1e-12);
        assert!(!r.passed);
        assert!(r.reasons.iter().any(|s| s.starts_with("U drop")) && r.reasons.iter().any(|s| s.starts_with("omega drop")), "{:?}", r.reasons);
        assert!(!r.reasons.iter().any(|s| s.starts_with("p ")), "{:?}", r.reasons);
        assert!((r.p_final_median - 5e-05).abs() < 1e-12 || (r.p_final_median - 5e-04).abs() < 1e-12);
        assert_eq!(r.co_max_peak, 1.3);
        assert_eq!((r.dt_min_s, r.dt_max_s), (0.0004, 0.0004));
        // Only the first step counted: everything passes.
        let first = step_convergence(&steps[..1], 0.0, &StepRule::default()).unwrap();
        assert!(first.passed, "{:?}", first.reasons);
        // No steps after the start time is an error, not a pass.
        assert!(step_convergence(&steps, 11.0, &StepRule::default()).is_err());
    }
}

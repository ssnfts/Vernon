//! Strict statistical convergence gate for a force-coefficient history.
//!
//! Pure functions over slices; no I/O. The gate looks at the trailing window
//! of a coefficient history and passes only when all of these hold:
//!
//! * enough samples in total (default 2000);
//! * the window is stationary: no statistically *and* practically significant
//!   linear trend, no drift of the mean or the standard deviation between the
//!   two halves;
//! * the window mean is known well enough: the effective sample size (ESS)
//!   is large enough and the standard error of the mean is small relative to
//!   the mean;
//! * every residual field dropped by the required number of orders of
//!   magnitude.
//!
//! Methods (textbook statistics, no vendor source consulted):
//! * ordinary least squares slope with a z-style test using the standard
//!   error inflated by the autocorrelation (`n / ESS`), so a correlated
//!   series cannot pass a trend test it would fail for independent data;
//! * ESS from the autocorrelation function with the initial positive
//!   sequence estimator of Geyer, "Practical Markov Chain Monte Carlo",
//!   Statistical Science 7(4), 1992 (pairs of lags summed while positive,
//!   made monotone); the exact equation numbering is UNVERIFIED;
//! * normal quantile for the confidence level by bisection on the
//!   Abramowitz-Stegun 7.1.26 erf approximation (|error| < 1.5e-7).
//!
//! The trend test uses the normal quantile rather than Student's t: with a
//! window of hundreds of samples the difference is negligible, and the
//! default practical-significance tolerance dominates anyway.

/// Tunables of the gate. [`Default`] gives the strict production values.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvergenceParams {
    /// Trailing window length N, in samples.
    pub window: usize,
    /// Minimum number of samples in the whole history.
    pub min_samples: usize,
    /// Two-sided confidence level of the trend and half-drift tests, in (0, 1).
    pub confidence: f64,
    /// Largest accepted trend, per 1000 iterations, relative to
    /// `max(|mean|, std)`. A trend must be both significant and larger than
    /// this to fail.
    pub max_rel_slope_per_1000: f64,
    /// Smallest accepted effective sample size of the window.
    pub min_ess: f64,
    /// Largest accepted standard error of the mean relative to `|mean|`
    /// (checked only when `|mean|` is not negligible).
    pub max_rel_std_err: f64,
    /// Largest accepted relative change of the standard deviation between the
    /// first and the second half of the window.
    pub max_std_drift: f64,
    /// Required drop of every residual field, in orders of magnitude.
    pub min_residual_drop_orders: f64,
}

impl Default for ConvergenceParams {
    fn default() -> Self {
        Self {
            window: 1000,
            min_samples: 2000,
            confidence: 0.99,
            max_rel_slope_per_1000: 0.005,
            min_ess: 50.0,
            max_rel_std_err: 0.01,
            max_std_drift: 0.5,
            min_residual_drop_orders: 3.0,
        }
    }
}

/// Outcome of [`check_convergence`].
#[derive(Debug, Clone, PartialEq)]
pub struct ConvergenceReport {
    /// Samples in the whole history.
    pub samples: usize,
    /// Window length actually used (shorter than requested for a short history).
    pub window: usize,
    /// Linear trend of the window, coefficient units per 1000 iterations.
    pub slope_per_1000_iters: f64,
    /// Mean of the window.
    pub mean: f64,
    /// Standard error of the window mean, corrected for autocorrelation.
    pub std_err: f64,
    /// Effective sample size of the window.
    pub ess: f64,
    /// Smallest residual drop over all fields, in orders of magnitude.
    pub residual_drop_orders: f64,
    /// True when no criterion failed.
    pub passed: bool,
    /// One human-readable line per failed criterion.
    pub reasons: Vec<String>,
}

/// Two-sided standard normal quantile `z` with `P(|Z| <= z) = confidence`.
pub fn two_sided_z(confidence: f64) -> f64 {
    let c = confidence.clamp(1e-9, 1.0 - 1e-9);
    let (mut lo, mut hi) = (0.0_f64, 10.0_f64);
    for _ in 0..80 {
        let mid = 0.5 * (lo + hi);
        // P(|Z| <= mid) = erf(mid / sqrt 2)
        if erf(mid / std::f64::consts::SQRT_2) < c {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Abramowitz & Stegun 7.1.26, |error| < 1.5e-7.
fn erf(x: f64) -> f64 {
    let s = x.signum();
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let poly = t
        * (0.254_829_592
            + t * (-0.284_496_736 + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    s * (1.0 - poly * (-x * x).exp())
}

fn mean_of(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

/// Sample standard deviation (n - 1 denominator); 0 for fewer than 2 values.
fn std_of(v: &[f64]) -> f64 {
    if v.len() < 2 {
        return 0.0;
    }
    let m = mean_of(v);
    (v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (v.len() - 1) as f64).sqrt()
}

/// Effective sample size by Geyer's initial positive sequence estimator.
///
/// `tau = -1 + 2 * sum(Gamma_m)` with `Gamma_m = rho(2m) + rho(2m+1)`, summed
/// until the first non-positive pair and forced non-increasing; `ess = n / tau`
/// clamped to `[1, n]` (anti-correlated series are not credited above `n`).
/// A series with zero variance counts as `n` independent samples.
pub fn effective_sample_size(v: &[f64]) -> f64 {
    let n = v.len();
    if n < 4 {
        return n as f64;
    }
    let m = mean_of(v);
    let d: Vec<f64> = v.iter().map(|x| x - m).collect();
    let g0: f64 = d.iter().map(|x| x * x).sum::<f64>() / n as f64;
    if g0 <= 1e-300 {
        return n as f64;
    }
    let rho = |k: usize| -> f64 {
        let s: f64 = (0..n - k).map(|i| d[i] * d[i + k]).sum();
        s / n as f64 / g0
    };
    let mut sum = 0.0;
    let mut prev = f64::INFINITY;
    let mut k = 0;
    while k + 1 < n {
        let pair = rho(k) + rho(k + 1);
        if pair <= 0.0 {
            break;
        }
        let pair = pair.min(prev);
        sum += pair;
        prev = pair;
        k += 2;
    }
    let tau = (-1.0 + 2.0 * sum).max(1.0);
    (n as f64 / tau).clamp(1.0, n as f64)
}

/// Least-squares line through `(x, y)`: `(slope, residuals)`.
fn linear_fit(x: &[f64], y: &[f64]) -> (f64, Vec<f64>) {
    let n = x.len() as f64;
    let (xm, ym) = (x.iter().sum::<f64>() / n, y.iter().sum::<f64>() / n);
    let sxx: f64 = x.iter().map(|a| (a - xm) * (a - xm)).sum();
    let sxy: f64 = x.iter().zip(y).map(|(a, b)| (a - xm) * (b - ym)).sum();
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let res = x
        .iter()
        .zip(y)
        .map(|(a, b)| b - ym - slope * (a - xm))
        .collect();
    (slope, res)
}

/// Orders of magnitude dropped by one residual history: `log10(first / last)`.
/// `None` when it has fewer than 2 values or an end value is not a positive
/// finite number.
fn residual_drop(h: &[f64]) -> Option<f64> {
    let (first, last) = (*h.first()?, *h.last()?);
    if h.len() < 2 || !(first.is_finite() && last.is_finite()) || first <= 0.0 || last <= 0.0 {
        return None;
    }
    Some((first / last).log10())
}

/// Run the gate on a coefficient history `(iteration, value)` and the residual
/// histories `(field name, values)`.
pub fn check_convergence(
    history: &[(u64, f64)],
    residuals: &[(&str, &[f64])],
    p: &ConvergenceParams,
) -> ConvergenceReport {
    let mut reasons = Vec::new();
    let samples = history.len();
    if samples < p.min_samples {
        reasons.push(format!(
            "samples: {samples} < required {} iterations of history",
            p.min_samples
        ));
    }
    let w = p.window.min(samples);
    let win = &history[samples - w..];
    let mut report = ConvergenceReport {
        samples,
        window: w,
        slope_per_1000_iters: 0.0,
        mean: f64::NAN,
        std_err: f64::NAN,
        ess: 0.0,
        residual_drop_orders: 0.0,
        passed: false,
        reasons: Vec::new(),
    };

    if w < 8 {
        reasons.push(format!("window: only {w} samples, need at least 8"));
    } else if win.iter().any(|(_, v)| !v.is_finite()) {
        reasons.push("window: contains a non-finite value".into());
    } else {
        let x: Vec<f64> = win.iter().map(|(i, _)| *i as f64).collect();
        let y: Vec<f64> = win.iter().map(|(_, v)| *v).collect();
        window_tests(&x, &y, p, &mut report, &mut reasons);
    }

    // Residual drop: the smallest over all fields must reach the requirement.
    let mut min_drop = f64::INFINITY;
    for (name, h) in residuals {
        match residual_drop(h) {
            Some(d) => {
                min_drop = min_drop.min(d);
                if d < p.min_residual_drop_orders {
                    reasons.push(format!(
                        "residual: {name} dropped {d:.2} orders < required {:.2}",
                        p.min_residual_drop_orders
                    ));
                }
            }
            None => {
                min_drop = f64::NEG_INFINITY;
                reasons.push(format!("residual: {name} history is unusable"));
            }
        }
    }
    if residuals.is_empty() {
        if p.min_residual_drop_orders > 0.0 {
            reasons.push("residual: no residual histories supplied".into());
        }
    } else {
        report.residual_drop_orders = min_drop;
    }

    report.passed = reasons.is_empty();
    report.reasons = reasons;
    report
}

/// Stationarity and precision tests on the window (`x` iterations, `y` values).
fn window_tests(
    x: &[f64],
    y: &[f64],
    p: &ConvergenceParams,
    r: &mut ConvergenceReport,
    reasons: &mut Vec<String>,
) {
    let n = y.len();
    let z = two_sided_z(p.confidence);
    let mean = mean_of(y);
    let std = std_of(y);
    let ess = effective_sample_size(y);
    r.mean = mean;
    r.ess = ess;
    r.std_err = std / ess.sqrt();

    // Linear trend. Significance uses the autocorrelation of the detrended
    // residuals so a ramp cannot hide behind its own correlation.
    let (slope, res) = linear_fit(x, y);
    r.slope_per_1000_iters = slope * 1000.0;
    let ess_res = effective_sample_size(&res);
    let s2 = res.iter().map(|e| e * e).sum::<f64>() / (n - 2) as f64;
    let xm = mean_of(x);
    let sxx: f64 = x.iter().map(|a| (a - xm) * (a - xm)).sum();
    let se_slope = (s2 / sxx).sqrt() * (n as f64 / ess_res).sqrt();
    let scale = mean.abs().max(std).max(1e-300);
    let rel = r.slope_per_1000_iters.abs() / scale;
    if slope.abs() > z * se_slope && rel > p.max_rel_slope_per_1000 {
        reasons.push(format!(
            "slope: {:.4e} per 1000 iterations ({:.2}% of scale) is significant at {:.1}% confidence",
            r.slope_per_1000_iters,
            rel * 100.0,
            p.confidence * 100.0
        ));
    }

    // Drift between the window halves.
    let (h1, h2) = y.split_at(n / 2);
    let diff = (mean_of(h2) - mean_of(h1)).abs();
    let se_diff = 2.0 * std / ess.sqrt(); // sqrt(2) * std / sqrt(ess / 2)
    if diff > z * se_diff && diff / scale > p.max_rel_slope_per_1000 {
        reasons.push(format!(
            "mean drift: halves differ by {diff:.4e}, more than {:.1}% confidence allows",
            p.confidence * 100.0
        ));
    }
    let (s1, s2d) = (std_of(h1), std_of(h2));
    let sd_ref = s1.max(s2d);
    if sd_ref > 0.0 && (s2d - s1).abs() / sd_ref > p.max_std_drift {
        reasons.push(format!(
            "std drift: {s1:.4e} -> {s2d:.4e} between window halves"
        ));
    }

    // Precision of the mean.
    if ess < p.min_ess {
        reasons.push(format!(
            "ess: effective sample size {ess:.1} < required {:.1} (oscillating or strongly correlated)",
            p.min_ess
        ));
    }
    if mean.abs() > 1e-12 && r.std_err / mean.abs() > p.max_rel_std_err {
        reasons.push(format!(
            "std_err: {:.4e} is {:.2}% of the mean, limit {:.2}%",
            r.std_err,
            100.0 * r.std_err / mean.abs(),
            100.0 * p.max_rel_std_err
        ));
    }
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

    fn hist(v: impl Iterator<Item = f64>) -> Vec<(u64, f64)> {
        v.enumerate().map(|(i, y)| (i as u64, y)).collect()
    }

    fn decay(n: usize, orders: f64) -> Vec<f64> {
        (0..n)
            .map(|i| 10f64.powf(-orders * i as f64 / (n - 1) as f64))
            .collect()
    }

    #[test]
    fn convergence_quantile_matches_table() {
        assert!((two_sided_z(0.95) - 1.959_964).abs() < 1e-3);
        assert!((two_sided_z(0.99) - 2.575_829).abs() < 1e-3);
    }

    #[test]
    fn convergence_constant_series_passes() {
        let h = hist(std::iter::repeat_n(3.5, 3000));
        let r = decay(3000, 4.0);
        let rep = check_convergence(&h, &[("p", &r)], &ConvergenceParams::default());
        assert!(rep.passed, "{:?}", rep.reasons);
        assert_eq!(rep.samples, 3000);
        assert_eq!(rep.window, 1000);
        assert!((rep.mean - 3.5).abs() < 1e-12);
        assert_eq!(rep.std_err, 0.0);
        assert_eq!(rep.ess, 1000.0);
        assert!(rep.slope_per_1000_iters.abs() < 1e-9);
        assert!((rep.residual_drop_orders - 4.0).abs() < 1e-9);
    }

    #[test]
    fn convergence_ramp_fails_with_slope_reason() {
        // y = 2 + 0.001 i: slope 1.0 per 1000 iterations; the window is
        // i = 2000..2999 so its mean is 2 + 0.001 * 2499.5 = 4.4995.
        let h = hist((0..3000).map(|i| 2.0 + 0.001 * i as f64));
        let r = decay(3000, 4.0);
        let rep = check_convergence(&h, &[("p", &r)], &ConvergenceParams::default());
        assert!(!rep.passed);
        assert!((rep.slope_per_1000_iters - 1.0).abs() < 1e-9);
        assert!((rep.mean - 4.4995).abs() < 1e-9);
        assert!(rep.reasons.iter().any(|s| s.starts_with("slope")));
    }

    #[test]
    fn convergence_white_noise_passes_with_ess_near_n() {
        let v: Vec<f64> = noise(3000, 42).iter().map(|e| 1.0 + 0.02 * e).collect();
        let h = hist(v.into_iter());
        let r = decay(3000, 5.0);
        let rep = check_convergence(&h, &[("p", &r)], &ConvergenceParams::default());
        assert!(rep.passed, "{:?}", rep.reasons);
        assert!(rep.ess > 600.0, "ess {}", rep.ess);
        assert!((rep.mean - 1.0).abs() < 1e-3);
        // uniform on [-0.01, 0.01): std = 0.02 / sqrt(12) = 5.77e-3
        assert!((rep.std_err - 5.77e-3 / rep.ess.sqrt()).abs() < 5e-5);
    }

    #[test]
    fn convergence_period_200_oscillation_is_unsteady() {
        let h = hist(
            (0..3000).map(|i| 1.0 + 0.01 * (std::f64::consts::TAU * i as f64 / 200.0).sin()),
        );
        let r = decay(3000, 5.0);
        let rep = check_convergence(&h, &[("p", &r)], &ConvergenceParams::default());
        assert!(!rep.passed);
        assert!(rep.ess < 50.0, "ess {}", rep.ess);
        assert!(rep.reasons.iter().any(|s| s.starts_with("ess")));
        // five whole periods: the window mean is exactly the offset
        assert!((rep.mean - 1.0).abs() < 1e-9);
    }

    #[test]
    fn convergence_too_few_samples_fails() {
        let h = hist(std::iter::repeat_n(1.0, 1500));
        let r = decay(1500, 4.0);
        let rep = check_convergence(&h, &[("p", &r)], &ConvergenceParams::default());
        assert!(!rep.passed);
        assert!(rep.reasons.iter().any(|s| s.starts_with("samples")));
    }

    #[test]
    fn convergence_residual_drop_names_the_field() {
        let h = hist(std::iter::repeat_n(1.0, 3000));
        let good = decay(3000, 4.0);
        let bad = decay(3000, 1.0);
        let rep = check_convergence(
            &h,
            &[("p", &good), ("Ux", &bad)],
            &ConvergenceParams::default(),
        );
        assert!(!rep.passed);
        assert!((rep.residual_drop_orders - 1.0).abs() < 1e-9);
        assert_eq!(rep.reasons.len(), 1);
        assert!(rep.reasons[0].contains("Ux"));
        let none = check_convergence(&h, &[], &ConvergenceParams::default());
        assert!(none.reasons.iter().any(|s| s.starts_with("residual")));
    }
}

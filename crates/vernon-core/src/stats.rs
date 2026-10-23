//! Shared sample statistics, so numbers that one stage gates and another
//! compares are computed the same way everywhere.
//!
//! - Percentiles are **nearest-rank**: the value at 1-based rank `ceil(q N)`
//!   of the ascending sample, with no interpolation. A 1e-9 guard keeps
//!   products such as `0.95 * 1000` from rounding up to the next rank.
//! - Standard deviations use the **sample** divisor `N - 1` (0 for `N < 2`).

/// One-line statement of the convention, for report notes and help text.
pub const CONVENTION: &str = "percentiles are nearest-rank (the sample value at rank ceil(q N), no interpolation); standard deviations use the sample N - 1 divisor";

/// Nearest-rank percentile of an ascending sorted slice, `q` in 0..=1
/// (clamped; NaN reads as the median). Empty input gives 0.
pub fn nearest_rank(sorted: &[f64], q: f64) -> f64 {
    let n = sorted.len();
    if n == 0 {
        return 0.0;
    }
    let q = if q.is_nan() { 0.5 } else { q.clamp(0.0, 1.0) };
    let rank = (q * n as f64 - 1e-9).ceil();
    let idx = (rank.max(1.0) as usize).min(n);
    sorted[idx - 1]
}

/// Nearest-rank percentile with `p` in percent (0..=100).
pub fn percentile_pct(sorted: &[f64], p: f64) -> f64 {
    nearest_rank(sorted, p / 100.0)
}

/// Arithmetic mean; 0 for an empty slice.
pub fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

/// Sample standard deviation (N - 1 divisor, two-pass); 0 for fewer than two values.
pub fn sample_std(values: &[f64]) -> f64 {
    let n = values.len();
    if n < 2 {
        return 0.0;
    }
    let m = mean(values);
    let ss: f64 = values.iter().map(|v| (v - m) * (v - m)).sum();
    (ss / (n as f64 - 1.0)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_small_samples() {
        let v: Vec<f64> = (1..=10).map(f64::from).collect();
        assert_eq!(nearest_rank(&v, 0.95), 10.0);
        assert_eq!(nearest_rank(&v, 0.5), 5.0);
        assert_eq!(nearest_rank(&v, 0.0), 1.0);
        assert_eq!(nearest_rank(&v, 1.0), 10.0);
        let v: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(nearest_rank(&v, 0.95), 19.0);
        assert_eq!(percentile_pct(&v, 95.0), 19.0);
        // the guard: 0.95 * 1000 is 950.0000000000001 in floating point
        let v: Vec<f64> = (1..=1000).map(f64::from).collect();
        assert_eq!(nearest_rank(&v, 0.95), 950.0);
        assert_eq!(nearest_rank(&[], 0.5), 0.0);
        assert_eq!(nearest_rank(&v, f64::NAN), 500.0);
    }

    #[test]
    fn sample_std_uses_n_minus_one() {
        let v = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        // population std is 2; the sample std is sqrt(32 / 7)
        assert!((sample_std(&v) - (32.0f64 / 7.0).sqrt()).abs() < 1e-12);
        assert_eq!(sample_std(&[3.0]), 0.0);
        assert_eq!(mean(&v), 5.0);
    }
}

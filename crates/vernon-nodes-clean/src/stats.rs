//! Deviation statistics: moments, nearest-rank percentiles, tolerance
//! fractions and bands, histograms and the colour ramps used for deviation
//! maps. All functions are pure and work on plain `f64` slices.

use glam::DVec3;

/// Moments of a sample (signed values).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Moments {
    pub n: usize,
    pub mean: f64,
    /// Sample standard deviation (N - 1 divisor; 0 for N < 2).
    pub std: f64,
    pub rms: f64,
    pub min: f64,
    pub max: f64,
    pub mean_abs: f64,
    pub max_abs: f64,
}

/// Moments of `values`, optionally weighted. Non-finite values must have
/// been removed by the caller.
pub fn moments(values: &[f64], weights: Option<&[f64]>) -> Moments {
    let n = values.len();
    if n == 0 {
        return Moments::default();
    }
    let w = |i: usize| weights.map_or(1.0, |w| w[i]);
    let (mut ws, mut s1, mut s2, mut sa) = (0.0, 0.0, 0.0, 0.0);
    let (mut mn, mut mx, mut ma) = (f64::INFINITY, f64::NEG_INFINITY, 0.0f64);
    for (i, &v) in values.iter().enumerate() {
        let wi = w(i);
        ws += wi;
        s1 += wi * v;
        s2 += wi * v * v;
        sa += wi * v.abs();
        mn = mn.min(v);
        mx = mx.max(v);
        ma = ma.max(v.abs());
    }
    if ws <= 0.0 {
        return Moments {
            n,
            ..Default::default()
        };
    }
    let mean = s1 / ws;
    // Two-pass variance for stability.
    let mut ss = 0.0;
    for (i, &v) in values.iter().enumerate() {
        ss += w(i) * (v - mean) * (v - mean);
    }
    let std = match weights {
        None => {
            if n > 1 {
                (ss / (n as f64 - 1.0)).sqrt()
            } else {
                0.0
            }
        }
        Some(_) => (ss / ws).sqrt(),
    };
    Moments {
        n,
        mean,
        std,
        rms: (s2 / ws).sqrt(),
        min: mn,
        max: mx,
        mean_abs: sa / ws,
        max_abs: ma,
    }
}

/// Nearest-rank percentile of an ascending sorted slice: the value at
/// 1-based rank `ceil(q * N)`, `q` in 0..=1. Empty input gives 0.
pub fn nearest_rank(sorted: &[f64], q: f64) -> f64 {
    let n = sorted.len();
    if n == 0 {
        return 0.0;
    }
    // Tiny guard against products such as 0.95 * 1000 landing above 950.
    let rank = (q * n as f64 - 1e-9).ceil();
    let idx = (rank.max(1.0) as usize).min(n);
    sorted[idx - 1]
}

/// Weighted nearest-rank percentile: smallest value whose cumulative weight
/// reaches `q` of the total. `pairs` must be sorted ascending by value.
pub fn weighted_rank(pairs: &[(f64, f64)], q: f64) -> f64 {
    if pairs.is_empty() {
        return 0.0;
    }
    let total: f64 = pairs.iter().map(|p| p.1).sum();
    let target = q * total * (1.0 - 1e-12);
    let mut cum = 0.0;
    for &(v, w) in pairs {
        cum += w;
        if cum >= target {
            return v;
        }
    }
    pairs[pairs.len() - 1].0
}

/// Share of `abs_sorted` (ascending absolute values) that is `<= tol`.
pub fn fraction_within(abs_sorted: &[f64], tol: f64) -> f64 {
    if abs_sorted.is_empty() {
        return 0.0;
    }
    let c = abs_sorted.partition_point(|&v| v <= tol);
    c as f64 / abs_sorted.len() as f64
}

/// Weighted share of values with `|v| <= tol`.
pub fn weighted_fraction_within(values: &[f64], weights: &[f64], tol: f64) -> f64 {
    let mut num = 0.0;
    let mut den = 0.0;
    for (v, w) in values.iter().zip(weights) {
        den += w;
        if v.abs() <= tol {
            num += w;
        }
    }
    if den > 0.0 {
        num / den
    } else {
        0.0
    }
}

/// One tolerance band `[lo, hi)` (last band closed on the right).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub lo: f64,
    pub hi: f64,
    pub count: usize,
    pub percent: f64,
}

/// Band counts for ascending `edges`; returns (bands, below, above).
pub fn bands(values: &[f64], edges: &[f64]) -> (Vec<Band>, usize, usize) {
    let n = values.len().max(1) as f64;
    let mut counts = vec![0usize; edges.len().saturating_sub(1)];
    let (mut below, mut above) = (0usize, 0usize);
    if edges.len() < 2 {
        return (vec![], 0, 0);
    }
    let last = edges[edges.len() - 1];
    for &v in values {
        if v < edges[0] {
            below += 1;
        } else if v > last {
            above += 1;
        } else {
            let idx = edges.partition_point(|&e| e <= v);
            let b = idx.saturating_sub(1).min(counts.len() - 1);
            counts[b] += 1;
        }
    }
    let out = counts
        .iter()
        .enumerate()
        .map(|(i, &c)| Band {
            lo: edges[i],
            hi: edges[i + 1],
            count: c,
            percent: 100.0 * c as f64 / n,
        })
        .collect();
    (out, below, above)
}

/// Histogram counts: `bins` equal bins over `[lo, hi]`,
/// `idx = floor((v - lo) / (hi - lo) * bins)`, last bin right-closed.
/// Returns (counts, below, above).
pub fn histogram(values: &[f64], lo: f64, hi: f64, bins: usize) -> (Vec<u64>, u64, u64) {
    let mut counts = vec![0u64; bins];
    let (mut below, mut above) = (0u64, 0u64);
    let span = hi - lo;
    if bins == 0 || (span.is_nan() || span <= 0.0) {
        return (counts, 0, 0);
    }
    for &v in values {
        if v < lo {
            below += 1;
        } else if v > hi {
            above += 1;
        } else {
            // 1e-9 bin widths of slack keep values that sit on an edge in
            // the upper bin despite binary rounding.
            let t = (v - lo) / span * bins as f64 + 1e-9;
            let idx = (t.floor() as usize).min(bins - 1);
            counts[idx] += 1;
        }
    }
    (counts, below, above)
}

/// Colour scale for deviation maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    /// blue - cyan - green - yellow - red with zero on green.
    Diverging,
    /// hue ramp from blue through green to red.
    Spectrum,
    /// black to white.
    Grey,
}

impl Scale {
    pub fn parse(s: &str) -> Option<Scale> {
        match s {
            "diverging" => Some(Scale::Diverging),
            "spectrum" => Some(Scale::Spectrum),
            "grey" => Some(Scale::Grey),
            _ => None,
        }
    }
}

const DIVERGING: [[f64; 3]; 5] = [
    [0.05, 0.20, 0.90],
    [0.00, 0.80, 0.90],
    [0.10, 0.75, 0.20],
    [0.95, 0.90, 0.10],
    [0.90, 0.10, 0.10],
];

fn hsv(h: f64, s: f64, v: f64) -> DVec3 {
    let h6 = (h.rem_euclid(1.0)) * 6.0;
    let i = h6.floor();
    let f = h6 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as i32 {
        0 => DVec3::new(v, t, p),
        1 => DVec3::new(q, v, p),
        2 => DVec3::new(p, v, t),
        3 => DVec3::new(p, q, v),
        4 => DVec3::new(t, p, v),
        _ => DVec3::new(v, p, q),
    }
}

/// Colour for a position `t` in 0..=1 on `scale`.
pub fn ramp(scale: Scale, t: f64) -> DVec3 {
    let t = t.clamp(0.0, 1.0);
    match scale {
        Scale::Grey => DVec3::splat(t),
        Scale::Spectrum => hsv((1.0 - t) * (240.0 / 360.0), 0.9, 0.95),
        Scale::Diverging => {
            let x = t * 4.0;
            let i = (x.floor() as usize).min(3);
            let f = x - i as f64;
            let (a, b) = (DIVERGING[i], DIVERGING[i + 1]);
            DVec3::new(
                a[0] + (b[0] - a[0]) * f,
                a[1] + (b[1] - a[1]) * f,
                a[2] + (b[2] - a[2]) * f,
            )
        }
    }
}

/// Map a value to 0..=1 for a range `[lo, hi]`, quantised to `steps` bands
/// when `steps > 0`.
pub fn range_t(v: f64, lo: f64, hi: f64, steps: usize) -> f64 {
    let span = hi - lo;
    let t = if span > 0.0 { ((v - lo) / span).clamp(0.0, 1.0) } else { 0.5 };
    if steps >= 2 {
        let k = ((t * steps as f64).floor() as usize).min(steps - 1);
        (k as f64 + 0.5) / steps as f64
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted_abs_1000() -> Vec<f64> {
        (1..=1000).map(|k| k as f64 / 1000.0).collect()
    }

    #[test]
    fn golden_11_statistics() {
        let v = sorted_abs_1000();
        let m = moments(&v, None);
        assert!((nearest_rank(&v, 0.50) - 0.500).abs() < 1e-12);
        assert!((nearest_rank(&v, 0.95) - 0.950).abs() < 1e-12);
        assert!((m.mean - 0.5005).abs() < 1e-12);
        assert!((m.rms - 0.577783).abs() < 1e-6);
        assert!((fraction_within(&v, 0.5) - 0.5).abs() < 1e-12);
        // the 0.95-within-0.5 gate fails
        assert!(fraction_within(&v, 0.5) < 0.95);
    }

    #[test]
    fn golden_12_report_form() {
        let mut v = vec![0.2; 9500];
        v.extend(vec![0.8; 500]);
        v.sort_by(f64::total_cmp);
        let m = moments(&v, None);
        assert!(fraction_within(&v, 0.5) >= 0.95);
        assert!((m.mean_abs - 0.23).abs() < 1e-12);
        assert!((m.rms - 0.26458).abs() < 1e-4);
        assert!((nearest_rank(&v, 0.95) - 0.2).abs() < 1e-12);
        assert!((nearest_rank(&v, 0.99) - 0.8).abs() < 1e-12);
        let mut w = vec![0.2; 9499];
        w.extend(vec![0.8; 501]);
        w.sort_by(f64::total_cmp);
        assert!(fraction_within(&w, 0.5) < 0.95);
    }

    #[test]
    fn golden_13_histogram_edges() {
        let v = sorted_abs_1000();
        let (c, below, above) = histogram(&v, 0.0, 1.0, 10);
        assert_eq!(c, vec![99, 100, 100, 100, 100, 100, 100, 100, 100, 101]);
        assert_eq!((below, above), (0, 0));
    }

    #[test]
    fn bands_cover_everything() {
        let v = [-1.0, -0.2, 0.0, 0.1, 0.5, 2.0];
        let (b, below, above) = bands(&v, &[-0.5, 0.0, 0.5]);
        assert_eq!(below, 1);
        assert_eq!(above, 1);
        assert_eq!(b[0].count, 1); // -0.2
        assert_eq!(b[1].count, 3); // 0.0, 0.1, 0.5 (last band closed)
        assert!((b[1].percent - 50.0).abs() < 1e-9);
    }

    #[test]
    fn golden_14_colour_map() {
        let l = 0.5;
        let t = |d: f64| range_t(d, -l, l, 0);
        assert_eq!(t(-0.5), 0.0);
        assert_eq!(t(0.0), 0.5);
        assert_eq!(t(0.25), 0.75);
        assert_eq!(t(2.0), 1.0);
        let green = ramp(Scale::Diverging, 0.5);
        assert!(green.y > green.x && green.y > green.z);
        let blue = ramp(Scale::Diverging, 0.0);
        assert!(blue.z > blue.x && blue.z > blue.y);
        let red = ramp(Scale::Diverging, 1.0);
        assert!(red.x > red.y && red.x > red.z);
        let yellow = ramp(Scale::Diverging, 0.75);
        assert!(yellow.x > 0.8 && yellow.y > 0.8 && yellow.z < 0.3);
    }

    #[test]
    fn stepped_ramp_quantises() {
        let a = range_t(0.01, 0.0, 1.0, 4);
        let b = range_t(0.20, 0.0, 1.0, 4);
        assert_eq!(a, b);
        assert!((a - 0.125).abs() < 1e-12);
        assert!((range_t(1.0, 0.0, 1.0, 4) - 0.875).abs() < 1e-12);
    }

    #[test]
    fn weighted_rank_matches_unweighted_for_equal_weights() {
        let v = sorted_abs_1000();
        let pairs: Vec<(f64, f64)> = v.iter().map(|&x| (x, 2.0)).collect();
        assert_eq!(weighted_rank(&pairs, 0.95), nearest_rank(&v, 0.95));
        assert_eq!(weighted_rank(&pairs, 0.5), nearest_rank(&v, 0.5));
    }
}

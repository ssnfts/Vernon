//! Deterministic property tests for `vernon_core::units` and `vernon_core::stats`.
//! The random source is a seeded xorshift64* written here (no extra crates).
//!
//! NOTES (doubts and gaps, nothing is changed in library code)
//! * `units` has no numeric conversion functions (no mm per inch, no degrees to
//!   radians); it only holds unit-string constants, the alias table
//!   (`canonical`), `STANDARD_GRAVITY_M_S2` and the display formatters. The
//!   "25.4 mm per inch" style checks therefore cannot be written; the closest
//!   known constant, standard gravity, is pinned instead.
//! * Behaviour on NaN *data* is not documented for `mean`, `sample_std` or
//!   `nearest_rank` (only a NaN `q` is: it reads as the median), so no test
//!   asserts it. By IEEE rules `mean` of data containing NaN is NaN, and
//!   `nearest_rank` on a sorted slice with NaN has no defined order.
//! * `nearest_rank` assumes ascending input and does not check it.

use vernon_core::stats::{mean, nearest_rank, percentile_pct, sample_std};
use vernon_core::units::{self, canonical, clean_zero, fixed4, fmt_value};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Uniform in [lo, hi).
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
    fn len(&mut self, max: usize) -> usize {
        1 + (self.next() % max as u64) as usize
    }
}

fn sample(rng: &mut Rng, n: usize) -> Vec<f64> {
    (0..n).map(|_| rng.range(-1000.0, 1000.0)).collect()
}

fn sorted(v: &[f64]) -> Vec<f64> {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s
}

fn close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * a.abs().max(b.abs()).max(1.0)
}

// ------------------------------------------------------------------ stats

/// Property: mean([x + c]) = mean(x) + c (shift equivariance).
#[test]
fn units_stats_props_mean_shift_equivariance() {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..200 {
        let n = r.len(200);
        let x = sample(&mut r, n);
        let c = r.range(-500.0, 500.0);
        let shifted: Vec<f64> = x.iter().map(|v| v + c).collect();
        assert!(close(mean(&shifted), mean(&x) + c, 1e-12));
    }
}

/// Property: mean([k x]) = k mean(x) for any sign of k (scale equivariance).
#[test]
fn units_stats_props_mean_scale_equivariance() {
    let mut r = Rng(11);
    for _ in 0..200 {
        let n = r.len(200);
        let x = sample(&mut r, n);
        let k = r.range(-10.0, 10.0);
        let scaled: Vec<f64> = x.iter().map(|v| v * k).collect();
        assert!(close(mean(&scaled), k * mean(&x), 1e-12));
    }
}

/// Property: min <= mean <= max, and the mean of a constant sample is that constant.
#[test]
fn units_stats_props_mean_is_bounded_by_min_and_max() {
    let mut r = Rng(12);
    for _ in 0..200 {
        let n = r.len(100);
        let x = sample(&mut r, n);
        let s = sorted(&x);
        let m = mean(&x);
        assert!(m >= s[0] - 1e-9 && m <= s[n - 1] + 1e-9);
        let c = r.range(-50.0, 50.0);
        assert!(close(mean(&vec![c; n]), c, 1e-14));
    }
}

/// Property: documented degenerate inputs: empty mean is 0, a single element
/// is its own mean, and the standard deviation of fewer than two values is 0.
#[test]
fn units_stats_props_empty_and_single_element_inputs() {
    assert_eq!(mean(&[]), 0.0);
    assert_eq!(sample_std(&[]), 0.0);
    assert_eq!(sample_std(&[42.0]), 0.0);
    assert_eq!(mean(&[-3.5]), -3.5);
    assert_eq!(nearest_rank(&[], 0.5), 0.0);
    assert_eq!(percentile_pct(&[], 90.0), 0.0);
    for q in [0.0, 0.25, 0.5, 0.999, 1.0] {
        assert_eq!(nearest_rank(&[7.25], q), 7.25);
    }
}

/// Property: sample_std is non-negative, zero for constant data, invariant under
/// a shift and scales with |k|.
#[test]
fn units_stats_props_std_shift_invariance_and_scaling() {
    let mut r = Rng(13);
    for _ in 0..200 {
        let n = 2 + r.len(150);
        let x = sample(&mut r, n);
        let s = sample_std(&x);
        assert!(s >= 0.0);
        let c = r.range(-300.0, 300.0);
        let k = r.range(-8.0, 8.0);
        let shifted: Vec<f64> = x.iter().map(|v| v + c).collect();
        let scaled: Vec<f64> = x.iter().map(|v| v * k).collect();
        assert!(close(sample_std(&shifted), s, 1e-9));
        assert!(close(sample_std(&scaled), k.abs() * s, 1e-9));
        assert!(sample_std(&vec![c; n]).abs() < 1e-12);
    }
}

/// Property: the sample (N - 1) divisor: [2,4,4,4,5,5,7,9] has sum of squares
/// 32 about the mean 5, so std = sqrt(32/7); two values a, b give |a-b|/sqrt 2.
#[test]
fn units_stats_props_std_uses_n_minus_one() {
    let x = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
    assert!(close(sample_std(&x), (32.0f64 / 7.0).sqrt(), 1e-14));
    let mut r = Rng(14);
    for _ in 0..50 {
        let (a, b) = (r.range(-100.0, 100.0), r.range(-100.0, 100.0));
        assert!(close(sample_std(&[a, b]), (a - b).abs() / 2f64.sqrt(), 1e-12));
    }
}

/// Property: nearest-rank percentiles are members of the sample, lie within
/// [min, max], and q = 0 / q = 1 return the extremes.
#[test]
fn units_stats_props_percentile_is_a_member_between_the_extremes() {
    let mut r = Rng(15);
    for _ in 0..200 {
        let n = r.len(120);
        let s = sorted(&sample(&mut r, n));
        assert_eq!(nearest_rank(&s, 0.0), s[0]);
        assert_eq!(nearest_rank(&s, 1.0), s[n - 1]);
        for _ in 0..10 {
            let q = r.unit();
            let p = nearest_rank(&s, q);
            assert!(s.contains(&p));
            assert!(p >= s[0] && p <= s[n - 1]);
        }
    }
}

/// Property: percentiles are non-decreasing in q (ordering of percentiles).
#[test]
fn units_stats_props_percentiles_are_monotone_in_q() {
    let mut r = Rng(16);
    for _ in 0..100 {
        let n = r.len(300);
        let s = sorted(&sample(&mut r, n));
        let mut prev = f64::NEG_INFINITY;
        for i in 0..=100 {
            let p = percentile_pct(&s, i as f64);
            assert!(p >= prev);
            prev = p;
        }
        assert!(percentile_pct(&s, 25.0) <= percentile_pct(&s, 50.0));
        assert!(percentile_pct(&s, 50.0) <= percentile_pct(&s, 95.0));
    }
}

/// Property: for a positive scale k and any shift c, percentile(k x + c) =
/// k percentile(x) + c (an order-preserving map commutes with nearest rank).
#[test]
fn units_stats_props_percentile_affine_equivariance() {
    let mut r = Rng(17);
    for _ in 0..100 {
        let n = r.len(200);
        let x = sorted(&sample(&mut r, n));
        let (k, c) = (r.range(0.1, 9.0), r.range(-400.0, 400.0));
        let y: Vec<f64> = x.iter().map(|v| k * v + c).collect();
        for q in [0.0, 0.1, 0.5, 0.9, 0.99, 1.0] {
            assert!(close(nearest_rank(&y, q), k * nearest_rank(&x, q) + c, 1e-12));
        }
    }
}

/// Property: the input order of the *unsorted* sample does not matter once it
/// is sorted (permutation invariance of mean and percentiles).
#[test]
fn units_stats_props_permutation_invariance() {
    let mut r = Rng(18);
    for _ in 0..100 {
        let n = r.len(100);
        let x = sample(&mut r, n);
        let mut y = x.clone();
        for i in (1..n).rev() {
            let j = (r.next() % (i as u64 + 1)) as usize;
            y.swap(i, j);
        }
        assert!(close(mean(&x), mean(&y), 1e-12));
        assert!(close(sample_std(&x), sample_std(&y), 1e-12));
        assert_eq!(nearest_rank(&sorted(&x), 0.9), nearest_rank(&sorted(&y), 0.9));
    }
}

/// Property: documented nearest-rank convention on 1..=N: the value at rank
/// ceil(q N). The 1e-9 guard keeps 0.95 * 1000 from rounding up to rank 951.
#[test]
fn units_stats_props_nearest_rank_convention() {
    let s100: Vec<f64> = (1..=100).map(f64::from).collect();
    assert_eq!(percentile_pct(&s100, 50.0), 50.0);
    assert_eq!(percentile_pct(&s100, 95.0), 95.0);
    assert_eq!(percentile_pct(&s100, 99.0), 99.0);
    assert_eq!(percentile_pct(&s100, 0.5), 1.0);
    let s1000: Vec<f64> = (1..=1000).map(f64::from).collect();
    assert_eq!(nearest_rank(&s1000, 0.95), 950.0);
    assert_eq!(nearest_rank(&s1000, 0.951), 951.0);
    let s4 = [10.0, 20.0, 30.0, 40.0];
    assert_eq!(nearest_rank(&s4, 0.25), 10.0);
    assert_eq!(nearest_rank(&s4, 0.26), 20.0);
    assert_eq!(nearest_rank(&s4, 0.5), 20.0);
}

/// Property: q outside 0..=1 is clamped and a NaN q reads as the median
/// (both documented).
#[test]
fn units_stats_props_percentile_clamps_q_and_reads_nan_as_median() {
    let mut r = Rng(19);
    for _ in 0..50 {
        let n = r.len(99);
        let s = sorted(&sample(&mut r, n));
        assert_eq!(nearest_rank(&s, -3.0), s[0]);
        assert_eq!(nearest_rank(&s, 7.0), s[n - 1]);
        assert_eq!(nearest_rank(&s, f64::NAN), nearest_rank(&s, 0.5));
        assert_eq!(nearest_rank(&s, f64::NEG_INFINITY), s[0]);
        assert_eq!(nearest_rank(&s, f64::INFINITY), s[n - 1]);
    }
}

/// Property: percentile_pct(p) is nearest_rank(p / 100).
#[test]
fn units_stats_props_percent_form_matches_fraction_form() {
    let mut r = Rng(20);
    let s = sorted(&sample(&mut r, 77));
    for i in 0..=100 {
        assert_eq!(percentile_pct(&s, i as f64), nearest_rank(&s, i as f64 / 100.0));
    }
}

// ------------------------------------------------------------------ units

/// Property: canonical maps every legacy spelling to its single canonical
/// form, and is idempotent (canonical of a canonical name is unchanged).
#[test]
fn units_stats_props_canonical_maps_legacy_spellings() {
    for (old, new) in [
        ("mm2", units::MM2),
        ("mm3", units::MM3),
        ("m2", units::M2),
        ("m3", units::M3),
        ("degC", units::DEG_C),
        ("deg C", units::DEG_C),
        ("deg", units::DEG),
        ("W/m2", units::W_PER_M2),
        ("kWh/m2", units::KWH_PER_M2),
        ("kg/m3", units::KG_PER_M3),
        ("-", units::NONE),
        ("ratio", units::NONE),
        ("bool", units::NONE),
        ("flag", units::NONE),
        ("m/s2", "m/s²"),
        ("t/mm3", "t/mm³"),
    ] {
        assert_eq!(canonical(old), new, "{old}");
        assert_eq!(canonical(&canonical(old)), canonical(old), "idempotent {old}");
    }
}

/// Property: unit names that are already canonical, or unknown, are returned
/// unchanged; random strings are fixed points of canonical after one pass.
#[test]
fn units_stats_props_canonical_keeps_canonical_and_unknown_names() {
    for u in [
        units::NONE,
        units::COUNT,
        units::PERCENT,
        units::MM,
        units::MM2,
        units::MM3,
        units::M,
        units::M2,
        units::M3,
        units::DEG,
        units::DEG_C,
        units::KELVIN,
        units::SECOND,
        units::HOUR,
        units::M_PER_S,
        units::KM_PER_H,
        units::NEWTON,
        units::PASCAL,
        units::MEGAPASCAL,
        units::WATT,
        units::W_PER_M2,
        units::KWH_PER_M2,
        units::KG_PER_M3,
        "furlong",
        "N/mm",
    ] {
        assert_eq!(canonical(u), u);
    }
    let mut r = Rng(21);
    let pool = ["mm", "mm2", "m3", "deg", "xyz", "", "ratio", "kWh/m2", "N"];
    for _ in 0..200 {
        let u = pool[(r.next() % pool.len() as u64) as usize];
        let once = canonical(u).into_owned();
        assert_eq!(canonical(&once), once.as_str());
    }
}

/// Property: unit constants are pairwise distinct strings (no two quantities
/// share a spelling) and standard gravity is the ISO 80000-3 value.
#[test]
fn units_stats_props_constants_are_distinct_and_g_is_standard() {
    let all = [
        units::NONE,
        units::COUNT,
        units::PERCENT,
        units::MM,
        units::MM2,
        units::MM3,
        units::M,
        units::M2,
        units::M3,
        units::DEG,
        units::DEG_C,
        units::KELVIN,
        units::SECOND,
        units::HOUR,
        units::M_PER_S,
        units::KM_PER_H,
        units::NEWTON,
        units::PASCAL,
        units::MEGAPASCAL,
        units::WATT,
        units::W_PER_M2,
        units::KWH_PER_M2,
        units::KG_PER_M3,
    ];
    for i in 0..all.len() {
        for j in i + 1..all.len() {
            assert_ne!(all[i], all[j]);
        }
    }
    assert_eq!(units::STANDARD_GRAVITY_M_S2, 9.80665);
    // 1 km/h = 1/3.6 m/s sanity of the two speed spellings' scale: 36 km/h = 10 m/s
    assert!(close(36.0 / 3.6, 10.0, 1e-15));
}

/// Property: clean_zero turns negative zero into positive zero, leaves every
/// other value (including NaN and infinities) bit-for-bit alone, and is idempotent.
#[test]
fn units_stats_props_clean_zero() {
    assert!(clean_zero(-0.0).is_sign_positive());
    assert_eq!(clean_zero(-0.0), 0.0);
    assert!(clean_zero(0.0).is_sign_positive());
    assert!(clean_zero(f64::NAN).is_nan());
    assert_eq!(clean_zero(f64::INFINITY), f64::INFINITY);
    assert_eq!(clean_zero(f64::NEG_INFINITY), f64::NEG_INFINITY);
    let mut r = Rng(22);
    for _ in 0..500 {
        let v = r.range(-1e6, 1e6);
        assert_eq!(clean_zero(v).to_bits(), v.to_bits());
        assert_eq!(clean_zero(clean_zero(v)).to_bits(), clean_zero(v).to_bits());
    }
}

/// Property: fixed4 has exactly four decimals, never prints a negative zero,
/// parses back within half a unit of the last place, is odd-symmetric away
/// from zero and monotone non-decreasing in its numeric value.
#[test]
fn units_stats_props_fixed4() {
    assert_eq!(fixed4(-0.0), "0.0000");
    assert_eq!(fixed4(-0.00001), "0.0000");
    assert_eq!(fixed4(-0.00004), "0.0000");
    assert_eq!(fixed4(-0.00006), "-0.0001");
    assert_eq!(fixed4(2.5), "2.5000");
    let mut r = Rng(23);
    let mut vals: Vec<f64> = (0..400).map(|_| r.range(-5000.0, 5000.0)).collect();
    vals.extend((0..100).map(|_| r.range(-1e-4, 1e-4)));
    vals.sort_by(f64::total_cmp);
    let mut prev = f64::NEG_INFINITY;
    for v in vals {
        let s = fixed4(v);
        assert_eq!(s.split('.').nth(1).map(str::len), Some(4), "{s}");
        let back: f64 = s.parse().unwrap();
        assert!((back - v).abs() <= 5.0000001e-5, "{v} -> {s}");
        assert!(back >= prev, "monotone at {v}");
        prev = back;
        if back != 0.0 {
            assert_eq!(fixed4(-v), format!("{}{}", if v < 0.0 { "" } else { "-" }, s.trim_start_matches('-')));
        } else {
            assert!(!s.starts_with('-'), "{s}");
        }
    }
}

/// Property: fmt_value round-trips (relative or absolute error from the four
/// printed decimals), has no trailing zeros in fixed notation, switches to
/// scientific outside 1e-3..1e6, and never prints a negative zero.
#[test]
fn units_stats_props_fmt_value() {
    assert_eq!(fmt_value(0.0), "0");
    assert_eq!(fmt_value(-0.0), "0");
    assert_eq!(fmt_value(2.5), "2.5");
    assert_eq!(fmt_value(3.0), "3");
    assert_eq!(fmt_value(-0.00001), "-1.0000e-5");
    assert_eq!(fmt_value(1e6), "1.0000e6");
    assert_eq!(fmt_value(5e-4), "5.0000e-4");
    assert_eq!(fmt_value(f64::INFINITY), "inf");
    assert_eq!(fmt_value(f64::NEG_INFINITY), "-inf");
    assert_eq!(fmt_value(f64::NAN), "NaN");
    let mut r = Rng(24);
    for _ in 0..2000 {
        // log-uniform magnitude over 1e-8 .. 1e9, random sign
        let mag = 10f64.powf(r.range(-8.0, 9.0));
        let v = if r.next() & 1 == 0 { mag } else { -mag };
        let s = fmt_value(v);
        assert_ne!(s, "-0");
        let back: f64 = s.parse().unwrap_or_else(|_| panic!("unparsable {s}"));
        if v.abs() >= 1e-3 && v.abs() < 1e6 {
            assert!(!s.contains('e'), "{s}");
            assert!((back - v).abs() <= 5.0001e-5, "{v} -> {s}");
            if s.contains('.') {
                assert!(!s.ends_with('0'), "trailing zero in {s}");
            }
        } else {
            assert!(s.contains('e'), "{s}");
            assert!((back - v).abs() <= 6e-5 * v.abs(), "{v} -> {s}");
        }
        // odd symmetry: the sign is the only difference
        let neg = fmt_value(-v);
        assert_eq!(neg.trim_start_matches('-'), s.trim_start_matches('-'));
    }
    // just under the 1e-3 switch point the mantissa rounds up, still scientific
    assert_eq!(fmt_value(-0.0009999999999), "-1.0000e-3");
}

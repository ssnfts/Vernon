//! The unit spellings reports use, and number formatting for them.
//!
//! Every [`Metric`](crate::Metric) stores its unit through [`canonical`], so
//! the same quantity reads the same in the CLI, the report grid and the gate
//! summaries whichever stage wrote it: `mm²` not `mm2`, `°C` not `degC` or
//! `deg C`, and the empty string for a pure number (no `-`, `flag`, `bool` or
//! `ratio` placeholders). Producers may use the constants here or their old
//! ASCII spellings; both end up identical.
//!
//! Lengths are millimetres unless a metric says otherwise, physics is SI.

use std::borrow::Cow;

pub const NONE: &str = "";
pub const COUNT: &str = "count";
pub const PERCENT: &str = "%";
pub const MM: &str = "mm";
pub const MM2: &str = "mm²";
pub const MM3: &str = "mm³";
pub const M: &str = "m";
pub const M2: &str = "m²";
pub const M3: &str = "m³";
pub const DEG: &str = "°";
pub const DEG_C: &str = "°C";
pub const KELVIN: &str = "K";
pub const SECOND: &str = "s";
pub const HOUR: &str = "h";
pub const M_PER_S: &str = "m/s";
pub const KM_PER_H: &str = "km/h";
pub const NEWTON: &str = "N";
pub const PASCAL: &str = "Pa";
pub const MEGAPASCAL: &str = "MPa";
pub const WATT: &str = "W";
pub const W_PER_M2: &str = "W/m²";
pub const KWH_PER_M2: &str = "kWh/m²";
pub const KG_PER_M3: &str = "kg/m³";

/// Standard gravity, m/s2 (ISO 80000-3). Every stage derives its g from this.
pub const STANDARD_GRAVITY_M_S2: f64 = 9.80665;

/// Spellings that are really the same unit, as `(legacy, canonical)`.
const ALIASES: &[(&str, &str)] = &[
    ("-", NONE),
    ("flag", NONE),
    ("bool", NONE),
    ("ratio", NONE),
    ("mm2", MM2),
    ("mm3", MM3),
    ("m2", M2),
    ("m3", M3),
    ("degC", DEG_C),
    ("deg C", DEG_C),
    ("deg", DEG),
    ("W/m2", W_PER_M2),
    ("kWh/m2", KWH_PER_M2),
    ("kg/m3", KG_PER_M3),
    ("m2/s", "m²/s"),
    ("m2/s2", "m²/s²"),
    ("m/s2", "m/s²"),
    ("s/m2", "s/m²"),
    ("1/m2", "1/m²"),
    ("1/mm2", "1/mm²"),
    ("t/mm3", "t/mm³"),
    ("m3/s", "m³/s"),
    ("points/mm2", "points/mm²"),
];

/// The canonical spelling of `unit`; anything not in the table is kept.
pub fn canonical(unit: &str) -> Cow<'_, str> {
    match ALIASES.iter().find(|(old, _)| *old == unit) {
        Some((_, new)) => Cow::Borrowed(new),
        None => Cow::Borrowed(unit),
    }
}

/// `v` with a negative zero turned into a plain zero, so it never prints as
/// `-0`.
pub fn clean_zero(v: f64) -> f64 {
    if v == 0.0 {
        0.0
    } else {
        v
    }
}

/// `v` with four decimals, for a table column; a value that rounds to zero
/// prints as `0.0000`, never `-0.0000`.
pub fn fixed4(v: f64) -> String {
    let s = format!("{:.4}", clean_zero(v));
    match s.strip_prefix('-') {
        Some(rest) if rest.bytes().all(|b| b == b'0' || b == b'.') => rest.to_string(),
        _ => s,
    }
}

/// A report value for display: four decimals without trailing zeros,
/// scientific notation outside `1e-3..1e6`, never `-0`.
pub fn fmt_value(v: f64) -> String {
    let v = clean_zero(v);
    if !v.is_finite() {
        return format!("{v}");
    }
    let t = if v != 0.0 && (v.abs() >= 1e6 || v.abs() < 1e-3) {
        format!("{v:.4e}")
    } else {
        let t = format!("{v:.4}");
        t.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    // A small negative that rounds to zero would still print as "-0".
    if t == "-0" {
        "0".to_string()
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_spellings_map_to_one_form() {
        for old in ["degC", "deg C", "°C"] {
            assert_eq!(canonical(old), DEG_C);
        }
        for old in ["mm2", "mm²"] {
            assert_eq!(canonical(old), MM2);
        }
        for old in ["-", "flag", "bool", "ratio", ""] {
            assert_eq!(canonical(old), NONE, "{old}");
        }
        assert_eq!(canonical("W/m2"), W_PER_M2);
        assert_eq!(canonical("count"), COUNT);
        assert_eq!(canonical("furlong"), "furlong");
    }

    #[test]
    fn metric_units_are_normalised_on_construction() {
        let m = crate::Metric::new(1.0, "deg C");
        assert_eq!(m.unit, "°C");
        assert_eq!(crate::Metric::new(1.0, String::from("m2")).unit, "m²");
        assert!(crate::Metric::new(-0.0, "mm").value.is_sign_positive());
    }

    #[test]
    fn negative_zero_never_prints() {
        assert_eq!(fmt_value(-0.0), "0");
        assert_eq!(fmt_value(0.0), "0");
        assert_eq!(fmt_value(-0.00001), "-1.0000e-5");
        assert_eq!(fmt_value(-0.0004), "-4.0000e-4");
        assert_eq!(fmt_value(-1e-3 * 0.99999), "-9.9999e-4");
        assert_eq!(fmt_value(2.5), "2.5");
        assert_eq!(fmt_value(-4.0), "-4");
        assert_eq!(fmt_value(1.0e7), "1.0000e7");
        assert_eq!(clean_zero(-0.0).to_bits(), 0.0f64.to_bits());
        assert_eq!(fmt_value(f64::NAN), "NaN");
        assert_eq!(fixed4(-0.0), "0.0000");
        assert_eq!(fixed4(-0.00001), "0.0000");
        assert_eq!(fixed4(-1.5), "-1.5000");
    }
}

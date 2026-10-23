//! Moist-air state shared by the airflow and performance stages: one density
//! formula, one set of gas constants and one hand-off schema.
//!
//! Hand-off keys (all SI): `t_c` (deg C), `p_pa` (Pa), `rh` (fraction 0..1),
//! `rh_pct` (the same humidity in percent), `rho` (kg/m3). Readers also accept
//! the older spellings `T_c` and a percent value under `rh` (anything above 1).

/// Specific gas constant of dry air, J/(kg K).
pub const R_DRY: f64 = 287.058;
/// Specific gas constant of water vapour, J/(kg K).
pub const R_VAPOUR: f64 = 461.495;

/// Saturation vapour pressure over water by the Tetens form, Pa.
pub fn sat_pressure_pa(t_c: f64) -> f64 {
    610.78 * (17.27 * t_c / (t_c + 237.3)).exp()
}

/// Moist-air density (kg/m3) from temperature (deg C), total pressure (Pa),
/// relative humidity as a fraction (clamped to 0..1) and the dry-air gas constant.
pub fn moist_air_density(t_c: f64, p_pa: f64, rh: f64, r_dry: f64) -> f64 {
    let t = t_c + 273.15;
    let pv = (rh.clamp(0.0, 1.0) * sat_pressure_pa(t_c)).min(p_pa.max(0.0));
    (p_pa - pv) / (r_dry * t) + pv / (R_VAPOUR * t)
}

/// Relative humidity as a fraction from a value that may be a fraction (0..=1)
/// or a percentage (above 1, up to 100). `None` for negative, non-finite or
/// above-100 values.
pub fn rh_fraction(v: f64) -> Option<f64> {
    if !v.is_finite() || v < 0.0 || v > 100.0 {
        None
    } else if v > 1.0 {
        Some(v / 100.0)
    } else {
        Some(v)
    }
}

/// Air state handed between nodes. Unknown fields are NaN.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AirState {
    pub rho: f64,
    pub t_c: f64,
    pub p_pa: f64,
    /// Relative humidity, fraction 0..1.
    pub rh: f64,
}

impl AirState {
    /// State from temperature, pressure and humidity (fraction), density computed.
    pub fn from_state(t_c: f64, p_pa: f64, rh: f64, r_dry: f64) -> Self {
        Self {
            rho: moist_air_density(t_c, p_pa, rh, r_dry),
            t_c,
            p_pa,
            rh,
        }
    }

    /// A state known only by its density.
    pub fn from_rho(rho: f64) -> Self {
        Self {
            rho,
            t_c: f64::NAN,
            p_pa: f64::NAN,
            rh: f64::NAN,
        }
    }

    /// The hand-off fields in the shared schema.
    pub fn fields(&self) -> [(&'static str, f64); 5] {
        [
            ("rho", self.rho),
            ("t_c", self.t_c),
            ("p_pa", self.p_pa),
            ("rh", self.rh),
            ("rh_pct", self.rh * 100.0),
        ]
    }

    /// Read a state from a key lookup (for example a Json object). Accepts
    /// `t_c` or `T_c`, and `rh` (fraction, or percent when above 1) or
    /// `rh_pct`. Density is read from `rho`, else computed from `t_c` and
    /// `p_pa` with [`R_DRY`].
    pub fn from_lookup(get: impl Fn(&str) -> Option<f64>) -> Result<Self, String> {
        let t_c = get("t_c").or_else(|| get("T_c"));
        let rh = match (get("rh"), get("rh_pct")) {
            (Some(v), _) => Some(rh_fraction(v).ok_or_else(|| format!("air state: rh {v} is not a humidity (0..1 or 0..100 %)"))?),
            (None, Some(p)) => Some(rh_fraction(p / 100.0).ok_or_else(|| format!("air state: rh_pct {p} is not within 0..100"))?),
            (None, None) => None,
        };
        let p_pa = get("p_pa");
        let rho = match get("rho") {
            Some(r) => r,
            None => match (t_c, p_pa) {
                (Some(t), Some(p)) => moist_air_density(t, p, rh.unwrap_or(0.0), R_DRY),
                _ => return Err("air state: needs rho, or t_c and p_pa".into()),
            },
        };
        if !(rho.is_finite() && rho > 0.0) {
            return Err(format!("air state: density {rho} is not a positive number"));
        }
        Ok(Self {
            rho,
            t_c: t_c.unwrap_or(f64::NAN),
            p_pa: p_pa.unwrap_or(f64::NAN),
            rh: rh.unwrap_or(f64::NAN),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(kv: &'a [(&'a str, f64)]) -> impl Fn(&str) -> Option<f64> + 'a {
        move |k| kv.iter().find(|(n, _)| *n == k).map(|(_, v)| *v)
    }

    #[test]
    fn density_goldens() {
        // ISA sea level, dry
        assert!((moist_air_density(15.0, 101_325.0, 0.0, 287.05) - 1.22501).abs() < 1e-4);
        // 45 C, 30 % RH
        assert!((moist_air_density(45.0, 101_325.0, 0.30, R_DRY) / 1.09757 - 1.0).abs() < 1e-4);
        assert!((sat_pressure_pa(45.0) - 9582.0).abs() < 5.0);
    }

    #[test]
    fn rh_percent_and_fraction_conventions() {
        assert_eq!(rh_fraction(0.4), Some(0.4));
        assert_eq!(rh_fraction(40.0), Some(0.4));
        assert_eq!(rh_fraction(150.0), None);
        assert_eq!(rh_fraction(-1.0), None);
        // the two schemas give the same state
        let a = AirState::from_lookup(lookup(&[("t_c", 30.0), ("p_pa", 101_325.0), ("rh_pct", 40.0)])).unwrap();
        let b = AirState::from_lookup(lookup(&[("T_c", 30.0), ("p_pa", 101_325.0), ("rh", 0.4)])).unwrap();
        let c = AirState::from_lookup(lookup(&[("T_c", 30.0), ("p_pa", 101_325.0), ("rh", 40.0)])).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert!((a.rh - 0.4).abs() < 1e-12 && (a.t_c - 30.0).abs() < 1e-12);
        let f = a.fields();
        assert!(f.contains(&("rh_pct", 40.0)) && f.contains(&("t_c", 30.0)));
        assert!(AirState::from_lookup(lookup(&[("rho", -1.0)])).is_err());
        assert!(AirState::from_lookup(lookup(&[])).is_err());
        assert!(AirState::from_lookup(lookup(&[("rho", 1.2), ("rh", 250.0)])).is_err());
    }
}

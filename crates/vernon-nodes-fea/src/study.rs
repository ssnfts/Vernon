//! Mesh-convergence study, singularity flag, hand calculation of bolted joints
//! and the oven creep plan.

use serde::{Deserialize, Serialize};

/// Outcome of the grid-convergence check of one quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Pass,
    /// Monotone and converging but the index is above the limit.
    TooCoarse,
    /// The three values oscillate.
    NonMonotone,
    /// Monotone but the changes grow with refinement.
    Diverging,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::TooCoarse => "gci above limit",
            Verdict::NonMonotone => "non-monotone",
            Verdict::Diverging => "diverging",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Gci {
    /// Observed order of convergence (None when undefined).
    pub p: Option<f64>,
    /// Richardson extrapolation to zero element size.
    pub f_ext: f64,
    /// Fine-mesh grid convergence index, percent.
    pub gci_fine_pct: f64,
    /// `(f3 - f2) / (f2 - f1)`.
    pub ratio: f64,
    pub verdict: Verdict,
}

/// Grid convergence index of Roache from three meshes with refinement ratio
/// `r`: `f1` fine, `f2` medium, `f3` coarse.
pub fn gci(f1: f64, f2: f64, f3: f64, r: f64, gci_max_pct: f64) -> Gci {
    let d21 = f2 - f1;
    let d32 = f3 - f2;
    if d21.abs() <= 1e-15 * f1.abs().max(1e-300) {
        let verdict = if d32.abs() <= 1e-15 * f1.abs().max(1e-300) { Verdict::Pass } else { Verdict::NonMonotone };
        return Gci { p: None, f_ext: f1, gci_fine_pct: 0.0, ratio: f64::NAN, verdict };
    }
    let ratio = d32 / d21;
    if ratio <= 0.0 {
        return Gci { p: None, f_ext: f1, gci_fine_pct: 100.0 * (d21 / f1).abs() * 1.25, ratio, verdict: Verdict::NonMonotone };
    }
    if ratio <= 1.0 {
        return Gci { p: Some(ratio.ln() / r.ln()), f_ext: f1, gci_fine_pct: 100.0 * (d21 / f1).abs() * 1.25, ratio, verdict: Verdict::Diverging };
    }
    let p = ratio.ln() / r.ln();
    let rp = ratio; // r^p
    let f_ext = f1 + (f1 - f2) / (rp - 1.0);
    let g = 1.25 * (d21 / f1).abs() / (rp - 1.0) * 100.0;
    Gci { p: Some(p), f_ext, gci_fine_pct: g, ratio, verdict: if g < gci_max_pct { Verdict::Pass } else { Verdict::TooCoarse } }
}

/// A mount is a stress singularity when the raw peak safety factor moves by
/// more than 20 % between two mesh levels while the averaged one moves by
/// less than 5 %.
pub fn is_singular(peak_prev: f64, peak_next: f64, avg_prev: f64, avg_next: f64) -> bool {
    let rel = |a: f64, b: f64| ((b - a) / a).abs();
    rel(peak_prev, peak_next) > 0.20 && rel(avg_prev, avg_next) < 0.05
}

/// Inputs of the bolted-joint hand calculation.
#[derive(Debug, Clone, Copy)]
pub struct JointInput {
    /// Load carried by one bolt (N).
    pub load_n: f64,
    /// Fraction of the load acting along the bolt axis (pull-through); the rest is in the plate plane.
    pub axial_fraction: f64,
    pub bolt_d_mm: f64,
    pub plate_t_mm: f64,
    /// Edge distance from the hole centre (mm).
    pub edge_mm: f64,
    pub head_d_mm: f64,
    pub xt: f64,
    pub zt: f64,
    pub s12: f64,
    pub s13: f64,
    /// The in-plane load pushes through the stacking direction (bolt axis in the print plane).
    pub load_through_stack: bool,
    /// Layers lie parallel to the plate.
    pub plate_flat: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JointResult {
    pub bearing_mpa: f64,
    pub bearing_sf: f64,
    pub pullthrough_mpa: f64,
    pub pullthrough_sf: f64,
    pub shearout_mpa: f64,
    pub shearout_sf: f64,
    pub min_sf: f64,
}

fn ratio(strength: f64, stress: f64) -> f64 {
    if stress < 1e-12 {
        f64::INFINITY
    } else {
        strength / stress
    }
}

/// Bearing (`F / (d t)`), pull-through shear on the head circle
/// (`F / (pi D t)`) and shear-out (`F / (2 (e - d/2) t)`) with the matching
/// allowables.
pub fn joint_hand_check(j: &JointInput) -> Result<JointResult, String> {
    if j.edge_mm <= 0.5 * j.bolt_d_mm {
        return Err(format!("edge distance {} mm must exceed the bolt radius {} mm", j.edge_mm, 0.5 * j.bolt_d_mm));
    }
    if j.bolt_d_mm <= 0.0 || j.plate_t_mm <= 0.0 || j.head_d_mm <= 0.0 {
        return Err("bolt diameter, plate thickness and head diameter must be positive".into());
    }
    let f_ax = j.load_n * j.axial_fraction;
    let f_in = j.load_n * (1.0 - j.axial_fraction);
    let bearing = f_in / (j.bolt_d_mm * j.plate_t_mm);
    let pull = f_ax / (std::f64::consts::PI * j.head_d_mm * j.plate_t_mm);
    let shear_out = f_in / (2.0 * (j.edge_mm - 0.5 * j.bolt_d_mm) * j.plate_t_mm);
    let bearing_allow = if j.load_through_stack { j.zt } else { j.xt };
    let pull_allow = if j.plate_flat { j.s13 } else { j.s12 };
    let shear_allow = if j.load_through_stack { j.s13 } else { j.s12 };
    let r = JointResult {
        bearing_mpa: bearing,
        bearing_sf: ratio(bearing_allow, bearing),
        pullthrough_mpa: pull,
        pullthrough_sf: ratio(pull_allow, pull),
        shearout_mpa: shear_out,
        shearout_sf: ratio(shear_allow, shear_out),
        min_sf: 0.0,
    };
    let min_sf = r.bearing_sf.min(r.pullthrough_sf).min(r.shearout_sf);
    Ok(JointResult { min_sf, ..r })
}

/// One row of the oven creep plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreepRow {
    pub temperature_c: f64,
    pub hold_h: f64,
    /// Sustained stress applied to the coupon (MPa).
    pub stress_mpa: f64,
    pub load_n: f64,
    /// Margin to the heat deflection temperature (degC), if known.
    pub hdt_margin_c: Option<f64>,
    /// Times (h) at which to record strain.
    pub readings_h: Vec<f64>,
    /// Retention factor to adopt from a measured creep strain, if one was entered.
    pub suggested_creep_factor: Option<f64>,
}

/// Reading times for a hold: 0.1, 1, 4, 24, 72, 168 h up to the hold length, then the end.
pub fn reading_times(hold_h: f64) -> Vec<f64> {
    let mut t: Vec<f64> = [0.1, 1.0, 4.0, 24.0, 72.0, 168.0].into_iter().filter(|&x| x < hold_h).collect();
    t.push(hold_h);
    t
}

/// Creep factor suggested by a coupon test: the allowed strain divided by
/// the measured strain, clamped to `[0.1, 1]`; `None` when nothing was measured.
pub fn suggested_creep_factor(measured_strain_pct: f64, allowed_strain_pct: f64) -> Option<f64> {
    (measured_strain_pct > 0.0).then(|| (allowed_strain_pct / measured_strain_pct).clamp(0.1, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g10_convergence_index() {
        let g = gci(1.6120, 1.6060, 1.5800, 2.0, 5.0);
        assert!((g.p.unwrap() - 2.115).abs() < 2e-3, "{:?}", g.p);
        assert!((g.f_ext - 1.6138).abs() < 1e-4, "{}", g.f_ext);
        assert!((g.gci_fine_pct - 0.14).abs() < 0.01, "{}", g.gci_fine_pct);
        assert_eq!(g.verdict, Verdict::Pass);
        let bad = gci(1.60, 1.61, 1.58, 2.0, 5.0);
        assert_eq!(bad.verdict, Verdict::NonMonotone);
        assert_eq!(bad.verdict.label(), "non-monotone");
    }

    #[test]
    fn diverging_and_coarse_cases() {
        // changes grow with refinement
        assert_eq!(gci(1.0, 1.1, 1.15, 2.0, 5.0).verdict, Verdict::Diverging);
        // converging but with a big index
        let g = gci(1.0, 1.2, 1.8, 2.0, 5.0);
        assert_eq!(g.verdict, Verdict::TooCoarse);
        // identical values: converged
        assert_eq!(gci(2.0, 2.0, 2.0, 2.0, 5.0).verdict, Verdict::Pass);
    }

    #[test]
    fn singular_flag_thresholds() {
        assert!(is_singular(4.0, 3.0, 3.5, 3.45));
        assert!(!is_singular(4.0, 3.9, 3.5, 3.45));
        assert!(!is_singular(4.0, 3.0, 3.5, 3.0));
    }

    #[test]
    fn g8_hand_check_numbers() {
        let base = JointInput {
            load_n: 428.25,
            axial_fraction: 0.0,
            bolt_d_mm: 5.0,
            plate_t_mm: 4.0,
            edge_mm: 10.0,
            head_d_mm: 10.0,
            xt: 168.0,
            zt: 57.0,
            s12: 80.0,
            s13: 40.0,
            load_through_stack: false,
            plate_flat: true,
        };
        let a = joint_hand_check(&base).unwrap();
        assert!((a.bearing_mpa - 21.4125).abs() < 1e-9);
        assert!(a.bearing_sf > 3.0 && (a.bearing_sf - 168.0 / 21.4125).abs() < 1e-9);
        let b = joint_hand_check(&JointInput { load_through_stack: true, ..base }).unwrap();
        assert!(b.bearing_sf < 3.0, "through-stack bearing must fail the SF 3 target: {}", b.bearing_sf);
        assert!((b.bearing_sf - 57.0 / 21.4125).abs() < 1e-9);
        // 669 N per mount at 250 km/h
        let c = joint_hand_check(&JointInput { load_n: 669.0, ..base }).unwrap();
        assert!((c.bearing_mpa - 33.45).abs() < 1e-9);
        // shear-out: 428.25 / (2 * 7.5 * 4)
        assert!((a.shearout_mpa - 7.1375).abs() < 1e-9);
        // pull-through only
        let d = joint_hand_check(&JointInput { axial_fraction: 1.0, ..base }).unwrap();
        assert!((d.pullthrough_mpa - 428.25 / (std::f64::consts::PI * 40.0)).abs() < 1e-9);
        assert!(d.bearing_sf.is_infinite() && d.min_sf == d.pullthrough_sf.min(d.shearout_sf));
        assert!(joint_hand_check(&JointInput { edge_mm: 2.0, ..base }).is_err());
    }

    #[test]
    fn creep_plan_helpers() {
        assert_eq!(reading_times(24.0), vec![0.1, 1.0, 4.0, 24.0]);
        assert_eq!(reading_times(2.0), vec![0.1, 1.0, 2.0]);
        assert_eq!(suggested_creep_factor(0.0, 0.5), None);
        assert_eq!(suggested_creep_factor(1.0, 0.5), Some(0.5));
        assert_eq!(suggested_creep_factor(0.2, 0.5), Some(1.0));
        assert_eq!(suggested_creep_factor(100.0, 0.5), Some(0.1));
    }
}

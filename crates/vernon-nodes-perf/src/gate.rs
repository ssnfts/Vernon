//! Validation gate: turns a claimed aero change plus the lap, coastdown and pressure
//! evidence into one pass/warn/fail report. Spec section 4 (gate rules 1-4).

use vernon_core::{Metric, Report, Severity};

/// Everything the gate can look at; every part is optional.
#[derive(Debug, Clone, Default)]
pub struct GateInput {
    pub claimed_dcda_m2: f64,
    pub claimed_dcla_m2: f64,
    /// Lap-time value of the claimed change, s (positive = slower).
    pub lap_value_s: Option<f64>,
    /// Top-speed value of the claimed change, km/h (negative = slower).
    pub top_speed_value_kmh: Option<f64>,
    /// Coastdown minimum detectable change in CdA, m2.
    pub min_detectable_dcda: Option<f64>,
    /// Coastdown CdA with its 95 percent interval.
    pub coast_cda: Option<(f64, f64, f64)>,
    pub cfd_cda: Option<f64>,
    pub cda_tol_m2: f64,
    pub cp_rms: Option<f64>,
    pub cp_sign_disagree: Option<f64>,
    pub cp_rms_limit: f64,
    pub fail_on_miss: bool,
}

/// Evaluate the gate rules into a report; `status` is 0 pass, 1 warn, 2 fail.
pub fn evaluate(g: &GateInput) -> Report {
    let mut r = Report::new("Validation gate");
    r.metric("claimed_dcda", Metric::new(g.claimed_dcda_m2, "m2"));
    r.metric("claimed_dcla", Metric::new(g.claimed_dcla_m2, "m2"));

    // rule 1: is the claimed change above the coastdown noise floor
    if let (Some(md), true) = (g.min_detectable_dcda, g.claimed_dcda_m2 != 0.0) {
        r.metric("min_detectable_dcda", Metric::new(md, "m2"));
        r.metric("detectability_ratio", Metric::new(g.claimed_dcda_m2.abs() / md.max(1e-12), "-"));
        if g.claimed_dcda_m2.abs() < md {
            r.note(
                Severity::Warning,
                format!(
                    "below noise floor: claimed dCdA {:.4} m2 is smaller than the smallest change the coastdown can detect ({:.4} m2)",
                    g.claimed_dcda_m2, md
                ),
            );
        } else {
            r.note(
                Severity::Info,
                format!("claimed dCdA {:.4} m2 is above the coastdown noise floor ({:.4} m2)", g.claimed_dcda_m2, md),
            );
        }
    } else if g.claimed_dcda_m2 != 0.0 {
        r.note(Severity::Info, "no coastdown fit connected: detectability of the claimed dCdA is not checked");
    }

    // rule 2: CFD CdA against the coastdown interval
    if let (Some(cfd), Some((c, lo, hi))) = (g.cfd_cda, g.coast_cda) {
        let excess = if cfd < lo {
            lo - cfd
        } else if cfd > hi {
            cfd - hi
        } else {
            0.0
        };
        let mut m = Metric::new(excess, "m2");
        if g.fail_on_miss {
            m = m.with_limit(g.cda_tol_m2, true);
        }
        r.metric("cfd_outside_coast_ci", m);
        r.metric("cfd_cda", Metric::new(cfd, "m2"));
        r.metric("coast_cda", Metric::new(c, "m2"));
        if excess > g.cda_tol_m2 {
            let text = format!(
                "CFD CdA {cfd:.3} m2 lies {excess:.3} m2 outside the coastdown 95 percent interval [{lo:.3}, {hi:.3}] m2 (allowed {:.3})",
                g.cda_tol_m2
            );
            r.note(if g.fail_on_miss { Severity::Error } else { Severity::Warning }, text);
        } else {
            r.note(Severity::Info, "CFD CdA agrees with the coastdown interval");
        }
    }

    // rule 3: pressure taps
    if let Some(rms) = g.cp_rms {
        r.metric("cp_rms", Metric::new(rms, "-"));
        if rms > g.cp_rms_limit {
            r.note(Severity::Warning, format!("Cp RMS residual {rms:.3} exceeds {:.3}", g.cp_rms_limit));
        }
    }
    if let Some(d) = g.cp_sign_disagree {
        r.metric("cp_sign_disagree", Metric::new(d, "taps"));
        if d > 0.0 {
            r.note(Severity::Warning, format!("{d:.0} tap(s) disagree in sign with the simulation, whatever the RMS"));
        }
    }

    // rule 4: always print the value of the claimed change
    match g.lap_value_s {
        Some(v) => {
            r.metric("lap_value", Metric::new(v, "s"));
            r.note(Severity::Info, format!("the claimed change is worth {v:+.4} s per lap"));
        }
        None => {
            r.note(
                Severity::Warning,
                "no lap-time value: connect a vehicle, track and air state, or a sweep report",
            );
        }
    }
    if let Some(v) = g.top_speed_value_kmh {
        r.metric("top_speed_value", Metric::new(v, "km/h"));
        r.note(Severity::Info, format!("the claimed change is worth {v:+.2} km/h of top speed"));
    }

    let status = if r.notes.iter().any(|(s, _)| *s == Severity::Error) {
        2.0
    } else if r.notes.iter().any(|(s, _)| *s == Severity::Warning) {
        1.0
    } else {
        0.0
    };
    r.metric("status", Metric::new(status, "0 pass, 1 warn, 2 fail"));
    r
}

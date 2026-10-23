//! Failure criteria with direction-dependent allowables.
//!
//! Stresses are `[xx, yy, zz, xy, yz, zx]` in the *material* frame (axes 1
//! and 2 in the print plane, 3 along the layer stacking). Every function
//! returns a safety factor (strength ratio); `f64::INFINITY` means "no load".

use crate::linalg::{eig_sym3, stress_to_tensor, von_mises, V6};
use crate::material::MaterialCard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Criterion {
    VonMises,
    MaxPrincipal,
    TsaiWu,
    Interlayer,
}

impl Criterion {
    pub const ALL: [Criterion; 4] = [Criterion::VonMises, Criterion::MaxPrincipal, Criterion::TsaiWu, Criterion::Interlayer];

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "von_mises" => Some(Self::VonMises),
            "max_principal" => Some(Self::MaxPrincipal),
            "tsai_wu" => Some(Self::TsaiWu),
            "interlayer" => Some(Self::Interlayer),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::VonMises => "von_mises",
            Self::MaxPrincipal => "max_principal",
            Self::TsaiWu => "tsai_wu",
            Self::Interlayer => "interlayer",
        }
    }
}

/// Strengths (MPa) at one temperature.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strengths {
    pub xt: f64,
    pub xc: f64,
    pub zt: f64,
    pub zc: f64,
    pub s12: f64,
    pub s13: f64,
    pub s23: f64,
}

impl Strengths {
    /// Card strengths times `r` (retention at the point's temperature) times
    /// `fraction` (allowable fraction, e.g. yield over ultimate).
    pub fn from_card(c: &MaterialCard, r: f64, fraction: f64) -> Self {
        let k = r * fraction;
        Self { xt: c.xt * k, xc: c.xc * k, zt: c.zt * k, zc: c.zc * k, s12: c.s12 * k, s13: c.s13 * k, s23: c.s23 * k }
    }
}

const TINY: f64 = 1e-12;

/// Isotropic check on the in-plane tensile strength.
pub fn sf_von_mises(s: &V6, st: &Strengths) -> f64 {
    let vm = von_mises(s);
    if vm < TINY {
        f64::INFINITY
    } else {
        st.xt / vm
    }
}

/// Principal-stress check with an ellipse interpolation of the allowable
/// between the in-plane and the stacking-direction strength.
pub fn sf_max_principal(s: &V6, st: &Strengths) -> f64 {
    let (vals, vecs) = eig_sym3(&stress_to_tensor(s));
    // Ellipse of semi-axes `x` (in plane) and `z` (stacking): the allowable along a
    // direction with cos(theta) = v[2] to the stacking axis is
    // 1 / sqrt(sin^2/x^2 + cos^2/z^2) (the square root keeps the unit MPa).
    let allow = |x: f64, z: f64, v: &[f64; 3]| {
        let c2 = (v[2] * v[2]).clamp(0.0, 1.0);
        1.0 / ((1.0 - c2) / (x * x) + c2 / (z * z)).sqrt()
    };
    let mut sf = f64::INFINITY;
    if vals[2] > TINY {
        sf = sf.min(allow(st.xt, st.zt, &vecs[2]) / vals[2]);
    }
    if vals[0] < -TINY {
        sf = sf.min(allow(st.xc, st.zc, &vecs[0]) / -vals[0]);
    }
    sf
}

/// Tsai-Wu strength ratio (quadratic form with the usual F12 = -1/2 sqrt(F11 F22)
/// estimate, extended to F13 and F23).
pub fn sf_tsai_wu(s: &V6, st: &Strengths) -> f64 {
    let f1 = 1.0 / st.xt - 1.0 / st.xc;
    let f3 = 1.0 / st.zt - 1.0 / st.zc;
    let f11 = 1.0 / (st.xt * st.xc);
    let f33 = 1.0 / (st.zt * st.zc);
    let f12 = -0.5 * f11;
    let f13 = -0.5 * (f11 * f33).sqrt();
    let (s1, s2, s3) = (s[0], s[1], s[2]);
    let (t12, t23, t13) = (s[3], s[4], s[5]);
    let b = f1 * (s1 + s2) + f3 * s3;
    let a = f11 * (s1 * s1 + s2 * s2)
        + f33 * s3 * s3
        + 2.0 * f12 * s1 * s2
        + 2.0 * f13 * s3 * (s1 + s2)
        + t23 * t23 / (st.s23 * st.s23)
        + t13 * t13 / (st.s13 * st.s13)
        + t12 * t12 / (st.s12 * st.s12);
    let d = (b * b + 4.0 * a.max(0.0)).sqrt();
    let den = d + b;
    if den < TINY {
        f64::INFINITY
    } else {
        2.0 / den
    }
}

/// Delamination index from the through-stack tension and the two
/// interlayer shears.
pub fn sf_interlayer(s: &V6, st: &Strengths) -> f64 {
    let idx = (s[2].max(0.0) / st.zt).powi(2) + (s[5] / st.s13).powi(2) + (s[4] / st.s23).powi(2);
    if idx < TINY {
        f64::INFINITY
    } else {
        1.0 / idx.sqrt()
    }
}

pub fn safety_factor(c: Criterion, s: &V6, st: &Strengths) -> f64 {
    match c {
        Criterion::VonMises => sf_von_mises(s, st),
        Criterion::MaxPrincipal => sf_max_principal(s, st),
        Criterion::TsaiWu => sf_tsai_wu(s, st),
        Criterion::Interlayer => sf_interlayer(s, st),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// G7 strengths: XY tension 50, compression 60, Z tension 27.5 (0.55 x 50), Z compression 60, S12 30, S13 = S23 20.
    fn g7() -> Strengths {
        Strengths { xt: 50.0, xc: 60.0, zt: 27.5, zc: 60.0, s12: 30.0, s13: 20.0, s23: 20.0 }
    }

    #[test]
    fn g7_pure_loads() {
        assert!((sf_tsai_wu(&[10.0, 0.0, 0.0, 0.0, 0.0, 0.0], &g7()) - 5.0).abs() < 1e-9);
        assert!((sf_tsai_wu(&[0.0, 0.0, 5.0, 0.0, 0.0, 0.0], &g7()) - 5.5).abs() < 1e-9);
        // compression in plane: 60 / 12
        assert!((sf_tsai_wu(&[-12.0, 0.0, 0.0, 0.0, 0.0, 0.0], &g7()) - 5.0).abs() < 1e-9);
        // pure shear in plane 15 against S12 30
        assert!((sf_tsai_wu(&[0.0, 0.0, 0.0, 15.0, 0.0, 0.0], &g7()) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn g7_mixed_state_shows_the_isotropic_check_overstates_safety() {
        let s = [10.0, 0.0, 5.0, 0.0, 0.0, 3.0];
        let tw = sf_tsai_wu(&s, &g7());
        assert!((tw - 3.381).abs() < 5e-3, "{tw}");
        let vm_stress = von_mises(&s);
        assert!((vm_stress - 10.0995).abs() < 1e-3, "{vm_stress}");
        let vm = sf_von_mises(&s, &g7());
        assert!((vm - 4.951).abs() < 1e-3, "{vm}");
        assert!(vm / tw > 1.4 && vm / tw < 1.5, "overstatement {}", vm / tw - 1.0);
        let il = sf_interlayer(&[0.0, 0.0, 5.0, 0.0, 0.0, 3.0], &g7());
        assert!((il - 4.24).abs() < 5e-3, "{il}");
    }

    #[test]
    fn von_mises_uniaxial_and_shear() {
        let st = g7();
        assert!((sf_von_mises(&[25.0, 0.0, 0.0, 0.0, 0.0, 0.0], &st) - 2.0).abs() < 1e-12);
        let tau = 10.0;
        assert!((von_mises(&[0.0, 0.0, 0.0, tau, 0.0, 0.0]) - 3f64.sqrt() * tau).abs() < 1e-12);
        assert!(sf_von_mises(&[0.0; 6], &st).is_infinite());
    }

    #[test]
    fn principal_allowable_follows_the_stacking_axis() {
        let st = g7();
        // tension along the stack: limited by Zt, along x: by Xt
        assert!((sf_max_principal(&[0.0, 0.0, 10.0, 0.0, 0.0, 0.0], &st) - 2.75).abs() < 1e-9);
        assert!((sf_max_principal(&[10.0, 0.0, 0.0, 0.0, 0.0, 0.0], &st) - 5.0).abs() < 1e-9);
        // 45 degrees between x and z: harmonic ellipse between 50 and 27.5
        let s = [5.0, 0.0, 5.0, 0.0, 0.0, 5.0];
        let sf = sf_max_principal(&s, &st);
        let allow = 1.0 / (0.5 / 2500.0 + 0.5 / 756.25f64).sqrt();
        assert!((sf - allow / 10.0).abs() < 1e-9, "{sf}");
        // compression is checked against the compressive strengths
        assert!((sf_max_principal(&[-30.0, 0.0, 0.0, 0.0, 0.0, 0.0], &st) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn tsai_wu_is_linear_in_load() {
        let s = [8.0, 3.0, 2.0, 1.0, 0.5, 1.5];
        let k = sf_tsai_wu(&s, &g7());
        let s2 = s.map(|v| v * 2.0);
        assert!((sf_tsai_wu(&s2, &g7()) - k / 2.0).abs() < 1e-9);
        assert!(sf_tsai_wu(&[0.0; 6], &g7()).is_infinite());
        // hydrostatic compression is very safe in this form
        assert!(sf_tsai_wu(&[-5.0, -5.0, -5.0, 0.0, 0.0, 0.0], &g7()) > 20.0);
    }

    #[test]
    fn strengths_scale_with_retention() {
        let c = MaterialCard::trans_iso("x", 2500.0, 2000.0, 0.35, 0.3, 800.0);
        let st = Strengths::from_card(&c, 0.5, 0.8);
        assert!((st.xt - 20.0).abs() < 1e-12 && (st.s12 - 12.0).abs() < 1e-12);
        assert_eq!(Criterion::parse("tsai_wu"), Some(Criterion::TsaiWu));
        assert!(Criterion::parse("nope").is_none());
        for c in Criterion::ALL {
            assert_eq!(Criterion::parse(c.name()), Some(c));
        }
    }
}

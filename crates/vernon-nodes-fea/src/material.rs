//! Printed-material database, orthotropic constants and temperature retention.
//!
//! Units: MPa, tonne/mm3, degC. Material axes: 1 and 2 in the print plane,
//! 3 along the layer-stacking (build) direction.
//!
//! The absolute moduli and strengths in the table are generic order-of-
//! magnitude placeholders (the project report states ratios, HDT and water
//! absorption, not absolute values); replace them with datasheet or coupon
//! data for the actual spool before trusting a safety factor.

use serde::{Deserialize, Serialize};

use vernon_core::materials::{self as m, PrintMaterial};

use crate::linalg::{invert6, M6};

/// Reference room temperature for retention ratios (degC).
pub const ROOM_C: f64 = 23.0;
/// Apparent modulus at the deflection-temperature test (0.45 MPa / 0.2 % strain).
pub const HDT_045_MODULUS_MPA: f64 = 225.0;
/// Apparent modulus at the deflection-temperature test (1.8 MPa / 0.2 % strain).
pub const HDT_18_MODULUS_MPA: f64 = 900.0;

/// Storage-modulus anchors from a dynamic-mechanical study (ratios only).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DmaAnchors {
    /// E(40 degC) / E(20 degC).
    pub r40: f64,
    /// Temperature where the modulus starts to fall again (degC).
    pub t_onset_c: f64,
    /// Glass-transition onset (degC).
    pub tg_onset_c: f64,
}

/// One row of the printed-material table.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    /// In-plane modulus (MPa), placeholder magnitude.
    pub e_xy: f64,
    /// Stacking-direction modulus (MPa), placeholder magnitude.
    pub e_z: f64,
    pub nu_xy: f64,
    pub nu_xz: f64,
    /// In-plane tensile strength (MPa), placeholder magnitude (PPA-CF: report value).
    pub xt: f64,
    /// Stacking-direction strength divided by in-plane strength.
    pub z_ratio: f64,
    pub z_ratio_basis: &'static str,
    pub density_g_cm3: f64,
    /// Heat deflection temperature at 0.45 MPa, as printed (degC).
    pub hdt_045_c: f64,
    /// Heat deflection temperature at 0.45 MPa after annealing, when it differs.
    pub hdt_045_annealed_c: Option<f64>,
    pub hdt_18_c: Option<f64>,
    pub water_abs_pct: Option<f64>,
    pub dma: Option<DmaAnchors>,
    pub note: &'static str,
    /// The shared row (heat, Z ratio, strength, density, water) with its source tags.
    #[serde(skip)]
    pub shared: &'static PrintMaterial,
}

impl Preset {
    pub fn hdt_045(&self, annealed: bool) -> f64 {
        match (annealed, self.hdt_045_annealed_c) {
            (true, Some(h)) => h,
            _ => self.hdt_045_c,
        }
    }
}

const PLACEHOLDER: &str = "absolute modulus and strength are placeholders: use datasheet or coupon data";

pub const PRESETS: [Preset; 6] = [
    Preset {
        id: "pla",
        name: "PLA",
        e_xy: 3000.0,
        e_z: 2400.0,
        nu_xy: 0.35,
        nu_xz: 0.30,
        xt: m::PLA.sigma_xy_mpa,
        z_ratio: m::PLA.z_ratio,
        z_ratio_basis: m::PLA.z_ratio_basis,
        density_g_cm3: m::PLA.density_g_cm3,
        hdt_045_c: m::PLA.hdt_045_c,
        hdt_045_annealed_c: m::PLA.hdt_045_annealed_c,
        hdt_18_c: m::PLA.hdt_18_c,
        water_abs_pct: m::PLA.water_abs_pct,
        shared: &m::PLA,
        dma: Some(DmaAnchors { r40: 0.919, t_onset_c: 57.0, tg_onset_c: 59.1 }),
        note: PLACEHOLDER,
    },
    Preset {
        id: "petg",
        name: "PETG",
        e_xy: 2100.0,
        e_z: 1700.0,
        nu_xy: 0.38,
        nu_xz: 0.33,
        xt: m::PETG.sigma_xy_mpa,
        z_ratio: m::PETG.z_ratio,
        z_ratio_basis: m::PETG.z_ratio_basis,
        density_g_cm3: m::PETG.density_g_cm3,
        hdt_045_c: m::PETG.hdt_045_c,
        hdt_045_annealed_c: m::PETG.hdt_045_annealed_c,
        hdt_18_c: m::PETG.hdt_18_c,
        water_abs_pct: m::PETG.water_abs_pct,
        shared: &m::PETG,
        dma: Some(DmaAnchors { r40: 0.803, t_onset_c: 80.0, tg_onset_c: 80.7 }),
        note: "HDT is the low end of the 69-72 degC range in the report (shared with the print and solar stages); absolute values are placeholders",
    },
    Preset {
        id: "asa",
        name: "ASA",
        e_xy: 2000.0,
        e_z: 1600.0,
        nu_xy: 0.38,
        nu_xz: 0.33,
        xt: m::ASA.sigma_xy_mpa,
        z_ratio: m::ASA.z_ratio,
        z_ratio_basis: m::ASA.z_ratio_basis,
        density_g_cm3: m::ASA.density_g_cm3,
        hdt_045_c: m::ASA.hdt_045_c,
        hdt_045_annealed_c: m::ASA.hdt_045_annealed_c,
        hdt_18_c: m::ASA.hdt_18_c,
        water_abs_pct: m::ASA.water_abs_pct,
        shared: &m::ASA,
        dma: Some(DmaAnchors { r40: 0.807, t_onset_c: 50.0, tg_onset_c: 105.7 }),
        note: "HDT is the low end of the 100-103 degC range in the report (shared with the print and solar stages); absolute values are placeholders",
    },
    Preset {
        id: "pet_cf",
        name: "PET-CF",
        e_xy: 5000.0,
        e_z: 3500.0,
        nu_xy: 0.35,
        nu_xz: 0.30,
        xt: m::PET_CF.sigma_xy_mpa,
        z_ratio: m::PET_CF.z_ratio,
        z_ratio_basis: m::PET_CF.z_ratio_basis,
        density_g_cm3: m::PET_CF.density_g_cm3,
        hdt_045_c: m::PET_CF.hdt_045_c,
        hdt_045_annealed_c: m::PET_CF.hdt_045_annealed_c,
        hdt_18_c: m::PET_CF.hdt_18_c,
        water_abs_pct: m::PET_CF.water_abs_pct,
        shared: &m::PET_CF,
        dma: None,
        note: PLACEHOLDER,
    },
    Preset {
        id: "paht_cf",
        name: "PAHT-CF",
        e_xy: 5500.0,
        e_z: 3800.0,
        nu_xy: 0.38,
        nu_xz: 0.33,
        xt: m::PAHT_CF.sigma_xy_mpa,
        z_ratio: m::PAHT_CF.z_ratio,
        z_ratio_basis: m::PAHT_CF.z_ratio_basis,
        density_g_cm3: m::PAHT_CF.density_g_cm3,
        hdt_045_c: m::PAHT_CF.hdt_045_c,
        hdt_045_annealed_c: m::PAHT_CF.hdt_045_annealed_c,
        hdt_18_c: m::PAHT_CF.hdt_18_c,
        water_abs_pct: m::PAHT_CF.water_abs_pct,
        shared: &m::PAHT_CF,
        dma: None,
        note: "HDT is conditional on annealing (84.8 as printed, 190.7 annealed); water uptake is assumed equal to the PA6-CF value in the report",
    },
    Preset {
        id: "ppa_cf",
        name: "PPA-CF",
        e_xy: 7500.0,
        e_z: 4500.0,
        nu_xy: 0.35,
        nu_xz: 0.30,
        xt: m::PPA_CF.sigma_xy_mpa,
        z_ratio: m::PPA_CF.z_ratio,
        z_ratio_basis: m::PPA_CF.z_ratio_basis,
        density_g_cm3: m::PPA_CF.density_g_cm3,
        hdt_045_c: m::PPA_CF.hdt_045_c,
        hdt_045_annealed_c: m::PPA_CF.hdt_045_annealed_c,
        hdt_18_c: m::PPA_CF.hdt_18_c,
        water_abs_pct: m::PPA_CF.water_abs_pct,
        shared: &m::PPA_CF,
        dma: None,
        note: "strengths are report values; moduli and density are placeholders",
    },
];

/// Look up a preset by id or name, ignoring case and `-`/`_`/space.
pub fn preset(id: &str) -> Option<&'static Preset> {
    let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let want = norm(id);
    PRESETS.iter().find(|p| norm(p.id) == want || norm(p.name) == want)
}

/// Piecewise-linear retention ratio `r(T)` (stiffness and strength fraction
/// of the room-temperature value), clamped at both ends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetentionTable {
    /// `(degC, ratio)` with strictly increasing temperature.
    pub points: Vec<(f64, f64)>,
    /// How the table was built (`dma_csv`, `dma_anchor`, `hdt_anchor`).
    pub source: String,
    /// True when the numbers are placeholders until a measured curve is given.
    pub uncalibrated: bool,
}

impl RetentionTable {
    pub fn at(&self, t: f64) -> f64 {
        let p = &self.points;
        if p.is_empty() {
            return 1.0;
        }
        if t <= p[0].0 {
            return p[0].1;
        }
        for w in p.windows(2) {
            if t <= w[1].0 {
                let f = (t - w[0].0) / (w[1].0 - w[0].0);
                return w[0].1 + f * (w[1].1 - w[0].1);
            }
        }
        p[p.len() - 1].1
    }

    pub fn temps(&self) -> Vec<f64> {
        self.points.iter().map(|p| p.0).collect()
    }
}

/// Printed material card handed to the deck writer and the safety factors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialCard {
    pub name: String,
    /// `orthotropic` or `isotropic_z`.
    pub mode: String,
    pub e1: f64,
    pub e2: f64,
    pub e3: f64,
    pub nu12: f64,
    pub nu13: f64,
    pub nu23: f64,
    pub g12: f64,
    pub g13: f64,
    pub g23: f64,
    /// tonne/mm3
    pub density_t_mm3: f64,
    pub xt: f64,
    pub xc: f64,
    pub zt: f64,
    pub zc: f64,
    pub s12: f64,
    pub s13: f64,
    pub s23: f64,
    pub hdt_045_c: Option<f64>,
    pub hdt_18_c: Option<f64>,
    pub dma: Option<DmaAnchors>,
    pub retention: Option<RetentionTable>,
    pub annealed: bool,
    /// Sustained-load knockdown applied on top of `retention` (1 = none).
    pub creep_factor: f64,
    pub water_abs_pct: Option<f64>,
    pub notes: Vec<String>,
}

impl MaterialCard {
    /// Transversely isotropic card: axes 1 = 2 in plane, 3 = stacking.
    pub fn trans_iso(name: &str, e_xy: f64, e_z: f64, nu_xy: f64, nu_xz: f64, g_xz: f64) -> Self {
        Self {
            name: name.into(),
            mode: "orthotropic".into(),
            e1: e_xy,
            e2: e_xy,
            e3: e_z,
            nu12: nu_xy,
            nu13: nu_xz,
            nu23: nu_xz,
            g12: e_xy / (2.0 * (1.0 + nu_xy)),
            g13: g_xz,
            g23: g_xz,
            density_t_mm3: 1.24e-9,
            xt: 50.0,
            xc: 60.0,
            zt: 27.5,
            zc: 60.0,
            s12: 30.0,
            s13: 20.0,
            s23: 20.0,
            hdt_045_c: None,
            hdt_18_c: None,
            dma: None,
            retention: None,
            annealed: false,
            creep_factor: 1.0,
            water_abs_pct: None,
            notes: vec![],
        }
    }

    pub fn z_ratio(&self) -> f64 {
        self.zt / self.xt
    }

    /// Minor Poisson ratios `(nu21, nu31, nu32)` from reciprocity.
    pub fn minor_nu(&self) -> (f64, f64, f64) {
        (
            self.nu12 * self.e2 / self.e1,
            self.nu13 * self.e3 / self.e1,
            self.nu23 * self.e3 / self.e2,
        )
    }

    /// Determinant term `Delta` of the orthotropic stiffness.
    pub fn delta(&self) -> f64 {
        let (nu21, nu31, nu32) = self.minor_nu();
        1.0 - self.nu12 * nu21 - self.nu23 * nu32 - nu31 * self.nu13 - 2.0 * nu21 * nu32 * self.nu13
    }

    /// Stability gate. Returns `Delta` or the reason the constants are rejected.
    ///
    /// Besides positive moduli and `nu_ij^2 < E_i/E_j`, Vernon requires
    /// `Delta > 0.01` and `|nu| < 0.5` (a plausibility bound for printed polymers).
    pub fn check_stability(&self) -> Result<f64, String> {
        for (n, v) in [
            ("E1", self.e1),
            ("E2", self.e2),
            ("E3", self.e3),
            ("G12", self.g12),
            ("G13", self.g13),
            ("G23", self.g23),
        ] {
            if !(v.is_finite() && v > 0.0) {
                return Err(format!("{n} must be positive and finite, got {v}"));
            }
        }
        for (n, nu, ei, ej) in [
            ("nu12", self.nu12, self.e1, self.e2),
            ("nu13", self.nu13, self.e1, self.e3),
            ("nu23", self.nu23, self.e2, self.e3),
        ] {
            if !nu.is_finite() || nu * nu >= ei / ej {
                return Err(format!("{n} = {nu} violates nu^2 < E_i/E_j ({:.4})", ei / ej));
            }
            if nu.abs() >= 0.5 {
                return Err(format!("{n} = {nu} is outside the plausible polymer range |nu| < 0.5"));
            }
        }
        let d = self.delta();
        if d <= 0.01 {
            return Err(format!("stiffness is (nearly) singular: Delta = {d:.5} <= 0.01"));
        }
        Ok(d)
    }

    /// Compliance in the order `[11, 22, 33, 12, 23, 13]`.
    pub fn compliance(&self) -> M6 {
        let mut s = [[0.0; 6]; 6];
        s[0][0] = 1.0 / self.e1;
        s[1][1] = 1.0 / self.e2;
        s[2][2] = 1.0 / self.e3;
        s[0][1] = -self.nu12 / self.e1;
        s[1][0] = s[0][1];
        s[0][2] = -self.nu13 / self.e1;
        s[2][0] = s[0][2];
        s[1][2] = -self.nu23 / self.e2;
        s[2][1] = s[1][2];
        s[3][3] = 1.0 / self.g12;
        s[4][4] = 1.0 / self.g23;
        s[5][5] = 1.0 / self.g13;
        s
    }

    /// Stiffness in the material frame, order `[11, 22, 33, 12, 23, 13]`.
    pub fn stiffness(&self) -> Option<M6> {
        invert6(&self.compliance())
    }

    /// `ORTHO` constants `[D1111, D1122, D2222, D1133, D2233, D3333, D1212, D1313, D2323]`.
    pub fn ortho_d(&self) -> [f64; 9] {
        let (nu21, nu31, nu32) = self.minor_nu();
        let d = self.delta();
        [
            self.e1 * (1.0 - self.nu23 * nu32) / d,
            self.e1 * (nu21 + nu31 * self.nu23) / d,
            self.e2 * (1.0 - self.nu13 * nu31) / d,
            self.e1 * (nu31 + nu21 * nu32) / d,
            self.e2 * (nu32 + self.nu12 * nu31) / d,
            self.e3 * (1.0 - self.nu12 * nu21) / d,
            self.g12,
            self.g13,
            self.g23,
        ]
    }

    /// Stiffness and moduli scaled by `r` (strengths untouched).
    pub fn stiffness_scaled(&self, r: f64) -> Self {
        let mut c = self.clone();
        for v in [&mut c.e1, &mut c.e2, &mut c.e3, &mut c.g12, &mut c.g13, &mut c.g23] {
            *v *= r;
        }
        c
    }

    /// Retention ratio at `t` including the creep knockdown.
    pub fn retention_at(&self, t: f64) -> f64 {
        let r = self.retention.as_ref().map_or(1.0, |tab| tab.at(t));
        (r * self.creep_factor).clamp(1e-3, 1.0)
    }

    pub fn hdt_045(&self) -> Option<f64> {
        self.hdt_045_c
    }
}

/// Apparent modulus at temperature `t` from heat-deflection anchors, by
/// log-linear interpolation through `(23, e23)`, `(t1, 900)`, `(t2, 225)`
/// and extrapolation along the last segment.
pub fn hdt_modulus_mpa(e23: f64, hdt_045: f64, hdt_18: Option<f64>, t: f64) -> f64 {
    let (t1, t2) = hdt_anchor_temps(hdt_045, hdt_18);
    let pts = [(ROOM_C, e23.ln()), (t1, HDT_18_MODULUS_MPA.ln()), (t2, HDT_045_MODULUS_MPA.ln())];
    let seg = if t <= pts[1].0 { 0 } else { 1 };
    let (a, b) = (pts[seg], pts[seg + 1]);
    (a.1 + (t - a.0) / (b.0 - a.0) * (b.1 - a.1)).exp()
}

fn hdt_anchor_temps(hdt_045: f64, hdt_18: Option<f64>) -> (f64, f64) {
    match hdt_18 {
        Some(h18) if h18 < hdt_045 => (h18, hdt_045),
        _ => (hdt_045 - 15.0, hdt_045),
    }
}

/// Retention table anchored on heat-deflection temperatures.
pub fn hdt_anchor_table(e23: f64, hdt_045: f64, hdt_18: Option<f64>, n: usize, floor: f64) -> Result<RetentionTable, String> {
    let (t1, t2) = hdt_anchor_temps(hdt_045, hdt_18);
    if t1 <= ROOM_C + 1.0 {
        return Err(format!("HDT {hdt_045} degC is too close to room temperature for an anchored curve"));
    }
    let tmax = hdt_045 + 20.0;
    let mut ts = vec![ROOM_C, t1, t2, tmax];
    fill_gaps(&mut ts, n.clamp(4, 8));
    let points = ts
        .iter()
        .map(|&t| (t, (hdt_modulus_mpa(e23, hdt_045, hdt_18, t) / e23).clamp(floor, 1.0)))
        .collect();
    Ok(RetentionTable { points, source: "hdt_anchor".into(), uncalibrated: true })
}

/// Retention table from the stored dynamic-mechanical anchors.
pub fn dma_anchor_table(a: &DmaAnchors, floor: f64) -> RetentionTable {
    let raw = [
        (ROOM_C, 1.0),
        (40.0, a.r40),
        (a.t_onset_c.max(41.0), a.r40),
        (a.tg_onset_c, 0.2),
        (a.tg_onset_c + 20.0, 0.05),
    ];
    let mut points: Vec<(f64, f64)> = Vec::new();
    for (t, r) in raw {
        if points.last().is_none_or(|p| t > p.0) {
            points.push((t, r.clamp(floor, 1.0)));
        }
    }
    RetentionTable { points, source: "dma_anchor".into(), uncalibrated: true }
}

/// Retention table from a measured `T, E'` curve, normalised at 23 degC.
pub fn dma_csv_table(text: &str, _n: usize, floor: f64) -> Result<RetentionTable, String> {
    let mut rows: Vec<(f64, f64)> = Vec::new();
    for line in text.lines() {
        let cells: Vec<&str> = line.split([',', ';', '\t']).map(str::trim).collect();
        if cells.len() < 2 {
            continue;
        }
        if let (Ok(t), Ok(e)) = (cells[0].parse::<f64>(), cells[1].parse::<f64>()) {
            if t.is_finite() && e.is_finite() && e > 0.0 {
                rows.push((t, e));
            }
        }
    }
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.dedup_by(|a, b| a.0 == b.0);
    if rows.len() < 2 {
        return Err("a DMA curve needs at least two numeric `T,E` rows".into());
    }
    let interp = |t: f64| -> f64 {
        if t <= rows[0].0 {
            return rows[0].1;
        }
        for w in rows.windows(2) {
            if t <= w[1].0 {
                return w[0].1 + (t - w[0].0) / (w[1].0 - w[0].0) * (w[1].1 - w[0].1);
            }
        }
        rows[rows.len() - 1].1
    };
    let e23 = interp(ROOM_C);
    let tmax = rows[rows.len() - 1].0;
    if tmax <= ROOM_C {
        return Err("DMA curve must extend above 23 degC".into());
    }
    let mut ts: Vec<f64> = vec![ROOM_C];
    ts.extend(rows.iter().map(|r| r.0).filter(|&t| t > ROOM_C));
    ts.sort_by(|a, b| a.total_cmp(b));
    ts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    if ts.len() > 8 {
        let step = (ts.len() - 1) as f64 / 7.0;
        ts = (0..8).map(|i| ts[((i as f64) * step).round() as usize]).collect();
    }
    let points = ts.iter().map(|&t| (t, (interp(t) / e23).clamp(floor, 1.0))).collect();
    Ok(RetentionTable { points, source: "dma_csv".into(), uncalibrated: false })
}

/// Insert midpoints of the widest gap until `ts` has `n` sorted unique values.
fn fill_gaps(ts: &mut Vec<f64>, n: usize) {
    ts.sort_by(|a, b| a.total_cmp(b));
    ts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    while ts.len() < n {
        let (i, _) = ts
            .windows(2)
            .enumerate()
            .map(|(i, w)| (i, w[1] - w[0]))
            .fold((0, 0.0), |acc, x| if x.1 > acc.1 { x } else { acc });
        let mid = 0.5 * (ts[i] + ts[i + 1]);
        ts.insert(i + 1, mid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g2_card() -> MaterialCard {
        MaterialCard::trans_iso("g2", 2500.0, 2000.0, 0.35, 0.30, 800.0)
    }

    #[test]
    fn g2_constants_and_closed_form_agree_with_inversion() {
        let c = g2_card();
        assert!((c.g12 - 925.926).abs() < 1e-3);
        let (nu21, nu31, _) = c.minor_nu();
        assert!((nu21 - 0.35).abs() < 1e-12 && (nu31 - 0.24).abs() < 1e-12);
        assert!((c.delta() - 0.6831).abs() < 1e-4);
        let d = c.ortho_d();
        let expect = [3396.282, 1544.430, 3396.282, 1185.771, 1185.771, 2569.170, 925.926, 800.0, 800.0];
        for (a, b) in d.iter().zip(expect) {
            assert!((a - b).abs() < 5e-3, "{a} vs {b}");
        }
        let k = c.stiffness().unwrap();
        assert!((k[0][0] - d[0]).abs() < 1e-8);
        assert!((k[0][1] - d[1]).abs() < 1e-8);
        assert!((k[1][1] - d[2]).abs() < 1e-8);
        assert!((k[0][2] - d[3]).abs() < 1e-8);
        assert!((k[1][2] - d[4]).abs() < 1e-8);
        assert!((k[2][2] - d[5]).abs() < 1e-8);
        assert!((k[3][3] - d[6]).abs() < 1e-8);
        assert!((k[5][5] - d[7]).abs() < 1e-8);
        assert!((k[4][4] - d[8]).abs() < 1e-8);
        assert!(c.check_stability().is_ok());
    }

    #[test]
    fn isotropic_lame_constants() {
        let mut c = MaterialCard::trans_iso("iso", 3000.0, 3000.0, 0.35, 0.35, 3000.0 / 2.7);
        c.g12 = 3000.0 / 2.7;
        let d = c.ortho_d();
        assert!((d[0] - 4814.815).abs() < 1e-3);
        assert!((d[1] - 2592.593).abs() < 1e-3);
        assert!((c.g12 - 1111.111).abs() < 1e-3);
    }

    #[test]
    fn unstable_constants_are_rejected() {
        let mut c = g2_card();
        c.nu13 = 0.9;
        assert!(c.check_stability().is_err());
        let mut c = g2_card();
        c.e3 = -1.0;
        assert!(c.check_stability().is_err());
        let mut c = g2_card();
        c.nu12 = 0.8;
        assert!(c.check_stability().is_err());
    }

    #[test]
    fn presets_are_consistent_with_the_report() {
        for p in &PRESETS {
            let card = MaterialCard::trans_iso(p.name, p.e_xy, p.e_z, p.nu_xy, p.nu_xz, p.e_z / (2.0 * (1.0 + p.nu_xz)));
            assert!(card.check_stability().is_ok(), "{}", p.name);
        }
        let ppa = preset("PPA-CF").unwrap();
        assert!((ppa.xt * ppa.z_ratio - 57.0).abs() < 1e-9);
        assert!((ppa.z_ratio - 0.3393).abs() < 1e-4);
        assert_eq!(preset("paht cf").unwrap().hdt_045(true), 190.7);
        assert_eq!(preset("paht_cf").unwrap().hdt_045(false), 84.8);
        assert_eq!(preset("pla").unwrap().z_ratio, 0.55);
        assert_eq!(preset("petg").unwrap().z_ratio, 0.46);
        assert_eq!(preset("asa").unwrap().z_ratio, 0.29);
        assert_eq!(preset("pet-cf").unwrap().water_abs_pct, Some(0.37));
        assert!(preset("nope").is_none());
    }

    #[test]
    fn presets_read_the_shared_material_table() {
        // heat, Z ratio, strength and density are the numbers the print and
        // solar stages use, so a material cannot pass one stage on table drift
        for p in &PRESETS {
            let s = vernon_core::materials::print_material(p.id).unwrap();
            assert!(std::ptr::eq(p.shared, s) || p.shared == s, "{}", p.id);
            assert_eq!((p.hdt_045_c, p.hdt_045_annealed_c, p.z_ratio, p.xt, p.density_g_cm3), (s.hdt_045_c, s.hdt_045_annealed_c, s.z_ratio, s.sigma_xy_mpa, s.density_g_cm3));
        }
        assert_eq!(preset("petg").unwrap().hdt_045_c, 69.0);
        assert_eq!(preset("asa").unwrap().hdt_045_c, 100.0);
        assert!(preset("paht_cf").unwrap().shared.z_ratio_assumed());
    }

    #[test]
    fn g6_dma_curve_returns_the_given_ratios() {
        for (pla, e23, e40) in [("PLA", 1980.0, 1820.0), ("PETG", 1100.0, 883.0), ("ASA", 1210.0, 976.0), ("ABS", 1030.0, 875.0)] {
            let csv = format!("T,E\n23,{e23}\n40,{e40}\n");
            let t = dma_csv_table(&csv, 5, 0.05).unwrap();
            assert!((t.at(23.0) - 1.0).abs() < 1e-12, "{pla}");
            assert!((t.at(40.0) - e40 / e23).abs() < 1e-3, "{pla}");
        }
        assert!(dma_csv_table("junk", 5, 0.05).is_err());
    }

    #[test]
    fn g6_hdt_anchors_interpolate_log_linearly() {
        let e = hdt_modulus_mpa(7500.0, 227.0, Some(196.0), 211.5);
        assert!((e - 450.0).abs() < 1e-9);
        assert!((hdt_modulus_mpa(7500.0, 227.0, Some(196.0), 196.0) - 900.0).abs() < 1e-9);
        assert!((hdt_modulus_mpa(7500.0, 227.0, Some(196.0), 227.0) - 225.0).abs() < 1e-9);
        let t = hdt_anchor_table(7500.0, 227.0, Some(196.0), 6, 0.05).unwrap();
        assert!(t.points.len() == 6 && (t.at(23.0) - 1.0).abs() < 1e-12);
        assert!(t.points.windows(2).all(|w| w[1].0 > w[0].0 && w[1].1 <= w[0].1));
        assert!(t.at(300.0) >= 0.05);
        assert!(hdt_anchor_table(7500.0, 30.0, None, 6, 0.05).is_err());
    }

    #[test]
    fn dma_anchor_table_is_monotone() {
        for p in PRESETS.iter().filter(|p| p.dma.is_some()) {
            let t = dma_anchor_table(&p.dma.unwrap(), 0.05);
            assert!(t.points.windows(2).all(|w| w[1].0 > w[0].0), "{}", p.name);
            assert!((t.at(40.0) - p.dma.unwrap().r40).abs() < 1e-9);
        }
    }

    #[test]
    fn retention_includes_creep_factor() {
        let mut c = g2_card();
        c.retention = Some(RetentionTable { points: vec![(23.0, 1.0), (60.0, 0.5)], source: "t".into(), uncalibrated: false });
        c.creep_factor = 0.8;
        assert!((c.retention_at(23.0) - 0.8).abs() < 1e-12);
        assert!((c.retention_at(60.0) - 0.4).abs() < 1e-12);
    }
}

//! Shared printed-material data: the one table the print, solar and
//! structural stages read, so the same material choice cannot pass one
//! stage's heat or strength check and fail the next on table drift alone.
//!
//! Every value carries a source tag. `report` values come from the project's
//! body-kit toolchain note; ranges in the note are taken at their LOW end
//! (the conservative side for every heat check). `assumed` values are
//! placeholders until measured and every node that uses one says so.

use serde::Serialize;

/// Where a number comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Source {
    /// Stated in the project's toolchain note.
    Report,
    /// A filament vendor's datasheet figure quoted by the note.
    Vendor,
    /// Placeholder until datasheet or coupon data for the actual spool is used.
    Assumed,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Report => "report",
            Source::Vendor => "vendor datasheet",
            Source::Assumed => "assumed",
        }
    }
}

/// One printable material.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PrintMaterial {
    /// Lower-case id (`petg`, `paht_cf`).
    pub id: &'static str,
    /// Display name (`PETG`, `PAHT-CF`).
    pub name: &'static str,
    /// Heat deflection temperature at 0.45 MPa as printed, degC.
    pub hdt_045_c: f64,
    /// Same after a post-print anneal, when it differs.
    pub hdt_045_annealed_c: Option<f64>,
    /// Heat deflection temperature at 1.8 MPa, when given.
    pub hdt_18_c: Option<f64>,
    /// Vendor datasheet heat-deflection figure, when the note quotes one
    /// that differs from the as-printed value.
    pub hdt_vendor_c: Option<f64>,
    pub hdt_source: Source,
    pub hdt_basis: &'static str,
    /// Stacking-direction (Z) allowable over the in-plane allowable.
    pub z_ratio: f64,
    pub z_ratio_source: Source,
    pub z_ratio_basis: &'static str,
    /// In-plane tensile allowable, MPa.
    pub sigma_xy_mpa: f64,
    pub sigma_source: Source,
    /// Density, g/cm3.
    pub density_g_cm3: f64,
    pub density_source: Source,
    /// Water absorption, percent by mass.
    pub water_abs_pct: Option<f64>,
    pub water_source: Source,
}

impl PrintMaterial {
    /// Heat deflection temperature at 0.45 MPa for the chosen treatment.
    pub fn hdt(&self, annealed: bool) -> f64 {
        match (annealed, self.hdt_045_annealed_c) {
            (true, Some(h)) => h,
            _ => self.hdt_045_c,
        }
    }

    /// True when the Z ratio is a placeholder.
    pub fn z_ratio_assumed(&self) -> bool {
        self.z_ratio_source == Source::Assumed
    }

    /// Note for a report that uses the Z ratio.
    pub fn z_ratio_note(&self) -> String {
        format!("{} Z-strength ratio {:.2} ({}: {})", self.name, self.z_ratio, self.z_ratio_source.label(), self.z_ratio_basis)
    }
}

pub const PLA: PrintMaterial = PrintMaterial {
    id: "pla",
    name: "PLA",
    hdt_045_c: 57.0,
    hdt_045_annealed_c: None,
    hdt_18_c: None,
    hdt_vendor_c: None,
    hdt_source: Source::Report,
    hdt_basis: "57 degC, below every dark or mid-grey panel temperature",
    z_ratio: 0.55,
    z_ratio_source: Source::Report,
    z_ratio_basis: "upright strength about 55 % of flat",
    sigma_xy_mpa: 50.0,
    sigma_source: Source::Assumed,
    density_g_cm3: 1.24,
    density_source: Source::Assumed,
    water_abs_pct: None,
    water_source: Source::Assumed,
};

pub const PETG: PrintMaterial = PrintMaterial {
    id: "petg",
    name: "PETG",
    hdt_045_c: 69.0,
    hdt_045_annealed_c: None,
    hdt_18_c: None,
    hdt_vendor_c: None,
    hdt_source: Source::Report,
    hdt_basis: "69 to 72 degC; the low end is used",
    z_ratio: 0.46,
    z_ratio_source: Source::Report,
    z_ratio_basis: "upright strength about 46 % of flat",
    sigma_xy_mpa: 45.0,
    sigma_source: Source::Assumed,
    density_g_cm3: 1.27,
    density_source: Source::Assumed,
    water_abs_pct: None,
    water_source: Source::Assumed,
};

pub const ASA: PrintMaterial = PrintMaterial {
    id: "asa",
    name: "ASA",
    hdt_045_c: 100.0,
    hdt_045_annealed_c: None,
    hdt_18_c: None,
    hdt_vendor_c: None,
    hdt_source: Source::Report,
    hdt_basis: "100 to 103 degC; the low end is used; marginal on black",
    z_ratio: 0.29,
    z_ratio_source: Source::Report,
    z_ratio_basis: "upright strength about 29 % of flat",
    sigma_xy_mpa: 40.0,
    sigma_source: Source::Assumed,
    density_g_cm3: 1.07,
    density_source: Source::Assumed,
    water_abs_pct: None,
    water_source: Source::Assumed,
};

pub const PET_CF: PrintMaterial = PrintMaterial {
    id: "pet_cf",
    name: "PET-CF",
    hdt_045_c: 205.0,
    hdt_045_annealed_c: None,
    hdt_18_c: None,
    hdt_vendor_c: None,
    hdt_source: Source::Report,
    hdt_basis: "205 degC",
    z_ratio: 0.40,
    z_ratio_source: Source::Assumed,
    z_ratio_basis: "not in the report: measure on coupons",
    sigma_xy_mpa: 70.0,
    sigma_source: Source::Assumed,
    density_g_cm3: 1.30,
    density_source: Source::Assumed,
    water_abs_pct: Some(0.37),
    water_source: Source::Report,
};

pub const PAHT_CF: PrintMaterial = PrintMaterial {
    id: "paht_cf",
    name: "PAHT-CF",
    hdt_045_c: 84.8,
    hdt_045_annealed_c: Some(190.7),
    hdt_18_c: None,
    hdt_vendor_c: Some(194.0),
    hdt_source: Source::Report,
    hdt_basis: "84.8 degC as printed, 190.7 degC after a post-print anneal (vendor quotes 194 degC)",
    z_ratio: 0.40,
    z_ratio_source: Source::Assumed,
    z_ratio_basis: "not in the report: measure on coupons",
    sigma_xy_mpa: 85.0,
    sigma_source: Source::Assumed,
    density_g_cm3: 1.25,
    density_source: Source::Assumed,
    water_abs_pct: Some(2.35),
    water_source: Source::Assumed,
};

pub const PPA_CF: PrintMaterial = PrintMaterial {
    id: "ppa_cf",
    name: "PPA-CF",
    hdt_045_c: 227.0,
    hdt_045_annealed_c: None,
    hdt_18_c: Some(196.0),
    hdt_vendor_c: None,
    hdt_source: Source::Report,
    hdt_basis: "227 degC at 0.45 MPa, 196 degC at 1.8 MPa",
    z_ratio: 57.0 / 168.0,
    z_ratio_source: Source::Report,
    z_ratio_basis: "57 MPa through-stack against 168 MPa in plane",
    sigma_xy_mpa: 168.0,
    sigma_source: Source::Report,
    density_g_cm3: 1.28,
    density_source: Source::Assumed,
    water_abs_pct: None,
    water_source: Source::Assumed,
};

/// Every printable material, in menu order.
pub const PRINT_MATERIALS: [PrintMaterial; 6] = [PLA, PETG, ASA, PET_CF, PAHT_CF, PPA_CF];

/// Look up a material by id or name, ignoring case and `-`/`_`/space.
pub fn print_material(id: &str) -> Option<&'static PrintMaterial> {
    let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let want = norm(id);
    PRINT_MATERIALS.iter().find(|m| norm(m.id) == want || norm(m.name) == want)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_and_conventions() {
        assert_eq!(print_material("PAHT-CF").unwrap().id, "paht_cf");
        assert_eq!(print_material("pet_cf").unwrap().name, "PET-CF");
        assert!(print_material("nylon").is_none());
        // ranges in the note are taken at the low end
        assert_eq!(PETG.hdt_045_c, 69.0);
        assert_eq!(ASA.hdt_045_c, 100.0);
        assert_eq!(ASA.z_ratio, 0.29);
        assert_eq!(PAHT_CF.hdt(true), 190.7);
        assert_eq!(PAHT_CF.hdt(false), 84.8);
        // unsourced Z ratios are flagged
        assert!(PET_CF.z_ratio_assumed() && PAHT_CF.z_ratio_assumed());
        assert!(!PPA_CF.z_ratio_assumed());
        assert!(PET_CF.z_ratio_note().contains("assumed"));
    }
}

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

/// Weathering evidence for one material: the strength it kept and the colour
/// change it showed after accelerated ultraviolet exposure. The figures come
/// from a manufacturer's white paper cited in the project note; there is no
/// published factor that turns test hours into outdoor years for a hot, sunny
/// climate, so they rank materials and do not predict a service life.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct UvData {
    /// Stress at break kept after the exposure, percent of the unexposed value.
    pub retention_pct: f64,
    /// Colour change dE after the same exposure, when the source gives a number.
    pub delta_e: Option<f64>,
    /// Exposure in hours of fluorescent-UV weathering (a G154-type cycle).
    pub hours: f64,
    pub source: Source,
    pub basis: &'static str,
}

/// One material with weathering evidence; not every one is a printable material of the table.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct UvEvidence {
    pub name: &'static str,
    pub uv: UvData,
}

const ASA_UV: UvData = UvData {
    retention_pct: 97.0,
    delta_e: None,
    hours: 1000.0,
    source: Source::Report,
    basis: "kept 97.0 % of its stress at break after 1000 h of fluorescent-UV exposure",
};

/// The three materials the project note quotes UV figures for.
pub const UV_EVIDENCE: [UvEvidence; 3] = [
    UvEvidence { name: "ASA", uv: ASA_UV },
    UvEvidence {
        name: "Nylon 12CF",
        uv: UvData {
            retention_pct: 136.8,
            delta_e: None,
            hours: 1000.0,
            source: Source::Report,
            basis: "ended at 136.8 % of its stress at break with almost no colour change after 1000 h of fluorescent-UV exposure",
        },
    },
    UvEvidence {
        name: "PC (unpainted)",
        uv: UvData {
            retention_pct: 70.8,
            delta_e: Some(33.0),
            hours: 1000.0,
            source: Source::Report,
            basis: "fell to 70.8 % of its stress at break with a colour change of about 33 dE after 1000 h of fluorescent-UV exposure",
        },
    },
];

/// Weathering evidence for a material name, ignoring case and `-`/`_`/space; `None` when the note has none.
pub fn uv_data(name: &str) -> Option<UvData> {
    let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let want = norm(name);
    UV_EVIDENCE.iter().find(|e| norm(e.name) == want).map(|e| e.uv)
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
    /// Weathering evidence, when the note quotes some. A material without it
    /// has no UV figure at all: its protection has to come from a coating.
    pub uv: Option<UvData>,
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
    uv: None,
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
    uv: None,
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
    uv: Some(ASA_UV),
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
    uv: None,
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
    uv: None,
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
    uv: None,
};

/// Every printable material, in menu order.
pub const PRINT_MATERIALS: [PrintMaterial; 6] = [PLA, PETG, ASA, PET_CF, PAHT_CF, PPA_CF];

/// Look up a material by id or name, ignoring case and `-`/`_`/space.
pub fn print_material(id: &str) -> Option<&'static PrintMaterial> {
    let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let want = norm(id);
    PRINT_MATERIALS.iter().find(|m| norm(m.id) == want || norm(m.name) == want)
}

/// Initial snap-strain screening allowance, percent, for a printed material.
/// These are deliberately assumed values, not a filament certification or
/// a transfer of molded-material strain limits to a printed joint. The
/// rectangular cantilever method is from Covestro, Snap-Fit Joints for Plastic,
/// table 1; its table 2 applies to specific molded grades at 23 C. Actual print
/// orientation, temperature, conditioning, cycles and root concentration need
/// coupons before relying on these screening limits.
/// Source: https://solutions.covestro.com/-/media/covestro/solution-center/brands/downloads/imported/1557218421.pdf
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SnapStrainAllowance {
    pub material_id: &'static str,
    pub allowable_pct: f64,
    pub source: Source,
    pub basis: &'static str,
}

pub const SNAP_STRAIN_ALLOWANCES: [SnapStrainAllowance; 6] = [
    SnapStrainAllowance {material_id:"pla",allowable_pct:1.0,source:Source::Assumed,basis:"initial room-temperature printed PLA screening assumption"},
    SnapStrainAllowance {material_id:"petg",allowable_pct:2.0,source:Source::Assumed,basis:"initial room-temperature printed PETG screening assumption"},
    SnapStrainAllowance {material_id:"asa",allowable_pct:2.0,source:Source::Assumed,basis:"initial room-temperature printed ASA screening assumption"},
    SnapStrainAllowance {material_id:"pet_cf",allowable_pct:0.6,source:Source::Assumed,basis:"initial room-temperature filled-polyester screening assumption"},
    SnapStrainAllowance {material_id:"paht_cf",allowable_pct:1.0,source:Source::Assumed,basis:"initial room-temperature filled-nylon screening assumption"},
    SnapStrainAllowance {material_id:"ppa_cf",allowable_pct:0.8,source:Source::Assumed,basis:"initial room-temperature filled-PPA screening assumption"},
];

pub fn snap_strain_allowance(id: &str) -> Option<&'static SnapStrainAllowance> {
    let material = print_material(id)?;
    SNAP_STRAIN_ALLOWANCES.iter().find(|entry| entry.material_id==material.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_screening_table_covers_each_print_material_and_labels_assumptions() {
        for material in PRINT_MATERIALS {
            let limit = snap_strain_allowance(material.name).unwrap();
            assert_eq!(limit.material_id,material.id);
            assert!(limit.allowable_pct.is_finite() && limit.allowable_pct>0.0 && limit.allowable_pct<5.0);
            assert_eq!(limit.source,Source::Assumed);
            assert!(limit.basis.contains("assumption"));
        }
        assert!(snap_strain_allowance("unknown").is_none());
    }

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

    #[test]
    fn weathering_evidence_is_the_notes_three_materials() {
        let asa = uv_data("ASA").unwrap();
        assert_eq!((asa.retention_pct, asa.delta_e, asa.hours), (97.0, None, 1000.0));
        assert_eq!(ASA.uv, Some(asa));
        let pc = uv_data("pc (unpainted)").unwrap();
        assert_eq!((pc.retention_pct, pc.delta_e), (70.8, Some(33.0)));
        assert_eq!(uv_data("Nylon-12CF").unwrap().retention_pct, 136.8);
        // every other printable material has no UV figure: its protection must come from the coating
        for m in PRINT_MATERIALS.iter().filter(|m| m.id != "asa") {
            assert!(m.uv.is_none() && uv_data(m.name).is_none(), "{}", m.name);
        }
        assert!(UV_EVIDENCE.iter().all(|e| e.uv.source == Source::Report));
    }
}

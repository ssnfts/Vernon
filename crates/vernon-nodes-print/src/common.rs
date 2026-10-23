//! Shared tables and helpers for the print-stage nodes.

use std::path::{Path, PathBuf};

use glam::DVec3;
use serde_json::Value;
use vernon_core::{Report, Severity};
use vernon_graph::{CookContext, CookResult, NodeTypeInfo, Stage};

use crate::cut::Plane;

pub const SPEC: &str = "docs/specs/slicing-printing.md";

/// Start a Stage::Print node description with the shared spec path.
pub fn info(id: &str, label: &str, help: &str) -> NodeTypeInfo {
    NodeTypeInfo::new(id, label, Stage::Print).help(help).spec(SPEC)
}

/// Add the standard white-box warning to a report.
pub fn placeholder(report: &mut Report, what: &str) {
    report.note(Severity::Warning, format!("white-box placeholder: {what}"));
}

/// Ensure the node work directory exists and return it.
pub fn work_dir(ctx: &CookContext) -> CookResult<PathBuf> {
    std::fs::create_dir_all(&ctx.work_dir)?;
    Ok(ctx.work_dir.clone())
}

/// A path parameter, falling back to `work_dir/default_name` when empty.
pub fn path_or_default(ctx: &CookContext, value: &str, default_name: &str) -> CookResult<PathBuf> {
    if value.trim().is_empty() {
        Ok(work_dir(ctx)?.join(default_name))
    } else {
        let p = Path::new(value);
        if let Some(parent) = p.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        Ok(p.to_path_buf())
    }
}

// ---------------------------------------------------------------- printers

pub const PRINTER_IDS: [&str; 5] = ["k1_max", "k2_plus", "h2d", "giga_800", "custom"];

/// Build volume in mm for a printer preset id; `custom` returns `custom_bed`.
pub fn printer_bed(id: &str, custom_bed: DVec3) -> DVec3 {
    match id {
        "k1_max" => DVec3::new(300.0, 300.0, 300.0),
        "k2_plus" => DVec3::new(350.0, 350.0, 350.0),
        "h2d" => DVec3::new(350.0, 320.0, 325.0),
        "giga_800" => DVec3::new(800.0, 800.0, 1000.0),
        _ => custom_bed,
    }
}

// --------------------------------------------------------------- materials

pub const MATERIAL_IDS: [&str; 6] = ["pla", "petg", "asa", "pet_cf", "paht_cf", "ppa_cf"];

/// Material data: heat, strength and density come from the shared table in
/// `vernon_core::materials` (also read by the solar and structural stages);
/// only the printing temperatures are print-specific.
#[derive(Debug, Clone, Copy)]
pub struct Material {
    pub id: &'static str,
    /// The shared row, with its source tags.
    pub shared: &'static vernon_core::materials::PrintMaterial,
    /// Ratio of the Z (layer-normal) allowable to the in-plane allowable.
    pub z_ratio: f64,
    /// Heat deflection temperature at 0.45 MPa, deg C.
    pub hdt_c: f64,
    /// Same after annealing (equal when annealing does not apply).
    pub hdt_annealed_c: f64,
    /// Density g/cm3 (assumed typical value).
    pub density: f64,
    /// In-plane tensile allowable MPa (assumed except where the report gives it).
    pub sigma_xy_mpa: f64,
    pub nozzle_c: f64,
    pub bed_c: f64,
}

const fn mat(m: &'static vernon_core::materials::PrintMaterial, nozzle_c: f64, bed_c: f64) -> Material {
    Material {
        id: m.id,
        shared: m,
        z_ratio: m.z_ratio,
        hdt_c: m.hdt_045_c,
        hdt_annealed_c: match m.hdt_045_annealed_c {
            Some(h) => h,
            None => m.hdt_045_c,
        },
        density: m.density_g_cm3,
        sigma_xy_mpa: m.sigma_xy_mpa,
        nozzle_c,
        bed_c,
    }
}

const MATERIALS: [Material; 6] = {
    use vernon_core::materials as m;
    [
        mat(&m::PLA, 215.0, 60.0),
        mat(&m::PETG, 240.0, 80.0),
        mat(&m::ASA, 250.0, 100.0),
        mat(&m::PET_CF, 280.0, 80.0),
        mat(&m::PAHT_CF, 290.0, 90.0),
        mat(&m::PPA_CF, 300.0, 100.0),
    ]
};

pub fn material(id: &str) -> Material {
    MATERIALS.iter().copied().find(|m| m.id == id).unwrap_or(MATERIALS[0])
}

// --------------------------------------------------------------- json help

pub fn v3_json(v: DVec3) -> Value {
    serde_json::json!([v.x, v.y, v.z])
}

pub fn json_v3(v: &Value) -> Option<DVec3> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some(DVec3::new(a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?))
}

/// Parse a plane list: an array of `{origin|point:[x,y,z], normal:[x,y,z]}`
/// objects or `{planes:[...]}`. Unusable entries are returned as messages.
pub fn planes_from_json(v: &Value) -> (Vec<Plane>, Vec<String>) {
    let arr = match v.get("planes").and_then(|p| p.as_array()).or_else(|| v.as_array()) {
        Some(a) => a,
        None => return (Vec::new(), vec!["plane list is not an array".into()]),
    };
    let mut planes = Vec::new();
    let mut warn = Vec::new();
    for (i, e) in arr.iter().enumerate() {
        let o = e.get("origin").or_else(|| e.get("point")).and_then(json_v3);
        let n = e.get("normal").and_then(json_v3);
        match (o, n) {
            (Some(o), Some(n)) => match Plane::new(o, n) {
                Some(p) => planes.push(p),
                None => warn.push(format!("plane {i}: zero or non-finite normal skipped")),
            },
            _ => warn.push(format!("plane {i}: needs origin and normal, skipped")),
        }
    }
    (planes, warn)
}

#[cfg(test)]
pub mod testutil {
    use std::path::PathBuf;

    use vernon_graph::{CookContext, CookResult, Data, NodeOp, ParamValue, Params};

    /// Cook a node with default parameters plus overrides.
    pub fn cook(op: &dyn NodeOp, name: &str, inputs: &[Option<Data>], set: &[(&str, ParamValue)]) -> CookResult<Vec<Data>> {
        let info = op.info();
        let mut p = Params::from_specs(&info.params);
        for (k, v) in set {
            p.set(k, v.clone());
        }
        op.cook(&ctx(name), inputs, &p)
    }

    pub fn ctx(name: &str) -> CookContext {
        let dir = std::env::temp_dir().join(format!("vernon-print-{name}-{}", std::process::id()));
        CookContext { node_name: name.into(), work_dir: dir, frame: 0.0 }
    }

    pub fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vernon-print-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}

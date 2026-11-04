//! Static schema of slicer preset keys and a validator for flat key/value presets.
//!
//! Key names follow the plain-text preset format used by the open-source slicers in the
//! Slic3r family (INI / JSON presets); a key name is a fact about a file format, not
//! copied text. Each entry carries a `doc` topic naming the public settings-reference
//! section where the setting is described. The topics are generic section titles;
//! exact page URLs are UNVERIFIED and deliberately not recorded here.
//!
//! NOTES - UNVERIFIED / left out: per-feature speed keys for bridges over infill,
//! "seam gap", adaptive pressure-advance model tables, and any key introduced after
//! the presets this schema was written against. They are not listed because I could
//! not tie them to a documentation page; `validate_profile` reports them as
//! `UnknownKey` rather than guessing a range. The numeric ranges are sanity bounds
//! for a desktop FFF printer, not values taken from the documentation (UNVERIFIED).
use std::collections::BTreeMap;

/// Value type and permitted range / vocabulary of one key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyType {
    /// Integer in `[min, max]`.
    Int(i64, i64),
    /// Finite float in `[min, max]`.
    Float(f64, f64),
    /// Percentage written with a trailing `%`, number in `[min, max]`.
    Percent(f64, f64),
    /// Float, or a percentage of another quantity (e.g. `"150%"`), number in `[min, max]`.
    FloatOrPercent(f64, f64),
    /// `0`/`1`/`true`/`false`.
    Bool,
    /// One of the listed tokens.
    Enum(&'static [&'static str]),
    /// Non-empty free text (names, templates).
    Text,
    /// Comma- or semicolon-separated floats, each in `[min, max]`.
    FloatList(f64, f64),
}

/// One schema entry.
#[derive(Debug, Clone, Copy)]
pub struct KeySpec {
    pub name: &'static str,
    pub ty: KeyType,
    /// Public documentation topic.
    pub doc: &'static str,
}

const FLAVOURS: &[&str] = &["marlin", "marlin2", "klipper", "reprapfirmware", "repetier", "reprap"];
const INFILL: &[&str] = &[
    "grid", "triangles", "stars", "cubic", "line", "concentric", "honeycomb", "gyroid",
    "3dhoneycomb", "adaptivecubic", "alignedrectilinear", "lightning", "crosshatch",
];
const SEAM: &[&str] = &["nearest", "aligned", "back", "random"];
const SUPPORT: &[&str] = &["normal(auto)", "tree(auto)", "normal(manual)", "tree(manual)"];
const SKIRT_TYPE: &[&str] = &["combined", "perobject"];
const BED_TYPES: &[&str] = &["Cool Plate", "Engineering Plate", "High Temp Plate", "Textured PEI Plate"];
const WALL_ORDER: &[&str] = &["inner wall/outer wall", "outer wall/inner wall", "inner-outer-inner wall"];
const IRONING: &[&str] = &["no ironing", "top", "topmost", "solid"];

use KeyType::*;

const fn k(name: &'static str, ty: KeyType, doc: &'static str) -> KeySpec {
    KeySpec { name, ty, doc }
}

/// The schema, sorted by name (checked by a test).
pub static SCHEMA: &[KeySpec] = &[
    k("activate_chamber_temp_control", Bool, "filament: chamber"),
    k("bottom_shell_layers", Int(0, 1000), "quality: shells"),
    k("bottom_shell_thickness", Float(0.0, 100.0), "quality: shells"),
    k("bridge_flow", Float(0.5, 2.0), "quality: bridging"),
    k("bridge_speed", Float(1.0, 1000.0), "speed: bridges"),
    k("brim_width", Float(0.0, 100.0), "adhesion: brim"),
    k("chamber_temperature", Float(0.0, 100.0), "filament: chamber"),
    k("cool_plate_temp", FloatList(0.0, 150.0), "filament: bed temperature"),
    k("curr_bed_type", Enum(BED_TYPES), "filament: bed type"),
    k("default_acceleration", Float(0.0, 100000.0), "speed: acceleration"),
    k("detect_overhang_wall", Bool, "quality: overhangs"),
    k("disable_m73", Bool, "printer: g-code"),
    k("draft_shield", Enum(&["disabled", "enabled", "limited"]), "adhesion: skirt"),
    k("enable_pressure_advance", Bool, "filament: pressure advance"),
    k("enable_prime_tower", Bool, "multi-material: prime tower"),
    k("enable_support", Bool, "support: enable"),
    k("eng_plate_temp", FloatList(0.0, 150.0), "filament: bed temperature"),
    k("fan_max_speed", FloatList(0.0, 100.0), "filament: cooling"),
    k("fan_min_speed", FloatList(0.0, 100.0), "filament: cooling"),
    k("filament_density", FloatList(0.1, 30.0), "filament: basic"),
    k("filament_diameter", FloatList(0.5, 5.0), "filament: basic"),
    k("filament_flow_ratio", FloatList(0.5, 1.5), "filament: flow"),
    k("filament_max_volumetric_speed", FloatList(0.0, 200.0), "filament: flow"),
    k("filament_type", Text, "filament: basic"),
    k("first_layer_bed_temperature", FloatList(0.0, 150.0), "filament: bed temperature"),
    k("first_layer_temperature", FloatList(0.0, 500.0), "filament: nozzle temperature"),
    k("gap_infill_speed", Float(1.0, 1000.0), "speed: gap fill"),
    k("gcode_flavor", Enum(FLAVOURS), "printer: g-code flavour"),
    k("hot_plate_temp", FloatList(0.0, 150.0), "filament: bed temperature"),
    k("infill_direction", Float(0.0, 360.0), "strength: infill"),
    k("infill_wall_overlap", Percent(0.0, 100.0), "strength: infill"),
    k("initial_layer_acceleration", Float(0.0, 100000.0), "speed: acceleration"),
    k("initial_layer_print_height", Float(0.01, 5.0), "quality: layer height"),
    k("initial_layer_speed", Float(1.0, 1000.0), "speed: first layer"),
    k("internal_solid_infill_speed", Float(1.0, 1000.0), "speed: infill"),
    k("ironing_type", Enum(IRONING), "quality: ironing"),
    k("layer_height", Float(0.01, 5.0), "quality: layer height"),
    k("line_width", Float(0.1, 10.0), "quality: line width"),
    k("max_layer_height", FloatList(0.01, 5.0), "printer: extruder"),
    k("min_layer_height", FloatList(0.01, 5.0), "printer: extruder"),
    k("nozzle_diameter", FloatList(0.05, 5.0), "printer: extruder"),
    k("nozzle_temperature", FloatList(0.0, 500.0), "filament: nozzle temperature"),
    k("nozzle_temperature_initial_layer", FloatList(0.0, 500.0), "filament: nozzle temperature"),
    k("outer_wall_acceleration", Float(0.0, 100000.0), "speed: acceleration"),
    k("outer_wall_speed", Float(1.0, 1000.0), "speed: walls"),
    k("overhang_fan_speed", FloatList(0.0, 100.0), "filament: cooling"),
    k("pressure_advance", FloatList(0.0, 2.0), "filament: pressure advance"),
    k("print_sequence", Enum(&["by layer", "by object"]), "other: sequence"),
    k("printable_height", Float(1.0, 5000.0), "printer: bed shape"),
    k("raft_layers", Int(0, 100), "support: raft"),
    k("reduce_infill_retraction", Bool, "printer: retraction"),
    k("retract_before_wipe", Percent(0.0, 100.0), "printer: retraction"),
    k("retract_length_toolchange", FloatList(0.0, 100.0), "printer: retraction"),
    k("retraction_length", FloatList(0.0, 100.0), "printer: retraction"),
    k("retraction_minimum_travel", FloatList(0.0, 100.0), "printer: retraction"),
    k("retraction_speed", FloatList(1.0, 1000.0), "printer: retraction"),
    k("seam_position", Enum(SEAM), "quality: seam"),
    k("skirt_distance", Float(0.0, 100.0), "adhesion: skirt"),
    k("skirt_height", Int(0, 1000), "adhesion: skirt"),
    k("skirt_loops", Int(0, 1000), "adhesion: skirt"),
    k("skirt_type", Enum(SKIRT_TYPE), "adhesion: skirt"),
    k("slow_down_layer_time", FloatList(0.0, 600.0), "filament: cooling"),
    k("slow_down_min_speed", FloatList(1.0, 1000.0), "filament: cooling"),
    k("small_perimeter_speed", FloatOrPercent(1.0, 1000.0), "speed: walls"),
    k("sparse_infill_density", Percent(0.0, 100.0), "strength: infill"),
    k("sparse_infill_pattern", Enum(INFILL), "strength: infill"),
    k("sparse_infill_speed", Float(1.0, 1000.0), "speed: infill"),
    k("support_threshold_angle", Int(0, 90), "support: overhang angle"),
    k("support_type", Enum(SUPPORT), "support: type"),
    k("textured_plate_temp", FloatList(0.0, 150.0), "filament: bed temperature"),
    k("top_shell_layers", Int(0, 1000), "quality: shells"),
    k("top_shell_thickness", Float(0.0, 100.0), "quality: shells"),
    k("top_surface_speed", Float(1.0, 1000.0), "speed: top surface"),
    k("travel_acceleration", Float(0.0, 100000.0), "speed: acceleration"),
    k("travel_speed", Float(1.0, 2000.0), "speed: travel"),
    k("use_firmware_retraction", Bool, "printer: retraction"),
    k("use_relative_e_distances", Bool, "printer: g-code"),
    k("wall_generator", Enum(&["classic", "arachne"]), "quality: wall generator"),
    k("wall_loops", Int(1, 1000), "quality: walls"),
    k("wall_sequence", Enum(WALL_ORDER), "quality: wall order"),
    k("wipe", FloatList(0.0, 1.0), "printer: retraction"),
    k("z_hop", FloatList(0.0, 50.0), "printer: retraction"),
];

/// Kind of problem found in a preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingKind {
    UnknownKey,
    WrongType,
    OutOfRange,
}

/// One validation finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileFinding {
    pub key: String,
    pub kind: FindingKind,
    pub message: String,
}

/// Look a key up in the schema (binary search; the schema is sorted).
pub fn lookup(name: &str) -> Option<&'static KeySpec> {
    SCHEMA.binary_search_by(|k| k.name.cmp(name)).ok().map(|i| &SCHEMA[i])
}

enum Check {
    Ok,
    Type(String),
    Range(String),
}

fn num(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

fn ranged(v: f64, lo: f64, hi: f64) -> Check {
    if v < lo || v > hi {
        Check::Range(format!("{v} outside [{lo}, {hi}]"))
    } else {
        Check::Ok
    }
}

fn check(ty: KeyType, raw: &str) -> Check {
    let s = raw.trim();
    match ty {
        Int(lo, hi) => match s.parse::<i64>() {
            Ok(v) if v < lo || v > hi => Check::Range(format!("{v} outside [{lo}, {hi}]")),
            Ok(_) => Check::Ok,
            Err(_) => Check::Type(format!("`{s}` is not an integer")),
        },
        Float(lo, hi) => match num(s) {
            Some(v) => ranged(v, lo, hi),
            None => Check::Type(format!("`{s}` is not a finite number")),
        },
        Percent(lo, hi) => match s.strip_suffix('%').and_then(num) {
            Some(v) => ranged(v, lo, hi),
            None => Check::Type(format!("`{s}` is not a percentage like 15%")),
        },
        FloatOrPercent(lo, hi) => match num(s.strip_suffix('%').unwrap_or(s)) {
            Some(v) => ranged(v, lo, hi),
            None => Check::Type(format!("`{s}` is not a number or percentage")),
        },
        Bool => {
            if matches!(s, "0" | "1" | "true" | "false") {
                Check::Ok
            } else {
                Check::Type(format!("`{s}` is not 0/1/true/false"))
            }
        }
        Enum(vals) => {
            if vals.contains(&s) {
                Check::Ok
            } else {
                Check::Type(format!("`{s}` is not one of {}", vals.join(", ")))
            }
        }
        Text => {
            if s.is_empty() {
                Check::Type("empty text".into())
            } else {
                Check::Ok
            }
        }
        FloatList(lo, hi) => {
            if s.is_empty() {
                return Check::Type("empty list".into());
            }
            for item in s.split([',', ';']) {
                match num(item) {
                    None => return Check::Type(format!("`{}` is not a finite number", item.trim())),
                    Some(v) => {
                        if let Check::Range(m) = ranged(v, lo, hi) {
                            return Check::Range(m);
                        }
                    }
                }
            }
            Check::Ok
        }
    }
}

/// Validate a flat preset. Findings come out in key order (the map is sorted).
pub fn validate_profile(map: &BTreeMap<String, String>) -> Vec<ProfileFinding> {
    let mut out = Vec::new();
    for (key, value) in map {
        let (kind, message) = match lookup(key) {
            None => (FindingKind::UnknownKey, format!("`{key}` is not a documented preset key")),
            Some(spec) => match check(spec.ty, value) {
                Check::Ok => continue,
                Check::Type(m) => (FindingKind::WrongType, m),
                Check::Range(m) => (FindingKind::OutOfRange, m),
            },
        };
        out.push(ProfileFinding { key: key.clone(), kind, message });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn valid() -> Vec<(&'static str, &'static str)> {
        vec![
            ("layer_height", "0.2"),
            ("wall_loops", "3"),
            ("sparse_infill_density", "15%"),
            ("sparse_infill_pattern", "gyroid"),
            ("outer_wall_speed", "60"),
            ("retraction_length", "0.8"),
            ("pressure_advance", "0.04"),
            ("nozzle_temperature", "215"),
            ("fan_min_speed", "40,60"),
            ("gcode_flavor", "klipper"),
            ("use_relative_e_distances", "1"),
            ("small_perimeter_speed", "50%"),
        ]
    }

    fn with(key: &'static str, value: &'static str) -> BTreeMap<String, String> {
        let mut v = valid();
        v.retain(|(k, _)| *k != key);
        v.push((key, value));
        map(&v)
    }

    #[test]
    fn profile_schema_has_enough_sorted_unique_keys() {
        assert!(SCHEMA.len() >= 60, "{}", SCHEMA.len());
        let mut names: Vec<_> = SCHEMA.iter().map(|k| k.name).collect();
        let before = names.clone();
        names.sort_unstable();
        names.dedup();
        assert_eq!(before, names, "schema must be sorted and unique");
        assert!(lookup("layer_height").is_some() && lookup("nope").is_none());
    }

    #[test]
    fn profile_schema_valid_profile_has_no_findings() {
        assert!(validate_profile(&map(&valid())).is_empty());
    }

    #[test]
    fn profile_schema_unknown_key() {
        let mut v = valid();
        v.push(("layer_hight", "0.2"));
        let f = validate_profile(&map(&v));
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].key.as_str(), f[0].kind), ("layer_hight", FindingKind::UnknownKey));
    }

    #[test]
    fn profile_schema_wrong_type_integer() {
        let f = validate_profile(&with("wall_loops", "2.5"));
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].key.as_str(), f[0].kind), ("wall_loops", FindingKind::WrongType));
    }

    #[test]
    fn profile_schema_out_of_range_layer_height() {
        let f = validate_profile(&with("layer_height", "9"));
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].key.as_str(), f[0].kind), ("layer_height", FindingKind::OutOfRange));
    }

    #[test]
    fn profile_schema_bad_enum_and_missing_percent() {
        let f = validate_profile(&map(&[("gcode_flavor", "grbl"), ("sparse_infill_density", "15")]));
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].key.as_str(), f[0].kind), ("gcode_flavor", FindingKind::WrongType));
        assert_eq!((f[1].key.as_str(), f[1].kind), ("sparse_infill_density", FindingKind::WrongType));
    }

    #[test]
    fn profile_schema_list_with_one_bad_element() {
        let f = validate_profile(&map(&[("fan_min_speed", "40,150")]));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, FindingKind::OutOfRange);
        let f = validate_profile(&map(&[("fan_min_speed", "40,abc")]));
        assert_eq!(f[0].kind, FindingKind::WrongType);
    }

    #[test]
    fn profile_schema_nan_rejected() {
        let f = validate_profile(&map(&[("layer_height", "NaN")]));
        assert_eq!(f[0].kind, FindingKind::WrongType);
    }
}

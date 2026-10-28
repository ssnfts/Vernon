//! Small documented preset subset and independent serialization checks.
use crate::slicer::{build_profiles, ProfileSpec};
use serde_json::Value;
use std::collections::BTreeMap;

pub fn documents(spec: &ProfileSpec, ini: bool) -> Result<[(String, String); 3], String> {
    for (name, value, low, high) in [
        ("infill", spec.infill_pct, 0., 100.),
        ("flow ratio", spec.flow_ratio, 0.8, 1.2),
        ("chamber temperature", spec.chamber_c, 0., 90.),
        ("pressure advance", spec.pressure_advance, 0., 2.),
        ("volumetric speed", spec.max_volumetric_speed, 0., 60.),
        ("pellet coefficient", spec.pellet_coef, 0.05, 5.),
    ] {
        if !value.is_finite() || !(low..=high).contains(&value) {
            return Err(format!("{name} outside preset bounds"));
        }
    }
    if !(1..=10).contains(&spec.walls)
        || !crate::gcode::FLAVOURS.contains(&spec.flavour.as_str())
        || !crate::common::MATERIAL_IDS.contains(&spec.material.as_str())
    {
        return Err("invalid preset walls, dialect or material".into());
    }
    if !spec.bed.is_finite()
        || spec.bed.min_element() <= 0.
        || !spec.nozzle_mm.is_finite()
        || spec.nozzle_mm <= 0.
        || !spec.layer_height.is_finite()
        || spec.layer_height <= 0.
        || spec.layer_height > spec.nozzle_mm * 0.8
    {
        return Err("invalid bed, nozzle or layer dimensions".into());
    }
    if spec.z_anti_alias {
        return Err(
            "layer surface modulation has no verified preset key in the checked format".into(),
        );
    }
    if !ini {
        let (p, m, f) = build_profiles(spec);
        let mut out = Vec::new();
        for (name, value) in [("process", p), ("printer", m), ("filament", f)] {
            let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
            let parsed: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            validate_json(&parsed, name)?;
            if parsed != value {
                return Err("JSON round trip changed preset data".into());
            }
            out.push((format!("{name}.json"), text));
        }
        return out.try_into().map_err(|_| "expected three presets".into());
    }
    if spec.pellet || spec.chamber_c > 0. || spec.pressure_advance > 0. {
        return Err(
            "INI subset does not support pellet, chamber or pressure-advance extensions".into(),
        );
    }
    let m = crate::common::material(&spec.material);
    let flavor = match spec.flavour.as_str() {
        "marlin" => "reprap",
        "marlin2" => "marlin2",
        "klipper" => "klipper",
        "reprapfirmware" => "reprapfirmware",
        "repetier" => "repetier",
        _ => return Err("unknown preset dialect".into()),
    };
    let lists = [
        vec![
            ("layer_height", spec.layer_height.to_string()),
            ("first_layer_height", spec.layer_height.to_string()),
            ("perimeters", spec.walls.to_string()),
            ("fill_density", format!("{}%", spec.infill_pct)),
            ("fill_pattern", "grid".into()),
        ],
        vec![
            ("gcode_flavor", flavor.into()),
            (
                "bed_shape",
                format!(
                    "0x0,{}x0,{}x{},0x{}",
                    spec.bed.x, spec.bed.x, spec.bed.y, spec.bed.y
                ),
            ),
            ("max_print_height", spec.bed.z.to_string()),
            ("nozzle_diameter", spec.nozzle_mm.to_string()),
            (
                "use_relative_e_distances",
                u8::from(spec.relative_e).to_string(),
            ),
            (
                "use_firmware_retraction",
                u8::from(spec.firmware_retraction).to_string(),
            ),
        ],
        vec![
            ("temperature", m.nozzle_c.to_string()),
            ("first_layer_temperature", m.nozzle_c.to_string()),
            ("bed_temperature", m.bed_c.to_string()),
            ("first_layer_bed_temperature", m.bed_c.to_string()),
            ("filament_diameter", "1.75".into()),
            ("filament_density", m.density.to_string()),
            ("extrusion_multiplier", spec.flow_ratio.to_string()),
            (
                "filament_max_volumetric_speed",
                spec.max_volumetric_speed.to_string(),
            ),
        ],
    ];
    let mut out = Vec::new();
    for (name, values) in ["process", "printer", "filament"].into_iter().zip(lists) {
        let expected: BTreeMap<String, String> =
            values.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
        let text = expected
            .iter()
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect::<String>();
        let parsed = parse_ini(&text)?;
        validate_ini(&parsed, name)?;
        if parsed != expected {
            return Err("INI round trip changed preset data".into());
        }
        out.push((format!("{name}.ini"), text));
    }
    out.try_into().map_err(|_| "expected three presets".into())
}

fn scalar<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing text preset key {key}"))
}
fn number(s: &str, key: &str) -> Result<f64, String> {
    let n = s
        .parse::<f64>()
        .map_err(|_| format!("invalid preset number {key}"))?;
    if !n.is_finite() {
        return Err(format!("non-finite preset number {key}"));
    }
    Ok(n)
}
fn list_number(v: &Value, key: &str, positive: bool) -> Result<(), String> {
    let a = v[key]
        .as_array()
        .filter(|a| a.len() == 1)
        .ok_or_else(|| format!("{key} must be a single-extruder list"))?;
    let n = number(
        a[0].as_str()
            .ok_or_else(|| format!("{key} must contain strings"))?,
        key,
    )?;
    if n < 0. || (positive && n == 0.) {
        return Err(format!("{key} out of range"));
    }
    Ok(())
}

pub fn validate_json(v: &Value, kind: &str) -> Result<(), String> {
    let ty = if kind == "printer" { "machine" } else { kind };
    if v["type"] != ty {
        return Err("preset type does not match its role".into());
    }
    scalar(v, "name")?;
    match kind {
        "process" => {
            let h = number(scalar(v, "layer_height")?, "layer_height")?;
            if h <= 0. {
                return Err("layer height must be positive".into());
            }
            let fill = number(
                scalar(v, "sparse_infill_density")?.trim_end_matches('%'),
                "infill",
            )?;
            if !(0. ..=100.).contains(&fill) {
                return Err("infill outside 0..100%".into());
            }
        }
        "printer" => {
            if !crate::gcode::FLAVOURS.contains(&scalar(v, "gcode_flavor")?) {
                return Err("unknown preset dialect".into());
            }
            list_number(v, "nozzle_diameter", true)?;
            if number(scalar(v, "printable_height")?, "height")? <= 0. {
                return Err("height must be positive".into());
            }
            if v["printable_area"]
                .as_array()
                .is_none_or(|a| a.len() != 4 || a.iter().any(|x| x.as_str().is_none()))
            {
                return Err("printable area requires four point strings".into());
            }
        }
        "filament" => {
            for k in [
                "nozzle_temperature",
                "hot_plate_temp",
                "filament_diameter",
                "filament_density",
                "chamber_temperature",
            ] {
                list_number(v, k, matches!(k, "filament_diameter" | "filament_density"))?;
            }
            for k in ["activate_chamber_temp_control", "enable_pressure_advance"] {
                list_number(v, k, false)?;
            }
        }
        _ => return Err("unknown preset role".into()),
    }
    Ok(())
}

pub fn parse_ini(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let s = line.trim();
        if s.is_empty() || s.starts_with('#') || s.starts_with(';') {
            continue;
        }
        let (k, v) = s.split_once('=').ok_or("INI line needs key = value")?;
        let k = k.trim();
        let v = v.trim();
        if k.is_empty()
            || v.is_empty()
            || !k
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err("invalid INI key or value".into());
        }
        if out.insert(k.into(), v.into()).is_some() {
            return Err(format!("duplicate INI key {k}"));
        }
    }
    Ok(out)
}
fn validate_ini(v: &BTreeMap<String, String>, kind: &str) -> Result<(), String> {
    let keys: &[&str] = match kind {
        "process" => &["layer_height", "perimeters", "fill_density"],
        "printer" => &[
            "gcode_flavor",
            "bed_shape",
            "max_print_height",
            "nozzle_diameter",
        ],
        "filament" => &[
            "temperature",
            "bed_temperature",
            "filament_diameter",
            "filament_density",
        ],
        _ => return Err("unknown INI role".into()),
    };
    for key in keys {
        if !v.contains_key(*key) {
            return Err(format!("missing INI key {key}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wp5_parser_rejects_invalid_presets() {
        assert!(parse_ini("temperature = 200\ntemperature = 300").is_err());
        assert!(parse_ini("layer_height").is_err());
        assert!(validate_json(&serde_json::json!({"type":"machine","name":"x","gcode_flavor":"klipper","nozzle_diameter":"0.4"}),"printer").is_err());
        assert!(validate_json(&serde_json::json!({"type":"process","name":"x","layer_height":"NaN","sparse_infill_density":"15%"}),"process").is_err());
    }
}

//! Bounded dialect and physical checks for a Cartesian, single-tool job.
use glam::DVec3;
use std::collections::BTreeMap;
use std::io::{BufRead, Read};

#[derive(Debug, Clone)]
pub struct CheckOptions<'a> {
    pub flavour: &'a str,
    pub bed: DVec3,
    pub max_nozzle_c: f64,
    pub max_bed_c: f64,
    pub max_chamber_c: f64,
    pub relative_e: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct Issue {
    pub line: usize,
    pub error: bool,
    pub message: String,
}

fn allowed(command: &str, flavour: &str) -> bool {
    if flavour == "klipper" && ["G17", "G20", "M0", "M1", "M110", "M201", "M203"].contains(&command)
    {
        return false;
    }
    const COMMON: &[&str] = &[
        "G0", "G1", "G2", "G3", "G4", "G10", "G11", "G17", "G20", "G21", "G28", "G90", "G91",
        "G92", "M0", "M1", "M18", "M73", "M82", "M83", "M84", "M104", "M105", "M106", "M107",
        "M109", "M110", "M112", "M114", "M115", "M117", "M118", "M140", "M190", "M201", "M203",
        "M204", "M220", "M221", "M400",
    ];
    const MARLIN: &[&str] = &[
        "G29", "G30", "M17", "M42", "M75", "M76", "M77", "M92", "M108", "M141", "M155", "M191",
        "M205", "M206", "M207", "M208", "M209", "M211", "M300", "M301", "M302", "M303", "M304",
        "M355", "M410", "M420", "M500", "M501", "M502", "M503", "M600", "M900", "M999",
    ];
    const KLIPPER: &[&str] = &[
        "BED_MESH_CALIBRATE",
        "BED_MESH_CLEAR",
        "BED_MESH_PROFILE",
        "SET_PRESSURE_ADVANCE",
        "SET_VELOCITY_LIMIT",
        "SET_GCODE_VARIABLE",
        "EXCLUDE_OBJECT_DEFINE",
        "EXCLUDE_OBJECT_START",
        "EXCLUDE_OBJECT_END",
        "EXCLUDE_OBJECT",
        "SET_PRINT_STATS_INFO",
        "TURN_OFF_HEATERS",
        "TEMPERATURE_WAIT",
        "SET_HEATER_TEMPERATURE",
        "SET_FAN_SPEED",
        "SET_GCODE_OFFSET",
        "SAVE_GCODE_STATE",
        "RESTORE_GCODE_STATE",
        "PAUSE",
        "RESUME",
        "CLEAR_PAUSE",
        "CANCEL_PRINT",
        "RESTART",
        "FIRMWARE_RESTART",
        "STATUS",
        "HELP",
        "GET_POSITION",
    ];
    const RRF: &[&str] = &[
        "G29", "G30", "G31", "G32", "M42", "M98", "M99", "M116", "M141", "M191", "M201.1", "M207",
        "M208", "M350", "M400", "M500", "M501", "M502", "M503", "M563", "M564", "M566", "M568",
        "M572", "M573", "M574", "M577", "M581", "M584", "M591", "M593", "M600", "M669", "M671",
        "M906", "M913", "M950", "M997", "M999",
    ];
    const REPETIER: &[&str] = &[
        "G29", "G30", "G31", "G32", "M42", "M80", "M81", "M92", "M111", "M119", "M205", "M206",
        "M207", "M232", "M300", "M302", "M303", "M320", "M321", "M322", "M500", "M501", "M502",
        "M600", "M908",
    ];
    COMMON.contains(&command)
        || match flavour {
            "marlin" => MARLIN.contains(&command),
            "marlin2" => MARLIN.contains(&command) || ["M486", "M593"].contains(&command),
            "klipper" => KLIPPER.contains(&command),
            "reprapfirmware" => RRF.contains(&command),
            "repetier" => REPETIER.contains(&command),
            _ => false,
        }
        || command
            .strip_prefix('T')
            .is_some_and(|n| n.parse::<u8>().is_ok())
}

/// Parse compact letter/number words, optional line number and checksum.
/// Text-bearing and extended commands retain their names, not their text as words.
fn words(line: &str) -> Result<(String, BTreeMap<char, f64>), String> {
    let mut clean = String::new();
    let mut comment = false;
    for c in line.chars() {
        if c == ';' || c == '*' {
            break;
        }
        if c == '(' {
            comment = true;
            continue;
        }
        if c == ')' {
            comment = false;
            continue;
        }
        if !comment {
            clean.push(c.to_ascii_uppercase());
        }
    }
    let mut s = clean.trim();
    if s.starts_with('N') && s.as_bytes().get(1).is_some_and(u8::is_ascii_digit) {
        let end = s[1..]
            .find(|c: char| !c.is_ascii_digit())
            .map_or(s.len(), |i| i + 1);
        s = s[end..].trim_start();
    }
    if s.is_empty() || s == "%" {
        return Ok((String::new(), BTreeMap::new()));
    }
    let end = if matches!(s.as_bytes()[0], b'G' | b'M' | b'T')
        && s.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
    {
        s[1..]
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .map_or(s.len(), |i| i + 1)
    } else {
        s.find(char::is_whitespace).unwrap_or(s.len())
    };
    let mut cmd = s[..end].to_owned();
    if matches!(cmd.as_bytes()[0], b'G' | b'M' | b'T') {
        if let Ok(n) = cmd[1..].parse::<u32>() {
            cmd = format!("{}{n}", &cmd[..1]);
        }
    }
    let mut params = BTreeMap::new();
    if cmd == "M117" || cmd == "M118" || cmd.contains('_') {
        return Ok((cmd, params));
    }
    let bytes = s.as_bytes();
    let mut i = end;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if !bytes[i].is_ascii_alphabetic() {
            return Err("invalid G-code word".into());
        }
        let key = bytes[i] as char;
        i += 1;
        let start = i;
        if i < bytes.len() && matches!(bytes[i], b'+' | b'-') {
            i += 1;
        }
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        // Empty axis flags are valid for homing; numbers use fixed decimal G-code syntax.
        if i == start {
            if cmd == "G28" && matches!(key, 'X' | 'Y' | 'Z') {
                params.insert(key, 0.0);
                continue;
            }
            return Err(format!("missing numeric {key} word"));
        }
        let value = s[start..i]
            .parse::<f64>()
            .map_err(|_| "invalid numeric G-code word")?;
        if !value.is_finite() {
            return Err("non-finite G-code word".into());
        }
        if params.insert(key, value).is_some() {
            return Err(format!("duplicate {key} word"));
        }
    }
    Ok((cmd, params))
}

pub fn check<R: BufRead>(mut reader: R, options: &CheckOptions<'_>) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut line = 0;
    let mut buffer = Vec::new();
    let mut pos = DVec3::ZERO;
    let mut offset = DVec3::ZERO;
    let mut absolute = true;
    let mut relative_e = false;
    let mut scale = 1.0;
    let mut known = true;
    loop {
        buffer.clear();
        line += 1;
        // take bounds individual input lines without allocating an unbounded String.
        match reader.by_ref().take(65_537).read_until(b'\n', &mut buffer) {
            Ok(0) => break,
            Ok(_) if buffer.len() > 65_536 => {
                out.push(Issue {
                    line,
                    error: true,
                    message: "G-code line exceeds 64 KiB".into(),
                });
                break;
            }
            Err(e) => {
                out.push(Issue {
                    line,
                    error: true,
                    message: format!("cannot read G-code: {e}"),
                });
                break;
            }
            _ => {}
        }
        let text = match std::str::from_utf8(&buffer) {
            Ok(s) => s,
            Err(_) => {
                out.push(Issue {
                    line,
                    error: true,
                    message: "G-code is not UTF-8 text".into(),
                });
                continue;
            }
        };
        let (cmd, p) = match words(text) {
            Ok(v) => v,
            Err(e) => {
                out.push(Issue {
                    line,
                    error: true,
                    message: e,
                });
                known = false;
                continue;
            }
        };
        if cmd.is_empty() {
            continue;
        }
        let mut issue = |error, message: String| {
            out.push(Issue {
                line,
                error,
                message,
            })
        };
        if !allowed(&cmd, options.flavour) {
            issue(
                false,
                format!("unknown or unsupported {} command: {cmd}", options.flavour),
            );
        }
        if options.flavour == "reprapfirmware" && matches!(cmd.as_str(), "G10" | "M568") {
            for key in ['S', 'R'] {
                if p.get(&key)
                    .is_some_and(|t| *t < 0.0 || *t > options.max_nozzle_c)
                {
                    issue(
                        true,
                        "tool temperature target exceeds the nozzle limit".into(),
                    );
                }
            }
        }
        if cmd == "SET_HEATER_TEMPERATURE" {
            let fields: BTreeMap<_, _> = text
                .split(';')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .skip(1)
                .filter_map(|s| s.split_once('='))
                .map(|(k, v)| (k.to_ascii_uppercase(), v.to_ascii_uppercase()))
                .collect();
            let limit = match fields.get("HEATER").map(String::as_str) {
                Some("EXTRUDER") => Some(options.max_nozzle_c),
                Some("HEATER_BED") => Some(options.max_bed_c),
                _ => None,
            };
            match (
                limit,
                fields.get("TARGET").and_then(|s| s.parse::<f64>().ok()),
            ) {
                (Some(limit), Some(t)) if t.is_finite() && t >= 0.0 && t <= limit => {}
                (Some(_), _) => issue(
                    true,
                    "heater target is invalid or exceeds its temperature limit".into(),
                ),
                _ => issue(
                    false,
                    "custom heater target is outside the checked nozzle/bed set".into(),
                ),
            }
        }
        match cmd.as_str() {
            "T0" => {}
            c if c.strip_prefix('T').is_some_and(|n| n.parse::<u8>().is_ok()) => {
                known = false;
                issue(
                    false,
                    "multiple tool coordinate offsets are outside the checked single-tool subset"
                        .into(),
                );
            }
            "G20" => scale = 25.4,
            "G21" => scale = 1.0,
            "G90" | "G91" => {
                absolute = cmd == "G90";
                if matches!(options.flavour, "marlin" | "marlin2") {
                    relative_e = !absolute;
                }
            }
            "M82" => relative_e = false,
            "M83" => relative_e = true,
            "G28" => {
                // Full homing establishes the declared Cartesian origin.
                if p.is_empty() {
                    pos = DVec3::ZERO;
                    offset = DVec3::ZERO;
                } else {
                    known = false;
                    issue(
                        false,
                        "partial homing cannot establish all axis coordinates".into(),
                    );
                }
            }
            "G92" => {
                for (a, c) in ['X', 'Y', 'Z'].iter().enumerate() {
                    if let Some(v) = p.get(c) {
                        let next = pos[a] - v * scale;
                        if next.is_finite() {
                            offset[a] = next;
                        } else {
                            known = false;
                            issue(
                                true,
                                "non-finite coordinate offset after unit conversion".into(),
                            );
                        }
                    }
                }
            }
            "G0" | "G1" | "G2" | "G3" => {
                if !known {
                    issue(true,"move bounds cannot be verified after an unsupported coordinate change in this job".into());
                }
                let previous = pos;
                for (a, c) in ['X', 'Y', 'Z'].iter().enumerate() {
                    if let Some(v) = p.get(c) {
                        let next = if absolute {
                            v * scale + offset[a]
                        } else {
                            pos[a] + v * scale
                        };
                        if next.is_finite() {
                            pos[a] = next;
                        } else {
                            known = false;
                            issue(
                                true,
                                "non-finite move coordinate after unit conversion".into(),
                            );
                        }
                    }
                }
                if pos.cmplt(DVec3::splat(-1e-6)).any()
                    || pos.cmpgt(options.bed + DVec3::splat(1e-6)).any()
                {
                    issue(
                        true,
                        format!(
                            "move outside build volume at ({:.4}, {:.4}, {:.4}) mm",
                            pos.x, pos.y, pos.z
                        ),
                    );
                }
                if matches!(cmd.as_str(), "G2" | "G3") {
                    if let (Some(i), Some(j)) = (p.get(&'I'), p.get(&'J')) {
                        let centre = previous.truncate() + glam::DVec2::new(*i, *j) * scale;
                        let a = previous.truncate() - centre;
                        let b = pos.truncate() - centre;
                        let radius = a.length();
                        if !centre.is_finite()
                            || !a.is_finite()
                            || !b.is_finite()
                            || !radius.is_finite()
                            || !b.length().is_finite()
                        {
                            known = false;
                            issue(true, "non-finite arc geometry after unit conversion".into());
                        } else {
                            let tau = std::f64::consts::TAU;
                            let angle = |v: glam::DVec2| v.y.atan2(v.x).rem_euclid(tau);
                            let start = angle(a);
                            let finish = angle(b);
                            let ccw = cmd == "G3";
                            let distance = |v: f64| {
                                if ccw {
                                    (v - start).rem_euclid(tau)
                                } else {
                                    (start - v).rem_euclid(tau)
                                }
                            };
                            let sweep = if a.distance(b) < 1e-8 {
                                tau
                            } else {
                                distance(finish)
                            };
                            if radius < 1e-9 || (b.length() - radius).abs() > 1e-4 {
                                issue(true, "arc endpoints do not share a valid radius".into());
                            }
                            for k in 0..4 {
                                let t = k as f64 * std::f64::consts::FRAC_PI_2;
                                if distance(t) <= sweep + 1e-12 {
                                    let q = centre + glam::DVec2::new(t.cos(), t.sin()) * radius;
                                    if q.cmplt(glam::DVec2::splat(-1e-6)).any()
                                        || q.cmpgt(
                                            options.bed.truncate() + glam::DVec2::splat(1e-6),
                                        )
                                        .any()
                                    {
                                        issue(true, "arc sweep leaves the build volume".into());
                                        break;
                                    }
                                }
                            }
                        }
                    } else {
                        issue(true,"arc bounds require XY-plane I and J offsets; radius and other-plane arcs are unsupported".into());
                    }
                }
                if p.contains_key(&'E')
                    && options
                        .relative_e
                        .is_some_and(|expected| expected != relative_e)
                {
                    issue(
                        true,
                        "extrusion mode differs from the requested profile mode".into(),
                    );
                }
                if p.get(&'F').is_some_and(|f| *f <= 0.0) {
                    issue(true, "move feed rate must be positive".into());
                }
            }
            "M104" | "M109" | "M140" | "M190" | "M141" | "M191" => {
                let limit = if matches!(cmd.as_str(), "M141" | "M191") {
                    options.max_chamber_c
                } else if matches!(cmd.as_str(), "M140" | "M190") {
                    options.max_bed_c
                } else {
                    options.max_nozzle_c
                };
                for c in ['S', 'R'] {
                    if let Some(t) = p.get(&c) {
                        if *t < 0.0 || *t > limit {
                            issue(
                                true,
                                format!("{cmd} temperature {t} exceeds 0..{limit} deg C"),
                            );
                        }
                    }
                }
            }
            "G17" => {}
            "G18"
            | "G19"
            | "G53"
            | "G54"
            | "G55"
            | "G56"
            | "G57"
            | "G58"
            | "G59"
            | "SET_GCODE_OFFSET"
            | "RESTORE_GCODE_STATE"
            | "M206" => {
                known = false;
                issue(
                    false,
                    "coordinate-changing command is outside the checked Cartesian motion subset"
                        .into(),
                );
            }
            "G10" if p.keys().any(|k| matches!(k, 'X' | 'Y' | 'Z' | 'L')) => {
                known = false;
                issue(
                    false,
                    "G10 coordinate configuration is outside the checked subset".into(),
                );
            }
            "M98" | "M99" | "M501" | "M502" | "G29" | "G30" | "G31" | "G32" | "RESTART"
            | "FIRMWARE_RESTART" | "PAUSE" | "RESUME" => {
                known = false;
                issue(false,"macro, calibration or restored machine state cannot be simulated by the static bounds check".into());
            }
            _ => {
                let numeric_m = cmd
                    .strip_prefix('M')
                    .is_some_and(|n| n.parse::<f64>().is_ok_and(|x| x.is_finite()));
                if !allowed(&cmd, options.flavour) && !numeric_m {
                    known = false;
                }
            }
        }
        if out.len() > 10_000 {
            out.push(Issue {
                line,
                error: true,
                message: "diagnostic limit reached".into(),
            });
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn inspect(text: &str, flavour: &str, relative: Option<bool>) -> Vec<Issue> {
        check(
            text.as_bytes(),
            &CheckOptions {
                flavour,
                bed: DVec3::splat(100.),
                max_nozzle_c: 300.,
                max_bed_c: 110.,
                max_chamber_c: 90.,
                relative_e: relative,
            },
        )
    }
    #[test]
    fn wp5_compact_numbered_modes_offsets_and_limits() {
        assert!(inspect(
            "N1 G90*42\nM83\nG1X90Y2E1\nG92X0\nG1X5\nG91\nG1X6\n",
            "klipper",
            None
        )
        .iter()
        .any(|i| i.error && i.message.contains("outside")));
        assert!(inspect("G1X10Y20\nG91\nG1X5\nG90\nG92X0\nG1X80\n", "klipper", None).is_empty());
        assert!(inspect("M83\nG90\nG1E1\n", "marlin", Some(true))
            .iter()
            .any(|i| i.error));
        assert!(inspect("M83\nG90\nG1E1\n", "reprapfirmware", Some(true)).is_empty());
    }
    #[test]
    fn wp5_swept_arc_is_checked_between_endpoints() {
        let issues = inspect("G1X50Y10\nG2X50Y10I0J50\n", "marlin", None);
        assert!(issues
            .iter()
            .any(|i| i.error && i.message.contains("arc sweep")));
    }
    #[test]
    fn wp5_bad_input_and_unsupported_motion_are_honest() {
        assert!(inspect("G1X1X2\n", "klipper", None).iter().any(|i| i.error));
        assert!(inspect("SET_GCODE_OFFSET X=2\nG1X5\n", "klipper", None)
            .iter()
            .any(|i| i.error));
        for flavour in crate::gcode::FLAVOURS {
            assert!(
                inspect("G90\nM83\nG1X10Y10Z1E1\nM104S200\nM140S60\n", flavour, None).is_empty()
            );
        }
        assert!(inspect("M900K0.1\n", "klipper", None)
            .iter()
            .any(|i| !i.error));
    }

    #[test]
    fn wp5_homing_does_not_reset_unknown_coordinate_frames_or_arc_planes() {
        assert!(inspect("G54\nG28\nG1X5\n", "marlin", None)
            .iter()
            .any(|i| i.error));
        assert!(inspect("G18\nG28\nG2X1Z1I1J0\n", "marlin", None)
            .iter()
            .any(|i| i.error));
    }

    #[test]
    fn wp5_native_heater_commands_obey_temperature_limits() {
        assert!(inspect("M141S999\nM191S999\n", "marlin", None)
            .iter()
            .any(|i| i.error));
        assert!(inspect("G10P0S400\n", "reprapfirmware", None)
            .iter()
            .any(|i| i.error));
        assert!(inspect(
            "SET_HEATER_TEMPERATURE HEATER=heater_bed TARGET=150\n",
            "klipper",
            None
        )
        .iter()
        .any(|i| i.error));
        assert!(inspect(
            "SET_HEATER_TEMPERATURE HEATER=extruder TARGET=NaN\n",
            "klipper",
            None
        )
        .iter()
        .any(|i| i.error));
    }

    #[test]
    fn wp5_numeric_aliases_and_indirect_motion_do_not_evade_checks() {
        assert!(inspect("G01X9999\n", "marlin", None)
            .iter()
            .any(|i| i.error));
        assert!(inspect("T1\nG1X10\n", "klipper", None)
            .iter()
            .any(|i| i.error));
        assert!(inspect("M98P123\nG1X10\n", "reprapfirmware", None)
            .iter()
            .any(|i| i.error));
        assert!(inspect("M201X1000\nM203X1000\n", "klipper", None)
            .iter()
            .any(|i| !i.error));
    }

    #[test]
    fn wp5_unknown_m_prefixed_macros_invalidate_position() {
        let issues = inspect("MOVE_HEAD\nG1X10\n", "klipper", None);
        assert!(issues
            .iter()
            .any(|i| !i.error && i.message.contains("MOVE_HEAD")));
        assert!(issues
            .iter()
            .any(|i| i.error && i.message.contains("cannot be verified")));
        assert!(!inspect("M900K0.1\nG1X10\n", "klipper", None)
            .iter()
            .any(|i| i.error));
    }

    #[test]
    fn wp5_transformed_coordinate_overflow_is_an_error() {
        let giant = format!("1{}", "0".repeat(308));
        for text in [
            format!("G20\nG92X{giant}\nG1X{giant}\n"),
            format!("G20\nG1X{giant}\n"),
            format!("G20\nG2X0Y0I{giant}J{giant}\n"),
        ] {
            assert!(
                inspect(&text, "marlin", None)
                    .iter()
                    .any(|i| i.error && i.message.contains("non-finite")),
                "{text}"
            );
        }
    }
}

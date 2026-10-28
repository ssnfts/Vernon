//! G-code reading: summary numbers (layers, time, filament), flavour guess and
//! flavour conformance checks. Streaming, one pass, no allocation per line
//! beyond the line buffer.

use std::collections::{BTreeSet, HashSet};
use std::io::BufRead;

pub const FLAVOURS: [&str; 5] = ["marlin", "marlin2", "klipper", "reprapfirmware", "repetier"];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SummaryOptions {
    /// None detects M82 / M83 in the file.
    pub relative_e: Option<bool>,
    pub filament_diameter: f64,
    /// g/cm3.
    pub density: f64,
}

impl Default for SummaryOptions {
    fn default() -> Self {
        SummaryOptions { relative_e: None, filament_diameter: 1.75, density: 1.24 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GcodeSummary {
    pub layers: usize,
    pub time_s: f64,
    /// "comment", "m73" or "estimate".
    pub time_source: &'static str,
    pub filament_mm: f64,
    pub filament_g: f64,
    pub flavour: &'static str,
    pub relative_e: bool,
    pub lines: usize,
    pub moves: usize,
    pub max_z: f64,
}

/// Seconds from text such as `1d 2h 3m 4s`.
pub fn parse_duration(s: &str) -> Option<f64> {
    let mut total = 0.0;
    let mut found = false;
    let mut num = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            num.push(ch);
        } else if let Some(mult) = match ch {
            'd' => Some(86400.0),
            'h' => Some(3600.0),
            'm' => Some(60.0),
            's' => Some(1.0),
            _ => None,
        } {
            if let Ok(v) = num.parse::<f64>() {
                total += v * mult;
                found = true;
            }
            num.clear();
        } else if ch.is_whitespace() {
            continue;
        } else {
            num.clear();
        }
    }
    found.then_some(total)
}

/// The number after the first `=` or `:` of a comment.
fn number_after_sep(s: &str) -> Option<f64> {
    let i = s.find(['=', ':'])?;
    s[i + 1..].split_whitespace().next()?.parse().ok()
}

fn word_value(tok: &str, letter: char) -> Option<f64> {
    let mut c = tok.chars();
    if c.next()?.to_ascii_uppercase() != letter {
        return None;
    }
    tok[1..].parse().ok()
}

#[derive(Default)]
struct Signatures {
    klipper: bool,
    rrf: bool,
    marlin2: bool,
    any_command: bool,
}

impl Signatures {
    fn flavour(&self) -> &'static str {
        if self.klipper {
            "klipper"
        } else if self.rrf {
            "reprapfirmware"
        } else if self.marlin2 {
            "marlin2"
        } else if self.any_command {
            "marlin"
        } else {
            "unknown"
        }
    }
}

const KLIPPER_ONLY: [&str; 6] = [
    "EXCLUDE_OBJECT_DEFINE",
    "EXCLUDE_OBJECT_START",
    "EXCLUDE_OBJECT_END",
    "SET_PRESSURE_ADVANCE",
    "SET_VELOCITY_LIMIT",
    "SET_PRINT_STATS_INFO",
];

fn scan_signature(sig: &mut Signatures, cmd: &str, toks: &[&str]) {
    sig.any_command = true;
    if KLIPPER_ONLY.contains(&cmd) || cmd == "SET_GCODE_VARIABLE" {
        sig.klipper = true;
    }
    match cmd {
        "M572" | "M98" => sig.rrf = true,
        "M204" if toks.iter().skip(1).any(|t| t.starts_with('P') || t.starts_with('T')) => sig.marlin2 = true,
        _ => {}
    }
}

/// Summarise a G-code stream.
pub fn summarize<R: BufRead>(reader: R, opts: &SummaryOptions) -> GcodeSummary {
    let mut sig = Signatures::default();
    let (mut lines, mut moves) = (0usize, 0usize);
    let (mut layer_change, mut layer_cura) = (0usize, 0usize);
    let mut z_layers: BTreeSet<i64> = BTreeSet::new();
    let mut relative = opts.relative_e.unwrap_or(false);
    let auto = opts.relative_e.is_none();
    let (mut pos, mut e_abs, mut e_net, mut feed) = ([0.0f64; 3], 0.0f64, 0.0f64, 1500.0f64);
    let mut est_time = 0.0;
    let (mut comment_time, mut m73_time): (Option<f64>, Option<f64>) = (None, None);
    let (mut comment_g, mut comment_mm): (Option<f64>, Option<f64>) = (None, None);
    let mut max_z = 0.0f64;
    for line in reader.lines().map_while(Result::ok) {
        lines += 1;
        let (code, comment) = match line.find(';') {
            Some(i) => (&line[..i], Some(&line[i + 1..])),
            None => (line.as_str(), None),
        };
        if let Some(c) = comment {
            let lc = c.trim();
            if lc.starts_with("LAYER_CHANGE") {
                layer_change += 1;
            } else if lc.strip_prefix("LAYER:").is_some_and(|v| v.trim().parse::<i64>().is_ok_and(|n| n >= 0)) {
                layer_cura += 1;
            } else if let Some(t) = lc.strip_prefix("TIME:") {
                comment_time = comment_time.or(t.trim().parse().ok());
            } else {
                let low = lc.to_ascii_lowercase();
                if low.contains("estimated printing time") && comment_time.is_none() {
                    if let Some(i) = lc.find('=') {
                        comment_time = parse_duration(&lc[i + 1..]);
                    }
                } else if low.contains("filament") && low.contains("[g]") && (low.contains("weight") || low.contains("used")) {
                    comment_g = comment_g.or(number_after_sep(lc));
                } else if low.contains("filament used") && low.contains("[mm]") {
                    comment_mm = comment_mm.or(number_after_sep(lc));
                }
            }
        }
        let toks: Vec<&str> = code.split_whitespace().collect();
        let Some(first) = toks.first() else { continue };
        let cmd = first.to_ascii_uppercase();
        scan_signature(&mut sig, &cmd, &toks);
        match cmd.as_str() {
            "M82" if auto => relative = false,
            "M83" if auto => relative = true,
            "G92" => {
                if let Some(v) = toks.iter().skip(1).find_map(|t| word_value(t, 'E')) {
                    e_abs = v;
                }
            }
            "M73" => {
                if m73_time.is_none() {
                    if let Some(r) = toks.iter().skip(1).find_map(|t| word_value(t, 'R')) {
                        m73_time = Some(r * 60.0);
                    }
                }
            }
            "G0" | "G1" | "G2" | "G3" => {
                moves += 1;
                let (mut x, mut y, mut z, mut e) = (None, None, None, None);
                for t in toks.iter().skip(1) {
                    if let Some(v) = word_value(t, 'X') {
                        x = Some(v);
                    } else if let Some(v) = word_value(t, 'Y') {
                        y = Some(v);
                    } else if let Some(v) = word_value(t, 'Z') {
                        z = Some(v);
                    } else if let Some(v) = word_value(t, 'E') {
                        e = Some(v);
                    } else if let Some(v) = word_value(t, 'F') {
                        if v > 0.0 {
                            feed = v;
                        }
                    }
                }
                let np = [x.unwrap_or(pos[0]), y.unwrap_or(pos[1]), z.unwrap_or(pos[2])];
                let dist = ((np[0] - pos[0]).powi(2) + (np[1] - pos[1]).powi(2) + (np[2] - pos[2]).powi(2)).sqrt();
                let mut de = 0.0;
                if let Some(ev) = e {
                    if relative {
                        de = ev;
                    } else {
                        de = ev - e_abs;
                        e_abs = ev;
                    }
                    e_net += de;
                }
                let travel = if dist > 0.0 { dist } else { de.abs() };
                est_time += travel / (feed / 60.0);
                if de > 0.0 && np[2] > 0.0 {
                    z_layers.insert((np[2] * 1000.0).round() as i64);
                }
                pos = np;
                max_z = max_z.max(pos[2]);
            }
            _ => {}
        }
    }
    let layers = if layer_change > 0 {
        layer_change
    } else if layer_cura > 0 {
        layer_cura
    } else {
        z_layers.len()
    };
    let (time_s, time_source) = match (comment_time, m73_time) {
        (Some(t), _) => (t, "comment"),
        (None, Some(t)) => (t, "m73"),
        _ => (est_time, "estimate"),
    };
    let filament_mm = if e_net > 0.0 { e_net } else { comment_mm.unwrap_or(0.0) };
    let area = std::f64::consts::FRAC_PI_4 * opts.filament_diameter.powi(2);
    let filament_g = comment_g.unwrap_or(filament_mm * area / 1000.0 * opts.density);
    GcodeSummary {
        layers,
        time_s,
        time_source,
        filament_mm,
        filament_g,
        flavour: sig.flavour(),
        relative_e: relative,
        lines,
        moves,
        max_z,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub line: usize,
    pub message: String,
}

/// Remove the transport line number and trailing comment/checksum for the
/// conformance passes. Checksum integrity belongs to the printer transport.
fn conformance_code(line: &str) -> &str {
    let mut code = line.split([';', '*']).next().unwrap_or("").trim_start();
    if matches!(code.as_bytes().first(), Some(b'N' | b'n'))
        && code.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
    {
        let end = code[1..].find(|c: char| !c.is_ascii_digit()).map_or(code.len(), |i| i + 1);
        code = code[end..].trim_start();
    }
    code
}

/// Check a G-code stream against the dialect of `flavour`.
pub fn check_flavour<R: BufRead>(reader: R, flavour: &str) -> Vec<Violation> {
    let mut out = Vec::new();
    let klipper = flavour == "klipper";
    let mut defined: HashSet<String> = HashSet::new();
    let mut open: Option<String> = None;
    let mut started_any = false;
    let name_of = |toks: &[&str]| {
        toks.iter().skip(1).find_map(|t| (t.len() > 5 && t.is_char_boundary(5) && t[..5].eq_ignore_ascii_case("NAME=")).then(|| t[5..].to_string()))
    };
    for (i, line) in reader.lines().map_while(Result::ok).enumerate() {
        let n = i + 1;
        let code = conformance_code(&line);
        let toks: Vec<&str> = code.split_whitespace().collect();
        let Some(first) = toks.first() else { continue };
        let cmd = first.to_ascii_uppercase();
        let is_klipper_cmd = KLIPPER_ONLY.contains(&cmd.as_str()) || cmd == "SET_GCODE_VARIABLE";
        if is_klipper_cmd && !klipper {
            out.push(Violation { line: n, message: format!("{cmd} is a Klipper command and is not part of the {flavour} dialect") });
            continue;
        }
        if klipper && cmd == "M900" {
            out.push(Violation { line: n, message: "pressure advance uses SET_PRESSURE_ADVANCE, not M900".into() });
        }
        if flavour == "reprapfirmware" && cmd == "M900" {
            out.push(Violation { line: n, message: "pressure advance uses M572, not M900".into() });
        }
        if !klipper {
            continue;
        }
        match cmd.as_str() {
            "EXCLUDE_OBJECT_DEFINE" => match name_of(&toks) {
                None => out.push(Violation { line: n, message: "EXCLUDE_OBJECT_DEFINE without NAME".into() }),
                Some(nm) => {
                    if started_any {
                        out.push(Violation { line: n, message: format!("object {nm} defined after the first object started") });
                    }
                    if !defined.insert(nm.clone()) {
                        out.push(Violation { line: n, message: format!("object {nm} defined twice") });
                    }
                }
            },
            "EXCLUDE_OBJECT_START" => {
                started_any = true;
                match (name_of(&toks), &open) {
                    (None, _) => out.push(Violation { line: n, message: "EXCLUDE_OBJECT_START without NAME".into() }),
                    (Some(nm), Some(cur)) => out.push(Violation { line: n, message: format!("object {nm} started inside object {cur}") }),
                    (Some(nm), None) => {
                        if !defined.contains(&nm) {
                            out.push(Violation { line: n, message: format!("object {nm} started but never defined") });
                        }
                        open = Some(nm);
                    }
                }
            }
            "EXCLUDE_OBJECT_END" => {
                match (&open, name_of(&toks)) {
                    (None, _) => out.push(Violation { line: n, message: "EXCLUDE_OBJECT_END without a matching START".into() }),
                    (Some(cur), Some(nm)) if *cur != nm => {
                        out.push(Violation { line: n, message: format!("END for {nm} closes object {cur}") })
                    }
                    _ => {}
                }
                open = None;
            }
            _ => {}
        }
    }
    if let Some(cur) = open {
        out.push(Violation { line: 0, message: format!("object {cur} is never closed") });
    }
    out
}

/// Number of EXCLUDE_OBJECT_DEFINE lines.
pub fn count_object_defines<R: BufRead>(reader: R) -> usize {
    reader.lines().map_while(Result::ok).filter(|l| {
        conformance_code(l).split_whitespace().next().is_some_and(|cmd| cmd.eq_ignore_ascii_case("EXCLUDE_OBJECT_DEFINE"))
    }).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum(text: &str) -> GcodeSummary {
        summarize(text.as_bytes(), &SummaryOptions::default())
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("1h 2m 3s"), Some(3723.0));
        assert_eq!(parse_duration(" 1d 0h 0m 5s"), Some(86405.0));
        assert_eq!(parse_duration("nothing"), None);
    }

    #[test]
    fn relative_e_summary_with_retraction() {
        let g = "; estimated printing time (normal mode) = 1h 2m 3s\nM83\nG1 Z0.2 F600\n;LAYER_CHANGE\nG1 X10 Y0 E1.0 F1200\nG1 X10 Y10 E1.0\nG1 E-0.8\nG1 E0.8\n;LAYER_CHANGE\nG1 Z0.4\nG1 X0 Y10 E1.0\n";
        let s = sum(g);
        assert_eq!(s.layers, 2);
        assert!((s.filament_mm - 3.0).abs() < 1e-12);
        assert_eq!(s.time_s, 3723.0);
        assert_eq!(s.time_source, "comment");
        assert!(s.relative_e);
        assert!((s.max_z - 0.4).abs() < 1e-12);
        // mass from length: pi/4 * 1.75^2 * 3 mm * 1.24 g/cm3
        assert!((s.filament_g - std::f64::consts::FRAC_PI_4 * 1.75f64.powi(2) * 3.0 / 1000.0 * 1.24).abs() < 1e-12);
    }

    #[test]
    fn absolute_e_with_reset_and_z_layers() {
        let g = "M82\nG92 E0\nG1 Z0.2\nG1 X10 E1\nG1 X20 E2\nG92 E0\nG1 Z0.4\nG1 X30 E1\n";
        let s = sum(g);
        assert!((s.filament_mm - 3.0).abs() < 1e-12);
        assert_eq!(s.layers, 2); // counted by extruding moves at new Z
        assert!(!s.relative_e);
        assert_eq!(s.time_source, "estimate");
        assert!(s.time_s > 0.0);
    }

    #[test]
    fn forced_mode_overrides_detection() {
        let g = "M82\nG1 X1 E1\nG1 X2 E1\n";
        let rel = summarize(g.as_bytes(), &SummaryOptions { relative_e: Some(true), ..Default::default() });
        assert!((rel.filament_mm - 2.0).abs() < 1e-12);
        let auto = sum(g);
        assert!((auto.filament_mm - 1.0).abs() < 1e-12);
    }

    #[test]
    fn m73_time_and_weight_comment() {
        let g = "M73 P0 R12\n; total filament weight [g] : 4.5\nG1 X1 E1\n";
        let s = sum(g);
        assert_eq!((s.time_s, s.time_source), (720.0, "m73"));
        assert_eq!(s.filament_g, 4.5);
    }

    #[test]
    fn flavour_guess() {
        assert_eq!(sum("EXCLUDE_OBJECT_DEFINE NAME=a\nG1 X1\n").flavour, "klipper");
        assert_eq!(sum("M572 D0 S0.05\nG1 X1\n").flavour, "reprapfirmware");
        assert_eq!(sum("M204 P500 R500 T800\n").flavour, "marlin2");
        assert_eq!(sum("M900 K0.05\n").flavour, "marlin");
        assert_eq!(sum("; nothing\n").flavour, "unknown");
    }

    const KLIPPER_OK: &str = "EXCLUDE_OBJECT_DEFINE NAME=a CENTER=10,10\nEXCLUDE_OBJECT_DEFINE NAME=b CENTER=50,10\n;LAYER_CHANGE\nEXCLUDE_OBJECT_START NAME=a\nG1 X1 E1\nEXCLUDE_OBJECT_END NAME=a\nEXCLUDE_OBJECT_START NAME=b\nG1 X2 E1\nEXCLUDE_OBJECT_END NAME=b\n";

    #[test]
    fn klipper_object_conformance() {
        assert!(check_flavour(KLIPPER_OK.as_bytes(), "klipper").is_empty());
        assert_eq!(count_object_defines(KLIPPER_OK.as_bytes()), 2);
        // marlin output must not contain them
        let v = check_flavour(KLIPPER_OK.as_bytes(), "marlin");
        assert_eq!(v.len(), 6);
        assert!(check_flavour("G1 X1 E1\nM900 K0.1\n".as_bytes(), "marlin").is_empty());
    }

    #[test]
    fn klipper_violations_are_found() {
        let bad = "EXCLUDE_OBJECT_START NAME=a\nEXCLUDE_OBJECT_END NAME=b\nEXCLUDE_OBJECT_DEFINE NAME=c\nM900 K0.1\nEXCLUDE_OBJECT_START NAME=c\n";
        let v = check_flavour(bad.as_bytes(), "klipper");
        let msgs: Vec<&str> = v.iter().map(|x| x.message.as_str()).collect();
        assert!(msgs.iter().any(|m| m.contains("never defined")));
        assert!(msgs.iter().any(|m| m.contains("closes object a")));
        assert!(msgs.iter().any(|m| m.contains("defined after")));
        assert!(msgs.iter().any(|m| m.contains("SET_PRESSURE_ADVANCE")));
        assert!(msgs.iter().any(|m| m.contains("never closed")));
    }

    #[test]
    fn wp5_numbered_object_markers_preserve_validation_and_counts() {
        let numbered = "N1 EXCLUDE_OBJECT_DEFINE NAME=a*0\nn2EXCLUDE_OBJECT_DEFINE NAME=b*0\nN3 EXCLUDE_OBJECT_START NAME=a*0\nN4 EXCLUDE_OBJECT_END NAME=a*0\nN5 EXCLUDE_OBJECT_START NAME=b*0\nN6 EXCLUDE_OBJECT_END NAME=b*0\n";
        assert!(check_flavour(numbered.as_bytes(), "klipper").is_empty());
        assert_eq!(count_object_defines(numbered.as_bytes()), 2);
        assert_eq!(check_flavour(numbered.as_bytes(), "marlin").len(), 6);
    }

    #[test]
    fn wp5_numbered_object_markers_expose_undefined_mismatched_and_unclosed() {
        let bad = "N1EXCLUDE_OBJECT_START NAME=a*0\nN2 EXCLUDE_OBJECT_END NAME=b*0\nN3 EXCLUDE_OBJECT_DEFINE NAME=c*0\nN4 EXCLUDE_OBJECT_START NAME=c*0\n";
        let violations = check_flavour(bad.as_bytes(), "klipper");
        assert!(violations.iter().any(|v| v.line == 1 && v.message.contains("never defined")));
        assert!(violations.iter().any(|v| v.line == 2 && v.message.contains("END for b closes object a")));
        assert!(violations.iter().any(|v| v.line == 3 && v.message.contains("defined after")));
        assert!(violations.iter().any(|v| v.line == 0 && v.message.contains("object c is never closed")));
    }
}

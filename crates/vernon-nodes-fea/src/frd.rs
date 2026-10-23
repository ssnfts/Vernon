//! Readers for the solver's result files.
//!
//! * `.frd` (fixed-width ASCII blocks): fields are sliced by column width,
//!   never split on whitespace, because a negative `E12.5` value can touch
//!   the one before it.
//! * `.dat` (text tables for the `*NODE PRINT` / `*EL PRINT` requests): rows
//!   are split into numbers with a tokenizer that also separates fused values.
//!
//! Component order of tensors is `[xx, yy, zz, xy, yz, zx]` (the order of the
//! `.frd` stress block); the `.dat` stress rows list `sxx syy szz sxy sxz syz`
//! and are reordered on read.

use std::collections::BTreeMap;

use glam::DVec3;

/// One result block of a `.frd` file.
#[derive(Debug, Clone, PartialEq)]
pub struct FrdDataset {
    /// Dataset name as written by the solver (`DISP`, `STRESS`, `FORC`, ...).
    pub name: String,
    pub time: f64,
    pub components: Vec<String>,
    /// `(node id, values)` in file order.
    pub rows: Vec<(u32, Vec<f64>)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrdFile {
    pub node_count: usize,
    pub datasets: Vec<FrdDataset>,
}

impl FrdFile {
    /// The last block with this name (the final increment).
    pub fn dataset(&self, name: &str) -> Option<&FrdDataset> {
        self.datasets.iter().rev().find(|d| d.name.eq_ignore_ascii_case(name))
    }
}

/// Parse the fixed-width fields of one ` -1` / ` -2` row.
fn parse_row(line: &str) -> Result<(Option<u32>, Vec<f64>), String> {
    let l = line.trim_end();
    let body = l.get(3..).ok_or("short result row")?;
    let n = body.len();
    // node id width 10 (long) or 5 (short); the remainder is a whole number of E12.5 fields
    let w = if n >= 10 && (n - 10) % 12 == 0 {
        10
    } else if n >= 5 && (n - 5) % 12 == 0 {
        5
    } else if l.starts_with(" -2") && n % 12 == 0 {
        0
    } else {
        return Err(format!("cannot read result row (width {}): {l:?}", l.len()));
    };
    let id = if w > 0 {
        Some(body[..w].trim().parse::<u32>().map_err(|e| format!("bad node id in {l:?}: {e}"))?)
    } else {
        None
    };
    let vals = body[w..]
        .as_bytes()
        .chunks(12)
        .map(|c| {
            let t = std::str::from_utf8(c).unwrap_or("").trim();
            t.parse::<f64>().map_err(|e| format!("bad value {t:?}: {e}"))
        })
        .collect::<Result<Vec<f64>, String>>()?;
    Ok((id, vals))
}

/// Parse a `.frd` text. Fails with "incomplete result" when the `9999` end
/// marker is missing (truncated file).
pub fn parse_frd(text: &str) -> Result<FrdFile, String> {
    let mut out = FrdFile::default();
    let mut ended = false;
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim_start();
        if t.starts_with("9999") {
            ended = true;
            break;
        }
        if t.starts_with("2C") {
            // node block: count rows
            while let Some(l) = lines.peek() {
                if l.starts_with(" -3") {
                    lines.next();
                    break;
                }
                if l.starts_with(" -1") {
                    out.node_count += 1;
                }
                lines.next();
            }
        } else if t.starts_with("3C") {
            // element block: connectivity comes from the model, skip it
            for l in lines.by_ref() {
                if l.starts_with(" -3") {
                    break;
                }
            }
        } else if t.starts_with("100C") {
            let time = t
                .split_whitespace()
                .skip(1)
                .find(|tok| tok.contains('.') && tok.to_ascii_uppercase().contains('E'))
                .and_then(|tok| tok.parse::<f64>().ok())
                .unwrap_or(0.0);
            let mut ds = FrdDataset { name: String::new(), time, components: vec![], rows: vec![] };
            for l in lines.by_ref() {
                if l.starts_with(" -3") {
                    break;
                }
                if l.starts_with(" -4") {
                    ds.name = l.split_whitespace().nth(1).unwrap_or("").to_string();
                } else if l.starts_with(" -5") {
                    if let Some(c) = l.split_whitespace().nth(1) {
                        ds.components.push(c.to_string());
                    }
                } else if l.starts_with(" -1") {
                    let (id, vals) = parse_row(l)?;
                    ds.rows.push((id.ok_or("result row without node id")?, vals));
                } else if l.starts_with(" -2") {
                    let (_, vals) = parse_row(l)?;
                    ds.rows.last_mut().ok_or("continuation row without a start row")?.1.extend(vals);
                }
            }
            if ds.name.is_empty() {
                return Err("result block without a dataset name".into());
            }
            out.datasets.push(ds);
        }
    }
    if !ended {
        return Err("incomplete result: the file has no 9999 end marker (truncated or solver still running)".into());
    }
    Ok(out)
}

/// Dense per-node 3-vectors for point indices `0..n` (node id = index + 1).
/// `needed[i]` marks nodes the model really uses; a missing needed node is an error.
pub fn dense_vec3(ds: &FrdDataset, n: usize, needed: &[bool]) -> Result<Vec<DVec3>, String> {
    let mut out = vec![DVec3::ZERO; n];
    let mut seen = vec![false; n];
    for (id, v) in &ds.rows {
        let i = *id as usize;
        if i == 0 || i > n || v.len() < 3 {
            continue;
        }
        out[i - 1] = DVec3::new(v[0], v[1], v[2]);
        seen[i - 1] = true;
    }
    if let Some(i) = (0..n).find(|&i| needed.get(i).copied().unwrap_or(true) && !seen[i]) {
        return Err(format!("result block {} has no value for node {}", ds.name, i + 1));
    }
    Ok(out)
}

/// Dense per-node 6-component tensors (order xx yy zz xy yz zx).
pub fn dense_tensor6(ds: &FrdDataset, n: usize, needed: &[bool]) -> Result<Vec<[f64; 6]>, String> {
    let mut out = vec![[0.0; 6]; n];
    let mut seen = vec![false; n];
    for (id, v) in &ds.rows {
        let i = *id as usize;
        if i == 0 || i > n || v.len() < 6 {
            continue;
        }
        out[i - 1] = [v[0], v[1], v[2], v[3], v[4], v[5]];
        seen[i - 1] = true;
    }
    if let Some(i) = (0..n).find(|&i| needed.get(i).copied().unwrap_or(true) && !seen[i]) {
        return Err(format!("result block {} has no value for node {}", ds.name, i + 1));
    }
    Ok(out)
}

/// One `E12.5` field (`-1.23456E+00`).
pub fn format_frd_value(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return format!("{:>12}", "0.00000E+00");
    }
    let mut e = v.abs().log10().floor() as i32;
    let mut m = v / 10f64.powi(e);
    if (m.abs() * 1e5).round() >= 1e6 {
        e += 1;
        m = v / 10f64.powi(e);
    }
    format!("{:>12}", format!("{m:.5}E{}{:02}", if e < 0 { '-' } else { '+' }, e.abs()))
}

fn frd_row(prefix: &str, id: usize, vals: &[f64]) -> String {
    let mut s = format!("{prefix}{id:>10}");
    for v in vals {
        s.push_str(&format_frd_value(*v));
    }
    s
}

/// Minimal `.frd` writer: nodes, an optional `DISP` block and an optional
/// `STRESS` block (long node-id format), ended by `9999`.
pub fn write_frd(nodes: &[DVec3], disp: Option<&[DVec3]>, stress: Option<&[[f64; 6]]>) -> String {
    let mut s = String::new();
    s.push_str("    1C\n    1UVernon white-box writer\n");
    s.push_str(&format!("    2C{:>12}{:>37}{}\n", nodes.len(), "", 1));
    for (i, p) in nodes.iter().enumerate() {
        s.push_str(&frd_row(" -1", i + 1, &[p.x, p.y, p.z]));
        s.push('\n');
    }
    s.push_str(" -3\n");
    if let Some(d) = disp {
        s.push_str(&format!("  100CL  101{}{:>12}{:>20}{:>2}{:>5}{:>5}\n", format_frd_value(1.0), d.len(), "", 0, 1, 1));
        s.push_str(" -4  DISP        4    1\n");
        for c in ["D1", "D2", "D3"] {
            s.push_str(&format!(" -5  {c:<8}   1    2    {}    0\n", &c[1..]));
        }
        s.push_str(" -5  ALL         1    2    0    0    1ALL\n");
        for (i, v) in d.iter().enumerate() {
            s.push_str(&frd_row(" -1", i + 1, &[v.x, v.y, v.z]));
            s.push('\n');
        }
        s.push_str(" -3\n");
    }
    if let Some(t) = stress {
        s.push_str(&format!("  100CL  101{}{:>12}{:>20}{:>2}{:>5}{:>5}\n", format_frd_value(1.0), t.len(), "", 0, 1, 1));
        s.push_str(" -4  STRESS      6    1\n");
        for (k, c) in ["SXX", "SYY", "SZZ", "SXY", "SYZ", "SZX"].iter().enumerate() {
            s.push_str(&format!(" -5  {c:<8}   1    4    {}    0\n", k + 1));
        }
        for (i, v) in t.iter().enumerate() {
            s.push_str(&frd_row(" -1", i + 1, v));
            s.push('\n');
        }
        s.push_str(" -3\n");
    }
    s.push_str(" 9999\n");
    s
}

/// Split a text line into numbers, separating values that touch
/// (`-1.2E+00-3.4E+00`).
pub fn numbers(line: &str) -> Vec<f64> {
    let mut out = Vec::new();
    for tok in line.split_whitespace() {
        let b = tok.as_bytes();
        let mut start = 0;
        for i in 1..=b.len() {
            let split = i == b.len() || ((b[i] == b'-' || b[i] == b'+') && !matches!(b[i - 1], b'E' | b'e'));
            if split {
                if let Ok(v) = tok[start..i].parse::<f64>() {
                    out.push(v);
                }
                start = i;
            }
        }
    }
    out
}

/// Tables read from the `.dat` text output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DatFile {
    /// Total force per node set (`*NODE PRINT ... TOTALS=ONLY`, key `RF`) in N.
    pub totals: BTreeMap<String, [f64; 3]>,
    /// Integration-point stresses `(element id, point, [xx,yy,zz,xy,yz,zx])`.
    pub ip_stress: Vec<(u32, u32, [f64; 6])>,
    /// Displacement rows `(node id, [ux,uy,uz])` when printed.
    pub disp: Vec<(u32, [f64; 3])>,
    /// Total volume of the printed element set (mm3).
    pub volume: Option<f64>,
    /// Time of the last table.
    pub time: f64,
}

#[derive(Clone, Copy, PartialEq)]
enum Table {
    None,
    TotalForce,
    Stress,
    Disp,
    Volume,
    Skip,
}

fn header_set_and_time(line: &str) -> (Option<String>, f64) {
    let lower = line.to_ascii_lowercase();
    let set = lower.find("for set ").map(|i| {
        let rest = &line[i + 8..];
        rest.split(" and time").next().unwrap_or(rest).trim().to_ascii_uppercase()
    });
    let time = lower.find("time").map(|i| numbers(&line[i + 4..])).and_then(|v| v.first().copied()).unwrap_or(0.0);
    (set, time)
}

/// Parse a `.dat` text. Unknown tables are skipped.
pub fn parse_dat(text: &str) -> DatFile {
    let mut out = DatFile::default();
    let mut table = Table::None;
    let mut set = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let lower = t.to_ascii_lowercase();
        let starts_alpha = t.chars().next().is_some_and(|c| c.is_alphabetic());
        if starts_alpha {
            let (s, time) = header_set_and_time(t);
            set = s.unwrap_or_default();
            out.time = time;
            table = if lower.starts_with("total force") {
                Table::TotalForce
            } else if lower.starts_with("stresses") {
                Table::Stress
            } else if lower.starts_with("displacements") {
                Table::Disp
            } else if lower.contains("volume") && lower.contains("total") {
                Table::Volume
            } else {
                Table::Skip
            };
            continue;
        }
        let v = numbers(t);
        match table {
            Table::TotalForce if v.len() >= 3 => {
                out.totals.insert(set.clone(), [v[0], v[1], v[2]]);
            }
            Table::Stress if v.len() >= 8 => {
                // file order: sxx syy szz sxy sxz syz
                out.ip_stress.push((v[0] as u32, v[1] as u32, [v[2], v[3], v[4], v[5], v[7], v[6]]));
            }
            Table::Disp if v.len() >= 4 => out.disp.push((v[0] as u32, [v[1], v[2], v[3]])),
            Table::Volume if !v.is_empty() => out.volume = v.last().copied(),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_format_is_twelve_wide() {
        assert_eq!(format_frd_value(-1.23456), "-1.23456E+00");
        assert_eq!(format_frd_value(0.000123456), " 1.23456E-04");
        assert_eq!(format_frd_value(12345.6), " 1.23456E+04");
        assert_eq!(format_frd_value(0.0), " 0.00000E+00");
        assert_eq!(format_frd_value(9.999999), " 1.00000E+01");
    }

    #[test]
    fn g11_fused_values_and_both_id_formats() {
        let long = " -1         7-1.23456E+00-2.34567E+00 3.00000E+00";
        let (id, v) = parse_row(long).unwrap();
        assert_eq!(id, Some(7));
        assert_eq!(v, vec![-1.23456, -2.34567, 3.0]);
        let short = " -1    7-1.23456E+00-2.34567E+00 3.00000E+00";
        let (id, v) = parse_row(short).unwrap();
        assert_eq!(id, Some(7));
        assert_eq!(v.len(), 3);
        assert!(parse_row(" -1  garbage").is_err());
    }

    #[test]
    fn frd_round_trip_and_truncation() {
        let nodes = vec![DVec3::ZERO, DVec3::new(10.0, 0.0, 0.0), DVec3::new(0.0, 10.0, 0.0)];
        let disp = vec![DVec3::ZERO, DVec3::new(0.001, -0.002, 0.0), DVec3::new(-1.5e-3, 2.5e-4, 3.0)];
        let stress = vec![[1.0, 2.0, 3.0, -4.0, 5.0, -6.0]; 3];
        let text = write_frd(&nodes, Some(&disp), Some(&stress));
        let f = parse_frd(&text).unwrap();
        assert_eq!(f.node_count, 3);
        assert_eq!(f.datasets.len(), 2);
        let d = f.dataset("disp").unwrap();
        assert_eq!(d.components, vec!["D1", "D2", "D3", "ALL"]);
        let dv = dense_vec3(d, 3, &[true; 3]).unwrap();
        assert!((dv[1].x - 0.001).abs() < 1e-8 && (dv[2].x + 1.5e-3).abs() < 1e-8 && (dv[2].z - 3.0).abs() < 1e-5);
        let sv = dense_tensor6(f.dataset("STRESS").unwrap(), 3, &[true; 3]).unwrap();
        assert_eq!(sv[0], [1.0, 2.0, 3.0, -4.0, 5.0, -6.0]);
        assert!((f.dataset("STRESS").unwrap().time - 1.0).abs() < 1e-9);
        // a truncated file is an error, not a short result
        let cut = text.replace(" 9999\n", "");
        let e = parse_frd(&cut).unwrap_err();
        assert!(e.contains("incomplete result"), "{e}");
        let cut2: String = text.lines().take(10).collect::<Vec<_>>().join("\n");
        assert!(parse_frd(&cut2).is_err());
    }

    #[test]
    fn missing_nodes_are_reported_unless_unused() {
        let nodes = vec![DVec3::ZERO; 3];
        let disp = vec![DVec3::X; 2];
        let f = parse_frd(&write_frd(&nodes, Some(&disp), None)).unwrap();
        let d = f.dataset("DISP").unwrap();
        assert!(dense_vec3(d, 3, &[true; 3]).is_err());
        assert!(dense_vec3(d, 3, &[true, true, false]).is_ok());
    }

    #[test]
    fn dat_tables() {
        let dat = "\n total force (fx,fy,fz) for set MOUNT1 and time  0.1000000E+01\n\n      -1.200000E+00 -3.400000E+00  8.560000E+02\n\n stresses (elem, integ.pnt.,sxx,syy,szz,sxy,sxz,syz) for set EALL and time  0.1000000E+01\n\n         1    1  1.000000E+00 2.000000E+00 3.000000E+00 4.000000E+00 5.000000E+00 6.000000E+00\n         1    2 -1.000000E+00-2.000000E+00 3.000000E+00 4.000000E+00 5.000000E+00 6.000000E+00\n\n total volume for set EALL and time  0.1000000E+01\n\n  8.000000E+03\n";
        let d = parse_dat(dat);
        assert_eq!(d.totals["MOUNT1"], [-1.2, -3.4, 856.0]);
        assert_eq!(d.ip_stress.len(), 2);
        // file order sxx syy szz sxy sxz syz -> xx yy zz xy yz zx
        assert_eq!(d.ip_stress[0].2, [1.0, 2.0, 3.0, 4.0, 6.0, 5.0]);
        assert_eq!(d.ip_stress[1].2[0], -1.0);
        assert_eq!(d.volume, Some(8000.0));
        assert!((d.time - 1.0).abs() < 1e-12);
    }

    #[test]
    fn number_tokenizer_splits_fused_values() {
        assert_eq!(numbers("1.5E+02-2.5E-03 +7"), vec![150.0, -0.0025, 7.0]);
        assert_eq!(numbers("abc 3"), vec![3.0]);
    }
}

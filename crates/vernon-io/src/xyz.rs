//! Plain-text point clouds: XYZ and Leica-style PTS.
//!
//! Read: whitespace, comma or semicolon separated numbers, one point per line.
//! Comment lines (`#`, `//`) and header lines (a lone point count, or a line of
//! words such as `x y z`) before the first point are skipped; a lone integer
//! later in the file starts another PTS scan and is skipped too.
//!
//! Columns (all rows must agree):
//!
//! | n | meaning |
//! |---|---------|
//! | 3 | `x y z` |
//! | 4 | `x y z intensity` |
//! | 6 | `x y z nx ny nz` when every row's last three values are unit length, else `x y z r g b` |
//! | 7 | `x y z intensity r g b` |
//! | 9 | `x y z nx ny nz r g b` |
//!
//! Attributes: normals -> `N`, colour -> `Cd` (0..1; values above 1 or all
//! integers are taken as 8-bit), intensity -> `intensity`.

use std::io::Write;

use vernon_core::{AttribArray, DVec3, Geometry};

use crate::{parse_err, IoResult};

/// Rows scanned to decide whether 6 columns are normals or colours.
const SNIFF_ROWS: usize = 4096;

fn split(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .filter(|t| !t.is_empty())
}

fn is_comment(line: &str) -> bool {
    line.starts_with('#') || line.starts_with("//")
}

pub(crate) fn read(bytes: &[u8], file: &str) -> IoResult<Geometry> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut rows: Vec<f64> = Vec::new();
    let mut ncols = 0usize;
    let mut nrows = 0usize;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || is_comment(line) {
            continue;
        }
        let mut vals = [0.0f64; 9];
        let mut n = 0;
        let mut numeric = true;
        let mut first = "";
        let mut more = false;
        for (k, tok) in split(line).enumerate() {
            if k == 0 {
                first = tok;
            }
            match tok.parse::<f64>() {
                Ok(v) if n < 9 => {
                    vals[n] = v;
                    n += 1;
                }
                Ok(_) => more = true,
                Err(_) => numeric = false,
            }
            if !numeric {
                break;
            }
        }
        let single_int = n == 1 && !more && first.parse::<u64>().is_ok();
        if !numeric || single_int {
            // Header text or a PTS point-count line. Only allowed before the
            // first point, except for repeated counts between PTS scans.
            if nrows == 0 || single_int {
                continue;
            }
            return Err(parse_err(
                file,
                i + 1,
                format!("expected numbers, found '{line}'"),
            ));
        }
        if more || !matches!(n, 3 | 4 | 6 | 7 | 9) {
            return Err(parse_err(
                file,
                i + 1,
                format!(
                    "{} columns: expected 3, 4, 6, 7 or 9",
                    if more {
                        ">9".to_string()
                    } else {
                        n.to_string()
                    }
                ),
            ));
        }
        if nrows == 0 {
            ncols = n;
        } else if n != ncols {
            return Err(parse_err(
                file,
                i + 1,
                format!("{n} columns, but earlier rows have {ncols}"),
            ));
        }
        if !vals[..3].iter().all(|v| v.is_finite()) {
            return Err(parse_err(file, i + 1, "non-finite coordinate"));
        }
        rows.extend_from_slice(&vals[..n]);
        nrows += 1;
    }
    if nrows == 0 {
        return Err(parse_err(file, 0, "no points found"));
    }

    // Column roles.
    let (normal_at, colour_at, intensity_at) = match ncols {
        3 => (None, None, None),
        4 => (None, None, Some(3)),
        6 => {
            let unit_rows = rows
                .chunks(6)
                .take(SNIFF_ROWS)
                .all(|r| (DVec3::new(r[3], r[4], r[5]).length() - 1.0).abs() < 1e-2);
            if unit_rows {
                (Some(3), None, None)
            } else {
                (None, Some(3), None)
            }
        }
        7 => (None, Some(4), Some(3)),
        _ => (Some(3), Some(6), None),
    };
    let get = |r: &[f64], o: usize| DVec3::new(r[o], r[o + 1], r[o + 2]);
    let finite3 = |v: DVec3| if v.is_finite() { v } else { DVec3::ZERO };
    let mut geo = Geometry::from_points(rows.chunks(ncols).map(|r| get(r, 0)).collect());
    if let Some(o) = normal_at {
        let n: Vec<DVec3> = rows.chunks(ncols).map(|r| finite3(get(r, o))).collect();
        geo.point_attribs.set("N", AttribArray::Vec3(n));
    }
    if let Some(o) = colour_at {
        let c: Vec<DVec3> = rows.chunks(ncols).map(|r| finite3(get(r, o))).collect();
        // 8-bit if anything exceeds 1, or every value is a whole number.
        let eight_bit = c
            .iter()
            .any(|v| v.max_element() > 1.0 || v.min_element() < 0.0)
            || c.iter()
                .all(|v| v.to_array().iter().all(|x| x.fract() == 0.0));
        let k = if eight_bit { 1.0 / 255.0 } else { 1.0 };
        let c = c
            .into_iter()
            .map(|v| (v * k).clamp(DVec3::ZERO, DVec3::ONE))
            .collect();
        geo.point_attribs.set("Cd", AttribArray::Vec3(c));
    }
    if let Some(o) = intensity_at {
        let v: Vec<f64> = rows
            .chunks(ncols)
            .map(|r| if r[o].is_finite() { r[o] } else { 0.0 })
            .collect();
        geo.point_attribs.set("intensity", AttribArray::Float(v));
    }
    Ok(geo)
}

fn fmt(v: f64) -> f64 {
    if v == 0.0 {
        0.0
    } else {
        v
    }
}

pub(crate) fn write<W: Write>(w: &mut W, geo: &Geometry, scale: f64, pts: bool) -> IoResult<()> {
    let normals = geo.point_attribs.vec3s("N").ok();
    let colours = geo.point_attribs.vec3s("Cd").ok();
    let intensity = geo.point_attribs.floats("intensity").ok();
    let rgb = |c: DVec3| {
        let b = (c.clamp(DVec3::ZERO, DVec3::ONE) * 255.0).round();
        (b.x as u8, b.y as u8, b.z as u8)
    };
    if pts {
        writeln!(w, "{}", geo.point_count())?;
        for (i, p) in geo.points.iter().enumerate() {
            let q = *p * scale;
            let inten = intensity.map_or(0.0, |v| v[i]);
            write!(w, "{} {} {} {}", fmt(q.x), fmt(q.y), fmt(q.z), fmt(inten))?;
            if let Some(c) = colours {
                let (r, g, b) = rgb(c[i]);
                write!(w, " {r} {g} {b}")?;
            }
            writeln!(w)?;
        }
        return Ok(());
    }
    for (i, p) in geo.points.iter().enumerate() {
        let q = *p * scale;
        write!(w, "{} {} {}", fmt(q.x), fmt(q.y), fmt(q.z))?;
        if let Some(n) = normals {
            write!(w, " {} {} {}", fmt(n[i].x), fmt(n[i].y), fmt(n[i].z))?;
        }
        if let Some(c) = colours {
            let (r, g, b) = rgb(c[i]);
            write!(w, " {r} {g} {b}")?;
        }
        writeln!(w)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rd(s: &str) -> IoResult<Geometry> {
        read(s.as_bytes(), "t.xyz")
    }

    #[test]
    fn plain_xyz_with_comments_headers_and_separators() {
        let g = rd("# cloud\n// another\nx y z\n1 2 3\n4,5,6\n\n7;8;9\n").unwrap();
        assert_eq!(g.point_count(), 3);
        assert!(g.is_point_cloud());
        assert_eq!(g.points[1], DVec3::new(4.0, 5.0, 6.0));
        assert!(g.point_attribs.is_empty());
    }

    #[test]
    fn six_columns_are_normals_or_colours() {
        let g = rd("0 0 0 0 0 1\n1 0 0 1 0 0\n").unwrap();
        assert_eq!(g.point_attribs.vec3s("N").unwrap()[0], DVec3::Z);
        assert!(!g.point_attribs.contains("Cd"));
        let g = rd("0 0 0 255 128 0\n1 0 0 0 0 0\n").unwrap();
        let c = g.point_attribs.vec3s("Cd").unwrap();
        assert!((c[0].y - 128.0 / 255.0).abs() < 1e-12 && c[0].x == 1.0);
        let g = rd("0 0 0 0.5 0.25 1.0\n").unwrap();
        assert_eq!(
            g.point_attribs.vec3s("Cd").unwrap()[0],
            DVec3::new(0.5, 0.25, 1.0)
        );
    }

    #[test]
    fn nine_columns_carry_normals_and_colour() {
        let g = rd("1 2 3 0 1 0 10 20 30\n").unwrap();
        assert_eq!(g.point_attribs.vec3s("N").unwrap()[0], DVec3::Y);
        assert!((g.point_attribs.vec3s("Cd").unwrap()[0].z - 30.0 / 255.0).abs() < 1e-12);
    }

    #[test]
    fn pts_count_lines_intensity_and_colour() {
        let g = rd("3\n1 2 3 100 255 0 0\n4 5 6 200 0 255 0\n7 8 9 300 0 0 255\n").unwrap();
        assert_eq!(g.point_count(), 3);
        assert_eq!(
            g.point_attribs.floats("intensity").unwrap(),
            &[100.0, 200.0, 300.0]
        );
        assert_eq!(g.point_attribs.vec3s("Cd").unwrap()[2], DVec3::Z);
        // A second scan header in the middle is skipped.
        let g = rd("2\n0 0 0 1\n1 1 1 2\n2\n2 2 2 3\n3 3 3 4\n").unwrap();
        assert_eq!(g.point_count(), 4);
        // Count lines with a trailing comment-free single integer only.
        assert_eq!(rd("1\n5 5 5\n").unwrap().point_count(), 1);
    }

    #[test]
    fn bad_input_reports_line_numbers() {
        for (src, needle) in [
            ("1 2 3\n1 2\n", "line 2"),
            ("1 2 3\n4 5 six\n", "line 2"),
            ("1 2 3\n1 2 3 4 5\n", "line 2"),
            ("1 2 3 4 5 6 7 8 9 10\n", "line 1"),
            ("1 nan 3\n", "line 1"),
            ("1 inf 3\n", "line 1"),
        ] {
            let e = rd(src).unwrap_err().to_string();
            assert!(e.contains(needle), "{src:?} -> {e}");
        }
        assert!(rd("").is_err());
        assert!(rd("# nothing\n").is_err());
        assert!(rd("x y z\n").is_err());
    }

    #[test]
    fn xyz_round_trip_keeps_normals_and_quantises_colour() {
        let mut g = Geometry::from_points(vec![
            DVec3::new(-0.0, 1.5, 1e-7),
            DVec3::new(1234.5678, -9.0, 0.1 + 0.2),
        ]);
        g.point_attribs.set(
            "N",
            AttribArray::Vec3(vec![DVec3::Z, DVec3::new(0.6, 0.0, 0.8)]),
        );
        g.point_attribs.set(
            "Cd",
            AttribArray::Vec3(vec![DVec3::new(1.0, 0.0, 0.5), DVec3::new(0.2, 0.4, 0.6)]),
        );
        let mut buf = Vec::new();
        write(&mut buf, &g, 1.0, false).unwrap();
        let text = String::from_utf8(buf.clone()).unwrap();
        assert!(
            text.starts_with("0 1.5 0.0000001 0 0 1 255 0 128\n"),
            "{text}"
        );
        let b = read(&buf, "t").unwrap();
        assert_eq!(b.points[1], g.points[1]);
        assert_eq!(
            b.point_attribs.vec3s("N").unwrap(),
            g.point_attribs.vec3s("N").unwrap()
        );
        let (c0, c1) = (
            g.point_attribs.vec3s("Cd").unwrap(),
            b.point_attribs.vec3s("Cd").unwrap(),
        );
        assert!(c0
            .iter()
            .zip(c1)
            .all(|(a, b)| (*a - *b).abs().max_element() <= 0.5 / 255.0 + 1e-12));
    }

    #[test]
    fn pts_round_trip() {
        let mut g =
            Geometry::from_points(vec![DVec3::new(1.0, 2.0, 3.0), DVec3::new(-4.0, 5.5, 6.0)]);
        g.point_attribs
            .set("intensity", AttribArray::Float(vec![0.25, -800.0]));
        g.point_attribs.set(
            "Cd",
            AttribArray::Vec3(vec![DVec3::X, DVec3::new(0.0, 1.0, 1.0)]),
        );
        let mut buf = Vec::new();
        write(&mut buf, &g, 1.0, true).unwrap();
        assert!(String::from_utf8_lossy(&buf).starts_with("2\n1 2 3 0.25 255 0 0\n"));
        let b = read(&buf, "t.pts").unwrap();
        assert_eq!(b.points, g.points);
        assert_eq!(
            b.point_attribs.floats("intensity").unwrap(),
            &[0.25, -800.0]
        );
        assert_eq!(
            b.point_attribs.vec3s("Cd").unwrap(),
            g.point_attribs.vec3s("Cd").unwrap()
        );
    }

    #[test]
    fn scale_is_applied_on_write() {
        let g = Geometry::from_points(vec![DVec3::new(25.4, 0.0, 0.0)]);
        let mut buf = Vec::new();
        write(&mut buf, &g, 1.0 / 25.4, false).unwrap();
        let b = read(&buf, "t").unwrap();
        assert!((b.points[0].x - 1.0).abs() < 1e-12);
    }
}

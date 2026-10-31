//! LAS 1.2 to 1.4 point-cloud reader (uncompressed).
//!
//! Written from the public format description. Reads the public header block,
//! the variable length records that describe the coordinate system, and point
//! data records of formats 0 to 10. Positions are `integer * scale + offset`
//! per axis. Attributes: `intensity` (the stored 16-bit value), `Cd` (red,
//! green, blue mapped to 0..1; channels whose largest value is 255 or less are
//! taken as 8-bit) for formats that carry colour, and `classification`.
//!
//! Units. The header has no unit field, so the length unit comes from the
//! caller if given, else from the coordinate-system records: the GeoTIFF key
//! directory (ProjLinearUnitsGeoKey for X and Y, VerticalUnitsGeoKey for Z,
//! which follows the horizontal unit when absent) or a WKT string (the
//! projected CRS unit, used for all three axes). A file that states no
//! horizontal unit, or only an EPSG code, is taken as metres, the usual case;
//! a CRS that is geographic (degrees) is an error unless the caller names a
//! unit. The result is always millimetres.
//!
//! How the unit was decided is recorded in the `las_unit_source` detail
//! attribute: `parameter` (the caller named it), `header` (the file's
//! coordinate-system records state it) or `assumed_metres` (they do not, so
//! the metre was a guess and a US-foot file would be off by a factor of 3.28).
//!
//! LAZ (compressed) files are detected and refused with a clear error. Every
//! count and offset is checked against the file length before use.

use vernon_core::{AttribArray, DVec3, Geometry};

use crate::bin::{f64_at, i32_at, u16_at, u32_at, u64_at, u8_at};
use crate::{parse_err, FileUnit, IoResult};

type R<T> = Result<T, String>;

/// Smallest public header block (LAS 1.2).
const HEADER_1_2: usize = 227;
const HEADER_1_3: usize = 235;
const HEADER_1_4: usize = 375;
const VLR_HEADER: usize = 54;
/// Bytes per point record before any extra bytes, by point data format.
const RECORD_BASE: [usize; 11] = [20, 28, 26, 34, 57, 63, 30, 36, 38, 59, 67];
const METRE_MM: f64 = 1000.0;
const LAZ_HELP: &str = "LAZ (compressed LAS) files are not supported; decompress the file to plain LAS first";

/// Value of the `las_unit_source` detail attribute: the caller named the unit.
pub(crate) const UNIT_FROM_PARAMETER: &str = "parameter";
/// The file's coordinate-system records state the unit.
pub(crate) const UNIT_FROM_HEADER: &str = "header";
/// The file does not state its horizontal unit; metres were assumed.
pub(crate) const UNIT_ASSUMED_METRES: &str = "assumed_metres";

/// Coordinate system as far as units go.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Crs {
    /// A projected or local CRS. The units are millimetres per file unit for
    /// the horizontal axes and for Z; `None` when the records do not state it.
    Projected { xy: Option<f64>, z: Option<f64> },
    Geographic,
}

impl Crs {
    /// A projected CRS whose records state no unit at all.
    const UNSTATED: Crs = Crs::Projected { xy: None, z: None };
}

pub(crate) fn read(bytes: &[u8], file: &str, unit: Option<FileUnit>) -> IoResult<Geometry> {
    read_inner(bytes, unit).map_err(|m| parse_err(file, 0, format!("LAS: {m}")))
}

fn read_inner(bytes: &[u8], unit: Option<FileUnit>) -> R<Geometry> {
    if bytes.len() < 4 || &bytes[..4] != b"LASF" {
        return Err("not a LAS file (the LASF signature is missing)".into());
    }
    if bytes.len() < HEADER_1_2 {
        return Err(format!("the file is {} bytes, shorter than a LAS header", bytes.len()));
    }
    let (major, minor) = (u8_at(bytes, 24)?, u8_at(bytes, 25)?);
    if major != 1 || !(2..=4).contains(&minor) {
        return Err(format!("unsupported LAS version {major}.{minor} (supported: 1.2 to 1.4)"));
    }
    let header_size = usize::from(u16_at(bytes, 94)?);
    let min_header = match minor {
        2 => HEADER_1_2,
        3 => HEADER_1_3,
        _ => HEADER_1_4,
    };
    if header_size < min_header || header_size > bytes.len() {
        return Err(format!(
            "header size {header_size} is invalid for LAS 1.{minor} in a {}-byte file",
            bytes.len()
        ));
    }
    let data_start = usize::try_from(u32_at(bytes, 96)?).unwrap_or(usize::MAX);
    if data_start < header_size || data_start > bytes.len() {
        return Err(format!(
            "the point data offset {data_start} lies outside the file or inside the header"
        ));
    }

    let format_byte = u8_at(bytes, 104)?;
    if format_byte & 0xC0 != 0 {
        return Err(LAZ_HELP.into());
    }
    let fmt = usize::from(format_byte);
    let newest = match minor {
        2 => 3,
        3 => 5,
        _ => 10,
    };
    if fmt > newest {
        return Err(format!("point data format {fmt} does not exist in LAS 1.{minor}"));
    }
    let record_len = usize::from(u16_at(bytes, 105)?);
    if record_len < RECORD_BASE[fmt] {
        return Err(format!(
            "point record length {record_len} is shorter than the {} bytes format {fmt} needs",
            RECORD_BASE[fmt]
        ));
    }
    let legacy = u64::from(u32_at(bytes, 107)?);
    let count = if minor >= 4 {
        match u64_at(bytes, 247)? {
            0 => legacy,
            n => n,
        }
    } else {
        legacy
    };
    let avail = (bytes.len() - data_start) as u128;
    if u128::from(count) * record_len as u128 > avail {
        return Err(format!(
            "the header declares {count} points of {record_len} bytes but only {avail} bytes follow the header: the file is truncated"
        ));
    }
    let count = count as usize; // at most avail / 20, so it fits

    let triple = |at: usize| -> R<[f64; 3]> {
        Ok([f64_at(bytes, at)?, f64_at(bytes, at + 8)?, f64_at(bytes, at + 16)?])
    };
    let (scale, offset) = (triple(131)?, triple(155)?);
    if scale.iter().any(|v| !v.is_finite() || *v == 0.0) || offset.iter().any(|v| !v.is_finite()) {
        return Err("the header has a zero or non-finite scale or offset".into());
    }

    // Millimetres per file unit for X and Y, and for Z, and how they were decided.
    let ([xy_mm, z_mm], unit_source) = match unit {
        Some(u) => ([u.to_mm(); 2], UNIT_FROM_PARAMETER),
        None => {
            let n_vlr = u32_at(bytes, 100)?;
            let wkt_first = u16_at(bytes, 6)? & (1 << 4) != 0;
            match crs_of_vlrs(&bytes[header_size..data_start], n_vlr, wkt_first)? {
                Some(Crs::Projected { xy: Some(xy), z }) => ([xy, z.unwrap_or(xy)], UNIT_FROM_HEADER),
                // The horizontal unit is not stated, so it is a guess even when
                // the vertical unit is.
                Some(Crs::Projected { xy: None, z }) => {
                    ([METRE_MM, z.unwrap_or(METRE_MM)], UNIT_ASSUMED_METRES)
                }
                Some(Crs::Geographic) => {
                    return Err(
                        "the coordinates are geographic (degrees), not lengths; name a unit to read them anyway"
                            .into(),
                    )
                }
                None => ([METRE_MM; 2], UNIT_ASSUMED_METRES),
            }
        }
    };
    let axis_mm = [xy_mm, xy_mm, z_mm];

    let rgb_at = match fmt {
        2 => Some(20),
        3 | 5 => Some(28),
        7 | 8 | 10 => Some(30),
        _ => None,
    };
    let (class_at, class_mask) = if fmt <= 5 { (15, 0x1F) } else { (16, 0xFF) };
    let body = &bytes[data_start..data_start + count * record_len];
    let mut points = Vec::with_capacity(count);
    let mut intensity = Vec::with_capacity(count);
    let mut class = Vec::with_capacity(count);
    let mut raw_rgb: Vec<[u16; 3]> = Vec::new();
    for rec in body.chunks_exact(record_len) {
        let ints = [i32_at(rec, 0)?, i32_at(rec, 4)?, i32_at(rec, 8)?];
        let p = |a: usize| (f64::from(ints[a]) * scale[a] + offset[a]) * axis_mm[a];
        points.push(DVec3::new(p(0), p(1), p(2)));
        intensity.push(f64::from(u16_at(rec, 12)?));
        class.push(i64::from(u8_at(rec, class_at)? & class_mask));
        if let Some(o) = rgb_at {
            raw_rgb.push([u16_at(rec, o)?, u16_at(rec, o + 2)?, u16_at(rec, o + 4)?]);
        }
    }
    let mut geo = Geometry::from_points(points);
    geo.detail_attribs
        .set("las_unit_source", AttribArray::Str(vec![unit_source.to_string()]));
    geo.point_attribs.set("intensity", AttribArray::Float(intensity));
    geo.point_attribs.set("classification", AttribArray::Int(class));
    if rgb_at.is_some() {
        let largest = raw_rgb.iter().flatten().copied().max().unwrap_or(0);
        let div = if largest <= 255 { 255.0 } else { 65535.0 };
        let cd = raw_rgb
            .iter()
            .map(|c| DVec3::new(f64::from(c[0]), f64::from(c[1]), f64::from(c[2])) / div)
            .collect();
        geo.point_attribs.set("Cd", AttribArray::Vec3(cd));
    }
    Ok(geo)
}

// ----------------------------------------------------- coordinate system

/// Walk the variable length records in `region` (header end to point data)
/// and report the CRS unit. Refuses LAZ. `None` when no CRS record exists.
fn crs_of_vlrs(region: &[u8], n_vlr: u32, wkt_first: bool) -> R<Option<Crs>> {
    if u128::from(n_vlr) * VLR_HEADER as u128 > region.len() as u128 {
        return Err(format!(
            "{n_vlr} variable length records cannot fit between the header and the point data"
        ));
    }
    let (mut geotiff, mut wkt) = (None, None);
    let mut pos = 0usize;
    for _ in 0..n_vlr {
        let head = region
            .get(pos..pos + VLR_HEADER)
            .ok_or("a variable length record header is cut off")?;
        let user = &head[2..18];
        let user = &user[..user.iter().position(|&b| b == 0).unwrap_or(user.len())];
        let record_id = u16_at(head, 18)?;
        let len = usize::from(u16_at(head, 20)?);
        let body = region
            .get(pos + VLR_HEADER..pos + VLR_HEADER + len)
            .ok_or("a variable length record runs past the point data offset")?;
        if user == b"laszip encoded" {
            return Err(LAZ_HELP.into());
        }
        if user == b"LASF_Projection" {
            match record_id {
                34735 => geotiff = Some(geotiff_crs(body)?),
                2111 | 2112 => wkt = Some(wkt_crs(body)?),
                _ => {}
            }
        }
        pos += VLR_HEADER + len;
    }
    let order = if wkt_first { [wkt, geotiff] } else { [geotiff, wkt] };
    // The first record that states the horizontal unit (or is geographic)
    // wins; failing that, one that states at least the vertical unit.
    let first = |keep: fn(&Crs) -> bool| order.into_iter().flatten().find(keep);
    Ok(first(|c| matches!(c, Crs::Geographic | Crs::Projected { xy: Some(_), .. }))
        .or_else(|| first(|c| *c != Crs::UNSTATED))
        .or_else(|| geotiff.or(wkt)))
}

/// GeoKeyDirectoryTag: a 4-word header, then 4-word key entries.
fn geotiff_crs(body: &[u8]) -> R<Crs> {
    let n = usize::from(u16_at(body, 6)?);
    if 8 + n * 8 > body.len() {
        return Err("the GeoTIFF key directory is cut off".into());
    }
    let (mut model, mut horizontal, mut vertical) = (None, None, None);
    for i in 0..n {
        let o = 8 + i * 8;
        let (key, location, value) = (u16_at(body, o)?, u16_at(body, o + 2)?, u16_at(body, o + 6)?);
        if location != 0 {
            continue; // the value lives in another tag, not in the entry
        }
        match key {
            1024 => model = Some(value),      // GTModelTypeGeoKey: 1 projected, 2 geographic
            3076 => horizontal = Some(value), // ProjLinearUnitsGeoKey: X and Y
            4099 => vertical = Some(value),   // VerticalUnitsGeoKey: Z
            // ProjectedCSTypeGeoKey (3072) names an EPSG code; reading the
            // unit out of the code would need the EPSG database, so a CRS
            // given only that way states no unit.
            _ => {}
        }
    }
    if model == Some(2) {
        return Ok(Crs::Geographic);
    }
    Ok(Crs::Projected {
        xy: horizontal.map(|c| geotiff_unit(c, "linear")).transpose()?,
        z: vertical.map(|c| geotiff_unit(c, "vertical")).transpose()?,
    })
}

/// Millimetres per unit for an EPSG length unit code (metre, foot, US survey foot).
fn geotiff_unit(code: u16, which: &str) -> R<f64> {
    match code {
        9001 => Ok(METRE_MM),
        9002 => Ok(304.8),
        9003 => Ok(1200.0 / 3937.0 * METRE_MM),
        _ => Err(format!(
            "the GeoTIFF {which} unit code {code} is not supported; name a unit to read the file"
        )),
    }
}

/// WKT 1 (`PROJCS[... UNIT["name", factor]]`) or WKT 2 (`LENGTHUNIT`): the
/// last length unit inside the projected CRS block is the one of the plane
/// coordinates.
fn wkt_crs(body: &[u8]) -> R<Crs> {
    let text = String::from_utf8_lossy(body).to_ascii_uppercase();
    let text = text.trim_end_matches('\0');
    let Some(start) = text.find("PROJCS[").or_else(|| text.find("PROJCRS[")) else {
        let geographic = ["GEOGCS[", "GEOGCRS[", "GEODCRS["].iter().any(|k| text.contains(k));
        return Ok(if geographic { Crs::Geographic } else { Crs::UNSTATED });
    };
    // Cut the PROJCS block out by bracket matching so a vertical CRS that
    // follows in a compound system cannot supply the unit.
    let mut depth = 0i32;
    let mut quoted = false;
    let mut end = text.len();
    for (i, c) in text[start..].char_indices() {
        match c {
            '"' => quoted = !quoted,
            '[' if !quoted => depth += 1,
            ']' if !quoted => {
                depth -= 1;
                if depth == 0 {
                    end = start + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    let block = &text[start..end];
    let mut last = None;
    for (m, _) in block.match_indices("UNIT[") {
        let before = &block[..m];
        let kw = before.len() - before.chars().rev().take_while(char::is_ascii_alphabetic).count();
        if matches!(&before[kw..], "" | "LENGTH") {
            last = Some(m + 5);
        }
    }
    let Some(at) = last else {
        return Ok(Crs::UNSTATED);
    };
    let rest = &block[at..];
    let after_name = if let Some(quoted) = rest.strip_prefix('"') {
        quoted.find('"').map(|q| &quoted[q + 1..])
    } else {
        rest.find(',').map(|c| &rest[c..])
    };
    let factor = after_name
        .map(|r| r.trim_start_matches(',').trim_start())
        .and_then(|r| r.split([',', ']']).next())
        .and_then(|n| n.trim().parse::<f64>().ok())
        .filter(|f| f.is_finite() && *f > 0.0)
        .ok_or("cannot read the length unit in the WKT coordinate system")?;
    // A compound system's vertical CRS is not read: Z takes the plane unit.
    Ok(Crs::Projected { xy: Some(factor * METRE_MM), z: None })
}

#[cfg(test)]
#[allow(clippy::needless_range_loop)]
pub(crate) mod testing {
    //! Builds small LAS files for tests (here and in `lib.rs`).

    /// One point: stored integers plus the attributes the reader exposes.
    #[derive(Clone)]
    pub struct P {
        pub xyz: [i32; 3],
        pub intensity: u16,
        pub class: u8,
        pub rgb: [u16; 3],
    }

    #[derive(Clone)]
    pub struct Spec {
        pub minor: u8,
        pub fmt: u8,
        pub scale: [f64; 3],
        pub offset: [f64; 3],
        pub pts: Vec<P>,
        /// (user id, record id, body)
        pub vlrs: Vec<(&'static str, u16, Vec<u8>)>,
        pub extra_record_bytes: usize,
        pub global_encoding: u16,
    }

    impl Spec {
        pub fn new(minor: u8, fmt: u8, pts: Vec<P>) -> Self {
            Self {
                minor,
                fmt,
                scale: [0.001; 3],
                offset: [100.0, -50.0, 2.5],
                pts,
                vlrs: Vec::new(),
                extra_record_bytes: 0,
                global_encoding: 0,
            }
        }
    }

    pub const BASE: [usize; 11] = [20, 28, 26, 34, 57, 63, 30, 36, 38, 59, 67];

    pub fn points(n: usize) -> Vec<P> {
        (0..n as i32)
            .map(|i| P {
                xyz: [i * 1500 - 20_000, -i * 777 + 5, i * i * 3 - 100],
                intensity: (i as u16).wrapping_mul(997),
                class: (i % 20) as u8,
                rgb: [
                    (i as u16).wrapping_mul(4000).wrapping_add(300),
                    65535u16.wrapping_sub((i as u16).wrapping_mul(3000)),
                    256 + i as u16,
                ],
            })
            .collect()
    }

    pub fn vlr(user: &'static str, record: u16, body: Vec<u8>) -> (&'static str, u16, Vec<u8>) {
        (user, record, body)
    }

    /// GeoKeyDirectoryTag body from (key, value) pairs stored inline.
    pub fn geokeys(keys: &[(u16, u16)]) -> Vec<u8> {
        let mut words = vec![1u16, 1, 0, keys.len() as u16];
        for (k, v) in keys {
            words.extend([*k, 0, 1, *v]);
        }
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    pub fn build(s: &Spec) -> Vec<u8> {
        let header = match s.minor {
            2 => 227,
            3 => 235,
            _ => 375,
        };
        let vlr_bytes: usize = s.vlrs.iter().map(|v| 54 + v.2.len()).sum();
        let rec_len = BASE[s.fmt as usize] + s.extra_record_bytes;
        let mut b = vec![0u8; header];
        b[0..4].copy_from_slice(b"LASF");
        b[6..8].copy_from_slice(&s.global_encoding.to_le_bytes());
        b[24] = 1;
        b[25] = s.minor;
        b[94..96].copy_from_slice(&(header as u16).to_le_bytes());
        b[96..100].copy_from_slice(&((header + vlr_bytes) as u32).to_le_bytes());
        b[100..104].copy_from_slice(&(s.vlrs.len() as u32).to_le_bytes());
        b[104] = s.fmt;
        b[105..107].copy_from_slice(&(rec_len as u16).to_le_bytes());
        let n = s.pts.len() as u32;
        let legacy = if s.minor == 4 && s.fmt >= 6 { 0 } else { n };
        b[107..111].copy_from_slice(&legacy.to_le_bytes());
        for a in 0..3 {
            b[131 + 8 * a..139 + 8 * a].copy_from_slice(&s.scale[a].to_le_bytes());
            b[155 + 8 * a..163 + 8 * a].copy_from_slice(&s.offset[a].to_le_bytes());
        }
        if s.minor == 4 {
            b[247..255].copy_from_slice(&u64::from(n).to_le_bytes());
        }
        for (user, id, body) in &s.vlrs {
            let mut h = vec![0u8; 54];
            h[2..2 + user.len()].copy_from_slice(user.as_bytes());
            h[18..20].copy_from_slice(&id.to_le_bytes());
            h[20..22].copy_from_slice(&(body.len() as u16).to_le_bytes());
            b.extend(h);
            b.extend_from_slice(body);
        }
        let rgb_at = match s.fmt {
            2 => Some(20),
            3 | 5 => Some(28),
            7 | 8 | 10 => Some(30),
            _ => None,
        };
        for p in &s.pts {
            let mut r = vec![0u8; rec_len];
            for a in 0..3 {
                r[4 * a..4 * a + 4].copy_from_slice(&p.xyz[a].to_le_bytes());
            }
            r[12..14].copy_from_slice(&p.intensity.to_le_bytes());
            let class_at = if s.fmt <= 5 { 15 } else { 16 };
            // Flag bits above the class (formats 0 to 5) must be ignored.
            r[class_at] = if s.fmt <= 5 { p.class | 0xE0 } else { p.class };
            if let Some(o) = rgb_at {
                for c in 0..3 {
                    r[o + 2 * c..o + 2 * c + 2].copy_from_slice(&p.rgb[c].to_le_bytes());
                }
            }
            b.extend(r);
        }
        b
    }

    /// Small valid file used by `lib.rs` tests: 25 points, format 2 (colour).
    pub fn sample_file() -> Vec<u8> {
        build(&Spec::new(2, 2, points(25)))
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    fn rd(bytes: &[u8]) -> IoResult<Geometry> {
        read(bytes, "t.las", None)
    }

    fn err_text(bytes: &[u8]) -> String {
        rd(bytes).expect_err("expected an error").to_string()
    }

    fn mm(spec: &Spec, i: usize, factor: f64) -> DVec3 {
        let p = &spec.pts[i];
        let a = |k: usize| (f64::from(p.xyz[k]) * spec.scale[k] + spec.offset[k]) * factor;
        DVec3::new(a(0), a(1), a(2))
    }

    fn minor_for(fmt: u8) -> u8 {
        match fmt {
            0..=3 => 2,
            4 | 5 => 3,
            _ => 4,
        }
    }

    #[test]
    fn every_point_format_reads_back_in_millimetres() {
        for fmt in 0..=10u8 {
            let spec = Spec::new(minor_for(fmt), fmt, points(9));
            let g = rd(&build(&spec)).unwrap_or_else(|e| panic!("format {fmt}: {e}"));
            assert_eq!(g.point_count(), 9, "format {fmt}");
            for i in 0..9 {
                // File numbers are metres: 1000 mm each.
                assert!((g.points[i] - mm(&spec, i, 1000.0)).abs().max_element() < 1e-6, "format {fmt} point {i}");
            }
            let it = g.point_attribs.floats("intensity").unwrap();
            assert_eq!(it[3], f64::from(spec.pts[3].intensity), "format {fmt}");
            let cl = g.point_attribs.ints("classification").unwrap();
            assert_eq!(cl[5], i64::from(spec.pts[5].class), "format {fmt}");
            let has_rgb = matches!(fmt, 2 | 3 | 5 | 7 | 8 | 10);
            assert_eq!(g.point_attribs.contains("Cd"), has_rgb, "format {fmt}");
            if has_rgb {
                let cd = g.point_attribs.vec3s("Cd").unwrap()[2];
                let want = spec.pts[2].rgb;
                assert!((cd.x - f64::from(want[0]) / 65535.0).abs() < 1e-12, "format {fmt}");
                assert!((cd.z - f64::from(want[2]) / 65535.0).abs() < 1e-12, "format {fmt}");
            }
        }
    }

    #[test]
    fn scale_and_offset_golden_value() {
        let mut spec = Spec::new(2, 0, vec![P { xyz: [-123_456, 7, 2_000_000], intensity: 0, class: 0, rgb: [0; 3] }]);
        spec.scale = [0.001, 0.0025, 0.01];
        spec.offset = [1000.5, -3.0, 0.0];
        let g = rd(&build(&spec)).unwrap();
        // -123456 * 0.001 + 1000.5 = 877.044 m; 7 * 0.0025 - 3 = -2.9825 m; 20000 m.
        assert!((g.points[0] - DVec3::new(877_044.0, -2_982.5, 20_000_000.0)).abs().max_element() < 1e-6);
    }

    #[test]
    fn extra_record_bytes_are_skipped() {
        let mut spec = Spec::new(4, 7, points(6));
        spec.extra_record_bytes = 5;
        let g = rd(&build(&spec)).unwrap();
        assert_eq!(g.point_count(), 6);
        assert!((g.points[4] - mm(&spec, 4, 1000.0)).abs().max_element() < 1e-6);
    }

    #[test]
    fn colour_stored_as_eight_bit_is_detected() {
        let mut pts = points(3);
        for (i, p) in pts.iter_mut().enumerate() {
            p.rgb = [255, (i * 100) as u16, 0];
        }
        let g = rd(&build(&Spec::new(2, 2, pts))).unwrap();
        let cd = g.point_attribs.vec3s("Cd").unwrap();
        assert_eq!(cd[0].x, 1.0);
        assert!((cd[2].y - 200.0 / 255.0).abs() < 1e-12);
    }

    #[test]
    fn explicit_unit_overrides_the_default_and_the_header() {
        let mut spec = Spec::new(2, 0, points(4));
        let bytes = build(&spec);
        let g = read(&bytes, "t.las", Some(FileUnit::Centimetre)).unwrap();
        assert!((g.points[1] - mm(&spec, 1, 10.0)).abs().max_element() < 1e-6);
        let g = read(&bytes, "t.las", Some(FileUnit::Millimetre)).unwrap();
        assert!((g.points[1] - mm(&spec, 1, 1.0)).abs().max_element() < 1e-9);
        // A header that says feet is overridden by an explicit unit.
        spec.vlrs.push(vlr("LASF_Projection", 34735, geokeys(&[(1024, 1), (3076, 9002)])));
        let g = read(&build(&spec), "t.las", Some(FileUnit::Inch)).unwrap();
        assert!((g.points[1] - mm(&spec, 1, 25.4)).abs().max_element() < 1e-6);
    }

    #[test]
    fn geotiff_keys_give_the_unit() {
        let with = |keys: &[(u16, u16)]| {
            let mut spec = Spec::new(2, 0, points(4));
            spec.vlrs.push(vlr("LASF_Projection", 34735, geokeys(keys)));
            (rd(&build(&spec)), spec)
        };
        let (g, spec) = with(&[(1024, 1), (3076, 9002)]);
        assert!((g.unwrap().points[2] - mm(&spec, 2, 304.8)).abs().max_element() < 1e-6);
        let (g, spec) = with(&[(1024, 1), (3076, 9003)]);
        assert!((g.unwrap().points[2] - mm(&spec, 2, 304.800_609_601_219_2)).abs().max_element() < 1e-6);
        let (g, spec) = with(&[(1024, 1), (3076, 9001)]);
        assert!((g.unwrap().points[2] - mm(&spec, 2, 1000.0)).abs().max_element() < 1e-6);
        // Projected by EPSG code only: no unit key, taken as metres.
        let (g, spec) = with(&[(1024, 1), (3072, 32633)]);
        assert!((g.unwrap().points[2] - mm(&spec, 2, 1000.0)).abs().max_element() < 1e-6);
        // Geographic degrees are refused, and allowed once a unit is named.
        let (g, spec) = with(&[(1024, 2), (2048, 4326)]);
        assert!(g.unwrap_err().to_string().contains("geographic"));
        let g = read(&build(&spec), "t.las", Some(FileUnit::Metre)).unwrap();
        assert_eq!(g.point_count(), 4);
        // An unknown unit code is an error, not a guess.
        let (g, _) = with(&[(1024, 1), (3076, 9005)]);
        assert!(g.unwrap_err().to_string().contains("9005"));
    }

    /// Millimetres of point `i` with a separate factor per axis.
    fn mm3(spec: &Spec, i: usize, k: [f64; 3]) -> DVec3 {
        let p = &spec.pts[i];
        let a = |a: usize| (f64::from(p.xyz[a]) * spec.scale[a] + spec.offset[a]) * k[a];
        DVec3::new(a(0), a(1), a(2))
    }

    fn source(g: &Geometry) -> String {
        g.detail_attribs.strs("las_unit_source").expect("las_unit_source").join("|")
    }

    fn with_keys(keys: &[(u16, u16)]) -> (IoResult<Geometry>, Spec) {
        let mut spec = Spec::new(2, 0, points(4));
        spec.vlrs.push(vlr("LASF_Projection", 34735, geokeys(keys)));
        (rd(&build(&spec)), spec)
    }

    #[test]
    fn vertical_units_key_applies_to_z_only() {
        const FT: f64 = 304.8;
        const USFT: f64 = 304.800_609_601_219_2;
        // Metre horizontal, foot vertical: Z alone is scaled by the foot.
        let (g, spec) = with_keys(&[(1024, 1), (3076, 9001), (4099, 9002)]);
        let g = g.unwrap();
        for i in 0..4 {
            assert!((g.points[i] - mm3(&spec, i, [1000.0, 1000.0, FT])).abs().max_element() < 1e-6, "point {i}");
        }
        // The other way round, and with the US survey foot.
        let (g, spec) = with_keys(&[(1024, 1), (3076, 9002), (4099, 9001)]);
        assert!((g.unwrap().points[2] - mm3(&spec, 2, [FT, FT, 1000.0])).abs().max_element() < 1e-6);
        let (g, spec) = with_keys(&[(1024, 1), (3076, 9001), (4099, 9003)]);
        assert!((g.unwrap().points[2] - mm3(&spec, 2, [1000.0, 1000.0, USFT])).abs().max_element() < 1e-6);
        // No vertical key: Z follows the horizontal unit, as before.
        let (g, spec) = with_keys(&[(1024, 1), (3076, 9002)]);
        assert!((g.unwrap().points[2] - mm3(&spec, 2, [FT, FT, FT])).abs().max_element() < 1e-6);
        // A vertical key beside an EPSG-only horizontal CRS still sets Z.
        let (g, spec) = with_keys(&[(1024, 1), (3072, 32633), (4099, 9002)]);
        let g = g.unwrap();
        assert!((g.points[2] - mm3(&spec, 2, [1000.0, 1000.0, FT])).abs().max_element() < 1e-6);
        assert_eq!(source(&g), "assumed_metres");
        // An unknown vertical unit code is an error, not a guess.
        let (g, _) = with_keys(&[(1024, 1), (3076, 9001), (4099, 9005)]);
        let msg = g.unwrap_err().to_string();
        assert!(msg.contains("vertical") && msg.contains("9005"), "{msg}");
        // A named unit overrides both keys.
        let (_, spec) = with_keys(&[(1024, 1), (3076, 9001), (4099, 9002)]);
        let g = read(&build(&spec), "t.las", Some(FileUnit::Centimetre)).unwrap();
        assert!((g.points[2] - mm3(&spec, 2, [10.0, 10.0, 10.0])).abs().max_element() < 1e-6);
    }

    #[test]
    fn the_unit_source_is_recorded_on_the_geometry() {
        let spec = Spec::new(2, 0, points(3));
        // No coordinate-system record at all.
        assert_eq!(source(&rd(&build(&spec)).unwrap()), "assumed_metres");
        // A unit named by the caller.
        let named = read(&build(&spec), "t.las", Some(FileUnit::Inch)).unwrap();
        assert_eq!(source(&named), "parameter");
        // Linear unit key, with or without a vertical key.
        assert_eq!(source(&with_keys(&[(1024, 1), (3076, 9002)]).0.unwrap()), "header");
        assert_eq!(source(&with_keys(&[(1024, 1), (3076, 9001), (4099, 9002)]).0.unwrap()), "header");
        // An EPSG code (ProjectedCSTypeGeoKey) without any unit key.
        assert_eq!(source(&with_keys(&[(1024, 1), (3072, 2263)]).0.unwrap()), "assumed_metres");
        // Model type only.
        assert_eq!(source(&with_keys(&[(1024, 1)]).0.unwrap()), "assumed_metres");
        // WKT that states a unit, and WKT that does not.
        let wkt = |text: &str| {
            let mut spec = Spec::new(4, 6, points(3));
            spec.vlrs.push(vlr("LASF_Projection", 2112, text.as_bytes().to_vec()));
            rd(&build(&spec)).unwrap()
        };
        assert_eq!(source(&wkt("PROJCS[\"p\",UNIT[\"US survey foot\",0.3048006096012192]]")), "header");
        assert_eq!(source(&wkt("PROJCS[\"p\",PROJECTION[\"x\"]]")), "assumed_metres");
        // A geographic CRS read with a named unit is "parameter".
        let (_, geographic) = with_keys(&[(1024, 2), (2048, 4326)]);
        let g = read(&build(&geographic), "t.las", Some(FileUnit::Metre)).unwrap();
        assert_eq!(source(&g), "parameter");
        // The attribute is one value (a detail attribute) and the geometry is valid.
        assert_eq!(g.detail_attribs.strs("las_unit_source").unwrap().len(), 1);
        g.validate().unwrap();
    }

    #[test]
    fn wkt_gives_the_unit() {
        let feet = "PROJCS[\"NAD83 / Test (ftUS)\",GEOGCS[\"NAD83\",DATUM[\"D\",SPHEROID[\"GRS 1980\",6378137,298.257]],PRIMEM[\"Greenwich\",0],UNIT[\"degree\",0.0174532925199433]],PROJECTION[\"Transverse_Mercator\"],PARAMETER[\"false_easting\",1968500],UNIT[\"US survey foot\",0.3048006096012192],AXIS[\"X\",EAST],AXIS[\"Y\",NORTH]]";
        let metre = "PROJCS[\"UTM\",GEOGCS[\"WGS 84\",UNIT[\"degree\",0.0174532925199433]],PROJECTION[\"Transverse_Mercator\"],UNIT[\"metre\",1]]";
        let compound = "COMPD_CS[\"x\",PROJCS[\"UTM\",GEOGCS[\"WGS 84\",UNIT[\"degree\",0.0174532925199433]],UNIT[\"metre\",1]],VERT_CS[\"v\",VERT_DATUM[\"d\",2005],UNIT[\"foot\",0.3048]]]";
        let wkt2 = "PROJCRS[\"p\",BASEGEOGCRS[\"b\",ANGLEUNIT[\"degree\",0.0174532925199433]],CONVERSION[\"c\",PARAMETER[\"f\",0,LENGTHUNIT[\"metre\",1]]],CS[Cartesian,2],AXIS[\"E\",east,LENGTHUNIT[\"foot\",0.3048]],AXIS[\"N\",north,LENGTHUNIT[\"foot\",0.3048]],LENGTHUNIT[\"foot\",0.3048]]";
        let geographic = "GEOGCS[\"WGS 84\",DATUM[\"WGS_1984\"],UNIT[\"degree\",0.0174532925199433]]";
        let broken = "PROJCS[\"p\",UNIT[\"metre\",oops]]";
        let run = |text: &str, wkt_flag: bool| {
            let mut spec = Spec::new(4, 6, points(3));
            spec.global_encoding = if wkt_flag { 1 << 4 } else { 0 };
            let mut body = text.as_bytes().to_vec();
            body.push(0);
            spec.vlrs.push(vlr("LASF_Projection", 2112, body));
            (rd(&build(&spec)), spec)
        };
        let (g, spec) = run(feet, true);
        assert!((g.unwrap().points[1] - mm(&spec, 1, 304.800_609_601_219_2)).abs().max_element() < 1e-6);
        let (g, spec) = run(metre, true);
        assert!((g.unwrap().points[1] - mm(&spec, 1, 1000.0)).abs().max_element() < 1e-6);
        let (g, spec) = run(compound, true);
        assert!((g.unwrap().points[1] - mm(&spec, 1, 1000.0)).abs().max_element() < 1e-6, "vertical unit must not leak in");
        let (g, spec) = run(wkt2, true);
        assert!((g.unwrap().points[1] - mm(&spec, 1, 304.8)).abs().max_element() < 1e-6);
        assert!(run(geographic, true).0.unwrap_err().to_string().contains("geographic"));
        assert!(run(broken, true).0.unwrap_err().to_string().contains("WKT"));
    }

    #[test]
    fn wkt_wins_when_flagged_and_geotiff_otherwise() {
        let mut spec = Spec::new(4, 6, points(2));
        spec.vlrs.push(vlr("LASF_Projection", 34735, geokeys(&[(1024, 1), (3076, 9002)])));
        spec.vlrs.push(vlr("LASF_Projection", 2112, b"PROJCS[\"p\",UNIT[\"metre\",1]]".to_vec()));
        let g = rd(&build(&spec)).unwrap();
        assert!((g.points[1] - mm(&spec, 1, 304.8)).abs().max_element() < 1e-6);
        spec.global_encoding = 1 << 4;
        let g = rd(&build(&spec)).unwrap();
        assert!((g.points[1] - mm(&spec, 1, 1000.0)).abs().max_element() < 1e-6);
    }

    #[test]
    fn a_record_that_states_the_horizontal_unit_beats_one_that_does_not() {
        // The GeoTIFF directory only has a vertical unit; the WKT states the plane unit.
        let mut spec = Spec::new(4, 6, points(2));
        spec.vlrs.push(vlr("LASF_Projection", 34735, geokeys(&[(1024, 1), (3072, 2263), (4099, 9001)])));
        spec.vlrs.push(vlr("LASF_Projection", 2112, b"PROJCS[\"p\",UNIT[\"foot\",0.3048]]".to_vec()));
        let g = rd(&build(&spec)).unwrap();
        assert_eq!(source(&g), "header");
        assert!((g.points[1] - mm(&spec, 1, 304.8)).abs().max_element() < 1e-6);
    }

    #[test]
    fn laz_is_detected_and_refused() {
        let mut bytes = build(&Spec::new(2, 1, points(3)));
        bytes[104] |= 0x80;
        assert!(err_text(&bytes).contains("LAZ"));
        let mut spec = Spec::new(4, 6, points(3));
        spec.vlrs.push(vlr("laszip encoded", 22204, vec![0; 34]));
        assert!(err_text(&build(&spec)).contains("LAZ"));
    }

    #[test]
    fn header_problems_are_explicit_errors() {
        let good = build(&Spec::new(4, 7, points(5)));
        assert!(rd(&good).is_ok());
        let patched = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut b = good.clone();
            f(&mut b);
            err_text(&b)
        };
        assert!(err_text(b"").contains("LASF"));
        assert!(err_text(b"LASF").contains("shorter"));
        assert!(patched(&|b| b[0] = b'X').contains("LASF"));
        assert!(patched(&|b| b[25] = 1).contains("version 1.1"));
        assert!(patched(&|b| b[25] = 5).contains("version 1.5"));
        assert!(patched(&|b| b[24] = 2).contains("version 2.4"));
        assert!(patched(&|b| b[94..96].copy_from_slice(&300u16.to_le_bytes())).contains("header size"));
        assert!(patched(&|b| b[96..100].copy_from_slice(&10u32.to_le_bytes())).contains("point data offset"));
        assert!(patched(&|b| b[96..100].copy_from_slice(&u32::MAX.to_le_bytes())).contains("point data offset"));
        assert!(patched(&|b| b[104] = 11).contains("does not exist"));
        assert!(patched(&|b| b[105..107].copy_from_slice(&10u16.to_le_bytes())).contains("shorter"));
        assert!(patched(&|b| b[247..255].copy_from_slice(&u64::MAX.to_le_bytes())).contains("truncated"));
        assert!(patched(&|b| b[131..139].copy_from_slice(&0.0f64.to_le_bytes())).contains("scale"));
        assert!(patched(&|b| b[155..163].copy_from_slice(&f64::NAN.to_le_bytes())).contains("scale or offset"));
        assert!(patched(&|b| b[100..104].copy_from_slice(&u32::MAX.to_le_bytes())).contains("variable length"));
        // A format newer than the version.
        let old = build(&Spec::new(2, 0, points(2)));
        let mut b = old.clone();
        b[104] = 6;
        assert!(err_text(&b).contains("does not exist in LAS 1.2"));
    }

    #[test]
    fn truncation_at_every_length_is_an_error_not_a_panic() {
        let mut spec = Spec::new(4, 8, points(7));
        spec.vlrs.push(vlr("LASF_Projection", 34735, geokeys(&[(1024, 1), (3076, 9002)])));
        let good = build(&spec);
        assert!(rd(&good).is_ok());
        for len in 0..good.len() {
            assert!(rd(&good[..len]).is_err(), "length {len}");
        }
    }

    #[test]
    fn flipped_header_and_vlr_bytes_never_panic() {
        let mut spec = Spec::new(4, 8, points(4));
        spec.vlrs.push(vlr("LASF_Projection", 34735, geokeys(&[(1024, 1), (3076, 9002)])));
        spec.vlrs.push(vlr("LASF_Projection", 2112, b"PROJCS[\"p\",UNIT[\"metre\",1]]".to_vec()));
        let good = build(&spec);
        let body_start = u32::from_le_bytes([good[96], good[97], good[98], good[99]]) as usize;
        for i in 0..body_start {
            for mask in [0xFFu8, 0x80, 0x01] {
                let mut bad = good.clone();
                bad[i] ^= mask;
                let _ = rd(&bad);
                let _ = read(&bad, "t.las", Some(FileUnit::Metre));
            }
        }
    }
}

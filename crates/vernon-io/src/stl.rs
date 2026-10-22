//! STL: binary or ASCII on read (detected), binary or ASCII on write.

use std::io::Write;

use vernon_core::{DVec3, Geometry, Prim};

use crate::{parse_err, IoError, IoResult};

pub(crate) fn read(bytes: &[u8], file: &str) -> IoResult<Geometry> {
    match detect(bytes) {
        Kind::Binary => read_binary(bytes, file),
        Kind::Ascii => {
            if !starts_with_solid(bytes) {
                return Err(parse_err(
                    file,
                    0,
                    "not an STL file: no 'solid' keyword and too short for a binary header",
                ));
            }
            let g = read_ascii(bytes, file)?;
            // Binary files whose header happens to start with "solid" but carry
            // trailing bytes: fall back to binary if ASCII found nothing.
            if g.prims.is_empty() && binary_count(bytes).is_some_and(|n| n > 0) {
                return read_binary(bytes, file);
            }
            Ok(g)
        }
    }
}

#[derive(Debug, PartialEq)]
enum Kind {
    Ascii,
    Binary,
}

/// Triangle count from a binary header, if the file is large enough to hold it.
fn binary_count(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < 84 {
        return None;
    }
    let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as u64;
    (84 + 50 * n <= bytes.len() as u64).then_some(n)
}

fn detect(bytes: &[u8]) -> Kind {
    if bytes.len() >= 84 {
        let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as u64;
        if 84 + 50 * n == bytes.len() as u64 {
            return Kind::Binary;
        }
    }
    if starts_with_solid(bytes) || bytes.len() < 84 {
        Kind::Ascii
    } else {
        Kind::Binary
    }
}

fn starts_with_solid(bytes: &[u8]) -> bool {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    bytes[start..]
        .get(..5)
        .is_some_and(|h| h.eq_ignore_ascii_case(b"solid"))
}

fn read_binary(bytes: &[u8], file: &str) -> IoResult<Geometry> {
    if bytes.len() < 84 {
        return Err(parse_err(
            file,
            0,
            "binary STL shorter than its 84-byte header",
        ));
    }
    let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
    let need = 84u64 + 50 * n as u64;
    if (bytes.len() as u64) < need {
        return Err(parse_err(
            file,
            0,
            format!(
                "truncated binary STL: header declares {n} triangles ({need} bytes), file has {}",
                bytes.len()
            ),
        ));
    }
    let f =
        |o: usize| f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]) as f64;
    let mut points = Vec::with_capacity(3 * n);
    let mut prims = Vec::with_capacity(n);
    for t in 0..n {
        let o = 84 + 50 * t + 12; // skip the facet normal
        for k in 0..3 {
            let v = o + 12 * k;
            points.push(DVec3::new(f(v), f(v + 4), f(v + 8)));
        }
        let b = (3 * t) as u32;
        prims.push(Prim::polygon(vec![b, b + 1, b + 2]));
    }
    Ok(Geometry {
        points,
        prims,
        ..Default::default()
    })
}

fn read_ascii(bytes: &[u8], file: &str) -> IoResult<Geometry> {
    let text = String::from_utf8_lossy(bytes);
    let mut points = Vec::new();
    let mut prims = Vec::new();
    let mut cur: Vec<u32> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let mut toks = line.split_ascii_whitespace();
        let Some(kw) = toks.next() else { continue };
        match kw.to_ascii_lowercase().as_str() {
            "vertex" => {
                let mut c = [0.0f64; 3];
                for v in &mut c {
                    let t = toks
                        .next()
                        .ok_or_else(|| parse_err(file, i + 1, "vertex needs 3 coordinates"))?;
                    *v = t
                        .parse()
                        .map_err(|_| parse_err(file, i + 1, format!("bad number '{t}'")))?;
                }
                cur.push(points.len() as u32);
                points.push(DVec3::from_array(c));
            }
            "facet" | "outer" => cur.clear(),
            "endloop" | "endfacet" => {
                if cur.len() >= 3 {
                    prims.push(Prim::polygon(std::mem::take(&mut cur)));
                }
                cur.clear();
            }
            _ => {}
        }
    }
    Ok(Geometry {
        points,
        prims,
        ..Default::default()
    })
}

fn facet_normal(a: DVec3, b: DVec3, c: DVec3) -> DVec3 {
    (b - a).cross(c - a).normalize_or_zero()
}

pub(crate) fn write<W: Write>(w: &mut W, geo: &Geometry, scale: f64, ascii: bool) -> IoResult<()> {
    let tris = crate::tri::triangulate(geo);
    if tris.is_empty() {
        return Err(IoError::Other(
            "STL needs closed polygons; the geometry has none".into(),
        ));
    }
    let p = |i: u32| geo.points[i as usize] * scale;
    if ascii {
        writeln!(w, "solid vernon")?;
        for (_, [a, b, c]) in &tris {
            let (a, b, c) = (p(*a), p(*b), p(*c));
            let n = facet_normal(a, b, c);
            writeln!(w, "  facet normal {} {} {}", n.x, n.y, n.z)?;
            writeln!(w, "    outer loop")?;
            for v in [a, b, c] {
                writeln!(w, "      vertex {} {} {}", v.x, v.y, v.z)?;
            }
            writeln!(w, "    endloop")?;
            writeln!(w, "  endfacet")?;
        }
        writeln!(w, "endsolid vernon")?;
        return Ok(());
    }
    let count = u32::try_from(tris.len())
        .map_err(|_| IoError::Other("too many triangles for binary STL".into()))?;
    // The header must not start with "solid" or readers may take it for ASCII.
    let mut header = [b' '; 80];
    let text = b"Vernon binary STL";
    header[..text.len()].copy_from_slice(text);
    w.write_all(&header)?;
    w.write_all(&count.to_le_bytes())?;
    let mut rec = [0u8; 50];
    for (_, [a, b, c]) in &tris {
        let (a, b, c) = (p(*a), p(*b), p(*c));
        let n = facet_normal(a, b, c);
        for (k, v) in [n, a, b, c].iter().enumerate() {
            for (j, x) in [v.x, v.y, v.z].iter().enumerate() {
                let o = 12 * k + 4 * j;
                rec[o..o + 4].copy_from_slice(&(*x as f32).to_le_bytes());
            }
        }
        // rec[48..50] attribute byte count stays 0
        w.write_all(&rec)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;
    use crate::{read_bytes, write_bytes, Format, ReadOptions, WriteOptions};

    fn no_weld() -> ReadOptions {
        ReadOptions {
            weld: false,
            ..Default::default()
        }
    }

    #[test]
    fn concave_quad_is_written_without_flipped_triangles() {
        // A dart (arrowhead) whose first vertex cannot see the whole polygon:
        // a plain fan from vertex 0 would flip a triangle.
        let pts = vec![
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(4.0, 0.0, 0.0),
            DVec3::new(1.0, 1.0, 0.0),
            DVec3::new(0.0, 4.0, 0.0),
        ];
        let g = Geometry::from_polygons(pts, vec![vec![1, 2, 3, 0]]);
        let bytes = write_bytes(Format::Stl, &g, &WriteOptions::default()).unwrap();
        let back = read_bytes(Format::Stl, &bytes, &ReadOptions::default()).unwrap();
        assert_eq!(back.prim_count(), 2);
        let mut area = 0.0;
        for t in back.triangles() {
            let [a, b, c] = t.1.map(|i| back.points[i as usize]);
            let n = (b - a).cross(c - a);
            assert!(n.z > 0.0, "flipped triangle {t:?}");
            area += n.z / 2.0;
        }
        assert!((area - 4.0).abs() < 1e-9, "{area}");
    }

    #[test]
    fn binary_round_trip_and_weld() {
        let g = Geometry::make_box(DVec3::new(10.0, 20.0, 30.0));
        let bytes = write_bytes(Format::Stl, &g, &WriteOptions::default()).unwrap();
        assert_eq!(bytes.len(), 84 + 50 * 12);
        assert!(!bytes.starts_with(b"solid"));
        assert_eq!(detect(&bytes), Kind::Binary);

        let soup = read_bytes(Format::Stl, &bytes, &no_weld()).unwrap();
        assert_eq!(soup.point_count(), 36);
        assert_eq!(soup.prim_count(), 12);

        let welded = read_bytes(Format::Stl, &bytes, &ReadOptions::default()).unwrap();
        assert_eq!(welded.point_count(), 8);
        assert_eq!(welded.prim_count(), 12);
        assert_eq!(sorted_points(&welded), sorted_points(&g));
        assert!((welded.surface_area() - g.surface_area()).abs() < 1e-6);
        // Winding survives: the stored facet normal agrees with the vertex order.
        let n = &bytes[84..96];
        let nz = f32::from_le_bytes([n[8], n[9], n[10], n[11]]);
        assert_eq!(nz, -1.0); // first face of make_box is the -Z bottom
    }

    #[test]
    fn ascii_round_trip_and_detection() {
        let g = Geometry::make_box(DVec3::new(1.0, 2.0, 3.0));
        let opts = WriteOptions {
            ascii: true,
            ..Default::default()
        };
        let bytes = write_bytes(Format::Stl, &g, &opts).unwrap();
        assert!(bytes.starts_with(b"solid"));
        assert_eq!(detect(&bytes), Kind::Ascii);
        let back = read_bytes(Format::Stl, &bytes, &ReadOptions::default()).unwrap();
        assert_eq!(back.point_count(), 8);
        assert_eq!(back.prim_count(), 12);
        assert_eq!(sorted_points(&back), sorted_points(&g));
    }

    #[test]
    fn binary_with_solid_header_is_binary() {
        let g = Geometry::make_box(DVec3::ONE);
        let mut bytes = write_bytes(Format::Stl, &g, &WriteOptions::default()).unwrap();
        bytes[..5].copy_from_slice(b"solid");
        assert_eq!(detect(&bytes), Kind::Binary);
        // Trailing junk breaks the exact-size test; the fallback still reads it.
        bytes.extend_from_slice(b"\0\0\0");
        let back = read_bytes(Format::Stl, &bytes, &ReadOptions::default()).unwrap();
        assert_eq!(back.prim_count(), 12);
        assert_eq!(back.point_count(), 8);
    }

    #[test]
    fn weld_tolerance_is_respected() {
        let src = "solid t
facet normal 0 0 1
 outer loop
  vertex 0 0 0
  vertex 1 0 0
  vertex 0 1 0
 endloop
endfacet
facet normal 0 0 1
 outer loop
  vertex 1.0001 0 0
  vertex 1 1 0
  vertex 0 1.0001 0
 endloop
endfacet
endsolid t
";
        let coarse = ReadOptions {
            weld_tolerance: 1e-3,
            ..Default::default()
        };
        let g = read_bytes(Format::Stl, src.as_bytes(), &coarse).unwrap();
        assert_eq!(g.point_count(), 4);
        let fine = read_bytes(Format::Stl, src.as_bytes(), &ReadOptions::default()).unwrap();
        assert_eq!(fine.point_count(), 6);
    }

    #[test]
    fn truncated_binary_is_an_error() {
        let g = Geometry::make_box(DVec3::ONE);
        let bytes = write_bytes(Format::Stl, &g, &WriteOptions::default()).unwrap();
        let cut = &bytes[..bytes.len() - 60];
        assert!(read_bytes(Format::Stl, cut, &ReadOptions::default()).is_err());
    }

    #[test]
    fn point_cloud_cannot_be_written() {
        let g = Geometry::from_points(vec![DVec3::ZERO]);
        assert!(write_bytes(Format::Stl, &g, &WriteOptions::default()).is_err());
    }
}

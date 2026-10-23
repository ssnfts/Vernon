//! 3MF package writing and reading on top of `vernon_io`, with the checks the
//! print stage needs: outward winding per part, closed manifold shells and a
//! volume round trip.

use std::path::Path;

use vernon_core::Geometry;
use vernon_io::{FileUnit, ReadOptions, WriteOptions};

use crate::mesh;

pub fn unit_of(s: &str) -> FileUnit {
    match s {
        "centimeter" => FileUnit::Centimetre,
        "meter" => FileUnit::Metre,
        "inch" => FileUnit::Inch,
        _ => FileUnit::Millimetre,
    }
}

pub const UNIT_IDS: [&str; 4] = ["millimeter", "centimeter", "meter", "inch"];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PackageStats {
    pub parts: usize,
    pub triangles: usize,
    pub volume_mm3: f64,
    pub open_edges: usize,
    pub non_manifold_edges: usize,
    /// Parts whose winding was reversed to point outward.
    pub flipped_parts: usize,
    pub bytes: u64,
    /// Relative volume difference after writing and reading back (written files only).
    pub roundtrip_error: f64,
}

/// Keep the closed faces, make every part wind outward, count edge defects.
pub fn prepare(geo: &Geometry) -> (Geometry, PackageStats) {
    let ids = mesh::part_ids(geo);
    let mut stats = PackageStats::default();
    let mut parts = Vec::new();
    for id in mesh::unique_parts(geo) {
        let prims: Vec<usize> = (0..geo.prims.len()).filter(|&i| ids[i] == id && geo.prims[i].closed && geo.prims[i].vertices.len() >= 3).collect();
        let mut g = mesh::extract_prims(geo, &prims);
        let e = mesh::edge_stats(&g);
        stats.open_edges += e.open;
        stats.non_manifold_edges += e.non_manifold;
        if e.open == 0 && mesh::signed_volume(&g) < 0.0 {
            mesh::flip_all(&mut g);
            stats.flipped_parts += 1;
        }
        stats.volume_mm3 += mesh::signed_volume(&g);
        stats.triangles += g.triangles().len();
        stats.parts += 1;
        parts.push(g);
    }
    (mesh::merge_all(&parts), stats)
}

/// Creation date written into reproducible packages: the Unix epoch, the
/// usual "no date" value, so equal inputs give equal bytes.
pub const REPRODUCIBLE_DATE: i64 = 0;

/// Write a package with a fixed creation date (identical bytes on every
/// cook). With `strict`, open or non-manifold shells are refused.
pub fn write_package(geo: &Geometry, path: &Path, unit: FileUnit, strict: bool) -> Result<PackageStats, String> {
    write_package_dated(geo, path, unit, strict, Some(REPRODUCIBLE_DATE))
}

/// [`write_package`] with an explicit creation date in Unix seconds; `None`
/// stamps the current time (or `SOURCE_DATE_EPOCH`).
pub fn write_package_dated(geo: &Geometry, path: &Path, unit: FileUnit, strict: bool, creation_date: Option<i64>) -> Result<PackageStats, String> {
    let (clean, mut stats) = prepare(geo);
    if stats.parts == 0 {
        return Err("there are no closed faces to write".into());
    }
    if strict && (stats.open_edges > 0 || stats.non_manifold_edges > 0) {
        return Err(format!(
            "the parts are not closed manifold shells ({} open edges, {} non-manifold edges); repair them or switch strict mode off",
            stats.open_edges, stats.non_manifold_edges
        ));
    }
    vernon_io::write(path, &clean, &WriteOptions { unit, ascii: false, creation_date }).map_err(|e| e.to_string())?;
    stats.bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let back = vernon_io::read(path, &ReadOptions::default()).map_err(|e| format!("the written package does not read back: {e}"))?;
    let v_back = mesh::signed_volume(&back);
    stats.roundtrip_error = if stats.volume_mm3.abs() > 0.0 { (v_back - stats.volume_mm3).abs() / stats.volume_mm3.abs() } else { 0.0 };
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::testutil::tri_box;
    use glam::{DMat4, DVec3};

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("vernon-print-pkg-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn packages_are_byte_identical_across_writes() {
        let g = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        let d = dir("repro");
        let (a, b) = (d.join("a.3mf"), d.join("b.3mf"));
        write_package(&g, &a, FileUnit::Millimetre, true).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write_package(&g, &b, FileUnit::Millimetre, true).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    }

    #[test]
    fn cube_round_trip_golden() {
        let g = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        let p = dir("cube").join("cube.3mf");
        let s = write_package(&g, &p, FileUnit::Millimetre, true).unwrap();
        assert_eq!((s.parts, s.triangles, s.open_edges), (1, 12, 0));
        assert!((s.volume_mm3 - 1000.0).abs() < 1e-9);
        assert!(s.roundtrip_error < 1e-9);
        let back = vernon_io::read(&p, &ReadOptions::default()).unwrap();
        assert_eq!(back.points.len(), 8);
        assert_eq!(back.triangles().len(), 12);
        assert!((mesh::signed_volume(&back) - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn inch_unit_reads_back_in_millimetres() {
        let g = tri_box(DVec3::ZERO, DVec3::splat(1.0));
        let p = dir("inch").join("cube_inch.3mf");
        let s = write_package(&g, &p, FileUnit::Inch, true).unwrap();
        assert!(s.roundtrip_error < 1e-9, "{}", s.roundtrip_error);
        let back = vernon_io::read(&p, &ReadOptions::default()).unwrap();
        // the file stores 1 / 25.4 per mm; reading converts to a 1 mm cube -> 1 mm3
        assert!((mesh::signed_volume(&back) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn mirrored_part_is_wound_outward() {
        let mut g = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        g.transform(DMat4::from_scale(DVec3::new(-1.0, 1.0, 1.0)));
        // mirrored without re-winding: signed volume is negative
        assert!(mesh::signed_volume(&g) < 0.0);
        let p = dir("mirror").join("m.3mf");
        let s = write_package(&g, &p, FileUnit::Millimetre, true).unwrap();
        assert_eq!(s.flipped_parts, 1);
        assert!((s.volume_mm3 - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn open_shell_is_refused_in_strict_mode() {
        let mut g = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        g.prims.pop();
        let p = dir("open").join("o.3mf");
        assert!(write_package(&g, &p, FileUnit::Millimetre, true).is_err());
        assert!(write_package(&g, &p, FileUnit::Millimetre, false).is_ok());
    }

    #[test]
    fn parts_become_objects_and_keep_ids() {
        let mut a = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        let mut b = tri_box(DVec3::new(20.0, 0.0, 0.0), DVec3::splat(5.0));
        let (na, nb) = (a.prims.len(), b.prims.len());
        mesh::set_int_prim_attr(&mut a, "part", vec![0; na]);
        mesh::set_int_prim_attr(&mut b, "part", vec![1; nb]);
        a.merge(&b);
        let p = dir("two").join("two.3mf");
        let s = write_package(&a, &p, FileUnit::Millimetre, true).unwrap();
        assert_eq!(s.parts, 2);
        let back = vernon_io::read(&p, &ReadOptions::default()).unwrap();
        assert_eq!(mesh::unique_parts(&back).len(), 2);
        assert!((mesh::signed_volume(&mesh::extract_part(&back, 1)) - 125.0).abs() < 1e-6);
    }
}

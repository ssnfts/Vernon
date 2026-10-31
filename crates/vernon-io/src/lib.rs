//! Vernon file I/O.
//!
//! Every reader returns a [`Geometry`] in millimetres; every writer takes one.
//! The format is picked from the file extension by [`read()`] and [`write()`].
//!
//! CONTRACT (other crates compile against these signatures; keep them stable):
//! - `read(path, &ReadOptions) -> IoResult<Geometry>`
//! - `write(path, &Geometry, &WriteOptions) -> IoResult<()>`
//! - `Format::from_path(path) -> Option<Format>`
//!
//! Additional entry points: [`read_bytes`] / [`write_bytes`] work on in-memory
//! buffers, and [`weld_points`] is the hash-grid vertex weld used by the STL
//! reader.
//!
//! Attribute mapping (see `vernon_core::Geometry` for the standard names):
//!
//! | Format | Read | Write |
//! |--------|------|-------|
//! | OBJ | `v` (+ optional `r g b` -> `Cd`), `vn` -> point `N`, `vt` -> point `uv`, `f`/`l` with n-gons and negative indices, `g`/`o` -> prim groups (`part_NN` groups covering every prim -> `part`). Materials ignored. | `v` (+ `Cd`), `vt`, `vn`, `f`, `l`, `g` for prim groups and `part` |
//! | STL | ASCII or binary (detected), weld optional | binary (or ASCII with `WriteOptions::ascii`), polygons ear-clipped (concave-safe) |
//! | PLY | ASCII, binary little/big endian; `x y z`, `nx ny nz` -> `N`, `red green blue` -> `Cd` (0..1), `s t` -> `uv`, other scalars -> Float/Int attribs, face `vertex_indices` | binary little endian (or ASCII); positions as `double` |
//! | 3MF | all build items, components and transforms flattened; model `unit` honoured; prim `part` = build item index | one object per distinct prim `part` value |
//! | XYZ/PTS | `x y z [nx ny nz] [r g b]`, PTS `x y z i [r g b]`; headers and comments skipped | XYZ: `x y z [nx ny nz] [r g b]`; PTS: count line then `x y z i [r g b]` |
//! | E57 | every `data3D` scan: `cartesianX/Y/Z`, `colorRed/Green/Blue` -> `Cd` (0..1), `intensity`; scan pose applied; metres -> mm; page checksums verified | read only |
//! | LAS 1.2-1.4 | point formats 0-10: scale and offset applied, `intensity`, `classification`, colour -> `Cd` (0..1); unit from the CRS records (horizontal and vertical units apart) or [`read_las_bytes`], metres by default; the detail attribute `las_unit_source` says which (`parameter`, `header` or `assumed_metres`, see [`las_unit_was_assumed`]); LAZ refused | read only |
//!
//! E57 and LAS carry their own length units (E57 always metres), so
//! [`ReadOptions::unit`] does not apply to them, as with 3MF. Reading a
//! `.laz` path reports that LAZ is unsupported. E57 refuses a kept point
//! whose `intensity` or `Cd` value is NaN or infinite.
//!
//! Open polylines are only written by OBJ; STL, PLY and 3MF write closed
//! polygons only.
//!
//! Safety rules shared by all formats: readers reject non-finite coordinates
//! (after unit conversion) and never trust declared counts; writers refuse
//! non-finite points and write through a temporary file that is renamed over
//! the target, so a failed write leaves an existing file untouched. STL weld
//! tolerances are floored at `f32` resolution (an exact weld, tolerance 0, is
//! honoured). Z is up and axes are never swapped.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use vernon_core::Geometry;

mod bin;
mod e57;
mod las;
mod obj;
mod ply;
mod stl;
mod threemf;
mod tri;
mod weld;
mod xml;
mod xyz;
mod zip;

pub use weld::weld_points;

/// Supported file formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Obj,
    /// Binary or ASCII STL, detected on read; binary on write.
    Stl,
    /// ASCII or binary little-endian PLY, detected on read; binary on write.
    Ply,
    /// 3D Manufacturing Format (zip + XML). Mesh objects only.
    ThreeMf,
    /// Plain-text point cloud: one `x y z [nx ny nz] [r g b]` per line.
    Xyz,
    /// ASTM E2807 point-cloud container. Read only.
    E57,
    /// LAS 1.2 to 1.4 airborne/terrestrial point cloud. Read only; LAZ is refused.
    Las,
}

impl Format {
    pub fn from_path(path: &Path) -> Option<Format> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "obj" => Format::Obj,
            "stl" => Format::Stl,
            "ply" => Format::Ply,
            "3mf" => Format::ThreeMf,
            "xyz" | "pts" | "txt" => Format::Xyz,
            "e57" => Format::E57,
            // `.laz` is accepted so that reading it says LAZ is unsupported.
            "las" | "laz" => Format::Las,
            _ => return None,
        })
    }

    /// Identify a file image from its leading bytes. Only formats with a
    /// signature are recognised (PLY, 3MF/zip, E57, LAS, STL ASCII or binary);
    /// OBJ and XYZ/PTS are plain text and need their extension.
    pub fn sniff(bytes: &[u8]) -> Option<Format> {
        if bytes.starts_with(b"ASTM-E57") {
            return Some(Format::E57);
        }
        if bytes.starts_with(b"LASF") {
            return Some(Format::Las);
        }
        if bytes.starts_with(b"ply") && matches!(bytes.get(3), Some(b'\n' | b'\r')) {
            return Some(Format::Ply);
        }
        if bytes.starts_with(b"PK\x03\x04") {
            return Some(Format::ThreeMf);
        }
        // Binary STL: 80-byte header, u32 count, 50 bytes per facet. The size
        // test comes first because binary headers may also start with "solid".
        if let Some(n) = bytes.get(80..84).and_then(|b| <[u8; 4]>::try_from(b).ok()) {
            let n = u32::from_le_bytes(n) as usize;
            if n.checked_mul(50).and_then(|b| b.checked_add(84)) == Some(bytes.len()) {
                return Some(Format::Stl);
            }
        }
        let head = &bytes[..bytes.len().min(1024)];
        if head.starts_with(b"solid") && head.windows(5).any(|w| w == b"facet") {
            return Some(Format::Stl);
        }
        None
    }

    /// Extensions [`read()`] accepts.
    pub fn extensions() -> &'static [&'static str] {
        &["obj", "stl", "ply", "3mf", "xyz", "pts", "e57", "las"]
    }

    /// Extensions [`write()`] accepts: [`Format::extensions`] without the
    /// read-only point-cloud containers.
    pub fn write_extensions() -> &'static [&'static str] {
        &["obj", "stl", "ply", "3mf", "xyz", "pts"]
    }

    /// Whether this format can be written (E57 and LAS are read only).
    pub fn can_write(self) -> bool {
        !matches!(self, Format::E57 | Format::Las)
    }
}

/// Unit of the numbers stored in a file; converted to millimetres on read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileUnit {
    Millimetre,
    Centimetre,
    Metre,
    Inch,
}

impl FileUnit {
    pub fn to_mm(self) -> f64 {
        match self {
            FileUnit::Millimetre => 1.0,
            FileUnit::Centimetre => 10.0,
            FileUnit::Metre => 1000.0,
            FileUnit::Inch => 25.4,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReadOptions {
    /// Unit the file is in. 3MF declares its own unit and ignores this.
    pub unit: FileUnit,
    /// Merge coincident vertices (STL stores each triangle separately).
    /// Applied to STL only; the other formats store shared vertices already.
    pub weld: bool,
    /// Weld tolerance in mm.
    pub weld_tolerance: f64,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            unit: FileUnit::Millimetre,
            weld: true,
            weld_tolerance: 1e-6,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WriteOptions {
    pub unit: FileUnit,
    /// Write ASCII instead of binary where the format allows it.
    pub ascii: bool,
    /// 3MF `CreationDate` as seconds since the Unix epoch. `None` takes
    /// `SOURCE_DATE_EPOCH` when set and the current time otherwise; a fixed
    /// value makes the written bytes identical from run to run.
    pub creation_date: Option<i64>,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            unit: FileUnit::Millimetre,
            ascii: false,
            creation_date: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("unsupported file format: {0}")]
    Unsupported(String),
    #[error("parse error in {file} at line {line}: {message}")]
    Parse {
        file: String,
        line: usize,
        message: String,
    },
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

pub type IoResult<T> = Result<T, IoError>;

/// Build a parse error. `line` is 1-based for text formats, 0 for binary data.
pub(crate) fn parse_err(file: &str, line: usize, message: impl Into<String>) -> IoError {
    IoError::Parse {
        file: file.to_string(),
        line,
        message: message.into(),
    }
}

/// Read any supported file, picking the format from the extension.
pub fn read(path: &Path, opts: &ReadOptions) -> IoResult<Geometry> {
    let format =
        Format::from_path(path).ok_or_else(|| IoError::Unsupported(path.display().to_string()))?;
    let bytes = std::fs::read(path)?;
    read_named(format, &bytes, opts, &path.display().to_string())
}

/// Read a file image held in memory.
pub fn read_bytes(format: Format, bytes: &[u8], opts: &ReadOptions) -> IoResult<Geometry> {
    read_named(format, bytes, opts, "<memory>")
}

/// Read a LAS 1.2 to 1.4 file image held in memory, naming the unit of its
/// coordinates. `None` takes the unit from the file's coordinate-system
/// records and falls back to metres (what [`read_bytes`] does). The result is
/// in millimetres.
pub fn read_las_bytes(bytes: &[u8], unit: Option<FileUnit>) -> IoResult<Geometry> {
    let geo = las::read(bytes, "<memory>", unit)?;
    finish_read(geo, &ReadOptions::default(), Format::Las, "<memory>")
}

/// Whether `geo`, as returned by the LAS reader, took its length unit by
/// assuming metres because the file's coordinate-system records state none
/// (its `las_unit_source` detail attribute is `assumed_metres`). A caller
/// should tell the user, since a foot-based file would be off by 3.28. False
/// for every other geometry.
pub fn las_unit_was_assumed(geo: &Geometry) -> bool {
    geo.detail_attribs
        .strs("las_unit_source")
        .is_ok_and(|v| v.first().map(String::as_str) == Some(las::UNIT_ASSUMED_METRES))
}

fn read_named(format: Format, bytes: &[u8], opts: &ReadOptions, file: &str) -> IoResult<Geometry> {
    let geo = match format {
        Format::Obj => obj::read(bytes, file)?,
        Format::Stl => stl::read(bytes, file)?,
        Format::Ply => ply::read(bytes, file)?,
        Format::ThreeMf => threemf::read(bytes, file)?,
        Format::Xyz => xyz::read(bytes, file)?,
        Format::E57 => e57::read(bytes, file)?,
        Format::Las => las::read(bytes, file, None)?,
    };
    finish_read(geo, opts, format, file)
}

/// Unit conversion, finiteness check, optional STL weld and validation shared
/// by every reader.
fn finish_read(mut geo: Geometry, opts: &ReadOptions, format: Format, file: &str) -> IoResult<Geometry> {
    // 3MF, E57 and LAS state their own unit and arrive in millimetres.
    if !matches!(format, Format::ThreeMf | Format::E57 | Format::Las) {
        let k = opts.unit.to_mm();
        if k != 1.0 {
            for p in &mut geo.points {
                *p *= k;
            }
        }
    }
    if let Some(i) = geo.points.iter().position(|p| !p.is_finite()) {
        return Err(parse_err(
            file,
            0,
            format!("point {i} has a non-finite coordinate"),
        ));
    }
    if opts.weld && format == Format::Stl {
        let tol = stl_weld_tolerance(&geo, opts.weld_tolerance);
        weld_points(&mut geo, tol);
    }
    geo.validate()
        .map_err(|e| parse_err(file, 0, e.to_string()))?;
    Ok(geo)
}

/// STL stores `f32`, so two copies of one vertex may differ by an ulp of the
/// model's largest coordinate. Tolerances below that are floored to it; an
/// exact weld (`tolerance == 0`) is left alone.
fn stl_weld_tolerance(geo: &Geometry, tolerance: f64) -> f64 {
    if tolerance <= 0.0 || !tolerance.is_finite() {
        return tolerance;
    }
    let max_abs = geo
        .points
        .iter()
        .fold(0.0_f64, |m, p| m.max(p.abs().max_element()));
    tolerance.max(F32_ULP_REL * max_abs)
}

/// Worst-case relative spacing of `f32` (2^-23).
const F32_ULP_REL: f64 = 1.192_092_9e-7;

/// Write any supported file, picking the format from the extension.
/// A `.pts` extension writes the Leica PTS layout (count line, `x y z i [r g b]`).
pub fn write(path: &Path, geo: &Geometry, opts: &WriteOptions) -> IoResult<()> {
    let format =
        Format::from_path(path).ok_or_else(|| IoError::Unsupported(path.display().to_string()))?;
    ensure_writable_format(format)?;
    let pts = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pts"));
    check_writable(geo)?;
    write_atomic(path, |w| write_to(w, format, geo, opts, pts))
}

fn ensure_writable_format(format: Format) -> IoResult<()> {
    if format.can_write() {
        Ok(())
    } else {
        Err(IoError::Other(format!(
            "{format:?} files can be read but not written"
        )))
    }
}

/// Preconditions shared by every writer: consistent attribute lengths, valid
/// point references and finite coordinates (a NaN would corrupt the file).
fn check_writable(geo: &Geometry) -> IoResult<()> {
    geo.validate().map_err(|e| IoError::Other(e.to_string()))?;
    if let Some(i) = geo.points.iter().position(|p| !p.is_finite()) {
        return Err(IoError::Other(format!(
            "point {i} has a non-finite coordinate; refusing to write it"
        )));
    }
    Ok(())
}

/// Stream into a temporary file next to `path`, then rename it over `path`,
/// so a failed write never truncates or half-replaces an existing file.
fn write_atomic(
    path: &Path,
    body: impl FnOnce(&mut BufWriter<File>) -> IoResult<()>,
) -> IoResult<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .ok_or_else(|| IoError::Other(format!("'{}' is not a file path", path.display())))?;
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(
        ".{}.{}-{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| -> IoResult<()> {
        let mut w = BufWriter::new(File::create(&tmp)?);
        body(&mut w)?;
        w.flush()?;
        Ok(())
    })();
    let result = written.and_then(|()| std::fs::rename(&tmp, path).map_err(IoError::from));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Encode a geometry into an in-memory file image (`Format::Xyz` writes the
/// plain XYZ layout).
pub fn write_bytes(format: Format, geo: &Geometry, opts: &WriteOptions) -> IoResult<Vec<u8>> {
    ensure_writable_format(format)?;
    check_writable(geo)?;
    if format == Format::ThreeMf {
        return threemf::write(geo, opts);
    }
    let mut out = Vec::new();
    write_to(&mut out, format, geo, opts, false)?;
    Ok(out)
}

fn write_to<W: Write>(
    w: &mut W,
    format: Format,
    geo: &Geometry,
    opts: &WriteOptions,
    pts: bool,
) -> IoResult<()> {
    let scale = 1.0 / opts.unit.to_mm();
    match format {
        Format::Obj => obj::write(w, geo, scale),
        Format::Stl => stl::write(w, geo, scale, opts.ascii),
        Format::Ply => ply::write(w, geo, scale, opts.ascii),
        Format::Xyz => xyz::write(w, geo, scale, pts),
        Format::E57 | Format::Las => {
            ensure_writable_format(format)?;
            Ok(())
        }
        Format::ThreeMf => {
            let bytes = threemf::write(geo, opts)?;
            w.write_all(&bytes)?;
            Ok(())
        }
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use vernon_core::{DVec3, Geometry};

    pub fn approx(a: DVec3, b: DVec3, tol: f64) -> bool {
        (a - b).abs().max_element() <= tol
    }

    /// Sorted point list, for order-independent comparisons.
    pub fn sorted_points(g: &Geometry) -> Vec<[i64; 3]> {
        let mut v: Vec<[i64; 3]> = g
            .points
            .iter()
            .map(|p| {
                [
                    (p.x * 1e4).round() as i64,
                    (p.y * 1e4).round() as i64,
                    (p.z * 1e4).round() as i64,
                ]
            })
            .collect();
        v.sort();
        v
    }

    /// A unique scratch directory for a test.
    pub fn temp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("vernon-io-{}-{}", name, std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;
    use vernon_core::{AttribArray, DVec3};

    #[test]
    fn format_from_extension() {
        assert_eq!(Format::from_path(Path::new("a/B.OBJ")), Some(Format::Obj));
        assert_eq!(Format::from_path(Path::new("x.3mf")), Some(Format::ThreeMf));
        assert_eq!(Format::from_path(Path::new("x.pts")), Some(Format::Xyz));
        assert_eq!(Format::from_path(Path::new("x.step")), None);
        assert!(matches!(
            read(Path::new("x.step"), &ReadOptions::default()),
            Err(IoError::Unsupported(_))
        ));
    }

    #[test]
    fn every_format_round_trips_through_files() {
        let dir = temp_dir("files");
        let mut g = Geometry::make_box(DVec3::new(10.0, 20.0, 30.0));
        let n = g.prim_count();
        g.prim_attribs.set("part", AttribArray::Int(vec![0; n]));
        for ext in ["obj", "stl", "ply", "3mf"] {
            let path = dir.join(format!("box.{ext}"));
            write(&path, &g, &WriteOptions::default()).unwrap();
            let back = read(&path, &ReadOptions::default()).unwrap();
            assert_eq!(back.point_count(), 8, "{ext}");
            assert!(
                (back.surface_area() - g.surface_area()).abs() < 1e-6,
                "{ext}"
            );
            assert_eq!(sorted_points(&back), sorted_points(&g), "{ext}");
        }
        for ext in ["xyz", "pts"] {
            let path = dir.join(format!("cloud.{ext}"));
            write(&path, &g, &WriteOptions::default()).unwrap();
            let back = read(&path, &ReadOptions::default()).unwrap();
            assert!(back.is_point_cloud(), "{ext}");
            assert_eq!(sorted_points(&back), sorted_points(&g), "{ext}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn units_convert_on_read_and_write() {
        let g = Geometry::make_box(DVec3::splat(254.0));
        let opts = WriteOptions {
            unit: FileUnit::Inch,
            ..Default::default()
        };
        for format in [
            Format::Obj,
            Format::Stl,
            Format::Ply,
            Format::Xyz,
            Format::ThreeMf,
        ] {
            let bytes = write_bytes(format, &g, &opts).unwrap();
            // Read back without conversion: numbers are inches (box is 10 in).
            let raw = read_bytes(format, &bytes, &ReadOptions::default()).unwrap();
            let expect = if format == Format::ThreeMf {
                254.0
            } else {
                10.0
            };
            assert!((raw.bounds().size().x - expect).abs() < 1e-4, "{format:?}");
            // Read back with conversion: millimetres again.
            let ropts = ReadOptions {
                unit: FileUnit::Inch,
                ..Default::default()
            };
            let mm = read_bytes(format, &bytes, &ropts).unwrap();
            assert!((mm.bounds().size().x - 254.0).abs() < 1e-3, "{format:?}");
        }
    }

    #[test]
    fn invalid_geometry_is_rejected_on_write() {
        let mut g = Geometry::make_box(DVec3::ONE);
        g.prims[0].vertices[0] = 99;
        assert!(matches!(
            write_bytes(Format::Obj, &g, &WriteOptions::default()),
            Err(IoError::Other(_))
        ));
    }

    // ------------------------------------------------------------ contract

    const ALL_MESH_FORMATS: [Format; 4] = [Format::Obj, Format::Stl, Format::Ply, Format::ThreeMf];

    fn tetra() -> Geometry {
        Geometry::from_polygons(
            vec![
                DVec3::new(4999.123456789, -1234.987654321, 87.000000001),
                DVec3::new(100_000.5, 3.25, -0.001953125),
                DVec3::new(-77.7, 100_001.123456, 12.0),
                DVec3::new(1.0 / 3.0, -4999.9999, 99_999.0),
            ],
            vec![vec![0, 2, 1], vec![0, 1, 3], vec![1, 2, 3], vec![2, 0, 3]],
        )
    }

    fn sorted_exact(g: &Geometry) -> Vec<[f64; 3]> {
        let mut v: Vec<[f64; 3]> = g.points.iter().map(|p| p.to_array()).collect();
        v.sort_by(|a, b| {
            a[0].total_cmp(&b[0])
                .then(a[1].total_cmp(&b[1]))
                .then(a[2].total_cmp(&b[2]))
        });
        v
    }

    #[test]
    fn car_scale_round_trip_tolerances() {
        let g = tetra();
        let want = sorted_exact(&g);
        for format in [
            Format::Obj,
            Format::Ply,
            Format::ThreeMf,
            Format::Xyz,
            Format::Stl,
        ] {
            let bytes = write_bytes(format, &g, &WriteOptions::default()).unwrap();
            let back = read_bytes(format, &bytes, &ReadOptions::default()).unwrap();
            let got = sorted_exact(&back);
            assert_eq!(got.len(), want.len(), "{format:?}");
            for (a, b) in got.iter().zip(&want) {
                for k in 0..3 {
                    // Text and double formats are exact; STL stores f32 (half ulp).
                    let tol = if format == Format::Stl {
                        b[k].abs() * 6e-8 + 1e-12
                    } else {
                        0.0
                    };
                    assert!(
                        (a[k] - b[k]).abs() <= tol,
                        "{format:?}: {} vs {}",
                        a[k],
                        b[k]
                    );
                }
            }
        }
    }

    #[test]
    fn axes_are_never_swapped() {
        let g = Geometry::from_polygons(
            vec![
                DVec3::new(1.0, 2.0, 3.0),
                DVec3::new(4.0, 2.0, 3.0),
                DVec3::new(1.0, 5.0, 3.0),
            ],
            vec![vec![0, 1, 2]],
        );
        for format in ALL_MESH_FORMATS.into_iter().chain([Format::Xyz]) {
            let bytes = write_bytes(format, &g, &WriteOptions::default()).unwrap();
            let back = read_bytes(format, &bytes, &ReadOptions::default()).unwrap();
            assert_eq!(sorted_points(&back), sorted_points(&g), "{format:?}");
        }
    }

    #[test]
    fn unit_round_trip_mm_inch_mm_is_tight() {
        let g = tetra();
        let wopts = WriteOptions {
            unit: FileUnit::Inch,
            ..Default::default()
        };
        let ropts = ReadOptions {
            unit: FileUnit::Inch,
            weld: false,
            ..Default::default()
        };
        for format in [Format::Obj, Format::Ply, Format::ThreeMf, Format::Xyz] {
            let bytes = write_bytes(format, &g, &wopts).unwrap();
            let back = read_bytes(format, &bytes, &ropts).unwrap();
            for (a, b) in sorted_exact(&back).iter().zip(&sorted_exact(&g)) {
                for k in 0..3 {
                    assert!((a[k] - b[k]).abs() < 1e-9, "{format:?}");
                }
            }
        }
    }

    #[test]
    fn non_finite_coordinates_are_rejected_on_read() {
        let mut stl = vec![b' '; 80];
        stl.extend_from_slice(&1u32.to_le_bytes());
        let mut facet = vec![0u8; 50];
        facet[12..16].copy_from_slice(&f32::NAN.to_le_bytes());
        stl.extend_from_slice(&facet);
        let ply = b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nend_header\n1 NaN 3\n";
        let cases: [(Format, &[u8]); 5] = [
            (Format::Obj, b"v nan 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n"),
            (Format::Obj, b"v 0 inf 0\n"),
            (Format::Ply, ply),
            (Format::Xyz, b"1 2 inf\n"),
            (Format::Stl, &stl),
        ];
        for (format, bytes) in cases {
            let e = read_bytes(format, bytes, &ReadOptions::default());
            assert!(matches!(e, Err(IoError::Parse { .. })), "{format:?}: {e:?}");
        }
        // Unit conversion that overflows to infinity is caught too.
        let opts = ReadOptions {
            unit: FileUnit::Inch,
            ..Default::default()
        };
        assert!(read_bytes(Format::Obj, b"v 1e308 0 0\n", &opts).is_err());
    }

    #[test]
    fn non_finite_points_are_rejected_on_write() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut g = Geometry::make_box(DVec3::ONE);
            g.points[2].z = bad;
            for format in ALL_MESH_FORMATS.into_iter().chain([Format::Xyz]) {
                let e = write_bytes(format, &g, &WriteOptions::default());
                assert!(matches!(e, Err(IoError::Other(_))), "{format:?} {bad}");
            }
        }
    }

    #[test]
    fn failed_writes_leave_the_existing_file_untouched() {
        let dir = temp_dir("atomic");
        let good = Geometry::make_box(DVec3::ONE);
        let tmp_files = || {
            std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
                .count()
        };
        for ext in ["obj", "stl", "3mf"] {
            let path = dir.join(format!("keep.{ext}"));
            write(&path, &good, &WriteOptions::default()).unwrap();
            assert_eq!(tmp_files(), 0, "{ext}: success leaves no temp file");
            let before = std::fs::read(&path).unwrap();
            // Rejected up front (NaN) ...
            let mut nan = good.clone();
            nan.points[0].x = f64::NAN;
            assert!(write(&path, &nan, &WriteOptions::default()).is_err());
            // ... and failing inside the encoder after the temp file exists
            // (STL and 3MF cannot hold a bare point cloud).
            if ext != "obj" {
                let cloud = Geometry::from_points(vec![DVec3::ZERO]);
                assert!(
                    write(&path, &cloud, &WriteOptions::default()).is_err(),
                    "{ext}"
                );
            }
            assert_eq!(std::fs::read(&path).unwrap(), before, "{ext}");
            assert_eq!(tmp_files(), 0, "{ext}: failure cleans up its temp file");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn truncated_and_corrupt_files_never_panic() {
        let mut g = Geometry::make_box(DVec3::new(10.0, 20.0, 30.0));
        g.point_attribs
            .set("N", AttribArray::Vec3(vec![DVec3::Z; 8]));
        g.point_attribs
            .set("Cd", AttribArray::Vec3(vec![DVec3::new(1.0, 0.5, 0.0); 8]));
        let ascii = WriteOptions {
            ascii: true,
            ..Default::default()
        };
        let binary = WriteOptions::default();
        let cases = [
            (Format::Obj, &binary, false),
            (Format::Xyz, &binary, false),
            (Format::Stl, &ascii, false),
            (Format::Ply, &ascii, false),
            (Format::Stl, &binary, true),
            (Format::Ply, &binary, true),
            (Format::ThreeMf, &binary, true),
        ];
        for (format, opts, strict) in cases {
            let bytes = write_bytes(format, &g, opts).unwrap();
            let ropts = ReadOptions::default();
            for n in 0..bytes.len() {
                let r = read_bytes(format, &bytes[..n], &ropts);
                // Binary containers declare their sizes, so any cut is an error.
                assert!(!strict || r.is_err(), "{format:?} prefix {n}");
            }
            for i in 0..bytes.len() {
                let mut b = bytes.clone();
                b[i] ^= 0xFF;
                let _ = read_bytes(format, &b, &ropts);
            }
        }
    }

    #[test]
    fn sniff_recognises_signatures() {
        let g = Geometry::make_box(DVec3::ONE);
        let bin = WriteOptions::default();
        let asc = WriteOptions {
            ascii: true,
            ..Default::default()
        };
        let sniff = |f, o: &WriteOptions| Format::sniff(&write_bytes(f, &g, o).unwrap());
        assert_eq!(sniff(Format::Ply, &bin), Some(Format::Ply));
        assert_eq!(sniff(Format::Ply, &asc), Some(Format::Ply));
        assert_eq!(sniff(Format::ThreeMf, &bin), Some(Format::ThreeMf));
        assert_eq!(sniff(Format::Stl, &bin), Some(Format::Stl));
        assert_eq!(sniff(Format::Stl, &asc), Some(Format::Stl));
        assert_eq!(sniff(Format::Obj, &bin), None);
        assert_eq!(sniff(Format::Xyz, &bin), None);
        assert_eq!(Format::sniff(b""), None);
        assert_eq!(Format::sniff(b"plyish"), None);
        // A binary STL whose header begins with "solid" is still binary.
        let mut b = write_bytes(Format::Stl, &g, &bin).unwrap();
        b[..5].copy_from_slice(b"solid");
        assert_eq!(Format::sniff(&b), Some(Format::Stl));
    }

    #[test]
    fn stl_weld_tolerance_is_floored_to_f32_resolution() {
        // Shared corners that differ by about one f32 ulp at 2.5 m.
        let stl = b"solid t
 facet normal 0 0 1
  outer loop
   vertex 2500 0 0
   vertex 2600 0 0
   vertex 2500 100 0
  endloop
 endfacet
 facet normal 0 0 1
  outer loop
   vertex 2500.0002 0 0
   vertex 2400 100 0
   vertex 2500 100.0002 0
  endloop
 endfacet
endsolid t
";
        let welded = read_bytes(Format::Stl, stl, &ReadOptions::default()).unwrap();
        assert_eq!(welded.point_count(), 4);
        // An exact weld (tolerance 0) is honoured, and a bare 1e-6 mm weld
        // does not join the corners: the hazard the floor guards against.
        let exact = ReadOptions {
            weld_tolerance: 0.0,
            ..Default::default()
        };
        assert_eq!(
            read_bytes(Format::Stl, stl, &exact).unwrap().point_count(),
            6
        );
        let raw = ReadOptions {
            weld: false,
            ..Default::default()
        };
        let mut soup = read_bytes(Format::Stl, stl, &raw).unwrap();
        weld_points(&mut soup, 1e-6);
        assert_eq!(soup.point_count(), 6);
    }

    #[test]
    fn weld_is_idempotent_and_preserves_area() {
        // Deterministic pseudo-random jitter on the triangle soup of a 5x5 grid.
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut rnd = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        let tol = 1e-3;
        for round in 0..20 {
            let (mut pts, mut faces) = (Vec::new(), Vec::new());
            for i in 0..5 {
                for j in 0..5 {
                    let c = |a: i32, b: i32| {
                        DVec3::new((i + a) as f64 * 10.0, (j + b) as f64 * 10.0, 0.0)
                    };
                    let (p00, p10, p11, p01) = (c(0, 0), c(1, 0), c(1, 1), c(0, 1));
                    for tri in [[p00, p10, p11], [p00, p11, p01]] {
                        let base = pts.len() as u32;
                        for p in tri {
                            pts.push(p + DVec3::new(rnd(), rnd(), rnd()) * (tol * 0.2));
                        }
                        faces.push(vec![base, base + 1, base + 2]);
                    }
                }
            }
            let mut g = Geometry::from_polygons(pts, faces);
            let area = g.surface_area();
            weld_points(&mut g, tol);
            assert_eq!(g.point_count(), 36, "round {round}");
            assert!(
                (g.surface_area() - area).abs() < tol * 4.0 * 100.0,
                "round {round}"
            );
            let once = g.clone();
            assert_eq!(weld_points(&mut g, tol), 0, "round {round}");
            assert_eq!(g, once, "round {round}");
        }
    }

    #[test]
    fn point_cloud_containers_are_registered() {
        for (name, want) in [
            ("scan.e57", Format::E57),
            ("SCAN.E57", Format::E57),
            ("tile.las", Format::Las),
            ("a/b/Tile.LAS", Format::Las),
            ("tile.laz", Format::Las),
        ] {
            assert_eq!(Format::from_path(Path::new(name)), Some(want), "{name}");
        }
        assert!(Format::extensions().contains(&"e57") && Format::extensions().contains(&"las"));
        assert!(!Format::write_extensions().contains(&"e57"));
        assert!(!Format::write_extensions().contains(&"las"));
        assert!(Format::write_extensions()
            .iter()
            .all(|e| Format::extensions().contains(e)));
        assert!(!Format::E57.can_write() && !Format::Las.can_write() && Format::Ply.can_write());
        assert_eq!(
            Format::sniff(&crate::e57::testing::sample_file()),
            Some(Format::E57)
        );
        assert_eq!(
            Format::sniff(&crate::las::testing::sample_file()),
            Some(Format::Las)
        );
    }

    #[test]
    fn e57_and_las_read_in_millimetres_whatever_unit_is_asked_for() {
        let dir = temp_dir("pointcloud-read");
        let e57_path = dir.join("scan.e57");
        let las_path = dir.join("tile.las");
        std::fs::write(&e57_path, crate::e57::testing::sample_file()).unwrap();
        std::fs::write(&las_path, crate::las::testing::sample_file()).unwrap();
        let inch = ReadOptions {
            unit: FileUnit::Inch,
            ..Default::default()
        };
        for path in [&e57_path, &las_path] {
            let a = read(path, &ReadOptions::default()).unwrap();
            let b = read(path, &inch).unwrap();
            assert!(a.is_point_cloud(), "{}", path.display());
            assert_eq!(a.points, b.points, "{}", path.display());
            assert!(a.point_attribs.vec3s("Cd").is_ok(), "{}", path.display());
            let from_memory = read_bytes(
                Format::from_path(path).unwrap(),
                &std::fs::read(path).unwrap(),
                &ReadOptions::default(),
            )
            .unwrap();
            assert_eq!(from_memory.points, a.points);
        }
        assert_eq!(read(&e57_path, &inch).unwrap().point_count(), 40);
        assert_eq!(read(&las_path, &inch).unwrap().point_count(), 25);
        // LAS coordinates are metres unless told otherwise.
        let las_bytes = std::fs::read(&las_path).unwrap();
        let as_metres = read_las_bytes(&las_bytes, None).unwrap();
        let as_mm = read_las_bytes(&las_bytes, Some(FileUnit::Millimetre)).unwrap();
        assert_eq!(as_metres, read(&las_path, &ReadOptions::default()).unwrap());
        for (m, mm) in as_metres.points.iter().zip(&as_mm.points) {
            assert!((*m - *mm * 1000.0).abs().max_element() < 1e-6);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn laz_paths_report_that_laz_is_unsupported() {
        let dir = temp_dir("laz");
        let mut bytes = crate::las::testing::sample_file();
        bytes[104] |= 0x80; // the point format byte carries the compression flag
        let path = dir.join("tile.laz");
        std::fs::write(&path, &bytes).unwrap();
        let err = read(&path, &ReadOptions::default()).unwrap_err().to_string();
        assert!(err.contains("LAZ"), "{err}");
        let err = read_las_bytes(&bytes, Some(FileUnit::Metre)).unwrap_err().to_string();
        assert!(err.contains("LAZ"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn e57_and_las_are_read_only() {
        let dir = temp_dir("pointcloud-write");
        let g = Geometry::make_box(DVec3::splat(10.0));
        for ext in ["e57", "las"] {
            let path = dir.join(format!("out.{ext}"));
            let err = write(&path, &g, &WriteOptions::default())
                .unwrap_err()
                .to_string();
            assert!(err.contains("read but not written"), "{ext}: {err}");
            assert!(!path.exists(), "{ext}: a refused write must not create the file");
        }
        for format in [Format::E57, Format::Las] {
            assert!(write_bytes(format, &g, &WriteOptions::default()).is_err());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn damaged_point_cloud_files_fail_cleanly_through_the_public_api() {
        let mut e57 = crate::e57::testing::sample_file();
        let last = e57.len() - 1;
        e57[last] ^= 0xFF; // inside the last page's checksum
        let err = read_bytes(Format::E57, &e57, &ReadOptions::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("checksum"), "{err}");
        let las = crate::las::testing::sample_file();
        let err = read_bytes(Format::Las, &las[..las.len() - 1], &ReadOptions::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("truncated"), "{err}");
    }
}

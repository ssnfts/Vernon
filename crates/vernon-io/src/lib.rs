//! Vernon file I/O.
//!
//! Every reader returns a [`Geometry`] in millimetres; every writer takes one.
//! The format is picked from the file extension by [`read`] and [`write`].
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
//! | OBJ | `v` (+ optional `r g b` -> `Cd`), `vn` -> point `N`, `vt` -> point `uv`, `f`/`l` with n-gons and negative indices, `g`/`o` -> prim groups. Materials ignored. | `v` (+ `Cd`), `vt`, `vn`, `f`, `l` |
//! | STL | ASCII or binary (detected), weld optional | binary (or ASCII with `WriteOptions::ascii`), fan-triangulated |
//! | PLY | ASCII, binary little/big endian; `x y z`, `nx ny nz` -> `N`, `red green blue` -> `Cd` (0..1), `s t` -> `uv`, other scalars -> Float/Int attribs, face `vertex_indices` | binary little endian (or ASCII); positions as `double` |
//! | 3MF | all build items, components and transforms flattened; model `unit` honoured; prim `part` = build item index | one object per distinct prim `part` value |
//! | XYZ/PTS | `x y z [nx ny nz] [r g b]`, PTS `x y z i [r g b]`; headers and comments skipped | XYZ: `x y z [nx ny nz] [r g b]`; PTS: count line then `x y z i [r g b]` |
//!
//! Open polylines are only written by OBJ; STL, PLY and 3MF write closed
//! polygons only.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use vernon_core::Geometry;

mod obj;
mod ply;
mod stl;
mod threemf;
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
            _ => return None,
        })
    }

    pub fn extensions() -> &'static [&'static str] {
        &["obj", "stl", "ply", "3mf", "xyz", "pts"]
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
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            unit: FileUnit::Millimetre,
            ascii: false,
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
    let format = Format::from_path(path)
        .ok_or_else(|| IoError::Unsupported(path.display().to_string()))?;
    let bytes = std::fs::read(path)?;
    read_named(format, &bytes, opts, &path.display().to_string())
}

/// Read a file image held in memory.
pub fn read_bytes(format: Format, bytes: &[u8], opts: &ReadOptions) -> IoResult<Geometry> {
    read_named(format, bytes, opts, "<memory>")
}

fn read_named(format: Format, bytes: &[u8], opts: &ReadOptions, file: &str) -> IoResult<Geometry> {
    let mut geo = match format {
        Format::Obj => obj::read(bytes, file)?,
        Format::Stl => stl::read(bytes, file)?,
        Format::Ply => ply::read(bytes, file)?,
        Format::ThreeMf => threemf::read(bytes, file)?,
        Format::Xyz => xyz::read(bytes, file)?,
    };
    if format != Format::ThreeMf {
        let k = opts.unit.to_mm();
        if k != 1.0 {
            for p in &mut geo.points {
                *p *= k;
            }
        }
    }
    if opts.weld && format == Format::Stl {
        weld_points(&mut geo, opts.weld_tolerance);
    }
    geo.validate()
        .map_err(|e| parse_err(file, 0, e.to_string()))?;
    Ok(geo)
}

/// Write any supported file, picking the format from the extension.
/// A `.pts` extension writes the Leica PTS layout (count line, `x y z i [r g b]`).
pub fn write(path: &Path, geo: &Geometry, opts: &WriteOptions) -> IoResult<()> {
    let format = Format::from_path(path)
        .ok_or_else(|| IoError::Unsupported(path.display().to_string()))?;
    let pts = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pts"));
    geo.validate().map_err(|e| IoError::Other(e.to_string()))?;
    // Encode fully before touching the file so a failed write leaves no stub.
    if format == Format::ThreeMf {
        let bytes = threemf::write(geo, opts)?;
        std::fs::write(path, bytes)?;
        return Ok(());
    }
    let mut w = BufWriter::new(File::create(path)?);
    write_to(&mut w, format, geo, opts, pts)?;
    w.flush()?;
    Ok(())
}

/// Encode a geometry into an in-memory file image (`Format::Xyz` writes the
/// plain XYZ layout).
pub fn write_bytes(format: Format, geo: &Geometry, opts: &WriteOptions) -> IoResult<Vec<u8>> {
    geo.validate().map_err(|e| IoError::Other(e.to_string()))?;
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
            assert!((back.surface_area() - g.surface_area()).abs() < 1e-6, "{ext}");
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
            ascii: false,
        };
        for format in [Format::Obj, Format::Stl, Format::Ply, Format::Xyz, Format::ThreeMf] {
            let bytes = write_bytes(format, &g, &opts).unwrap();
            // Read back without conversion: numbers are inches (box is 10 in).
            let raw = read_bytes(format, &bytes, &ReadOptions::default()).unwrap();
            let expect = if format == Format::ThreeMf { 254.0 } else { 10.0 };
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
}

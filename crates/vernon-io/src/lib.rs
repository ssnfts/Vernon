//! Vernon file I/O.
//!
//! Every reader returns a [`Geometry`] in millimetres; every writer takes one.
//! The format is picked from the file extension by [`read`] and [`write`].
//!
//! CONTRACT (other crates compile against these signatures; keep them stable):
//! - `read(path, &ReadOptions) -> IoResult<Geometry>`
//! - `write(path, &Geometry, &WriteOptions) -> IoResult<()>`
//! - `Format::from_path(path) -> Option<Format>`

use std::path::Path;

use vernon_core::Geometry;

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

/// Read any supported file, picking the format from the extension.
pub fn read(path: &Path, opts: &ReadOptions) -> IoResult<Geometry> {
    let format = Format::from_path(path)
        .ok_or_else(|| IoError::Unsupported(path.display().to_string()))?;
    let _ = (format, opts);
    Err(IoError::Unsupported(format!(
        "{format:?} reader not implemented yet"
    )))
}

/// Write any supported file, picking the format from the extension.
pub fn write(path: &Path, geo: &Geometry, opts: &WriteOptions) -> IoResult<()> {
    let format = Format::from_path(path)
        .ok_or_else(|| IoError::Unsupported(path.display().to_string()))?;
    let _ = (format, geo, opts);
    Err(IoError::Unsupported(format!(
        "{format:?} writer not implemented yet"
    )))
}

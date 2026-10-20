use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use vernon_core::{Geometry, Report};

/// The kind of value flowing along a wire. Used for port type checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DataKind {
    Geometry,
    Report,
    /// A file on disk (STEP, 3MF, G-code, OpenFOAM case directory...).
    File,
    /// Arbitrary structured data (tables, solver settings, lap traces).
    Json,
    /// Accepts anything.
    Any,
}

/// A value flowing along a wire. Geometry is reference-counted so passing it
/// through unchanged is free; clone-on-write with `Arc::make_mut`.
#[derive(Debug, Clone)]
pub enum Data {
    Geometry(Arc<Geometry>),
    Report(Arc<Report>),
    File(PathBuf),
    Json(Arc<serde_json::Value>),
}

impl Data {
    pub fn geometry(g: Geometry) -> Self {
        Data::Geometry(Arc::new(g))
    }

    pub fn report(r: Report) -> Self {
        Data::Report(Arc::new(r))
    }

    pub fn json(v: serde_json::Value) -> Self {
        Data::Json(Arc::new(v))
    }

    pub fn kind(&self) -> DataKind {
        match self {
            Data::Geometry(_) => DataKind::Geometry,
            Data::Report(_) => DataKind::Report,
            Data::File(_) => DataKind::File,
            Data::Json(_) => DataKind::Json,
        }
    }

    pub fn as_geometry(&self) -> Option<&Arc<Geometry>> {
        match self {
            Data::Geometry(g) => Some(g),
            _ => None,
        }
    }

    pub fn as_report(&self) -> Option<&Arc<Report>> {
        match self {
            Data::Report(r) => Some(r),
            _ => None,
        }
    }

    pub fn as_file(&self) -> Option<&PathBuf> {
        match self {
            Data::File(p) => Some(p),
            _ => None,
        }
    }

    pub fn as_json(&self) -> Option<&Arc<serde_json::Value>> {
        match self {
            Data::Json(v) => Some(v),
            _ => None,
        }
    }
}

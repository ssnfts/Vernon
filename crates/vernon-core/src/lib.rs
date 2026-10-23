//! Vernon geometry kernel.
//!
//! The model is deliberately simple and generic: a [`Geometry`] holds a list of
//! points, a list of primitives that reference points by index, and typed
//! attribute tables at point, primitive and detail (whole-geometry) level.
//! A point cloud is a `Geometry` with no primitives. A polygon mesh uses
//! closed primitives; polylines use open ones.
//!
//! All lengths are millimetres unless an attribute or report says otherwise.

pub mod attrib;
pub mod geometry;
pub mod materials;
pub mod process;
pub mod report;

pub use attrib::{AttribArray, AttribTable, AttribType};
pub use geometry::{Aabb, Geometry, Prim};
pub use report::{keys, Metric, Report, Severity};

pub use glam::{DMat4, DVec2, DVec3};

/// Errors raised by geometry operations.
#[derive(Debug, thiserror::Error)]
pub enum GeoError {
    #[error("attribute '{0}' not found")]
    MissingAttrib(String),
    #[error("attribute '{name}' has type {found:?}, expected {expected:?}")]
    AttribType {
        name: String,
        expected: AttribType,
        found: AttribType,
    },
    #[error("attribute '{name}' has {found} elements, expected {expected}")]
    AttribLen {
        name: String,
        expected: usize,
        found: usize,
    },
    #[error("primitive {prim} references point {point}, but there are only {count} points")]
    BadPointRef { prim: usize, point: u32, count: usize },
    #[error("{0}")]
    Other(String),
}

pub type GeoResult<T> = Result<T, GeoError>;

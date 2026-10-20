//! Vernon node graph.
//!
//! A node *type* implements [`NodeOp`]: it describes its ports and parameters
//! through [`NodeTypeInfo`] and turns input [`Data`] into output [`Data`] in
//! [`NodeOp::cook`]. Node crates register their types in a [`Registry`]
//! through a `pub fn register(reg: &mut Registry)` function.
//!
//! A [`Graph`] holds node *instances* (type id + parameter values + input
//! wiring) and cooks them lazily, caching outputs until a parameter or an
//! upstream node changes.

pub mod data;
pub mod graph;
pub mod node;
pub mod param;
pub mod registry;

pub use data::{Data, DataKind};
pub use graph::{Graph, NodeId, NodeInstance};
pub use node::{CookContext, NodeOp, NodeTypeInfo, PortSpec, Stage};
pub use param::{ParamKind, ParamSpec, ParamValue, Params};
pub use registry::Registry;

/// Errors raised while cooking.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CookError {
    #[error("unknown node type '{0}'")]
    UnknownType(String),
    #[error("unknown node {0}")]
    UnknownNode(NodeId),
    #[error("input {index} ('{name}') is required but not connected")]
    MissingInput { index: usize, name: String },
    #[error("input {index}: expected {expected:?}, got {found:?}")]
    WrongInput {
        index: usize,
        expected: DataKind,
        found: DataKind,
    },
    #[error("parameter '{0}' is missing or has the wrong type")]
    BadParam(String),
    #[error("cycle detected at node {0}")]
    Cycle(NodeId),
    #[error("node '{node}' failed: {message}")]
    Failed { node: String, message: String },
    /// A white-box node that has no real algorithm yet and cannot even
    /// produce a placeholder for this input.
    #[error("'{0}' is not implemented yet (white-box stub)")]
    NotImplemented(String),
    #[error("external tool '{tool}' failed: {message}")]
    External { tool: String, message: String },
    #[error("{0}")]
    Other(String),
}

impl From<vernon_core::GeoError> for CookError {
    fn from(e: vernon_core::GeoError) -> Self {
        CookError::Other(e.to_string())
    }
}

impl From<std::io::Error> for CookError {
    fn from(e: std::io::Error) -> Self {
        CookError::Other(e.to_string())
    }
}

pub type CookResult<T> = Result<T, CookError>;

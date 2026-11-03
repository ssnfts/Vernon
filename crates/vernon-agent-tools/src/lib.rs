//! Tools an AI agent uses to build and cook Vernon node graphs.
//!
//! The boundary is [`ToolHost`]: a list of [`ToolSpec`]s (name, description,
//! JSON Schema) and a `call` that takes JSON arguments and returns JSON or a
//! corrective error string. [`GraphToolHost`] implements it over a
//! [`vernon_graph::Graph`], a [`Registry`] and a work folder. It needs no
//! model: the same host serves the in-app agent and the MCP server.
//!
//! Safety: there is no shell and no arbitrary file access. File parameters
//! must be relative paths without `..`; `load_graph` / `save_graph` stay in
//! the work folder; the `run` switch of a node that drives an external
//! program stays off unless a call sets it, and the result says so.

mod host;
mod measure;
mod schema;

pub use host::GraphToolHost;
pub use measure::{measure, Measurement};

use serde_json::Value;

/// One tool as shown to a model.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema (`type: object`) of the arguments.
    pub parameters: Value,
}

/// What an agent loop or an MCP server needs from the tool layer.
pub trait ToolHost {
    fn tool_specs(&self) -> Vec<ToolSpec>;
    /// Run tool `name`. `Err` is a precise message telling the caller how to
    /// correct the call; it is meant to be handed back to the model.
    fn call(&mut self, name: &str, args: Value) -> Result<Value, String>;
}

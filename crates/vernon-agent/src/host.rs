//! The tool layer boundary. Owned by `vernon-agent-tools`; this copy is
//! swapped for a dependency on that crate once it lands.

/// One tool the model may call. `parameters` is a JSON Schema object.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

pub trait ToolHost {
    fn tool_specs(&self) -> Vec<ToolSpec>;
    fn call(&mut self, name: &str, args: serde_json::Value) -> Result<serde_json::Value, String>;
}

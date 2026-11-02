//! A stdio MCP server (JSON-RPC 2.0, one JSON message per line) over any
//! [`ToolHost`]. Hand-rolled on `serde_json`: `initialize` with protocol
//! version negotiation, `ping`, `tools/list` and `tools/call`.
//! Nothing but protocol messages is ever written to the output.

use std::io::{BufRead, Write};

use serde_json::{json, Value};
use vernon_agent_tools::ToolHost;

/// Protocol versions this server speaks, newest first.
pub const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

const INSTRUCTIONS: &str = "Vernon node-graph tools. Read the agent guide first if you have it; \
otherwise call list_nodes, describe_node, then add_node / connect / cook. \
Lengths are millimetres, Z is up. Errors name the valid choices.";

pub struct McpServer<H: ToolHost> {
    host: H,
}

fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

fn ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

impl<H: ToolHost> McpServer<H> {
    pub fn new(host: H) -> Self {
        Self { host }
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    /// Handle one raw line. `None` when nothing is to be sent back (a
    /// notification, or an empty line).
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            Err(e) => Some(error(Value::Null, PARSE_ERROR, format!("parse error: {e}"))),
            Ok(Value::Array(items)) if items.is_empty() => Some(error(Value::Null, INVALID_REQUEST, "empty batch")),
            Ok(Value::Array(items)) => {
                let replies: Vec<Value> = items.iter().filter_map(|m| self.handle(m)).collect();
                if replies.is_empty() {
                    None
                } else {
                    Some(Value::Array(replies))
                }
            }
            Ok(msg) => self.handle(&msg),
        };
        reply.map(|r| r.to_string())
    }

    /// Handle one JSON-RPC message.
    pub fn handle(&mut self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            // A response from the client, or junk: only junk with an id gets an answer.
            return match (msg.get("result").or_else(|| msg.get("error")), id) {
                (Some(_), _) => None,
                (None, id) => Some(error(id.unwrap_or(Value::Null), INVALID_REQUEST, "not a JSON-RPC request: missing 'method'")),
            };
        };
        let Some(id) = id else {
            // Notification (`notifications/initialized`, `notifications/cancelled`, ...): no reply.
            return None;
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        Some(match method {
            "initialize" => ok(id, self.initialize(&params)),
            "ping" => ok(id, json!({})),
            "tools/list" => {
                let tools: Vec<Value> = self
                    .host
                    .tool_specs()
                    .into_iter()
                    .map(|t| json!({"name": t.name, "description": t.description, "inputSchema": t.parameters}))
                    .collect();
                ok(id, json!({"tools": tools}))
            }
            "tools/call" => self.call_tool(id, &params),
            other => error(id, METHOD_NOT_FOUND, format!("method not found: {other}")),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let wanted = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
        let version = SUPPORTED_VERSIONS.iter().find(|v| **v == wanted).unwrap_or(&SUPPORTED_VERSIONS[0]);
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "vernon", "version": env!("CARGO_PKG_VERSION")},
            "instructions": INSTRUCTIONS,
        })
    }

    fn call_tool(&mut self, id: Value, params: &Value) -> Value {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return error(id, INVALID_PARAMS, "tools/call needs params.name");
        };
        if !self.host.tool_specs().iter().any(|t| t.name == name) {
            return error(id, INVALID_PARAMS, format!("unknown tool '{name}'"));
        }
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        let (text, is_error) = match self.host.call(name, args) {
            Ok(v) => (serde_json::to_string_pretty(&v).unwrap_or_default(), false),
            Err(e) => (e, true),
        };
        ok(id, json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
    }
}

/// Serve until `input` ends: one JSON message per line in, one per line out.
pub fn serve<H: ToolHost>(host: H, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
    let mut server = McpServer::new(host);
    for line in input.lines() {
        if let Some(reply) = server.handle_line(&line?) {
            writeln!(output, "{reply}")?;
            output.flush()?;
        }
    }
    Ok(())
}

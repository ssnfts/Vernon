//! Chat messages, the `ChatModel` trait and the OpenAI-compatible client.

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde_json::{json, Value};

use crate::host::ToolSpec;
use crate::http;

/// A tool call as the model wrote it. `arguments` is the raw JSON text so
/// malformed arguments reach the loop (which tells the model) instead of
/// failing inside the client.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    System(String),
    User(String),
    Assistant { content: String, tool_calls: Vec<ToolCall> },
    Tool { call_id: String, name: String, content: String },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelReply {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

pub trait ChatModel: Send {
    fn chat(&mut self, messages: &[Message], tools: &[ToolSpec], cancel: &AtomicBool) -> Result<ModelReply, String>;
}

/// Connection settings. The key is never serialised or printed.
#[derive(Clone)]
pub struct ClientConfig {
    /// e.g. `http://localhost:11434/v1`
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub timeout: Duration,
}

impl std::fmt::Debug for ClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientConfig")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "<set>"))
            .finish()
    }
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11434/v1".into(),
            model: String::new(),
            api_key: None,
            timeout: Duration::from_secs(300),
        }
    }
}

impl ClientConfig {
    /// Defaults from `VERNON_AGENT_URL`, `VERNON_AGENT_MODEL` and the key
    /// from `VERNON_AGENT_KEY`.
    pub fn from_env() -> Self {
        let mut c = Self::default();
        if let Ok(v) = std::env::var("VERNON_AGENT_URL") {
            c.base_url = v;
        }
        if let Ok(v) = std::env::var("VERNON_AGENT_MODEL") {
            c.model = v;
        }
        c.api_key = api_key_from_env("VERNON_AGENT_KEY");
        c
    }

    fn endpoint(&self) -> String {
        let b = self.base_url.trim().trim_end_matches('/');
        if b.ends_with("/chat/completions") {
            b.to_string()
        } else {
            format!("{b}/chat/completions")
        }
    }
}

pub fn api_key_from_env(var: &str) -> Option<String> {
    std::env::var(var).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub struct OpenAiClient {
    pub config: ClientConfig,
}

impl OpenAiClient {
    pub fn new(config: ClientConfig) -> Self {
        Self { config }
    }
}

pub fn request_body(model: &str, messages: &[Message], tools: &[ToolSpec]) -> Value {
    let msgs: Vec<Value> = messages
        .iter()
        .map(|m| match m {
            Message::System(c) => json!({"role": "system", "content": c}),
            Message::User(c) => json!({"role": "user", "content": c}),
            Message::Assistant { content, tool_calls } => {
                let mut v = json!({"role": "assistant", "content": content});
                if !tool_calls.is_empty() {
                    v["tool_calls"] = tool_calls
                        .iter()
                        .map(|t| json!({"id": t.id, "type": "function", "function": {"name": t.name, "arguments": t.arguments}}))
                        .collect();
                }
                v
            }
            Message::Tool { call_id, name, content } => json!({"role": "tool", "tool_call_id": call_id, "name": name, "content": content}),
        })
        .collect();
    let mut body = json!({"model": model, "messages": msgs, "stream": false});
    if !tools.is_empty() {
        body["tools"] = tools
            .iter()
            .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}}))
            .collect();
    }
    body
}

/// Read `choices[0].message` of a chat-completions response. Tolerates
/// `arguments` given as an object and a missing `content`.
pub fn parse_reply(v: &Value) -> Result<ModelReply, String> {
    if let Some(err) = v.get("error") {
        let text = err.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| err.to_string());
        return Err(format!("model server error: {text}"));
    }
    let msg = v
        .pointer("/choices/0/message")
        .ok_or_else(|| format!("response has no choices[0].message: {}", crate::agent::clip(&v.to_string(), 200)))?;
    let content = msg.get("content").and_then(Value::as_str).unwrap_or("").to_string();
    let mut tool_calls = Vec::new();
    if let Some(arr) = msg.get("tool_calls").and_then(Value::as_array) {
        for (i, t) in arr.iter().enumerate() {
            let f = t.get("function").unwrap_or(t);
            let name = f.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let arguments = match f.get("arguments") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Null) | None => "{}".to_string(),
                Some(other) => other.to_string(),
            };
            let id = t.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("call_{i}"));
            tool_calls.push(ToolCall { id, name, arguments });
        }
    }
    Ok(ModelReply { content, tool_calls })
}

impl ChatModel for OpenAiClient {
    fn chat(&mut self, messages: &[Message], tools: &[ToolSpec], cancel: &AtomicBool) -> Result<ModelReply, String> {
        let body = request_body(&self.config.model, messages, tools).to_string();
        let mut headers = Vec::new();
        if let Some(k) = &self.config.api_key {
            headers.push(("Authorization".to_string(), format!("Bearer {k}")));
        }
        let resp = http::post_json(&self.config.endpoint(), &headers, body.as_bytes(), self.config.timeout, Some(cancel))?;
        let text = String::from_utf8_lossy(&resp.body);
        if !(200..300).contains(&resp.status) {
            return Err(format!("model server returned HTTP {}: {}", resp.status, crate::agent::clip(&text, 300)));
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("model server sent invalid JSON ({e}): {}", crate::agent::clip(&text, 200)))?;
        parse_reply(&v)
    }
}

/// A model that replays fixed replies (tests, recorded tasks).
pub struct ScriptedModel {
    pub replies: VecDeque<ModelReply>,
    /// Message lists the loop sent, one per call.
    pub seen: Vec<Vec<Message>>,
}

impl ScriptedModel {
    pub fn new(replies: impl IntoIterator<Item = ModelReply>) -> Self {
        Self { replies: replies.into_iter().collect(), seen: Vec::new() }
    }
}

impl ChatModel for ScriptedModel {
    fn chat(&mut self, messages: &[Message], _tools: &[ToolSpec], _cancel: &AtomicBool) -> Result<ModelReply, String> {
        self.seen.push(messages.to_vec());
        self.replies.pop_front().ok_or_else(|| "scripted model ran out of replies".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn parses_tool_calls_with_string_and_object_arguments() {
        let v = json!({"choices": [{"message": {"content": null, "tool_calls": [
            {"id": "a", "type": "function", "function": {"name": "x", "arguments": "{\"k\":1}"}},
            {"function": {"name": "y", "arguments": {"k": 2}}}
        ]}}]});
        let r = parse_reply(&v).unwrap();
        assert_eq!(r.tool_calls[0].arguments, "{\"k\":1}");
        assert_eq!(r.tool_calls[1].id, "call_1");
        assert_eq!(r.tool_calls[1].arguments, "{\"k\":2}");
    }

    #[test]
    fn error_objects_surface() {
        assert!(parse_reply(&json!({"error": {"message": "no such model"}})).unwrap_err().contains("no such model"));
    }

    #[test]
    fn key_is_hidden_from_debug() {
        let c = ClientConfig { api_key: Some("sk-secret".into()), ..Default::default() };
        assert!(!format!("{c:?}").contains("sk-secret"));
    }

    #[test]
    fn full_round_trip_against_a_local_server() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            while !String::from_utf8_lossy(&req).contains("\"messages\"") {
                let n = s.read(&mut buf).unwrap();
                req.extend_from_slice(&buf[..n]);
            }
            let body = r#"{"choices":[{"message":{"content":"hi"}}]}"#;
            write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
            String::from_utf8_lossy(&req).into_owned()
        });
        let mut c = OpenAiClient::new(ClientConfig {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: "m".into(),
            api_key: Some("k".into()),
            timeout: Duration::from_secs(5),
        });
        let r = c.chat(&[Message::User("yo".into())], &[], &AtomicBool::new(false)).unwrap();
        assert_eq!(r.content, "hi");
        let req = t.join().unwrap();
        assert!(req.starts_with("POST /v1/chat/completions"));
        assert!(req.contains("Bearer k"));
    }

    #[test]
    fn https_is_refused_clearly() {
        let mut c = OpenAiClient::new(ClientConfig { base_url: "https://api.example.com/v1".into(), ..Default::default() });
        assert!(c.chat(&[], &[], &AtomicBool::new(false)).unwrap_err().contains("gateway"));
    }
}

//! Vernon's built-in agent client: an OpenAI-compatible chat client (plain
//! `http://`, no TLS stack) and a tool-calling loop over a [`ToolHost`].

pub mod agent;
pub mod client;
pub mod host;
pub mod http;

pub use agent::{load_system_prompt, run_agent, AgentConfig, AgentEvent, AgentOutcome, StopReason, ToolLogEntry};
pub use client::{ChatModel, ClientConfig, Message, ModelReply, OpenAiClient, ScriptedModel, ToolCall};
pub use host::{ToolHost, ToolSpec};

//! The agent loop: model call, tool calls, results back, until the model
//! answers without a tool call or a limit stops it.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};

use crate::client::{ChatModel, Message, ModelReply, ToolCall};
use crate::host::{ToolHost, ToolSpec};

/// Used when `docs/AGENT_GUIDE.md` is not found.
pub const BUILTIN_GUIDE: &str = "You build and run Vernon node graphs by calling tools. \
Work in small steps: look at what exists, add nodes, set parameters, connect them, cook, \
then read the report metrics. Lengths are millimetres, Z is up. If a tool returns an error, \
read it and change your call; do not repeat the same call. When the goal is met, or you cannot \
meet it, reply in plain text with a short summary and no tool call.";

/// The guide file as the system prompt, searched in `dir`/docs and its
/// ancestors; the built-in short guide otherwise.
pub fn load_system_prompt(dir: Option<&Path>) -> String {
    let mut cur = dir.map(Path::to_path_buf);
    while let Some(d) = cur {
        if let Ok(text) = std::fs::read_to_string(d.join("docs").join("AGENT_GUIDE.md")) {
            if !text.trim().is_empty() {
                return text;
            }
        }
        cur = d.parent().map(Path::to_path_buf);
    }
    BUILTIN_GUIDE.to_string()
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub system_prompt: String,
    /// Model calls allowed in one run.
    pub max_steps: usize,
    /// Most recent tool exchanges sent to the model; older ones are
    /// replaced by a one-line note.
    pub keep_turns: usize,
    /// Longest tool result sent to the model, in characters.
    pub max_result_chars: usize,
    /// The same call (name and arguments) is refused from this count on.
    pub repeat_limit: usize,
    /// Tools the model may see and call: exact names, or a prefix ending in
    /// `*`. `None` allows all. The group tools are never shown.
    pub tool_filter: Option<Vec<String>>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: BUILTIN_GUIDE.to_string(),
            max_steps: 40,
            keep_turns: 6,
            max_result_chars: 4000,
            repeat_limit: 3,
            tool_filter: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    /// The model answered without a tool call.
    Done,
    StepLimit,
    Cancelled,
    /// Repeated refused calls or empty replies: the model is going nowhere.
    Stuck,
    ModelError(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolLogEntry {
    pub name: String,
    pub args: String,
    pub ok: bool,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentOutcome {
    pub final_text: String,
    pub steps: usize,
    pub log: Vec<ToolLogEntry>,
    pub stop: StopReason,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Assistant(String),
    Tool(ToolLogEntry),
}

pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}...[{} more chars]", s.chars().count() - max)
}

fn allowed(filter: &Option<Vec<String>>, name: &str) -> bool {
    match filter {
        None => true,
        Some(list) => list.iter().any(|p| match p.strip_suffix('*') {
            Some(prefix) => name.starts_with(prefix),
            None => p == name,
        }),
    }
}

const BEGIN: &str = "begin_group";
const END: &str = "end_group";

struct Exchange {
    assistant: Message,
    results: Vec<Message>,
}

fn compact(goal: &str, cfg: &AgentConfig, nudges: &[String], history: &[Exchange]) -> Vec<Message> {
    let mut msgs = vec![Message::System(cfg.system_prompt.clone()), Message::User(goal.to_string())];
    let split = history.len().saturating_sub(cfg.keep_turns);
    if split > 0 {
        let names: Vec<String> = history[..split]
            .iter()
            .flat_map(|e| e.results.iter())
            .filter_map(|m| match m {
                Message::Tool { name, content, .. } => Some(format!("{name}{}", if content.starts_with("error") { "(error)" } else { "" })),
                _ => None,
            })
            .collect();
        msgs.push(Message::User(format!(
            "[{split} earlier steps omitted to save space. Calls made: {}. Call a read tool again if you need their state.]",
            clip(&names.join(", "), 600)
        )));
    }
    for e in &history[split..] {
        msgs.push(e.assistant.clone());
        msgs.extend(e.results.iter().cloned());
    }
    for n in nudges {
        msgs.push(Message::User(n.clone()));
    }
    msgs
}

/// Run one goal to completion. The host is used as given; if it offers
/// `begin_group` and `end_group` they bracket the run so it is one undo step.
pub fn run_agent(
    model: &mut dyn ChatModel,
    host: &mut dyn ToolHost,
    goal: &str,
    cfg: &AgentConfig,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(AgentEvent),
) -> AgentOutcome {
    let all = host.tool_specs();
    let grouped = all.iter().any(|t| t.name == BEGIN) && all.iter().any(|t| t.name == END);
    let specs: Vec<ToolSpec> = all.into_iter().filter(|t| t.name != BEGIN && t.name != END && allowed(&cfg.tool_filter, &t.name)).collect();
    if grouped {
        let _ = host.call(BEGIN, json!({"label": clip(goal, 60)}));
    }
    let outcome = drive(model, host, goal, cfg, cancel, on_event, &specs);
    if grouped {
        let _ = host.call(END, json!({}));
    }
    outcome
}

fn drive(
    model: &mut dyn ChatModel,
    host: &mut dyn ToolHost,
    goal: &str,
    cfg: &AgentConfig,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(AgentEvent),
    specs: &[ToolSpec],
) -> AgentOutcome {
    let mut out = AgentOutcome { final_text: String::new(), steps: 0, log: Vec::new(), stop: StopReason::StepLimit };
    let mut history: Vec<Exchange> = Vec::new();
    let mut nudges: Vec<String> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let (mut refused, mut empty) = (0usize, 0usize);
    let names: Vec<&str> = specs.iter().map(|t| t.name.as_str()).collect();

    while out.steps < cfg.max_steps {
        if cancel.load(Ordering::Relaxed) {
            out.stop = StopReason::Cancelled;
            return out;
        }
        out.steps += 1;
        let msgs = compact(goal, cfg, &nudges, &history);
        let reply = match model.chat(&msgs, specs, cancel) {
            Ok(r) => r,
            Err(e) => {
                out.stop = if e == "cancelled" || cancel.load(Ordering::Relaxed) { StopReason::Cancelled } else { StopReason::ModelError(e) };
                return out;
            }
        };
        let ModelReply { content, mut tool_calls } = reply;
        if tool_calls.is_empty() {
            tool_calls = extract_text_calls(&content, &names);
        }
        if !content.trim().is_empty() {
            on_event(AgentEvent::Assistant(content.clone()));
        }
        if tool_calls.is_empty() {
            if content.trim().is_empty() {
                empty += 1;
                if empty >= 3 {
                    out.stop = StopReason::Stuck;
                    return out;
                }
                nudges = vec!["Your last reply was empty. Call a tool, or answer in plain text if you are finished.".into()];
                continue;
            }
            out.final_text = content;
            out.stop = StopReason::Done;
            return out;
        }
        nudges.clear();
        let mut results = Vec::new();
        for tc in &tool_calls {
            if cancel.load(Ordering::Relaxed) {
                out.stop = StopReason::Cancelled;
                return out;
            }
            let (ok, text) = execute(host, tc, specs, cfg, &mut seen, &mut refused);
            let entry = ToolLogEntry { name: tc.name.clone(), args: clip(&tc.arguments, 120), ok, summary: clip(&text, 160) };
            on_event(AgentEvent::Tool(entry.clone()));
            out.log.push(entry);
            let body = if ok { clip(&text, cfg.max_result_chars) } else { format!("error: {}", clip(&text, cfg.max_result_chars)) };
            results.push(Message::Tool { call_id: tc.id.clone(), name: tc.name.clone(), content: body });
        }
        history.push(Exchange { assistant: Message::Assistant { content, tool_calls }, results });
        if refused >= 4 {
            out.stop = StopReason::Stuck;
            return out;
        }
    }
    out
}

fn execute(host: &mut dyn ToolHost, tc: &ToolCall, specs: &[ToolSpec], cfg: &AgentConfig, seen: &mut HashMap<String, usize>, refused: &mut usize) -> (bool, String) {
    if !specs.iter().any(|t| t.name == tc.name) {
        *refused += 1;
        let list: Vec<&str> = specs.iter().map(|t| t.name.as_str()).collect();
        return (false, format!("unknown tool '{}'. Available tools: {}", tc.name, list.join(", ")));
    }
    let raw = if tc.arguments.trim().is_empty() { "{}" } else { tc.arguments.trim() };
    let args: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => return (false, format!("arguments are not valid JSON ({e}). Send one JSON object matching the tool's parameters.")),
    };
    if !args.is_object() {
        return (false, "arguments must be a JSON object".into());
    }
    let key = format!("{}\u{1}{}", tc.name, args);
    let n = seen.entry(key).and_modify(|n| *n += 1).or_insert(1);
    if *n >= cfg.repeat_limit {
        *refused += 1;
        return (false, format!("you already made this exact call {} times; it will not change. Try a different call or finish.", *n - 1));
    }
    match host.call(&tc.name, args) {
        Ok(v) => (true, if let Value::String(s) = &v { s.clone() } else { v.to_string() }),
        Err(e) => (false, e),
    }
}

/// Weak models often write the call as JSON in plain text (optionally in a
/// code fence) instead of using `tool_calls`. Accepts `{"name":..,
/// "arguments":..}` and common spellings of both keys, a `function`
/// wrapper, or an array of these; only for tools that exist.
pub fn extract_text_calls(content: &str, known: &[&str]) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    let mut from = 0;
    while let Some(off) = content[from..].find(['{', '[']) {
        let start = from + off;
        let mut it = serde_json::Deserializer::from_str(&content[start..]).into_iter::<Value>();
        match it.next() {
            Some(Ok(v)) => {
                collect_calls(&v, known, &mut calls);
                from = start + it.byte_offset().max(1);
            }
            _ => from = start + 1,
        }
    }
    for (i, c) in calls.iter_mut().enumerate() {
        c.id = format!("text_{i}");
    }
    calls
}

fn collect_calls(v: &Value, known: &[&str], out: &mut Vec<ToolCall>) {
    match v {
        Value::Array(items) => items.iter().for_each(|i| collect_calls(i, known, out)),
        Value::Object(map) => {
            let name = ["name", "tool", "tool_name", "function_name"].iter().find_map(|k| map.get(*k).and_then(Value::as_str));
            if let Some(name) = name.filter(|n| known.contains(n)) {
                let args = ["arguments", "parameters", "args", "input"].iter().find_map(|k| map.get(*k));
                let arguments = match args {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => "{}".to_string(),
                };
                out.push(ToolCall { id: String::new(), name: name.to_string(), arguments });
            } else {
                for k in ["function", "tool_call", "tool_calls", "call"] {
                    if let Some(inner) = map.get(k) {
                        collect_calls(inner, known, out);
                    }
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ScriptedModel;

    struct Echo {
        calls: Vec<(String, Value)>,
        group: bool,
    }

    impl ToolHost for Echo {
        fn tool_specs(&self) -> Vec<ToolSpec> {
            let mut names = vec!["ping", "geo_add", "cfd_run"];
            if self.group {
                names.extend([BEGIN, END]);
            }
            names.into_iter().map(|n| ToolSpec { name: n.into(), description: String::new(), parameters: json!({"type": "object"}) }).collect()
        }
        fn call(&mut self, name: &str, args: Value) -> Result<Value, String> {
            self.calls.push((name.into(), args.clone()));
            if args.get("fail").is_some() {
                return Err("nope".into());
            }
            Ok(json!({"ok": true}))
        }
    }

    fn call(name: &str, args: &str) -> ToolCall {
        ToolCall { id: format!("id_{name}"), name: name.into(), arguments: args.into() }
    }

    fn reply(calls: Vec<ToolCall>) -> ModelReply {
        ModelReply { content: String::new(), tool_calls: calls }
    }

    fn text(s: &str) -> ModelReply {
        ModelReply { content: s.into(), tool_calls: vec![] }
    }

    fn run(model: &mut ScriptedModel, host: &mut Echo, cfg: &AgentConfig) -> AgentOutcome {
        run_agent(model, host, "goal", cfg, &AtomicBool::new(false), &mut |_| {})
    }

    #[test]
    fn step_limit_stops_a_looping_model() {
        let replies = (0..10).map(|i| reply(vec![call("ping", &format!("{{\"i\":{i}}}"))]));
        let mut m = ScriptedModel::new(replies);
        let mut h = Echo { calls: vec![], group: false };
        let o = run(&mut m, &mut h, &AgentConfig { max_steps: 3, ..Default::default() });
        assert_eq!((o.stop, o.steps, h.calls.len()), (StopReason::StepLimit, 3, 3));
    }

    #[test]
    fn cancel_before_and_during() {
        let mut m = ScriptedModel::new([reply(vec![call("ping", "{}")])]);
        let mut h = Echo { calls: vec![], group: false };
        let flag = AtomicBool::new(true);
        let o = run_agent(&mut m, &mut h, "g", &AgentConfig::default(), &flag, &mut |_| {});
        assert_eq!(o.stop, StopReason::Cancelled);
        assert!(h.calls.is_empty());
    }

    #[test]
    fn cancel_flag_raised_by_a_tool_stops_the_run() {
        let flag = std::sync::Arc::new(AtomicBool::new(false));
        struct Raiser(std::sync::Arc<AtomicBool>);
        impl ToolHost for Raiser {
            fn tool_specs(&self) -> Vec<ToolSpec> {
                vec![ToolSpec { name: "ping".into(), description: String::new(), parameters: json!({}) }]
            }
            fn call(&mut self, _: &str, _: Value) -> Result<Value, String> {
                self.0.store(true, Ordering::Relaxed);
                Ok(json!(1))
            }
        }
        let mut m = ScriptedModel::new([reply(vec![call("ping", "{}")]), text("never")]);
        let o = run_agent(&mut m, &mut Raiser(flag.clone()), "g", &AgentConfig::default(), &flag, &mut |_| {});
        assert_eq!(o.stop, StopReason::Cancelled);
        assert_eq!(o.log.len(), 1);
    }

    #[test]
    fn malformed_json_is_fed_back_and_not_executed() {
        let mut m = ScriptedModel::new([reply(vec![call("ping", "{not json")]), text("done")]);
        let mut h = Echo { calls: vec![], group: false };
        let o = run(&mut m, &mut h, &AgentConfig::default());
        assert!(h.calls.is_empty());
        assert!(!o.log[0].ok && o.log[0].summary.contains("not valid JSON"));
        let Message::Tool { content, .. } = &m.seen[1].last().unwrap() else { panic!() };
        assert!(content.starts_with("error: arguments are not valid JSON"));
        assert_eq!(o.stop, StopReason::Done);
    }

    #[test]
    fn identical_calls_are_refused_then_stuck() {
        let replies = (0..10).map(|_| reply(vec![call("ping", "{\"a\":1}")]));
        let mut m = ScriptedModel::new(replies);
        let mut h = Echo { calls: vec![], group: false };
        let o = run(&mut m, &mut h, &AgentConfig::default());
        assert_eq!(h.calls.len(), 2);
        assert_eq!(o.stop, StopReason::Stuck);
    }

    #[test]
    fn tool_filter_hides_and_refuses() {
        let mut m = ScriptedModel::new([reply(vec![call("cfd_run", "{}")]), text("ok")]);
        let mut h = Echo { calls: vec![], group: false };
        let cfg = AgentConfig { tool_filter: Some(vec!["geo_*".into(), "ping".into()]), ..Default::default() };
        let o = run(&mut m, &mut h, &cfg);
        assert!(h.calls.is_empty());
        assert!(o.log[0].summary.contains("unknown tool"));
    }

    #[test]
    fn group_tools_bracket_the_run_and_are_hidden() {
        let mut m = ScriptedModel::new([reply(vec![call("ping", "{}")]), text("fin")]);
        let mut h = Echo { calls: vec![], group: true };
        let o = run(&mut m, &mut h, &AgentConfig::default());
        let names: Vec<&str> = h.calls.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(names, [BEGIN, "ping", END]);
        assert_eq!(o.final_text, "fin");
    }

    #[test]
    fn host_errors_reach_the_model() {
        let mut m = ScriptedModel::new([reply(vec![call("ping", "{\"fail\":1}")]), text("sorry")]);
        let mut h = Echo { calls: vec![], group: false };
        run(&mut m, &mut h, &AgentConfig::default());
        let Message::Tool { content, .. } = m.seen[1].last().unwrap() else { panic!() };
        assert_eq!(content, "error: nope");
    }

    #[test]
    fn old_exchanges_are_dropped_and_results_capped() {
        let replies = (0..8).map(|i| reply(vec![call("ping", &format!("{{\"i\":{i}}}"))])).chain([text("done")]);
        let mut m = ScriptedModel::new(replies);
        let mut h = Echo { calls: vec![], group: false };
        run(&mut m, &mut h, &AgentConfig { keep_turns: 2, ..Default::default() });
        let last = m.seen.last().unwrap();
        // system + goal + omitted note + 2 exchanges of (assistant + 1 result)
        assert_eq!(last.len(), 3 + 4);
        assert!(matches!(&last[2], Message::User(t) if t.contains("6 earlier steps omitted")));
        assert_eq!(clip(&"x".repeat(50), 10).len(), 10 + "...[40 more chars]".len());
    }

    #[test]
    fn text_fallback_variants() {
        let k = ["add_node", "cook"];
        let a = extract_text_calls("Sure!\n```json\n{\"name\": \"add_node\", \"arguments\": {\"type\": \"geo.box\"}}\n```", &k);
        assert_eq!((a[0].name.as_str(), a[0].arguments.as_str()), ("add_node", "{\"type\":\"geo.box\"}"));
        let b = extract_text_calls("[{\"tool\":\"add_node\",\"parameters\":{}}, {\"function\":{\"name\":\"cook\",\"arguments\":\"{\\\"node\\\":\\\"a\\\"}\"}}]", &k);
        assert_eq!(b.len(), 2);
        assert_eq!(b[1].arguments, "{\"node\":\"a\"}");
        assert!(extract_text_calls("use {\"name\":\"rm_rf\",\"arguments\":{}}", &k).is_empty());
        assert!(extract_text_calls("no json here {oops", &k).is_empty());
    }

    #[test]
    fn model_errors_and_empty_replies() {
        let mut m = ScriptedModel::new([]);
        let mut h = Echo { calls: vec![], group: false };
        assert!(matches!(run(&mut m, &mut h, &AgentConfig::default()).stop, StopReason::ModelError(_)));
        let mut m = ScriptedModel::new([text(""), text(""), text("")]);
        assert_eq!(run(&mut m, &mut h, &AgentConfig::default()).stop, StopReason::Stuck);
    }

    #[test]
    fn guide_falls_back_to_builtin() {
        assert_eq!(load_system_prompt(Some(Path::new("/definitely/not/here"))), BUILTIN_GUIDE);
    }
}

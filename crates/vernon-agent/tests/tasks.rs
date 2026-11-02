//! Recorded tasks: each `tests/tasks/*.json` has a user goal, the model
//! replies to replay, and the graph the mock host must end with.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use serde_json::{json, Value};
use vernon_agent::{run_agent, AgentConfig, ModelReply, ScriptedModel, StopReason, ToolCall, ToolHost, ToolSpec};

const TYPES: [&str; 6] = ["geo.box", "geo.sphere", "clean.smooth", "surface.wrap", "print.slice", "cfd.setup"];

#[derive(Default)]
struct MockHost {
    nodes: BTreeMap<String, String>,
    params: BTreeMap<String, Value>,
    edges: Vec<(String, String)>,
    groups_begun: usize,
    groups_ended: usize,
}

fn spec(name: &str, required: &[&str]) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: format!("mock {name}"),
        parameters: json!({"type": "object", "required": required}),
    }
}

fn arg<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    a.get(k).and_then(Value::as_str).ok_or_else(|| format!("missing string argument '{k}'"))
}

impl ToolHost for MockHost {
    fn tool_specs(&self) -> Vec<ToolSpec> {
        vec![
            spec("list_node_types", &[]),
            spec("add_node", &["type", "name"]),
            spec("set_param", &["node", "name", "value"]),
            spec("connect", &["from", "to"]),
            spec("cook", &["node"]),
            spec("begin_group", &[]),
            spec("end_group", &[]),
        ]
    }

    fn call(&mut self, name: &str, a: Value) -> Result<Value, String> {
        match name {
            "list_node_types" => Ok(json!(TYPES)),
            "add_node" => {
                let (ty, n) = (arg(&a, "type")?, arg(&a, "name")?);
                if !TYPES.contains(&ty) {
                    return Err(format!("unknown node type '{ty}'. Types: {}", TYPES.join(", ")));
                }
                if self.nodes.contains_key(n) {
                    return Err(format!("node '{n}' already exists"));
                }
                self.nodes.insert(n.into(), ty.into());
                Ok(json!({"name": n}))
            }
            "set_param" => {
                let n = arg(&a, "node")?;
                if !self.nodes.contains_key(n) {
                    return Err(format!("no node named '{n}'"));
                }
                self.params.insert(format!("{n}.{}", arg(&a, "name")?), a.get("value").cloned().ok_or("missing 'value'")?);
                Ok(json!("ok"))
            }
            "connect" => {
                let (f, t) = (arg(&a, "from")?, arg(&a, "to")?);
                for n in [f, t] {
                    if !self.nodes.contains_key(n) {
                        return Err(format!("no node named '{n}'"));
                    }
                }
                self.edges.push((f.into(), t.into()));
                Ok(json!("ok"))
            }
            "cook" => {
                let n = arg(&a, "node")?;
                if !self.nodes.contains_key(n) {
                    return Err(format!("no node named '{n}'"));
                }
                // Metrics are a pure function of the chain upstream of `n`.
                let mut chain = vec![n.to_string()];
                while let Some((f, _)) = self.edges.iter().find(|(_, t)| t == chain.last().unwrap()) {
                    chain.push(f.clone());
                }
                let size = chain.iter().find_map(|c| self.params.get(&format!("{c}.size_x"))).and_then(Value::as_f64).unwrap_or(100.0);
                Ok(json!({"metrics": {"upstream_nodes": chain.len(), "size_x": size}}))
            }
            "begin_group" => {
                self.groups_begun += 1;
                Ok(json!("ok"))
            }
            "end_group" => {
                self.groups_ended += 1;
                Ok(json!("ok"))
            }
            other => Err(format!("unknown tool {other}")),
        }
    }
}

fn reply_from(v: &Value) -> ModelReply {
    let tool_calls = v
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, t)| ToolCall {
                    id: format!("c{i}"),
                    name: t["name"].as_str().unwrap().to_string(),
                    arguments: match &t["arguments"] {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    },
                })
                .collect()
        })
        .unwrap_or_default();
    ModelReply { content: v.get("content").and_then(Value::as_str).unwrap_or("").to_string(), tool_calls }
}

fn run_task(path: &PathBuf) {
    let task: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let name = task["name"].as_str().unwrap();
    let mut cfg = AgentConfig::default();
    if let Some(c) = task.get("config") {
        if let Some(n) = c["max_steps"].as_u64() {
            cfg.max_steps = n as usize;
        }
        if let Some(f) = c.get("tool_filter") {
            cfg.tool_filter = Some(f.as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect());
        }
    }
    let mut model = ScriptedModel::new(task["replies"].as_array().unwrap().iter().map(reply_from));
    let mut host = MockHost::default();
    let out = run_agent(&mut model, &mut host, task["goal"].as_str().unwrap(), &cfg, &AtomicBool::new(false), &mut |_| {});
    let e = &task["expect"];

    let stop = e["stop"].as_str().unwrap_or("Done");
    let got = match &out.stop {
        StopReason::Done => "Done",
        StopReason::StepLimit => "StepLimit",
        StopReason::Cancelled => "Cancelled",
        StopReason::Stuck => "Stuck",
        StopReason::ModelError(_) => "ModelError",
    };
    assert_eq!(got, stop, "{name}: stop reason");
    if let Some(t) = e["final_contains"].as_str() {
        assert!(out.final_text.contains(t), "{name}: final text '{}' lacks '{t}'", out.final_text);
    }
    let want_nodes: BTreeMap<String, String> = serde_json::from_value(e.get("nodes").cloned().unwrap_or(json!({}))).unwrap();
    assert_eq!(host.nodes, want_nodes, "{name}: nodes");
    let want_params: BTreeMap<String, Value> = serde_json::from_value(e.get("params").cloned().unwrap_or(json!({}))).unwrap();
    assert_eq!(host.params, want_params, "{name}: params");
    let want_edges: Vec<(String, String)> = serde_json::from_value(e.get("edges").cloned().unwrap_or(json!([]))).unwrap();
    assert_eq!(host.edges, want_edges, "{name}: edges");
    let errors = out.log.iter().filter(|l| !l.ok).count() as u64;
    assert_eq!(errors, e["tool_errors"].as_u64().unwrap_or(0), "{name}: tool errors {:?}", out.log);
    if let Some(n) = e["calls"].as_u64() {
        assert_eq!(out.log.len() as u64, n, "{name}: calls");
    }
    if let Some(m) = e["last_cook"].as_object() {
        // Metrics the model last saw from cook: re-run it on the final host.
        let r = host.call("cook", json!({"node": m["node"]})).unwrap();
        for (k, v) in m["metrics"].as_object().unwrap() {
            assert_eq!(&r["metrics"][k], v, "{name}: metric {k}");
        }
    }
    // A host that offers groups always sees them balanced, one per run.
    assert_eq!((host.groups_begun, host.groups_ended), (1, 1), "{name}: undo group");
}

#[test]
fn recorded_tasks() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/tasks");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    files.sort();
    assert!(files.len() >= 12, "only {} recorded tasks", files.len());
    for f in &files {
        run_task(f);
    }
}

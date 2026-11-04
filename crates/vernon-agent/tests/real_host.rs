//! The agent loop against the real `GraphToolHost` (full node registry).

use std::sync::atomic::AtomicBool;

use serde_json::{json, Value};
use vernon_agent::{run_agent, ToolHost, AgentConfig, ModelReply, ScriptedModel, StopReason, ToolCall};
use vernon_agent_tools::GraphToolHost;

fn call(name: &str, args: Value) -> ToolCall {
    ToolCall { id: format!("id_{name}"), name: name.into(), arguments: args.to_string() }
}

fn calls(cs: Vec<ToolCall>) -> ModelReply {
    ModelReply { content: String::new(), tool_calls: cs }
}

fn text(s: &str) -> ModelReply {
    ModelReply { content: s.into(), tool_calls: vec![] }
}

fn host() -> GraphToolHost {
    GraphToolHost::new(vernon_cli::registry(), std::env::temp_dir().join(format!("vernon-agent-real-{}", std::process::id())))
}

fn run(h: &mut GraphToolHost, replies: Vec<ModelReply>) -> vernon_agent::AgentOutcome {
    let mut m = ScriptedModel::new(replies);
    run_agent(&mut m, h, "goal", &AgentConfig::default(), &AtomicBool::new(false), &mut |_| {})
}

#[test]
fn builds_and_cooks_a_chain_in_one_undo_group() {
    let mut h = host();
    let out = run(
        &mut h,
        vec![
            calls(vec![call("add_node", json!({"type_id": "geo.box", "name": "b"})), call("add_node", json!({"type_id": "geo.triangulate", "name": "t"}))]),
            calls(vec![call("connect", json!({"from": "b", "to": "t"})), call("set_param", json!({"node": "b", "param": "size", "value": [250, 100, 80]}))]),
            calls(vec![call("cook", json!({"node": "t"}))]),
            text("Built b -> t."),
        ],
    );
    assert_eq!(out.stop, StopReason::Done);
    assert!(out.log.iter().all(|l| l.ok), "{:?}", out.log);
    assert_eq!(h.graph().nodes.len(), 2);
    // The whole run is one group: a single undo removes everything.
    h.call("undo", json!({})).unwrap();
    assert_eq!(h.graph().nodes.len(), 0);
}

#[test]
fn real_errors_reach_the_model_and_it_recovers() {
    let mut h = host();
    let out = run(
        &mut h,
        vec![
            calls(vec![call("add_node", json!({"type_id": "geo.nonexistent"}))]),
            calls(vec![call("add_node", json!({"type_id": "geo.sphere", "name": "s"}))]),
            text("ok"),
        ],
    );
    assert_eq!(out.stop, StopReason::Done);
    assert_eq!(out.log.iter().filter(|l| !l.ok).count(), 1);
    assert!(h.graph().find("s").is_some());
}

#[test]
fn text_fallback_drives_the_real_host() {
    let mut h = host();
    let out = run(
        &mut h,
        vec![text("```json\n{\"name\": \"add_node\", \"arguments\": {\"type_id\": \"geo.box\", \"name\": \"fromtext\"}}\n```"), text("done")],
    );
    assert!(out.log[0].ok, "{:?}", out.log);
    assert!(h.graph().find("fromtext").is_some());
}

#[test]
fn a_failed_run_with_no_changes_leaves_no_undo_entry() {
    let mut h = host();
    run(&mut h, vec![calls(vec![call("add_node", json!({"type_id": "nope"}))]), text("gave up")]);
    assert!(h.call("undo", json!({})).is_err(), "a run that changed nothing adds no undo step");
}

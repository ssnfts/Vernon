//! The MCP server driven by scripted JSON-RPC transcripts, no model involved.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use vernon_agent_tools::GraphToolHost;
use vernon_cli::mcp::McpServer;

fn work() -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("vernon_mcp_{}_{nanos}", std::process::id()))
}

struct Client {
    server: McpServer<GraphToolHost>,
    next: u64,
}

impl Client {
    fn new() -> Self {
        Self { server: McpServer::new(GraphToolHost::new(vernon_cli::registry(), work())), next: 1 }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        let reply: Value = serde_json::from_str(&self.server.handle_line(&line).expect("a reply")).unwrap();
        assert_eq!(reply["jsonrpc"], "2.0");
        assert_eq!(reply["id"], id);
        reply
    }

    /// `tools/call`: the parsed text payload and the isError flag.
    fn call(&mut self, name: &str, args: Value) -> (Value, bool) {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}));
        let res = &r["result"];
        let text = res["content"][0]["text"].as_str().unwrap_or_else(|| panic!("no text in {r}")).to_string();
        let is_err = res["isError"].as_bool().unwrap();
        (serde_json::from_str(&text).unwrap_or(Value::String(text)), is_err)
    }

    fn ok(&mut self, name: &str, args: Value) -> Value {
        let (v, e) = self.call(name, args);
        assert!(!e, "{name} failed: {v}");
        v
    }
}

#[test]
fn full_session_transcript() {
    let mut c = Client::new();

    let init = c.request("initialize", json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}));
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(init["result"]["serverInfo"]["name"], "vernon");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    // a notification gets no reply
    assert!(c.server.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    assert_eq!(c.request("ping", json!({}))["result"], json!({}));

    let list = c.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().unwrap();
    assert!(tools.len() >= 20);
    for t in tools {
        assert!(t["name"].is_string() && t["description"].is_string());
        assert_eq!(t["inputSchema"]["type"], "object");
    }
    assert!(tools.iter().any(|t| t["name"] == "cook"));

    // A 3-node chain: panel -> move -> build volume check (a 900 mm panel cannot fit a 300 mm bed).
    c.ok("add_node", json!({"type_id": "geo.car_panel", "name": "panel"}));
    c.ok("add_node", json!({"type_id": "geo.transform", "name": "move", "params": {"translate": [0, 0, 5]}}));
    c.ok("add_node", json!({"type_id": "print.build_fit", "name": "fit", "params": {"printer": "bed_300"}}));
    c.ok("connect", json!({"from": "panel", "to": "move"}));
    let wired = c.ok("connect", json!({"from": "move", "to": "fit"}));
    assert_eq!(wired["connected"], "move.geometry -> fit.parts");

    let cooked = c.ok("cook", json!({"node": "fit"}));
    assert_eq!(cooked["cooked"][0]["status"], "ok", "{cooked}");
    assert!(cooked["failing_gate_count"].as_u64().unwrap() >= 1, "{cooked}");
    let gate = &cooked["failed_gates"][0];
    assert_eq!(gate["node"], "fit");
    let metric = gate["metric"].as_str().unwrap().to_string();
    let m = c.ok("get_metric", json!({"node": "fit", "metric": metric}));
    assert_eq!(m["passes"], false);
    let gates = c.ok("list_gates", json!({}));
    assert!(gates["failing"].as_u64().unwrap() >= 1);

    // undo the last change (the second connect), then redo it
    c.ok("undo", json!({}));
    let g = c.ok("get_graph", json!({}));
    assert!(g["nodes"][2]["inputs"][0]["from"].is_null());
    c.ok("redo", json!({}));
    let g = c.ok("get_graph", json!({}));
    assert_eq!(g["nodes"][2]["inputs"][0]["from"], "move.geometry");

    // path traversal and absolute paths are refused, as tool errors
    for bad in ["../escape.vrn", "/etc/x.vrn", "C:\\x.vrn"] {
        let (e, is_err) = c.call("save_graph", json!({"path": bad}));
        assert!(is_err, "{bad} must be refused");
        assert!(e.as_str().unwrap().contains("relative") || e.as_str().unwrap().contains("leaves"), "{e}");
    }
    c.ok("save_graph", json!({"path": "chain.vrn"}));
}

#[test]
fn protocol_errors_and_version_negotiation() {
    let mut c = Client::new();
    // unknown requested version: the server answers with its newest
    let init = c.request("initialize", json!({"protocolVersion": "1999-01-01"}));
    assert_eq!(init["result"]["protocolVersion"], vernon_cli::mcp::SUPPORTED_VERSIONS[0]);
    let init = c.request("initialize", json!({"protocolVersion": "2024-11-05"}));
    assert_eq!(init["result"]["protocolVersion"], "2024-11-05");

    assert_eq!(c.request("nope/nothing", json!({}))["error"]["code"], -32601);
    assert_eq!(c.request("tools/call", json!({}))["error"]["code"], -32602);
    assert_eq!(c.request("tools/call", json!({"name": "rm_rf"}))["error"]["code"], -32602);

    let bad: Value = serde_json::from_str(&c.server.handle_line("{not json").unwrap()).unwrap();
    assert_eq!(bad["error"]["code"], -32700);
    assert!(bad["id"].is_null());
    let bad: Value = serde_json::from_str(&c.server.handle_line(r#"{"jsonrpc":"2.0","id":9}"#).unwrap()).unwrap();
    assert_eq!(bad["error"]["code"], -32600);
    assert!(c.server.handle_line("   ").is_none());

    // a tool error is a normal result with isError set and a corrective message
    let (e, is_err) = c.call("add_node", json!({"type_id": "clean.icp_alignn"}));
    assert!(is_err);
    assert!(e.as_str().unwrap().contains("did you mean") || e.as_str().unwrap().contains("unknown node type"));
    // missing arguments object is fine for tools that need none
    let r = c.request("tools/call", json!({"name": "get_graph"}));
    assert_eq!(r["result"]["isError"], false);
}

#[test]
fn batches_are_answered_in_one_array() {
    let mut c = Client::new();
    let out = c
        .server
        .handle_line(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"},{"jsonrpc":"2.0","id":2,"method":"ping"}]"#)
        .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 2);
}

#[test]
fn the_real_binary_speaks_mcp_over_stdio() {
    let dir = work();
    let mut child = Command::new(env!("CARGO_BIN_EXE_vernon-cli"))
        .args(["mcp", "--work"])
        .arg(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut ask = |v: Value| -> Value {
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("stdout must carry only JSON lines: {e}: {line:?}"))
    };
    let r = ask(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}));
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    let r = ask(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "list_nodes", "arguments": {"stage": "perf"}}}));
    assert_eq!(r["result"]["isError"], false);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

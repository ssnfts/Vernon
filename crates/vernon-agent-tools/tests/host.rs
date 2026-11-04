//! Tool-layer tests with a tiny fake registry: no model, no node crates.

use serde_json::{json, Value};
use vernon_agent_tools::{GraphToolHost, ToolHost};
use vernon_core::{DVec3, Geometry, Metric, Report};
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

struct Src;
impl NodeOp for Src {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.src", "Src", Stage::Geometry)
            .output(PortSpec::new("geo", DataKind::Geometry))
            .param(ParamSpec::float("size", "Size", 10.0, 1.0, 100.0).unit("mm"))
            .param(ParamSpec::int("n", "N", 1, 1, 5))
            .param(ParamSpec::choice("mode", "Mode", "a", &["a", "b"]))
            .param(ParamSpec::vec3("at", "At", DVec3::ZERO))
            .param(ParamSpec::path("file", "File", &["stl"], false))
            .implemented()
    }
    fn cook(&self, _: &CookContext, _: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        Ok(vec![Data::geometry(Geometry::make_box(DVec3::splat(p.float("size")?)))])
    }
}

struct Gate;
impl NodeOp for Gate {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("perf.gate", "Gate", Stage::Performance)
            .input(PortSpec::new("geo", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("limit", "Limit", 5.0, 0.0, 1000.0))
            .param(ParamSpec::bool("run", "Run", false).action())
            .external("SomeSolver")
            .implemented()
    }
    fn cook(&self, _: &CookContext, i: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let g = vernon_graph::node::inputs::geometry(i, 0)?;
        let mut r = Report::new("gate");
        r.metric("width", Metric::new(g.bounds().size().x, "mm").with_limit(p.float("limit")?, true));
        Ok(vec![Data::report(r)])
    }
}

struct Pass;
impl NodeOp for Pass {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.pass", "Pass", Stage::Geometry)
            .input(PortSpec::new("geo", DataKind::Geometry))
            .output(PortSpec::new("geo", DataKind::Geometry))
            .implemented()
    }
    fn cook(&self, _: &CookContext, i: &[Option<Data>], _: &Params) -> CookResult<Vec<Data>> {
        Ok(vec![Data::Geometry(vernon_graph::node::inputs::geometry(i, 0)?)])
    }
}

fn host() -> (GraphToolHost, std::path::PathBuf) {
    let mut reg = Registry::new();
    reg.add(Src);
    reg.add(Gate);
    reg.add(Pass);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("vernon_tools_{}_{nanos}", std::process::id())).join("work");
    (GraphToolHost::new(reg, &dir), dir)
}

fn ok(h: &mut GraphToolHost, t: &str, a: Value) -> Value {
    h.call(t, a).unwrap_or_else(|e| panic!("{t} failed: {e}"))
}

fn err(h: &mut GraphToolHost, t: &str, a: Value) -> String {
    h.call(t, a).expect_err("expected an error")
}

fn names(h: &mut GraphToolHost) -> Vec<String> {
    ok(h, "get_graph", json!({}))["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn specs_are_complete_json_schema_objects() {
    let (h, _) = host();
    let specs = h.tool_specs();
    for want in [
        "list_nodes", "describe_node", "add_node", "set_param", "connect", "disconnect", "delete_node", "move_node", "rename_node", "cook",
        "read_report", "get_metric", "list_gates", "measure_geometry", "undo", "redo", "begin_group", "end_group", "load_graph", "save_graph",
    ] {
        assert!(specs.iter().any(|s| s.name == want), "missing tool {want}");
    }
    for s in &specs {
        assert_eq!(s.parameters["type"], "object", "{}", s.name);
        assert!(!s.description.is_empty());
    }
}

#[test]
fn build_cook_and_read_a_failed_gate() {
    let (mut h, _) = host();
    ok(&mut h, "add_node", json!({"type_id": "geo.src", "name": "s", "params": {"size": 12.5}}));
    ok(&mut h, "add_node", json!({"type_id": "perf.gate", "name": "g", "params": {"limit": 5}}));
    let c = ok(&mut h, "connect", json!({"from": "s", "to": "g"}));
    assert_eq!(c["connected"], "s.geo -> g.geo");
    let r = ok(&mut h, "cook", json!({"node": "g"}));
    assert_eq!(r["failing_gate_count"], 1);
    assert_eq!(r["failed_gates"][0]["metric"], "width");
    assert_eq!(r["reports"][0]["metrics"]["width"]["passes"], false);
    let m = ok(&mut h, "get_metric", json!({"node": "g", "metric": "width"}));
    assert_eq!(m["value"], 12.5);
    assert_eq!(m["passes"], false);
    assert!(err(&mut h, "get_metric", json!({"node": "g", "metric": "nope"})).contains("available: width"));
    ok(&mut h, "set_param", json!({"node": "g", "param": "limit", "value": 20}));
    let gates = ok(&mut h, "list_gates", json!({}));
    assert_eq!(gates["failing"], 0);
    assert_eq!(gates["gates"][0]["state"], "pass");
    let rep = ok(&mut h, "read_report", json!({"node": "g"}));
    assert_eq!(rep["reports"][0]["title"], "gate");
}

#[test]
fn measure_geometry_reports_bounds_area_and_watertight() {
    let (mut h, _) = host();
    ok(&mut h, "add_node", json!({"type_id": "geo.src", "name": "s", "params": {"size": 10}}));
    let m = ok(&mut h, "measure_geometry", json!({"node": "s"}));
    assert_eq!(m["watertight"], true);
    assert_eq!(m["size_mm"], json!([10.0, 10.0, 10.0]));
    assert!((m["area_mm2"].as_f64().unwrap() - 600.0).abs() < 1e-6);
}

#[test]
fn bad_calls_get_corrective_errors() {
    let (mut h, _) = host();
    assert!(err(&mut h, "frobnicate", json!({})).contains("unknown tool"));
    assert!(err(&mut h, "add_node", json!({"type_id": "geo.sorce"})).contains("unknown node type"));
    assert!(err(&mut h, "add_node", json!({"type_id": "geo.src", "bogus": 1})).contains("unknown argument 'bogus'"));
    assert!(err(&mut h, "add_node", json!({})).contains("needs argument 'type_id'"));
    ok(&mut h, "add_node", json!({"type_id": "geo.src", "name": "s"}));
    ok(&mut h, "add_node", json!({"type_id": "perf.gate", "name": "g"}));
    assert!(err(&mut h, "set_param", json!({"node": "zz", "param": "size", "value": 1})).contains("existing nodes: s, g"));
    assert!(err(&mut h, "set_param", json!({"node": "s", "param": "sise", "value": 1})).contains("parameters: size, n, mode, at, file"));
    let e = err(&mut h, "set_param", json!({"node": "s", "param": "size", "value": 1000}));
    assert!(e.contains("between 1 and 100 mm"), "{e}");
    assert!(err(&mut h, "set_param", json!({"node": "s", "param": "n", "value": 2.5})).contains("integer"));
    assert!(err(&mut h, "set_param", json!({"node": "s", "param": "mode", "value": "c"})).contains("one of: a, b"));
    assert!(err(&mut h, "set_param", json!({"node": "s", "param": "at", "value": [1, 2]})).contains("[x, y, z]"));
    ok(&mut h, "connect", json!({"from": "s", "to": "g"}));
    ok(&mut h, "add_node", json!({"type_id": "geo.pass", "name": "p"}));
    let e = err(&mut h, "connect", json!({"from": "g", "to": "p", "to_input": 0}));
    assert!(e.contains("cannot connect"), "{e}");
    assert!(err(&mut h, "connect", json!({"from": "g", "to": "p"})).contains("no free input"));
    assert!(err(&mut h, "connect", json!({"from": "s", "to": "g", "to_input": 3})).contains("no input 3"));
    ok(&mut h, "connect", json!({"from": "s", "to": "p"}));
    ok(&mut h, "add_node", json!({"type_id": "geo.pass", "name": "q"}));
    ok(&mut h, "connect", json!({"from": "p", "to": "q"}));
    assert!(err(&mut h, "connect", json!({"from": "q", "to": "p", "to_input": 0})).contains("cycle"));
    assert!(err(&mut h, "rename_node", json!({"node": "q", "new_name": "p"})).contains("already exists"));
    assert!(err(&mut h, "rename_node", json!({"node": "q", "new_name": "../x"})).contains("not allowed"));
    assert!(err(&mut h, "disconnect", json!({"node": "s", "input": 0})).contains("no input 0"));
}

#[test]
fn path_params_must_be_relative_and_run_flag_is_announced() {
    let (mut h, _) = host();
    ok(&mut h, "add_node", json!({"type_id": "geo.src", "name": "s"}));
    for bad in ["C:\\Windows\\x.stl", "/etc/passwd", "../x.stl", "a/../../x.stl", "\\\\srv\\share\\x.stl"] {
        let e = err(&mut h, "set_param", json!({"node": "s", "param": "file", "value": bad}));
        assert!(e.contains("plain relative path"), "{bad}: {e}");
    }
    ok(&mut h, "set_param", json!({"node": "s", "param": "file", "value": "scans/x.stl"}));
    ok(&mut h, "add_node", json!({"type_id": "perf.gate", "name": "g"}));
    let g = ok(&mut h, "get_graph", json!({}));
    assert_eq!(g["nodes"][1]["params"]["run"], false, "run defaults to false");
    let r = ok(&mut h, "set_param", json!({"node": "g", "param": "run", "value": true}));
    assert!(r["warnings"][0].as_str().unwrap().contains("SomeSolver"));
    ok(&mut h, "connect", json!({"from": "s", "to": "g"}));
    let c = ok(&mut h, "cook", json!({"node": "g"}));
    assert_eq!(c["external_runs_requested"][0], "g.run");
    assert!(c["note"].as_str().unwrap().contains("switch"));
    let g = ok(&mut h, "get_graph", json!({}));
    assert_eq!(g["nodes"][1]["params"]["run"], false, "run switches off after a cook");
}

#[test]
fn every_mutation_is_one_undo_step_and_redo_replays() {
    let (mut h, _) = host();
    ok(&mut h, "add_node", json!({"type_id": "geo.src", "name": "s"}));
    ok(&mut h, "set_param", json!({"node": "s", "param": "size", "value": 20}));
    ok(&mut h, "rename_node", json!({"node": "s", "new_name": "box"}));
    assert_eq!(names(&mut h), ["box"]);
    ok(&mut h, "undo", json!({}));
    assert_eq!(names(&mut h), ["s"]);
    ok(&mut h, "undo", json!({}));
    assert_eq!(ok(&mut h, "get_graph", json!({}))["nodes"][0]["params"]["size"], 10.0);
    ok(&mut h, "redo", json!({}));
    assert_eq!(ok(&mut h, "get_graph", json!({}))["nodes"][0]["params"]["size"], 20.0);
    ok(&mut h, "undo", json!({}));
    ok(&mut h, "undo", json!({}));
    assert!(names(&mut h).is_empty());
    assert!(err(&mut h, "undo", json!({})).contains("nothing to undo"));
    ok(&mut h, "redo", json!({}));
    let before = h.can_undo();
    err(&mut h, "set_param", json!({"node": "s", "param": "size", "value": 5000}));
    assert_eq!(h.can_undo(), before);
}

#[test]
fn a_group_is_a_single_undo_step() {
    let (mut h, _) = host();
    ok(&mut h, "begin_group", json!({}));
    ok(&mut h, "add_node", json!({"type_id": "geo.src", "name": "s"}));
    ok(&mut h, "add_node", json!({"type_id": "perf.gate", "name": "g"}));
    ok(&mut h, "connect", json!({"from": "s", "to": "g"}));
    assert!(err(&mut h, "undo", json!({})).contains("end_group"));
    assert!(err(&mut h, "begin_group", json!({})).contains("already open"));
    let e = ok(&mut h, "end_group", json!({}));
    assert_eq!(e["changed"], true);
    ok(&mut h, "undo", json!({}));
    assert_eq!(ok(&mut h, "get_graph", json!({}))["count"], 0);
    assert!(err(&mut h, "undo", json!({})).contains("nothing to undo"));
    assert!(err(&mut h, "end_group", json!({})).contains("no group"));
    ok(&mut h, "redo", json!({}));
    assert_eq!(ok(&mut h, "get_graph", json!({}))["count"], 2);
}

#[test]
fn save_and_load_stay_inside_the_work_folder() {
    let (mut h, dir) = host();
    ok(&mut h, "add_node", json!({"type_id": "geo.src", "name": "s"}));
    ok(&mut h, "save_graph", json!({"path": "a/scene.vrn"}));
    assert!(dir.join("a").join("scene.vrn").exists());
    for bad in ["../evil.vrn", "/abs.vrn", "C:\\evil.vrn", "a/../../evil.vrn", "\\\\host\\s\\x.vrn", "no_extension", "x.txt"] {
        for tool in ["save_graph", "load_graph"] {
            let e = err(&mut h, tool, json!({"path": bad}));
            assert!(e.contains("relative") || e.contains(".vrn") || e.contains("leaves"), "{tool} {bad}: {e}");
        }
    }
    assert!(!dir.parent().unwrap().join("evil.vrn").exists());
    ok(&mut h, "delete_node", json!({"node": "s"}));
    let l = ok(&mut h, "load_graph", json!({"path": "a/scene.vrn"}));
    assert_eq!(l["nodes"], 1);
    ok(&mut h, "undo", json!({}));
    assert_eq!(ok(&mut h, "get_graph", json!({}))["count"], 0, "load is undoable");
    assert!(err(&mut h, "load_graph", json!({"path": "missing.vrn"})).contains("cannot read"));
}

#[test]
fn loading_holds_nodes_with_unsafe_paths_and_switches_run_off() {
    let (mut h, dir) = host();
    std::fs::create_dir_all(&dir).unwrap();
    let vrn = json!({"nodes": {
        "0": {"type_id": "geo.src", "name": "s", "params": {"file": "C:/secret/x.stl", "size": 5000.0}},
        "1": {"type_id": "perf.gate", "name": "g", "inputs": [[0, 0]], "params": {"run": true}}
    }});
    std::fs::write(dir.join("bad.vrn"), vrn.to_string()).unwrap();
    let l = ok(&mut h, "load_graph", json!({"path": "bad.vrn"}));
    let w = l["warnings"].to_string();
    assert!(w.contains("will not cook") && w.contains("clamped") && w.contains("switched off"), "{w}");
    let c = ok(&mut h, "cook", json!({"node": "s"}));
    assert_eq!(c["cooked"][0]["status"], "failed");
}

#[test]
fn list_and_describe_nodes() {
    let (mut h, _) = host();
    let all = ok(&mut h, "list_nodes", json!({}));
    assert_eq!(all["count"], 3);
    let perf = ok(&mut h, "list_nodes", json!({"stage": "perf"}));
    assert_eq!(perf["nodes"][0]["id"], "perf.gate");
    assert!(err(&mut h, "list_nodes", json!({"stage": "zzz"})).contains("use one of"));
    let d = ok(&mut h, "describe_node", json!({"type_id": "geo.src"}));
    assert_eq!(d["params"][0]["max"], 100.0);
    assert_eq!(d["params"][0]["unit"], "mm");
    assert_eq!(d["params"][2]["options"], json!(["a", "b"]));
    assert_eq!(ok(&mut h, "describe_node", json!({"type_id": "perf.gate"}))["external_program"], "SomeSolver");
}

#[test]
fn delete_cuts_wires_and_move_sets_position() {
    let (mut h, _) = host();
    ok(&mut h, "add_node", json!({"type_id": "geo.src", "name": "s"}));
    ok(&mut h, "add_node", json!({"type_id": "geo.pass", "name": "p", "x": 3, "y": 4}));
    ok(&mut h, "connect", json!({"from": "s", "to": "p"}));
    ok(&mut h, "move_node", json!({"node": "p", "x": 100, "y": 50}));
    assert_eq!(ok(&mut h, "get_graph", json!({}))["nodes"][1]["pos"], json!([100.0, 50.0]));
    ok(&mut h, "delete_node", json!({"node": "s"}));
    assert!(ok(&mut h, "get_graph", json!({}))["nodes"][0]["inputs"][0]["from"].is_null());
}

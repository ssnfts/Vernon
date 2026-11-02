use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Map, Value};
use vernon_core::{Report, Severity};
use vernon_graph::param::{clamp_to_specs, out_of_range};
use vernon_graph::{
    validate_node_name, CookError, Data, Graph, NodeId, NodeTypeInfo, ParamKind, ParamSpec, ParamValue, PortSpec, Registry, Stage,
};

use crate::{measure, schema, ToolHost, ToolSpec};

const UNDO_LIMIT: usize = 200;

/// Tool host over one graph. Owns the graph, the registry and the work
/// folder; everything the agent reads or writes lives under that folder.
pub struct GraphToolHost {
    reg: Registry,
    graph: Graph,
    work_dir: PathBuf,
    undo: Vec<String>,
    redo: Vec<String>,
    /// `(snapshot before the group, whether anything changed)`.
    group: Option<(String, bool)>,
}

impl GraphToolHost {
    /// An empty graph. `work_dir` is the only place files are read or written
    /// (it is created on first use); node scratch folders go in `work_dir/work`.
    pub fn new(reg: Registry, work_dir: impl Into<PathBuf>) -> Self {
        let mut host = Self {
            reg,
            graph: Graph::new(),
            work_dir: work_dir.into(),
            undo: vec![],
            redo: vec![],
            group: None,
        };
        host.wire_graph();
        host
    }

    /// Host over an existing graph (the app's current scene), keeping its cache.
    pub fn with_graph(reg: Registry, graph: Graph, work_dir: impl Into<PathBuf>) -> Self {
        let mut host = Self::new(reg, work_dir);
        host.graph = graph;
        host.wire_graph();
        host
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    pub fn registry(&self) -> &Registry {
        &self.reg
    }

    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    fn wire_graph(&mut self) {
        self.graph.scene_dir = Some(self.work_dir.clone());
        self.graph.work_root = Some(self.work_dir.join("work"));
    }

    // ------------------------------------------------------------ undo

    /// Run an edit as one undo step (or part of the open group). A failed
    /// edit leaves the graph as it was.
    fn mutate<R>(&mut self, edit: impl FnOnce(&mut Self) -> Result<R, String>) -> Result<R, String> {
        let snap = self.graph.to_json();
        match edit(self) {
            Ok(r) => {
                if self.graph.to_json() != snap {
                    self.record(snap);
                }
                Ok(r)
            }
            Err(e) => {
                if self.graph.to_json() != snap {
                    self.restore(&snap);
                }
                Err(e)
            }
        }
    }

    fn record(&mut self, snap: String) {
        if let Some((_, changed)) = &mut self.group {
            *changed = true;
            return;
        }
        self.push_undo(snap);
    }

    fn push_undo(&mut self, snap: String) {
        self.undo.push(snap);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Like the app: a restored graph keeps the old cooked results for nodes
    /// that still exist, and never has a one-shot action switched on.
    fn restore(&mut self, json: &str) {
        let Ok(mut g) = Graph::from_json(json) else { return };
        g.work_root = self.graph.work_root.clone();
        g.scene_dir = self.graph.scene_dir.clone();
        g.held = self.graph.held.clone();
        let old = std::mem::replace(&mut self.graph, g);
        self.graph.absorb_cache(old);
        self.reset_actions(None);
    }

    /// Switch every one-shot action parameter off (of `only` nodes, or all).
    /// Returns `node.param` of those that were on.
    fn reset_actions(&mut self, only: Option<&HashSet<NodeId>>) -> Vec<String> {
        let mut was_on = vec![];
        let ids: Vec<NodeId> = self.graph.nodes.keys().copied().collect();
        for id in ids {
            if only.is_some_and(|o| !o.contains(&id)) {
                continue;
            }
            let node = &self.graph.nodes[&id];
            let Some(info) = self.reg.info(&node.type_id) else { continue };
            let name = node.name.clone();
            let on: Vec<&ParamSpec> = info
                .params
                .iter()
                .filter(|s| s.action && node.params.get(&s.name) != Some(&s.default_value()))
                .collect();
            for spec in on {
                was_on.push(format!("{name}.{}", spec.name));
                let _ = self.graph.set_param(id, &spec.name, spec.default_value());
            }
        }
        was_on
    }

    // ------------------------------------------------------------ lookups

    fn info_of(&self, id: NodeId) -> Result<&NodeTypeInfo, String> {
        let n = &self.graph.nodes[&id];
        self.reg.info(&n.type_id).ok_or_else(|| format!("node '{}' has unknown type '{}'", n.name, n.type_id))
    }

    fn node_list(&self) -> String {
        if self.graph.nodes.is_empty() {
            "the graph is empty (use add_node)".into()
        } else {
            format!("existing nodes: {}", self.graph.nodes.values().map(|n| n.name.as_str()).collect::<Vec<_>>().join(", "))
        }
    }

    fn node_id(&self, args: &Value, key: &str) -> Result<NodeId, String> {
        let v = args.get(key).ok_or_else(|| format!("missing argument '{key}' (a node name)"))?;
        let name = v.as_str().ok_or_else(|| format!("argument '{key}' must be a node name string"))?;
        self.graph
            .find(name)
            .ok_or_else(|| format!("no node named '{name}'; {}", self.node_list()))
    }

    fn type_info(&self, type_id: &str) -> Result<&NodeTypeInfo, String> {
        self.reg.info(type_id).ok_or_else(|| {
            let tail = type_id.rsplit('.').next().unwrap_or(type_id);
            let similar: Vec<&str> = self
                .reg
                .types()
                .filter(|i| i.id.contains(tail) || (!tail.is_empty() && tail.contains(i.id.rsplit('.').next().unwrap_or(""))))
                .map(|i| i.id.as_str())
                .take(8)
                .collect();
            if similar.is_empty() {
                format!("unknown node type '{type_id}'; call list_nodes to see valid ids")
            } else {
                format!("unknown node type '{type_id}'; did you mean: {}? (list_nodes shows all)", similar.join(", "))
            }
        })
    }

    // ------------------------------------------------------------ dispatch

    fn check_args(&self, name: &str, args: &Value) -> Result<(), String> {
        let spec = schema::all()
            .into_iter()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("unknown tool '{name}'; tools: {}", schema::all().iter().map(|s| s.name.clone()).collect::<Vec<_>>().join(", ")))?;
        let obj = args.as_object().ok_or("arguments must be a JSON object")?;
        let props = spec.parameters["properties"].as_object().cloned().unwrap_or_default();
        if let Some(k) = obj.keys().find(|k| !props.contains_key(*k)) {
            let valid: Vec<&String> = props.keys().collect();
            return Err(format!("unknown argument '{k}' for {name}; valid arguments: {}", if valid.is_empty() { "none".into() } else { valid.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ") }));
        }
        for r in spec.parameters["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            if !obj.contains_key(r) {
                return Err(format!("{name} needs argument '{r}'"));
            }
        }
        Ok(())
    }

    fn dispatch(&mut self, name: &str, args: Value) -> Result<Value, String> {
        match name {
            "list_nodes" => self.list_nodes(&args),
            "describe_node" => {
                let t = str_arg(&args, "type_id")?;
                Ok(describe(self.type_info(&t)?))
            }
            "get_graph" => Ok(self.get_graph()),
            "add_node" => self.mutate(|h| h.add_node(&args)),
            "set_param" => self.mutate(|h| h.set_param(&args)),
            "connect" => self.mutate(|h| h.connect(&args)),
            "disconnect" => self.mutate(|h| h.disconnect(&args)),
            "delete_node" => self.mutate(|h| {
                let id = h.node_id(&args, "node")?;
                let n = h.graph.nodes[&id].name.clone();
                h.graph.remove_node(id);
                h.graph.held.remove(&id);
                Ok(json!({"deleted": n}))
            }),
            "move_node" => self.mutate(|h| {
                let id = h.node_id(&args, "node")?;
                let (x, y) = (num_arg(&args, "x")?, num_arg(&args, "y")?);
                h.graph.nodes.get_mut(&id).expect("exists").pos = [x as f32, y as f32];
                Ok(json!({"node": h.graph.nodes[&id].name, "x": x, "y": y}))
            }),
            "rename_node" => self.mutate(|h| {
                let id = h.node_id(&args, "node")?;
                let new = str_arg(&args, "new_name")?;
                validate_node_name(&new)?;
                if h.graph.nodes[&id].name == new {
                    return Err(format!("node is already named '{new}'"));
                }
                if h.graph.find(&new).is_some() {
                    return Err(format!("a node named '{new}' already exists; pick another name"));
                }
                let old = std::mem::replace(&mut h.graph.nodes.get_mut(&id).expect("exists").name, new.clone());
                Ok(json!({"renamed": old, "to": new}))
            }),
            "cook" => {
                let ids = match args.get("node") {
                    Some(_) => vec![self.node_id(&args, "node")?],
                    None => self.graph.nodes.keys().copied().collect(),
                };
                Ok(self.do_cook(&ids, args.get("node").is_some()).0)
            }
            "read_report" => {
                let id = self.node_id(&args, "node")?;
                let (_, outs) = self.do_cook(&[id], true);
                match outs.into_iter().next().map(|o| o.1) {
                    Some(Ok(data)) => {
                        let reports: Vec<Value> = data.iter().filter_map(|d| d.as_report()).map(|r| report_json(r)).collect();
                        if reports.is_empty() {
                            Err(format!("node '{}' produced no report; use measure_geometry for geometry or cook to see its outputs", self.graph.nodes[&id].name))
                        } else {
                            Ok(json!({"node": self.graph.nodes[&id].name, "reports": reports}))
                        }
                    }
                    Some(Err(e)) => Err(format!("node '{}' failed to cook: {e}", self.graph.nodes[&id].name)),
                    None => Err("nothing cooked".into()),
                }
            }
            "get_metric" => self.get_metric(&args),
            "list_gates" => self.list_gates(&args),
            "measure_geometry" => self.measure_geometry(&args),
            "undo" => {
                if self.group.is_some() {
                    return Err("end_group before undo".into());
                }
                let s = self.undo.pop().ok_or("nothing to undo")?;
                self.redo.push(self.graph.to_json());
                self.restore(&s);
                Ok(json!({"undone": true, "undo_left": self.undo.len(), "redo_left": self.redo.len()}))
            }
            "redo" => {
                if self.group.is_some() {
                    return Err("end_group before redo".into());
                }
                let s = self.redo.pop().ok_or("nothing to redo")?;
                self.undo.push(self.graph.to_json());
                self.restore(&s);
                Ok(json!({"redone": true, "undo_left": self.undo.len(), "redo_left": self.redo.len()}))
            }
            "begin_group" => {
                if self.group.is_some() {
                    return Err("a group is already open; call end_group first".into());
                }
                self.group = Some((self.graph.to_json(), false));
                Ok(json!({"group": "open"}))
            }
            "end_group" => {
                let (snap, changed) = self.group.take().ok_or("no group is open (call begin_group first)")?;
                if changed {
                    self.push_undo(snap);
                }
                Ok(json!({"group": "closed", "changed": changed}))
            }
            "load_graph" => self.mutate(|h| h.load_graph(&args)),
            "save_graph" => self.save_graph(&args),
            _ => unreachable!("checked by check_args"),
        }
    }

    // ------------------------------------------------------------ tools

    fn list_nodes(&self, args: &Value) -> Result<Value, String> {
        let stage = match args.get("stage").and_then(Value::as_str) {
            None => None,
            Some(s) => Some(parse_stage(s)?),
        };
        let rows: Vec<Value> = self
            .reg
            .types()
            .filter(|i| stage.is_none_or(|s| i.stage == s))
            .map(|i| json!({"id": i.id, "label": i.label, "stage": stage_prefix(i.stage), "help": i.help, "placeholder": i.whitebox}))
            .collect();
        Ok(json!({"count": rows.len(), "nodes": rows}))
    }

    fn get_graph(&self) -> Value {
        let nodes: Vec<Value> = self
            .graph
            .nodes
            .values()
            .map(|n| {
                let info = self.reg.info(&n.type_id);
                let inputs: Vec<Value> = n
                    .inputs
                    .iter()
                    .enumerate()
                    .map(|(i, src)| {
                        let port = info.and_then(|f| f.inputs.get(i)).map(|p| p.name.clone()).unwrap_or_default();
                        let from = src.and_then(|(s, o)| {
                            let sn = self.graph.nodes.get(&s)?;
                            let on = self.reg.info(&sn.type_id).and_then(|f| f.outputs.get(o)).map(|p| p.name.clone()).unwrap_or_default();
                            Some(format!("{}.{on}", sn.name))
                        });
                        json!({"index": i, "port": port, "from": from})
                    })
                    .collect();
                json!({
                    "name": n.name, "type": n.type_id, "inputs": inputs,
                    "params": serde_json::to_value(&n.params.0).unwrap_or(Value::Null),
                    "pos": n.pos, "bypass": n.bypass,
                })
            })
            .collect();
        json!({"nodes": nodes, "count": nodes.len()})
    }

    fn add_node(&mut self, args: &Value) -> Result<Value, String> {
        let type_id = str_arg(args, "type_id")?;
        let info = self.type_info(&type_id)?.clone();
        // Validate everything before touching the graph.
        let mut values = Vec::new();
        if let Some(p) = args.get("params") {
            let obj = p.as_object().ok_or("'params' must be an object of parameter values")?;
            for (k, v) in obj {
                let spec = find_spec(&info, k)?;
                values.push((k.clone(), coerce(&info, spec, v)?));
            }
        }
        let name = args.get("name").map(|_| str_arg(args, "name")).transpose()?;
        if let Some(n) = &name {
            validate_node_name(n)?;
            if self.graph.find(n).is_some() {
                return Err(format!("a node named '{n}' already exists; pick another name or omit 'name'"));
            }
        }
        let id = self.graph.add_node(&self.reg, &type_id).map_err(|e| e.to_string())?;
        let mut warnings = vec![];
        for (k, v) in values {
            warnings.extend(run_warning(&info, &k, &v));
            self.graph.set_param(id, &k, v).map_err(|e| e.to_string())?;
        }
        if let Some(n) = name {
            self.graph.nodes.get_mut(&id).expect("just added").name = n;
        }
        let (x, y) = (opt_num(args, "x")?, opt_num(args, "y")?);
        if x.is_some() || y.is_some() {
            self.graph.nodes.get_mut(&id).expect("just added").pos = [x.unwrap_or(0.0) as f32, y.unwrap_or(0.0) as f32];
        }
        Ok(json!({
            "node": self.graph.nodes[&id].name, "type": type_id,
            "inputs": ports(&info.inputs, true), "outputs": ports(&info.outputs, false),
            "warnings": warnings,
        }))
    }

    fn set_param(&mut self, args: &Value) -> Result<Value, String> {
        let id = self.node_id(args, "node")?;
        let pname = str_arg(args, "param")?;
        let info = self.info_of(id)?.clone();
        let spec = find_spec(&info, &pname)?;
        let value = coerce(&info, spec, args.get("value").ok_or("set_param needs 'value'")?)?;
        let warnings: Vec<String> = run_warning(&info, &pname, &value).into_iter().collect();
        self.graph.set_param(id, &pname, value.clone()).map_err(|e| e.to_string())?;
        Ok(json!({
            "node": self.graph.nodes[&id].name, "param": pname,
            "value": serde_json::to_value(&value).unwrap_or(Value::Null),
            "warnings": warnings,
        }))
    }

    fn connect(&mut self, args: &Value) -> Result<Value, String> {
        let from = self.node_id(args, "from")?;
        let to = self.node_id(args, "to")?;
        let (fi, ti) = (self.info_of(from)?.clone(), self.info_of(to)?.clone());
        let (fname, tname) = (self.graph.nodes[&from].name.clone(), self.graph.nodes[&to].name.clone());
        let out = match args.get("from_output") {
            Some(v) => port_index(&fi.outputs, v, &fname, "output")?,
            None => 0,
        };
        let src = fi
            .outputs
            .get(out)
            .ok_or_else(|| format!("'{fname}' ({}) has no outputs", fi.id))?;
        let inp = match args.get("to_input") {
            Some(v) => port_index(&ti.inputs, v, &tname, "input")?,
            None => {
                let node = &self.graph.nodes[&to];
                ti.inputs
                    .iter()
                    .enumerate()
                    .position(|(i, p)| node.inputs.get(i).is_some_and(|s| s.is_none()) && vernon_graph::kinds_fit(src.kind, p.kind))
                    .ok_or_else(|| {
                        format!(
                            "no free input of '{tname}' accepts {:?} from '{fname}.{}'; inputs of '{tname}': {}. Pass to_input to replace a wire.",
                            src.kind,
                            src.name,
                            port_summary(&ti.inputs)
                        )
                    })?
            }
        };
        self.graph.connect_checked(&self.reg, from, out, to, inp).map_err(|e| match e {
            CookError::Cycle(_) => format!("cannot connect: '{tname}' is already upstream of '{fname}', the wire would make a cycle"),
            CookError::WrongInput { expected, found, .. } => format!(
                "cannot connect {found:?} output '{fname}.{}' to {expected:?} input '{tname}.{}'; inputs of '{tname}': {}",
                src.name,
                ti.inputs[inp].name,
                port_summary(&ti.inputs)
            ),
            other => other.to_string(),
        })?;
        Ok(json!({"connected": format!("{fname}.{} -> {tname}.{}", src.name, ti.inputs[inp].name)}))
    }

    fn disconnect(&mut self, args: &Value) -> Result<Value, String> {
        let id = self.node_id(args, "node")?;
        let info = self.info_of(id)?.clone();
        let name = self.graph.nodes[&id].name.clone();
        let i = port_index(&info.inputs, args.get("input").ok_or("disconnect needs 'input'")?, &name, "input")?;
        if self.graph.nodes[&id].inputs.get(i).is_none_or(|s| s.is_none()) {
            return Err(format!("input '{}' of '{name}' has no wire", info.inputs[i].name));
        }
        self.graph.disconnect(id, i);
        Ok(json!({"disconnected": format!("{name}.{}", info.inputs[i].name)}))
    }

    /// Cook `ids`; returns the JSON result and the raw outcomes.
    #[allow(clippy::type_complexity)]
    fn do_cook(&mut self, ids: &[NodeId], detail: bool) -> (Value, Vec<(NodeId, Result<Vec<Data>, CookError>)>) {
        let results = self.graph.cook_many(&self.reg, ids, None, &mut |_| {});
        // Everything that was cooked: the requested nodes and their upstream.
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut stack: Vec<NodeId> = ids.to_vec();
        while let Some(n) = stack.pop() {
            if seen.insert(n) {
                if let Some(node) = self.graph.nodes.get(&n) {
                    stack.extend(node.inputs.iter().flatten().map(|(s, _)| *s));
                }
            }
        }
        let ran = self.reset_actions(Some(&seen));
        let mut cooked = vec![];
        let mut gates = vec![];
        let mut reports = vec![];
        for (id, res) in &results {
            let node = &self.graph.nodes[id];
            let info = self.reg.info(&node.type_id);
            match res {
                Ok(data) => {
                    let outs: Vec<Value> = data
                        .iter()
                        .enumerate()
                        .map(|(i, d)| {
                            let port = info.and_then(|f| f.outputs.get(i)).map(|p| p.name.clone()).unwrap_or_default();
                            json!({"output": port, "kind": format!("{:?}", d.kind()), "summary": data_summary(d)})
                        })
                        .collect();
                    let mut warnings = vec![];
                    for r in data.iter().filter_map(Data::as_report) {
                        for (sev, text) in &r.notes {
                            if *sev >= Severity::Warning {
                                warnings.push(format!("{}: {text}", if *sev == Severity::Error { "error" } else { "warning" }));
                            }
                        }
                        gates.extend(failed_gates(&node.name, r, &|k| node.expect_fail.get(k).cloned()));
                        if detail {
                            reports.push(report_json(r));
                        }
                    }
                    cooked.push(json!({"node": node.name, "type": node.type_id, "status": "ok", "outputs": outs, "warnings": warnings}));
                }
                Err(e) => cooked.push(json!({"node": node.name, "type": node.type_id, "status": "failed", "error": e.to_string()})),
            }
        }
        let open = gates.iter().filter(|g| g["expected"].is_null()).count();
        let mut out = json!({
            "cooked": cooked,
            "failed_gates": gates,
            "failing_gate_count": open,
            "external_runs_requested": ran,
        });
        if detail {
            out["reports"] = Value::Array(reports);
        }
        if !ran.is_empty() {
            out["note"] = json!(format!("run switch was on for {}: the external program may have been started; the switch is now off again", ran.join(", ")));
        }
        (out, results)
    }

    fn get_metric(&mut self, args: &Value) -> Result<Value, String> {
        let id = self.node_id(args, "node")?;
        let metric = str_arg(args, "metric")?;
        let name = self.graph.nodes[&id].name.clone();
        let (_, outs) = self.do_cook(&[id], false);
        let data = outs.into_iter().next().map(|o| o.1).ok_or("nothing cooked")?.map_err(|e| format!("node '{name}' failed to cook: {e}"))?;
        let mut names = vec![];
        for r in data.iter().filter_map(Data::as_report) {
            if let Some(m) = r.metrics.get(&metric) {
                return Ok(json!({"node": name, "metric": metric, "value": m.value, "unit": m.unit, "limit": m.limit, "lower_is_better": m.lower_is_better, "passes": m.passes()}));
            }
            names.extend(r.metrics.keys().cloned());
        }
        if names.is_empty() {
            Err(format!("node '{name}' has no report metrics"))
        } else {
            Err(format!("node '{name}' has no metric '{metric}'; available: {}", names.join(", ")))
        }
    }

    fn list_gates(&mut self, args: &Value) -> Result<Value, String> {
        let ids: Vec<NodeId> = match args.get("node") {
            Some(_) => vec![self.node_id(args, "node")?],
            None => self.graph.nodes.keys().copied().collect(),
        };
        let (_, results) = self.do_cook(&ids, false);
        let mut rows = vec![];
        let mut errors = vec![];
        for (id, res) in results {
            let node = &self.graph.nodes[&id];
            match res {
                Ok(data) => {
                    for r in data.iter().filter_map(Data::as_report) {
                        for (mname, m) in &r.metrics {
                            let (Some(limit), Some(pass)) = (m.limit, m.passes()) else { continue };
                            let expected = node.expect_fail.get(mname);
                            let state = if pass { "pass" } else if expected.is_some() { "expected_fail" } else { "fail" };
                            rows.push(json!({
                                "node": node.name, "report": r.title, "metric": mname, "value": m.value, "unit": m.unit,
                                "limit": limit, "lower_is_better": m.lower_is_better,
                                "margin": vernon_graph::gate::margin(m), "state": state, "expected_reason": expected,
                            }));
                        }
                    }
                }
                Err(e) => errors.push(json!({"node": node.name, "error": e.to_string()})),
            }
        }
        let failing = rows.iter().filter(|r| r["state"] == "fail").count();
        Ok(json!({"gates": rows, "failing": failing, "cook_errors": errors}))
    }

    fn measure_geometry(&mut self, args: &Value) -> Result<Value, String> {
        let id = self.node_id(args, "node")?;
        let info = self.info_of(id)?.clone();
        let name = self.graph.nodes[&id].name.clone();
        let out = match args.get("output") {
            Some(v) => port_index(&info.outputs, v, &name, "output")?,
            None => info
                .outputs
                .iter()
                .position(|p| p.kind == vernon_graph::DataKind::Geometry)
                .ok_or_else(|| format!("'{name}' has no geometry output; outputs: {}", port_summary(&info.outputs)))?,
        };
        let (_, outs) = self.do_cook(&[id], false);
        let data = outs.into_iter().next().map(|o| o.1).ok_or("nothing cooked")?.map_err(|e| format!("node '{name}' failed to cook: {e}"))?;
        match data.get(out).and_then(Data::as_geometry) {
            Some(g) => {
                let mut v = measure(g).to_json();
                v["node"] = json!(name);
                Ok(v)
            }
            None => Err(format!("output {out} of '{name}' is not geometry; outputs: {}", port_summary(&info.outputs))),
        }
    }

    // ------------------------------------------------------------ files

    /// A path inside the work folder for `path`, checked against traversal,
    /// absolute paths and symlinks that leave the folder.
    fn work_path(&self, path: &str) -> Result<PathBuf, String> {
        relative_path_ok(path)?;
        if !path.to_ascii_lowercase().ends_with(".vrn") {
            return Err(format!("'{path}' must end in .vrn"));
        }
        std::fs::create_dir_all(&self.work_dir).map_err(|e| format!("work folder unavailable: {e}"))?;
        let root = self.work_dir.canonicalize().map_err(|e| format!("work folder unavailable: {e}"))?;
        let joined = root.join(path);
        let mut probe = joined.as_path();
        while !probe.exists() {
            probe = probe.parent().ok_or("invalid path")?;
        }
        let real = probe.canonicalize().map_err(|e| e.to_string())?;
        if !real.starts_with(&root) {
            return Err(format!("'{path}' leaves the work folder; use a plain relative path like 'scene.vrn'"));
        }
        Ok(joined)
    }

    fn load_graph(&mut self, args: &Value) -> Result<Value, String> {
        let p = str_arg(args, "path")?;
        let file = self.work_path(&p)?;
        let text = std::fs::read_to_string(&file).map_err(|e| format!("cannot read '{p}' in the work folder: {e}"))?;
        let mut g = Graph::from_json(&text).map_err(|e| format!("'{p}' is not a valid .vrn graph: {e}"))?;
        let unknown = g.normalize(&self.reg);
        if !unknown.is_empty() {
            return Err(format!("'{p}' uses unknown node type(s): {}", unknown.join(", ")));
        }
        g.scene_dir = Some(self.work_dir.clone());
        g.work_root = Some(self.work_dir.join("work"));
        let mut warnings = vec![];
        let ids: Vec<NodeId> = g.nodes.keys().copied().collect();
        for id in ids {
            let Some(info) = self.reg.info(&g.nodes[&id].type_id).cloned() else { continue };
            let node = g.nodes.get_mut(&id).expect("exists");
            for name in out_of_range(&info.params, &node.params) {
                warnings.push(format!("{}.{name} was outside its range and was clamped", node.name));
            }
            node.params = clamp_to_specs(&info.params, &node.params);
            for spec in &info.params {
                if let (ParamKind::Path { .. }, Some(ParamValue::Str(s))) = (&spec.kind, node.params.get(&spec.name)) {
                    if let Err(why) = relative_path_ok(s) {
                        let msg = format!("parameter '{}' = '{s}' {why}; the node will not cook until it is changed", spec.name);
                        warnings.push(format!("{}: {msg}", node.name));
                        g.held.insert(id, msg);
                    }
                }
            }
        }
        self.graph = g;
        let off = self.reset_actions(None);
        if !off.is_empty() {
            warnings.push(format!("switched off run/action parameters: {}", off.join(", ")));
        }
        Ok(json!({"loaded": p, "nodes": self.graph.nodes.len(), "warnings": warnings}))
    }

    fn save_graph(&mut self, args: &Value) -> Result<Value, String> {
        let p = str_arg(args, "path")?;
        let file = self.work_path(&p)?;
        let mut copy = self.graph.clone();
        // Never save an action switched on, like the app.
        for id in copy.nodes.keys().copied().collect::<Vec<_>>() {
            if let Some(info) = self.reg.info(&copy.nodes[&id].type_id) {
                for s in info.params.iter().filter(|s| s.action) {
                    let _ = copy.set_param(id, &s.name, s.default_value());
                }
            }
        }
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&file, copy.to_json()).map_err(|e| format!("cannot write '{p}': {e}"))?;
        Ok(json!({"saved": p, "nodes": copy.nodes.len()}))
    }
}

impl ToolHost for GraphToolHost {
    fn tool_specs(&self) -> Vec<ToolSpec> {
        schema::all()
    }

    fn call(&mut self, name: &str, args: Value) -> Result<Value, String> {
        let args = if args.is_null() { Value::Object(Map::new()) } else { args };
        self.check_args(name, &args)?;
        self.dispatch(name, args)
    }
}

// ---------------------------------------------------------------- helpers

fn str_arg(args: &Value, key: &str) -> Result<String, String> {
    match args.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(format!("argument '{key}' must be a string")),
        None => Err(format!("missing argument '{key}'")),
    }
}

fn num_arg(args: &Value, key: &str) -> Result<f64, String> {
    args.get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("argument '{key}' must be a number"))
}

fn opt_num(args: &Value, key: &str) -> Result<Option<f64>, String> {
    args.get(key).map(|_| num_arg(args, key)).transpose()
}

fn stage_prefix(s: Stage) -> &'static str {
    match s {
        Stage::Geometry => "geo",
        Stage::Capture => "capture",
        Stage::Clean => "clean",
        Stage::Surface => "surface",
        Stage::Print => "print",
        Stage::Cfd => "cfd",
        Stage::Fea => "fea",
        Stage::Solar => "solar",
        Stage::Flatten => "flatten",
        Stage::Performance => "perf",
    }
}

fn parse_stage(s: &str) -> Result<Stage, String> {
    let want = s.trim().to_ascii_lowercase();
    Stage::ALL
        .into_iter()
        .find(|st| stage_prefix(*st) == want || format!("{st:?}").to_ascii_lowercase() == want)
        .ok_or_else(|| format!("unknown stage '{s}'; use one of: {}", Stage::ALL.iter().map(|s| stage_prefix(*s)).collect::<Vec<_>>().join(", ")))
}

/// `Ok` for a plain relative path inside a folder: no `..`, no drive, no
/// root, no NUL.
fn relative_path_ok(p: &str) -> Result<(), String> {
    let path = Path::new(p);
    let rooted = p.starts_with(['/', '\\']) || p.contains('\0');
    let bad = path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
        || p.split(['/', '\\']).any(|s| s == "..");
    if path.is_absolute() || rooted || bad || p.as_bytes().get(1) == Some(&b':') {
        return Err(format!("is not a plain relative path (absolute paths, drive letters and '..' are not allowed: '{p}')"));
    }
    Ok(())
}

fn port_summary(ports: &[PortSpec]) -> String {
    if ports.is_empty() {
        return "none".into();
    }
    ports.iter().enumerate().map(|(i, p)| format!("{i}:{} ({:?})", p.name, p.kind)).collect::<Vec<_>>().join(", ")
}

fn ports(ports: &[PortSpec], with_required: bool) -> Vec<Value> {
    ports
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut v = json!({"index": i, "name": p.name, "kind": format!("{:?}", p.kind)});
            if with_required {
                v["required"] = json!(p.required);
            }
            v
        })
        .collect()
}

fn port_index(ports: &[PortSpec], v: &Value, node: &str, what: &str) -> Result<usize, String> {
    match v {
        Value::Number(n) => {
            let i = n.as_u64().ok_or_else(|| format!("{what} index must be a non-negative integer"))? as usize;
            if i < ports.len() {
                Ok(i)
            } else {
                Err(format!("'{node}' has no {what} {i}; {what}s: {}", port_summary(ports)))
            }
        }
        Value::String(s) => ports
            .iter()
            .position(|p| p.name == *s)
            .ok_or_else(|| format!("'{node}' has no {what} named '{s}'; {what}s: {}", port_summary(ports))),
        _ => Err(format!("{what} must be a port index or name")),
    }
}

fn find_spec<'a>(info: &'a NodeTypeInfo, name: &str) -> Result<&'a ParamSpec, String> {
    info.params.iter().find(|s| s.name == name).ok_or_else(|| {
        format!(
            "'{}' has no parameter '{name}'; parameters: {}",
            info.id,
            if info.params.is_empty() { "none".into() } else { info.params.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ") }
        )
    })
}

fn unit_of(spec: &ParamSpec) -> String {
    spec.unit.as_deref().map(|u| format!(" {u}")).unwrap_or_default()
}

/// Turn a JSON value into the value of `spec`, or say exactly what is wrong.
fn coerce(info: &NodeTypeInfo, spec: &ParamSpec, v: &Value) -> Result<ParamValue, String> {
    let ctx = format!("parameter '{}' of '{}'", spec.name, info.id);
    match &spec.kind {
        ParamKind::Float { min, max, .. } => {
            let x = v.as_f64().ok_or_else(|| format!("{ctx} must be a number{} (got {v})", unit_of(spec)))?;
            if !x.is_finite() || x < *min || x > *max {
                return Err(format!("{ctx} must be between {min} and {max}{} (got {x})", unit_of(spec)));
            }
            Ok(ParamValue::Float(x))
        }
        ParamKind::Int { min, max, .. } => {
            let i = v
                .as_i64()
                .or_else(|| v.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 9e15).map(|f| f as i64))
                .ok_or_else(|| format!("{ctx} must be an integer (got {v})"))?;
            if i < *min || i > *max {
                return Err(format!("{ctx} must be between {min} and {max}{} (got {i})", unit_of(spec)));
            }
            Ok(ParamValue::Int(i))
        }
        ParamKind::Bool { .. } => v.as_bool().map(ParamValue::Bool).ok_or_else(|| format!("{ctx} must be true or false (got {v})")),
        ParamKind::Vec3 { .. } => {
            let a = v.as_array().filter(|a| a.len() == 3).ok_or_else(|| format!("{ctx} must be [x, y, z]{} (got {v})", unit_of(spec)))?;
            let c: Option<Vec<f64>> = a.iter().map(|x| x.as_f64().filter(|f| f.is_finite())).collect();
            let c = c.ok_or_else(|| format!("{ctx} must be three finite numbers"))?;
            Ok(ParamValue::Vec3(vernon_core::DVec3::new(c[0], c[1], c[2])))
        }
        ParamKind::Str { .. } => v.as_str().map(|s| ParamValue::Str(s.into())).ok_or_else(|| format!("{ctx} must be a string (got {v})")),
        ParamKind::Path { extensions, .. } => {
            let s = v.as_str().ok_or_else(|| format!("{ctx} must be a path string (got {v})"))?;
            if !s.is_empty() {
                relative_path_ok(s).map_err(|why| {
                    format!("{ctx} {why}. Files must live in the work folder; use a relative path{}", if extensions.is_empty() { String::new() } else { format!(" ({})", extensions.join("/")) })
                })?;
            }
            Ok(ParamValue::Str(s.into()))
        }
        ParamKind::Enum { options, .. } => {
            let s = v.as_str().ok_or_else(|| format!("{ctx} must be one of: {}", options.join(", ")))?;
            if options.iter().any(|o| o == s) {
                Ok(ParamValue::Str(s.into()))
            } else {
                Err(format!("{ctx} must be one of: {} (got '{s}')", options.join(", ")))
            }
        }
    }
}

/// Say so when a call switches on a one-shot action (starting a program).
fn run_warning(info: &NodeTypeInfo, name: &str, value: &ParamValue) -> Option<String> {
    let spec = info.params.iter().find(|s| s.name == name)?;
    if spec.action && *value != spec.default_value() {
        let tool = info.external_tool.as_deref().map(|t| format!(" and start the external program {t}")).unwrap_or_default();
        Some(format!("'{name}' is ON: the next cook of this node will run{tool}. It switches off again after that cook."))
    } else {
        None
    }
}

fn describe(info: &NodeTypeInfo) -> Value {
    let params: Vec<Value> = info
        .params
        .iter()
        .map(|s| {
            let mut v = json!({"name": s.name, "label": s.label, "help": s.help, "unit": s.unit, "one_shot_action": s.action});
            match &s.kind {
                ParamKind::Float { default, min, max } => v.as_object_mut().unwrap().extend([("type".into(), json!("float")), ("default".into(), json!(default)), ("min".into(), json!(min)), ("max".into(), json!(max))]),
                ParamKind::Int { default, min, max } => v.as_object_mut().unwrap().extend([("type".into(), json!("int")), ("default".into(), json!(default)), ("min".into(), json!(min)), ("max".into(), json!(max))]),
                ParamKind::Bool { default } => v.as_object_mut().unwrap().extend([("type".into(), json!("bool")), ("default".into(), json!(default))]),
                ParamKind::Vec3 { default } => v.as_object_mut().unwrap().extend([("type".into(), json!("vec3")), ("default".into(), json!([default.x, default.y, default.z]))]),
                ParamKind::Str { default } => v.as_object_mut().unwrap().extend([("type".into(), json!("string")), ("default".into(), json!(default))]),
                ParamKind::Path { default, extensions, save } => v.as_object_mut().unwrap().extend([("type".into(), json!("path")), ("default".into(), json!(default)), ("extensions".into(), json!(extensions)), ("writes".into(), json!(save))]),
                ParamKind::Enum { default, options } => v.as_object_mut().unwrap().extend([("type".into(), json!("enum")), ("default".into(), json!(default)), ("options".into(), json!(options))]),
            }
            v
        })
        .collect();
    json!({
        "id": info.id, "label": info.label, "stage": stage_prefix(info.stage), "help": info.help,
        "inputs": ports(&info.inputs, true), "outputs": ports(&info.outputs, false), "params": params,
        "placeholder": info.whitebox, "external_program": info.external_tool,
    })
}

fn data_summary(d: &Data) -> String {
    match d {
        Data::Geometry(g) => format!("geometry: {} points, {} prims", g.point_count(), g.prim_count()),
        Data::Report(r) => format!("report '{}': {} metrics", r.title, r.metrics.len()),
        Data::File(p) => format!("file {}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        Data::Json(v) => match &**v {
            Value::Object(o) => format!("json object with {} keys", o.len()),
            Value::Array(a) => format!("json array of {}", a.len()),
            _ => "json value".into(),
        },
    }
}

fn report_json(r: &Report) -> Value {
    let metrics: Map<String, Value> = r
        .metrics
        .iter()
        .map(|(k, m)| (k.clone(), json!({"value": m.value, "unit": m.unit, "limit": m.limit, "lower_is_better": m.lower_is_better, "passes": m.passes()})))
        .collect();
    let notes: Vec<Value> = r
        .notes
        .iter()
        .map(|(s, t)| json!({"severity": match s { Severity::Info => "info", Severity::Warning => "warning", Severity::Error => "error" }, "text": t}))
        .collect();
    json!({"title": r.title, "passes": r.passes(), "metrics": metrics, "notes": notes})
}

fn failed_gates(node: &str, r: &Report, expect: &dyn Fn(&str) -> Option<String>) -> Vec<Value> {
    let mut v = vec![];
    for (name, m) in &r.metrics {
        if let (Some(limit), Some(false)) = (m.limit, m.passes()) {
            v.push(json!({
                "node": node, "report": r.title, "metric": name, "value": m.value, "unit": m.unit, "limit": limit,
                "lower_is_better": m.lower_is_better, "margin": vernon_graph::gate::margin(m),
                "expected": expect(name),
            }));
        }
    }
    for (sev, text) in &r.notes {
        if *sev == Severity::Error {
            v.push(json!({"node": node, "report": r.title, "error": text, "expected": Value::Null}));
        }
    }
    v
}

//! The app's stand-in tool host: a small set of graph tools over a private
//! copy of the scene. The agent edits the copy; the panel applies it to the
//! document in one undo step. Replaced by `vernon-agent-tools` when it lands.

use glam::DVec3;
use serde_json::{json, Value};
use vernon_agent::{ToolHost, ToolSpec};
use vernon_graph::graph::validate_node_name;
use vernon_graph::{Data, Graph, NodeId, ParamKind, ParamValue, Registry};

pub struct GraphHost {
    pub graph: Graph,
    registry: Registry,
}

impl GraphHost {
    pub fn new(graph: Graph, registry: Registry) -> Self {
        Self { graph, registry }
    }

    fn node(&self, name: &str) -> Result<NodeId, String> {
        self.graph.find(name).ok_or_else(|| {
            let names: Vec<&str> = self.graph.nodes.values().map(|n| n.name.as_str()).collect();
            format!("no node named '{name}'. Nodes: {}", names.join(", "))
        })
    }

    fn summary(&self) -> Value {
        let nodes: Vec<Value> = self
            .graph
            .nodes
            .values()
            .map(|n| {
                let inputs: Vec<Value> = n
                    .inputs
                    .iter()
                    .map(|i| i.and_then(|(id, out)| self.graph.nodes.get(&id).map(|u| json!(format!("{}:{out}", u.name)))).unwrap_or(Value::Null))
                    .collect();
                json!({"name": n.name, "type": n.type_id, "inputs": inputs, "params": n.params})
            })
            .collect();
        json!({"nodes": nodes})
    }
}

fn spec(name: &str, description: &str, props: Value, required: &[&str]) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: description.into(),
        parameters: json!({"type": "object", "properties": props, "required": required}),
    }
}

fn text<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    a.get(k).and_then(Value::as_str).ok_or_else(|| format!("missing string argument '{k}'"))
}

fn to_param(kind: &ParamKind, v: &Value) -> Result<ParamValue, String> {
    let bad = || format!("value {v} does not fit a {} parameter", kind_name(kind));
    match kind {
        ParamKind::Float { .. } => v.as_f64().map(ParamValue::Float).ok_or_else(bad),
        ParamKind::Int { .. } => v.as_i64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)).map(ParamValue::Int).ok_or_else(bad),
        ParamKind::Bool { .. } => v.as_bool().map(ParamValue::Bool).ok_or_else(bad),
        ParamKind::Vec3 { .. } => match v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect::<Vec<_>>()) {
            Some(c) if c.len() == 3 => Ok(ParamValue::Vec3(DVec3::new(c[0], c[1], c[2]))),
            _ => Err(bad()),
        },
        ParamKind::Str { .. } | ParamKind::Path { .. } => v.as_str().map(|s| ParamValue::Str(s.into())).ok_or_else(bad),
        ParamKind::Enum { options, .. } => match v.as_str() {
            Some(s) if options.iter().any(|o| o == s) => Ok(ParamValue::Str(s.into())),
            _ => Err(format!("value {v} is not one of: {}", options.join(", "))),
        },
    }
}

fn kind_name(kind: &ParamKind) -> &'static str {
    match kind {
        ParamKind::Float { .. } => "number",
        ParamKind::Int { .. } => "integer",
        ParamKind::Bool { .. } => "boolean",
        ParamKind::Vec3 { .. } => "[x, y, z]",
        ParamKind::Str { .. } | ParamKind::Path { .. } => "string",
        ParamKind::Enum { .. } => "choice",
    }
}

impl ToolHost for GraphHost {
    fn tool_specs(&self) -> Vec<ToolSpec> {
        vec![
            spec("list_node_types", "List node types (id, label, help). Optional stage prefix such as 'geo' or 'cfd'.", json!({"stage": {"type": "string"}}), &[]),
            spec("node_type_info", "Ports and parameters (with ranges and units) of one node type.", json!({"type": {"type": "string"}}), &["type"]),
            spec("graph_summary", "Every node in the scene with its type, parameters and inputs.", json!({}), &[]),
            spec("add_node", "Add a node. 'name' is optional and must be unique.", json!({"type": {"type": "string"}, "name": {"type": "string"}}), &["type"]),
            spec("set_param", "Set one parameter of a node.", json!({"node": {"type": "string"}, "name": {"type": "string"}, "value": {}}), &["node", "name", "value"]),
            spec("connect", "Wire output 'output' (default 0) of 'from' to input 'input' (default 0) of 'to'.", json!({"from": {"type": "string"}, "to": {"type": "string"}, "output": {"type": "integer"}, "input": {"type": "integer"}}), &["from", "to"]),
            spec("remove_node", "Delete a node.", json!({"node": {"type": "string"}}), &["node"]),
            spec("cook", "Cook a node and return its outputs: report metrics and notes, geometry sizes.", json!({"node": {"type": "string"}}), &["node"]),
        ]
    }

    fn call(&mut self, name: &str, a: Value) -> Result<Value, String> {
        match name {
            "list_node_types" => {
                let prefix = a.get("stage").and_then(Value::as_str).map(|s| format!("{}.", s.trim_end_matches('.')));
                let list: Vec<Value> = self
                    .registry
                    .types()
                    .filter(|t| prefix.as_ref().is_none_or(|p| t.id.starts_with(p.as_str())))
                    .map(|t| json!({"id": t.id, "label": t.label, "help": vernon_agent::agent::clip(&t.help, 100)}))
                    .collect();
                Ok(json!(list))
            }
            "node_type_info" => {
                let t = text(&a, "type")?;
                let i = self.registry.info(t).ok_or_else(|| format!("unknown node type '{t}'"))?;
                let ports = |ps: &[vernon_graph::PortSpec]| ps.iter().map(|p| json!({"name": p.name, "kind": format!("{:?}", p.kind), "required": p.required})).collect::<Vec<_>>();
                Ok(json!({"id": i.id, "help": i.help, "inputs": ports(&i.inputs), "outputs": ports(&i.outputs), "params": i.params}))
            }
            "graph_summary" => Ok(self.summary()),
            "add_node" => {
                let t = text(&a, "type")?;
                if let Some(n) = a.get("name").and_then(Value::as_str) {
                    validate_node_name(n)?;
                    if self.graph.find(n).is_some() {
                        return Err(format!("a node named '{n}' already exists"));
                    }
                }
                let id = self.graph.add_node(&self.registry, t).map_err(|e| format!("{e}. Use list_node_types."))?;
                if let Some(n) = a.get("name").and_then(Value::as_str) {
                    self.graph.nodes[&id].name = n.to_string();
                }
                Ok(json!({"name": self.graph.nodes[&id].name}))
            }
            "set_param" => {
                let id = self.node(text(&a, "node")?)?;
                let p = text(&a, "name")?;
                let ty = self.graph.nodes[&id].type_id.clone();
                let info = self.registry.info(&ty).ok_or("node type vanished")?;
                let s = info.params.iter().find(|s| s.name == p).ok_or_else(|| {
                    let names: Vec<&str> = info.params.iter().map(|s| s.name.as_str()).collect();
                    format!("{ty} has no parameter '{p}'. Parameters: {}", names.join(", "))
                })?;
                let v = to_param(&s.kind, a.get("value").ok_or("missing 'value'")?)?;
                self.graph.set_param(id, p, v).map_err(|e| e.to_string())?;
                Ok(json!("ok"))
            }
            "connect" => {
                let (f, t) = (self.node(text(&a, "from")?)?, self.node(text(&a, "to")?)?);
                let idx = |k: &str| a.get(k).and_then(Value::as_u64).unwrap_or(0) as usize;
                self.graph.connect_checked(&self.registry, f, idx("output"), t, idx("input")).map_err(|e| e.to_string())?;
                Ok(json!("ok"))
            }
            "remove_node" => {
                let id = self.node(text(&a, "node")?)?;
                self.graph.remove_node(id);
                Ok(json!("ok"))
            }
            "cook" => {
                let id = self.node(text(&a, "node")?)?;
                let outs = self.graph.cook(&self.registry, id).map_err(|e| e.to_string())?;
                let outs: Vec<Value> = outs
                    .iter()
                    .map(|d| match d {
                        Data::Report(r) => {
                            let metrics: serde_json::Map<String, Value> = r.metrics.iter().map(|(k, m)| (k.clone(), json!({"value": m.value, "unit": m.unit, "limit": m.limit}))).collect();
                            let notes: Vec<String> = r.notes.iter().map(|(s, t)| format!("{s:?}: {t}")).collect();
                            json!({"report": r.title, "passes": r.passes(), "metrics": metrics, "notes": notes})
                        }
                        Data::Geometry(g) => json!({"geometry": {"points": g.point_count(), "prims": g.prim_count()}}),
                        Data::File(p) => json!({"file": p.display().to_string()}),
                        Data::Json(_) => json!({"json": "structured data"}),
                    })
                    .collect();
                Ok(json!(outs))
            }
            other => Err(format!("unknown tool '{other}'")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> GraphHost {
        GraphHost::new(Graph::new(), crate::registry::build_registry())
    }

    #[test]
    fn builds_and_cooks_a_box() {
        let mut h = host();
        let n = h.call("add_node", json!({"type": "geo.box", "name": "b"})).unwrap();
        assert_eq!(n["name"], "b");
        h.call("cook", json!({"node": "b"})).unwrap();
        assert_eq!(h.graph.nodes.len(), 1);
    }

    #[test]
    fn errors_name_the_valid_choices() {
        let mut h = host();
        assert!(h.call("add_node", json!({"type": "nope"})).unwrap_err().contains("list_node_types"));
        h.call("add_node", json!({"type": "geo.box", "name": "b"})).unwrap();
        assert!(h.call("set_param", json!({"node": "b", "name": "zzz", "value": 1})).unwrap_err().contains("Parameters:"));
        assert!(h.call("connect", json!({"from": "b", "to": "q"})).unwrap_err().contains("Nodes: b"));
        assert!(h.call("add_node", json!({"type": "geo.box", "name": "b"})).unwrap_err().contains("already exists"));
    }

    #[test]
    fn the_scene_round_trips_to_json() {
        let mut h = host();
        h.call("add_node", json!({"type": "geo.box", "name": "b"})).unwrap();
        let again = Graph::from_json(&h.graph.to_json()).unwrap();
        assert!(again.find("b").is_some());
    }
}

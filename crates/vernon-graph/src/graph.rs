use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{CookContext, CookError, CookResult, Data, ParamValue, Params, Registry};

pub type NodeId = u32;

/// One node in a graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeInstance {
    pub type_id: String,
    /// Unique, user-editable name, e.g. `icp_align1`.
    pub name: String,
    /// Missing entries take their defaults in [`Graph::normalize`].
    #[serde(default)]
    pub params: Params,
    /// `inputs[i] = Some((upstream node, upstream output index))`.
    #[serde(default)]
    pub inputs: Vec<Option<(NodeId, usize)>>,
    /// Pass input 0 straight through instead of cooking.
    #[serde(default)]
    pub bypass: bool,
    /// Node-editor position.
    #[serde(default)]
    pub pos: [f32; 2],
}

#[derive(Debug, Clone)]
struct CacheEntry {
    signature: u64,
    result: CookResult<Vec<Data>>,
}

/// A network of node instances. Serialises to the `.vrn` JSON file format.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: IndexMap<NodeId, NodeInstance>,
    /// The node whose output the viewport shows.
    pub display: Option<NodeId>,
    /// Hand-written files may leave this out; [`Graph::normalize`] repairs it.
    #[serde(default)]
    next_id: NodeId,
    #[serde(skip)]
    cache: HashMap<NodeId, CacheEntry>,
    /// Root for per-node scratch directories. Defaults to the system temp dir.
    #[serde(skip)]
    pub work_root: Option<PathBuf>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a node of `type_id` with default parameters. The name is made
    /// unique by appending a number.
    pub fn add_node(&mut self, registry: &Registry, type_id: &str) -> CookResult<NodeId> {
        let info = registry
            .info(type_id)
            .ok_or_else(|| CookError::UnknownType(type_id.into()))?;
        let base = type_id.rsplit('.').next().unwrap_or(type_id);
        let name = self.unique_name(base);
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.insert(
            id,
            NodeInstance {
                type_id: type_id.into(),
                name,
                params: Params::from_specs(&info.params),
                inputs: vec![None; info.inputs.len()],
                bypass: false,
                pos: [0.0, 0.0],
            },
        );
        Ok(id)
    }

    fn unique_name(&self, base: &str) -> String {
        let taken: HashSet<&str> = self.nodes.values().map(|n| n.name.as_str()).collect();
        (1..)
            .map(|i| format!("{base}{i}"))
            .find(|n| !taken.contains(n.as_str()))
            .expect("infinite iterator")
    }

    pub fn remove_node(&mut self, id: NodeId) {
        self.nodes.shift_remove(&id);
        self.cache.remove(&id);
        for node in self.nodes.values_mut() {
            for input in node.inputs.iter_mut() {
                if matches!(input, Some((src, _)) if *src == id) {
                    *input = None;
                }
            }
        }
        if self.display == Some(id) {
            self.display = None;
        }
    }

    /// Wire `from`'s output `out_index` into `to`'s input `in_index`.
    pub fn connect(&mut self, from: NodeId, out_index: usize, to: NodeId, in_index: usize) -> CookResult<()> {
        if !self.nodes.contains_key(&from) {
            return Err(CookError::UnknownNode(from));
        }
        let node = self.nodes.get_mut(&to).ok_or(CookError::UnknownNode(to))?;
        if in_index >= node.inputs.len() {
            node.inputs.resize(in_index + 1, None);
        }
        node.inputs[in_index] = Some((from, out_index));
        if self.reaches(to, from) {
            if let Some(node) = self.nodes.get_mut(&to) {
                node.inputs[in_index] = None;
            }
            return Err(CookError::Cycle(to));
        }
        Ok(())
    }

    pub fn disconnect(&mut self, to: NodeId, in_index: usize) {
        if let Some(node) = self.nodes.get_mut(&to) {
            if let Some(slot) = node.inputs.get_mut(in_index) {
                *slot = None;
            }
        }
    }

    /// True if `target` is reachable upstream from `start` (inclusive).
    fn reaches(&self, start: NodeId, target: NodeId) -> bool {
        let mut stack = vec![target];
        let mut seen = HashSet::new();
        // Walk upstream from `target`; if we meet `start`, wiring start->...->target
        // plus target->start forms a cycle.
        while let Some(n) = stack.pop() {
            if n == start {
                return true;
            }
            if !seen.insert(n) {
                continue;
            }
            if let Some(node) = self.nodes.get(&n) {
                stack.extend(node.inputs.iter().flatten().map(|(src, _)| *src));
            }
        }
        false
    }

    pub fn set_param(&mut self, id: NodeId, name: &str, value: ParamValue) -> CookResult<()> {
        let node = self.nodes.get_mut(&id).ok_or(CookError::UnknownNode(id))?;
        node.params.set(name, value);
        Ok(())
    }

    pub fn find(&self, name: &str) -> Option<NodeId> {
        self.nodes.iter().find(|(_, n)| n.name == name).map(|(id, _)| *id)
    }

    /// The linear chain feeding `id` through input 0, bottom (source) first.
    /// This is what the Modify panel shows as the modifier stack.
    pub fn stack(&self, id: NodeId) -> Vec<NodeId> {
        let mut chain = vec![id];
        let mut cur = id;
        let mut seen = HashSet::from([id]);
        while let Some(Some((src, _))) = self.nodes.get(&cur).and_then(|n| n.inputs.first()) {
            if !seen.insert(*src) {
                break;
            }
            chain.push(*src);
            cur = *src;
        }
        chain.reverse();
        chain
    }

    /// Cook `id` and return its outputs, reusing cached results when neither
    /// its parameters nor anything upstream changed.
    pub fn cook(&mut self, registry: &Registry, id: NodeId) -> CookResult<Vec<Data>> {
        let mut visiting = HashSet::new();
        self.cook_inner(registry, id, &mut visiting).map(|(_, d)| d)
    }

    /// Cook a single output.
    pub fn cook_output(&mut self, registry: &Registry, id: NodeId, output: usize) -> CookResult<Data> {
        let outs = self.cook(registry, id)?;
        outs.into_iter()
            .nth(output)
            .ok_or_else(|| CookError::Other(format!("node {id} has no output {output}")))
    }

    fn cook_inner(
        &mut self,
        registry: &Registry,
        id: NodeId,
        visiting: &mut HashSet<NodeId>,
    ) -> CookResult<(u64, Vec<Data>)> {
        if !visiting.insert(id) {
            return Err(CookError::Cycle(id));
        }
        let node = self.nodes.get(&id).cloned().ok_or(CookError::UnknownNode(id))?;
        let op = registry
            .get(&node.type_id)
            .cloned()
            .ok_or_else(|| CookError::UnknownType(node.type_id.clone()))?;
        let info = op.info();

        let mut hasher = DefaultHasher::new();
        node.type_id.hash(&mut hasher);
        serde_json::to_string(&node.params)
            .unwrap_or_default()
            .hash(&mut hasher);
        node.bypass.hash(&mut hasher);

        let mut inputs: Vec<Option<Data>> = Vec::with_capacity(node.inputs.len());
        for (i, wire) in node.inputs.iter().enumerate() {
            match wire {
                Some((src, out)) => {
                    let (sig, outs) = self.cook_inner(registry, *src, visiting)?;
                    (i, sig, *out).hash(&mut hasher);
                    let data = outs.get(*out).cloned().ok_or_else(|| {
                        CookError::Other(format!("node {src} has no output {out}"))
                    })?;
                    inputs.push(Some(data));
                }
                None => {
                    (i, "none").hash(&mut hasher);
                    inputs.push(None);
                }
            }
        }
        visiting.remove(&id);
        let signature = hasher.finish();

        if let Some(entry) = self.cache.get(&id) {
            if entry.signature == signature {
                return entry.result.clone().map(|d| (signature, d));
            }
        }

        let result = if node.bypass {
            match inputs.first().cloned().flatten() {
                Some(d) => Ok(vec![d; info.outputs.len().max(1)]),
                None => Err(CookError::MissingInput {
                    index: 0,
                    name: "bypassed input".into(),
                }),
            }
        } else {
            self.check_inputs(&info, &inputs)
                .and_then(|_| {
                    let ctx = CookContext {
                        node_name: node.name.clone(),
                        work_dir: self.work_dir(&node.name),
                        frame: 0.0,
                    };
                    op.cook(&ctx, &inputs, &node.params)
                })
                .and_then(|outs| {
                    if outs.len() == info.outputs.len() {
                        Ok(outs)
                    } else {
                        Err(CookError::Failed {
                            node: node.name.clone(),
                            message: format!(
                                "returned {} outputs, declared {}",
                                outs.len(),
                                info.outputs.len()
                            ),
                        })
                    }
                })
        };
        self.cache.insert(
            id,
            CacheEntry {
                signature,
                result: result.clone(),
            },
        );
        result.map(|d| (signature, d))
    }

    fn check_inputs(&self, info: &crate::NodeTypeInfo, inputs: &[Option<Data>]) -> CookResult<()> {
        for (i, port) in info.inputs.iter().enumerate() {
            match inputs.get(i).and_then(|d| d.as_ref()) {
                None if port.required => {
                    return Err(CookError::MissingInput {
                        index: i,
                        name: port.name.clone(),
                    })
                }
                Some(d) if port.kind != crate::DataKind::Any && d.kind() != port.kind => {
                    return Err(CookError::WrongInput {
                        index: i,
                        expected: port.kind,
                        found: d.kind(),
                    })
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn work_dir(&self, node_name: &str) -> PathBuf {
        self.work_root
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("vernon"))
            .join(node_name)
    }

    /// Make a loaded graph cookable: order nodes by id, add parameters the
    /// file left at their defaults, pad unconnected trailing inputs and keep
    /// `next_id` above every id. Returns the type ids the registry does not
    /// know (those nodes are left untouched and fail when cooked).
    pub fn normalize(&mut self, registry: &Registry) -> Vec<String> {
        self.nodes.sort_keys();
        if let Some(max) = self.nodes.keys().max() {
            self.next_id = self.next_id.max(max + 1);
        }
        let mut unknown = Vec::new();
        for node in self.nodes.values_mut() {
            let Some(info) = registry.info(&node.type_id) else {
                unknown.push(node.type_id.clone());
                continue;
            };
            for spec in &info.params {
                if node.params.get(&spec.name).is_none() {
                    node.params.set(&spec.name, spec.default_value());
                }
            }
            if node.inputs.len() < info.inputs.len() {
                node.inputs.resize(info.inputs.len(), None);
            }
        }
        self.cache.clear();
        unknown
    }

    /// Drop all cached results.
    pub fn invalidate_all(&mut self) {
        self.cache.clear();
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("graph serialises")
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use vernon_core::{DVec3, Geometry};

    use super::*;
    use crate::node::inputs;
    use crate::{DataKind, NodeOp, NodeTypeInfo, ParamSpec, PortSpec, Stage};

    struct BoxOp;
    impl NodeOp for BoxOp {
        fn info(&self) -> NodeTypeInfo {
            NodeTypeInfo::new("geo.box", "Box", Stage::Geometry)
                .output(PortSpec::new("geo", DataKind::Geometry))
                .param(ParamSpec::float("size", "Size", 10.0, 0.0, 1e6).unit("mm"))
                .implemented()
        }
        fn cook(&self, _: &CookContext, _: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
            Ok(vec![Data::geometry(Geometry::make_box(DVec3::splat(p.float("size")?)))])
        }
    }

    struct CountOp(Arc<AtomicUsize>);
    impl NodeOp for CountOp {
        fn info(&self) -> NodeTypeInfo {
            NodeTypeInfo::new("geo.count", "Count", Stage::Geometry)
                .input(PortSpec::new("geo", DataKind::Geometry))
                .output(PortSpec::new("geo", DataKind::Geometry))
                .implemented()
        }
        fn cook(&self, _: &CookContext, ins: &[Option<Data>], _: &Params) -> CookResult<Vec<Data>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(vec![Data::Geometry(inputs::geometry(ins, 0)?)])
        }
    }

    #[test]
    fn caches_until_upstream_changes() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut reg = Registry::new();
        reg.add(BoxOp);
        reg.add(CountOp(counter.clone()));
        let mut g = Graph::new();
        let b = g.add_node(&reg, "geo.box").unwrap();
        let c = g.add_node(&reg, "geo.count").unwrap();
        g.connect(b, 0, c, 0).unwrap();

        g.cook(&reg, c).unwrap();
        g.cook(&reg, c).unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        g.set_param(b, "size", ParamValue::Float(20.0)).unwrap();
        let out = g.cook_output(&reg, c, 0).unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        assert!((out.as_geometry().unwrap().bounds().size().x - 20.0).abs() < 1e-9);

        assert_eq!(g.stack(c), vec![b, c]);
        let json = g.to_json();
        let g2 = Graph::from_json(&json).unwrap();
        assert_eq!(g2.nodes.len(), 2);
    }

    #[test]
    fn normalize_fills_hand_written_files() {
        let mut reg = Registry::new();
        reg.add(BoxOp);
        reg.add(CountOp(Arc::new(AtomicUsize::new(0))));
        let mut g = Graph::from_json(
            r#"{ "nodes": {
                "3": { "type_id": "geo.count", "name": "c", "params": {}, "inputs": [] },
                "1": { "type_id": "geo.box", "name": "b", "params": {}, "inputs": [] },
                "2": { "type_id": "geo.nope", "name": "n", "params": {}, "inputs": [] } } }"#,
        )
        .unwrap();
        assert_eq!(g.normalize(&reg), vec!["geo.nope".to_string()]);
        assert_eq!(g.nodes.keys().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(g.nodes[&1].params.float("size").unwrap(), 10.0);
        assert_eq!(g.nodes[&3].inputs.len(), 1);
        g.connect(1, 0, 3, 0).unwrap();
        assert!(g.cook(&reg, 3).is_ok());
        assert_eq!(g.add_node(&reg, "geo.box").unwrap(), 4);
    }

    #[test]
    fn rejects_cycles_and_missing_inputs() {
        let mut reg = Registry::new();
        reg.add(CountOp(Arc::new(AtomicUsize::new(0))));
        let mut g = Graph::new();
        let a = g.add_node(&reg, "geo.count").unwrap();
        let b = g.add_node(&reg, "geo.count").unwrap();
        g.connect(a, 0, b, 0).unwrap();
        assert!(g.connect(b, 0, a, 0).is_err());
        assert!(matches!(g.cook(&reg, a), Err(CookError::MissingInput { .. })));
    }
}

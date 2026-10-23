//! Document state without any GUI: graph, registry, selection, undo (graph
//! JSON snapshots), cook results and the `.vrn` file.
//!
//! An *object* is a node that no other node consumes; its modifier stack is
//! [`Graph::stack`], the chain feeding it through input 0.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use vernon_core::{Geometry, Report};
use vernon_graph::{Data, DataKind, Graph, NodeId, NodeTypeInfo, ParamValue, Registry};

pub const UNDO_LIMIT: usize = 200;
const SCHEMA_VERSION: u32 = 1;

/// Per-scene display data saved next to the graph in the `.vrn` file under
/// the key `ui` (the graph loader ignores unknown keys).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneUi {
    pub schema_version: u32,
    pub colors: BTreeMap<NodeId, [u8; 3]>,
    pub hidden: BTreeSet<NodeId>,
}

#[derive(Debug, Clone)]
pub enum NodeResult {
    Ok(Vec<Data>),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub error: bool,
    pub text: String,
}

const PALETTE: [[u8; 3]; 8] = [
    [0x8F, 0xB8, 0xDE],
    [0xE0, 0xA4, 0x58],
    [0x9C, 0xCC, 0x7A],
    [0xD9, 0x86, 0xA8],
    [0x7E, 0xD0, 0xC4],
    [0xC9, 0xB3, 0x7C],
    [0xA9, 0x9B, 0xE0],
    [0xD0, 0xD0, 0xD0],
];

pub struct AppState {
    pub registry: Registry,
    pub graph: Graph,
    /// Selected nodes in click order; the last one is the primary.
    pub selection: Vec<NodeId>,
    /// Highlighted entry of the primary's modifier stack.
    pub stack_entry: Option<NodeId>,
    /// Show the whole stack (on) or only up to the highlighted entry (off).
    pub show_end_result: bool,
    pub scene: SceneUi,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    undo: Vec<String>,
    redo: Vec<String>,
    last_checkpoint: Option<(String, f64)>,
    pub results: HashMap<NodeId, NodeResult>,
    revision: u64,
    cooked_revision: u64,
    pub log: Vec<LogLine>,
}

impl AppState {
    pub fn new(registry: Registry) -> Self {
        Self {
            registry,
            graph: Graph::new(),
            selection: Vec::new(),
            stack_entry: None,
            show_end_result: true,
            scene: SceneUi {
                schema_version: SCHEMA_VERSION,
                ..Default::default()
            },
            path: None,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            last_checkpoint: None,
            results: HashMap::new(),
            revision: 1,
            cooked_revision: 0,
            log: Vec::new(),
        }
    }

    fn touch(&mut self) {
        self.revision += 1;
        self.dirty = true;
    }

    pub fn info_text(&mut self, text: impl Into<String>) {
        self.push_log(false, text.into());
    }

    pub fn error_text(&mut self, text: impl Into<String>) {
        self.push_log(true, text.into());
    }

    fn push_log(&mut self, error: bool, text: String) {
        self.log.push(LogLine { error, text });
        if self.log.len() > 500 {
            self.log.drain(..100);
        }
    }

    // ------------------------------------------------------------ undo

    /// Save the current graph so the next change can be undone.
    pub fn checkpoint(&mut self) {
        self.undo.push(self.graph.to_json());
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.last_checkpoint = None;
    }

    /// Like [`checkpoint`](Self::checkpoint) but merges repeated edits of the
    /// same `key` (a dragged spinner) within 1.5 s into one undo step.
    pub fn checkpoint_coalesced(&mut self, key: &str, now: f64) {
        if let Some((k, t)) = &mut self.last_checkpoint {
            if k == key && now - *t < 1.5 {
                *t = now;
                return;
            }
        }
        self.checkpoint();
        self.last_checkpoint = Some((key.to_string(), now));
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(s) = self.undo.pop() else { return false };
        self.redo.push(self.graph.to_json());
        self.restore(&s);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(s) = self.redo.pop() else { return false };
        self.undo.push(self.graph.to_json());
        self.restore(&s);
        true
    }

    fn restore(&mut self, json: &str) {
        match Graph::from_json(json) {
            Ok(mut g) => {
                g.work_root = self.graph.work_root.clone();
                self.graph = g;
                self.results.clear();
                self.last_checkpoint = None;
                self.prune();
                self.touch();
            }
            Err(e) => self.error_text(format!("undo snapshot unreadable: {e}")),
        }
    }

    /// Drop selection and display entries of nodes that no longer exist.
    fn prune(&mut self) {
        let nodes = &self.graph.nodes;
        self.selection.retain(|id| nodes.contains_key(id));
        if self.stack_entry.is_some_and(|e| !nodes.contains_key(&e)) {
            self.stack_entry = None;
        }
        self.scene.colors.retain(|id, _| nodes.contains_key(id));
        self.scene.hidden.retain(|id| nodes.contains_key(id));
    }

    // ------------------------------------------------------------ queries

    pub fn info(&self, id: NodeId) -> Option<&NodeTypeInfo> {
        self.graph.nodes.get(&id).and_then(|n| self.registry.info(&n.type_id))
    }

    pub fn name(&self, id: NodeId) -> &str {
        self.graph.nodes.get(&id).map(|n| n.name.as_str()).unwrap_or("?")
    }

    /// Nodes wired to an output of `id`: (consumer, input slot).
    pub fn consumers(&self, id: NodeId) -> Vec<(NodeId, usize)> {
        let mut out = Vec::new();
        for (cid, n) in &self.graph.nodes {
            for (slot, w) in n.inputs.iter().enumerate() {
                if matches!(w, Some((src, _)) if *src == id) {
                    out.push((*cid, slot));
                }
            }
        }
        out
    }

    /// Objects: nodes with no downstream consumer, in creation order.
    pub fn objects(&self) -> Vec<NodeId> {
        let used: HashSet<NodeId> = self
            .graph
            .nodes
            .values()
            .flat_map(|n| n.inputs.iter().flatten().map(|(s, _)| *s))
            .collect();
        self.graph.nodes.keys().copied().filter(|id| !used.contains(id)).collect()
    }

    pub fn is_object(&self, id: NodeId) -> bool {
        self.consumers(id).is_empty()
    }

    /// The object a node ends up in, following the first consumer downstream.
    pub fn object_of(&self, id: NodeId) -> NodeId {
        let mut cur = id;
        let mut seen = HashSet::from([id]);
        while let Some((next, _)) = self.consumers(cur).first().copied() {
            if !seen.insert(next) {
                break;
            }
            cur = next;
        }
        cur
    }

    pub fn primary(&self) -> Option<NodeId> {
        self.selection.last().copied()
    }

    /// The primary's stack, base first (empty with nothing selected).
    #[cfg(test)]
    pub fn primary_stack(&self) -> Vec<NodeId> {
        self.primary().map(|p| self.graph.stack(p)).unwrap_or_default()
    }

    /// The stack entry the Modify panel edits: the highlighted entry when it
    /// belongs to the primary's stack, else the primary itself.
    pub fn current_entry(&self) -> Option<NodeId> {
        let p = self.primary()?;
        match self.stack_entry {
            Some(e) if self.graph.stack(p).contains(&e) => Some(e),
            _ => Some(p),
        }
    }

    /// The node whose output the viewport shows for `object`.
    pub fn display_node(&self, object: NodeId) -> NodeId {
        if !self.show_end_result && self.primary() == Some(object) {
            if let Some(e) = self.current_entry() {
                return e;
            }
        }
        object
    }

    /// First output of `src` that can feed a port of `kind`.
    pub fn compatible_output(&self, src: NodeId, kind: DataKind) -> Option<usize> {
        let info = self.info(src)?;
        info.outputs
            .iter()
            .position(|o| kind == DataKind::Any || o.kind == DataKind::Any || o.kind == kind)
    }

    pub fn color_of(&self, id: NodeId) -> [u8; 3] {
        self.scene
            .colors
            .get(&id)
            .copied()
            .unwrap_or(PALETTE[id as usize % PALETTE.len()])
    }

    pub fn is_hidden(&self, id: NodeId) -> bool {
        self.scene.hidden.contains(&id)
    }

    // ------------------------------------------------------------ selection

    pub fn select_only(&mut self, id: NodeId) {
        self.selection = vec![id];
        self.stack_entry = None;
    }

    pub fn toggle_select(&mut self, id: NodeId) {
        if let Some(i) = self.selection.iter().position(|s| *s == id) {
            self.selection.remove(i);
        } else {
            self.selection.push(id);
        }
        self.stack_entry = None;
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
        self.stack_entry = None;
    }

    pub fn select_all(&mut self) {
        self.selection = self.objects().into_iter().filter(|id| !self.is_hidden(*id)).collect();
        self.stack_entry = None;
    }

    pub fn select_invert(&mut self) {
        let sel: HashSet<NodeId> = self.selection.iter().copied().collect();
        self.selection = self.objects().into_iter().filter(|id| !sel.contains(id)).collect();
        self.stack_entry = None;
    }

    pub fn select_by_name(&mut self, name: &str) -> bool {
        match self.graph.find(name.trim()) {
            Some(id) => {
                self.select_only(id);
                true
            }
            None => false,
        }
    }

    // ------------------------------------------------------------ edits

    fn place(&mut self, id: NodeId, near: Option<NodeId>) {
        let pos = match near.and_then(|n| self.graph.nodes.get(&n)) {
            Some(n) => [n.pos[0], n.pos[1] + 80.0],
            None => {
                let max_x = self
                    .graph
                    .nodes
                    .iter()
                    .filter(|(k, _)| **k != id)
                    .map(|(_, n)| n.pos[0])
                    .fold(f32::NEG_INFINITY, f32::max);
                [if max_x.is_finite() { max_x + 200.0 } else { 40.0 }, 40.0]
            }
        };
        if let Some(n) = self.graph.nodes.get_mut(&id) {
            n.pos = pos;
        }
    }

    /// Create a node of `type_id`. A node whose first input accepts the
    /// primary selection's output goes on top of that stack (as a modifier);
    /// anything else starts a new object.
    pub fn create(&mut self, type_id: &str) -> Result<NodeId, String> {
        let info = self
            .registry
            .info(type_id)
            .cloned()
            .ok_or_else(|| format!("unknown node type '{type_id}'"))?;
        if let (Some(port), Some(p)) = (info.inputs.first(), self.primary()) {
            if self.compatible_output(p, port.kind).is_some() {
                return self.add_modifier(type_id);
            }
        }
        self.checkpoint();
        let id = self.graph.add_node(&self.registry, type_id).map_err(|e| e.to_string())?;
        self.place(id, None);
        self.select_only(id);
        self.touch();
        self.info_text(format!("created {} ({})", self.name(id), info.label));
        Ok(id)
    }

    /// Insert a node above the highlighted stack entry of the primary
    /// selection, wired through input 0.
    pub fn add_modifier(&mut self, type_id: &str) -> Result<NodeId, String> {
        let primary = self.primary().ok_or("select an object first")?;
        let info = self
            .registry
            .info(type_id)
            .cloned()
            .ok_or_else(|| format!("unknown node type '{type_id}'"))?;
        let port = info.inputs.first().ok_or_else(|| format!("{} has no input", info.label))?;
        let base = self.current_entry().unwrap_or(primary);
        let out = self
            .compatible_output(base, port.kind)
            .ok_or_else(|| format!("{} cannot take the output of {}", info.label, self.name(base)))?;
        let chain = self.graph.stack(primary);
        let next = chain.iter().position(|n| *n == base).and_then(|i| chain.get(i + 1)).copied();

        self.checkpoint();
        let id = self.graph.add_node(&self.registry, type_id).map_err(|e| e.to_string())?;
        if let Err(e) = self.graph.connect(base, out, id, 0) {
            self.graph.remove_node(id);
            self.undo.pop();
            return Err(e.to_string());
        }
        if let Some(next) = next {
            let kind = self.info(next).and_then(|i| i.inputs.first()).map(|p| p.kind);
            if let Some(o) = kind.and_then(|k| self.compatible_output(id, k)) {
                let _ = self.graph.connect(id, o, next, 0);
            }
        }
        self.place(id, Some(base));
        if next.is_none() {
            // The new node is the stack top: it becomes the object.
            if let Some(c) = self.scene.colors.get(&primary).copied() {
                self.scene.colors.insert(id, c);
            }
            if let Some(i) = self.selection.iter().position(|s| *s == primary) {
                self.selection[i] = id;
            }
        }
        self.stack_entry = Some(id);
        self.touch();
        self.info_text(format!("added {} on {}", self.name(id), self.name(base)));
        Ok(id)
    }

    /// Delete nodes, reconnecting each one's input 0 source to its
    /// consumers when the port kinds fit (spec 3.7).
    pub fn delete(&mut self, ids: &[NodeId]) {
        let ids: Vec<NodeId> = ids.iter().copied().filter(|i| self.graph.nodes.contains_key(i)).collect();
        if ids.is_empty() {
            return;
        }
        self.checkpoint();
        let mut reselect = Vec::new();
        for id in &ids {
            let src = self.graph.nodes.get(id).and_then(|n| n.inputs.first().copied().flatten());
            let consumers = self.consumers(*id);
            for (c, slot) in &consumers {
                let want = self.info(*c).and_then(|i| i.inputs.get(*slot)).map(|p| p.kind);
                let fits = match (src, want) {
                    (Some((s, o)), Some(k)) => self
                        .info(s)
                        .and_then(|i| i.outputs.get(o))
                        .is_some_and(|out| k == DataKind::Any || out.kind == DataKind::Any || out.kind == k),
                    _ => false,
                };
                match (fits, src) {
                    (true, Some((s, o))) if !ids.contains(&s) => {
                        let _ = self.graph.connect(s, o, *c, *slot);
                    }
                    _ => self.graph.disconnect(*c, *slot),
                }
            }
            if consumers.is_empty() {
                if let Some((s, _)) = src {
                    reselect.push(s);
                }
            }
            self.graph.remove_node(*id);
        }
        self.prune();
        if self.selection.is_empty() {
            self.selection = reselect.into_iter().filter(|s| self.graph.nodes.contains_key(s)).collect();
        }
        self.touch();
        self.info_text(format!("deleted {} node(s)", ids.len()));
    }

    pub fn delete_selected(&mut self) {
        let sel = self.selection.clone();
        self.delete(&sel);
    }

    pub fn connect(&mut self, from: NodeId, out: usize, to: NodeId, input: usize) -> Result<(), String> {
        let snapshot = self.graph.to_json();
        self.graph.connect(from, out, to, input).map_err(|e| e.to_string())?;
        self.undo.push(snapshot);
        self.redo.clear();
        self.last_checkpoint = None;
        self.touch();
        Ok(())
    }

    pub fn disconnect(&mut self, to: NodeId, input: usize) {
        self.checkpoint();
        self.graph.disconnect(to, input);
        self.touch();
    }

    /// Set a parameter; repeated edits of one parameter within a short time
    /// share an undo step.
    pub fn set_param(&mut self, id: NodeId, name: &str, value: ParamValue, now: f64) {
        if let ParamValue::Float(v) = value {
            if !v.is_finite() {
                return;
            }
        }
        self.checkpoint_coalesced(&format!("param:{id}:{name}"), now);
        let _ = self.graph.set_param(id, name, value);
        self.touch();
    }

    /// Set a parameter without an undo step (the caller checkpointed, e.g.
    /// at the start of a viewport drag).
    pub fn set_param_silent(&mut self, id: NodeId, name: &str, value: ParamValue) {
        if self.graph.set_param(id, name, value).is_ok() {
            self.touch();
        }
    }

    /// The node the Move/Rotate/Scale tools edit for `object`: the object
    /// itself when it is a transform, else a new transform on top of it.
    pub fn ensure_transform(&mut self, object: NodeId) -> Result<NodeId, String> {
        if self.graph.nodes.get(&object).is_some_and(|n| n.type_id == "geo.transform") {
            return Ok(object);
        }
        let pivot = self.geometry(object).map(|g| g.bounds()).filter(|b| !b.is_empty()).map(|b| b.center());
        let saved = std::mem::replace(&mut self.selection, vec![object]);
        self.stack_entry = None;
        let made = self.add_modifier("geo.transform");
        let id = match made {
            Ok(id) => id,
            Err(e) => {
                self.selection = saved;
                return Err(e);
            }
        };
        // Keep the other selected objects selected, swapping in the new top.
        self.selection = saved.into_iter().map(|s| if s == object { id } else { s }).collect();
        if let Some(c) = pivot {
            let _ = self.graph.set_param(id, "pivot", ParamValue::Vec3(c));
        }
        Ok(id)
    }

    /// Add the body-kit example (`examples/bodykit_pipeline.vrn`) to the
    /// scene: car panel > noisy scan > ICP > deviation > surface fit >
    /// split and 3MF > CFD case > FEA deck > solar vs HDT > flatten > lap
    /// numbers. Every new object except the example's display node starts
    /// hidden so the viewports are not cluttered with coincident panels.
    pub fn build_demo_pipeline(&mut self) -> Result<(), String> {
        let mut demo = Graph::from_json(crate::registry::DEMO_PIPELINE).map_err(|e| e.to_string())?;
        let unknown = demo.normalize(&self.registry);
        if !unknown.is_empty() {
            return Err(format!("demo pipeline uses unknown node types: {}", unknown.join(", ")));
        }
        self.checkpoint();
        self.clear_selection();
        let dx = self
            .graph
            .nodes
            .values()
            .map(|n| n.pos[0] + 300.0)
            .fold(0.0_f32, f32::max);
        let before: HashSet<NodeId> = self.objects().into_iter().collect();
        let mut map = HashMap::new();
        for (old, node) in &demo.nodes {
            let id = self.graph.add_node(&self.registry, &node.type_id).map_err(|e| e.to_string())?;
            map.insert(*old, id);
        }
        for (old, node) in &demo.nodes {
            let id = map[old];
            let name_free = !self.graph.nodes.iter().any(|(k, n)| *k != id && n.name == node.name);
            let Some(new) = self.graph.nodes.get_mut(&id) else { continue };
            new.params = node.params.clone();
            new.pos = [node.pos[0] + dx, node.pos[1]];
            new.bypass = node.bypass;
            new.inputs = node
                .inputs
                .iter()
                .map(|w| w.and_then(|(src, out)| map.get(&src).map(|&s| (s, out))))
                .collect();
            if name_free {
                new.name = node.name.clone();
            }
        }
        let shown = demo.display.and_then(|d| map.get(&d).copied());
        for obj in self.objects() {
            if !before.contains(&obj) && Some(obj) != shown {
                self.set_hidden(obj, true);
            }
        }
        if let Some(s) = shown {
            self.select_only(s);
        }
        self.touch();
        self.info_text(
            "demo pipeline loaded: car panel > noisy scan > alignment > deviation > surface > split/3MF > \
             CFD case > FEA deck > solar vs HDT > flatten > lap numbers",
        );
        Ok(())
    }

    pub fn reset_params(&mut self, id: NodeId) {
        let Some(specs) = self.info(id).map(|i| i.params.clone()) else { return };
        self.checkpoint();
        if let Some(n) = self.graph.nodes.get_mut(&id) {
            n.params = vernon_graph::Params::from_specs(&specs);
        }
        self.touch();
    }

    pub fn toggle_bypass(&mut self, id: NodeId) {
        self.checkpoint();
        if let Some(n) = self.graph.nodes.get_mut(&id) {
            n.bypass = !n.bypass;
        }
        self.touch();
    }

    pub fn rename(&mut self, id: NodeId, name: &str, now: f64) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("name cannot be empty".into());
        }
        if self.graph.find(name).is_some_and(|other| other != id) {
            return Err(format!("'{name}' is already used"));
        }
        self.checkpoint_coalesced(&format!("rename:{id}"), now);
        if let Some(n) = self.graph.nodes.get_mut(&id) {
            n.name = name.to_string();
        }
        self.touch();
        Ok(())
    }

    /// Move a node in the node editor (no undo step: the caller checkpoints).
    pub fn move_node(&mut self, id: NodeId, pos: [f32; 2]) {
        if let Some(n) = self.graph.nodes.get_mut(&id) {
            n.pos = pos;
            self.dirty = true;
        }
    }

    pub fn set_color(&mut self, id: NodeId, rgb: [u8; 3]) {
        self.scene.colors.insert(id, rgb);
        self.dirty = true;
        self.revision += 1;
    }

    pub fn set_hidden(&mut self, id: NodeId, hidden: bool) {
        if hidden {
            self.scene.hidden.insert(id);
        } else {
            self.scene.hidden.remove(&id);
        }
        self.dirty = true;
        self.revision += 1;
    }

    // ------------------------------------------------------------ cooking

    /// Cook every node when anything changed since the last cook.
    pub fn cook_all(&mut self) {
        if self.cooked_revision == self.revision {
            return;
        }
        let ids: Vec<NodeId> = self.graph.nodes.keys().copied().collect();
        self.results.clear();
        for id in ids {
            let r = match self.graph.cook(&self.registry, id) {
                Ok(d) => NodeResult::Ok(d),
                Err(e) => NodeResult::Err(e.to_string()),
            };
            self.results.insert(id, r);
        }
        self.cooked_revision = self.revision;
    }

    /// Drop every cached result and cook again.
    pub fn recook(&mut self) {
        self.graph.invalidate_all();
        self.revision += 1;
        self.cook_all();
    }

    pub fn error_of(&self, id: NodeId) -> Option<&str> {
        match self.results.get(&id) {
            Some(NodeResult::Err(e)) => Some(e),
            _ => None,
        }
    }

    pub fn outputs(&self, id: NodeId) -> Option<&[Data]> {
        match self.results.get(&id) {
            Some(NodeResult::Ok(d)) => Some(d),
            _ => None,
        }
    }

    pub fn geometry(&self, id: NodeId) -> Option<Arc<Geometry>> {
        self.outputs(id)?.iter().find_map(|d| d.as_geometry().cloned())
    }

    pub fn report(&self, id: NodeId) -> Option<Arc<Report>> {
        self.outputs(id)?.iter().find_map(|d| d.as_report().cloned())
    }

    /// Nodes that produced a report, in creation order.
    pub fn report_nodes(&self) -> Vec<NodeId> {
        self.graph.nodes.keys().copied().filter(|id| self.report(*id).is_some()).collect()
    }

    // ------------------------------------------------------------ files

    pub fn new_scene(&mut self) {
        let work_root = self.graph.work_root.clone();
        self.graph = Graph::new();
        self.graph.work_root = work_root;
        self.scene = SceneUi {
            schema_version: SCHEMA_VERSION,
            ..Default::default()
        };
        self.selection.clear();
        self.stack_entry = None;
        self.undo.clear();
        self.redo.clear();
        self.results.clear();
        self.path = None;
        self.touch();
        self.dirty = false;
    }

    pub fn to_vrn(&self) -> Result<String, String> {
        let mut v = serde_json::to_value(&self.graph).map_err(|e| e.to_string())?;
        let ui = serde_json::to_value(&self.scene).map_err(|e| e.to_string())?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("ui".into(), ui);
        }
        serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
    }

    pub fn load_vrn(&mut self, text: &str) -> Result<(), String> {
        let mut v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let scene = v
            .as_object_mut()
            .and_then(|o| o.remove("ui"))
            .and_then(|u| serde_json::from_value::<SceneUi>(u).ok())
            .unwrap_or_default();
        let mut graph: Graph = serde_json::from_value(v).map_err(|e| e.to_string())?;
        graph.work_root = self.graph.work_root.clone();
        // Hand-written files (like the shipped example) may omit defaults.
        let unknown = graph.normalize(&self.registry);
        self.new_scene();
        self.graph = graph;
        self.scene = scene;
        self.prune();
        if !unknown.is_empty() {
            self.error_text(format!("unknown node types in file: {}", unknown.join(", ")));
        }
        self.touch();
        self.dirty = false;
        Ok(())
    }

    pub fn save(&mut self, path: &Path) -> Result<(), String> {
        let text = self.to_vrn()?;
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        self.info_text(format!("saved {}", path.display()));
        Ok(())
    }

    pub fn open(&mut self, path: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.load_vrn(&text)?;
        self.path = Some(path.to_path_buf());
        self.info_text(format!("opened {}", path.display()));
        Ok(())
    }

    /// Add an import node reading `path` as a new object.
    pub fn import(&mut self, path: &Path) -> Result<NodeId, String> {
        self.clear_selection();
        let id = self.create("geo.file_read")?;
        let _ = self
            .graph
            .set_param(id, "path", ParamValue::Str(path.display().to_string()));
        self.touch();
        Ok(id)
    }

    /// Write the primary selection's displayed geometry.
    pub fn export(&mut self, path: &Path) -> Result<(), String> {
        let id = self.primary().ok_or("select an object to export")?;
        self.cook_all();
        let geo = self
            .geometry(self.display_node(id))
            .ok_or("the selection has no geometry")?;
        vernon_io::write(path, &geo, &vernon_io::WriteOptions::default()).map_err(|e| e.to_string())?;
        self.info_text(format!("exported {} to {}", self.name(id), path.display()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::build_registry;
    use vernon_graph::Stage;

    fn state() -> AppState {
        AppState::new(build_registry())
    }

    #[test]
    fn objects_and_stack() {
        let mut s = state();
        let b = s.create("geo.box").unwrap();
        assert_eq!(s.objects(), vec![b]);
        let sm = s.create("clean.smooth_mesh").unwrap();
        // The modifier became the object and the selection followed it.
        assert_eq!(s.objects(), vec![sm]);
        assert_eq!(s.primary(), Some(sm));
        assert_eq!(s.primary_stack(), vec![b, sm]);
        // Insert above the base entry: box -> transform -> smooth.
        s.stack_entry = Some(b);
        let t = s.add_modifier("geo.transform").unwrap();
        assert_eq!(s.primary_stack(), vec![b, t, sm]);
        assert_eq!(s.primary(), Some(sm));
        assert_eq!(s.object_of(b), sm);
    }

    #[test]
    fn delete_reconnects_and_undo_restores() {
        let mut s = state();
        let b = s.create("geo.box").unwrap();
        let t = s.create("geo.transform").unwrap();
        let sm = s.create("clean.smooth_mesh").unwrap();
        s.delete(&[t]);
        assert_eq!(s.graph.stack(sm), vec![b, sm]);
        assert!(s.undo());
        assert_eq!(s.graph.stack(sm), vec![b, t, sm]);
        assert!(s.redo());
        assert_eq!(s.graph.nodes.len(), 2);
        // Deleting the object selects what was under it.
        s.select_only(sm);
        s.delete_selected();
        assert_eq!(s.selection, vec![b]);
    }

    #[test]
    fn param_edits_coalesce() {
        let mut s = state();
        let b = s.create("geo.box").unwrap();
        let before = s.undo.len();
        for i in 0..10 {
            s.set_param(b, "center", ParamValue::Vec3(glam::DVec3::new(i as f64, 0.0, 0.0)), i as f64 * 0.1);
        }
        assert_eq!(s.undo.len(), before + 1);
        s.set_param(b, "center", ParamValue::Vec3(glam::DVec3::ZERO), 10.0);
        assert_eq!(s.undo.len(), before + 2);
        s.set_param(b, "center", ParamValue::Float(f64::NAN), 20.0);
        assert_eq!(s.undo.len(), before + 2);
    }

    #[test]
    fn cook_reports_errors_and_reports() {
        let mut s = state();
        let b = s.create("geo.box").unwrap();
        let c = s.create("print.plane_cut").unwrap();
        s.clear_selection();
        let i = s.create("geo.file_read").unwrap();
        s.cook_all();
        assert!(s.error_of(i).is_some());
        assert!(s.geometry(b).is_some());
        assert!(s.report(c).is_some());
        assert_eq!(s.report_nodes(), vec![c]);
        s.toggle_bypass(c);
        s.cook_all();
        assert!(s.error_of(c).is_none());
    }

    #[test]
    fn show_end_result_off_displays_entry() {
        let mut s = state();
        let b = s.create("geo.box").unwrap();
        let c = s.create("print.plane_cut").unwrap();
        s.stack_entry = Some(b);
        assert_eq!(s.display_node(c), c);
        s.show_end_result = false;
        assert_eq!(s.display_node(c), b);
    }

    #[test]
    fn vrn_round_trip_keeps_colours() {
        let mut s = state();
        let b = s.create("geo.box").unwrap();
        s.set_color(b, [1, 2, 3]);
        s.set_hidden(b, true);
        let text = s.to_vrn().unwrap();
        // The file is still a plain graph for other readers.
        assert!(Graph::from_json(&text).is_ok());
        let mut t = state();
        t.load_vrn(&text).unwrap();
        assert_eq!(t.graph.nodes.len(), 1);
        assert_eq!(t.color_of(b), [1, 2, 3]);
        assert!(t.is_hidden(b));
        assert!(!t.dirty);
    }

    #[test]
    fn demo_pipeline_cooks_every_stage() {
        let mut s = state();
        s.build_demo_pipeline().unwrap();
        s.cook_all();
        for id in s.graph.nodes.keys() {
            assert!(s.error_of(*id).is_none(), "{}: {:?}", s.name(*id), s.error_of(*id));
        }
        let gates = crate::gate::stage_gates(&s);
        let missing: Vec<Stage> = gates
            .iter()
            .filter(|g| g.state == crate::gate::GateState::Missing)
            .map(|g| g.stage)
            .collect();
        assert_eq!(missing, Vec::<Stage>::new());
        let failing: Vec<String> = gates
            .iter()
            .filter(|g| matches!(g.state, crate::gate::GateState::Fail | crate::gate::GateState::Error))
            .map(|g| format!("{:?}: {:?}", g.stage, g.report.as_ref().map(|r| &r.metrics)))
            .collect();
        assert!(failing.is_empty(), "{failing:#?}");
        // Only the display object is visible, and it carries deviation colours.
        let visible: Vec<NodeId> = s.objects().into_iter().filter(|o| !s.is_hidden(*o)).collect();
        assert_eq!(visible.len(), 1);
        assert_eq!(s.name(visible[0]), "surface_deviation");
        assert!(s.geometry(visible[0]).unwrap().point_attribs.contains("Cd"));
        // Loading it twice keeps names unique and adds a second copy.
        let n = s.graph.nodes.len();
        s.build_demo_pipeline().unwrap();
        assert_eq!(s.graph.nodes.len(), 2 * n);
        let names: HashSet<&str> = s.graph.nodes.values().map(|n| n.name.as_str()).collect();
        assert_eq!(names.len(), 2 * n);
        // Opening the example file directly works too (defaults filled in).
        let mut t = state();
        t.load_vrn(crate::registry::DEMO_PIPELINE).unwrap();
        assert_eq!(t.graph.nodes.len(), n);
        assert!(t.log.iter().all(|l| !l.error));
    }

    #[test]
    fn ensure_transform_adds_once() {
        let mut s = state();
        let b = s.create("geo.box").unwrap();
        s.cook_all();
        let t = s.ensure_transform(b).unwrap();
        assert_ne!(t, b);
        assert_eq!(s.selection, vec![t]);
        assert_eq!(s.ensure_transform(t).unwrap(), t);
        assert_eq!(
            s.graph.nodes[&t].params.get("pivot"),
            Some(&ParamValue::Vec3(glam::DVec3::ZERO))
        );
    }

    #[test]
    fn rename_rejects_duplicates() {
        let mut s = state();
        let a = s.create("geo.box").unwrap();
        s.clear_selection();
        let b = s.create("geo.box").unwrap();
        let taken = s.name(a).to_string();
        assert!(s.rename(b, &taken, 0.0).is_err());
        assert!(s.rename(b, "body", 0.0).is_ok());
        assert!(s.select_by_name("body"));
        assert_eq!(s.primary(), Some(b));
    }
}

//! Document state without any GUI: graph, registry, selection, undo (graph
//! JSON snapshots), cook results and the `.vrn` file.
//!
//! An *object* is a node that no other node consumes; its modifier stack is
//! [`Graph::stack`], the chain feeding it through input 0.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use vernon_core::{Geometry, Report};
use vernon_graph::{CookResult, Data, DataKind, Graph, NodeId, NodeTypeInfo, ParamKind, ParamValue, Registry};

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
    /// Objects that cannot be picked or transformed in the viewports.
    pub locked: BTreeSet<NodeId>,
    /// Preserve Stack Position: the last highlighted entry per stack,
    /// keyed by the stack's base node.
    pub stack_positions: BTreeMap<NodeId, NodeId>,
}

/// Length of the recently used modifier list (spec 2.7).
pub const RECENT_MODIFIERS: usize = 20;

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

/// What a background cook is doing, shared with its thread.
#[derive(Debug, Clone, Default)]
pub struct CookProgress {
    /// Nodes cooked so far (cache hits are not counted).
    pub cooked: usize,
    /// The node being cooked now.
    pub current: Option<String>,
}

/// Cook state for the status bar.
#[derive(Debug, Clone, Default)]
pub struct CookStatus {
    pub busy: bool,
    /// The results shown are older than the scene.
    pub stale: bool,
    /// The user stopped the cook; nothing restarts it until the next edit.
    pub stopped: bool,
    pub progress: CookProgress,
    pub seconds: f64,
}

type CookOutput = (Graph, Vec<(NodeId, CookResult<Vec<Data>>)>);

/// A cook running on its own thread over a copy of the graph.
struct CookJob {
    revision: u64,
    epoch: u64,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<CookProgress>>,
    started: Instant,
    handle: std::thread::JoinHandle<CookOutput>,
}

/// Folder of a scene file, made absolute: relative file parameters of the
/// scene resolve against it.
pub fn scene_dir(path: &Path) -> Option<PathBuf> {
    std::path::absolute(path).ok()?.parent().map(Path::to_path_buf)
}

/// Scratch folder of a saved scene: `<file>.work` next to it.
pub fn scene_work_root(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "scene".into());
    path.with_file_name(format!("{name}.work"))
}

/// Scratch folder of an unsaved scene, unique per scene and process.
fn untitled_work_root() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join("vernon").join(format!("untitled-{}-{n}", std::process::id()))
}

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
    /// True inside [`State::as_one_undo_step`]: nested edits add no entries.
    undo_grouped: bool,
    pub results: HashMap<NodeId, NodeResult>,
    revision: u64,
    /// Count of scene edits (parameter, wiring, node changes); unlike
    /// `revision` it does not move when a held-back write or a changed input
    /// file triggers a cook.
    edits: u64,
    cooked_revision: u64,
    pub log: Vec<LogLine>,
    /// Copy of the registry for cook threads.
    shared_registry: Arc<Registry>,
    job: Option<CookJob>,
    /// Bumped when every cached result is dropped, so a cook that was
    /// already running cannot bring them back.
    cache_epoch: u64,
    /// The revision whose cook the user stopped.
    stopped_revision: Option<u64>,
    /// False for a scene read from a file until the user trusts it: its
    /// actions (external programs, uploads) are refused and nodes writing
    /// files at paths the scene chose are held.
    pub trusted: bool,
    /// Ask the user whether to trust the scene.
    pub trust_prompt: bool,
    /// When a parameter was last changed by a drag or a keystroke. While
    /// edits keep coming (see [`EDIT_SETTLE`]), nodes that write files keep
    /// their last result instead of writing on every change.
    last_edit: Option<Instant>,
    /// A cook held writer nodes back; cook again once the edit settles.
    writes_pending: bool,
    /// Size and time stamps of the files the scene reads, and when they
    /// were last looked at: a changed file cooks its readers again.
    files_stamp: u64,
    files_checked: Option<Instant>,
    /// Re-selecting an object returns to its last highlighted stack entry.
    pub preserve_stack: bool,
    /// Modifier types added most recently, newest first.
    pub recent_modifiers: Vec<String>,
    /// When each node last produced a new result (this session).
    pub last_run: HashMap<NodeId, std::time::SystemTime>,
}

/// How long after the last parameter edit file writes stay deferred.
pub const EDIT_SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

/// How often the files a scene reads are checked for changes.
const FILES_POLL: std::time::Duration = std::time::Duration::from_secs(1);

impl AppState {
    pub fn new(registry: Registry) -> Self {
        let mut graph = Graph::new();
        graph.work_root = Some(untitled_work_root());
        Self {
            shared_registry: Arc::new(registry.clone()),
            registry,
            graph,
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
            undo_grouped: false,
            results: HashMap::new(),
            revision: 1,
            edits: 0,
            cooked_revision: 0,
            log: Vec::new(),
            job: None,
            cache_epoch: 0,
            stopped_revision: None,
            trusted: true,
            trust_prompt: false,
            last_edit: None,
            writes_pending: false,
            files_stamp: 0,
            files_checked: None,
            preserve_stack: true,
            recent_modifiers: Vec::new(),
            last_run: HashMap::new(),
        }
    }

    pub(crate) fn touch(&mut self) {
        self.revision += 1;
        self.edits += 1;
        self.dirty = true;
    }

    /// Number of scene edits so far (see the `edits` field).
    pub fn edit_count(&self) -> u64 {
        self.edits
    }

    /// True when the last cook is current and nothing is cooking.
    pub fn cooked(&self) -> bool {
        self.job.is_none() && self.cooked_revision == self.revision
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
        if self.undo_grouped {
            return;
        }
        self.undo.push(self.graph.to_json());
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.last_checkpoint = None;
    }

    /// Run `edit` as a single undo step: one checkpoint of the state before
    /// it, and none from the edits inside (a drag that adds a transform to
    /// each selected object is one step, not one per object plus one more).
    pub fn as_one_undo_step<R>(&mut self, edit: impl FnOnce(&mut Self) -> R) -> R {
        self.checkpoint();
        let outer = std::mem::replace(&mut self.undo_grouped, true);
        let r = edit(self);
        self.undo_grouped = outer;
        r
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
                g.scene_dir = self.graph.scene_dir.clone();
                // Nodes the undo did not touch keep their cooked results
                // (and an action that ran is not started again).
                let old = std::mem::replace(&mut self.graph, g);
                self.graph.absorb_cache(old);
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
        self.scene.locked.retain(|id| nodes.contains_key(id));
        self.scene.stack_positions.retain(|b, e| nodes.contains_key(b) && nodes.contains_key(e));
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
        self.primary().map(|p| self.entry_of(p))
    }

    /// The highlighted entry when it belongs to `object`'s stack, else the
    /// object itself.
    pub fn entry_of(&self, object: NodeId) -> NodeId {
        match self.stack_entry {
            Some(e) if self.graph.stack(object).contains(&e) => e,
            _ => object,
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

    /// Before a selection change: remember the primary's highlighted stack
    /// entry (Preserve Stack Position) and drop it.
    fn leave_selection(&mut self) {
        if let (Some(p), Some(e)) = (self.primary(), self.stack_entry) {
            let stack = self.graph.stack(p);
            if let (Some(base), true) = (stack.first().copied(), stack.contains(&e)) {
                self.scene.stack_positions.insert(base, e);
            }
        }
        self.stack_entry = None;
    }

    /// After a selection change: return to the new primary's last entry.
    fn enter_selection(&mut self) {
        self.stack_entry = None;
        let Some(p) = self.primary().filter(|_| self.preserve_stack) else { return };
        let stack = self.graph.stack(p);
        if let Some(e) = stack.first().and_then(|b| self.scene.stack_positions.get(b)) {
            if stack.contains(e) {
                self.stack_entry = Some(*e);
            }
        }
    }

    pub fn select_only(&mut self, id: NodeId) {
        self.leave_selection();
        self.selection = vec![id];
        self.enter_selection();
    }

    pub fn toggle_select(&mut self, id: NodeId) {
        self.leave_selection();
        if let Some(i) = self.selection.iter().position(|s| *s == id) {
            self.selection.remove(i);
        } else {
            self.selection.push(id);
        }
        self.enter_selection();
    }

    pub fn clear_selection(&mut self) {
        self.leave_selection();
        self.selection.clear();
    }

    pub fn select_all(&mut self) {
        self.leave_selection();
        self.selection = self.objects().into_iter().filter(|id| !self.is_hidden(*id)).collect();
        self.enter_selection();
    }

    /// Select every visible object of the same node type as the primary
    /// selection; returns how many are selected. Nothing changes when
    /// nothing is selected.
    pub fn select_similar(&mut self) -> usize {
        let Some(ty) = self.primary().and_then(|p| self.graph.nodes.get(&p)).map(|n| n.type_id.clone()) else {
            return 0;
        };
        self.leave_selection();
        self.selection = self
            .objects()
            .into_iter()
            .filter(|id| !self.is_hidden(*id) && self.graph.nodes.get(id).is_some_and(|n| n.type_id == ty))
            .collect();
        self.enter_selection();
        self.selection.len()
    }

    pub fn select_invert(&mut self) {
        self.leave_selection();
        let sel: HashSet<NodeId> = self.selection.iter().copied().collect();
        self.selection = self.objects().into_iter().filter(|id| !sel.contains(id)).collect();
        self.enter_selection();
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
        self.add_modifier_to(primary, type_id)
    }

    /// [`add_modifier`](Self::add_modifier) on the stack of `primary`
    /// (selected or not: the Modify panel can be pinned to an object).
    pub fn add_modifier_to(&mut self, primary: NodeId, type_id: &str) -> Result<NodeId, String> {
        let info = self
            .registry
            .info(type_id)
            .cloned()
            .ok_or_else(|| format!("unknown node type '{type_id}'"))?;
        let port = info.inputs.first().ok_or_else(|| format!("{} has no input", info.label))?;
        let base = self.entry_of(primary);
        let out = self
            .compatible_output(base, port.kind)
            .ok_or_else(|| format!("{} cannot take the output of {}", info.label, self.name(base)))?;
        let chain = self.graph.stack(primary);
        let next = chain.iter().position(|n| *n == base).and_then(|i| chain.get(i + 1)).copied();

        let recorded = !self.undo_grouped;
        self.checkpoint();
        let id = self.graph.add_node(&self.registry, type_id).map_err(|e| e.to_string())?;
        if let Err(e) = self.graph.connect_checked(&self.registry, base, out, id, 0) {
            self.graph.remove_node(id);
            if recorded {
                self.undo.pop();
            }
            return Err(e.to_string());
        }
        if let Some(next) = next {
            let kind = self.info(next).and_then(|i| i.inputs.first()).map(|p| p.kind);
            if let Some(o) = kind.and_then(|k| self.compatible_output(id, k)) {
                let _ = self.graph.connect_checked(&self.registry, id, o, next, 0);
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
        self.recent_modifiers.retain(|t| t != type_id);
        self.recent_modifiers.insert(0, type_id.to_string());
        self.recent_modifiers.truncate(RECENT_MODIFIERS);
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
                        let _ = self.graph.connect_checked(&self.registry, s, o, *c, *slot);
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
        // A refused wire leaves the graph (and any wire already there) as it was.
        self.graph.connect_checked(&self.registry, from, out, to, input).map_err(|e| e.to_string())?;
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
        self.last_edit = Some(Instant::now());
        self.touch();
    }

    /// Set a parameter without an undo step (the caller checkpointed, e.g.
    /// at the start of a viewport drag).
    pub fn set_param_silent(&mut self, id: NodeId, name: &str, value: ParamValue) {
        if self.graph.set_param(id, name, value).is_ok() {
            self.last_edit = Some(Instant::now());
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
            new.expect_fail = node.expect_fail.clone();
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
        vernon_graph::validate_node_name(name)?;
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

    /// Display only: no re-cook.
    pub fn set_color(&mut self, id: NodeId, rgb: [u8; 3]) {
        self.scene.colors.insert(id, rgb);
        self.dirty = true;
    }

    pub fn set_hidden(&mut self, id: NodeId, hidden: bool) {
        if hidden {
            self.scene.hidden.insert(id);
        } else {
            self.scene.hidden.remove(&id);
        }
        self.dirty = true;
    }

    // ------------------------------------------------------------ cooking

    /// Cook every node when anything changed since the last cook, waiting
    /// for the result (tests, export, scripts). The UI uses
    /// [`cook_step`](Self::cook_step) instead.
    pub fn cook_all(&mut self) {
        if let Some(job) = &self.job {
            if job.revision != self.revision {
                job.cancel.store(true, Ordering::Relaxed);
            }
        }
        self.finish_job(true);
        if self.cooked_revision == self.revision {
            return;
        }
        let mut graph = self.job_graph();
        let ids: Vec<NodeId> = graph.nodes.keys().copied().collect();
        let results = graph.cook_many(&self.registry, &ids, None, &mut |_| {});
        self.graph.absorb_cache(graph);
        self.apply_results(results);
        self.cooked_revision = self.revision;
        self.stopped_revision = None;
    }

    /// Advance the background cook; call once per frame. Collects a
    /// finished cook, cancels one the scene has moved past and starts a new
    /// one when the results are out of date. Returns true while a cook runs.
    pub fn cook_step(&mut self) -> bool {
        self.finish_job(false);
        if let Some(job) = &self.job {
            if job.revision != self.revision {
                job.cancel.store(true, Ordering::Relaxed);
            }
            return true;
        }
        // Writers held back during an edit write once it settles.
        let editing = self.editing();
        if self.writes_pending && !editing {
            self.writes_pending = false;
            self.revision += 1;
        }
        // A file the scene reads changed on disk: cook its readers again
        // (their signatures include the files' size and time stamp).
        if self.files_checked.is_none_or(|t| t.elapsed() >= FILES_POLL) {
            self.files_checked = Some(Instant::now());
            if self.graph.read_files_stamp_checked(&self.registry).is_none_or(|stamp|stamp != self.files_stamp) {
                self.revision += 1;
            }
        }
        if self.cooked_revision == self.revision || self.stopped_revision == Some(self.revision) {
            return false;
        }
        let mut graph = self.job_graph();
        graph.defer_writes = editing;
        self.files_stamp = graph.read_files_stamp(&self.registry);
        let ids: Vec<NodeId> = graph.nodes.keys().copied().collect();
        let registry = self.shared_registry.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(CookProgress::default()));
        let (stop, shared) = (cancel.clone(), progress.clone());
        let spawned = std::thread::Builder::new().name("vernon-cook".into()).spawn(move || {
            let mut graph = graph;
            let names: HashMap<NodeId, String> = graph.nodes.iter().map(|(id, n)| (*id, n.name.clone())).collect();
            // Stopping the cook also stops a program a node is running.
            let results = vernon_core::process::with_cancel(stop.clone(), || {
                graph.cook_many(&registry, &ids, Some(&stop), &mut |id| {
                    if let Ok(mut p) = shared.lock() {
                        p.cooked += 1;
                        p.current = names.get(&id).cloned();
                    }
                })
            });
            (graph, results)
        });
        match spawned {
            Ok(handle) => {
                self.job = Some(CookJob {
                    revision: self.revision,
                    epoch: self.cache_epoch,
                    cancel,
                    progress,
                    started: Instant::now(),
                    handle,
                });
                true
            }
            Err(e) => {
                self.error_text(format!("cannot start a cook thread ({e}); cooking here"));
                self.cook_all();
                false
            }
        }
    }

    /// True while parameter edits keep coming (a drag or typing): file
    /// writes wait until [`EDIT_SETTLE`] has passed since the last one.
    pub fn editing(&self) -> bool {
        self.last_edit.is_some_and(|t| t.elapsed() < EDIT_SETTLE)
    }

    /// When the UI should run [`cook_step`](Self::cook_step) again although
    /// nothing happens on screen: to write files held back during an edit,
    /// and to notice changed input files.
    pub fn wake_after(&self) -> std::time::Duration {
        if self.writes_pending {
            EDIT_SETTLE
        } else {
            FILES_POLL
        }
    }

    /// Stop the running cook after the node it is on; the results stay out
    /// of date until the next edit.
    pub fn stop_cook(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
            self.stopped_revision = Some(self.revision);
            self.info_text("cook stopped; results are out of date until the next change");
        }
    }

    /// `None` when the results are current and nothing is cooking.
    pub fn cook_status(&self) -> Option<CookStatus> {
        let stale = self.cooked_revision != self.revision;
        if self.job.is_none() && !stale {
            return None;
        }
        Some(CookStatus {
            busy: self.job.is_some(),
            stale,
            stopped: self.stopped_revision == Some(self.revision),
            progress: self
                .job
                .as_ref()
                .and_then(|j| j.progress.lock().ok().map(|p| p.clone()))
                .unwrap_or_default(),
            seconds: self.job.as_ref().map_or(0.0, |j| j.started.elapsed().as_secs_f64()),
        })
    }

    /// Collect the background cook if it finished (or wait for it).
    fn finish_job(&mut self, wait: bool) {
        let Some(job) = self.job.take() else { return };
        if !wait && !job.handle.is_finished() {
            self.job = Some(job);
            return;
        }
        let cancelled = job.cancel.load(Ordering::Relaxed);
        match job.handle.join() {
            Ok((graph, results)) => {
                if !cancelled && graph.deferred_writes() > 0 {
                    self.writes_pending = true;
                }
                if job.epoch == self.cache_epoch {
                    self.graph.absorb_cache(graph);
                }
                if !cancelled {
                    // Newer than what is shown even when the scene moved on.
                    self.apply_results(results);
                    if job.revision == self.revision {
                        self.cooked_revision = job.revision;
                    }
                }
            }
            Err(_) => self.error_text("the cook thread stopped unexpectedly"),
        }
    }

    /// Forget a running cook without waiting (the scene it cooks is gone).
    /// Its thread ends after the node it is on.
    fn discard_job(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn apply_results(&mut self, results: Vec<(NodeId, CookResult<Vec<Data>>)>) {
        // "Last run": the nodes whose result is new (cache hits hand back
        // the same shared data).
        let now = std::time::SystemTime::now();
        for (id, r) in &results {
            let same = match (self.results.get(id), r) {
                (Some(NodeResult::Ok(a)), Ok(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_data(x, y)),
                (Some(NodeResult::Err(a)), Err(b)) => *a == b.to_string(),
                _ => false,
            };
            if !same {
                self.last_run.insert(*id, now);
            }
        }
        self.last_run.retain(|id, _| self.graph.nodes.contains_key(id));
        self.results = results
            .into_iter()
            .map(|(id, r)| {
                let r = match r {
                    Ok(d) => NodeResult::Ok(d),
                    Err(e) => NodeResult::Err(e.to_string()),
                };
                (id, r)
            })
            .collect();
    }

    /// The copy of the graph a cook works on. Action parameters the user
    /// switched on are switched off in the scene and stay on in the copy
    /// only, so no later cook, undo or save can start them again; in an
    /// untrusted scene they are refused, and nodes writing files at paths
    /// the scene chose are held.
    fn job_graph(&mut self) -> Graph {
        let mut started = Vec::new();
        for (id, node) in self.graph.nodes.iter_mut() {
            let Some(info) = self.registry.info(&node.type_id) else { continue };
            for spec in info.params.iter().filter(|s| s.action) {
                if matches!(node.params.get(&spec.name), Some(ParamValue::Bool(true))) {
                    node.params.set(&spec.name, ParamValue::Bool(false));
                    started.push((*id, spec.name.clone(), spec.label.clone(), node.name.clone()));
                }
            }
        }
        let mut job = self.graph.clone();
        for (id, param, label, name) in started {
            if self.trusted {
                let _ = job.set_param(id, &param, ParamValue::Bool(true));
                // Start it even when these inputs ran before.
                job.invalidate(id);
                self.info_text(format!("{name}: {label} started"));
            } else {
                self.trust_prompt = true;
                self.error_text(format!(
                    "{name}: {label} refused because this scene is not trusted yet (File > Trust Scene)"
                ));
            }
        }
        if !self.trusted {
            for (id, path) in self.outside_writes() {
                job.held.insert(
                    id,
                    format!("held: writes '{path}', a path chosen by the opened file; trust the scene to allow it"),
                );
            }
        }
        job
    }

    /// Nodes writing a file at a path the scene sets (save-path parameters
    /// that are not empty), with the path.
    fn outside_writes(&self) -> Vec<(NodeId, String)> {
        let mut out = Vec::new();
        for (id, node) in &self.graph.nodes {
            let Some(info) = self.registry.info(&node.type_id) else { continue };
            for spec in &info.params {
                if let (ParamKind::Path { save: true, .. }, Some(ParamValue::Str(s))) = (&spec.kind, node.params.get(&spec.name)) {
                    if !s.trim().is_empty() {
                        out.push((*id, s.clone()));
                    }
                }
            }
        }
        out
    }

    /// What this scene would start or write, one line each, for the trust
    /// prompt: the programs and settings of every node with an action, and
    /// every file it writes at a path of its own choosing.
    pub fn trust_findings(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for node in self.graph.nodes.values() {
            let Some(info) = self.registry.info(&node.type_id) else { continue };
            if !info.params.iter().any(|s| s.action) {
                continue;
            }
            for spec in &info.params {
                let Some(ParamValue::Str(v)) = node.params.get(&spec.name) else { continue };
                let shown = match &spec.kind {
                    ParamKind::Path { save: false, .. } => true,
                    ParamKind::Str { .. } => !["key", "token", "password"].iter().any(|k| spec.name.contains(k)),
                    ParamKind::Enum { default, .. } => v != default,
                    _ => false,
                };
                if shown && !v.trim().is_empty() {
                    lines.push(format!("{} ({}): {} = {v}", node.name, info.label, spec.label));
                }
            }
        }
        for (id, path) in self.outside_writes() {
            lines.push(format!("{}: writes {path}", self.name(id)));
        }
        lines
    }

    /// The user trusts the scene: allow its actions and writes.
    pub fn trust_scene(&mut self) {
        self.trusted = true;
        self.trust_prompt = false;
        self.revision += 1;
        self.info_text("scene trusted: its programs may be started and its files written");
    }

    /// Drop every cached result and cook again (in the background).
    pub fn recook(&mut self) {
        self.graph.invalidate_all();
        self.cache_epoch += 1;
        self.revision += 1;
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
        self.discard_job();
        self.graph = Graph::new();
        self.graph.work_root = Some(untitled_work_root());
        self.trusted = true;
        self.trust_prompt = false;
        self.stopped_revision = None;
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
        // Actions are never saved switched on.
        let mut graph = self.graph.clone();
        for node in graph.nodes.values_mut() {
            let Some(info) = self.registry.info(&node.type_id) else { continue };
            for spec in info.params.iter().filter(|s| s.action) {
                if node.params.get(&spec.name).is_some() {
                    node.params.set(&spec.name, ParamValue::Bool(false));
                }
            }
        }
        let mut v = serde_json::to_value(&graph).map_err(|e| e.to_string())?;
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
        // Hand-written files (like the shipped example) may omit defaults;
        // names are made valid and unique.
        let unknown = graph.normalize(&self.registry);
        // Values a hand-edited file put outside a parameter's range are
        // clamped when cooked; say which.
        let mut clamped = Vec::new();
        for node in graph.nodes.values() {
            let Some(info) = self.registry.info(&node.type_id) else { continue };
            for name in vernon_graph::param::out_of_range(&info.params, &node.params) {
                clamped.push(format!("{}.{name}", node.name));
            }
        }
        // A file is active content: it never starts a program by itself.
        let mut switched_off = 0;
        for node in graph.nodes.values_mut() {
            let Some(info) = self.registry.info(&node.type_id) else { continue };
            for spec in info.params.iter().filter(|s| s.action) {
                if matches!(node.params.get(&spec.name), Some(ParamValue::Bool(true))) {
                    node.params.set(&spec.name, ParamValue::Bool(false));
                    switched_off += 1;
                }
            }
        }
        self.new_scene();
        graph.work_root = self.graph.work_root.clone();
        self.graph = graph;
        self.scene = scene;
        self.prune();
        if !unknown.is_empty() {
            self.error_text(format!("unknown node types in file: {}", unknown.join(", ")));
        }
        if !clamped.is_empty() {
            self.error_text(format!(
                "{} parameter value(s) outside their range are clamped when cooked: {}",
                clamped.len(),
                clamped.join(", ")
            ));
        }
        if switched_off > 0 {
            self.info_text(format!("{switched_off} run switch(es) saved on were turned off; press Run to start them"));
        }
        // Until the user agrees, the scene's programs are not started and
        // files it would write at its own paths are held.
        if !self.trust_findings().is_empty() {
            self.trusted = false;
            self.trust_prompt = true;
        }
        self.touch();
        self.dirty = false;
        Ok(())
    }

    pub fn save(&mut self, path: &Path) -> Result<(), String> {
        let text = self.to_vrn()?;
        write_atomic(path, text.as_bytes())?;
        // the crash-recovery copy is stale once the scene itself is on disk
        let _ = std::fs::remove_file(self.autosave_path());
        self.path = Some(path.to_path_buf());
        let _ = std::fs::remove_file(self.autosave_path());
        // Node scratch files live next to the scene from now on, and
        // relative file parameters resolve against its folder.
        self.graph.work_root = Some(scene_work_root(path));
        self.graph.scene_dir = scene_dir(path);
        self.dirty = false;
        self.info_text(format!("saved {}", path.display()));
        Ok(())
    }

    /// Where the crash-recovery copy goes: next to the scene
    /// (`scene.vrn.autosave`), or the temp folder for an untitled scene.
    pub fn autosave_path(&self) -> PathBuf {
        match &self.path {
            Some(p) => p.with_extension("vrn.autosave"),
            None => std::env::temp_dir().join("vernon-untitled.vrn.autosave"),
        }
    }

    /// Write the crash-recovery copy. The scene path and the unsaved flag are
    /// left alone: the user's own file is only written by Save.
    pub fn autosave(&self) -> Result<PathBuf, String> {
        let p = self.autosave_path();
        write_atomic(&p, self.to_vrn()?.as_bytes())?;
        Ok(p)
    }

    pub fn open(&mut self, path: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.load_vrn(&text)?;
        self.path = Some(path.to_path_buf());
        self.graph.work_root = Some(scene_work_root(path));
        self.graph.scene_dir = scene_dir(path);
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
        if self.job.is_some() {
            return Err("still cooking; export when the status bar shows the cook has finished".into());
        }
        self.cook_all();
        let geo = self
            .geometry(self.display_node(id))
            .ok_or("the selection has no geometry")?;
        vernon_io::write(path, &geo, &vernon_io::WriteOptions::default()).map_err(|e| e.to_string())?;
        self.info_text(format!("exported {} to {}", self.name(id), path.display()));
        Ok(())
    }
}

/// Two outputs are the same result (shared data, or the same file).
fn same_data(a: &Data, b: &Data) -> bool {
    match (a, b) {
        (Data::Geometry(x), Data::Geometry(y)) => Arc::ptr_eq(x, y),
        (Data::Report(x), Data::Report(y)) => Arc::ptr_eq(x, y),
        (Data::Json(x), Data::Json(y)) => Arc::ptr_eq(x, y),
        (Data::File(x), Data::File(y)) => x == y,
        _ => false,
    }
}

/// Write `bytes` to a temporary file in the target's folder and rename it over
/// the target, so a crash mid-write never leaves a half-written scene.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let name = path.file_name().ok_or_else(|| format!("{}: not a file path", path.display()))?;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let tmp = dir.join(format!(".{}.{}.tmp", name.to_string_lossy(), std::process::id()));
    let res = std::fs::File::create(&tmp)
        .and_then(|mut f| f.write_all(bytes).and_then(|()| f.sync_all()))
        .and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: {e}", path.display()));
    }
    Ok(())
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
    fn stack_position_is_preserved_and_recent_modifiers_listed() {
        let mut s = state();
        let b = s.create("geo.box").unwrap();
        let sm = s.create("clean.smooth_mesh").unwrap();
        let t = s.add_modifier("geo.transform").unwrap();
        assert_eq!(s.recent_modifiers, vec!["geo.transform".to_string(), "clean.smooth_mesh".to_string()]);
        s.add_modifier("clean.smooth_mesh").unwrap();
        assert_eq!(s.recent_modifiers[0], "clean.smooth_mesh", "newest first, no duplicates");
        assert_eq!(s.recent_modifiers.len(), 2);
        let top = s.primary().unwrap();
        s.stack_entry = Some(sm);
        s.clear_selection();
        let other = s.create("geo.box").unwrap();
        assert_eq!(s.current_entry(), Some(other));
        s.select_only(top);
        assert_eq!(s.current_entry(), Some(sm), "re-selecting returns to the last entry");
        // Saved with the scene.
        let mut s2 = state();
        s2.load_vrn(&s.to_vrn().unwrap()).unwrap();
        s2.select_only(top);
        assert_eq!(s2.current_entry(), Some(sm));
        s2.preserve_stack = false;
        s2.clear_selection();
        s2.select_only(top);
        assert_eq!(s2.current_entry(), Some(top));
        assert!(s.graph.stack(top).starts_with(&[b, sm, t]));
    }

    #[test]
    fn last_run_moves_only_for_nodes_that_cooked() {
        let mut s = state();
        let b = s.create("geo.box").unwrap();
        s.clear_selection();
        let c = s.create("geo.box").unwrap();
        s.cook_all();
        let (tb, tc) = (s.last_run[&b], s.last_run[&c]);
        std::thread::sleep(std::time::Duration::from_millis(20));
        s.set_param(c, "size", ParamValue::Vec3(glam::DVec3::splat(50.0)), 0.0);
        s.cook_all();
        assert_eq!(s.last_run[&b], tb, "a cache hit is not a run");
        assert!(s.last_run[&c] > tc);
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
        // The demo has no wheels on its body and no FEA solver, so these two
        // gates fail on purpose and the scene lists them under expect_fail.
        // They stay visible on their nodes (every report node of a stage
        // counts, not just the newest) but, as in vernon-cli, they are
        // expected and do not fail the pipeline.
        let nodes = &s.graph.nodes;
        let mut expected: Vec<String> = gates
            .iter()
            .flat_map(|g| {
                g.nodes.iter().flat_map(move |ng| {
                    ng.expected
                        .iter()
                        .map(move |m| format!("{:?}: {} {m}", g.stage, nodes[&ng.node].type_id))
                })
            })
            .collect();
        expected.sort();
        assert_eq!(
            expected,
            vec!["Cfd: cfd.vehicle_prep wheels_found".to_string(), "Fea: fea.tet_mesh placeholder".to_string()],
        );
        assert!(gates.iter().all(|g| g.failing_metrics().is_empty()), "no unexpected failures");
        assert!(gates.iter().all(|g| g.state != crate::gate::GateState::Error));
        assert_eq!(crate::gate::overall(&gates), crate::gate::GateState::Pass);
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
    fn select_similar_picks_the_same_node_type() {
        let mut s = state();
        let a = s.create("geo.box").unwrap();
        let b = s.create("geo.box").unwrap();
        let c = s.create("geo.sphere").unwrap();
        s.cook_all();
        s.select_only(a);
        assert_eq!(s.select_similar(), 2);
        assert!(s.selection.contains(&a) && s.selection.contains(&b) && !s.selection.contains(&c));
        s.clear_selection();
        assert_eq!(s.select_similar(), 0);
    }

    #[test]
    fn a_tool_drag_on_fresh_objects_is_one_undo_step() {
        let mut s = state();
        let a = s.create("geo.box").unwrap();
        let b = s.create("geo.box").unwrap();
        s.cook_all();
        let n = s.undo.len();
        let (ta, tb) = s.as_one_undo_step(|st| (st.ensure_transform(a).unwrap(), st.ensure_transform(b).unwrap()));
        assert_ne!((ta, tb), (a, b));
        assert_eq!(s.undo.len(), n + 1, "one entry for the whole drag");
        // One undo takes both transforms away again.
        assert!(s.undo());
        assert!(!s.graph.nodes.contains_key(&ta) && !s.graph.nodes.contains_key(&tb));
        assert_eq!(s.undo.len(), n);
        // Existing transforms cost one entry, not two.
        let t = s.ensure_transform(a).unwrap();
        let n = s.undo.len();
        s.as_one_undo_step(|st| {
            assert_eq!(st.ensure_transform(t).unwrap(), t);
        });
        assert_eq!(s.undo.len(), n + 1);
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
        for bad in ["../../escaped_dir", "C:\\x", "a/b", "what?", "x."] {
            assert!(s.rename(b, bad, 0.0).is_err(), "{bad}");
        }
        assert_eq!(s.name(b), "body");
    }

    /// Counts its cooks, for checking what re-cooks.
    struct CountOp(Arc<std::sync::atomic::AtomicUsize>);
    impl vernon_graph::NodeOp for CountOp {
        fn info(&self) -> NodeTypeInfo {
            NodeTypeInfo::new("test.count", "Count", Stage::Geometry)
                .output(vernon_graph::PortSpec::new("geo", DataKind::Geometry))
                .param(vernon_graph::ParamSpec::float("size", "Size", 1.0, 0.0, 10.0))
        }
        fn cook(&self, _: &vernon_graph::CookContext, _: &[Option<Data>], p: &vernon_graph::Params) -> vernon_graph::CookResult<Vec<Data>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(vec![Data::geometry(Geometry::make_box(glam::DVec3::splat(p.float("size")?)))])
        }
    }

    #[test]
    fn cooks_in_the_background_and_undo_keeps_untouched_results() {
        let cooks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reg = build_registry();
        reg.add(CountOp(cooks.clone()));
        let mut s = AppState::new(reg);
        let counted = s.create("test.count").unwrap();
        s.clear_selection();
        let b = s.create("geo.box").unwrap();
        let wait = |s: &mut AppState| {
            let start = Instant::now();
            while s.cook_step() {
                assert!(start.elapsed().as_secs() < 60, "cook did not finish");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        assert!(s.cook_step(), "a cook starts on its own thread");
        assert!(s.cook_status().is_some_and(|st| st.busy && st.stale));
        wait(&mut s);
        assert!(s.cook_status().is_none());
        assert!(s.geometry(b).is_some() && s.geometry(counted).is_some());
        assert_eq!(cooks.load(Ordering::SeqCst), 1);
        // Edit the other node, then undo: the counting node is not cooked again.
        s.set_param(b, "size", ParamValue::Vec3(glam::DVec3::splat(5.0)), 0.0);
        wait(&mut s);
        assert!(s.undo());
        wait(&mut s);
        assert_eq!(cooks.load(Ordering::SeqCst), 1);
        // Colour and visibility are display only.
        s.set_hidden(b, true);
        s.set_color(b, [1, 2, 3]);
        assert!(!s.cook_step());
    }

    /// Passes its input on, counting cooks; has a save path, so it is a writer.
    struct WriteOp(Arc<std::sync::atomic::AtomicUsize>);
    impl vernon_graph::NodeOp for WriteOp {
        fn info(&self) -> NodeTypeInfo {
            NodeTypeInfo::new("test.write", "Write", Stage::Geometry)
                .input(vernon_graph::PortSpec::new("geo", DataKind::Geometry))
                .output(vernon_graph::PortSpec::new("geo", DataKind::Geometry))
                .param(vernon_graph::ParamSpec::path("file", "File", &["txt"], true))
        }
        fn cook(&self, _: &vernon_graph::CookContext, ins: &[Option<Data>], _: &vernon_graph::Params) -> vernon_graph::CookResult<Vec<Data>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(vec![Data::Geometry(vernon_graph::node::inputs::geometry(ins, 0)?)])
        }
    }

    #[test]
    fn writes_wait_for_the_edit_and_refused_wires_keep_the_old_one() {
        let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reg = build_registry();
        reg.add(WriteOp(writes.clone()));
        let mut s = AppState::new(reg);
        let b = s.create("geo.box").unwrap();
        s.clear_selection();
        let w = s.create("test.write").unwrap();
        s.connect(b, 0, w, 0).unwrap();
        let wait = |s: &mut AppState| {
            let start = Instant::now();
            while s.cook_step() {
                assert!(start.elapsed().as_secs() < 60, "cook did not finish");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        let settle = |s: &mut AppState| {
            std::thread::sleep(EDIT_SETTLE + std::time::Duration::from_millis(50));
            wait(s);
        };
        wait(&mut s);
        assert_eq!(writes.load(Ordering::SeqCst), 1, "no edit running: the writer cooks");
        // A drag: every frame changes the box, the writer keeps its result.
        for size in [20.0, 30.0, 40.0] {
            s.set_param_silent(b, "size", ParamValue::Vec3(glam::DVec3::splat(size)));
            wait(&mut s);
            assert!(s.editing() && s.outputs(w).is_some());
        }
        assert_eq!(writes.load(Ordering::SeqCst), 1, "nothing written during the drag");
        settle(&mut s);
        assert_eq!(writes.load(Ordering::SeqCst), 2, "written once the drag settled");
        assert!((s.geometry(w).unwrap().bounds().size().x - 40.0).abs() < 1e-9);

        // A refused wire (a loop) keeps the existing one and adds no undo step.
        let undo = s.undo.len();
        assert!(s.connect(w, 0, w, 0).is_err());
        assert!(s.connect(w, 0, b, 0).is_err(), "the box has no input");
        assert_eq!(s.graph.nodes[&w].inputs, vec![Some((b, 0))]);
        assert_eq!(s.undo.len(), undo);
    }

    #[test]
    fn run_switches_are_one_shot_and_never_saved_on() {
        let mut s = state();
        let solve = s.create("cfd.solve_steady").unwrap();
        s.set_param(solve, "run", ParamValue::Bool(true), 0.0);
        s.cook_all();
        assert_eq!(s.graph.nodes[&solve].params.get("run"), Some(&ParamValue::Bool(false)));
        assert!(s.log.iter().any(|l| l.text.contains("Run now started")));
        // Saved on by hand: the file opens with it off.
        s.graph.nodes.get_mut(&solve).unwrap().params.set("run", ParamValue::Bool(true));
        let text = s.to_vrn().unwrap();
        assert!(!text.contains("\"run\": true"), "{text}");
        let mut t = state();
        let text = text.replace("\"run\": false", "\"run\": true");
        // Defaults only: nothing chosen by the file, so it is trusted.
        t.load_vrn(&text).unwrap();
        assert!(t.trusted);
        let id = t.graph.find(s.name(solve)).unwrap();
        assert_eq!(t.graph.nodes[&id].params.get("run"), Some(&ParamValue::Bool(false)));
        // The file names where the programs live: not trusted, so Run is refused.
        t.load_vrn(&text.replace("\"bin_dir\": \"\"", "\"bin_dir\": \"C:/elsewhere/bin\"")).unwrap();
        assert!(!t.trusted && t.trust_prompt);
        assert!(t.trust_findings().iter().any(|l| l.contains("C:/elsewhere/bin")), "{:?}", t.trust_findings());
        t.set_param(id, "run", ParamValue::Bool(true), 0.0);
        t.cook_all();
        assert!(t.log.last().is_some_and(|l| l.error && l.text.contains("not trusted")));
        assert_eq!(t.graph.nodes[&id].params.get("run"), Some(&ParamValue::Bool(false)));
    }

    #[test]
    fn save_is_atomic_and_autosave_leaves_the_scene_alone() {
        let dir = std::env::temp_dir().join(format!("vernon-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("kit.vrn");
        let mut s = state();
        s.create("geo.box").unwrap();
        s.save(&path).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        // no temporary file is left behind
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.path().is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["kit.vrn".to_string()]);
        // an autosave of later edits goes next to the scene, not into it
        s.create("geo.box").unwrap();
        assert!(s.dirty);
        let auto = s.autosave().unwrap();
        assert_eq!(auto, dir.join("kit.vrn.autosave"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
        assert!(s.dirty && s.path.as_deref() == Some(path.as_path()));
        // saving replaces the file in place and clears the recovery copy
        s.save(&path).unwrap();
        assert!(!auto.exists());
        assert_ne!(std::fs::read_to_string(&path).unwrap(), saved);
        // a failed write keeps the old file and reports the path
        assert!(write_atomic(&dir.join("missing").join("x.vrn"), b"x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn untrusted_scene_holds_writes_until_trusted() {
        let out = std::env::temp_dir().join(format!("vernon-trust-{}.stl", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let path = serde_json::to_string(&out.display().to_string()).unwrap();
        let text = format!(
            r#"{{ "nodes": {{
                "0": {{ "type_id": "geo.box", "name": "b" }},
                "1": {{ "type_id": "geo.file_write", "name": "w", "inputs": [[0, 0]], "params": {{ "path": {path} }} }} }} }}"#
        );
        let mut s = state();
        s.load_vrn(&text).unwrap();
        assert!(!s.trusted && s.trust_prompt);
        assert!(s.trust_findings().iter().any(|l| l.contains("vernon-trust-")), "{:?}", s.trust_findings());
        s.cook_all();
        assert!(s.error_of(1).is_some_and(|e| e.contains("trust")), "{:?}", s.error_of(1));
        assert!(s.error_of(0).is_none());
        assert!(!out.exists());
        s.trust_scene();
        s.cook_all();
        assert!(s.error_of(1).is_none(), "{:?}", s.error_of(1));
        assert!(out.exists());
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn work_root_is_per_scene_and_follows_the_file() {
        let a = state();
        let b = state();
        let ra = a.graph.work_root.clone().unwrap();
        assert_ne!(Some(ra.clone()), b.graph.work_root);
        assert!(ra.starts_with(std::env::temp_dir()));
        let dir = std::env::temp_dir().join(format!("vernon-scene-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("panel.vrn");
        let mut s = state();
        s.create("geo.box").unwrap();
        s.save(&file).unwrap();
        assert_eq!(s.graph.work_root, Some(dir.join("panel.vrn.work")));
        let mut t = state();
        t.open(&file).unwrap();
        assert_eq!(t.graph.work_root, Some(dir.join("panel.vrn.work")));
        assert!(t.trusted, "a scene with nothing to start or write is trusted");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

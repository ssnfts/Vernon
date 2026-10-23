use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, ParamKind, ParamValue, Params, Registry};

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
    /// Report metrics whose failing limit is expected at this node, with
    /// the reason (e.g. `wheels_found`: "a single panel has no wheels").
    /// Gate summaries list them as expected instead of failing.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub expect_fail: IndexMap<String, String>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    signature: u64,
    /// Hash of the action parameters that were on when this was cooked
    /// (0 = none). See [`crate::ParamSpec::action`].
    actions: u64,
    result: CookResult<Vec<Data>>,
}

/// State shared by every node visited in one cook pass: each node's
/// signature and outputs are worked out once, however many paths reach it.
struct Pass<'a> {
    memo: HashMap<NodeId, CookResult<(u64, Vec<Data>)>>,
    visiting: HashSet<NodeId>,
    cancel: Option<&'a AtomicBool>,
    on_cook: &'a mut dyn FnMut(NodeId),
}

/// Error text of a cook stopped through the cancel flag of [`Graph::cook_many`].
pub const CANCELLED: &str = "cook cancelled";

/// Longest node name [`validate_node_name`] accepts.
pub const MAX_NAME_LEN: usize = 64;

/// `Ok` when `name` can name a node: 1 to 64 characters, each a letter, a
/// digit, `_`, `-`, `.` or a space, not starting or ending with a space or
/// a dot. This rules out path separators, drive prefixes, `..` and the
/// characters Windows refuses in file names.
pub fn validate_node_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name cannot be empty".into());
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(format!("name is longer than {MAX_NAME_LEN} characters"));
    }
    if let Some(c) = name.chars().find(|c| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ' '))) {
        return Err(format!("'{c}' is not allowed in a name (use letters, digits, _ - . and spaces)"));
    }
    if name.starts_with([' ', '.']) || name.ends_with([' ', '.']) {
        return Err("a name cannot start or end with a space or a dot".into());
    }
    Ok(())
}

/// A valid name made from `name`: disallowed characters become `_`, and an
/// empty result falls back to `fallback`.
fn sanitize_node_name(name: &str, fallback: &str) -> String {
    let mapped: String = name
        .chars()
        .take(MAX_NAME_LEN - 8)
        .map(|c| if c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ' ') { c } else { '_' })
        .collect();
    let trimmed = mapped.trim_matches([' ', '.']);
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Folder name of node `id`'s scratch directory: `n<id>_<name>` with every
/// character other than ASCII letters, digits, `_` and `-` replaced by `_`.
/// The id keeps it unique and the name can never leave the work root.
pub fn work_dir_name(id: NodeId, name: &str) -> String {
    let clean: String = name
        .chars()
        .take(48)
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    format!("n{id}_{clean}")
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
    /// Nodes that must not cook, with the reason reported as their error
    /// (the app holds nodes of an untrusted scene that write outside it).
    #[serde(skip)]
    pub held: HashMap<NodeId, String>,
    /// Node evaluations in the last cook pass (signature work included).
    #[serde(skip)]
    visits: usize,
    /// Folder of the scene file. Relative file parameters resolve against
    /// it (see [`Graph::resolve_path`]), not against the process folder.
    #[serde(skip)]
    pub scene_dir: Option<PathBuf>,
    /// Set while the user is dragging or typing: nodes that write files
    /// (see [`writes_files`]) keep their last result instead of writing on
    /// every change. Cook again with this off once the edit ends.
    #[serde(skip)]
    pub defer_writes: bool,
    /// Current study frame (the time slider), handed to every cook as
    /// [`CookContext::frame`](crate::node::CookContext). Not part of the
    /// cache signature: a sweep changes a parameter per frame, which is.
    #[serde(skip)]
    pub frame: f64,
    /// Writer nodes held back by `defer_writes` in the last cook pass.
    #[serde(skip)]
    deferred: usize,
    /// Most points plus primitive vertices one geometry output may hold;
    /// `None` uses [`DEFAULT_OUTPUT_BUDGET`]. A larger output fails its node.
    #[serde(skip)]
    pub output_budget: Option<usize>,
}

/// Default of [`Graph::output_budget`]: 60 million points plus vertices
/// (a few gigabytes with attributes).
pub const DEFAULT_OUTPUT_BUDGET: usize = 60_000_000;

/// Error of a file-writing node that has no earlier result to keep while
/// writes are deferred (see [`Graph::defer_writes`]).
pub const DEFERRED: &str = "waiting for the edit to end before writing files";

/// Whether an output of kind `out` may feed an input of kind `input`.
pub fn kinds_fit(out: DataKind, input: DataKind) -> bool {
    out == input || out == DataKind::Any || input == DataKind::Any
}

/// True when cooking a node of this type writes files: it has a save-path
/// parameter or a file output (exports, solver cases, packages).
pub fn writes_files(info: &NodeTypeInfo) -> bool {
    info.params.iter().any(|s| matches!(s.kind, ParamKind::Path { save: true, .. }))
        || info.outputs.iter().any(|p| p.kind == DataKind::File)
}

/// Size and modification time of a file or folder, `None` when missing.
fn file_stamp(path: &str) -> Option<(u64, std::time::SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
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
                expect_fail: IndexMap::new(),
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

    /// Wire `from`'s output `out_index` into `to`'s input `in_index`. Both
    /// nodes and the input slot must exist and the wire must not close a
    /// cycle. On any error the graph is unchanged, so an existing wire is
    /// kept. Port indices and kinds against the node types are checked by
    /// [`connect_checked`](Self::connect_checked).
    pub fn connect(&mut self, from: NodeId, out_index: usize, to: NodeId, in_index: usize) -> CookResult<()> {
        if !self.nodes.contains_key(&from) {
            return Err(CookError::UnknownNode(from));
        }
        let slots = self.nodes.get(&to).ok_or(CookError::UnknownNode(to))?.inputs.len();
        if in_index >= slots {
            return Err(CookError::Other(format!("node {to} has no input {in_index}")));
        }
        // `to` upstream of `from` (or the same node) closes a loop.
        if self.reaches(to, from) {
            return Err(CookError::Cycle(to));
        }
        if let Some(node) = self.nodes.get_mut(&to) {
            node.inputs[in_index] = Some((from, out_index));
        }
        Ok(())
    }

    /// [`connect`](Self::connect) after checking the ports against the node
    /// types: the output and the input must be declared and their kinds
    /// must fit (see [`kinds_fit`]). The editor and the app use this.
    pub fn connect_checked(&mut self, registry: &Registry, from: NodeId, out_index: usize, to: NodeId, in_index: usize) -> CookResult<()> {
        let info_of = |id: NodeId| -> CookResult<&crate::NodeTypeInfo> {
            let node = self.nodes.get(&id).ok_or(CookError::UnknownNode(id))?;
            registry.info(&node.type_id).ok_or_else(|| CookError::UnknownType(node.type_id.clone()))
        };
        let (src, dst) = (info_of(from)?, info_of(to)?);
        let out = src
            .outputs
            .get(out_index)
            .ok_or_else(|| CookError::Other(format!("'{}' has no output {out_index}", src.label)))?;
        let input = dst
            .inputs
            .get(in_index)
            .ok_or_else(|| CookError::Other(format!("'{}' has no input {in_index}", dst.label)))?;
        if !kinds_fit(out.kind, input.kind) {
            return Err(CookError::WrongInput {
                index: in_index,
                expected: input.kind,
                found: out.kind,
            });
        }
        self.connect(from, out_index, to, in_index)
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
        let mut noop = |_: NodeId| {};
        let mut pass = Pass {
            memo: HashMap::new(),
            visiting: HashSet::new(),
            cancel: None,
            on_cook: &mut noop,
        };
        self.visits = 0;
        self.deferred = 0;
        self.cook_inner(registry, id, &mut pass).map(|(_, d)| d)
    }

    /// Cook every node of `ids` in one pass that works out each upstream
    /// node once. `cancel` is checked before each node that needs real work;
    /// once it is set the remaining nodes fail with [`CANCELLED`] and nothing
    /// cancelled is cached. `on_cook` is called before each node that needs
    /// real work (not a cache hit), e.g. for a progress display.
    pub fn cook_many(
        &mut self,
        registry: &Registry,
        ids: &[NodeId],
        cancel: Option<&AtomicBool>,
        on_cook: &mut dyn FnMut(NodeId),
    ) -> Vec<(NodeId, CookResult<Vec<Data>>)> {
        let mut pass = Pass {
            memo: HashMap::new(),
            visiting: HashSet::new(),
            cancel,
            on_cook,
        };
        self.visits = 0;
        self.deferred = 0;
        ids.iter()
            .map(|&id| (id, self.cook_inner(registry, id, &mut pass).map(|(_, d)| d)))
            .collect()
    }

    /// Cook a single output.
    pub fn cook_output(&mut self, registry: &Registry, id: NodeId, output: usize) -> CookResult<Data> {
        let outs = self.cook(registry, id)?;
        outs.into_iter()
            .nth(output)
            .ok_or_else(|| CookError::Other(format!("node {id} has no output {output}")))
    }

    fn cook_inner(&mut self, registry: &Registry, id: NodeId, pass: &mut Pass) -> CookResult<(u64, Vec<Data>)> {
        if let Some(done) = pass.memo.get(&id) {
            return done.clone();
        }
        if !pass.visiting.insert(id) {
            return Err(CookError::Cycle(id));
        }
        let result = self.cook_node(registry, id, pass);
        pass.visiting.remove(&id);
        pass.memo.insert(id, result.clone());
        result
    }

    fn cook_node(&mut self, registry: &Registry, id: NodeId, pass: &mut Pass) -> CookResult<(u64, Vec<Data>)> {
        self.visits += 1;
        if let Some(reason) = self.held.get(&id) {
            return Err(CookError::Other(reason.clone()));
        }
        let node = self.nodes.get(&id).ok_or(CookError::UnknownNode(id))?;
        let info = registry
            .info(&node.type_id)
            .ok_or_else(|| CookError::UnknownType(node.type_id.clone()))?;
        let op = registry
            .get(&node.type_id)
            .ok_or_else(|| CookError::UnknownType(node.type_id.clone()))?;
        let is_action = |name: &str| info.params.iter().any(|s| s.action && s.name == name);

        // Action parameters stay out of the signature: a result produced
        // with one switched on stays valid after it is switched off again.
        let mut hasher = DefaultHasher::new();
        node.type_id.hash(&mut hasher);
        node.params.hash_into(&mut hasher, &is_action);
        node.bypass.hash(&mut hasher);
        // A file the node reads is one of its inputs: its size and time
        // stamp join the signature, so editing or replacing it cooks again.
        for spec in &info.params {
            if let (ParamKind::Path { save: false, .. }, Some(ParamValue::Str(s))) = (&spec.kind, node.params.get(&spec.name)) {
                if !s.trim().is_empty() {
                    file_stamp(&self.resolve_path(s, false)).hash(&mut hasher);
                }
            }
        }
        let mut on = DefaultHasher::new();
        let mut any_on = false;
        for spec in info.params.iter().filter(|s| s.action) {
            if matches!(node.params.get(&spec.name), Some(ParamValue::Bool(true))) {
                spec.name.hash(&mut on);
                any_on = true;
            }
        }
        let actions = if any_on { on.finish() | 1 } else { 0 };
        let wires = node.inputs.clone();
        let bypass = node.bypass;

        let mut inputs: Vec<Option<Data>> = Vec::with_capacity(wires.len());
        for (i, wire) in wires.iter().enumerate() {
            match wire {
                Some((src, out)) => {
                    let (sig, outs) = self.cook_inner(registry, *src, pass)?;
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
        let signature = hasher.finish();

        if let Some(entry) = self.cache.get(&id) {
            // An action that is on must not be served a result made without it.
            if entry.signature == signature && (actions == 0 || entry.actions == actions) {
                return entry.result.clone().map(|d| (signature, d));
            }
        }
        if pass.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(CookError::Other(CANCELLED.into()));
        }
        // While the user drags or types, a writer keeps its last result
        // (nothing is cached for the new signature) unless an action asked
        // for this run explicitly.
        if self.defer_writes && actions == 0 && writes_files(info) {
            self.deferred += 1;
            return match self.cache.get(&id) {
                Some(entry) => entry.result.clone().map(|d| (entry.signature, d)),
                None => Err(CookError::Other(DEFERRED.into())),
            };
        }
        (pass.on_cook)(id);

        let node = self.nodes.get(&id).ok_or(CookError::UnknownNode(id))?;
        let name = node.name.clone();
        let result = if bypass {
            bypass_outputs(info, &name, &inputs)
        } else {
            let mut params = crate::param::clamp_to_specs(&info.params, &node.params);
            for spec in &info.params {
                if let ParamKind::Path { save, .. } = &spec.kind {
                    if let Some(ParamValue::Str(s)) = params.get(&spec.name) {
                        let resolved = self.resolve_path(s, *save);
                        if resolved != *s {
                            params.set(&spec.name, ParamValue::Str(resolved));
                        }
                    }
                }
            }
            self.check_inputs(info, &inputs)
                .and_then(|_| {
                    let ctx = CookContext {
                        node_name: name.clone(),
                        work_dir: self.work_dir(id, &name),
                        frame: self.frame,
                    };
                    cook_contained(op.as_ref(), &ctx, &inputs, &params)
                })
                .and_then(|outs| {
                    if outs.len() == info.outputs.len() {
                        Ok(outs)
                    } else {
                        Err(CookError::Failed {
                            node: name.clone(),
                            message: format!(
                                "returned {} outputs, declared {}",
                                outs.len(),
                                info.outputs.len()
                            ),
                        })
                    }
                })
                .and_then(|outs| {
                    // A runaway output would take the rest of the graph
                    // (and the viewport) down with it.
                    let budget = self.output_budget.unwrap_or(DEFAULT_OUTPUT_BUDGET);
                    for (i, d) in outs.iter().enumerate() {
                        if let Data::Geometry(g) = d {
                            let size = g.points.len() + g.prims.iter().map(|p| p.vertices.len()).sum::<usize>();
                            if size > budget {
                                return Err(CookError::Failed {
                                    node: name.clone(),
                                    message: format!(
                                        "output {i} has {size} points and vertices, over the budget of {budget}; lower the resolution"
                                    ),
                                });
                            }
                        }
                    }
                    Ok(outs)
                })
        };
        self.cache.insert(
            id,
            CacheEntry {
                signature,
                actions,
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

    /// Scratch directory of node `id`: `<work root>/n<id>_<name>` (see
    /// [`work_dir_name`]).
    pub fn work_dir(&self, id: NodeId, node_name: &str) -> PathBuf {
        self.work_root
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("vernon"))
            .join(work_dir_name(id, node_name))
    }

    /// The path a file parameter means: a relative `value` resolves
    /// against [`scene_dir`](Self::scene_dir). A file to write always does;
    /// a file to read does when it names a folder or exists there, so a
    /// bare program name (`colmap`) is still searched on `PATH`. Absolute
    /// paths, empty values and graphs without a scene folder are unchanged.
    pub fn resolve_path(&self, value: &str, save: bool) -> String {
        let Some(dir) = &self.scene_dir else {
            return value.to_string();
        };
        let path = std::path::Path::new(value);
        let prefixed = matches!(path.components().next(), Some(std::path::Component::Prefix(_)));
        if value.trim().is_empty() || !path.is_relative() || prefixed {
            return value.to_string();
        }
        let joined = dir.join(path);
        if save || value.contains(['/', '\\']) || joined.exists() {
            joined.to_string_lossy().into_owned()
        } else {
            value.to_string()
        }
    }

    /// Writer nodes that kept an older result in the last cook pass
    /// because [`defer_writes`](Self::defer_writes) was on.
    pub fn deferred_writes(&self) -> usize {
        self.deferred
    }

    /// Node evaluations (signature work included) in the last cook pass:
    /// at most one per node, however many paths reach it.
    pub fn last_visits(&self) -> usize {
        self.visits
    }

    /// A hash of the size and time stamp of every file the graph's nodes
    /// read. The app polls it to cook again when such a file changes.
    pub fn read_files_stamp(&self, registry: &Registry) -> u64 {
        let mut h = DefaultHasher::new();
        for node in self.nodes.values() {
            let Some(info) = registry.info(&node.type_id) else { continue };
            for spec in &info.params {
                if let (ParamKind::Path { save: false, .. }, Some(ParamValue::Str(s))) = (&spec.kind, node.params.get(&spec.name)) {
                    if !s.trim().is_empty() {
                        file_stamp(&self.resolve_path(s, false)).hash(&mut h);
                    }
                }
            }
        }
        h.finish()
    }

    /// Make a loaded graph cookable: order nodes by id, add parameters the
    /// file left at their defaults, pad unconnected trailing inputs, make
    /// every name valid and unique (see [`validate_node_name`]) and keep
    /// `next_id` above every id. Returns the type ids the registry does not
    /// know (those nodes are left untouched and fail when cooked).
    pub fn normalize(&mut self, registry: &Registry) -> Vec<String> {
        self.nodes.sort_keys();
        if let Some(max) = self.nodes.keys().max() {
            self.next_id = self.next_id.max(max + 1);
        }
        let mut unknown = Vec::new();
        let mut taken: HashSet<String> = HashSet::new();
        for node in self.nodes.values_mut() {
            let mut name = match validate_node_name(&node.name) {
                Ok(()) => node.name.clone(),
                Err(_) => sanitize_node_name(&node.name, node.type_id.rsplit('.').next().unwrap_or("node")),
            };
            if taken.contains(&name) {
                name = (1..)
                    .map(|i| format!("{name}_{i}"))
                    .find(|n| !taken.contains(n))
                    .expect("infinite iterator");
            }
            taken.insert(name.clone());
            node.name = name;
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

    /// Drop the cached result of one node, so it cooks again even when
    /// nothing changed (the app does this when an action is started).
    pub fn invalidate(&mut self, id: NodeId) {
        self.cache.remove(&id);
    }

    /// Take over `other`'s cached results for nodes this graph still has
    /// (after an undo, or from a copy cooked on another thread). An entry
    /// is only reused when its signature matches, so stale ones are harmless.
    pub fn absorb_cache(&mut self, other: Graph) {
        for (id, entry) in other.cache {
            if self.nodes.contains_key(&id) {
                self.cache.insert(id, entry);
            }
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("graph serialises")
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

/// Outputs of a bypassed node: each output takes the input at the same
/// index when its kind fits, else the first input whose kind fits. An
/// output no input can fill gets an empty value of its kind (geometry,
/// report, JSON null); a file output with nothing to pass fails.
fn bypass_outputs(info: &NodeTypeInfo, node: &str, inputs: &[Option<Data>]) -> CookResult<Vec<Data>> {
    let Some(first) = inputs.first().cloned().flatten() else {
        return Err(CookError::MissingInput {
            index: 0,
            name: "bypassed input".into(),
        });
    };
    if info.outputs.is_empty() {
        return Ok(vec![first]);
    }
    let fits = |d: &Option<Data>, kind: DataKind| d.as_ref().filter(|d| kinds_fit(d.kind(), kind)).cloned();
    info.outputs
        .iter()
        .enumerate()
        .map(|(i, port)| {
            if let Some(d) = inputs.get(i).and_then(|d| fits(d, port.kind)) {
                return Ok(d);
            }
            if let Some(d) = inputs.iter().find_map(|d| fits(d, port.kind)) {
                return Ok(d);
            }
            match port.kind {
                DataKind::Geometry => Ok(Data::geometry(vernon_core::Geometry::new())),
                DataKind::Report => {
                    let mut r = vernon_core::Report::new(format!("{} (bypassed)", info.label));
                    r.note(vernon_core::Severity::Info, "bypassed: no input carries this report");
                    Ok(Data::report(r))
                }
                DataKind::Json => Ok(Data::json(serde_json::Value::Null)),
                DataKind::File | DataKind::Any => Err(CookError::Failed {
                    node: node.to_string(),
                    message: format!("bypassed, and no input can stand in for output '{}' ({:?})", port.name, port.kind),
                }),
            }
        })
        .collect()
}

/// Message of a caught panic payload.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Cook one node, turning a panic inside the node into a cook error so a
/// single faulty node cannot take down the app or the CLI.
pub fn cook_contained(
    op: &dyn crate::NodeOp,
    ctx: &CookContext,
    inputs: &[Option<Data>],
    params: &Params,
) -> CookResult<Vec<Data>> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| op.cook(ctx, inputs, params))).unwrap_or_else(|payload| {
        Err(CookError::Failed {
            node: ctx.node_name.clone(),
            message: format!("internal error (panic): {}", panic_message(payload.as_ref())),
        })
    })
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

    struct PanicOp;
    impl NodeOp for PanicOp {
        fn info(&self) -> NodeTypeInfo {
            NodeTypeInfo::new("geo.panic", "Panic", Stage::Geometry)
                .output(PortSpec::new("geo", DataKind::Geometry))
                .param(ParamSpec::int("index", "Index", 0, 0, 3))
                .implemented()
        }
        fn cook(&self, _: &CookContext, _: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
            let table = [1.0, 2.0, 3.0, 4.0];
            let i = p.int("index")? as usize;
            if table[i] > 3.5 {
                panic!("boom at {i}");
            }
            Ok(vec![Data::geometry(Geometry::make_box(DVec3::splat(table[i])))])
        }
    }

    #[test]
    fn node_panics_become_cook_errors_and_params_are_clamped() {
        let mut reg = Registry::new();
        reg.add(PanicOp);
        let mut g = Graph::new();
        let n = g.add_node(&reg, "geo.panic").unwrap();
        // Out of the declared range (a hand-edited file): clamped to 3, so
        // no out-of-bounds index; index 3 then panics inside the node.
        g.set_param(n, "index", ParamValue::Int(i64::MAX)).unwrap();
        let err = g.cook(&reg, n).unwrap_err().to_string();
        assert!(err.contains("panic") && err.contains("boom at 3"), "{err}");
        // The graph stays usable after the panic.
        g.set_param(n, "index", ParamValue::Int(-5)).unwrap();
        let out = g.cook_output(&reg, n, 0).unwrap();
        assert!((out.as_geometry().unwrap().bounds().size().x - 1.0).abs() < 1e-9);
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

    struct MergeOp;
    impl NodeOp for MergeOp {
        fn info(&self) -> NodeTypeInfo {
            NodeTypeInfo::new("geo.merge2", "Merge", Stage::Geometry)
                .input(PortSpec::new("a", DataKind::Geometry))
                .input(PortSpec::new("b", DataKind::Geometry))
                .output(PortSpec::new("geo", DataKind::Geometry))
                .implemented()
        }
        fn cook(&self, _: &CookContext, ins: &[Option<Data>], _: &Params) -> CookResult<Vec<Data>> {
            Ok(vec![Data::Geometry(inputs::geometry(ins, 0)?)])
        }
    }

    #[test]
    fn reconvergent_graphs_cook_in_linear_time() {
        let mut reg = Registry::new();
        reg.add(BoxOp);
        reg.add(MergeOp);
        let mut g = Graph::new();
        let mut prev = g.add_node(&reg, "geo.box").unwrap();
        // 2^40 root-to-source paths: path-by-path signatures would never end.
        for _ in 0..40 {
            let m = g.add_node(&reg, "geo.merge2").unwrap();
            g.connect(prev, 0, m, 0).unwrap();
            g.connect(prev, 0, m, 1).unwrap();
            prev = m;
        }
        g.cook(&reg, prev).unwrap();
        assert_eq!(g.visits, 41);
        g.cook(&reg, prev).unwrap();
        assert_eq!(g.visits, 41);
        let ids: Vec<NodeId> = g.nodes.keys().copied().collect();
        let mut cooked = 0;
        let out = g.cook_many(&reg, &ids, None, &mut |_| cooked += 1);
        assert!(out.iter().all(|(_, r)| r.is_ok()));
        assert_eq!(g.visits, 41);
        assert_eq!(cooked, 0, "everything was cached");
    }

    struct DirOp;
    impl NodeOp for DirOp {
        fn info(&self) -> NodeTypeInfo {
            NodeTypeInfo::new("geo.dir", "Dir", Stage::Geometry)
                .output(PortSpec::new("dir", DataKind::File))
                .implemented()
        }
        fn cook(&self, ctx: &CookContext, _: &[Option<Data>], _: &Params) -> CookResult<Vec<Data>> {
            Ok(vec![Data::File(ctx.work_dir.clone())])
        }
    }

    struct FrameOp;
    impl NodeOp for FrameOp {
        fn info(&self) -> NodeTypeInfo {
            NodeTypeInfo::new("perf.frame", "Frame", Stage::Performance)
                .output(PortSpec::new("frame", DataKind::Json))
                .implemented()
        }
        fn cook(&self, ctx: &CookContext, _: &[Option<Data>], _: &Params) -> CookResult<Vec<Data>> {
            Ok(vec![Data::json(serde_json::json!(ctx.frame))])
        }
    }

    #[test]
    fn the_study_frame_reaches_the_cook_context() {
        let mut reg = Registry::new();
        reg.add(FrameOp);
        let mut g = Graph::new();
        g.frame = 12.0;
        let id = g.add_node(&reg, "perf.frame").unwrap();
        let out = g.cook_output(&reg, id, 0).unwrap();
        assert_eq!(out.as_json().unwrap().as_f64(), Some(12.0));
    }

    #[test]
    fn work_dirs_come_from_the_id_and_stay_in_the_root() {
        let mut reg = Registry::new();
        reg.add(DirOp);
        let mut g = Graph::new();
        g.work_root = Some(PathBuf::from("root"));
        for bad in ["../../escaped_dir", "C:\\x", "a/b", "x:*?.", "dir1"] {
            let id = g.add_node(&reg, "geo.dir").unwrap();
            g.nodes.get_mut(&id).unwrap().name = bad.into();
            let dir = g.cook_output(&reg, id, 0).unwrap().as_file().unwrap().to_path_buf();
            assert_eq!(dir.parent(), Some(std::path::Path::new("root")), "{bad}");
            let leaf = dir.file_name().unwrap().to_str().unwrap().to_string();
            assert!(leaf.starts_with(&format!("n{id}_")), "{leaf}");
            assert!(leaf.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'), "{leaf}");
        }
    }

    #[test]
    fn node_names_are_validated_and_repaired_on_load() {
        for ok in ["icp_align1", "Front splitter", "v2.1-final"] {
            assert!(validate_node_name(ok).is_ok(), "{ok}");
        }
        let long = "n".repeat(MAX_NAME_LEN + 1);
        for bad in ["", "..", "../x", "C:\\x", "a/b", "a*", "x.", " x", long.as_str()] {
            assert!(validate_node_name(bad).is_err(), "{bad}");
        }
        let mut reg = Registry::new();
        reg.add(BoxOp);
        let mut g = Graph::from_json(
            r#"{ "nodes": {
                "1": { "type_id": "geo.box", "name": "b" },
                "2": { "type_id": "geo.box", "name": "b" },
                "3": { "type_id": "geo.box", "name": "../../evil" },
                "4": { "type_id": "geo.box", "name": "" } } }"#,
        )
        .unwrap();
        g.normalize(&reg);
        let names: Vec<String> = g.nodes.values().map(|n| n.name.clone()).collect();
        assert_eq!(names[0], "b");
        assert_eq!(names[1], "b_1");
        assert_eq!(names[3], "box");
        assert!(names.iter().all(|n| validate_node_name(n).is_ok()), "{names:?}");
        assert_eq!(names.iter().collect::<HashSet<_>>().len(), 4);
    }

    struct RunOp(Arc<AtomicUsize>, Arc<AtomicUsize>);
    impl NodeOp for RunOp {
        fn info(&self) -> NodeTypeInfo {
            NodeTypeInfo::new("geo.run", "Run", Stage::Geometry)
                .output(PortSpec::new("geo", DataKind::Geometry))
                .param(ParamSpec::bool("run", "Run now", false).action())
                .param(ParamSpec::float("size", "Size", 1.0, 0.0, 10.0))
                .implemented()
        }
        fn cook(&self, _: &CookContext, _: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            if p.bool("run")? {
                self.1.fetch_add(1, Ordering::SeqCst);
            }
            Ok(vec![Data::geometry(Geometry::make_box(DVec3::splat(p.float("size")?)))])
        }
    }

    #[test]
    fn action_results_survive_switching_the_action_off() {
        let (cooks, runs) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let mut reg = Registry::new();
        reg.add(RunOp(cooks.clone(), runs.clone()));
        let mut g = Graph::new();
        let n = g.add_node(&reg, "geo.run").unwrap();
        let count = |a: &Arc<AtomicUsize>| a.load(Ordering::SeqCst);
        g.cook(&reg, n).unwrap();
        g.set_param(n, "run", ParamValue::Bool(true)).unwrap();
        g.cook(&reg, n).unwrap();
        assert_eq!((count(&cooks), count(&runs)), (2, 1), "switching the action on cooks again");
        g.cook(&reg, n).unwrap();
        g.set_param(n, "run", ParamValue::Bool(false)).unwrap();
        g.cook(&reg, n).unwrap();
        assert_eq!((count(&cooks), count(&runs)), (2, 1), "the run's result is kept");
        g.set_param(n, "size", ParamValue::Float(5.0)).unwrap();
        g.cook(&reg, n).unwrap();
        assert_eq!((count(&cooks), count(&runs)), (3, 1), "an edit re-cooks without running");
        g.set_param(n, "run", ParamValue::Bool(true)).unwrap();
        g.cook(&reg, n).unwrap();
        g.invalidate(n);
        g.cook(&reg, n).unwrap();
        assert_eq!(count(&runs), 3);
    }

    #[test]
    fn held_and_cancelled_nodes_cache_nothing() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut reg = Registry::new();
        reg.add(BoxOp);
        reg.add(CountOp(counter.clone()));
        let mut g = Graph::new();
        let b = g.add_node(&reg, "geo.box").unwrap();
        let c = g.add_node(&reg, "geo.count").unwrap();
        g.connect(b, 0, c, 0).unwrap();
        g.held.insert(c, "held until the scene is trusted".into());
        assert!(g.cook(&reg, c).unwrap_err().to_string().contains("trusted"));
        g.held.clear();
        let stop = AtomicBool::new(true);
        let out = g.cook_many(&reg, &[b, c], Some(&stop), &mut |_| {});
        assert!(out.iter().all(|(_, r)| r.as_ref().is_err_and(|e| e.to_string() == CANCELLED)));
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        g.cook(&reg, c).unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    struct ReportOp;
    impl NodeOp for ReportOp {
        fn info(&self) -> NodeTypeInfo {
            NodeTypeInfo::new("geo.rep", "Rep", Stage::Geometry)
                .input(PortSpec::new("geo", DataKind::Geometry))
                .output(PortSpec::new("geo", DataKind::Geometry))
                .output(PortSpec::new("report", DataKind::Report))
                .implemented()
        }
        fn cook(&self, _: &CookContext, ins: &[Option<Data>], _: &Params) -> CookResult<Vec<Data>> {
            Ok(vec![Data::Geometry(inputs::geometry(ins, 0)?), Data::report(vernon_core::Report::new("r"))])
        }
    }

    #[test]
    fn rejected_wires_leave_the_graph_unchanged() {
        let mut reg = Registry::new();
        reg.add(BoxOp);
        reg.add(CountOp(Arc::new(AtomicUsize::new(0))));
        reg.add(ReportOp);
        let mut g = Graph::new();
        let b = g.add_node(&reg, "geo.box").unwrap();
        let a = g.add_node(&reg, "geo.count").unwrap();
        let c = g.add_node(&reg, "geo.count").unwrap();
        let r = g.add_node(&reg, "geo.rep").unwrap();
        g.connect(b, 0, a, 0).unwrap();
        g.connect(a, 0, c, 0).unwrap();
        // A cycle is refused and `a` keeps its wire from the box.
        assert!(matches!(g.connect(c, 0, a, 0), Err(CookError::Cycle(_))));
        assert_eq!(g.nodes[&a].inputs, vec![Some((b, 0))]);
        assert!(g.connect(a, 0, a, 0).is_err(), "self loop");
        // No input 7: refused, and the input list does not grow.
        assert!(g.connect(b, 0, c, 7).is_err());
        assert_eq!(g.nodes[&c].inputs.len(), 1);
        // Port checks against the types.
        assert!(g.connect_checked(&reg, b, 3, c, 0).is_err(), "no output 3");
        assert!(matches!(
            g.connect_checked(&reg, r, 1, c, 0),
            Err(CookError::WrongInput { expected: DataKind::Geometry, found: DataKind::Report, .. })
        ));
        assert_eq!(g.nodes[&c].inputs, vec![Some((a, 0))]);
        g.connect_checked(&reg, b, 0, r, 0).unwrap();
        g.connect_checked(&reg, r, 0, c, 0).unwrap();
        assert_eq!(g.nodes[&c].inputs, vec![Some((r, 0))]);
    }

    #[test]
    fn bypass_fills_outputs_by_kind() {
        let mut reg = Registry::new();
        reg.add(BoxOp);
        reg.add(ReportOp);
        let mut g = Graph::new();
        let b = g.add_node(&reg, "geo.box").unwrap();
        let r = g.add_node(&reg, "geo.rep").unwrap();
        g.connect(b, 0, r, 0).unwrap();
        g.nodes.get_mut(&r).unwrap().bypass = true;
        let out = g.cook(&reg, r).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind(), DataKind::Geometry);
        assert!(out[1].as_report().is_some_and(|r| r.title.contains("bypassed")), "{:?}", out[1].kind());
    }

    #[test]
    fn outputs_over_the_budget_fail_their_node() {
        let mut reg = Registry::new();
        reg.add(BoxOp);
        let mut g = Graph::new();
        let b = g.add_node(&reg, "geo.box").unwrap();
        g.output_budget = Some(10);
        let err = g.cook(&reg, b).unwrap_err().to_string();
        assert!(err.contains("budget"), "{err}");
        g.output_budget = None;
        g.invalidate(b);
        assert!(g.cook(&reg, b).is_ok());
    }

    struct ReadOp(Arc<AtomicUsize>);
    impl NodeOp for ReadOp {
        fn info(&self) -> NodeTypeInfo {
            NodeTypeInfo::new("geo.read", "Read", Stage::Geometry)
                .output(PortSpec::new("geo", DataKind::Geometry))
                .param(ParamSpec::path("file", "File", &["txt"], false))
                .implemented()
        }
        fn cook(&self, _: &CookContext, _: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let n = std::fs::read(p.str("file")?)?.len() as f64;
            Ok(vec![Data::geometry(Geometry::make_box(DVec3::splat(n)))])
        }
    }

    struct WriteOp(Arc<AtomicUsize>);
    impl NodeOp for WriteOp {
        fn info(&self) -> NodeTypeInfo {
            NodeTypeInfo::new("geo.write", "Write", Stage::Geometry)
                .input(PortSpec::new("geo", DataKind::Geometry))
                .output(PortSpec::new("geo", DataKind::Geometry))
                .param(ParamSpec::path("file", "File", &["txt"], true))
                .implemented()
        }
        fn cook(&self, _: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let geo = inputs::geometry(ins, 0)?;
            std::fs::write(p.str("file")?, format!("{}", geo.points.len()))?;
            Ok(vec![Data::Geometry(geo)])
        }
    }

    #[test]
    fn read_files_join_the_signature_and_writes_wait_for_the_edit() {
        let dir = std::env::temp_dir().join(format!("vernon-graph-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (reads, writes) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let mut reg = Registry::new();
        reg.add(ReadOp(reads.clone()));
        reg.add(WriteOp(writes.clone()));
        reg.add(BoxOp);
        assert!(writes_files(reg.info("geo.write").unwrap()) && !writes_files(reg.info("geo.read").unwrap()));
        let mut g = Graph::new();
        g.scene_dir = Some(dir.clone());
        let r = g.add_node(&reg, "geo.read").unwrap();
        // Relative paths resolve against the scene folder.
        std::fs::write(dir.join("in.txt"), "abc").unwrap();
        g.set_param(r, "file", ParamValue::Str("in.txt".into())).unwrap();
        let size = |g: &mut Graph| g.cook_output(&reg, r, 0).unwrap().as_geometry().unwrap().bounds().size().x;
        assert_eq!(size(&mut g), 3.0);
        let stamp = g.read_files_stamp(&reg);
        assert_eq!(size(&mut g), 3.0);
        assert_eq!(reads.load(Ordering::SeqCst), 1, "cached while the file is unchanged");
        std::fs::write(dir.join("in.txt"), "abcdefg").unwrap();
        assert_ne!(g.read_files_stamp(&reg), stamp);
        assert_eq!(size(&mut g), 7.0, "an edited file cooks again");
        assert_eq!(g.resolve_path("colmap", false), "colmap", "bare program names stay on PATH");

        let b = g.add_node(&reg, "geo.box").unwrap();
        let w = g.add_node(&reg, "geo.write").unwrap();
        g.connect(b, 0, w, 0).unwrap();
        g.set_param(w, "file", ParamValue::Str("out.txt".into())).unwrap();
        g.defer_writes = true;
        assert!(g.cook(&reg, w).unwrap_err().to_string().contains(DEFERRED), "nothing to keep yet");
        assert_eq!((writes.load(Ordering::SeqCst), g.deferred_writes()), (0, 1));
        g.defer_writes = false;
        g.cook(&reg, w).unwrap();
        assert!(dir.join("out.txt").is_file(), "save paths resolve against the scene folder");
        g.defer_writes = true;
        for size in [20.0, 30.0, 40.0] {
            g.set_param(b, "size", ParamValue::Float(size)).unwrap();
            let kept = g.cook_output(&reg, w, 0).unwrap();
            assert_eq!(kept.as_geometry().unwrap().bounds().size().x, 10.0, "the last written result is kept");
        }
        assert_eq!(writes.load(Ordering::SeqCst), 1, "no writes while the edit runs");
        g.defer_writes = false;
        let done = g.cook_output(&reg, w, 0).unwrap();
        assert_eq!(done.as_geometry().unwrap().bounds().size().x, 40.0);
        assert_eq!((writes.load(Ordering::SeqCst), g.deferred_writes()), (2, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
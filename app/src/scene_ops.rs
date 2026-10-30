//! Scene commands behind the explorer, the Modify panel and the Utilities
//! tab (spec 2.7, 2.10, 4.1): lock, isolate, hide unselected, Make Unique,
//! sub-graph copy and paste, external-tool paths and the report export.
//! Headless and unit-tested.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};
use vernon_graph::{validate_node_name, DataKind, NodeId, NodeInstance, ParamKind, ParamValue};

use crate::gate::{margin, metric_state};
use crate::state::{write_atomic, AppState};

/// Parameter names that hold the location of an external program.
pub const TOOL_PATH_PARAMS: [&str; 3] = ["executable", "bin_dir", "exe_path"];

/// Marks clipboard text as Vernon nodes, with its format version.
const CLIPBOARD_FORMAT: u32 = 1;

/// How far a pasted sub-graph lands from the original in the node editor.
const PASTE_OFFSET: f32 = 40.0;

/// Selected nodes on the clipboard. `inputs` only hold wires between them.
#[derive(Debug, Serialize, Deserialize)]
struct ClipboardNodes {
    vernon_nodes: u32,
    nodes: BTreeMap<NodeId, NodeInstance>,
    #[serde(default)]
    colors: BTreeMap<NodeId, [u8; 3]>,
}

/// String parameters holding a secret, which never go on the clipboard.
fn is_secret(name: &str) -> bool {
    ["key", "token", "password"].iter().any(|k| name.contains(k))
}

/// A free name for a copy of `name`: the name itself when nothing has it,
/// else its stem with the first free number. `None` when no valid name
/// comes of it (the node then keeps the name of its type).
fn paste_name(name: &str, taken: &dyn Fn(&str) -> bool) -> Option<String> {
    validate_node_name(name).ok()?;
    if !taken(name) {
        return Some(name.to_string());
    }
    let stem = name.trim_end_matches(|c: char| c.is_ascii_digit());
    if stem.is_empty() {
        return None;
    }
    let free = (1..).map(|i| format!("{stem}{i}")).find(|n| !taken(n))?;
    validate_node_name(&free).ok().map(|()| free)
}

/// One external program the scene drives, with the nodes that run it.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternalTool {
    pub tool: String,
    /// (node, path parameter name, current value).
    pub nodes: Vec<(NodeId, String, String)>,
}

impl AppState {
    pub fn is_locked(&self, id: NodeId) -> bool {
        self.scene.locked.contains(&self.object_of(id))
    }

    /// Locked objects cannot be picked in the viewports and the transform
    /// tools and type-in leave them alone.
    pub fn set_locked(&mut self, object: NodeId, locked: bool) {
        if locked {
            self.scene.locked.insert(object);
        } else {
            self.scene.locked.remove(&object);
        }
        self.dirty = true;
    }

    /// Hide every visible object outside `keep` (objects or nodes inside
    /// them). Returns the objects hidden, so the isolation can be ended.
    pub fn isolate(&mut self, keep: &[NodeId]) -> Vec<NodeId> {
        let keep: Vec<NodeId> = keep.iter().map(|k| self.object_of(*k)).collect();
        let hide: Vec<NodeId> = self
            .objects()
            .into_iter()
            .filter(|o| !keep.contains(o) && !self.is_hidden(*o))
            .collect();
        for o in &hide {
            self.set_hidden(*o, true);
        }
        hide
    }

    /// Show the objects an isolation hid (those still in the scene).
    pub fn end_isolate(&mut self, hidden: &[NodeId]) {
        for o in hidden {
            if self.graph.nodes.contains_key(o) {
                self.set_hidden(*o, false);
            }
        }
    }

    /// Hide every object that is not selected.
    pub fn hide_unselected(&mut self) -> usize {
        let keep = self.selection.clone();
        self.isolate(&keep).len()
    }

    /// Make Unique: give `object` its own copy of the shared stack entry
    /// `entry` (same type, parameters, inputs and bypass), wired into its
    /// stack in place of the shared one. The other objects keep the
    /// original. Returns the copy.
    pub fn make_unique(&mut self, object: NodeId, entry: NodeId) -> Result<NodeId, String> {
        let stack = self.graph.stack(object);
        let at = stack.iter().position(|n| *n == entry).ok_or("the entry is not in this object's stack")?;
        let next = *stack.get(at + 1).ok_or("the top of a stack is not shared")?;
        if self.consumers(entry).len() < 2 {
            return Err(format!("{} is not shared", self.name(entry)));
        }
        let orig = self.graph.nodes[&entry].clone();
        let slot = self.graph.nodes[&next]
            .inputs
            .iter()
            .position(|w| matches!(w, Some((s, _)) if *s == entry))
            .ok_or("the entry does not feed the next one")?;
        let out = self.graph.nodes[&next].inputs[slot].map(|(_, o)| o).unwrap_or(0);
        self.checkpoint();
        let id = self.graph.add_node(&self.registry, &orig.type_id).map_err(|e| e.to_string())?;
        let name = self.graph.nodes[&id].name.clone();
        if let Some(n) = self.graph.nodes.get_mut(&id) {
            n.params = orig.params.clone();
            n.bypass = orig.bypass;
            n.pos = [orig.pos[0] + 40.0, orig.pos[1] + 40.0];
            n.name = name;
        }
        for (i, w) in orig.inputs.iter().enumerate() {
            if let Some((s, o)) = w {
                self.graph.connect(*s, *o, id, i).map_err(|e| e.to_string())?;
            }
        }
        self.graph.connect(id, out, next, slot).map_err(|e| e.to_string())?;
        if self.stack_entry == Some(entry) {
            self.stack_entry = Some(id);
        }
        self.touch();
        self.info_text(format!("{} now has its own copy of {} ({})", self.name(object), self.name(entry), self.name(id)));
        Ok(id)
    }

    /// The selected nodes and the wires between them as clipboard JSON, with
    /// run switches off and access keys blank. Wires from outside are left out.
    pub fn copy_selection(&self) -> Result<String, String> {
        let mut clip = ClipboardNodes {
            vernon_nodes: CLIPBOARD_FORMAT,
            nodes: BTreeMap::new(),
            colors: BTreeMap::new(),
        };
        for id in &self.selection {
            let Some(node) = self.graph.nodes.get(id) else { continue };
            let mut node = node.clone();
            for input in &mut node.inputs {
                if input.is_some_and(|(s, _)| !self.selection.contains(&s)) {
                    *input = None;
                }
            }
            if let Some(info) = self.registry.info(&node.type_id) {
                for spec in &info.params {
                    if spec.action {
                        node.params.set(&spec.name, ParamValue::Bool(false));
                    } else if matches!(spec.kind, ParamKind::Str { .. }) && is_secret(&spec.name) {
                        node.params.set(&spec.name, ParamValue::Str(String::new()));
                    }
                }
            }
            clip.nodes.insert(*id, node);
            if let Some(c) = self.scene.colors.get(id) {
                clip.colors.insert(*id, *c);
            }
        }
        if clip.nodes.is_empty() {
            return Err("nothing selected to copy".into());
        }
        serde_json::to_string_pretty(&clip).map_err(|e| e.to_string())
    }

    /// Add the nodes of clipboard JSON with fresh ids and unique names, as
    /// one undo step, and select them. The text may come from anywhere, so it
    /// is treated like an opened file: bad text changes nothing, run switches
    /// stay off, and programs or file paths it brings ask for trust again.
    pub fn paste_nodes(&mut self, text: &str) -> Result<Vec<NodeId>, String> {
        let clip: ClipboardNodes = serde_json::from_str(text).map_err(|_| "the clipboard does not hold Vernon nodes")?;
        if clip.vernon_nodes != CLIPBOARD_FORMAT {
            return Err(format!("clipboard nodes use format {}, this version reads {CLIPBOARD_FORMAT}", clip.vernon_nodes));
        }
        if clip.nodes.is_empty() {
            return Err("the clipboard holds no nodes".into());
        }
        if let Some(n) = clip.nodes.values().find(|n| self.registry.info(&n.type_id).is_none()) {
            return Err(format!("unknown node type '{}' on the clipboard", n.type_id));
        }
        let findings_before = self.trust_findings().len();
        let ids = self.as_one_undo_step(|s| s.insert_clipboard(&clip));
        if self.trust_findings().len() > findings_before {
            self.trusted = false;
            self.trust_prompt = true;
        }
        self.info_text(format!("pasted {} node(s)", ids.len()));
        Ok(ids)
    }

    fn insert_clipboard(&mut self, clip: &ClipboardNodes) -> Vec<NodeId> {
        let mut fresh: HashMap<NodeId, NodeId> = HashMap::new();
        for (old, copied) in &clip.nodes {
            let Ok(id) = self.graph.add_node(&self.registry, &copied.type_id) else { continue };
            fresh.insert(*old, id);
            let name = paste_name(&copied.name, &|n| self.graph.nodes.iter().any(|(o, node)| *o != id && node.name == n));
            let Some(info) = self.registry.info(&copied.type_id) else { continue };
            let Some(node) = self.graph.nodes.get_mut(&id) else { continue };
            // Parameters the text leaves out keep the defaults from add_node.
            for (k, v) in &copied.params.0 {
                node.params.set(k, v.clone());
            }
            for spec in info.params.iter().filter(|s| s.action) {
                node.params.set(&spec.name, ParamValue::Bool(false));
            }
            node.bypass = copied.bypass;
            node.pos = [copied.pos[0] + PASTE_OFFSET, copied.pos[1] + PASTE_OFFSET];
            node.expect_fail = copied.expect_fail.clone();
            if let Some(name) = name {
                node.name = name;
            }
            if let Some(c) = clip.colors.get(old) {
                self.scene.colors.insert(id, *c);
            }
        }
        let mut dropped = 0;
        for (old, copied) in &clip.nodes {
            for (slot, wire) in copied.inputs.iter().enumerate() {
                let Some((src, out)) = wire else { continue };
                let wired = fresh
                    .get(src)
                    .is_some_and(|s| self.graph.connect_checked(&self.registry, *s, *out, fresh[old], slot).is_ok());
                if !wired {
                    dropped += 1;
                }
            }
        }
        if dropped > 0 {
            self.error_text(format!("paste: {dropped} wire(s) on the clipboard do not fit and were left out"));
        }
        let ids: Vec<NodeId> = clip.nodes.keys().filter_map(|o| fresh.get(o).copied()).collect();
        self.selection = ids.clone();
        self.stack_entry = None;
        self.touch();
        ids
    }

    /// Hide every object that outputs geometry, or show them all again when
    /// all are hidden. Returns the objects changed and whether they are hidden.
    pub fn toggle_geometry_hidden(&mut self) -> (usize, bool) {
        let geometry: Vec<NodeId> = self
            .objects()
            .into_iter()
            .filter(|o| self.info(*o).and_then(|i| i.outputs.first()).is_some_and(|p| p.kind == DataKind::Geometry))
            .collect();
        let hide = geometry.iter().any(|o| !self.is_hidden(*o));
        let change: Vec<NodeId> = geometry.into_iter().filter(|o| self.is_hidden(*o) != hide).collect();
        for o in &change {
            self.set_hidden(*o, hide);
        }
        (change.len(), hide)
    }

    /// External programs the scene's nodes run, grouped by program.
    pub fn external_tools(&self) -> Vec<ExternalTool> {
        let mut by: BTreeMap<String, Vec<(NodeId, String, String)>> = BTreeMap::new();
        for (id, n) in &self.graph.nodes {
            let Some(info) = self.registry.info(&n.type_id) else { continue };
            let Some(tool) = &info.external_tool else { continue };
            let param = info
                .params
                .iter()
                .find(|p| matches!(p.kind, ParamKind::Path { .. }) && TOOL_PATH_PARAMS.contains(&p.name.as_str()));
            let entry = by.entry(tool.clone()).or_default();
            if let Some(p) = param {
                entry.push((*id, p.name.clone(), n.params.str(&p.name).unwrap_or_default().to_string()));
            }
        }
        by.into_iter().map(|(tool, nodes)| ExternalTool { tool, nodes }).collect()
    }

    /// Set the program location of every node that runs `tool`. Returns
    /// the number of nodes changed.
    pub fn set_tool_path(&mut self, tool: &str, path: &str) -> usize {
        let targets: Vec<(NodeId, String)> = self
            .external_tools()
            .into_iter()
            .filter(|t| t.tool == tool)
            .flat_map(|t| t.nodes)
            .filter(|(_, _, v)| v != path)
            .map(|(id, p, _)| (id, p))
            .collect();
        if targets.is_empty() {
            return 0;
        }
        self.checkpoint();
        for (id, p) in &targets {
            self.set_param_silent(*id, p, ParamValue::Str(path.to_string()));
        }
        targets.len()
    }

    /// Write every report metric to a CSV file (node, report, metric,
    /// value, unit, limit, margin, state) and the notes after them.
    /// Returns the number of metric rows.
    pub fn export_reports(&self, path: &Path) -> Result<usize, String> {
        let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let mut csv = String::from("node,report,metric,value,unit,limit,margin,state\n");
        let mut notes = String::new();
        let mut rows = 0;
        for n in self.report_nodes() {
            let Some(r) = self.report(n) else { continue };
            for (k, m) in &r.metrics {
                let limit = m.limit.map(|l| l.to_string()).unwrap_or_default();
                let mg = margin(m).map(|v| v.to_string()).unwrap_or_default();
                csv.push_str(&format!(
                    "{},{},{},{},{},{limit},{mg},{}\n",
                    q(self.name(n)),
                    q(&r.title),
                    q(k),
                    m.value,
                    q(&m.unit),
                    metric_state(m).text()
                ));
                rows += 1;
            }
            for (sev, text) in &r.notes {
                notes.push_str(&format!("# {} {:?}: {}\n", self.name(n), sev, text.replace('\n', " ")));
            }
        }
        csv.push_str(&notes);
        write_atomic(path, csv.as_bytes())?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::build_registry;

    fn state() -> AppState {
        AppState::new(build_registry())
    }

    #[test]
    fn isolate_hides_the_rest_and_ends_cleanly() {
        let mut s = state();
        let a = s.create("geo.box").unwrap();
        s.clear_selection();
        let b = s.create("geo.box").unwrap();
        s.clear_selection();
        let c = s.create("geo.box").unwrap();
        s.set_hidden(c, true);
        let hidden = s.isolate(&[a]);
        assert_eq!(hidden, vec![b], "c was hidden already, a is kept");
        assert!(s.is_hidden(b) && !s.is_hidden(a));
        s.end_isolate(&hidden);
        assert!(!s.is_hidden(b) && s.is_hidden(c));
        s.select_only(b);
        assert_eq!(s.hide_unselected(), 1);
        assert!(s.is_hidden(a) && !s.is_hidden(b));
    }

    #[test]
    fn locking_is_per_object_and_saved() {
        let mut s = state();
        let b = s.create("geo.box").unwrap();
        let t = s.ensure_transform(b).unwrap();
        s.set_locked(t, true);
        assert!(s.is_locked(b), "a node inside a locked object is locked");
        let text = s.to_vrn().unwrap();
        let mut s2 = state();
        s2.load_vrn(&text).unwrap();
        assert!(s2.is_locked(t));
    }

    #[test]
    fn make_unique_detaches_a_shared_entry() {
        let mut s = state();
        let b = s.create("geo.box").unwrap();
        let sm = s.create("clean.smooth_mesh").unwrap();
        // A second object built on the same smoothing node.
        let t1 = s.add_modifier("geo.transform").unwrap();
        s.clear_selection();
        let t2 = s.graph.add_node(&s.registry, "geo.transform").unwrap();
        s.graph.connect(sm, 0, t2, 0).unwrap();
        assert_eq!(s.consumers(sm).len(), 2);
        s.set_param(sm, "iterations", ParamValue::Int(7), 0.0);
        let copy = s.make_unique(t2, sm).unwrap();
        assert_ne!(copy, sm);
        assert_eq!(s.graph.stack(t2), vec![b, copy, t2]);
        assert_eq!(s.graph.stack(t1), vec![b, sm, t1], "the other object keeps the original");
        assert_eq!(s.graph.nodes[&copy].params, s.graph.nodes[&sm].params);
        assert!(s.make_unique(t2, copy).is_err(), "the copy is not shared");
        assert!(s.undo());
        assert_eq!(s.graph.stack(t2), vec![b, sm, t2]);
    }

    #[test]
    fn copy_keeps_internal_wires_and_paste_makes_fresh_ids_and_names() {
        let mut s = state();
        let outside = s.create("geo.box").unwrap();
        let a = s.create("clean.smooth_mesh").unwrap();
        let b = s.create("geo.transform").unwrap();
        s.set_param(a, "iterations", ParamValue::Int(7), 0.0);
        s.set_color(a, [1, 2, 3]);
        s.selection = vec![a, b];
        let text = s.copy_selection().unwrap();
        let before = s.graph.nodes.len();

        let pasted = s.paste_nodes(&text).unwrap();
        assert_eq!(pasted.len(), 2);
        assert_eq!(s.selection, pasted);
        let (a2, b2) = (pasted[0], pasted[1]);
        assert!(![outside, a, b].contains(&a2) && ![outside, a, b].contains(&b2), "fresh ids");
        assert_eq!(s.graph.nodes[&a2].inputs[0], None, "the wire from outside stays behind");
        assert_eq!(s.graph.nodes[&b2].inputs[0], Some((a2, 0)), "the internal wire follows the new ids");
        assert_eq!(s.graph.nodes[&b].inputs[0], Some((a, 0)), "the originals are untouched");
        assert_eq!(s.graph.nodes[&a2].params, s.graph.nodes[&a].params);
        assert_eq!(s.scene.colors.get(&a2), Some(&[1, 2, 3]));
        assert_eq!(s.graph.nodes[&a2].pos[0], s.graph.nodes[&a].pos[0] + PASTE_OFFSET);

        // Unique names, also over repeated pastes.
        assert_eq!(s.name(a), "smooth_mesh1");
        assert_eq!(s.name(a2), "smooth_mesh2");
        let third = s.paste_nodes(&text).unwrap();
        assert_eq!(s.name(third[0]), "smooth_mesh3");
        let mut names: Vec<&str> = s.graph.nodes.values().map(|n| n.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), s.graph.nodes.len());

        // Each paste is one undo step.
        assert!(s.undo() && s.undo());
        assert_eq!(s.graph.nodes.len(), before);
    }

    #[test]
    fn paste_keeps_a_free_name_and_renumbers_a_taken_one() {
        let taken = |n: &str| ["box1", "box2", "part", "123"].contains(&n);
        assert_eq!(paste_name("free", &taken).as_deref(), Some("free"));
        assert_eq!(paste_name("box1", &taken).as_deref(), Some("box3"));
        assert_eq!(paste_name("part", &taken).as_deref(), Some("part1"));
        assert_eq!(paste_name("123", &taken), None, "nothing is left to number");
        assert_eq!(paste_name("../evil", &taken), None, "not a valid name");
        let long = "a".repeat(64);
        assert_eq!(paste_name(&long, &|n| n == long), None, "a number would make it too long");
    }

    #[test]
    fn bad_clipboard_text_changes_nothing() {
        let mut s = state();
        s.create("geo.box").unwrap();
        let before = s.graph.to_json();
        for bad in [
            "",
            "plain text",
            "{}",
            "[1]",
            r#"{"vernon_nodes": 99, "nodes": {}}"#,
            r#"{"vernon_nodes": 1, "nodes": {}}"#,
            r#"{"vernon_nodes": 1, "nodes": {"1": {"type_id": "no.such", "name": "x"}}}"#,
        ] {
            assert!(s.paste_nodes(bad).is_err(), "{bad}");
            assert_eq!(s.graph.to_json(), before, "{bad}");
        }
        assert_eq!(s.selection.len(), 1);
    }

    #[test]
    fn hand_written_clipboard_text_is_repaired_or_refused_safely() {
        let mut s = state();
        // Missing params take defaults; a dangling wire, a wire to a missing
        // output and a two-node loop cannot be made.
        let text = r#"{"vernon_nodes": 1, "nodes": {
            "5": {"type_id": "clean.smooth_mesh", "name": "bad/name", "inputs": [[6, 0]]},
            "6": {"type_id": "clean.smooth_mesh", "name": "ok", "inputs": [[5, 0]]},
            "7": {"type_id": "geo.transform", "name": "t", "inputs": [[99, 0]]},
            "8": {"type_id": "geo.transform", "name": "u", "inputs": [[6, 42], null, [6, 0]]}
        }}"#;
        let ids = s.paste_nodes(text).unwrap();
        assert_eq!(ids.len(), 4);
        assert_eq!(s.name(ids[0]), "smooth_mesh1", "an invalid name is replaced");
        assert_eq!(s.name(ids[1]), "ok");
        assert!(s.graph.nodes[&ids[0]].params.get("iterations").is_some());
        assert_eq!(s.graph.nodes[&ids[2]].inputs[0], None);
        assert_eq!(s.graph.nodes[&ids[3]].inputs[0], None);
        let wired = ids[..2].iter().filter(|i| s.graph.nodes[*i].inputs[0].is_some()).count();
        assert_eq!(wired, 1, "the loop is broken");
        assert!(s.log.iter().any(|l| l.error && l.text.contains("wire(s)")));
    }

    #[test]
    fn clipboard_never_carries_secrets_or_starts_programs() {
        let mut s = state();
        let id = s.graph.add_node(&s.registry, "print.remote_upload").unwrap();
        s.selection = vec![id];
        s.set_param_silent(id, "api_key", ParamValue::Str("hunter2".into()));
        let text = s.copy_selection().unwrap();
        assert!(!text.contains("hunter2"), "{text}");

        // Text from elsewhere with the switch on, naming a program: pasted
        // off, and the scene asks for trust again.
        let mut t = state();
        let solver = r#"{"vernon_nodes": 1, "nodes": {"1": {"type_id": "fea.solve_external", "name": "s",
            "params": {"exe_path": "C:/evil.exe", "run": true}}}}"#;
        let ids = t.paste_nodes(solver).unwrap();
        assert_eq!(t.graph.nodes[&ids[0]].params.get("run"), Some(&ParamValue::Bool(false)));
        assert!(!t.trusted && t.trust_prompt, "a pasted program needs trust");
    }

    #[test]
    fn geometry_objects_hide_and_show_together() {
        let mut s = state();
        let a = s.create("geo.box").unwrap();
        s.clear_selection();
        let b = s.create("geo.box").unwrap();
        s.set_hidden(b, true);
        assert_eq!(s.toggle_geometry_hidden(), (1, true), "the one still showing");
        assert!(s.is_hidden(a) && s.is_hidden(b));
        assert_eq!(s.toggle_geometry_hidden(), (2, false));
        assert!(!s.is_hidden(a) && !s.is_hidden(b));
    }

    #[test]
    fn tool_paths_and_report_export() {
        let mut s = state();
        let a = s.create("fea.solve_external").unwrap();
        s.clear_selection();
        let b = s.create("fea.solve_external").unwrap();
        let tools = s.external_tools();
        let solver = tools.iter().find(|t| t.nodes.iter().any(|(id, _, _)| *id == a)).expect("solver tool listed");
        assert_eq!(solver.nodes.len(), 2);
        let name = solver.tool.clone();
        assert_eq!(s.set_tool_path(&name, "C:/tools/solver.exe"), 2);
        assert_eq!(s.graph.nodes[&b].params.str("exe_path").ok(), Some("C:/tools/solver.exe"));
        assert_eq!(s.set_tool_path(&name, "C:/tools/solver.exe"), 0, "nothing left to change");

        let mut r = state();
        r.create("geo.box").unwrap();
        r.create("geo.measure").unwrap();
        r.cook_all();
        let dir = std::env::temp_dir().join(format!("vernon_report_export_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("reports.csv");
        let rows = r.export_reports(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(rows > 0, "{text}");
        assert!(text.starts_with("node,report,metric,value,unit,limit,margin,state\n"));
        assert_eq!(text.lines().filter(|l| !l.starts_with('#')).count(), rows + 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

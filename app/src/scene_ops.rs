//! Scene commands behind the explorer, the Modify panel and the Utilities
//! tab (spec 2.7, 2.10, 4.1): lock, isolate, hide unselected, Make Unique,
//! external-tool paths and the report export. Headless and unit-tested.

use std::collections::BTreeMap;
use std::path::Path;

use vernon_graph::{NodeId, ParamKind, ParamValue};

use crate::gate::{margin, metric_state};
use crate::state::{write_atomic, AppState};

/// Parameter names that hold the location of an external program.
pub const TOOL_PATH_PARAMS: [&str; 3] = ["executable", "bin_dir", "exe_path"];

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

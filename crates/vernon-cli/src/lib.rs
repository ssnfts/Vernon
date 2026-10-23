//! Headless Vernon: the full node registry, `.vrn` loading and cooking, and
//! the text/Markdown node catalogue. `main.rs` is a thin argument parser
//! over these functions so the integration tests can call them directly.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use vernon_core::{Report, Severity};
use vernon_graph::{Data, Graph, NodeId, NodeTypeInfo, Registry, Stage};

/// Every node type shipped with Vernon, from all stage crates.
pub fn registry() -> Registry {
    let mut reg = Registry::new();
    vernon_nodes_geo::register(&mut reg);
    vernon_nodes_capture::register(&mut reg);
    vernon_nodes_clean::register(&mut reg);
    vernon_nodes_surface::register(&mut reg);
    vernon_nodes_print::register(&mut reg);
    vernon_nodes_cfd::register(&mut reg);
    vernon_nodes_fea::register(&mut reg);
    vernon_nodes_solar::register(&mut reg);
    vernon_nodes_perf::register(&mut reg);
    reg
}

/// Parse a `.vrn` graph. Hand-written files may leave out `next_id`,
/// `params` entries that keep their defaults and trailing unconnected
/// inputs; these are filled in from the registry so every node can cook.
pub fn parse_graph(text: &str, reg: &Registry) -> Result<Graph> {
    let mut graph = Graph::from_json(text).context("not a valid .vrn graph")?;
    let unknown = graph.normalize(reg);
    if !unknown.is_empty() {
        bail!("unknown node type(s): {}", unknown.join(", "));
    }
    Ok(graph)
}

/// Read and parse a `.vrn` file.
pub fn load_graph(path: &Path, reg: &Registry) -> Result<Graph> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_graph(&text, reg).with_context(|| format!("loading {}", path.display()))
}

/// The result of cooking one node.
#[derive(Debug, Clone)]
pub struct NodeOutcome {
    pub id: NodeId,
    pub name: String,
    pub type_id: String,
    pub stage: Option<Stage>,
    pub result: Result<Vec<Data>, String>,
}

impl NodeOutcome {
    pub fn reports(&self) -> impl Iterator<Item = &Report> {
        self.result
            .iter()
            .flatten()
            .filter_map(|d| d.as_report().map(|r| r.as_ref()))
    }
}

/// Cook `only` (by instance name) or every node in file order. A failing
/// node is recorded, never a panic; downstream nodes report the same error.
pub fn cook_graph(
    graph: &mut Graph,
    reg: &Registry,
    only: Option<&str>,
) -> Result<Vec<NodeOutcome>> {
    let ids: Vec<NodeId> = match only {
        Some(name) => vec![graph
            .find(name)
            .ok_or_else(|| anyhow!("no node named '{name}'"))?],
        None => graph.nodes.keys().copied().collect(),
    };
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let node = &graph.nodes[&id];
        let (name, type_id) = (node.name.clone(), node.type_id.clone());
        let stage = reg.info(&type_id).map(|i| i.stage);
        let result = graph.cook(reg, id).map_err(|e| e.to_string());
        out.push(NodeOutcome {
            id,
            name,
            type_id,
            stage,
            result,
        });
    }
    Ok(out)
}

/// Pick the geometry to export: the named node, else the display node,
/// else the last node in the file that produced geometry.
pub fn export_geometry(
    outcomes: &[NodeOutcome],
    graph: &Graph,
    only: Option<&str>,
) -> Option<(String, Data)> {
    let first_geo = |o: &NodeOutcome| {
        o.result
            .as_ref()
            .ok()
            .and_then(|d| d.iter().find(|d| d.as_geometry().is_some()).cloned())
            .map(|d| (o.name.clone(), d))
    };
    if only.is_some() {
        return outcomes.first().and_then(first_geo);
    }
    if let Some(disp) = graph.display {
        if let Some(found) = outcomes.iter().find(|o| o.id == disp).and_then(first_geo) {
            return Some(found);
        }
    }
    outcomes.iter().rev().find_map(first_geo)
}

/// Human-readable report block.
pub fn format_report(report: &Report) -> String {
    let mut s = String::new();
    let verdict = if report.passes() { "PASS" } else { "FAIL" };
    let _ = writeln!(s, "  report: {} [{verdict}]", report.title);
    for (name, m) in &report.metrics {
        let limit = match (m.limit, m.passes()) {
            (Some(l), Some(ok)) => format!(
                "  (limit {} {l}: {})",
                if m.lower_is_better { "<=" } else { ">=" },
                if ok { "ok" } else { "FAIL" }
            ),
            _ => String::new(),
        };
        let _ = writeln!(s, "    {name} = {} {}{limit}", fmt_num(m.value), m.unit);
    }
    for (sev, text) in &report.notes {
        let tag = match sev {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        let _ = writeln!(s, "    [{tag}] {text}");
    }
    s
}

fn fmt_num(v: f64) -> String {
    if v != 0.0 && (v.abs() >= 1e6 || v.abs() < 1e-3) {
        format!("{v:.4e}")
    } else {
        let t = format!("{v:.4}");
        t.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// One-line description of a data value.
pub fn describe(data: &Data) -> String {
    match data {
        Data::Geometry(g) => format!(
            "geometry: {} points, {} prims",
            g.points.len(),
            g.prims.len()
        ),
        Data::Report(r) => format!("report: {} ({} metrics)", r.title, r.metrics.len()),
        Data::File(p) => format!("file: {}", p.display()),
        Data::Json(v) => {
            let t = v.to_string();
            if t.len() > 100 {
                format!(
                    "json: {}...",
                    &t[..t.char_indices().nth(97).map_or(t.len(), |(i, _)| i)]
                )
            } else {
                format!("json: {t}")
            }
        }
    }
}

/// Text printed by `vernon-cli cook`.
pub fn format_outcomes(outcomes: &[NodeOutcome]) -> String {
    let mut s = String::new();
    for o in outcomes {
        match &o.result {
            Ok(outs) => {
                let _ = writeln!(s, "{} ({})", o.name, o.type_id);
                for d in outs {
                    match d {
                        Data::Report(r) => s.push_str(&format_report(r)),
                        other => {
                            let _ = writeln!(s, "  {}", describe(other));
                        }
                    }
                }
            }
            Err(e) => {
                let _ = writeln!(s, "{} ({}) FAILED: {e}", o.name, o.type_id);
            }
        }
    }
    let failed = outcomes.iter().filter(|o| o.result.is_err()).count();
    let reports: usize = outcomes.iter().map(|o| o.reports().count()).sum();
    let _ = writeln!(
        s,
        "\n{} nodes cooked, {failed} failed, {reports} reports",
        outcomes.len()
    );
    s
}

fn by_stage(reg: &Registry) -> Vec<(Stage, Vec<&NodeTypeInfo>)> {
    Stage::ALL
        .iter()
        .map(|s| (*s, reg.by_stage(*s)))
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

/// Plain-text catalogue for `vernon-cli list`.
pub fn list_text(reg: &Registry) -> String {
    let mut s = String::new();
    for (stage, infos) in by_stage(reg) {
        let _ = writeln!(s, "{} ({} nodes)", stage.label(), infos.len());
        for i in infos {
            let flag = if i.whitebox { "  [white-box]" } else { "" };
            let ext = i
                .external_tool
                .as_deref()
                .map(|t| format!("  [runs {t}]"))
                .unwrap_or_default();
            let _ = writeln!(s, "  {:<32} {}{flag}{ext}", i.id, i.label);
            for e in &i.emulates {
                let _ = writeln!(s, "      emulates: {e}");
            }
        }
    }
    let wb = reg.types().filter(|i| i.whitebox).count();
    let _ = writeln!(s, "\n{} node types, {wb} white-box", reg.len());
    s
}

/// Ports and parameters of one node type, for `vernon-cli info`.
pub fn format_info(i: &NodeTypeInfo) -> String {
    let mut s = String::new();
    let flag = if i.whitebox { " [white-box]" } else { "" };
    let _ = writeln!(s, "{} - {} ({}){flag}", i.id, i.label, i.stage.label());
    if !i.help.is_empty() {
        let _ = writeln!(s, "  {}", i.help);
    }
    for (dir, ports) in [("in ", &i.inputs), ("out", &i.outputs)] {
        for (n, p) in ports.iter().enumerate() {
            let opt = if p.required || dir == "out" {
                ""
            } else {
                " (optional)"
            };
            let _ = writeln!(s, "  {dir} {n}: {} {:?}{opt}", p.name, p.kind);
        }
    }
    for p in &i.params {
        let unit = p
            .unit
            .as_deref()
            .map(|u| format!(" {u}"))
            .unwrap_or_default();
        let default = serde_json::to_string(&p.default_value()).unwrap_or_default();
        let options = match &p.kind {
            vernon_graph::ParamKind::Enum { options, .. } => format!(" {options:?}"),
            _ => String::new(),
        };
        let _ = writeln!(s, "  param {} = {default}{unit}{options}", p.name);
    }
    s
}

/// Per-stage counts (stage label, implemented, white-box).
pub fn stage_counts(reg: &Registry) -> Vec<(Stage, usize, usize)> {
    by_stage(reg)
        .into_iter()
        .map(|(s, v)| {
            let wb = v.iter().filter(|i| i.whitebox).count();
            (s, v.len() - wb, wb)
        })
        .collect()
}

fn md_escape(t: &str) -> String {
    t.replace('|', "\\|").replace('\n', " ")
}

/// Markdown catalogue for `vernon-cli list --markdown` (docs/NODES.md).
pub fn list_markdown(reg: &Registry) -> String {
    let mut s = String::new();
    s.push_str("# Vernon node catalogue\n\n");
    s.push_str("Generated by `vernon-cli list --markdown`; do not edit by hand.\n\n");
    s.push_str(
        "*White-box* nodes have their final ports and parameters but a placeholder \
         algorithm; they cook and say so in a warning. *Emulates* names the public \
         tool behaviour a node re-implements from documentation (clean-room, see \
         `CLEAN_ROOM.md`); those names are documentation only and never appear in the UI.\n\n",
    );
    s.push_str("| Stage | Implemented | White-box | Total |\n|---|---:|---:|---:|\n");
    let (mut ti, mut tw) = (0, 0);
    for (stage, imp, wb) in stage_counts(reg) {
        let _ = writeln!(s, "| {} | {imp} | {wb} | {} |", stage.label(), imp + wb);
        ti += imp;
        tw += wb;
    }
    let _ = writeln!(s, "| **All** | **{ti}** | **{tw}** | **{}** |\n", ti + tw);
    for (stage, infos) in by_stage(reg) {
        let _ = writeln!(s, "## {}\n", stage.label());
        s.push_str(
            "| Id | Label | Status | External | Emulates | Help |\n|---|---|---|---|---|---|\n",
        );
        for i in infos {
            let status = if i.whitebox {
                "white-box"
            } else {
                "implemented"
            };
            let _ = writeln!(
                s,
                "| `{}` | {} | {status} | {} | {} | {} |",
                i.id,
                md_escape(&i.label),
                i.external_tool.as_deref().unwrap_or(""),
                md_escape(&i.emulates.join("; ")),
                md_escape(&i.help),
            );
        }
        s.push('\n');
    }
    s
}

/// Default scratch directory for cooking `vrn`.
pub fn default_work_root(vrn: &Path) -> PathBuf {
    let stem = vrn.file_stem().and_then(|s| s.to_str()).unwrap_or("graph");
    std::env::temp_dir().join("vernon-cli").join(stem)
}

/// Write a geometry `Data` to `path` in the format its extension names.
pub fn write_geometry(data: &Data, path: &Path) -> Result<()> {
    let Some(geo) = data.as_geometry() else {
        bail!("output is not geometry");
    };
    vernon_io::write(path, geo, &vernon_io::WriteOptions::default())
        .with_context(|| format!("writing {}", path.display()))
}

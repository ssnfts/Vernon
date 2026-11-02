//! Headless Vernon: the full node registry, `.vrn` loading and cooking, and
//! the text/Markdown node catalogue. `main.rs` is a thin argument parser
//! over these functions so the integration tests can call them directly.

pub mod agent_guide;
pub mod mcp;

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

/// Read and parse a `.vrn` file. Relative file parameters resolve against
/// the file's folder.
pub fn load_graph(path: &Path, reg: &Registry) -> Result<Graph> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut graph =
        parse_graph(&text, reg).with_context(|| format!("loading {}", path.display()))?;
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    graph.scene_dir = full.parent().map(Path::to_path_buf);
    Ok(graph)
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
    // One pass: every upstream node is worked out once, however many
    // nodes downstream of it are cooked.
    let results = graph.cook_many(reg, &ids, None, &mut |_| {});
    Ok(results
        .into_iter()
        .map(|(id, result)| {
            let node = &graph.nodes[&id];
            let type_id = node.type_id.clone();
            NodeOutcome {
                id,
                name: node.name.clone(),
                stage: reg.info(&type_id).map(|i| i.stage),
                type_id,
                result: result.map_err(|e| e.to_string()),
            }
        })
        .collect())
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
    vernon_core::units::fmt_value(v)
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

/// One failing check in a cooked report: a metric outside its limit, or
/// an error note.
#[derive(Debug, Clone, PartialEq)]
pub struct GateFailure {
    pub node: String,
    pub report: String,
    /// Metric name, `None` for an error note.
    pub metric: Option<String>,
    pub value: f64,
    pub unit: String,
    pub limit: f64,
    pub lower_is_better: bool,
    /// Distance to the limit, negative when failing (`limit - value` when
    /// lower is better, else `value - limit`).
    pub margin: f64,
    /// The note text for an error note.
    pub text: String,
    /// The reason the scene expects this failure (`expect_fail` on the
    /// node), if it does.
    pub expected: Option<String>,
}

/// Every failing gate in `outcomes`. A metric listed in its node's
/// `expect_fail` comes back with `expected` set.
pub fn gate_failures(outcomes: &[NodeOutcome], graph: &Graph) -> Vec<GateFailure> {
    let mut out = Vec::new();
    for o in outcomes {
        let expect = graph.nodes.get(&o.id).map(|n| &n.expect_fail);
        for r in o.reports() {
            for (name, m) in &r.metrics {
                let (Some(limit), Some(false)) = (m.limit, m.passes()) else { continue };
                out.push(GateFailure {
                    node: o.name.clone(),
                    report: r.title.clone(),
                    metric: Some(name.clone()),
                    value: m.value,
                    unit: m.unit.clone(),
                    limit,
                    lower_is_better: m.lower_is_better,
                    margin: vernon_graph::gate::margin(m).unwrap_or(f64::NAN),
                    text: String::new(),
                    expected: expect.and_then(|e| e.get(name)).cloned(),
                });
            }
            for (sev, text) in &r.notes {
                if *sev == Severity::Error {
                    out.push(GateFailure {
                        node: o.name.clone(),
                        report: r.title.clone(),
                        metric: None,
                        value: f64::NAN,
                        unit: String::new(),
                        limit: f64::NAN,
                        lower_is_better: true,
                        margin: f64::NAN,
                        text: text.clone(),
                        expected: None,
                    });
                }
            }
        }
    }
    out
}

/// Gate summary printed after the cook: each failing metric with value,
/// limit and margin, expected failures marked, and a closing count line.
pub fn format_gates(gates: &[GateFailure]) -> String {
    let mut s = String::new();
    let open = gates.iter().filter(|g| g.expected.is_none()).count();
    let _ = writeln!(s, "\ngates: {open} failing, {} expected", gates.len() - open);
    for g in gates {
        let tag = if g.expected.is_some() { "expected" } else { "FAIL" };
        match &g.metric {
            Some(m) => {
                let op = if g.lower_is_better { "<=" } else { ">=" };
                let _ = write!(
                    s,
                    "  [{tag}] {} / {m} = {} {} (limit {op} {}, margin {} {})",
                    g.node,
                    fmt_num(g.value),
                    g.unit,
                    fmt_num(g.limit),
                    fmt_num(g.margin),
                    g.unit
                );
            }
            None => {
                let _ = write!(s, "  [{tag}] {} / {}: error: {}", g.node, g.report, g.text);
            }
        }
        match &g.expected {
            Some(why) => {
                let _ = writeln!(s, ": {why}");
            }
            None => s.push('\n'),
        }
    }
    s
}

fn by_stage(reg: &Registry) -> Vec<(Stage, Vec<&NodeTypeInfo>)> {
    Stage::ALL
        .iter()
        .map(|s| (*s, reg.by_stage(*s)))
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

/// Plain-text catalogue for `vernon-cli list`. The `emulates` lines name the
/// public tool behaviours a node re-implements; they are documentation, so
/// they are printed only with `docs` (`--docs`).
pub fn list_text(reg: &Registry, docs: bool) -> String {
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
            if docs {
                for e in &i.emulates {
                    let _ = writeln!(s, "      emulates: {e}");
                }
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

/// Default scratch directory for cooking `vrn`: `<temp>/vernon-cli/<stem>-<hash>`,
/// where the hash of the file's full path keeps two files with the same
/// name in different folders apart.
pub fn default_work_root(vrn: &Path) -> PathBuf {
    let stem = vrn.file_stem().and_then(|s| s.to_str()).unwrap_or("graph");
    let full = std::fs::canonicalize(vrn)
        .or_else(|_| std::path::absolute(vrn))
        .unwrap_or_else(|_| vrn.to_path_buf());
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in full.to_string_lossy().bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    std::env::temp_dir().join("vernon-cli").join(format!("{stem}-{:08x}", h as u32))
}

/// Write a geometry `Data` to `path` in the format its extension names.
pub fn write_geometry(data: &Data, path: &Path) -> Result<()> {
    let Some(geo) = data.as_geometry() else {
        bail!("output is not geometry");
    };
    vernon_io::write(path, geo, &vernon_io::WriteOptions::default())
        .with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod work_root_tests {
    use super::*;

    #[test]
    fn same_stem_in_other_folders_gets_its_own_work_root() {
        let a = default_work_root(Path::new("one/scene.vrn"));
        let b = default_work_root(Path::new("two/scene.vrn"));
        assert_ne!(a, b);
        assert_eq!(a, default_work_root(Path::new("one/scene.vrn")));
        assert!(a.file_name().unwrap().to_str().unwrap().starts_with("scene-"));
    }
}

#[cfg(test)]
mod pass_tests {
    use super::*;

    #[test]
    fn cooking_every_node_works_each_one_out_once() {
        // A ladder of merges, both inputs fed by the previous rung: the
        // number of source paths doubles per rung.
        let reg = registry();
        let mut g = Graph::new();
        let mut prev = g.add_node(&reg, "geo.box").unwrap();
        for _ in 0..12 {
            let m = g.add_node(&reg, "geo.merge").unwrap();
            g.connect(prev, 0, m, 0).unwrap();
            g.connect(prev, 0, m, 1).unwrap();
            prev = m;
        }
        let outcomes = cook_graph(&mut g, &reg, None).unwrap();
        assert_eq!(outcomes.len(), 13);
        assert!(outcomes.iter().all(|o| o.result.is_ok()), "{}", format_outcomes(&outcomes));
        assert_eq!(g.last_visits(), 13);
        cook_graph(&mut g, &reg, None).unwrap();
        assert_eq!(g.last_visits(), 13, "cached cooks still visit each node once");
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    use vernon_core::Metric;

    #[test]
    fn failing_limits_are_listed_with_margins_and_expected_ones_marked() {
        let graph = Graph::from_json(
            r#"{ "nodes": { "1": { "type_id": "x", "name": "b",
                 "expect_fail": { "wheels": "a panel has no wheels" } } } }"#,
        )
        .unwrap();
        let mut r = Report::new("prep");
        r.metric("wheels", Metric::new(0.0, "count").with_limit(4.0, false));
        r.metric("area", Metric::new(2.5, "m2").with_limit(2.0, true));
        r.metric("fine", Metric::new(1.0, "m2").with_limit(2.0, true));
        r.metric("free", Metric::new(9.0, "m2"));
        r.note(Severity::Error, "broken");
        let o = |id: NodeId, name: &str| NodeOutcome {
            id,
            name: name.into(),
            type_id: "x".into(),
            stage: None,
            result: Ok(vec![Data::report(r.clone())]),
        };
        let gates = gate_failures(&[o(1, "b"), o(2, "c")], &graph);
        assert_eq!(gates.len(), 6, "{gates:#?}");
        assert_eq!(gates[0].expected.as_deref(), Some("a panel has no wheels"));
        assert_eq!(gates[0].margin, -4.0);
        assert_eq!(gates[1].margin, -0.5);
        assert!(gates[3].expected.is_none(), "expect_fail is per node");
        let text = format_gates(&gates);
        assert!(text.contains("gates: 5 failing, 1 expected"), "{text}");
        assert!(
            text.contains("[expected] b / wheels = 0 count (limit >= 4, margin -4 count): a panel has no wheels"),
            "{text}"
        );
        assert!(text.contains("[FAIL] c / area = 2.5 m² (limit <= 2, margin -0.5 m²)"), "{text}");
        assert!(text.contains("[FAIL] c / prep: error: broken"), "{text}");
    }
}

#[cfg(test)]
mod format_tests {
    use super::*;
    use vernon_core::Metric;

    #[test]
    fn units_are_canonical_and_negative_zero_prints_as_zero() {
        let mut r = Report::new("t");
        r.metric("fail_area_pct", Metric::new(-0.0, "%"));
        r.metric("surface_temp", Metric::new(-0.0, "deg C"));
        r.metric("area", Metric::new(3.0, "mm2"));
        r.metric("flag", Metric::new(1.0, "flag"));
        let text = format_report(&r);
        assert!(text.contains("fail_area_pct = 0 %"), "{text}");
        assert!(text.contains("surface_temp = 0 °C"), "{text}");
        assert!(text.contains("area = 3 mm²"), "{text}");
        assert!(!text.contains("-0"), "{text}");
        assert!(text.contains("flag = 1 \n"), "{text:?}");
    }
}

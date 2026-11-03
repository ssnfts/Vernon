//! `docs/AGENT_GUIDE.md`: the short guide an AI agent reads before it builds
//! graphs. Generated from the registry so it cannot go stale: stage counts
//! come from the registry, and every recipe is a list of tool calls that the
//! tests replay against the real node types.

use std::fmt::Write as _;

use anyhow::{bail, Result};
use serde_json::{json, Value};
use vernon_graph::{Registry, Stage};

/// One step of a recipe.
pub enum Step {
    /// `add name = type {params}`
    Add(&'static str, &'static str, &'static str),
    /// `wire from[.port] -> to[.port]`
    Wire(&'static str, &'static str),
}

pub struct Recipe {
    pub title: &'static str,
    pub steps: &'static [Step],
    /// What to cook and read afterwards.
    pub finish: &'static str,
}

use Step::{Add, Wire};

pub const RECIPES: &[Recipe] = &[
    Recipe {
        title: "Scan, align, deviation",
        steps: &[
            Add("ref", "geo.car_panel", ""),
            Add("scan", "geo.noise", r#"{"sigma":0.2}"#),
            Wire("ref", "scan"),
            Add("pre", "clean.auto_prealign", ""),
            Wire("scan", "pre.moving"),
            Wire("ref", "pre.reference"),
            Add("icp", "clean.icp_align", ""),
            Wire("pre.geometry", "icp.moving"),
            Wire("ref", "icp.reference"),
            Add("dev", "clean.mesh_distance", ""),
            Wire("icp.geometry", "dev.compared"),
            Wire("ref", "dev.reference"),
            Add("stats", "clean.deviation_stats", r#"{"tolerance":0.5}"#),
            Wire("dev.geometry", "stats.geometry"),
            Add("gate", "clean.tolerance_gate", r#"{"tolerance":0.5}"#),
            Wire("stats.stats", "gate.stats"),
        ],
        finish: "cook gate; list_gates. With a real scan, replace ref/scan by capture.import_scan (set path, unit).",
    },
    Recipe {
        title: "Split for the print bed",
        steps: &[
            Add("body", "geo.car_panel", r#"{"thickness":4}"#),
            Add("split", "print.smart_partition", r#"{"printer":"bed_350"}"#),
            Wire("body", "split.mesh"),
            Add("fit", "print.build_fit", r#"{"printer":"bed_350"}"#),
            Wire("split.parts", "fit.parts"),
        ],
        finish: "Split needs a closed solid (thickness above 0). cook fit; read_report fit. Parts that do not fit the bed fail the gate: split more or change printer.",
    },
    Recipe {
        title: "CFD case setup (files only)",
        steps: &[
            Add("body", "geo.car_panel", ""),
            Add("prep", "cfd.vehicle_prep", ""),
            Wire("body", "prep.body"),
            Add("tunnel", "cfd.tunnel_domain", ""),
            Wire("prep.reference", "tunnel.reference"),
            Add("mesh", "cfd.mesh_setup", ""),
            Wire("tunnel.domain", "mesh.domain"),
            Wire("prep.vehicle", "mesh.vehicle"),
            Add("case", "cfd.case_setup", r#"{"u_ms":30}"#),
            Wire("tunnel.domain", "case.domain"),
            Wire("prep.reference", "case.reference"),
        ],
        finish: "cook case. Nodes only write input files; meshing or solving needs run=true and an installed program, which you must not enable unless asked.",
    },
    Recipe {
        title: "Hot-day heat check",
        steps: &[
            Add("body", "geo.car_panel", ""),
            Add("hour", "solar.design_hour", ""),
            Add("bal", "solar.surface_balance", ""),
            Wire("body", "bal.mesh"),
            Wire("hour.weather", "bal.weather"),
            Add("hdt", "solar.hdt_check", r#"{"material":"ASA"}"#),
            Wire("bal.mesh", "hdt.mesh"),
        ],
        finish: "cook hdt; read_report hdt. A negative worst margin means the material softens.",
    },
    Recipe {
        title: "Clean a mesh",
        steps: &[
            Add("body", "geo.car_panel", ""),
            Add("fill", "clean.fill_boundary_holes", ""),
            Wire("body", "fill.mesh"),
            Add("smooth", "clean.smooth_mesh", r#"{"iterations":10}"#),
            Wire("fill.geometry", "smooth.mesh"),
            Add("dec", "clean.decimate", r#"{"ratio":0.5}"#),
            Wire("smooth.geometry", "dec.mesh"),
        ],
        finish: "measure_geometry dec: check watertight and that size_mm did not change.",
    },
    Recipe {
        title: "Flatten a panel",
        steps: &[
            Add("body", "geo.car_panel", ""),
            Add("cut", "flatten.seam_cut", ""),
            Wire("body", "cut.mesh"),
            Add("flat", "flatten.conformal_map", r#"{"max_strain_pct":5}"#),
            Wire("cut.mesh", "flat.mesh"),
            Add("strain", "flatten.strain_report", ""),
            Wire("flat.mesh", "strain.mesh"),
            Wire("flat.pattern", "strain.pattern"),
        ],
        finish: "cook strain; read_report strain. Strain above the limit means add seams.",
    },
];

/// The tool calls a recipe makes, in order.
pub fn recipe_calls(r: &Recipe) -> Result<Vec<(&'static str, Value)>> {
    let mut calls = vec![];
    for s in r.steps {
        match s {
            Add(name, ty, params) => {
                let mut v = json!({"type_id": ty, "name": name});
                if !params.is_empty() {
                    v["params"] = serde_json::from_str(params)?;
                }
                calls.push(("add_node", v));
            }
            Wire(from, to) => {
                let (f, fo) = split_port(from);
                let (t, ti) = split_port(to);
                let mut v = json!({"from": f, "to": t});
                if let Some(p) = fo {
                    v["from_output"] = json!(p);
                }
                if let Some(p) = ti {
                    v["to_input"] = json!(p);
                }
                calls.push(("connect", v));
            }
        }
    }
    Ok(calls)
}

fn split_port(s: &str) -> (&str, Option<&str>) {
    match s.split_once('.') {
        Some((n, p)) => (n, Some(p)),
        None => (s, None),
    }
}

fn blurb(s: Stage) -> (&'static str, &'static str) {
    match s {
        Stage::Geometry => ("geo", "primitives, transform, merge, measure, file read/write"),
        Stage::Capture => ("capture", "import scans, scale, merge, photogrammetry, scan quality"),
        Stage::Clean => ("clean", "denoise, align (ICP), deviation, holes, remesh, tolerance gates"),
        Stage::Surface => ("surface", "mesh to quad/subd/NURBS surface, deviation, STEP export"),
        Stage::Print => ("print", "split, joints, build-volume fit, slicing, G-code"),
        Stage::Cfd => ("cfd", "vehicle prep, virtual tunnel, solver case, convergence, coefficients"),
        Stage::Fea => ("fea", "materials, loads, solid mesh, structural solve, safety factor"),
        Stage::Solar => ("solar", "sun, weather, surface temperature, heat-deflection check"),
        Stage::Flatten => ("flatten", "seams, flattening, strain"),
        Stage::Performance => ("perf", "vehicle, lap simulation, validation gates"),
    }
}

/// Names the guide relies on; each must be a node type (checked in `generate`).
const GATE_NODES: &[&str] = &["clean.tolerance_gate", "print.build_fit", "print.heat_margin", "solar.hdt_check", "perf.validation_gate", "perf.gate_summary"];

/// The Markdown guide for `reg`. Fails when a recipe or named node no longer
/// matches the registry, so a stale recipe cannot be published.
pub fn generate(reg: &Registry) -> Result<String> {
    for id in GATE_NODES {
        if reg.info(id).is_none() {
            bail!("agent guide names node '{id}' which is not registered");
        }
    }
    for r in RECIPES {
        for s in r.steps {
            if let Add(_, ty, params) = s {
                let Some(info) = reg.info(ty) else { bail!("recipe '{}': unknown node type '{ty}'", r.title) };
                if !params.is_empty() {
                    let v: Value = serde_json::from_str(params)?;
                    for k in v.as_object().into_iter().flat_map(|o| o.keys()) {
                        if !info.params.iter().any(|p| &p.name == k) {
                            bail!("recipe '{}': '{ty}' has no parameter '{k}'", r.title);
                        }
                    }
                }
            }
        }
    }
    let total = reg.len();
    let placeholders = reg.types().filter(|i| i.whitebox).count();
    let mut s = String::new();
    let _ = writeln!(s, "# Vernon agent guide\n");
    let _ = writeln!(s, "Generated by `vernon-cli agent-guide`; do not edit by hand.\n");
    let _ = writeln!(
        s,
        "Vernon is a node-graph 3D app for a car body-kit pipeline: scan, clean, surface, split/print, CFD, FEA, solar, performance. You build a graph with tools, cook it, and read the reports. {total} node types ({placeholders} placeholders that say so in a warning).\n"
    );
    let _ = writeln!(s, "## Working loop\n");
    let _ = writeln!(s, "1. `list_nodes(stage)` then `describe_node(type_id)` for ports, parameter ranges, units and options.");
    let _ = writeln!(s, "2. `add_node`, `set_param`, `connect` (by name; ports by name or index). Errors tell you the valid names and ranges.");
    let _ = writeln!(s, "3. `cook(node)`, then `read_report`, `get_metric`, `list_gates`, `measure_geometry`.");
    let _ = writeln!(s, "4. Wrap a whole task in `begin_group` / `end_group` so one `undo` reverts it. `save_graph` / `load_graph` use relative `.vrn` paths in the work folder only.\n");
    let _ = writeln!(s, "## Conventions\n");
    let _ = writeln!(s, "- Lengths are millimetres, Z is up, physics in SI with the unit shown in each parameter.");
    let _ = writeln!(s, "- Geometry attributes: point `N`, `Cd`, `uv`, `dist`; face `N`, `part`, `temp_c`, `Cp`.");
    let _ = writeln!(s, "- Wire kinds: Geometry, Report, File, Json. A Report output carries metrics for the next stage.");
    let _ = writeln!(s, "- File parameters must be relative paths without `..`.");
    let _ = writeln!(s, "- Nodes that drive an external program (solver, slicer) only write input files unless their `run` switch is on. Leave it off unless the user asked; it resets after each cook.\n");
    let _ = writeln!(s, "## Stages\n");
    let _ = writeln!(s, "| Prefix | Nodes | Use |\n|---|---:|---|");
    for st in Stage::ALL {
        let (prefix, what) = blurb(st);
        let _ = writeln!(s, "| `{prefix}.` | {} | {what} |", reg.by_stage(st).len());
    }
    let _ = writeln!(s, "\n## Gates\n");
    let _ = writeln!(s, "A metric with a limit is a gate: it passes when the value is on the right side of the limit (`lower_is_better` says which). A failing gate or an error note fails the report and the stages after it should not be trusted. A failure the scene lists under a node's `expect_fail` is reported as expected. `cook` returns `failed_gates` with value, limit and margin; fix the cause (parameters, upstream geometry), re-cook, do not edit limits to make a gate pass. Gate nodes: {}.\n", GATE_NODES.iter().map(|g| format!("`{g}`")).collect::<Vec<_>>().join(", "));
    let _ = writeln!(s, "## Recipes\n");
    let _ = writeln!(s, "`add name = type {{params}}`; `wire out[.port] -> in[.port]` is `connect`.\n");
    for (i, r) in RECIPES.iter().enumerate() {
        let _ = writeln!(s, "### {}. {}\n", i + 1, r.title);
        s.push_str("```\n");
        for st in r.steps {
            match st {
                Add(n, t, "") => {
                    let _ = writeln!(s, "add {n} = {t}");
                }
                Add(n, t, p) => {
                    let _ = writeln!(s, "add {n} = {t} {p}");
                }
                Wire(a, b) => {
                    let _ = writeln!(s, "wire {a} -> {b}");
                }
            }
        }
        s.push_str("```\n");
        let _ = writeln!(s, "{}\n", r.finish);
    }
    Ok(s)
}

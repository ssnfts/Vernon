//! `geo.ai_partcrafter` and `geo.ai_select_part`: PartCrafter image-to-parts.
//!
//! PartCrafter returns several separate meshes in its own normalised space. Vernon
//! rescales them uniformly to a chosen longest dimension, names them `part_NN`,
//! and keeps the inputs, settings, seed, model revision and every artifact in the
//! node's work folder. The result is *not* treated as scaled to a real object,
//! watertight, or simulation-ready; the report measures openness per part.

use std::path::Path;
use std::time::Duration;

use glam::DVec3;
use serde_json::{json, Value};
use vernon_core::report::{Metric, Severity};
use vernon_core::{AttribArray, Geometry, Prim, Report};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::bridge::{self, check_resources, with_margin, BridgeError, Install};
use crate::{fnv1a, prep};

pub const PARTS_LABEL: &str = "AI-generated geometry — arbitrary scale, not verified watertight or simulation-ready";
pub const DRAFT_PEAK_VRAM_GB: f64 = 4.31;
pub const STANDARD_PEAK_VRAM_GB: f64 = 4.31;
pub const PEAK_RSS_GB: f64 = 3.5;

pub fn register(reg: &mut Registry) {
    reg.add(PartCrafterNode);
    reg.add(SelectPartNode);
}

/// One part loaded from the bridge output.
#[derive(Debug, Clone)]
pub struct LoadedPart {
    pub index: usize,
    pub name: String,
    pub geometry: Geometry,
}

/// Merge parts into one geometry scaled so the longest side of the combined box is
/// `longest_mm`, centred on x/y with its lowest point at z = 0. Prim attributes `part`
/// (Int) and `part_name` (Str) and prim groups `part_NN` identify the parts.
pub fn assemble(parts: &[LoadedPart], longest_mm: f64) -> Result<Geometry, String> {
    if parts.is_empty() {
        return Err("no parts to assemble".into());
    }
    let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
    for p in parts {
        for q in &p.geometry.points {
            lo = lo.min(*q);
            hi = hi.max(*q);
        }
    }
    let ext = hi - lo;
    let longest = ext.max_element();
    if !longest.is_finite() || longest <= 0.0 {
        return Err("generated parts have zero size".into());
    }
    let s = longest_mm / longest;
    let shift = DVec3::new((lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5, lo.z);
    let mut g = Geometry::new();
    let (mut part_id, mut part_name) = (Vec::new(), Vec::new());
    for p in parts {
        let base = g.points.len() as u32;
        for q in &p.geometry.points {
            g.add_point((*q - shift) * s);
        }
        let first = g.prims.len() as u32;
        for pr in &p.geometry.prims {
            if !pr.closed {
                continue;
            }
            g.add_prim(Prim::polygon(pr.vertices.iter().map(|v| v + base).collect()));
            part_id.push(p.index as i64);
            part_name.push(p.name.clone());
        }
        let last = g.prims.len() as u32;
        g.prim_groups.insert(p.name.clone(), (first..last).collect());
    }
    g.prim_attribs.set("part", AttribArray::Int(part_id));
    g.prim_attribs.set("part_name", AttribArray::Str(part_name));
    Ok(g)
}

/// Keep only the primitives whose `part` equals `index`; points are compacted.
pub fn extract_part(geo: &Geometry, index: i64) -> Result<(Geometry, String), String> {
    let ids = geo.prim_attribs.ints("part").map_err(|_| "the geometry has no 'part' attribute; connect AI PartCrafter output".to_string())?;
    let keep: Vec<usize> = (0..geo.prims.len()).filter(|&i| ids.get(i) == Some(&index)).collect();
    if keep.is_empty() {
        let mut have: Vec<i64> = ids.to_vec();
        have.sort_unstable();
        have.dedup();
        return Err(format!("no part {index}; available parts: {have:?}"));
    }
    let mut remap = std::collections::HashMap::new();
    let mut out = Geometry::new();
    let mut src_pts = Vec::new();
    for &pi in &keep {
        let verts: Vec<u32> = geo.prims[pi]
            .vertices
            .iter()
            .map(|&v| {
                *remap.entry(v).or_insert_with(|| {
                    src_pts.push(v as usize);
                    out.add_point(geo.points[v as usize])
                })
            })
            .collect();
        out.add_prim(Prim { vertices: verts, closed: geo.prims[pi].closed });
    }
    out.prim_attribs = geo.prim_attribs.gather_all(&keep);
    out.point_attribs = geo.point_attribs.gather_all(&src_pts);
    let name = geo.prim_attribs.strs("part_name").ok().and_then(|v| v.get(keep[0]).cloned()).unwrap_or_else(|| format!("part_{index:02}"));
    out.prim_groups.insert(name.clone(), (0..out.prims.len() as u32).collect());
    Ok((out, name))
}

pub struct PartCrafterNode;

impl NodeOp for PartCrafterNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.ai_partcrafter", "AI Parts from Image", Stage::Geometry)
            .output(PortSpec::new("parts", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("manifest", DataKind::Json))
            .param(ParamSpec::path("ai_root", "AI-3D folder", &[], false).help("Folder holding the PartCrafter install. Blank uses the VERNON_AI3D_ROOT environment variable."))
            .param(ParamSpec::string("wsl_distro", "WSL distribution", "AI-3D").help("Linux environment that holds the model's Python. Blank runs Python directly (Linux hosts)."))
            .param(ParamSpec::path("image", "Image", &["png", "jpg", "jpeg", "webp"], false).help("A pre-cropped photo or render of one object or one scene on a plain background."))
            .param(ParamSpec::choice("input_mode", "Input type", "object", &["object", "scene"]).help("Object: one object split into parts. Scene: several objects in a scene."))
            .param(ParamSpec::int("parts", "Number of parts", 3, 1, 16).help("How many separate parts the model generates; empty parts are skipped with a warning."))
            .param(ParamSpec::int("seed", "Seed", 0, 0, 2_147_483_647).help("Same image, settings and seed give the same parts."))
            .param(ParamSpec::choice("quality", "Quality", "draft", &["draft", "standard"]).help("Draft: 10 steps, coarse. Standard: the steps and tokens below."))
            .param(ParamSpec::int("steps", "Steps (standard)", 50, 10, 100).help("Denoising steps used at standard quality."))
            .param(ParamSpec::int("tokens", "Tokens (standard)", 0, 0, 4096).help("0 = model default (1024 object, 2048 scene)."))
            .param(ParamSpec::bool("offload", "CPU offload", false).help("Trades speed for lower GPU memory."))
            .param(ParamSpec::float("longest_mm", "Longest dimension", 1000.0, 1.0, 10000.0).unit("mm").help("The model output has no real scale; it is scaled uniformly so the whole assembly's longest side is this long."))
            .param(ParamSpec::int("timeout_s", "Time limit", 3600, 60, 14400).unit("s").help("The model process is stopped after this long."))
            .param(ParamSpec::bool("run", "Run model", false).action().help("Starts the model. Without it the node loads an earlier result for identical inputs, if one exists."))
            .help("Generates separate mesh parts from one image with PartCrafter. Parts are named part_NN, selectable with Select AI Part, and saved with their inputs, seed and model revision. Output has arbitrary scale and is not checked for printing or simulation.")
            .emulates("image to structured multi-part 3D mesh generation")
            .spec("docs/specs/ai-models.md")
            .implemented()
            .external("PartCrafter")
    }

    fn cook(&self, ctx: &CookContext, _ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let image = p.str("image")?.trim().to_string();
        let mut rep = Report::new("AI parts from image");
        rep.note(Severity::Warning, PARTS_LABEL);
        if image.is_empty() {
            rep.note(Severity::Info, "choose an image to generate parts from");
            return Ok(empty(rep, json!({"status": "no_image", "label": PARTS_LABEL})));
        }
        let img_path = Path::new(&image);
        let bytes = std::fs::read(img_path).map_err(|e| ctx.fail(format!("cannot read image {image}: {e}")))?;
        let (mode, quality) = (p.str("input_mode")?.to_string(), p.str("quality")?.to_string());
        let (parts_n, seed, steps, tokens) = (p.int("parts")?, p.int("seed")?, p.int("steps")?, p.int("tokens")?);
        let offload = p.bool("offload")?;
        let key = {
            let mut b = bytes.clone();
            b.extend_from_slice(format!("{mode}|{quality}|{parts_n}|{seed}|{steps}|{tokens}|{offload}").as_bytes());
            fnv1a(&b)
        };
        let dir = ctx.work_dir.join(format!("partcrafter_{key:016x}"));
        let result_path = dir.join("result.json");
        if !result_path.is_file() {
            if !p.bool("run")? {
                rep.note(Severity::Info, "not run: switch on 'Run model' to generate parts");
                return Ok(empty(rep, json!({"status": "not_run", "label": PARTS_LABEL, "artifact_dir": dir.to_string_lossy()})));
            }
            run_model(ctx, p, &dir, img_path, &bytes, &mode, &quality)?;
        }
        let res: Value = serde_json::from_slice(&std::fs::read(&result_path)?).map_err(|e| ctx.fail(format!("unreadable result.json: {e}")))?;
        let mut loaded = Vec::new();
        for part in res.get("parts").and_then(Value::as_array).cloned().unwrap_or_default() {
            let file = part.get("file").and_then(Value::as_str).unwrap_or("");
            let name = part.get("name").and_then(Value::as_str).unwrap_or("part").to_string();
            let index = part.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let geometry = vernon_io::read(&dir.join(file), &vernon_io::ReadOptions::default()).map_err(|e| ctx.fail(format!("cannot import {file}: {e}")))?;
            loaded.push(LoadedPart { index, name, geometry });
        }
        let assembled = assemble(&loaded, p.float("longest_mm")?).map_err(|e| ctx.fail(e))?;
        rep.metric("ai_part_count", Metric::new(loaded.len() as f64, "count"));
        let mut part_rows = Vec::new();
        for lp in &loaded {
            let tris: Vec<[DVec3; 3]> = lp.geometry.triangles().iter().map(|(_, t)| [lp.geometry.points[t[0] as usize], lp.geometry.points[t[1] as usize], lp.geometry.points[t[2] as usize]]).collect();
            let (open, nm, comps) = prep::topology(&tris);
            rep.metric(format!("ai_{}_open_edges", lp.name), Metric::new(open as f64, "count"));
            if open > 0 {
                rep.note(Severity::Warning, format!("{} has {open} open edge(s): not watertight, repair before printing or simulation", lp.name));
            }
            part_rows.push(json!({"index": lp.index, "name": lp.name, "triangles": tris.len(), "open_edges": open, "non_manifold_edges": nm, "components": comps}));
        }
        if let Some(sk) = res.get("skipped_empty_parts").and_then(Value::as_array) {
            if !sk.is_empty() {
                rep.note(Severity::Warning, format!("the model returned empty meshes for part(s) {sk:?}; they were skipped"));
            }
        }
        rep.note(Severity::Info, "output scale is arbitrary (rescaled to the chosen longest dimension); orientation is the model's own. Do not treat it as simulation-ready: run the AI Aero Preview checks first");
        for k in ["peak_vram_GB", "seconds_generate", "seconds_load"] {
            if let Some(v) = res.get(k).and_then(Value::as_f64) {
                rep.metric(format!("ai_{}", k.to_lowercase()), Metric::new(v, if k.starts_with("seconds") { "s" } else { "GB" }));
            }
        }
        let manifest = json!({
            "status": "complete", "label": PARTS_LABEL, "artifact_dir": dir.to_string_lossy(), "model": res.get("model"), "weights": res.get("weights"),
            "mode": mode, "quality": quality, "seed": seed, "parts_requested": parts_n, "steps": res.get("steps"), "tokens": res.get("tokens"), "offload": offload,
            "image": {"path": image, "sha256": res.get("image_sha256")}, "longest_mm": p.float("longest_mm")?, "parts": part_rows,
            "peak_vram_GB": res.get("peak_vram_GB"), "seconds_generate": res.get("seconds_generate"), "seconds_load": res.get("seconds_load"),
        });
        let _ = std::fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap_or_default());
        Ok(vec![Data::geometry(assembled), Data::report(rep), Data::json(manifest)])
    }
}

fn empty(rep: Report, manifest: Value) -> Vec<Data> {
    vec![Data::geometry(Geometry::new()), Data::report(rep), Data::json(manifest)]
}

fn run_model(ctx: &CookContext, p: &Params, dir: &Path, img: &Path, bytes: &[u8], mode: &str, quality: &str) -> CookResult<()> {
    let install = Install::resolve(p.str("ai_root")?, p.str("wsl_distro")?).map_err(|e| ctx.fail(e.to_string()))?;
    install.require_model_dir("PartCrafter").map_err(|e| ctx.fail(e.to_string()))?;
    let have = bridge::query_resources(&install).map_err(|e| ctx.fail(e.to_string()))?;
    let vram = if quality == "draft" { DRAFT_PEAK_VRAM_GB } else { STANDARD_PEAK_VRAM_GB };
    if !p.bool("offload")? {
        check_resources(with_margin(vram, PEAK_RSS_GB), have, "PartCrafter").map_err(|e| ctx.fail(e.to_string()))?;
    }
    std::fs::create_dir_all(dir)?;
    let ext = img.extension().and_then(|e| e.to_str()).unwrap_or("png");
    let kept = dir.join(format!("input.{ext}"));
    std::fs::write(&kept, bytes)?;
    let request = json!({
        "ai_root": install.root.to_string_lossy(), "image": kept.to_string_lossy(), "out_dir": dir.to_string_lossy(),
        "mode": mode, "parts": p.int("parts")?, "seed": p.int("seed")?, "steps": p.int("steps")?, "tokens": p.int("tokens")?,
        "quality": quality, "offload": p.bool("offload")?,
    });
    let timeout = Duration::from_secs(p.int("timeout_s")?.max(60) as u64);
    bridge::run_job(&install, "PartCrafter", "partcrafter_bridge.py", bridge::PARTCRAFTER_BRIDGE, &request, dir, timeout).map_err(|e| match e {
        BridgeError::Cancelled => CookError::Other(vernon_graph::CANCELLED.into()),
        other => ctx.fail(other.to_string()),
    })?;
    Ok(())
}

pub struct SelectPartNode;

impl NodeOp for SelectPartNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.ai_select_part", "Select AI Part", Stage::Geometry)
            .input(PortSpec::new("parts", DataKind::Geometry))
            .output(PortSpec::new("part", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::int("part", "Part number", 0, 0, 15).help("Index of the part (part_00 is 0)."))
            .help("Picks one named part from an AI Parts from Image result so it can go on to cleaning, an aero preview or printing.")
            .emulates("selecting one object from a multi-part mesh")
            .spec("docs/specs/ai-models.md")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(ins, 0)?;
        let (g, name) = extract_part(&geo, p.int("part")?).map_err(|e| ctx.fail(e))?;
        let mut rep = Report::new("AI part selection");
        rep.note(Severity::Info, format!("selected {name}: {} triangles", g.prims.len()));
        rep.note(Severity::Warning, PARTS_LABEL);
        Ok(vec![Data::geometry(g), Data::report(rep)])
    }
}

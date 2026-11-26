//! `geo.ai_sketch_from_words`: a local image model draws one concept picture from a text prompt.
//!
//! The model (FLUX.2-klein-4B, Apache-2.0) runs as a separate Python process
//! (`scripts/ai/sketch_bridge.py`) on the GPU. The picture is a concept sketch, never a
//! design or a measurement, and it is labelled so everywhere. The work folder is named after
//! a hash of every input, so identical inputs reload an earlier picture without starting
//! the model.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use vernon_core::report::{Metric, Severity};
use vernon_core::Report;
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::bridge::{self, check_resources, with_margin, BridgeError, Install};
use crate::fnv1a;

pub const SKETCH_BRIDGE: &str = include_str!("../../../scripts/ai/sketch_bridge.py");
/// First line of the report.
pub const SKETCH_LABEL: &str = "AI picture: a concept sketch, not a design or a measurement";
/// Peak GPU memory and RAM of the real run, see docs/specs/ai-local/sketch-from-words.md.
pub const PEAK_VRAM_GB: f64 = 7.8;
pub const PEAK_RSS_GB: f64 = 16.7;
pub const DEFAULT_SIZE: i64 = 768;
pub const DEFAULT_STEPS: i64 = 4;

pub fn register(reg: &mut Registry) {
    reg.add(SketchFromWordsNode);
}

pub struct SketchFromWordsNode;

/// Folder name key for one set of inputs.
pub fn work_key(prompt: &str, width: i64, height: i64, steps: i64, seed: i64) -> u64 {
    fnv1a(format!("flux2-klein-4b|{width}|{height}|{steps}|{seed}|{prompt}").as_bytes())
}

/// Width and height must be multiples of 16 between 256 and 1024.
pub fn check_size(width: i64, height: i64) -> Result<(), String> {
    for (name, v) in [("width", width), ("height", height)] {
        if !(256..=1024).contains(&v) || v % 16 != 0 {
            return Err(format!("bad picture size: {name} is {v}; use a multiple of 16 from 256 to 1024"));
        }
    }
    Ok(())
}

/// Width and height from the PNG header, after checking the 8 byte signature and the first chunk.
pub fn png_size(bytes: &[u8]) -> Result<(u32, u32), String> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 24 || bytes[..8] != SIG {
        return Err("not a PNG file (bad signature)".into());
    }
    if bytes[8..12] != [0, 0, 0, 13] || &bytes[12..16] != b"IHDR" {
        return Err("PNG header chunk is missing or damaged".into());
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Ok((w, h))
}

/// Reads the picture and checks that it is a PNG of the requested size.
pub fn check_png(path: &Path, width: i64, height: i64) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("the picture {} is missing or unreadable: {e}", path.display()))?;
    let (w, h) = png_size(&bytes).map_err(|e| format!("the picture {} is unreadable: {e}", path.display()))?;
    if i64::from(w) != width || i64::from(h) != height {
        return Err(format!("the picture is {w}x{h} but {width}x{height} was asked for"));
    }
    Ok(())
}

pub fn request_json(install: &Install, dir: &Path, prompt: &str, width: i64, height: i64, steps: i64, seed: i64) -> Value {
    json!({"ai_root": install.root.to_string_lossy(), "out_dir": dir.to_string_lossy(), "prompt": prompt,
           "width": width, "height": height, "steps": steps, "seed": seed})
}

impl NodeOp for SketchFromWordsNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.ai_sketch_from_words", "AI Sketch from Words", Stage::Geometry)
            .output(PortSpec::new("image", DataKind::File))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("manifest", DataKind::Json))
            .param(ParamSpec::path("ai_root", "AI-3D folder", &[], false).help("Folder holding the Sketch install. Blank uses the VERNON_AI3D_ROOT environment variable."))
            .param(ParamSpec::string("wsl_distro", "WSL distribution", "AI-3D").help("Linux environment that holds the model's Python. Blank runs Python directly (Linux hosts)."))
            .param(ParamSpec::string("prompt", "Prompt", "").help("What to draw, in plain words. Short and concrete works best. Empty is an error."))
            .param(ParamSpec::int("width", "Width", DEFAULT_SIZE, 256, 1024).unit("px").help("Picture width in pixels. Must be a multiple of 16."))
            .param(ParamSpec::int("height", "Height", DEFAULT_SIZE, 256, 1024).unit("px").help("Picture height in pixels. Must be a multiple of 16."))
            .param(ParamSpec::int("steps", "Steps", DEFAULT_STEPS, 1, 8).help("Drawing steps. This model is made for about 4; more steps are slower and rarely better."))
            .param(ParamSpec::int("seed", "Seed", 0, 0, 2_147_483_647).help("Random seed. The same seed and inputs draw the same picture on the same machine."))
            .param(ParamSpec::int("timeout_s", "Time limit", 1800, 60, 7200).unit("s").help("The model process is stopped after this long."))
            .param(ParamSpec::bool("run", "Run model", false).action().help("Starts the model. Without it the node loads an earlier picture for identical inputs, if one exists."))
            .help("Draws one concept picture from a text prompt with a local image model. The picture is an idea sketch to look at: it is not a design, it measures nothing, and nothing in it is to scale.")
            .emulates("text to image generation")
            .spec("docs/specs/ai-local/sketch-from-words.md")
            .external("Sketch")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, _ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        sketch(ctx, p)
    }
}

/// The body of `cook`; the real-model test calls it directly.
pub fn sketch(ctx: &CookContext, p: &Params) -> CookResult<Vec<Data>> {
    let prompt = p.str("prompt")?.trim().to_string();
    if prompt.is_empty() {
        return Err(ctx.fail("there is no prompt: type what to draw in the 'Prompt' parameter"));
    }
    let (width, height) = (p.int("width")?, p.int("height")?);
    check_size(width, height).map_err(|e| ctx.fail(e))?;
    let (steps, seed) = (p.int("steps")?, p.int("seed")?);
    let dir = ctx.work_dir.join(format!("sketch_{:016x}", work_key(&prompt, width, height, steps, seed)));
    let png = dir.join("sketch.png");
    let result_path = dir.join("result.json");
    let mut rep = Report::new(SKETCH_LABEL);
    if !result_path.is_file() {
        if !p.bool("run")? {
            rep.note(Severity::Info, "not run: switch on 'Run model' to draw this picture");
            let manifest = json!({"status": "not_run", "advisory": true, "artifact_dir": dir.to_string_lossy()});
            return Ok(vec![Data::File(png), Data::report(rep), Data::json(manifest)]);
        }
        run_model(ctx, p, &dir, &prompt, width, height, steps, seed)?;
    }
    let res: Value = serde_json::from_slice(&std::fs::read(&result_path)?).map_err(|e| ctx.fail(format!("unreadable result.json: {e}")))?;
    check_png(&png, width, height).map_err(|e| ctx.fail(e))?;
    for (k, unit) in [("peak_vram_GB", "GB"), ("peak_rss_GB", "GB"), ("seconds_load", "s"), ("seconds_generate", "s")] {
        if let Some(v) = res.get(k).and_then(Value::as_f64) {
            rep.metric(format!("ai_sketch_{}", k.to_lowercase()), Metric::new(v, unit));
        }
    }
    rep.metric("ai_sketch_pixels", Metric::new((width * height) as f64, "px"));
    if res.get("offload").and_then(Value::as_bool) == Some(true) {
        rep.note(Severity::Info, "the model did not fit in free GPU memory, so parts of it were moved between GPU and RAM (slower)");
    }
    rep.note(Severity::Info, "a generated concept picture: it is not a design, it is not to scale and it measures nothing");
    let manifest = json!({
        "status": "complete", "advisory": true, "artifact_dir": dir.to_string_lossy(), "image": png.to_string_lossy(),
        "model": res.get("model"), "revision": res.get("revision"), "device": res.get("device"), "offload": res.get("offload"),
        "inputs": {"prompt": prompt, "width": width, "height": height, "steps": steps, "seed": seed},
        "pipeline": res.get("pipeline"), "diffusers": res.get("diffusers"), "torch": res.get("torch"),
        "peak_vram_GB": res.get("peak_vram_GB"), "peak_rss_GB": res.get("peak_rss_GB"),
        "seconds_load": res.get("seconds_load"), "seconds_generate": res.get("seconds_generate"),
    });
    let _ = std::fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap_or_default());
    Ok(vec![Data::File(png), Data::report(rep), Data::json(manifest)])
}

#[allow(clippy::too_many_arguments)]
fn run_model(ctx: &CookContext, p: &Params, dir: &Path, prompt: &str, width: i64, height: i64, steps: i64, seed: i64) -> CookResult<()> {
    let install = Install::resolve(p.str("ai_root")?, p.str("wsl_distro")?).map_err(|e| ctx.fail(e.to_string()))?;
    install.require_model_dir("Sketch").map_err(|e| ctx.fail(e.to_string()))?;
    let have = bridge::query_resources(&install).map_err(|e| ctx.fail(e.to_string()))?;
    check_resources(with_margin(PEAK_VRAM_GB, PEAK_RSS_GB), have, "Sketch").map_err(|e| ctx.fail(e.to_string()))?;
    std::fs::create_dir_all(dir)?;
    let request = request_json(&install, dir, prompt, width, height, steps, seed);
    let timeout = Duration::from_secs(p.int("timeout_s")?.max(60) as u64);
    bridge::run_job(&install, "Sketch", "sketch_bridge.py", SKETCH_BRIDGE, &request, dir, timeout).map_err(|e| match e {
        BridgeError::Cancelled => CookError::Other(vernon_graph::CANCELLED.into()),
        other => ctx.fail(other.to_string()),
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "sketch_tests.rs"]
mod tests;

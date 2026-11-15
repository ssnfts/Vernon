//! `geo.ai_vision_check`: a local vision-language model answers a question about an image.
//!
//! The model (Qwen3-VL-4B-Instruct, Apache-2.0) runs as a separate Python process
//! (`scripts/ai/vision_bridge.py`) on the GPU. The answer is a hint in words, never a
//! measurement: it is labelled as advisory everywhere and no gate may read it. The work
//! folder is named after a hash of the image bytes, the question and the token limit, so
//! identical inputs reload an earlier result without starting the model.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use vernon_core::report::{Metric, Severity};
use vernon_core::Report;
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::bridge::{self, check_resources, with_margin, BridgeError, Install};
use crate::fnv1a;

pub const VISION_BRIDGE: &str = include_str!("../../../scripts/ai/vision_bridge.py");
pub const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "ppm"];
pub const DEFAULT_QUESTION: &str = "Describe what this image shows and point out anything that looks broken, missing or out of place.";
/// First line of the report and the `label` field of the answer.
pub const ADVISORY_LABEL: &str = "AI description: a hint, not a measurement";
/// Peak GPU memory and RAM of the real run, see docs/specs/ai-local/vision-check.md.
pub const PEAK_VRAM_GB: f64 = 8.4;
pub const PEAK_RSS_GB: f64 = 9.2;

pub fn register(reg: &mut Registry) {
    reg.add(VisionCheckNode);
}

pub struct VisionCheckNode;

/// Folder name key for one image, one question and one token limit.
pub fn work_key(image: &[u8], question: &str, max_tokens: i64) -> u64 {
    let mut b = image.to_vec();
    b.extend_from_slice(format!("|qwen3-vl-4b-instruct|{max_tokens}|{question}").as_bytes());
    fnv1a(&b)
}

/// Lower-case extension if it is a supported image format.
pub fn image_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    IMAGE_EXTENSIONS.contains(&ext.as_str()).then_some(ext)
}

pub fn request_json(install: &Install, image: &Path, dir: &Path, question: &str, max_tokens: i64) -> Value {
    json!({"ai_root": install.root.to_string_lossy(), "out_dir": dir.to_string_lossy(), "image": image.to_string_lossy(),
           "question": question, "max_tokens": max_tokens})
}

/// The advisory answer object of a bridge result. An empty answer is an error.
pub fn parse_answer(res: &Value) -> Result<Value, String> {
    let answer = res.get("answer").and_then(Value::as_str).ok_or("result.json has no 'answer' string")?.trim();
    if answer.is_empty() {
        return Err("the model gave an empty answer; try rephrasing the question or raising 'max_tokens'".into());
    }
    let question = res.get("question").and_then(Value::as_str).unwrap_or("").trim();
    Ok(json!({"question": question, "answer": answer, "advisory": true, "label": ADVISORY_LABEL}))
}

impl NodeOp for VisionCheckNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.ai_vision_check", "AI Vision Check", Stage::Geometry)
            .output(PortSpec::new("answer", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("manifest", DataKind::Json))
            .param(ParamSpec::path("ai_root", "AI-3D folder", &[], false).help("Folder holding the Vision install. Blank uses the VERNON_AI3D_ROOT environment variable."))
            .param(ParamSpec::string("wsl_distro", "WSL distribution", "AI-3D").help("Linux environment that holds the model's Python. Blank runs Python directly (Linux hosts)."))
            .param(ParamSpec::path("image", "Image", &IMAGE_EXTENSIONS, false).help("A render or photo: png, jpg, jpeg, webp or ppm. Larger images are shrunk so the longer side is at most 1024 pixels."))
            .param(ParamSpec::string("question", "Question", DEFAULT_QUESTION).help("What to ask about the image, in plain words. Short, specific questions work best."))
            .param(ParamSpec::int("max_tokens", "Answer length limit", 256, 16, 1024).unit("tokens").help("The answer is cut off after this many tokens (a token is about three quarters of a word)."))
            .param(ParamSpec::int("timeout_s", "Time limit", 900, 60, 7200).unit("s").help("The model process is stopped after this long."))
            .param(ParamSpec::bool("run", "Run model", false).action().help("Starts the model. Without it the node loads an earlier result for identical inputs, if one exists."))
            .help("Asks a local vision-language model a question about a render or photo and returns its answer in words. The answer is a hint to read next to the numeric reports: it can be confidently wrong, it measures nothing, and no gate may use it.")
            .emulates("visual question answering on a render or photo")
            .spec("docs/specs/ai-local/vision-check.md")
            .implemented()
            .external("Vision")
    }

    fn cook(&self, ctx: &CookContext, _ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        vision_check(ctx, p)
    }
}

fn idle(rep: Report, manifest: Value) -> Vec<Data> {
    let answer = json!({"question": "", "answer": "", "advisory": true, "label": ADVISORY_LABEL});
    vec![Data::json(answer), Data::report(rep), Data::json(manifest)]
}

/// The body of `cook`; the real-model test calls it directly.
pub fn vision_check(ctx: &CookContext, p: &Params) -> CookResult<Vec<Data>> {
    let image = p.str("image")?.trim().to_string();
    let mut rep = Report::new(ADVISORY_LABEL);
    if image.is_empty() {
        rep.note(Severity::Info, "choose an image to look at");
        return Ok(idle(rep, json!({"status": "no_image"})));
    }
    let path = Path::new(&image);
    if !path.is_file() {
        return Err(ctx.fail(format!("image not found: {image}")));
    }
    if image_extension(path).is_none() {
        return Err(ctx.fail(format!("unsupported image format: {image}; use one of {}", IMAGE_EXTENSIONS.join(", "))));
    }
    let question = p.str("question")?.trim().to_string();
    if question.is_empty() {
        return Err(ctx.fail("there is no question: type one in the 'Question' parameter"));
    }
    let bytes = std::fs::read(path).map_err(|e| ctx.fail(format!("cannot read image {image}: {e}")))?;
    let max_tokens = p.int("max_tokens")?;
    let dir = ctx.work_dir.join(format!("vision_{:016x}", work_key(&bytes, &question, max_tokens)));
    let result_path = dir.join("result.json");
    if !result_path.is_file() {
        if !p.bool("run")? {
            rep.note(Severity::Info, "not run: switch on 'Run model' to ask this question");
            return Ok(idle(rep, json!({"status": "not_run", "artifact_dir": dir.to_string_lossy()})));
        }
        run_model(ctx, p, &dir, path, &bytes, &question, max_tokens)?;
    }
    let res: Value = serde_json::from_slice(&std::fs::read(&result_path)?).map_err(|e| ctx.fail(format!("unreadable result.json: {e}")))?;
    let answer = parse_answer(&res).map_err(|e| ctx.fail(e))?;
    let words = answer["answer"].as_str().unwrap_or("").split_whitespace().count();
    rep.metric("ai_vision_answer_words", Metric::new(words as f64, "count"));
    if let Some(n) = res.get("new_tokens").and_then(Value::as_f64) {
        rep.metric("ai_vision_new_tokens", Metric::new(n, "count"));
        if n >= max_tokens as f64 {
            rep.note(Severity::Warning, "the answer reached the length limit and may be cut off: raise 'Answer length limit'");
        }
    }
    for (k, unit) in [("peak_vram_GB", "GB"), ("peak_rss_GB", "GB"), ("seconds_load", "s"), ("seconds_generate", "s")] {
        if let Some(v) = res.get(k).and_then(Value::as_f64) {
            rep.metric(format!("ai_vision_{}", k.to_lowercase()), Metric::new(v, unit));
        }
    }
    rep.note(Severity::Info, "a model's description can be confidently wrong and says nothing measurable: check it against the numeric reports");
    let manifest = json!({
        "status": "complete", "advisory": true, "artifact_dir": dir.to_string_lossy(), "model": res.get("model"), "revision": res.get("revision"),
        "device": res.get("device"), "image": {"path": image, "size": res.get("image_size"), "encoded_size": res.get("encoded_size")},
        "question": question, "max_tokens": max_tokens, "new_tokens": res.get("new_tokens"),
        "peak_vram_GB": res.get("peak_vram_GB"), "peak_rss_GB": res.get("peak_rss_GB"),
        "seconds_load": res.get("seconds_load"), "seconds_generate": res.get("seconds_generate"),
    });
    let _ = std::fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap_or_default());
    Ok(vec![Data::json(answer), Data::report(rep), Data::json(manifest)])
}

fn run_model(ctx: &CookContext, p: &Params, dir: &Path, image: &Path, bytes: &[u8], question: &str, max_tokens: i64) -> CookResult<()> {
    let install = Install::resolve(p.str("ai_root")?, p.str("wsl_distro")?).map_err(|e| ctx.fail(e.to_string()))?;
    install.require_model_dir("Vision").map_err(|e| ctx.fail(e.to_string()))?;
    let have = bridge::query_resources(&install).map_err(|e| ctx.fail(e.to_string()))?;
    check_resources(with_margin(PEAK_VRAM_GB, PEAK_RSS_GB), have, "Vision").map_err(|e| ctx.fail(e.to_string()))?;
    std::fs::create_dir_all(dir)?;
    let ext = image_extension(image).unwrap_or_else(|| "png".into());
    let kept = dir.join(format!("input.{ext}"));
    std::fs::write(&kept, bytes)?;
    let request = request_json(&install, &kept, dir, question, max_tokens);
    let timeout = Duration::from_secs(p.int("timeout_s")?.max(60) as u64);
    bridge::run_job(&install, "Vision", "vision_bridge.py", VISION_BRIDGE, &request, dir, timeout).map_err(|e| match e {
        BridgeError::Cancelled => CookError::Other(vernon_graph::CANCELLED.into()),
        other => ctx.fail(other.to_string()),
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "vision_tests.rs"]
mod tests;

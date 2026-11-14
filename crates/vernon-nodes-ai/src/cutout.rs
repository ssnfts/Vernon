//! `capture.ai_photo_cutout`: click one object in a photo and cut it out with SAM 2.1.
//!
//! The model (facebook/sam2.1-hiera-large, Apache-2.0) runs as a separate process
//! (`scripts/ai/sam2_bridge.py`). It writes an RGBA PNG plus the raw mask bytes
//! (`mask.u8`, width x height, 0 or 255). Vernon reads the mask itself, checks its
//! length, recomputes the bounding box and area fraction, and refuses an empty mask
//! or one that covers almost the whole picture.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use vernon_core::report::{Metric, Severity};
use vernon_core::Report;
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::bridge::{self, check_resources, with_margin, BridgeError, Install};
use crate::fnv1a;

pub const SAM2_BRIDGE: &str = include_str!("../../../scripts/ai/sam2_bridge.py");
pub const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "ppm"];
/// A mask covering more than this fraction of the picture is refused.
pub const MAX_AREA_FRACTION: f64 = 0.99;
/// Peak GPU memory and RAM of the real run, see docs/specs/ai-local/photo-cutout.md.
pub const PEAK_VRAM_GB: f64 = 1.3;
pub const PEAK_RSS_GB: f64 = 1.7;

pub fn register(reg: &mut Registry) {
    reg.add(PhotoCutoutNode);
}

/// What the Rust side measured in a raw mask.
#[derive(Debug, Clone, PartialEq)]
pub struct MaskStats {
    pub width: usize,
    pub height: usize,
    pub pixels: usize,
    /// Inclusive pixel bounds `[x0, y0, x1, y1]` of the mask.
    pub bbox: [usize; 4],
    pub area_fraction: f64,
}

/// Measure a raw mask (any non-zero byte counts as inside). Refuses a wrong length,
/// an empty mask and one covering more than [`MAX_AREA_FRACTION`] of the image.
pub fn mask_stats(mask: &[u8], width: usize, height: usize) -> Result<MaskStats, String> {
    if width == 0 || height == 0 {
        return Err("the image has zero width or height".into());
    }
    if mask.len() != width * height {
        return Err(format!("the mask file has {} bytes but the image is {width} x {height} = {} pixels", mask.len(), width * height));
    }
    let (mut x0, mut y0, mut x1, mut y1, mut n) = (usize::MAX, usize::MAX, 0usize, 0usize, 0usize);
    for (i, &v) in mask.iter().enumerate() {
        if v != 0 {
            let (x, y) = (i % width, i / width);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            n += 1;
        }
    }
    if n == 0 {
        return Err("the model found nothing at that point (the mask is empty): click somewhere on the object".into());
    }
    let area_fraction = n as f64 / (width * height) as f64;
    if area_fraction > MAX_AREA_FRACTION {
        return Err(format!("the mask covers {:.1}% of the picture, so nothing was cut out: click on a single object, not the background", area_fraction * 100.0));
    }
    Ok(MaskStats { width, height, pixels: n, bbox: [x0, y0, x1, y1], area_fraction })
}

/// Hash of everything that changes the result: the image bytes and the clicked point.
pub fn input_key(image: &[u8], x: f64, y: f64) -> u64 {
    let mut b = format!("sam2.1-hiera-large|{x:.4}|{y:.4}|").into_bytes();
    b.extend_from_slice(image);
    fnv1a(&b)
}

pub fn build_request(install: &Install, dir: &Path, image: &Path, x: f64, y: f64) -> Value {
    json!({"ai_root": install.root.to_string_lossy(), "out_dir": dir.to_string_lossy(), "image": image.to_string_lossy(), "point_x": x, "point_y": y})
}

/// Lower-case extension if it is a supported image type.
pub fn image_extension(path: &str) -> Option<String> {
    let e = Path::new(path).extension()?.to_string_lossy().to_ascii_lowercase();
    IMAGE_EXTENSIONS.contains(&e.as_str()).then_some(e)
}

/// The message shown when the model process fails.
pub fn bridge_failure(e: &BridgeError) -> String {
    e.to_string()
}

pub struct PhotoCutoutNode;

impl NodeOp for PhotoCutoutNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("capture.ai_photo_cutout", "AI Photo Cut-out", Stage::Capture)
            .output(PortSpec::new("cutout", DataKind::File))
            .output(PortSpec::new("mask", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("manifest", DataKind::Json))
            .param(ParamSpec::path("ai_root", "AI-3D folder", &[], false).help("Folder holding the SAM2 install. Blank uses the VERNON_AI3D_ROOT environment variable."))
            .param(ParamSpec::string("wsl_distro", "WSL distribution", "AI-3D").help("Linux environment that holds the model's Python. Blank runs Python directly (Linux hosts)."))
            .param(ParamSpec::path("image", "Photo", &IMAGE_EXTENSIONS, false).help("The photo to cut an object out of (png, jpg, jpeg, webp or ppm)."))
            .param(ParamSpec::float("point_x", "Click X", 0.5, 0.0, 1.0).help("Where you click on the object, as a fraction of the picture width: 0 is the left edge, 1 the right edge."))
            .param(ParamSpec::float("point_y", "Click Y", 0.5, 0.0, 1.0).help("Where you click on the object, as a fraction of the picture height: 0 is the top edge, 1 the bottom edge."))
            .param(ParamSpec::int("timeout_s", "Time limit", 600, 30, 7200).unit("s").help("The model process is stopped after this long."))
            .param(ParamSpec::bool("run", "Run model", false).action().help("Starts the model. Without it the node loads an earlier result for the same photo and click, if one exists."))
            .help("Cuts one object out of a photo. The model marks the object under your click; the result is a PNG the same size as the photo, transparent everywhere except the object, plus the mask outline numbers (bounding box, area, model score). The mask is a model guess: check it by eye before you use it.")
            .emulates("click to segment one object in a photo")
            .spec("docs/specs/ai-local/photo-cutout.md")
            .implemented()
            .external("SAM2")
    }

    fn cook(&self, ctx: &CookContext, _ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        cut_out(ctx, p)
    }
}

fn idle(rep: Report, manifest: Value) -> Vec<Data> {
    vec![Data::File(Default::default()), Data::json(json!({})), Data::report(rep), Data::json(manifest)]
}

/// The body of `cook`; the real-model test calls it directly.
pub fn cut_out(ctx: &CookContext, p: &Params) -> CookResult<Vec<Data>> {
    let image = p.str("image")?.trim().to_string();
    if image.is_empty() {
        return Err(ctx.fail("there is no photo: set the 'Photo' parameter"));
    }
    if !Path::new(&image).is_file() {
        return Err(ctx.fail(format!("photo not found: {image}")));
    }
    let Some(ext) = image_extension(&image) else {
        return Err(ctx.fail(format!("unsupported photo type for {image}: use one of {}", IMAGE_EXTENSIONS.join(", "))));
    };
    let (x, y) = (p.float("point_x")?, p.float("point_y")?);
    if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return Err(ctx.fail(format!("the click point ({x}, {y}) is outside the picture: point_x and point_y must be between 0 and 1")));
    }
    let bytes = std::fs::read(&image).map_err(|e| ctx.fail(format!("cannot read {image}: {e}")))?;
    let key = input_key(&bytes, x, y);
    let dir = ctx.work_dir.join(format!("cutout_{key:016x}"));
    let result_path = dir.join("result.json");
    let mut rep = Report::new("AI photo cut-out");
    if !result_path.is_file() {
        if !p.bool("run")? {
            rep.note(Severity::Info, "not run: switch on 'Run model' to cut out the object at this point");
            return Ok(idle(rep, json!({"status": "not_run", "artifact_dir": dir.to_string_lossy()})));
        }
        run_model(ctx, p, &dir, &bytes, &ext, x, y)?;
    }
    let res: Value = serde_json::from_slice(&std::fs::read(&result_path)?).map_err(|e| ctx.fail(format!("unreadable result.json: {e}")))?;
    let dim = |k: &str| res.get(k).and_then(Value::as_u64).map(|v| v as usize);
    let (Some(w), Some(h)) = (dim("width"), dim("height")) else {
        return Err(ctx.fail("result.json has no image width and height"));
    };
    let mask_path = dir.join(res.get("mask").and_then(Value::as_str).unwrap_or("mask.u8"));
    let mask = std::fs::read(&mask_path).map_err(|e| ctx.fail(format!("cannot read {}: {e}", mask_path.display())))?;
    let st = mask_stats(&mask, w, h).map_err(|e| ctx.fail(format!("{}: {e}", mask_path.display())))?;
    let png: PathBuf = dir.join(res.get("cutout").and_then(Value::as_str).unwrap_or("cutout.png"));
    if !png.is_file() {
        return Err(ctx.fail(format!("the cut-out PNG is missing: {}", png.display())));
    }
    let score = res.get("score").and_then(Value::as_f64).unwrap_or(f64::NAN);
    rep.metric("ai_cutout_area_fraction", Metric::new(st.area_fraction, "fraction of image"));
    rep.metric("ai_cutout_width", Metric::new(w as f64, "px"));
    rep.metric("ai_cutout_height", Metric::new(h as f64, "px"));
    if score.is_finite() {
        rep.metric("ai_cutout_score", Metric::new(score, "model quality estimate, 0..1"));
    }
    for (k, unit) in [("seconds_load", "s"), ("seconds_segment", "s"), ("peak_vram_GB", "GB"), ("peak_rss_GB", "GB")] {
        if let Some(v) = res.get(k).and_then(Value::as_f64) {
            rep.metric(format!("ai_cutout_{}", k.to_lowercase()), Metric::new(v, unit));
        }
    }
    if score.is_finite() && score < 0.7 {
        rep.note(Severity::Warning, format!("the model is unsure about this mask (score {score:.2}): try another click point"));
    }
    rep.note(Severity::Info, "the mask is a model guess, not a measurement: check the cut-out by eye before using it");
    rep.note(Severity::Info, format!("cut out {:.1}% of the {w} x {h} picture, box [{}, {}, {}, {}]", st.area_fraction * 100.0, st.bbox[0], st.bbox[1], st.bbox[2], st.bbox[3]));
    let mask_json = json!({"width": w, "height": h, "bbox": st.bbox, "area_fraction": st.area_fraction, "score": score});
    let manifest = json!({
        "status": "complete", "artifact_dir": dir.to_string_lossy(), "cutout": png.to_string_lossy(), "mask_file": mask_path.to_string_lossy(),
        "model": res.get("model"), "revision": res.get("revision"), "library": res.get("library"), "device": res.get("device"),
        "image": {"path": image, "width": w, "height": h}, "point": [x, y], "mask": mask_json,
        "seconds_load": res.get("seconds_load"), "seconds_segment": res.get("seconds_segment"),
        "peak_vram_GB": res.get("peak_vram_GB"), "peak_rss_GB": res.get("peak_rss_GB"),
    });
    let _ = std::fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap_or_default());
    Ok(vec![Data::File(png), Data::json(mask_json), Data::report(rep), Data::json(manifest)])
}

fn run_model(ctx: &CookContext, p: &Params, dir: &Path, bytes: &[u8], ext: &str, x: f64, y: f64) -> CookResult<()> {
    let install = Install::resolve(p.str("ai_root")?, p.str("wsl_distro")?).map_err(|e| ctx.fail(e.to_string()))?;
    install.require_model_dir("SAM2").map_err(|e| ctx.fail(e.to_string()))?;
    let have = bridge::query_resources(&install).map_err(|e| ctx.fail(e.to_string()))?;
    check_resources(with_margin(PEAK_VRAM_GB, PEAK_RSS_GB), have, "SAM2").map_err(|e| ctx.fail(e.to_string()))?;
    std::fs::create_dir_all(dir)?;
    let kept = dir.join(format!("input.{ext}"));
    std::fs::write(&kept, bytes)?;
    let request = build_request(&install, dir, &kept, x, y);
    let timeout = Duration::from_secs(p.int("timeout_s")?.max(30) as u64);
    bridge::run_job(&install, "SAM2", "sam2_bridge.py", SAM2_BRIDGE, &request, dir, timeout).map_err(|e| match e {
        BridgeError::Cancelled => CookError::Other(vernon_graph::CANCELLED.into()),
        other => ctx.fail(bridge_failure(&other)),
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "cutout_tests.rs"]
mod tests;

//! `capture.ai_body_fit`: a body surface and key points from one photo of a person.
//!
//! The model (SAM 3D Body, DINOv3-H+ backbone) runs as a separate Python process
//! (`scripts/ai/bodyfit_bridge.py`) on the GPU. It is opt-in: its licence is not an
//! open-source one, the weights are gated, and the node refuses to start the model until
//! the user ticks the licence parameter. The result is a guess at shape and pose from one
//! picture, never a measurement of the person, and no gate may read it. The work folder is
//! named after a hash of the image bytes, so the same photo reloads an earlier result.

use std::path::Path;
use std::time::Duration;

use glam::DVec3;
use serde_json::{json, Value};
use vernon_core::report::{Metric, Severity};
use vernon_core::Report;
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::bridge::{self, check_resources, with_margin, BridgeError, Install};
use crate::{fnv1a, prep};

pub const BODYFIT_BRIDGE: &str = include_str!("../../../scripts/ai/bodyfit_bridge.py");
pub const IMAGE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];
/// First line of the report and the `label` field of the pose and manifest.
pub const BODY_LABEL: &str = "AI body estimate from one photo: a guess at shape and pose, not a measurement of the person";
/// Shown until the licence parameter is ticked, and stored in the manifest.
pub const LICENCE_NOTE: &str = "SAM License (not open source): no military, weapons, nuclear or espionage use; pass it on only under the same licence";
/// Frame of every position this node returns.
pub const FRAME: &str = "mm; z up; x to the picture's right; y away from the camera; lowest point at z = 0, centred on x and y";
/// Peak GPU memory and RAM of the real run, see docs/specs/ai-local/body-fit.md.
pub const PEAK_VRAM_GB: f64 = 3.4;
pub const PEAK_RSS_GB: f64 = 5.0;

pub fn register(reg: &mut Registry) {
    reg.add(BodyFitNode);
}

pub struct BodyFitNode;

/// Folder name key for one photo.
pub fn work_key(image: &[u8]) -> u64 {
    let mut b = image.to_vec();
    b.extend_from_slice(b"|sam-3d-body-dinov3|whole-picture");
    fnv1a(&b)
}

/// Lower-case extension if it is a supported image format.
pub fn image_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    IMAGE_EXTENSIONS.contains(&ext.as_str()).then_some(ext)
}

/// The model's camera frame (metres, x right, y down, z away) to the app's (mm, z up).
pub fn to_app_mm(p: DVec3) -> DVec3 {
    DVec3::new(p.x, p.z, -p.y) * 1000.0
}

/// How the body is moved and scaled after `to_app_mm`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub shift: DVec3,
    pub scale: f64,
    /// Top-to-bottom extent of the body as the model returned it.
    pub model_height_mm: f64,
}

impl Placement {
    pub fn apply(&self, p: DVec3) -> DVec3 {
        (p - self.shift) * self.scale
    }
}

/// Centres the body on x and y, puts its lowest point at z = 0, and, when `known_height_mm`
/// is above zero, scales it uniformly so its top-to-bottom extent is that height.
pub fn placement(points_mm: &[DVec3], known_height_mm: f64) -> Result<Placement, String> {
    if points_mm.is_empty() {
        return Err("the body mesh has no points".into());
    }
    let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
    for q in points_mm {
        lo = lo.min(*q);
        hi = hi.max(*q);
    }
    let height = hi.z - lo.z;
    if !height.is_finite() || !lo.is_finite() || !hi.is_finite() || height <= 0.0 {
        return Err("the body mesh has no height".into());
    }
    let scale = if known_height_mm > 0.0 { known_height_mm / height } else { 1.0 };
    Ok(Placement { shift: DVec3::new((lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5, lo.z), scale, model_height_mm: height })
}

/// One named key point: position in the model's camera frame (metres) and in the picture (pixels).
#[derive(Debug, Clone, PartialEq)]
pub struct KeyPoint {
    pub name: String,
    pub xyz_m: DVec3,
    pub uv_px: [f64; 2],
}

pub fn parse_keypoints(res: &Value) -> Result<Vec<KeyPoint>, String> {
    let rows = res.get("keypoints").and_then(Value::as_array).ok_or("result.json has no 'keypoints' list")?;
    let mut out = Vec::with_capacity(rows.len());
    for (i, r) in rows.iter().enumerate() {
        let nums = |key: &str, n: usize| -> Result<Vec<f64>, String> {
            let v: Vec<f64> = r.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
            if v.len() != n || v.iter().any(|c| !c.is_finite()) {
                return Err(format!("key point {i} has a bad '{key}'"));
            }
            Ok(v)
        };
        let (p, uv) = (nums("xyz_m", 3)?, nums("uv_px", 2)?);
        let name = r.get("name").and_then(Value::as_str).ok_or(format!("key point {i} has no name"))?.to_string();
        out.push(KeyPoint { name, xyz_m: DVec3::new(p[0], p[1], p[2]), uv_px: [uv[0], uv[1]] });
    }
    if out.is_empty() {
        return Err("result.json has an empty 'keypoints' list".into());
    }
    Ok(out)
}

impl NodeOp for BodyFitNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("capture.ai_body_fit", "AI Body Fit from Photo", Stage::Capture)
            .output(PortSpec::new("body", DataKind::Geometry))
            .output(PortSpec::new("pose", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("manifest", DataKind::Json))
            .param(ParamSpec::path("ai_root", "AI-3D folder", &[], false).help("Folder holding the BodyFit install. Blank uses the VERNON_AI3D_ROOT environment variable."))
            .param(ParamSpec::string("wsl_distro", "WSL distribution", "AI-3D").help("Linux environment that holds the model's Python. Blank runs Python directly (Linux hosts)."))
            .param(ParamSpec::path("image", "Photo", &IMAGE_EXTENSIONS, false).help("A photo of one person, cropped so the person fills the picture, whole body in view: png, jpg, jpeg or webp. The whole picture is treated as that one person."))
            .param(ParamSpec::bool("licence_accepted", "I accept the model's licence", false).help("This model is not open source. Tick only after reading the SAM License saved in the BodyFit folder: no military, weapons, nuclear or espionage use, and it may be passed on only under the same licence. The model does not start without this."))
            .param(ParamSpec::float("known_height_mm", "Known height", 0.0, 0.0, 2500.0).unit("mm").help("0 keeps the model's own guess of the body's size. Above 0, the body is scaled so its top-to-bottom extent is this height, which is only right when the person stands upright."))
            .param(ParamSpec::int("timeout_s", "Time limit", 900, 60, 7200).unit("s").help("The model process is stopped after this long."))
            .param(ParamSpec::bool("run", "Run model", false).action().help("Starts the model. Without it the node loads an earlier result for the same photo, if one exists."))
            .help("Estimates a 3D body surface and named key points from one photo of a person, for sizing something to be worn. It is a guess from a single picture: size, proportions and hidden sides can be wrong, it is not a measurement, and no gate may use it. The photo stays on this computer. The model is opt-in and under a restrictive licence.")
            .emulates("single-photo human body shape and pose estimation")
            .spec("docs/specs/ai-local/body-fit.md")
            .implemented()
            .external("BodyFit")
    }

    fn cook(&self, ctx: &CookContext, _ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        body_fit(ctx, p)
    }
}

fn idle(rep: Report, manifest: Value) -> Vec<Data> {
    let pose = json!({"keypoints": [], "advisory": true, "label": BODY_LABEL, "frame": FRAME});
    vec![Data::geometry(vernon_core::Geometry::new()), Data::json(pose), Data::report(rep), Data::json(manifest)]
}

/// The body of `cook`; the real-model test calls it directly.
pub fn body_fit(ctx: &CookContext, p: &Params) -> CookResult<Vec<Data>> {
    let image = p.str("image")?.trim().to_string();
    let mut rep = Report::new(BODY_LABEL);
    if image.is_empty() {
        rep.note(Severity::Info, "choose a photo of one person");
        return Ok(idle(rep, json!({"status": "no_image"})));
    }
    let path = Path::new(&image);
    if !path.is_file() {
        return Err(ctx.fail(format!("photo not found: {image}")));
    }
    if image_extension(path).is_none() {
        return Err(ctx.fail(format!("unsupported image format: {image}; use one of {}", IMAGE_EXTENSIONS.join(", "))));
    }
    let bytes = std::fs::read(path).map_err(|e| ctx.fail(format!("cannot read photo {image}: {e}")))?;
    let dir = ctx.work_dir.join(format!("bodyfit_{:016x}", work_key(&bytes)));
    let result_path = dir.join("result.json");
    if !result_path.is_file() {
        if !p.bool("run")? {
            rep.note(Severity::Info, "not run: switch on 'Run model' to fit a body to this photo");
            rep.note(Severity::Info, format!("opt-in model. {LICENCE_NOTE}"));
            return Ok(idle(rep, json!({"status": "not_run", "artifact_dir": dir.to_string_lossy(), "licence": LICENCE_NOTE})));
        }
        if !p.bool("licence_accepted")? {
            return Err(ctx.fail(format!("the model was not started: tick 'I accept the model's licence' first. {LICENCE_NOTE}")));
        }
        run_model(ctx, p, &dir, path, &bytes)?;
    }
    let res: Value = serde_json::from_slice(&std::fs::read(&result_path)?).map_err(|e| ctx.fail(format!("unreadable result.json: {e}")))?;
    let mesh_file = res.get("mesh").and_then(Value::as_str).unwrap_or("body.obj");
    let mut geo = vernon_io::read(&dir.join(mesh_file), &vernon_io::ReadOptions::default()).map_err(|e| ctx.fail(format!("cannot import {mesh_file}: {e}")))?;
    if let Some(n) = res.get("vertices").and_then(Value::as_u64) {
        if n as usize != geo.points.len() {
            return Err(ctx.fail(format!("{mesh_file} has {} points but result.json says {n}", geo.points.len())));
        }
    }
    let keypoints = parse_keypoints(&res).map_err(|e| ctx.fail(e))?;
    for q in geo.points.iter_mut() {
        *q = to_app_mm(*q);
    }
    let known = p.float("known_height_mm")?;
    let place = placement(&geo.points, known).map_err(|e| ctx.fail(e))?;
    for q in geo.points.iter_mut() {
        *q = place.apply(*q);
    }
    let tris: Vec<[DVec3; 3]> = geo.triangles().iter().map(|(_, t)| [geo.points[t[0] as usize], geo.points[t[1] as usize], geo.points[t[2] as usize]]).collect();
    if tris.is_empty() {
        return Err(ctx.fail(format!("{mesh_file} has no triangles")));
    }
    let (open, non_manifold, components) = prep::topology(&tris);
    let height_out = place.model_height_mm * place.scale;
    rep.metric("ai_body_points", Metric::new(geo.points.len() as f64, "count"));
    rep.metric("ai_body_triangles", Metric::new(tris.len() as f64, "count"));
    rep.metric("ai_body_model_height_mm", Metric::new(place.model_height_mm, "mm"));
    rep.metric("ai_body_height_mm", Metric::new(height_out, "mm"));
    rep.metric("ai_body_open_edges", Metric::new(open as f64, "count"));
    rep.metric("ai_body_keypoints", Metric::new(keypoints.len() as f64, "count"));
    for (k, unit) in [("peak_vram_GB", "GB"), ("peak_rss_GB", "GB"), ("seconds_load", "s"), ("seconds_fit", "s")] {
        if let Some(v) = res.get(k).and_then(Value::as_f64) {
            rep.metric(format!("ai_body_{}", k.to_lowercase()), Metric::new(v, unit));
        }
    }
    if known > 0.0 {
        rep.note(Severity::Info, format!("scaled by {:.3} so the body is {known:.0} mm top to bottom; this is only right if the person stands upright", place.scale));
    } else {
        rep.note(Severity::Warning, format!("the size is the model's own guess ({:.0} mm top to bottom); one photo cannot fix real size, so set 'Known height' if you know it", place.model_height_mm));
    }
    if open > 0 {
        rep.note(Severity::Info, format!("the surface has {open} open edge(s): it is not a closed solid"));
    }
    rep.note(Severity::Warning, "the whole picture was treated as one person; sides the camera did not see, loose clothing and hair are guessed, so check any size against a tape measure before making something to wear");
    let points: Vec<Value> = keypoints
        .iter()
        .map(|k| {
            let q = place.apply(to_app_mm(k.xyz_m));
            json!({"name": k.name, "xyz_mm": [q.x, q.y, q.z], "uv_px": k.uv_px})
        })
        .collect();
    let pose = json!({"keypoints": points, "advisory": true, "label": BODY_LABEL, "frame": FRAME, "image_size": res.get("image_size")});
    let manifest = json!({
        "status": "complete", "advisory": true, "label": BODY_LABEL, "licence": LICENCE_NOTE, "artifact_dir": dir.to_string_lossy(),
        "model": res.get("model"), "revision": res.get("revision"), "code_commit": res.get("code_commit"), "backbone_commit": res.get("backbone_commit"),
        "device": res.get("device"), "image": {"path": image, "size": res.get("image_size")}, "frame": FRAME,
        "points": geo.points.len(), "triangles": tris.len(), "open_edges": open, "non_manifold_edges": non_manifold, "components": components,
        "model_height_mm": place.model_height_mm, "known_height_mm": known, "scale": place.scale, "height_mm": height_out,
        "focal_length_px": res.get("focal_length_px"), "camera_translation_m": res.get("camera_translation_m"),
        "shape_params": res.get("shape_params"), "scale_params": res.get("scale_params"),
        "peak_vram_GB": res.get("peak_vram_GB"), "peak_rss_GB": res.get("peak_rss_GB"),
        "seconds_load": res.get("seconds_load"), "seconds_fit": res.get("seconds_fit"),
    });
    let _ = std::fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap_or_default());
    Ok(vec![Data::geometry(geo), Data::json(pose), Data::report(rep), Data::json(manifest)])
}

fn run_model(ctx: &CookContext, p: &Params, dir: &Path, image: &Path, bytes: &[u8]) -> CookResult<()> {
    let install = Install::resolve(p.str("ai_root")?, p.str("wsl_distro")?).map_err(|e| ctx.fail(e.to_string()))?;
    install.require_model_dir("BodyFit").map_err(|e| ctx.fail(e.to_string()))?;
    let have = bridge::query_resources(&install).map_err(|e| ctx.fail(e.to_string()))?;
    check_resources(with_margin(PEAK_VRAM_GB, PEAK_RSS_GB), have, "BodyFit").map_err(|e| ctx.fail(e.to_string()))?;
    std::fs::create_dir_all(dir)?;
    let ext = image_extension(image).unwrap_or_else(|| "png".into());
    let kept = dir.join(format!("input.{ext}"));
    std::fs::write(&kept, bytes)?;
    let request = json!({"ai_root": install.root.to_string_lossy(), "out_dir": dir.to_string_lossy(), "image": kept.to_string_lossy()});
    let timeout = Duration::from_secs(p.int("timeout_s")?.max(60) as u64);
    bridge::run_job(&install, "BodyFit", "bodyfit_bridge.py", BODYFIT_BRIDGE, &request, dir, timeout).map_err(|e| match e {
        BridgeError::Cancelled => CookError::Other(vernon_graph::CANCELLED.into()),
        other => ctx.fail(other.to_string()),
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "bodyfit_tests.rs"]
mod tests;

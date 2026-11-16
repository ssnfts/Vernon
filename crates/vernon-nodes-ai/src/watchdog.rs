//! `print.ai_print_watchdog`: asks the local vision model whether the newest camera
//! picture of a print looks like a failed print.
//!
//! The node looks in a folder that a printer camera or a time-lapse tool writes pictures
//! into, takes the newest picture, and hands it with a fixed yes/no question to the same
//! model and bridge as `geo.ai_vision_check`. The answer becomes one of three verdicts.
//! It is a hint: it can miss a failed print and raise a false alarm, it never stops or
//! pauses a printer, and no gate may read it. A picture that was already checked reloads
//! its earlier answer without starting the model.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use vernon_core::report::{Metric, Severity};
use vernon_core::Report;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, ParamValue, Params, PortSpec, Registry, Stage};

use crate::vision::{self, image_extension};

/// First line of the report and the `label` field of the verdict.
pub const WATCHDOG_LABEL: &str = "AI print check: a hint, not a measurement; it never stops a print";
pub const DEFAULT_QUESTION: &str = "This is a camera picture of a 3D printer while it prints. Does the print look failed, for example loose strands of plastic, a part that has come off the bed, or a collapsed part? Answer with the single word YES or NO first, then one short sentence saying what you see.";
pub const VERDICT_FAILURE: &str = "possible_failure";
pub const VERDICT_OK: &str = "looks_ok";
pub const VERDICT_UNCLEAR: &str = "unclear";
pub const VERDICT_NONE: &str = "not_checked";
/// A folder with more pictures than this is refused, so a cook never stalls on listing it.
pub const MAX_FRAMES: usize = 50_000;

pub fn register(reg: &mut Registry) {
    reg.add(PrintWatchdogNode);
}

pub struct PrintWatchdogNode;

/// Turns the model's words into a verdict: the first word decides.
pub fn verdict_of(answer: &str) -> &'static str {
    let first: String = answer.chars().skip_while(|c| !c.is_alphabetic()).take_while(|c| c.is_alphabetic()).collect();
    match first.to_lowercase().as_str() {
        "yes" => VERDICT_FAILURE,
        "no" => VERDICT_OK,
        _ => VERDICT_UNCLEAR,
    }
}

/// The newest supported picture directly inside `folder` (by modified time, then by name),
/// and how many supported pictures the folder holds. Sub-folders are not searched.
pub fn newest_frame(folder: &Path) -> Result<Option<(PathBuf, usize)>, String> {
    let mut best: Option<(std::time::SystemTime, String, PathBuf)> = None;
    let mut count = 0usize;
    for entry in std::fs::read_dir(folder).map_err(|e| format!("cannot list frames folder {}: {e}", folder.display()))? {
        let entry = entry.map_err(|e| format!("cannot list frames folder {}: {e}", folder.display()))?;
        let path = entry.path();
        if !path.is_file() || image_extension(&path).is_none() {
            continue;
        }
        count += 1;
        if count > MAX_FRAMES {
            return Err(format!("the frames folder holds more than {MAX_FRAMES} pictures: move old ones away"));
        }
        let modified = entry.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        let name = entry.file_name().to_string_lossy().to_string();
        let newer = match &best {
            None => true,
            Some((t, n, _)) => (modified, &name) > (*t, n),
        };
        if newer {
            best = Some((modified, name, path));
        }
    }
    Ok(best.map(|(_, _, p)| (p, count)))
}

impl NodeOp for PrintWatchdogNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("print.ai_print_watchdog", "AI Print Watchdog", Stage::Print)
            .output(PortSpec::new("verdict", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("manifest", DataKind::Json))
            .param(ParamSpec::path("ai_root", "AI-3D folder", &[], false).help("Folder holding the Vision install. Blank uses the VERNON_AI3D_ROOT environment variable."))
            .param(ParamSpec::string("wsl_distro", "WSL distribution", "AI-3D").help("Linux environment that holds the model's Python. Blank runs Python directly (Linux hosts)."))
            .param(ParamSpec::path("frames_folder", "Camera pictures folder", &[], false).help("Folder that the printer camera or a time-lapse tool saves pictures into (png, jpg, jpeg, webp or ppm). The newest picture is checked. Sub-folders are ignored."))
            .param(ParamSpec::string("question", "Question", DEFAULT_QUESTION).help("The yes/no question put to the model. An answer that starts with yes means a possible failure, no means it looks fine."))
            .param(ParamSpec::int("max_tokens", "Answer length limit", 64, 16, 256).unit("tokens").help("The answer is cut off after this many tokens (a token is about three quarters of a word)."))
            .param(ParamSpec::int("timeout_s", "Time limit", 900, 60, 7200).unit("s").help("The model process is stopped after this long."))
            .param(ParamSpec::bool("run", "Run model", false).action().help("Checks the newest picture. Without it the node loads an earlier answer for that picture, if one exists."))
            .help("Asks a local vision model whether the newest camera picture of a print looks failed. The verdict is a hint: it can miss a failed print and raise a false alarm, it does not stop or pause the printer, and no gate may use it.")
            .emulates("camera-based print failure detection")
            .spec("docs/specs/ai-local/print-watchdog.md")
            .implemented()
            .external("Vision")
    }

    fn cook(&self, ctx: &CookContext, _ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        print_watchdog(ctx, p)
    }
}

fn idle(rep: Report, frame: &str, manifest: Value) -> Vec<Data> {
    let verdict = json!({"frame": frame, "verdict": VERDICT_NONE, "answer": "", "advisory": true, "label": WATCHDOG_LABEL});
    vec![Data::json(verdict), Data::report(rep), Data::json(manifest)]
}

/// The body of `cook`; the real-model test calls it directly.
pub fn print_watchdog(ctx: &CookContext, p: &Params) -> CookResult<Vec<Data>> {
    let folder = p.str("frames_folder")?.trim().to_string();
    let mut rep = Report::new(WATCHDOG_LABEL);
    if folder.is_empty() {
        rep.note(Severity::Info, "choose the folder the printer camera saves pictures into");
        return Ok(idle(rep, "", json!({"status": "no_folder"})));
    }
    let dir = Path::new(&folder);
    if !dir.is_dir() {
        return Err(ctx.fail(format!("frames folder not found: {folder}")));
    }
    let question = p.str("question")?.trim().to_string();
    if question.is_empty() {
        return Err(ctx.fail("there is no question: type one in the 'Question' parameter"));
    }
    let Some((frame, count)) = newest_frame(dir).map_err(|e| ctx.fail(e))? else {
        rep.note(Severity::Info, "no pictures in the folder yet");
        return Ok(idle(rep, "", json!({"status": "no_frames", "frames_folder": folder})));
    };
    let frame_s = frame.to_string_lossy().to_string();
    rep.metric("ai_watchdog_frames_in_folder", Metric::new(count as f64, "count"));

    // Same model, bridge and result cache as the vision check node.
    let mut vp = Params::from_specs(&vision::VisionCheckNode.info().params);
    vp.set("ai_root", ParamValue::Str(p.str("ai_root")?.to_string()));
    vp.set("wsl_distro", ParamValue::Str(p.str("wsl_distro")?.to_string()));
    vp.set("image", ParamValue::Str(frame_s.clone()));
    vp.set("question", ParamValue::Str(question.clone()));
    vp.set("max_tokens", ParamValue::Int(p.int("max_tokens")?));
    vp.set("timeout_s", ParamValue::Int(p.int("timeout_s")?));
    vp.set("run", ParamValue::Bool(p.bool("run")?));
    let out = vision::vision_check(ctx, &vp)?;
    let (Some(Data::Json(answer)), Some(Data::Json(vm))) = (out.first(), out.get(2)) else {
        return Err(ctx.fail("the vision check returned no answer"));
    };
    if vm["status"] != "complete" {
        rep.note(Severity::Info, "not checked: switch on 'Run model' to check the newest picture");
        return Ok(idle(rep, &frame_s, json!({"status": "not_run", "frame": frame_s, "frames_in_folder": count})));
    }
    let words = answer["answer"].as_str().unwrap_or("").trim().to_string();
    let verdict = verdict_of(&words);
    match verdict {
        VERDICT_FAILURE => rep.note(Severity::Warning, "the model thinks this print may have failed: look at the printer yourself"),
        VERDICT_OK => rep.note(Severity::Info, "the model sees no failure in this picture"),
        _ => rep.note(Severity::Info, "the answer did not start with yes or no, so there is no verdict: read the answer"),
    };
    rep.note(Severity::Info, "this check can miss a failed print and can raise a false alarm; it does not stop or pause the printer");
    let result = json!({"frame": frame_s, "verdict": verdict, "answer": words, "question": question, "advisory": true, "label": WATCHDOG_LABEL});
    let manifest = json!({"status": "complete", "advisory": true, "frame": frame_s, "frames_in_folder": count, "verdict": verdict, "vision": &**vm});
    Ok(vec![Data::json(result), Data::report(rep), Data::json(manifest)])
}

#[cfg(test)]
#[path = "watchdog_tests.rs"]
mod tests;

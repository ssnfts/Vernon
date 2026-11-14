//! `geo.ai_speech_to_text`: speech to text with Whisper large-v3-turbo.
//!
//! The model runs as a separate Python process (`scripts/ai/whisper_bridge.py`). The
//! node keeps the audio copy, the settings and the model's result in a work folder
//! named after a hash of the audio bytes plus the settings, so identical inputs reload
//! an earlier result without starting the model.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use vernon_core::report::{Metric, Severity};
use vernon_core::Report;
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::bridge::{self, check_resources, with_margin, BridgeError, Install};
use crate::fnv1a;

pub const WHISPER_BRIDGE: &str = include_str!("../../../scripts/ai/whisper_bridge.py");
pub const AUDIO_EXTENSIONS: [&str; 5] = ["wav", "flac", "mp3", "ogg", "m4a"];
pub const LANGUAGES: [&str; 9] = ["auto", "en", "es", "fr", "de", "it", "pt", "ar", "zh"];
/// Peak GPU memory and RAM of the real run, see docs/specs/ai-local/speech-in.md.
pub const PEAK_VRAM_GB: f64 = 2.0;
pub const PEAK_RSS_GB: f64 = 3.5;

pub fn register(reg: &mut Registry) {
    reg.add(SpeechToTextNode);
}

pub struct SpeechToTextNode;

/// Folder name key for one audio file and one set of settings.
pub fn work_key(audio: &[u8], language: &str) -> u64 {
    let mut b = audio.to_vec();
    b.extend_from_slice(format!("|whisper-large-v3-turbo|{language}").as_bytes());
    fnv1a(&b)
}

/// Lower-case extension if it is a supported audio format.
pub fn audio_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    AUDIO_EXTENSIONS.contains(&ext.as_str()).then_some(ext)
}

impl NodeOp for SpeechToTextNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.ai_speech_to_text", "AI Text from Speech", Stage::Geometry)
            .output(PortSpec::new("text", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("manifest", DataKind::Json))
            .param(ParamSpec::path("ai_root", "AI-3D folder", &[], false).help("Folder holding the Whisper install. Blank uses the VERNON_AI3D_ROOT environment variable."))
            .param(ParamSpec::string("wsl_distro", "WSL distribution", "AI-3D").help("Linux environment that holds the model's Python. Blank runs Python directly (Linux hosts)."))
            .param(ParamSpec::path("audio", "Audio file", &AUDIO_EXTENSIONS, false).help("A recording of speech: wav, flac, mp3, ogg or m4a."))
            .param(ParamSpec::choice("language", "Language", "auto", &LANGUAGES).help("auto lets the model detect the spoken language. Choosing one forces it, which helps on short or noisy clips."))
            .param(ParamSpec::int("timeout_s", "Time limit", 1800, 60, 14400).unit("s").help("The model process is stopped after this long."))
            .param(ParamSpec::bool("run", "Run model", false).action().help("Starts the model. Without it the node loads an earlier result for identical inputs, if one exists."))
            .help("Turns a speech recording into text with Whisper. The text output is JSON with the full text, language, duration and timed segments. Transcripts can contain mistakes, especially for names, numbers and noisy audio: check them before relying on them.")
            .emulates("speech to text transcription")
            .spec("docs/specs/ai-local/speech-in.md")
            .implemented()
            .external("Whisper")
    }

    fn cook(&self, ctx: &CookContext, _ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let audio = p.str("audio")?.trim().to_string();
        let mut rep = Report::new("AI text from speech");
        if audio.is_empty() {
            rep.note(Severity::Info, "choose an audio file to transcribe");
            return Ok(empty(rep, json!({"status": "no_audio"})));
        }
        let path = Path::new(&audio);
        if !path.is_file() {
            return Err(ctx.fail(format!("audio file not found: {audio}")));
        }
        if audio_extension(path).is_none() {
            return Err(ctx.fail(format!("unsupported audio format: {audio}; use one of {}", AUDIO_EXTENSIONS.join(", "))));
        }
        let bytes = std::fs::read(path).map_err(|e| ctx.fail(format!("cannot read audio {audio}: {e}")))?;
        let language = p.str("language")?.to_string();
        let dir = ctx.work_dir.join(format!("transcript_{:016x}", work_key(&bytes, &language)));
        let result_path = dir.join("result.json");
        if !result_path.is_file() {
            if !p.bool("run")? {
                rep.note(Severity::Info, "not run: switch on 'Run model' to transcribe");
                return Ok(empty(rep, json!({"status": "not_run", "artifact_dir": dir.to_string_lossy()})));
            }
            run_model(ctx, p, &dir, path, &bytes, &language)?;
        }
        let res: Value = serde_json::from_slice(&std::fs::read(&result_path)?).map_err(|e| ctx.fail(format!("unreadable result.json: {e}")))?;
        let text = parse_text(&res).map_err(|e| ctx.fail(e))?;
        let words = text["text"].as_str().unwrap_or("").split_whitespace().count();
        let n_seg = text["segments"].as_array().map_or(0, Vec::len);
        rep.metric("ai_word_count", Metric::new(words as f64, "count"));
        rep.metric("ai_segment_count", Metric::new(n_seg as f64, "count"));
        if let Some(d) = text["duration_s"].as_f64() {
            rep.metric("ai_audio_duration", Metric::new(d, "s"));
        }
        if words == 0 {
            rep.note(Severity::Warning, "the transcript is empty: no speech was recognised (silence, music or an unsupported language?)");
        }
        rep.note(Severity::Info, "machine transcript: it can contain errors, especially names, numbers and noisy audio");
        for (k, unit) in [("peak_vram_GB", "GB"), ("peak_rss_GB", "GB"), ("seconds_load", "s"), ("seconds_transcribe", "s")] {
            if let Some(v) = res.get(k).and_then(Value::as_f64) {
                rep.metric(format!("ai_{}", k.to_lowercase()), Metric::new(v, unit));
            }
        }
        let manifest = json!({
            "status": "complete", "artifact_dir": dir.to_string_lossy(), "model": res.get("model"), "revision": res.get("revision"),
            "device": res.get("device"), "language_requested": language, "language_detected": text["language"],
            "audio": {"path": audio, "sha256": res.get("audio_sha256")}, "words": words, "segments": n_seg,
            "peak_vram_GB": res.get("peak_vram_GB"), "peak_rss_GB": res.get("peak_rss_GB"),
            "seconds_load": res.get("seconds_load"), "seconds_transcribe": res.get("seconds_transcribe"),
        });
        let _ = std::fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap_or_default());
        Ok(vec![Data::json(text), Data::report(rep), Data::json(manifest)])
    }
}

/// The `{text, language, duration_s, segments}` object of a bridge result, validated.
pub fn parse_text(res: &Value) -> Result<Value, String> {
    let t = res.get("text").filter(|t| t.is_object()).ok_or("result.json has no 'text' object")?;
    let text = t.get("text").and_then(Value::as_str).ok_or("result.json text has no 'text' string")?;
    let segments: Vec<Value> = t
        .get("segments")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|s| {
                    json!({
                        "start": s.get("start").and_then(Value::as_f64).unwrap_or(0.0),
                        "end": s.get("end").and_then(Value::as_f64).unwrap_or(0.0),
                        "text": s.get("text").and_then(Value::as_str).unwrap_or("").trim(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "text": text.trim(),
        "language": t.get("language").and_then(Value::as_str).unwrap_or("unknown"),
        "duration_s": t.get("duration_s").and_then(Value::as_f64).unwrap_or(0.0),
        "segments": segments,
    }))
}

fn empty(rep: Report, manifest: Value) -> Vec<Data> {
    vec![Data::json(json!({"text": "", "language": "", "duration_s": 0.0, "segments": []})), Data::report(rep), Data::json(manifest)]
}

fn run_model(ctx: &CookContext, p: &Params, dir: &Path, audio: &Path, bytes: &[u8], language: &str) -> CookResult<()> {
    let install = Install::resolve(p.str("ai_root")?, p.str("wsl_distro")?).map_err(|e| ctx.fail(e.to_string()))?;
    install.require_model_dir("Whisper").map_err(|e| ctx.fail(e.to_string()))?;
    let have = bridge::query_resources(&install).map_err(|e| ctx.fail(e.to_string()))?;
    check_resources(with_margin(PEAK_VRAM_GB, PEAK_RSS_GB), have, "Whisper").map_err(|e| ctx.fail(e.to_string()))?;
    std::fs::create_dir_all(dir)?;
    let ext = audio_extension(audio).unwrap_or_else(|| "wav".into());
    let kept = dir.join(format!("input.{ext}"));
    std::fs::write(&kept, bytes)?;
    let request = request_json(&install, &kept, dir, language);
    let timeout = Duration::from_secs(p.int("timeout_s")?.max(60) as u64);
    bridge::run_job(&install, "Whisper", "whisper_bridge.py", WHISPER_BRIDGE, &request, dir, timeout).map_err(|e| match e {
        BridgeError::Cancelled => CookError::Other(vernon_graph::CANCELLED.into()),
        other => ctx.fail(other.to_string()),
    })?;
    Ok(())
}

pub fn request_json(install: &Install, audio: &Path, out_dir: &Path, language: &str) -> Value {
    json!({"ai_root": install.root.to_string_lossy(), "audio": audio.to_string_lossy(), "out_dir": out_dir.to_string_lossy(), "language": language})
}

#[cfg(test)]
#[path = "transcribe_tests.rs"]
mod tests;

//! `geo.ai_text_to_speech`: text to speech with Kokoro-82M (Apache-2.0).
//!
//! The model runs as a separate process (`scripts/ai/kokoro_bridge.py`) on the CPU
//! and writes one 24 kHz mono WAV. Vernon reads the WAV header itself and reports
//! sample rate, duration and peak level; a silent or empty file is refused.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use vernon_core::report::{Metric, Severity};
use vernon_core::Report;
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::bridge::{self, BridgeError, Install};
use crate::fnv1a;

pub const KOKORO_BRIDGE: &str = include_str!("../../../scripts/ai/kokoro_bridge.py");
pub const VOICES: [&str; 6] = ["af_heart", "af_bella", "af_nicole", "am_michael", "bf_emma", "bm_george"];
/// Below this peak (fraction of full scale) a file counts as silent.
pub const SILENT_PEAK: f64 = 1e-4;

pub fn register(reg: &mut Registry) {
    reg.add(TextToSpeechNode);
}

/// What the Rust side measured in a WAV file.
#[derive(Debug, Clone, PartialEq)]
pub struct WavInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits: u16,
    pub frames: u64,
    pub duration_s: f64,
    /// Largest absolute sample as a fraction of full scale, 0..1.
    pub peak: f64,
}

fn u16le(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32le(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// Parse a RIFF/WAVE file (PCM 8/16/24/32-bit or 32-bit float). Errors say what is wrong.
pub fn parse_wav(b: &[u8]) -> Result<WavInfo, String> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<(usize, usize)> = None;
    let mut o = 12;
    while o + 8 <= b.len() {
        let id = &b[o..o + 4];
        let size = u32le(b, o + 4).ok_or("truncated chunk header")? as usize;
        let body = o + 8;
        if id == b"fmt " {
            if size < 16 || body + 16 > b.len() {
                return Err("truncated fmt chunk".into());
            }
            let tag = u16le(b, body).ok_or("truncated fmt chunk")?;
            let ch = u16le(b, body + 2).ok_or("truncated fmt chunk")?;
            let sr = u32le(b, body + 4).ok_or("truncated fmt chunk")?;
            let bits = u16le(b, body + 14).ok_or("truncated fmt chunk")?;
            let tag = if tag == 0xFFFE && size >= 26 { u16le(b, body + 24).unwrap_or(tag) } else { tag };
            fmt = Some((tag, ch, sr, bits));
        } else if id == b"data" {
            if body + size > b.len() {
                return Err(format!("truncated data chunk: header says {size} bytes, file has {}", b.len() - body));
            }
            data = Some((body, body + size));
            break;
        }
        o = body + size + (size & 1);
    }
    let (tag, ch, sr, bits) = fmt.ok_or("no fmt chunk")?;
    let (ds, de) = data.ok_or("no data chunk")?;
    if ch == 0 || sr == 0 {
        return Err("invalid fmt chunk (zero channels or sample rate)".into());
    }
    let ok = matches!((tag, bits), (1, 8) | (1, 16) | (1, 24) | (1, 32) | (3, 32));
    if !ok {
        return Err(format!("unsupported WAV format tag {tag} with {bits} bits"));
    }
    let bytes = (bits / 8) as usize;
    let frame_bytes = bytes * ch as usize;
    let frames = ((de - ds) / frame_bytes) as u64;
    let mut peak = 0.0f64;
    for s in b[ds..ds + frames as usize * frame_bytes].chunks_exact(bytes) {
        let v = match (tag, bits) {
            (1, 8) => (s[0] as f64 - 128.0) / 128.0,
            (1, 16) => i16::from_le_bytes([s[0], s[1]]) as f64 / 32768.0,
            (1, 24) => (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f64 / 8_388_608.0,
            (1, _) => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f64 / 2_147_483_648.0,
            _ => f32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f64,
        };
        peak = peak.max(v.abs());
    }
    Ok(WavInfo { sample_rate: sr, channels: ch, bits, frames, duration_s: frames as f64 / sr as f64, peak })
}

/// Parse and refuse zero-length or silent audio.
pub fn check_wav(b: &[u8]) -> Result<WavInfo, String> {
    let w = parse_wav(b)?;
    if w.frames == 0 {
        return Err("the audio file has zero length".into());
    }
    if !w.peak.is_finite() || w.peak < SILENT_PEAK {
        return Err("the audio file is silent".into());
    }
    Ok(w)
}

/// Hash of everything that changes the audio.
pub fn input_key(text: &str, voice: &str, speed: f64) -> u64 {
    fnv1a(format!("kokoro-82m|{voice}|{speed:.4}|{text}").as_bytes())
}

pub fn build_request(install: &Install, dir: &Path, text: &str, voice: &str, speed: f64) -> Value {
    json!({"ai_root": install.root.to_string_lossy(), "out_dir": dir.to_string_lossy(), "text": text, "voice": voice, "speed": speed})
}

pub struct TextToSpeechNode;

impl NodeOp for TextToSpeechNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.ai_text_to_speech", "AI Speech from Text", Stage::Geometry)
            .output(PortSpec::new("audio", DataKind::File))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("manifest", DataKind::Json))
            .param(ParamSpec::path("ai_root", "AI-3D folder", &[], false).help("Folder holding the Kokoro install. Blank uses the VERNON_AI3D_ROOT environment variable."))
            .param(ParamSpec::string("wsl_distro", "WSL distribution", "AI-3D").help("Linux environment that holds the model's Python. Blank runs Python directly (Linux hosts)."))
            .param(ParamSpec::string("text", "Text", "").help("The English text to speak. Long text is split into sentences automatically."))
            .param(ParamSpec::choice("voice", "Voice", "af_heart", &VOICES).help("af_ voices are American female, am_ American male, bf_ British female, bm_ British male."))
            .param(ParamSpec::float("speed", "Speed", 1.0, 0.5, 2.0).help("1.0 is normal speaking pace; below 1 is slower, above 1 is faster."))
            .param(ParamSpec::int("timeout_s", "Time limit", 600, 30, 7200).unit("s").help("The model process is stopped after this long."))
            .param(ParamSpec::bool("run", "Run model", false).action().help("Starts the model. Without it the node loads an earlier result for identical inputs, if one exists."))
            .help("Turns text into a 24 kHz mono WAV file with the Kokoro speech model on the CPU. The output file path, its measured length and peak level, and the model revision are reported. A silent or empty result is rejected.")
            .emulates("text to speech synthesis")
            .spec("docs/specs/ai-local/speech-out.md")
            .implemented()
            .external("Kokoro")
    }

    fn cook(&self, ctx: &CookContext, _ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        speak(ctx, p)
    }
}

fn idle(rep: Report, manifest: Value) -> Vec<Data> {
    vec![Data::File(Default::default()), Data::report(rep), Data::json(manifest)]
}

/// The body of `cook`; the real-model test calls it directly.
pub fn speak(ctx: &CookContext, p: &Params) -> CookResult<Vec<Data>> {
    let text = p.str("text")?.trim().to_string();
    if text.is_empty() {
        return Err(ctx.fail("there is no text to speak: type some text in the 'Text' parameter"));
    }
    let (voice, speed) = (p.str("voice")?.to_string(), p.float("speed")?);
    if !VOICES.contains(&voice.as_str()) {
        return Err(ctx.fail(format!("unknown voice '{voice}'")));
    }
    let key = input_key(&text, &voice, speed);
    let dir = ctx.work_dir.join(format!("speech_{key:016x}"));
    let result_path = dir.join("result.json");
    let mut rep = Report::new("AI speech from text");
    if !result_path.is_file() {
        if !p.bool("run")? {
            rep.note(Severity::Info, "not run: switch on 'Run model' to speak this text");
            return Ok(idle(rep, json!({"status": "not_run", "artifact_dir": dir.to_string_lossy()})));
        }
        run_model(ctx, p, &dir, &text, &voice, speed)?;
    }
    let res: Value = serde_json::from_slice(&std::fs::read(&result_path)?).map_err(|e| ctx.fail(format!("unreadable result.json: {e}")))?;
    let name = res.get("wav").and_then(Value::as_str).unwrap_or("speech.wav");
    let wav_path = dir.join(name);
    let bytes = std::fs::read(&wav_path).map_err(|e| ctx.fail(format!("cannot read {}: {e}", wav_path.display())))?;
    let w = check_wav(&bytes).map_err(|e| ctx.fail(format!("{}: {e}", wav_path.display())))?;
    rep.metric("ai_speech_sample_rate", Metric::new(w.sample_rate as f64, "Hz"));
    rep.metric("ai_speech_duration", Metric::new(w.duration_s, "s"));
    rep.metric("ai_speech_peak", Metric::new(w.peak, "fraction of full scale"));
    for (k, unit) in [("seconds_generate", "s"), ("seconds_load", "s"), ("peak_rss_GB", "GB")] {
        if let Some(v) = res.get(k).and_then(Value::as_f64) {
            rep.metric(format!("ai_speech_{}", k.to_lowercase()), Metric::new(v, unit));
        }
    }
    if w.peak > 0.99 {
        rep.note(Severity::Warning, "the audio peaks at full scale and may be clipped");
    }
    rep.note(Severity::Info, format!("spoke {} characters with voice {voice}: {:.2} s at {} Hz, peak {:.3}", text.chars().count(), w.duration_s, w.sample_rate, w.peak));
    let manifest = json!({
        "status": "complete", "artifact_dir": dir.to_string_lossy(), "wav": wav_path.to_string_lossy(),
        "model": res.get("model"), "revision": res.get("revision"), "voice": voice, "speed": speed, "text": text,
        "sample_rate": w.sample_rate, "channels": w.channels, "duration_s": w.duration_s, "peak": w.peak,
        "seconds_generate": res.get("seconds_generate"), "seconds_load": res.get("seconds_load"), "peak_rss_GB": res.get("peak_rss_GB"),
    });
    let _ = std::fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap_or_default());
    Ok(vec![Data::File(wav_path), Data::report(rep), Data::json(manifest)])
}

fn run_model(ctx: &CookContext, p: &Params, dir: &Path, text: &str, voice: &str, speed: f64) -> CookResult<()> {
    let install = Install::resolve(p.str("ai_root")?, p.str("wsl_distro")?).map_err(|e| ctx.fail(e.to_string()))?;
    install.require_model_dir("Kokoro").map_err(|e| ctx.fail(e.to_string()))?;
    let request = build_request(&install, dir, text, voice, speed);
    let timeout = Duration::from_secs(p.int("timeout_s")?.max(30) as u64);
    bridge::run_job(&install, "Kokoro", "kokoro_bridge.py", KOKORO_BRIDGE, &request, dir, timeout).map_err(|e| match e {
        BridgeError::Cancelled => CookError::Other(vernon_graph::CANCELLED.into()),
        other => ctx.fail(other.to_string()),
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "speak_tests.rs"]
mod tests;

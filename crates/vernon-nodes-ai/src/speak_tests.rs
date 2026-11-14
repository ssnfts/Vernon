use std::path::{Path, PathBuf};

use serde_json::json;
use vernon_graph::{CookContext, Data, NodeOp, ParamValue, Params};

use super::*;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vernon_tts_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ctx(dir: &Path) -> CookContext {
    CookContext { node_name: "t".into(), work_dir: dir.to_path_buf(), frame: 0.0 }
}

fn params() -> Params {
    Params::from_specs(&TextToSpeechNode.info().params)
}

/// A 16-bit mono PCM WAV with the given samples.
fn wav16(rate: u32, samples: &[i16]) -> Vec<u8> {
    let n = (samples.len() * 2) as u32;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + n).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&n.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    b
}

fn tone(n: usize, amp: f64) -> Vec<i16> {
    (0..n).map(|i| ((i as f64 * 0.1).sin() * amp * 32767.0) as i16).collect()
}

#[test]
fn node_info_is_complete() {
    let i = TextToSpeechNode.info();
    assert_eq!(i.id, "geo.ai_text_to_speech");
    assert_eq!(i.label, "AI Speech from Text");
    assert!(i.params.iter().all(|p| !p.help.is_empty()), "every param has help");
    assert_eq!(i.outputs.len(), 3);
    assert_eq!(i.outputs[0].kind, DataKind::File);
    assert_eq!(params().str("voice").unwrap(), "af_heart");
    assert_eq!(params().str("wsl_distro").unwrap(), "AI-3D");
}

#[test]
fn wav_header_parses_rate_duration_and_peak() {
    let w = parse_wav(&wav16(24000, &tone(48000, 0.5))).unwrap();
    assert_eq!(w.sample_rate, 24000);
    assert_eq!(w.frames, 48000);
    assert!((w.duration_s - 2.0).abs() < 1e-9);
    assert!((w.peak - 0.5).abs() < 0.01, "peak {}", w.peak);
}

#[test]
fn wav_truncated_and_garbage_are_errors() {
    let good = wav16(24000, &tone(1000, 0.5));
    let e = parse_wav(&good[..good.len() - 100]).unwrap_err();
    assert!(e.contains("truncated data"), "{e}");
    assert!(parse_wav(&good[..30]).is_err());
    assert!(parse_wav(b"hello world, not a wav").unwrap_err().contains("RIFF"));
}

#[test]
fn silent_and_empty_wav_are_refused() {
    assert!(check_wav(&wav16(24000, &vec![0i16; 5000])).unwrap_err().contains("silent"));
    assert!(check_wav(&wav16(24000, &[])).unwrap_err().contains("zero length"));
    assert!(check_wav(&wav16(24000, &tone(5000, 0.3))).is_ok());
}

#[test]
fn hash_is_stable_and_input_sensitive() {
    let a = input_key("hello", "af_heart", 1.0);
    assert_eq!(a, input_key("hello", "af_heart", 1.0));
    assert_eq!(a, fnv1a(b"kokoro-82m|af_heart|1.0000|hello"));
    assert_ne!(a, input_key("hello!", "af_heart", 1.0));
    assert_ne!(a, input_key("hello", "bf_emma", 1.0));
    assert_ne!(a, input_key("hello", "af_heart", 1.1));
}

#[test]
fn request_carries_all_inputs() {
    let install = Install { root: PathBuf::from("E:/AI-3D"), distro: "AI-3D".into() };
    let r = build_request(&install, Path::new("E:/w/speech_1"), "Hi there", "bm_george", 1.25);
    assert_eq!(r["text"], "Hi there");
    assert_eq!(r["voice"], "bm_george");
    assert_eq!(r["speed"], 1.25);
    assert!(r["out_dir"].as_str().unwrap().contains("speech_1"));
    assert!(r["ai_root"].as_str().unwrap().contains("AI-3D"));
}

#[test]
fn empty_text_is_an_error() {
    let dir = tmp("empty");
    let mut p = params();
    p.set("text", ParamValue::Str("   ".into()));
    p.set("run", ParamValue::Bool(true));
    let e = TextToSpeechNode.cook(&ctx(&dir), &[], &p).unwrap_err().to_string();
    assert!(e.contains("no text to speak"), "{e}");
}

#[test]
fn missing_install_is_an_error() {
    let dir = tmp("noinstall");
    let mut p = params();
    p.set("text", ParamValue::Str("Hello".into()));
    p.set("run", ParamValue::Bool(true));
    p.set("ai_root", ParamValue::Str("Z:/no/such/folder".into()));
    let e = TextToSpeechNode.cook(&ctx(&dir), &[], &p).unwrap_err().to_string();
    assert!(e.contains("AI model folder not found"), "{e}");
    // folder exists but Kokoro is not installed under it
    let root = tmp("emptyroot");
    p.set("ai_root", ParamValue::Str(root.to_string_lossy().into()));
    let e = TextToSpeechNode.cook(&ctx(&dir), &[], &p).unwrap_err().to_string();
    assert!(e.contains("Kokoro not installed"), "{e}");
}

#[test]
fn without_run_nothing_happens() {
    let dir = tmp("idle");
    let mut p = params();
    p.set("text", ParamValue::Str("Hello".into()));
    let out = TextToSpeechNode.cook(&ctx(&dir), &[], &p).unwrap();
    let Data::Json(j) = &out[2] else { panic!() };
    assert_eq!(j["status"], "not_run");
}

#[test]
fn cached_result_reloads_without_run_and_reports_measurements() {
    let dir = tmp("cached");
    let mut p = params();
    let text = "Cached sentence.";
    p.set("text", ParamValue::Str(text.into()));
    p.set("voice", ParamValue::Str("bf_emma".into()));
    p.set("speed", ParamValue::Float(1.2));
    let art = dir.join(format!("speech_{:016x}", input_key(text, "bf_emma", 1.2)));
    std::fs::create_dir_all(&art).unwrap();
    std::fs::write(art.join("speech.wav"), wav16(24000, &tone(36000, 0.4))).unwrap();
    std::fs::write(
        art.join("result.json"),
        serde_json::to_vec(&json!({"model": "hexgrad/Kokoro-82M", "revision": "abc123", "wav": "speech.wav", "seconds_generate": 1.5, "seconds_load": 3.0, "peak_rss_GB": 1.1})).unwrap(),
    )
    .unwrap();
    let out = TextToSpeechNode.cook(&ctx(&dir), &[], &p).unwrap();
    let Data::File(f) = &out[0] else { panic!() };
    assert!(f.ends_with("speech.wav"));
    let Data::Report(r) = &out[1] else { panic!() };
    assert_eq!(r.metrics["ai_speech_sample_rate"].value, 24000.0);
    assert!((r.metrics["ai_speech_duration"].value - 1.5).abs() < 1e-9);
    assert!((r.metrics["ai_speech_peak"].value - 0.4).abs() < 0.01);
    let Data::Json(j) = &out[2] else { panic!() };
    assert_eq!(j["status"], "complete");
    assert_eq!(j["revision"], "abc123");
    assert_eq!(j["sample_rate"], 24000);
    assert_eq!(j["voice"], "bf_emma");
}

#[test]
fn cached_silent_file_is_refused() {
    let dir = tmp("cached_silent");
    let mut p = params();
    p.set("text", ParamValue::Str("Quiet".into()));
    let art = dir.join(format!("speech_{:016x}", input_key("Quiet", "af_heart", 1.0)));
    std::fs::create_dir_all(&art).unwrap();
    std::fs::write(art.join("speech.wav"), wav16(24000, &vec![0i16; 4800])).unwrap();
    std::fs::write(art.join("result.json"), b"{\"wav\": \"speech.wav\"}").unwrap();
    let e = TextToSpeechNode.cook(&ctx(&dir), &[], &p).unwrap_err().to_string();
    assert!(e.contains("silent"), "{e}");
}

/// Runs the real Kokoro model. Needs VERNON_AI3D_ROOT and the WSL install.
#[test]
#[ignore = "needs the Kokoro install (VERNON_AI3D_ROOT)"]
fn real_kokoro_speaks() {
    let root = std::env::var("VERNON_AI3D_ROOT").expect("set VERNON_AI3D_ROOT");
    let dir = std::env::var("VERNON_TTS_OUT").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("vernon_tts_real"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut p = params();
    p.set("ai_root", ParamValue::Str(root));
    p.set("text", ParamValue::Str("Vernon speech check: the front splitter is ready to print.".into()));
    p.set("run", ParamValue::Bool(true));
    let t = std::time::Instant::now();
    let out = speak(&ctx(&dir), &p).unwrap();
    let wall = t.elapsed().as_secs_f64();
    let Data::File(f) = &out[0] else { panic!() };
    let Data::Json(j) = &out[2] else { panic!() };
    println!("wav: {}\nmanifest: {}\nwall: {wall:.1} s", f.display(), serde_json::to_string_pretty(&**j).unwrap());
    assert_eq!(j["sample_rate"], 24000);
    let d = j["duration_s"].as_f64().unwrap();
    assert!((2.0..=10.0).contains(&d), "duration {d}");
    assert!(j["peak"].as_f64().unwrap() > 0.05);
}

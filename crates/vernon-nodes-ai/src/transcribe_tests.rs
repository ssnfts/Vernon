use std::path::{Path, PathBuf};

use serde_json::json;
use vernon_core::report::Severity;
use vernon_graph::{CookContext, Data, NodeOp, ParamValue, Params};

use super::*;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vernon_stt_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ctx(dir: &Path) -> CookContext {
    CookContext { node_name: "t".into(), work_dir: dir.to_path_buf(), frame: 0.0 }
}

fn params(audio: &Path) -> Params {
    let mut p = Params::from_specs(&SpeechToTextNode.info().params);
    p.set("audio", ParamValue::Str(audio.to_string_lossy().into()));
    p
}

fn fake_result(text: &str) -> serde_json::Value {
    json!({"model": "openai/whisper-large-v3-turbo", "revision": "abc", "device": "cuda:0", "audio_sha256": "00",
        "text": {"text": text, "language": "en", "duration_s": 11.0,
                 "segments": [{"start": 0.0, "end": 5.5, "text": " hello "}, {"start": 5.5, "end": 11.0, "text": "world"}]},
        "seconds_load": 3.0, "seconds_transcribe": 1.5, "peak_vram_GB": 1.8, "peak_rss_GB": 3.0})
}

fn seed(dir: &Path, audio: &Path, language: &str, result: &serde_json::Value) -> PathBuf {
    let key = work_key(&std::fs::read(audio).unwrap(), language);
    let d = dir.join(format!("transcript_{key:016x}"));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("result.json"), serde_json::to_vec(result).unwrap()).unwrap();
    d
}

#[test]
fn node_info_is_registered_shape() {
    let i = SpeechToTextNode.info();
    assert_eq!(i.id, "geo.ai_speech_to_text");
    assert_eq!(i.outputs.len(), 3);
    assert!(i.params.iter().any(|p| p.name == "run"));
}

#[test]
fn request_is_built_from_install_and_paths() {
    let install = Install { root: PathBuf::from("root"), distro: "AI-3D".into() };
    let r = request_json(&install, Path::new("a/input.flac"), Path::new("w"), "en");
    assert_eq!(r["language"], "en");
    assert_eq!(r["ai_root"], "root");
    assert!(r["audio"].as_str().unwrap().ends_with("input.flac"));
    assert!(r["out_dir"].as_str().is_some());
}

#[test]
fn work_key_is_stable_and_depends_on_audio_and_language() {
    let a = work_key(b"abc", "auto");
    assert_eq!(a, work_key(b"abc", "auto"));
    assert_ne!(a, work_key(b"abc", "en"));
    assert_ne!(a, work_key(b"abd", "auto"));
    assert_eq!(format!("{:016x}", work_key(b"", "auto")).len(), 16);
}

#[test]
fn extensions_are_checked_case_insensitively() {
    assert_eq!(audio_extension(Path::new("x.FLAC")).as_deref(), Some("flac"));
    assert!(audio_extension(Path::new("x.txt")).is_none());
    assert!(audio_extension(Path::new("noext")).is_none());
}

#[test]
fn parse_text_trims_and_fills_defaults() {
    let t = parse_text(&fake_result(" hi there ")).unwrap();
    assert_eq!(t["text"], "hi there");
    assert_eq!(t["segments"][0]["text"], "hello");
    assert_eq!(t["segments"][1]["start"], 5.5);
    assert!(parse_text(&json!({})).is_err());
    assert_eq!(parse_text(&json!({"text": {"text": "x"}})).unwrap()["language"], "unknown");
}

#[test]
fn no_audio_and_not_run_do_nothing() {
    let dir = tmp("idle");
    let n = SpeechToTextNode;
    let out = n.cook(&ctx(&dir), &[], &Params::from_specs(&n.info().params)).unwrap();
    let Data::Json(j) = &out[2] else { panic!() };
    assert_eq!(j["status"], "no_audio");
    let a = dir.join("a.wav");
    std::fs::write(&a, b"RIFF").unwrap();
    let out = n.cook(&ctx(&dir), &[], &params(&a)).unwrap();
    let Data::Json(j) = &out[2] else { panic!() };
    assert_eq!(j["status"], "not_run");
}

#[test]
fn missing_file_and_bad_extension_have_distinct_errors() {
    let dir = tmp("errs");
    let n = SpeechToTextNode;
    let e = n.cook(&ctx(&dir), &[], &params(&dir.join("gone.wav"))).unwrap_err().to_string();
    assert!(e.contains("audio file not found"), "{e}");
    let t = dir.join("notes.txt");
    std::fs::write(&t, b"x").unwrap();
    let e = n.cook(&ctx(&dir), &[], &params(&t)).unwrap_err().to_string();
    assert!(e.contains("unsupported audio format"), "{e}");
}

#[test]
fn run_without_install_fails_clearly() {
    let dir = tmp("noinstall");
    let a = dir.join("a.wav");
    std::fs::write(&a, b"RIFF").unwrap();
    let n = SpeechToTextNode;
    let mut p = params(&a);
    p.set("run", ParamValue::Bool(true));
    p.set("ai_root", ParamValue::Str(dir.join("nope").to_string_lossy().into()));
    let e = n.cook(&ctx(&dir), &[], &p).unwrap_err().to_string();
    assert!(e.contains("AI model folder not found"), "{e}");
    // folder exists but Whisper is not installed in it
    let root = dir.join("root");
    std::fs::create_dir_all(&root).unwrap();
    p.set("ai_root", ParamValue::Str(root.to_string_lossy().into()));
    let e = n.cook(&ctx(&dir), &[], &p).unwrap_err().to_string();
    assert!(e.contains("Whisper not installed"), "{e}");
}

#[test]
fn cached_result_reloads_without_run() {
    let dir = tmp("cache");
    let a = dir.join("a.flac");
    std::fs::write(&a, b"fake audio bytes").unwrap();
    let d = seed(&dir, &a, "auto", &fake_result("ask not what"));
    let n = SpeechToTextNode;
    let out = n.cook(&ctx(&dir), &[], &params(&a)).unwrap();
    let Data::Json(t) = &out[0] else { panic!() };
    assert_eq!(t["text"], "ask not what");
    assert_eq!(t["segments"].as_array().unwrap().len(), 2);
    let Data::Json(m) = &out[2] else { panic!() };
    assert_eq!(m["status"], "complete");
    assert_eq!(m["revision"], "abc");
    assert!(d.join("manifest.json").is_file());
    // a different language setting is a different cache entry
    let mut p = params(&a);
    p.set("language", ParamValue::Str("en".into()));
    let out = n.cook(&ctx(&dir), &[], &p).unwrap();
    let Data::Json(m) = &out[2] else { panic!() };
    assert_eq!(m["status"], "not_run");
}

#[test]
fn empty_transcript_is_a_warning_not_an_error() {
    let dir = tmp("empty");
    let a = dir.join("silence.wav");
    std::fs::write(&a, b"silence").unwrap();
    seed(&dir, &a, "auto", &fake_result("   "));
    let out = SpeechToTextNode.cook(&ctx(&dir), &[], &params(&a)).unwrap();
    let Data::Report(r) = &out[1] else { panic!() };
    assert!(r.notes.iter().any(|(s, m)| *s == Severity::Warning && m.contains("transcript is empty")));
}

#[test]
fn corrupt_result_is_reported() {
    let dir = tmp("corrupt");
    let a = dir.join("a.wav");
    std::fs::write(&a, b"x").unwrap();
    seed(&dir, &a, "auto", &json!({"nothing": 1}));
    let e = SpeechToTextNode.cook(&ctx(&dir), &[], &params(&a)).unwrap_err().to_string();
    assert!(e.contains("no 'text' object"), "{e}");
}

#[test]
#[ignore = "needs the installed Whisper model, a CUDA GPU and WSL (set VERNON_AI3D_ROOT)"]
fn real_whisper_transcribes() {
    let root = std::env::var(crate::bridge::ROOT_ENV).expect("VERNON_AI3D_ROOT");
    let audio = PathBuf::from(&root).join("Whisper/samples/jfk.flac");
    let dir = std::env::var("VERNON_AI_E2E_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("vernon_ai_e2e")).join("whisper");
    std::fs::create_dir_all(&dir).unwrap();
    let n = SpeechToTextNode;
    let mut p = params(&audio);
    p.set("run", ParamValue::Bool(true));
    let t0 = std::time::Instant::now();
    let out = n.cook(&ctx(&dir), &[], &p).unwrap();
    println!("wall {:.1}s", t0.elapsed().as_secs_f64());
    let Data::Json(t) = &out[0] else { panic!() };
    println!("{}", serde_json::to_string_pretty(&**t).unwrap());
    let Data::Json(m) = &out[2] else { panic!() };
    println!("{}", serde_json::to_string_pretty(&**m).unwrap());
    assert_eq!(m["status"], "complete");
    let plain: String = t["text"].as_str().unwrap().to_lowercase().chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect();
    assert!(plain.contains("ask not what your country can do for you"), "{plain}");
}

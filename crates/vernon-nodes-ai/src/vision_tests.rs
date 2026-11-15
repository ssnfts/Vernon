use std::path::{Path, PathBuf};

use serde_json::json;
use vernon_core::report::Severity;
use vernon_graph::{CookContext, Data, NodeOp, ParamValue, Params};

use super::*;
use crate::bridge::{check_resources, with_margin, BridgeError, Resources};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vernon_vis_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ctx(dir: &Path) -> CookContext {
    CookContext { node_name: "t".into(), work_dir: dir.to_path_buf(), frame: 0.0 }
}

fn params(image: &Path) -> Params {
    let mut p = Params::from_specs(&VisionCheckNode.info().params);
    p.set("image", ParamValue::Str(image.to_string_lossy().into()));
    p
}

fn fake_result(answer: &str) -> serde_json::Value {
    json!({"model": "Qwen/Qwen3-VL-4B-Instruct", "revision": "abc", "device": "cuda", "question": DEFAULT_QUESTION,
        "answer": answer, "max_tokens": 256, "image_size": [64, 64], "encoded_size": [64, 64], "new_tokens": 12,
        "seconds_load": 20.0, "seconds_generate": 1.5, "peak_vram_GB": 9.0, "peak_rss_GB": 3.0})
}

fn seed(dir: &Path, image: &Path, question: &str, max_tokens: i64, result: &serde_json::Value) -> PathBuf {
    let key = work_key(&std::fs::read(image).unwrap(), question, max_tokens);
    let d = dir.join(format!("vision_{key:016x}"));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("result.json"), serde_json::to_vec(result).unwrap()).unwrap();
    d
}

#[test]
fn node_info_is_registered_shape() {
    let i = VisionCheckNode.info();
    assert_eq!(i.id, "geo.ai_vision_check");
    assert_eq!(i.label, "AI Vision Check");
    assert_eq!(i.outputs.len(), 3);
    assert_eq!(i.outputs[0].name, "answer");
    assert!(i.params.iter().all(|p| !p.help.is_empty()), "every param has help");
    assert!(i.params.iter().any(|p| p.name == "run"));
    let p = Params::from_specs(&i.params);
    assert_eq!(p.str("wsl_distro").unwrap(), "AI-3D");
    assert_eq!(p.str("question").unwrap(), DEFAULT_QUESTION);
    assert_eq!(p.int("max_tokens").unwrap(), 256);
}

#[test]
fn request_is_built_from_install_and_paths() {
    let install = Install { root: PathBuf::from("root"), distro: "AI-3D".into() };
    let r = request_json(&install, Path::new("a/input.png"), Path::new("w"), "how many?", 64);
    assert_eq!(r["question"], "how many?");
    assert_eq!(r["max_tokens"], 64);
    assert_eq!(r["ai_root"], "root");
    assert!(r["image"].as_str().unwrap().ends_with("input.png"));
    assert!(r["out_dir"].as_str().is_some());
}

#[test]
fn work_key_is_stable_and_depends_on_every_input() {
    let a = work_key(b"img", "q", 256);
    assert_eq!(a, work_key(b"img", "q", 256));
    assert_ne!(a, work_key(b"imh", "q", 256));
    assert_ne!(a, work_key(b"img", "r", 256));
    assert_ne!(a, work_key(b"img", "q", 255));
    assert_eq!(format!("{:016x}", work_key(b"", "", 16)).len(), 16);
}

#[test]
fn extensions_are_checked_case_insensitively() {
    assert_eq!(image_extension(Path::new("x.PNG")).as_deref(), Some("png"));
    assert_eq!(image_extension(Path::new("x.ppm")).as_deref(), Some("ppm"));
    assert!(image_extension(Path::new("x.gif")).is_none());
    assert!(image_extension(Path::new("noext")).is_none());
}

#[test]
fn parse_answer_is_always_advisory() {
    let a = parse_answer(&fake_result("  three squares  ")).unwrap();
    assert_eq!(a["answer"], "three squares");
    assert_eq!(a["advisory"], true);
    assert_eq!(a["label"], ADVISORY_LABEL);
    assert_eq!(a["question"], DEFAULT_QUESTION);
    assert!(parse_answer(&json!({})).is_err());
}

#[test]
fn empty_answer_is_an_error() {
    let e = parse_answer(&fake_result("   ")).unwrap_err();
    assert!(e.contains("empty answer"), "{e}");
}

#[test]
fn no_image_and_not_run_do_nothing_and_stay_advisory() {
    let dir = tmp("idle");
    let n = VisionCheckNode;
    let out = n.cook(&ctx(&dir), &[], &Params::from_specs(&n.info().params)).unwrap();
    let Data::Json(j) = &out[2] else { panic!() };
    assert_eq!(j["status"], "no_image");
    let Data::Json(a) = &out[0] else { panic!() };
    assert_eq!(a["advisory"], true);
    assert_eq!(a["label"], ADVISORY_LABEL);
    let img = dir.join("a.png");
    std::fs::write(&img, b"png").unwrap();
    let out = n.cook(&ctx(&dir), &[], &params(&img)).unwrap();
    let Data::Json(j) = &out[2] else { panic!() };
    assert_eq!(j["status"], "not_run");
    let Data::Json(a) = &out[0] else { panic!() };
    assert_eq!(a["advisory"], true);
    let Data::Report(r) = &out[1] else { panic!() };
    assert_eq!(r.title, ADVISORY_LABEL);
}

#[test]
fn missing_image_bad_extension_and_empty_question_have_distinct_errors() {
    let dir = tmp("errs");
    let n = VisionCheckNode;
    let e = n.cook(&ctx(&dir), &[], &params(&dir.join("gone.png"))).unwrap_err().to_string();
    assert!(e.contains("image not found"), "{e}");
    let t = dir.join("notes.txt");
    std::fs::write(&t, b"x").unwrap();
    let e = n.cook(&ctx(&dir), &[], &params(&t)).unwrap_err().to_string();
    assert!(e.contains("unsupported image format"), "{e}");
    let img = dir.join("a.jpg");
    std::fs::write(&img, b"x").unwrap();
    let mut p = params(&img);
    p.set("question", ParamValue::Str("   ".into()));
    let e = n.cook(&ctx(&dir), &[], &p).unwrap_err().to_string();
    assert!(e.contains("no question"), "{e}");
}

#[test]
fn run_without_install_fails_clearly() {
    let dir = tmp("noinstall");
    let img = dir.join("a.webp");
    std::fs::write(&img, b"x").unwrap();
    let n = VisionCheckNode;
    let mut p = params(&img);
    p.set("run", ParamValue::Bool(true));
    p.set("ai_root", ParamValue::Str(dir.join("nope").to_string_lossy().into()));
    let e = n.cook(&ctx(&dir), &[], &p).unwrap_err().to_string();
    assert!(e.contains("AI model folder not found"), "{e}");
    let root = dir.join("root");
    std::fs::create_dir_all(&root).unwrap();
    p.set("ai_root", ParamValue::Str(root.to_string_lossy().into()));
    let e = n.cook(&ctx(&dir), &[], &p).unwrap_err().to_string();
    assert!(e.contains("Vision not installed"), "{e}");
}

#[test]
fn gpu_memory_model_and_timeout_errors_are_distinct() {
    let have = Resources { vram_free_mib: 2000, vram_total_mib: 16376, ram_available_mib: 60000 };
    let e = check_resources(with_margin(PEAK_VRAM_GB, PEAK_RSS_GB), have, "Vision").unwrap_err().to_string();
    assert!(e.contains("free GPU memory"), "{e}");
    let ok = Resources { vram_free_mib: 14000, ..have };
    assert!(check_resources(with_margin(PEAK_VRAM_GB, PEAK_RSS_GB), ok, "Vision").is_ok());
    let e = BridgeError::Model { code: "generate_failed".into(), message: "boom".into() }.to_string();
    assert!(e.contains("model error (generate_failed)"), "{e}");
    let e = BridgeError::Timeout(900).to_string();
    assert!(e.contains("900 s time limit"), "{e}");
}

#[test]
fn cached_result_reloads_without_run() {
    let dir = tmp("cache");
    let img = dir.join("a.png");
    std::fs::write(&img, b"fake image bytes").unwrap();
    let d = seed(&dir, &img, DEFAULT_QUESTION, 256, &fake_result("A grey box."));
    let n = VisionCheckNode;
    let out = n.cook(&ctx(&dir), &[], &params(&img)).unwrap();
    let Data::Json(a) = &out[0] else { panic!() };
    assert_eq!(a["answer"], "A grey box.");
    assert_eq!(a["advisory"], true);
    assert_eq!(a["label"], ADVISORY_LABEL);
    let Data::Report(r) = &out[1] else { panic!() };
    assert_eq!(r.title, ADVISORY_LABEL, "the first line of the report is the label");
    assert!(r.metrics.keys().all(|k| k.starts_with("ai_")), "{:?}", r.metrics.keys().collect::<Vec<_>>());
    assert!(r.metrics.contains_key("ai_vision_answer_words"));
    assert!(r.notes.iter().all(|(s, _)| *s != Severity::Error), "the node never sets pass or fail");
    assert!(r.metrics.values().all(|m| m.limit.is_none()), "no metric carries a limit");
    let Data::Json(m) = &out[2] else { panic!() };
    assert_eq!(m["status"], "complete");
    assert_eq!(m["revision"], "abc");
    assert_eq!(m["advisory"], true);
    assert!(d.join("manifest.json").is_file());
    // a different question or token limit is a different cache entry
    let mut p = params(&img);
    p.set("question", ParamValue::Str("Is anything missing?".into()));
    let out = n.cook(&ctx(&dir), &[], &p).unwrap();
    let Data::Json(m) = &out[2] else { panic!() };
    assert_eq!(m["status"], "not_run");
    let mut p = params(&img);
    p.set("max_tokens", ParamValue::Int(128));
    let out = n.cook(&ctx(&dir), &[], &p).unwrap();
    let Data::Json(m) = &out[2] else { panic!() };
    assert_eq!(m["status"], "not_run");
}

#[test]
fn empty_cached_answer_is_reported() {
    let dir = tmp("empty");
    let img = dir.join("a.png");
    std::fs::write(&img, b"x").unwrap();
    seed(&dir, &img, DEFAULT_QUESTION, 256, &fake_result(""));
    let e = VisionCheckNode.cook(&ctx(&dir), &[], &params(&img)).unwrap_err().to_string();
    assert!(e.contains("empty answer"), "{e}");
}

#[test]
fn corrupt_result_is_reported() {
    let dir = tmp("corrupt");
    let img = dir.join("a.png");
    std::fs::write(&img, b"x").unwrap();
    seed(&dir, &img, DEFAULT_QUESTION, 256, &json!({"nothing": 1}));
    let e = VisionCheckNode.cook(&ctx(&dir), &[], &params(&img)).unwrap_err().to_string();
    assert!(e.contains("no 'answer' string"), "{e}");
}

/// 512x512 binary PPM: white background, three blue 90 px squares, one red disc of radius 50.
fn shapes_ppm() -> Vec<u8> {
    let (w, h) = (512usize, 512usize);
    let squares = [(40usize, 40usize), (200, 40), (360, 40)];
    let (cx, cy, r) = (256i64, 380i64, 50i64);
    let mut b = format!("P6\n{w} {h}\n255\n").into_bytes();
    for y in 0..h {
        for x in 0..w {
            let blue = squares.iter().any(|&(sx, sy)| x >= sx && x < sx + 90 && y >= sy && y < sy + 90);
            let (dx, dy) = (x as i64 - cx, y as i64 - cy);
            let px = if blue {
                [0u8, 0, 255]
            } else if dx * dx + dy * dy <= r * r {
                [255, 0, 0]
            } else {
                [255, 255, 255]
            };
            b.extend_from_slice(&px);
        }
    }
    b
}

#[test]
fn test_picture_has_the_documented_shapes() {
    let b = shapes_ppm();
    assert_eq!(b.len(), "P6\n512 512\n255\n".len() + 512 * 512 * 3);
    let px = |x: usize, y: usize| {
        let o = "P6\n512 512\n255\n".len() + (y * 512 + x) * 3;
        [b[o], b[o + 1], b[o + 2]]
    };
    assert_eq!(px(0, 0), [255, 255, 255]);
    assert_eq!(px(80, 80), [0, 0, 255]);
    assert_eq!(px(256, 380), [255, 0, 0]);
}

#[test]
#[ignore = "needs the installed Qwen3-VL model, a CUDA GPU and WSL (set VERNON_AI3D_ROOT)"]
fn real_vision_counts_shapes() {
    let root = std::env::var(crate::bridge::ROOT_ENV).expect("VERNON_AI3D_ROOT");
    let dir = std::env::var("VERNON_AI_E2E_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("vernon_ai_e2e"))
        .join("vision");
    std::fs::create_dir_all(&dir).unwrap();
    let image = dir.join("shapes.ppm");
    std::fs::write(&image, shapes_ppm()).unwrap();
    let ask = |question: &str, max_tokens: i64| {
        let mut p = params(&image);
        p.set("ai_root", ParamValue::Str(root.clone()));
        p.set("question", ParamValue::Str(question.into()));
        p.set("max_tokens", ParamValue::Int(max_tokens));
        p.set("timeout_s", ParamValue::Int(1800));
        p.set("run", ParamValue::Bool(true));
        let t0 = std::time::Instant::now();
        let out = VisionCheckNode.cook(&ctx(&dir), &[], &p).unwrap();
        println!("wall {:.1}s", t0.elapsed().as_secs_f64());
        let Data::Json(a) = &out[0] else { panic!() };
        let Data::Json(m) = &out[2] else { panic!() };
        println!("{}\n{}", serde_json::to_string_pretty(&**a).unwrap(), serde_json::to_string_pretty(&**m).unwrap());
        assert_eq!(m["status"], "complete");
        assert_eq!(a["advisory"], true);
        a["answer"].as_str().unwrap().to_lowercase()
    };
    let count = ask("How many blue squares are in this image? Answer with a single number.", 32);
    println!("ANSWER 1: {count}");
    assert!(count.contains('3') || count.contains("three"), "blue squares answer: {count}");
    let colour = ask("What colour is the circle? Answer with one word.", 32);
    println!("ANSWER 2: {colour}");
    assert!(colour.contains("red"), "circle colour answer: {colour}");
}

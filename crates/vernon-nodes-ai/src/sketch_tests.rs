use std::path::{Path, PathBuf};

use serde_json::json;
use vernon_core::report::Severity;
use vernon_graph::{CookContext, Data, NodeOp, ParamValue, Params};

use super::*;
use crate::bridge::{check_resources, with_margin, BridgeError, Resources};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vernon_sk_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ctx(dir: &Path) -> CookContext {
    CookContext { node_name: "t".into(), work_dir: dir.to_path_buf(), frame: 0.0 }
}

fn params(prompt: &str) -> Params {
    let mut p = Params::from_specs(&SketchFromWordsNode.info().params);
    p.set("prompt", ParamValue::Str(prompt.into()));
    p
}

/// Signature plus a header chunk of the given size (no pixel data: the node reads only the header).
fn png_header(w: u32, h: u32) -> Vec<u8> {
    let mut b = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
    b.extend_from_slice(b"IHDR");
    b.extend_from_slice(&w.to_be_bytes());
    b.extend_from_slice(&h.to_be_bytes());
    b.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
    b
}

fn fake_result() -> serde_json::Value {
    json!({"model": "black-forest-labs/FLUX.2-klein-4B", "revision": "abc", "device": "cuda", "offload": true,
        "pipeline": "Flux2KleinPipeline", "prompt": "a red ball", "width": 256, "height": 256, "steps": 4, "seed": 0,
        "seconds_load": 40.0, "seconds_generate": 6.5, "peak_vram_GB": 7.0, "peak_rss_GB": 5.0})
}

fn seed_dir(dir: &Path, prompt: &str, w: i64, h: i64) -> PathBuf {
    let d = dir.join(format!("sketch_{:016x}", work_key(prompt, w, h, DEFAULT_STEPS, 0)));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("result.json"), serde_json::to_vec(&fake_result()).unwrap()).unwrap();
    d
}

fn small(prompt: &str) -> Params {
    let mut p = params(prompt);
    p.set("width", ParamValue::Int(256));
    p.set("height", ParamValue::Int(256));
    p
}

#[test]
fn node_info_has_the_documented_shape() {
    let i = SketchFromWordsNode.info();
    assert_eq!(i.id, "geo.ai_sketch_from_words");
    assert_eq!(i.label, "AI Sketch from Words");
    assert_eq!(i.outputs.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["image", "report", "manifest"]);
    assert!(i.params.iter().all(|p| !p.help.is_empty()), "every param has help");
    let p = Params::from_specs(&i.params);
    assert_eq!(p.str("wsl_distro").unwrap(), "AI-3D");
    assert_eq!(p.int("width").unwrap(), 768);
    assert_eq!(p.int("height").unwrap(), 768);
    assert_eq!(p.int("steps").unwrap(), 4);
    assert_eq!(p.int("seed").unwrap(), 0);
    assert_eq!(p.int("timeout_s").unwrap(), 1800);
    assert!(!p.bool("run").unwrap());
}

#[test]
fn work_key_is_stable_and_depends_on_every_input() {
    let a = work_key("ball", 512, 512, 4, 7);
    assert_eq!(a, work_key("ball", 512, 512, 4, 7));
    assert_ne!(a, work_key("bal", 512, 512, 4, 7));
    assert_ne!(a, work_key("ball", 528, 512, 4, 7));
    assert_ne!(a, work_key("ball", 512, 528, 4, 7));
    assert_ne!(a, work_key("ball", 512, 512, 5, 7));
    assert_ne!(a, work_key("ball", 512, 512, 4, 8));
}

#[test]
fn size_must_be_a_multiple_of_16_in_range() {
    assert!(check_size(768, 256).is_ok());
    assert!(check_size(1024, 1024).is_ok());
    assert!(check_size(250, 256).unwrap_err().contains("width"));
    assert!(check_size(256, 1040).unwrap_err().contains("height"));
    assert!(check_size(256, 264).unwrap_err().contains("multiple of 16"));
    assert!(check_size(240, 256).is_err());
}

#[test]
fn request_is_built_from_install_and_inputs() {
    let install = Install { root: PathBuf::from("root"), distro: "AI-3D".into() };
    let r = request_json(&install, Path::new("w"), "a ball", 512, 256, 4, 7);
    assert_eq!(r["prompt"], "a ball");
    assert_eq!(r["width"], 512);
    assert_eq!(r["height"], 256);
    assert_eq!(r["steps"], 4);
    assert_eq!(r["seed"], 7);
    assert_eq!(r["ai_root"], "root");
}

#[test]
fn png_header_is_checked() {
    assert_eq!(png_size(&png_header(512, 384)).unwrap(), (512, 384));
    assert!(png_size(b"too short").is_err());
    let mut bad = png_header(8, 8);
    bad[1] = b'X';
    assert!(png_size(&bad).unwrap_err().contains("signature"));
    let mut bad = png_header(8, 8);
    bad[12] = b'X';
    assert!(png_size(&bad).unwrap_err().contains("header chunk"));
}

#[test]
fn empty_prompt_and_bad_size_have_distinct_errors() {
    let dir = tmp("errs");
    let n = SketchFromWordsNode;
    let e = n.cook(&ctx(&dir), &[], &params("   ")).unwrap_err().to_string();
    assert!(e.contains("no prompt"), "{e}");
    let mut p = params("a ball");
    p.set("width", ParamValue::Int(250));
    let e = n.cook(&ctx(&dir), &[], &p).unwrap_err().to_string();
    assert!(e.contains("bad picture size"), "{e}");
}

#[test]
fn not_run_does_nothing_and_says_so() {
    let dir = tmp("idle");
    let out = SketchFromWordsNode.cook(&ctx(&dir), &[], &params("a ball")).unwrap();
    let Data::Json(m) = &out[2] else { panic!() };
    assert_eq!(m["status"], "not_run");
    let Data::Report(r) = &out[1] else { panic!() };
    assert_eq!(r.title, SKETCH_LABEL);
    assert!(SKETCH_LABEL.contains("a concept sketch, not a design or a measurement"));
    assert!(matches!(out[0], Data::File(_)));
}

#[test]
fn run_without_install_fails_clearly() {
    let dir = tmp("noinstall");
    let n = SketchFromWordsNode;
    let mut p = params("a ball");
    p.set("run", ParamValue::Bool(true));
    p.set("ai_root", ParamValue::Str(dir.join("nope").to_string_lossy().into()));
    let e = n.cook(&ctx(&dir), &[], &p).unwrap_err().to_string();
    assert!(e.contains("AI model folder not found"), "{e}");
    let root = dir.join("root");
    std::fs::create_dir_all(&root).unwrap();
    p.set("ai_root", ParamValue::Str(root.to_string_lossy().into()));
    let e = n.cook(&ctx(&dir), &[], &p).unwrap_err().to_string();
    assert!(e.contains("Sketch not installed"), "{e}");
}

#[test]
fn gpu_memory_model_and_timeout_errors_are_distinct() {
    let have = Resources { vram_free_mib: 1000, vram_total_mib: 16376, ram_available_mib: 60000 };
    let e = check_resources(with_margin(PEAK_VRAM_GB, PEAK_RSS_GB), have, "Sketch").unwrap_err().to_string();
    assert!(e.contains("free GPU memory"), "{e}");
    let ok = Resources { vram_free_mib: 14000, ..have };
    assert!(check_resources(with_margin(PEAK_VRAM_GB, PEAK_RSS_GB), ok, "Sketch").is_ok());
    let e = BridgeError::Model { code: "generate_failed".into(), message: "boom".into() }.to_string();
    assert!(e.contains("model error (generate_failed)"), "{e}");
    let e = BridgeError::Timeout(900).to_string();
    assert!(e.contains("900 s time limit"), "{e}");
}

#[test]
fn cached_result_reloads_without_run() {
    let dir = tmp("cache");
    let d = seed_dir(&dir, "a red ball", 256, 256);
    std::fs::write(d.join("sketch.png"), png_header(256, 256)).unwrap();
    let out = SketchFromWordsNode.cook(&ctx(&dir), &[], &small("a red ball")).unwrap();
    let Data::File(f) = &out[0] else { panic!() };
    assert_eq!(f, &d.join("sketch.png"));
    let Data::Report(r) = &out[1] else { panic!() };
    assert_eq!(r.title, SKETCH_LABEL, "the first line of the report is the label");
    assert!(r.metrics.keys().all(|k| k.starts_with("ai_")));
    assert!(r.metrics.values().all(|m| m.limit.is_none()));
    assert!(r.notes.iter().all(|(s, _)| *s != Severity::Error));
    let Data::Json(m) = &out[2] else { panic!() };
    assert_eq!(m["status"], "complete");
    assert_eq!(m["revision"], "abc");
    assert_eq!(m["offload"], true);
    assert_eq!(m["inputs"]["prompt"], "a red ball");
    assert!(d.join("manifest.json").is_file());
    // another prompt, size, steps or seed is a different cache entry
    let mut p = small("a red cube");
    let out = SketchFromWordsNode.cook(&ctx(&dir), &[], &p).unwrap();
    let Data::Json(m) = &out[2] else { panic!() };
    assert_eq!(m["status"], "not_run");
    p = small("a red ball");
    p.set("seed", ParamValue::Int(1));
    let out = SketchFromWordsNode.cook(&ctx(&dir), &[], &p).unwrap();
    let Data::Json(m) = &out[2] else { panic!() };
    assert_eq!(m["status"], "not_run");
}

#[test]
fn missing_unreadable_and_wrong_size_pictures_are_distinct_errors() {
    let dir = tmp("badpng");
    let d = seed_dir(&dir, "a red ball", 256, 256);
    let n = SketchFromWordsNode;
    let e = n.cook(&ctx(&dir), &[], &small("a red ball")).unwrap_err().to_string();
    assert!(e.contains("missing or unreadable"), "{e}");
    std::fs::write(d.join("sketch.png"), b"this is not a picture at all").unwrap();
    let e = n.cook(&ctx(&dir), &[], &small("a red ball")).unwrap_err().to_string();
    assert!(e.contains("is unreadable") && e.contains("signature"), "{e}");
    std::fs::write(d.join("sketch.png"), png_header(512, 256)).unwrap();
    let e = n.cook(&ctx(&dir), &[], &small("a red ball")).unwrap_err().to_string();
    assert!(e.contains("512x256 but 256x256"), "{e}");
}

#[test]
fn corrupt_result_is_reported() {
    let dir = tmp("corrupt");
    let d = seed_dir(&dir, "a red ball", 256, 256);
    std::fs::write(d.join("result.json"), b"{not json").unwrap();
    let e = SketchFromWordsNode.cook(&ctx(&dir), &[], &small("a red ball")).unwrap_err().to_string();
    assert!(e.contains("unreadable result.json"), "{e}");
}

/// Mean colour of the square with corner (x0, y0) and side `side`.
fn mean_square(rgb: &[u8], width: usize, x0: usize, y0: usize, side: usize) -> [f64; 3] {
    let mut s = [0.0f64; 3];
    for y in y0..y0 + side {
        for x in x0..x0 + side {
            for (c, v) in s.iter_mut().enumerate() {
                *v += f64::from(rgb[(y * width + x) * 3 + c]);
            }
        }
    }
    let n = (side * side) as f64;
    [s[0] / n, s[1] / n, s[2] / n]
}

#[test]
fn mean_square_averages_the_right_pixels() {
    let mut rgb = vec![0u8; 4 * 4 * 3];
    for y in 0..2 {
        for x in 0..2 {
            rgb[(y * 4 + x) * 3] = 200;
        }
    }
    assert_eq!(mean_square(&rgb, 4, 0, 0, 2), [200.0, 0.0, 0.0]);
    assert_eq!(mean_square(&rgb, 4, 2, 2, 2), [0.0, 0.0, 0.0]);
}

#[test]
#[ignore = "needs the installed FLUX.2-klein-4B model, a CUDA GPU and WSL (set VERNON_AI3D_ROOT)"]
fn real_sketch_draws_red_ball() {
    let root = std::env::var(crate::bridge::ROOT_ENV).expect("VERNON_AI3D_ROOT");
    let dir = std::env::var("VERNON_AI_E2E_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("vernon_ai_e2e"))
        .join("sketch");
    std::fs::create_dir_all(&dir).unwrap();
    let timeout: i64 = std::env::var("VERNON_AI_E2E_TIMEOUT_S").ok().and_then(|v| v.parse().ok()).unwrap_or(1800);
    let mut p = params("a single bright red ball centred on a plain white background, studio photo");
    p.set("ai_root", ParamValue::Str(root));
    p.set("width", ParamValue::Int(512));
    p.set("height", ParamValue::Int(512));
    p.set("seed", ParamValue::Int(7));
    p.set("timeout_s", ParamValue::Int(timeout));
    p.set("run", ParamValue::Bool(true));
    let t0 = std::time::Instant::now();
    let out = SketchFromWordsNode.cook(&ctx(&dir), &[], &p).unwrap();
    println!("wall {:.1}s", t0.elapsed().as_secs_f64());
    let Data::File(png) = &out[0] else { panic!() };
    let Data::Json(m) = &out[2] else { panic!() };
    println!("{}", serde_json::to_string_pretty(&**m).unwrap());
    assert_eq!(m["status"], "complete");
    check_png(png, 512, 512).unwrap();
    // the bridge also writes the same picture as raw 8-bit RGB, so no PNG decoder is needed here
    let rgb = std::fs::read(png.with_file_name("sketch.rgb")).unwrap();
    let (w, h) = (512usize, 512usize);
    assert_eq!(rgb.len(), w * h * 3, "raw picture size");
    let n = (w * h) as f64;
    let mut mean = [0.0f64; 3];
    for px in rgb.chunks(3) {
        for c in 0..3 {
            mean[c] += f64::from(px[c]);
        }
    }
    mean.iter_mut().for_each(|v| *v /= n);
    let mut sd = [0.0f64; 3];
    for px in rgb.chunks(3) {
        for c in 0..3 {
            sd[c] += (f64::from(px[c]) - mean[c]).powi(2);
        }
    }
    sd.iter_mut().for_each(|v| *v = (*v / n).sqrt());
    let side = (w as f64 * 0.2).round() as usize;
    let centre = mean_square(&rgb, w, (w - side) / 2, (h - side) / 2, side);
    let cs = (w as f64 * 0.1).round() as usize;
    let corners = [(0, 0), (w - cs, 0), (0, h - cs), (w - cs, h - cs)].map(|(x, y)| mean_square(&rgb, w, x, y, cs));
    println!("channel std dev {sd:?}\ncentre 20% mean RGB {centre:?}\ncorner 10% mean RGB {corners:?}");
    assert!(sd.iter().any(|&s| s > 10.0), "the picture is flat: std dev {sd:?}");
    assert!(centre[0] >= 1.5 * centre[1] && centre[0] >= 1.5 * centre[2], "centre is not clearly red: {centre:?}");
    for c in corners {
        assert!(c.iter().all(|&v| v > 170.0), "a corner is not light: {c:?}");
    }
}

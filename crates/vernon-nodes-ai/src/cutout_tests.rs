use std::path::{Path, PathBuf};

use serde_json::json;
use vernon_graph::{CookContext, Data, NodeOp, ParamValue, Params};

use super::*;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vernon_cutout_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ctx(dir: &Path) -> CookContext {
    CookContext { node_name: "t".into(), work_dir: dir.to_path_buf(), frame: 0.0 }
}

fn params() -> Params {
    Params::from_specs(&PhotoCutoutNode.info().params)
}

fn with_image(p: &mut Params, path: &Path) {
    p.set("image", ParamValue::Str(path.to_string_lossy().into()));
}

fn cook_err(dir: &Path, p: &Params) -> String {
    PhotoCutoutNode.cook(&ctx(dir), &[], p).unwrap_err().to_string()
}

/// A binary PPM: mid-grey with mild deterministic variation and a red disc; also the true disc mask.
fn disc_ppm(w: usize, h: usize, cx: f64, cy: f64, r: f64) -> (Vec<u8>, Vec<u8>) {
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    let mut truth = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let inside = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt() <= r;
            let v = ((x * 7 + y * 13 + x * y) % 17) as i32 - 8;
            let px = if inside { [200, 30, 30] } else { [128 + v, 128 + v, 128 + v] };
            ppm.extend(px.iter().map(|&c| c as u8));
            if inside {
                truth[y * w + x] = 255;
            }
        }
    }
    (ppm, truth)
}

#[test]
fn node_info_is_complete() {
    let i = PhotoCutoutNode.info();
    assert_eq!(i.id, "capture.ai_photo_cutout");
    assert_eq!(i.label, "AI Photo Cut-out");
    assert!(i.params.iter().all(|p| !p.help.is_empty()), "every param has help");
    let kinds: Vec<_> = i.outputs.iter().map(|o| o.kind).collect();
    assert_eq!(kinds, [DataKind::File, DataKind::Json, DataKind::Report, DataKind::Json]);
    let p = params();
    assert_eq!(p.str("wsl_distro").unwrap(), "AI-3D");
    assert_eq!(p.float("point_x").unwrap(), 0.5);
    assert_eq!(p.float("point_y").unwrap(), 0.5);
    assert!(!p.bool("run").unwrap());
}

#[test]
fn mask_statistics_from_raw_bytes() {
    let (w, h) = (10, 8);
    let mut m = vec![0u8; w * h];
    for y in 2..5 {
        for x in 3..7 {
            m[y * w + x] = 255;
        }
    }
    let s = mask_stats(&m, w, h).unwrap();
    assert_eq!(s.bbox, [3, 2, 6, 4]);
    assert_eq!(s.pixels, 12);
    assert!((s.area_fraction - 12.0 / 80.0).abs() < 1e-12);
}

#[test]
fn empty_full_and_wrong_length_masks_are_refused() {
    let e = mask_stats(&[0u8; 100], 10, 10).unwrap_err();
    assert!(e.contains("empty"), "{e}");
    let e = mask_stats(&[255u8; 100], 10, 10).unwrap_err();
    assert!(e.contains("covers 100.0%"), "{e}");
    let e = mask_stats(&[255u8; 99], 10, 10).unwrap_err();
    assert!(e.contains("99 bytes") && e.contains("100 pixels"), "{e}");
    // just under the limit passes
    let mut m = vec![255u8; 10000];
    for v in m.iter_mut().take(150) {
        *v = 0;
    }
    assert!(mask_stats(&m, 100, 100).is_ok());
}

#[test]
fn hash_is_stable_and_input_sensitive() {
    let a = input_key(b"pixels", 0.5, 0.25);
    assert_eq!(a, input_key(b"pixels", 0.5, 0.25));
    assert_eq!(a, fnv1a(b"sam2.1-hiera-large|0.5000|0.2500|pixels"));
    assert_ne!(a, input_key(b"pixelz", 0.5, 0.25));
    assert_ne!(a, input_key(b"pixels", 0.51, 0.25));
    assert_ne!(a, input_key(b"pixels", 0.5, 0.26));
}

#[test]
fn request_carries_all_inputs() {
    let install = Install { root: PathBuf::from("E:/AI-3D"), distro: "AI-3D".into() };
    let r = build_request(&install, Path::new("E:/w/cutout_1"), Path::new("E:/w/cutout_1/input.png"), 0.3, 0.7);
    assert_eq!(r["point_x"], 0.3);
    assert_eq!(r["point_y"], 0.7);
    assert!(r["image"].as_str().unwrap().ends_with("input.png"));
    assert!(r["out_dir"].as_str().unwrap().contains("cutout_1"));
    assert!(r["ai_root"].as_str().unwrap().contains("AI-3D"));
}

#[test]
fn missing_image_and_unsupported_extension_are_errors() {
    let dir = tmp("img");
    let mut p = params();
    p.set("run", ParamValue::Bool(true));
    assert!(cook_err(&dir, &p).contains("no photo"));
    with_image(&mut p, &dir.join("nope.png"));
    assert!(cook_err(&dir, &p).contains("photo not found"));
    let bmp = dir.join("a.bmp");
    std::fs::write(&bmp, b"x").unwrap();
    with_image(&mut p, &bmp);
    let e = cook_err(&dir, &p);
    assert!(e.contains("unsupported photo type"), "{e}");
}

#[test]
fn point_outside_the_picture_is_an_error() {
    let dir = tmp("pt");
    let img = dir.join("a.ppm");
    std::fs::write(&img, disc_ppm(8, 8, 4.0, 4.0, 2.0).0).unwrap();
    let mut p = params();
    with_image(&mut p, &img);
    for (x, y) in [(1.5, 0.5), (0.5, -0.1), (f64::NAN, 0.5)] {
        p.set("point_x", ParamValue::Float(x));
        p.set("point_y", ParamValue::Float(y));
        let e = cook_err(&dir, &p);
        assert!(e.contains("outside the picture"), "{x},{y}: {e}");
    }
}

#[test]
fn missing_install_is_an_error() {
    let dir = tmp("noinstall");
    let img = dir.join("a.ppm");
    std::fs::write(&img, disc_ppm(8, 8, 4.0, 4.0, 2.0).0).unwrap();
    let mut p = params();
    with_image(&mut p, &img);
    p.set("run", ParamValue::Bool(true));
    p.set("ai_root", ParamValue::Str("Z:/no/such/folder".into()));
    assert!(cook_err(&dir, &p).contains("AI model folder not found"));
    let root = tmp("emptyroot");
    p.set("ai_root", ParamValue::Str(root.to_string_lossy().into()));
    assert!(cook_err(&dir, &p).contains("SAM2 not installed"));
}

#[test]
fn model_error_and_timeout_have_distinct_messages() {
    let m = bridge_failure(&BridgeError::Model { code: "segmentation_failed".into(), message: "boom".into() });
    assert!(m.contains("model error") && m.contains("segmentation_failed") && m.contains("boom"), "{m}");
    let t = bridge_failure(&BridgeError::Timeout(90));
    assert!(t.contains("90 s time limit"), "{t}");
    assert_ne!(m, t);
}

#[test]
fn without_run_nothing_happens() {
    let dir = tmp("idle");
    let img = dir.join("a.ppm");
    std::fs::write(&img, disc_ppm(8, 8, 4.0, 4.0, 2.0).0).unwrap();
    let mut p = params();
    with_image(&mut p, &img);
    let out = PhotoCutoutNode.cook(&ctx(&dir), &[], &p).unwrap();
    assert_eq!(out.len(), 4);
    let Data::Json(j) = &out[3] else { panic!() };
    assert_eq!(j["status"], "not_run");
}

/// Writes a fake finished result folder for the given image and point.
fn fake_result(dir: &Path, img: &Path, x: f64, y: f64, mask: &[u8], w: usize, h: usize) -> PathBuf {
    let art = dir.join(format!("cutout_{:016x}", input_key(&std::fs::read(img).unwrap(), x, y)));
    std::fs::create_dir_all(&art).unwrap();
    std::fs::write(art.join("mask.u8"), mask).unwrap();
    std::fs::write(art.join("cutout.png"), b"png").unwrap();
    std::fs::write(
        art.join("result.json"),
        serde_json::to_vec(&json!({"model": "facebook/sam2.1-hiera-large", "revision": "rev1", "library": "transformers Sam2Model", "device": "cuda",
            "width": w, "height": h, "score": 0.93, "cutout": "cutout.png", "mask": "mask.u8",
            "seconds_load": 4.0, "seconds_segment": 0.5, "peak_vram_GB": 1.2, "peak_rss_GB": 3.0}))
        .unwrap(),
    )
    .unwrap();
    art
}

#[test]
fn cached_result_reloads_without_run_and_recomputes_statistics() {
    let dir = tmp("cached");
    let img = dir.join("a.ppm");
    let (ppm, truth) = disc_ppm(64, 64, 32.0, 32.0, 10.0);
    std::fs::write(&img, ppm).unwrap();
    fake_result(&dir, &img, 0.5, 0.5, &truth, 64, 64);
    let mut p = params();
    with_image(&mut p, &img);
    let out = PhotoCutoutNode.cook(&ctx(&dir), &[], &p).unwrap();
    let Data::File(f) = &out[0] else { panic!() };
    assert!(f.ends_with("cutout.png"));
    let Data::Json(m) = &out[1] else { panic!() };
    assert_eq!(m["width"], 64);
    assert_eq!(m["bbox"], json!([22, 22, 42, 42]));
    assert_eq!(m["score"], 0.93);
    let frac = m["area_fraction"].as_f64().unwrap();
    assert!((frac - truth.iter().filter(|&&v| v != 0).count() as f64 / 4096.0).abs() < 1e-12);
    let Data::Report(r) = &out[2] else { panic!() };
    assert!((r.metrics["ai_cutout_area_fraction"].value - frac).abs() < 1e-12);
    let Data::Json(j) = &out[3] else { panic!() };
    assert_eq!(j["status"], "complete");
    assert_eq!(j["revision"], "rev1");
    // a different click point is a different work folder, so nothing is loaded
    p.set("point_x", ParamValue::Float(0.4));
    let out = PhotoCutoutNode.cook(&ctx(&dir), &[], &p).unwrap();
    let Data::Json(j) = &out[3] else { panic!() };
    assert_eq!(j["status"], "not_run");
}

#[test]
fn cached_bad_masks_are_refused() {
    let dir = tmp("cached_bad");
    let img = dir.join("a.ppm");
    std::fs::write(&img, disc_ppm(16, 16, 8.0, 8.0, 4.0).0).unwrap();
    let mut p = params();
    with_image(&mut p, &img);
    for (mask, want) in [(vec![0u8; 256], "empty"), (vec![255u8; 256], "covers"), (vec![255u8; 255], "255 bytes")] {
        fake_result(&dir, &img, 0.5, 0.5, &mask, 16, 16);
        let e = cook_err(&dir, &p);
        assert!(e.contains(want), "{want}: {e}");
    }
}

/// Runs the real SAM 2.1 model. Needs VERNON_AI3D_ROOT and the WSL install.
#[test]
#[ignore = "needs the SAM2 install (VERNON_AI3D_ROOT)"]
fn real_sam2_cuts_out_disc() {
    let root = std::env::var("VERNON_AI3D_ROOT").expect("set VERNON_AI3D_ROOT");
    let dir = std::env::var("VERNON_AI_E2E_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("vernon_cutout_real"));
    std::fs::create_dir_all(&dir).unwrap();
    let (ppm, truth) = disc_ppm(512, 512, 256.0, 256.0, 120.0);
    let img = dir.join("disc.ppm");
    std::fs::write(&img, ppm).unwrap();
    let mut p = params();
    p.set("ai_root", ParamValue::Str(root));
    with_image(&mut p, &img);
    p.set("run", ParamValue::Bool(true));
    // A cold first load on a busy PC with a slow weights drive can take far longer than the default.
    if let Some(t) = std::env::var("VERNON_AI_E2E_TIMEOUT_S").ok().and_then(|v| v.parse::<i64>().ok()) {
        p.set("timeout_s", ParamValue::Int(t));
    }
    let t = std::time::Instant::now();
    let out = cut_out(&ctx(&dir), &p).unwrap();
    let wall = t.elapsed().as_secs_f64();
    let Data::File(f) = &out[0] else { panic!() };
    let Data::Json(j) = &out[3] else { panic!() };
    println!("png: {}\nmanifest: {}\nwall: {wall:.1} s", f.display(), serde_json::to_string_pretty(&**j).unwrap());
    assert!(f.is_file(), "PNG missing");
    let mask = std::fs::read(j["mask_file"].as_str().unwrap()).unwrap();
    assert_eq!(mask.len(), truth.len());
    let (mut inter, mut union) = (0usize, 0usize);
    for (m, t) in mask.iter().zip(&truth) {
        let (a, b) = (*m != 0, *t != 0);
        inter += (a && b) as usize;
        union += (a || b) as usize;
    }
    let iou = inter as f64 / union as f64;
    println!("IoU {iou:.4}");
    assert!(iou >= 0.90, "IoU {iou:.4} is below 0.90");
}

/// Overlap of a model mask (0 / non-zero) with a reference (0 background, 1 object,
/// 2 not scored), over the scored pixels only.
fn scored_iou(mask: &[u8], truth: &[u8]) -> f64 {
    let (mut inter, mut union) = (0usize, 0usize);
    for (m, t) in mask.iter().zip(truth) {
        if *t == 2 {
            continue;
        }
        let (a, b) = (*m != 0, *t == 1);
        inter += (a && b) as usize;
        union += (a || b) as usize;
    }
    inter as f64 / union.max(1) as f64
}

#[test]
fn scored_iou_skips_unscored_pixels() {
    // object 2 px, one of them hit; one extra px marked; one unscored px marked (ignored)
    let truth = [1, 1, 0, 0, 2];
    let mask = [255, 0, 255, 0, 255];
    assert!((scored_iou(&mask, &truth) - 1.0 / 3.0).abs() < 1e-12);
    assert_eq!(scored_iou(&[0; 3], &[0; 3]), 0.0);
}

/// Runs the real SAM 2.1 model on real photos with hand-drawn reference outlines
/// (Oxford-IIIT Pet test photos; outlines CC BY-SA 4.0). VERNON_CUTOUT_PHOTOS_DIR holds
/// the photos, `<name>_truth.u8` (width x height bytes: 0 background, 1 pet, 2 the
/// outline's uncertain border, not scored) and `cases.json` with each photo's click
/// point, made beforehand by one fixed rule: the pet pixel farthest from its edge.
/// Pass bar, set before the first run: at least 9 of the 11 photos reach an overlap of
/// 0.75, and the median overlap is at least 0.85.
#[test]
#[ignore = "needs the SAM2 install (VERNON_AI3D_ROOT) and VERNON_CUTOUT_PHOTOS_DIR"]
fn real_sam2_on_pet_photos() {
    let root = std::env::var("VERNON_AI3D_ROOT").expect("set VERNON_AI3D_ROOT");
    let photos = PathBuf::from(std::env::var("VERNON_CUTOUT_PHOTOS_DIR").expect("set VERNON_CUTOUT_PHOTOS_DIR"));
    let dir = std::env::var("VERNON_AI_E2E_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("vernon_cutout_photos"));
    std::fs::create_dir_all(&dir).unwrap();
    let cases: serde_json::Value = serde_json::from_slice(&std::fs::read(photos.join("cases.json")).unwrap()).unwrap();
    let cases = cases.as_array().unwrap();
    assert_eq!(cases.len(), 11, "the bar is set for 11 photos");
    let mut ious = Vec::new();
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let truth = std::fs::read(photos.join(c["truth"].as_str().unwrap())).unwrap();
        let mut p = params();
        p.set("ai_root", ParamValue::Str(root.clone()));
        with_image(&mut p, &photos.join(c["image"].as_str().unwrap()));
        p.set("point_x", ParamValue::Float(c["point_x"].as_f64().unwrap()));
        p.set("point_y", ParamValue::Float(c["point_y"].as_f64().unwrap()));
        p.set("run", ParamValue::Bool(true));
        if let Some(t) = std::env::var("VERNON_AI_E2E_TIMEOUT_S").ok().and_then(|v| v.parse::<i64>().ok()) {
            p.set("timeout_s", ParamValue::Int(t));
        }
        let t = std::time::Instant::now();
        let out = cut_out(&ctx(&dir), &p).unwrap();
        let wall = t.elapsed().as_secs_f64();
        let Data::Json(j) = &out[3] else { panic!() };
        let mask = std::fs::read(j["mask_file"].as_str().unwrap()).unwrap();
        assert_eq!(mask.len(), truth.len(), "{name}: mask size");
        let iou = scored_iou(&mask, &truth);
        println!(
            "PHOTO {name} | {} | iou {iou:.3} | pet {:.3} | model score {} | wall {wall:.0}s | load {} | segment {} | vram {}",
            c["original"], c["pet_fraction"], j["mask"]["score"], j["seconds_load"], j["seconds_segment"], j["peak_vram_GB"]
        );
        ious.push(iou);
    }
    let mut sorted = ious.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[5];
    let good = ious.iter().filter(|&&v| v >= 0.75).count();
    println!("SUMMARY photos {} | iou >= 0.75: {good} | median {median:.3} | min {:.3} | max {:.3}", ious.len(), sorted[0], sorted[10]);
    assert!(good >= 9, "only {good} of 11 photos reached 0.75");
    assert!(median >= 0.85, "median overlap {median:.3} is below 0.85");
}

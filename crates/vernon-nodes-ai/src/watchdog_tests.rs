use std::path::{Path, PathBuf};

use serde_json::json;
use vernon_core::report::Severity;
use vernon_graph::{CookContext, Data, NodeOp, ParamValue, Params};

use super::*;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vernon_wd_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ctx(dir: &Path) -> CookContext {
    CookContext { node_name: "t".into(), work_dir: dir.to_path_buf(), frame: 0.0 }
}

fn params(folder: &Path) -> Params {
    let mut p = Params::from_specs(&PrintWatchdogNode.info().params);
    p.set("frames_folder", ParamValue::Str(folder.to_string_lossy().into()));
    p
}

/// A 256 x 256 white picture with one filled circle of the given colour.
fn circle_ppm(rgb: [u8; 3]) -> Vec<u8> {
    let n = 256i64;
    let mut b = format!("P6\n{n} {n}\n255\n").into_bytes();
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x - n / 2, y - n / 2);
            b.extend_from_slice(if dx * dx + dy * dy <= 80 * 80 { &rgb } else { &[255, 255, 255] });
        }
    }
    b
}

/// Puts a finished vision result for `frame` into the work folder, as the model would.
fn seed(work: &Path, frame: &Path, question: &str, max_tokens: i64, answer: &str) {
    let key = vision::work_key(&std::fs::read(frame).unwrap(), question, max_tokens);
    let d = work.join(format!("vision_{key:016x}"));
    std::fs::create_dir_all(&d).unwrap();
    let res = json!({"model": "m", "revision": "r", "device": "cuda", "question": question, "answer": answer, "new_tokens": 9});
    std::fs::write(d.join("result.json"), serde_json::to_vec(&res).unwrap()).unwrap();
}

fn outputs(out: &[Data]) -> (serde_json::Value, vernon_core::Report, serde_json::Value) {
    let (Data::Json(v), Data::Report(r), Data::Json(m)) = (&out[0], &out[1], &out[2]) else { panic!("unexpected outputs") };
    ((**v).clone(), (**r).clone(), (**m).clone())
}

#[test]
fn node_info_is_registered_shape() {
    let i = PrintWatchdogNode.info();
    assert_eq!(i.id, "print.ai_print_watchdog");
    assert_eq!(i.label, "AI Print Watchdog");
    assert_eq!(i.outputs.len(), 3);
    assert_eq!(i.outputs[0].name, "verdict");
    assert!(i.params.iter().all(|p| !p.help.is_empty()), "every param has help");
    let p = Params::from_specs(&i.params);
    assert_eq!(p.str("question").unwrap(), DEFAULT_QUESTION);
    assert_eq!(p.int("max_tokens").unwrap(), 64);
    assert!(!p.bool("run").unwrap());
}

#[test]
fn first_word_decides_the_verdict() {
    assert_eq!(verdict_of("YES. Loose strands cover the bed."), VERDICT_FAILURE);
    assert_eq!(verdict_of("  **Yes**, the part came off."), VERDICT_FAILURE);
    assert_eq!(verdict_of("No, the print looks intact."), VERDICT_OK);
    assert_eq!(verdict_of("no"), VERDICT_OK);
    assert_eq!(verdict_of("Not sure, the picture is dark."), VERDICT_UNCLEAR);
    assert_eq!(verdict_of("The print looks fine. No failure."), VERDICT_UNCLEAR);
    assert_eq!(verdict_of(""), VERDICT_UNCLEAR);
}

#[test]
fn newest_frame_ignores_other_files_and_subfolders() {
    let d = tmp("newest");
    assert_eq!(newest_frame(&d).unwrap(), None);
    std::fs::write(d.join("notes.txt"), b"x").unwrap();
    std::fs::create_dir_all(d.join("old")).unwrap();
    std::fs::write(d.join("old").join("frame_9999.png"), b"x").unwrap();
    assert_eq!(newest_frame(&d).unwrap(), None);
    std::fs::write(d.join("frame_0001.ppm"), b"a").unwrap();
    std::fs::write(d.join("frame_0002.PNG"), b"b").unwrap();
    // Push the first picture's time well into the past so the order is certain.
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options().write(true).open(d.join("frame_0001.ppm")).unwrap().set_modified(old).unwrap();
    let (p, n) = newest_frame(&d).unwrap().unwrap();
    assert_eq!(p.file_name().unwrap(), "frame_0002.PNG");
    assert_eq!(n, 2);
    // Time wins over name.
    let older = old - std::time::Duration::from_secs(3600);
    std::fs::File::options().write(true).open(d.join("frame_0002.PNG")).unwrap().set_modified(older).unwrap();
    assert_eq!(newest_frame(&d).unwrap().unwrap().0.file_name().unwrap(), "frame_0001.ppm");
    assert!(newest_frame(&d.join("missing")).is_err());
}

#[test]
fn no_folder_empty_folder_and_not_run_give_no_verdict() {
    let work = tmp("idle_work");
    let frames = tmp("idle_frames");
    for (p, status) in [(Params::from_specs(&PrintWatchdogNode.info().params), "no_folder"), (params(&frames), "no_frames")] {
        let (v, r, m) = outputs(&PrintWatchdogNode.cook(&ctx(&work), &[], &p).unwrap());
        assert_eq!(m["status"], status);
        assert_eq!(v["verdict"], VERDICT_NONE);
        assert_eq!(v["advisory"], true);
        assert_eq!(r.title, WATCHDOG_LABEL);
    }
    std::fs::write(frames.join("frame_0001.ppm"), circle_ppm([255, 0, 0])).unwrap();
    let (v, _, m) = outputs(&PrintWatchdogNode.cook(&ctx(&work), &[], &params(&frames)).unwrap());
    assert_eq!(m["status"], "not_run");
    assert_eq!(m["frames_in_folder"], 1);
    assert_eq!(v["verdict"], VERDICT_NONE);
    assert!(v["frame"].as_str().unwrap().ends_with("frame_0001.ppm"));
}

#[test]
fn missing_folder_and_empty_question_have_distinct_errors() {
    let work = tmp("err_work");
    let frames = tmp("err_frames");
    let e = PrintWatchdogNode.cook(&ctx(&work), &[], &params(&frames.join("nope"))).unwrap_err().to_string();
    assert!(e.contains("frames folder not found"), "{e}");
    let mut p = params(&frames);
    p.set("question", ParamValue::Str("  ".into()));
    let e = PrintWatchdogNode.cook(&ctx(&work), &[], &p).unwrap_err().to_string();
    assert!(e.contains("there is no question"), "{e}");
}

#[test]
fn run_without_install_fails_clearly() {
    let work = tmp("noinst_work");
    let frames = tmp("noinst_frames");
    std::fs::write(frames.join("frame_0001.ppm"), circle_ppm([255, 0, 0])).unwrap();
    let mut p = params(&frames);
    p.set("ai_root", ParamValue::Str(work.join("no_such_root").to_string_lossy().into()));
    p.set("run", ParamValue::Bool(true));
    assert!(PrintWatchdogNode.cook(&ctx(&work), &[], &p).is_err());
}

#[test]
fn earlier_answers_reload_and_map_to_verdicts() {
    let work = tmp("cache_work");
    for (i, (answer, verdict, warns)) in
        [("Yes. Strands of plastic cover the bed.", VERDICT_FAILURE, true), ("No, it looks intact.", VERDICT_OK, false), ("Hard to tell.", VERDICT_UNCLEAR, false)]
            .into_iter()
            .enumerate()
    {
        let frames = tmp(&format!("cache_frames_{i}"));
        let frame = frames.join("frame_0001.ppm");
        std::fs::write(&frame, circle_ppm([i as u8 * 40, 0, 0])).unwrap();
        seed(&work, &frame, DEFAULT_QUESTION, 64, answer);
        let (v, r, m) = outputs(&PrintWatchdogNode.cook(&ctx(&work), &[], &params(&frames)).unwrap());
        assert_eq!(m["status"], "complete");
        assert_eq!(m["advisory"], true);
        assert_eq!(v["verdict"], verdict);
        assert_eq!(m["verdict"], verdict);
        assert_eq!(v["answer"], answer);
        assert_eq!(v["label"], WATCHDOG_LABEL);
        assert_eq!(r.notes.iter().any(|(s, _)| *s == Severity::Warning), warns, "{answer}");
        assert!(r.notes.iter().any(|(_, t)| t.contains("does not stop or pause the printer")));
        assert!(r.metrics.keys().all(|k| k.starts_with("ai_")), "metric names: {:?}", r.metrics.keys().collect::<Vec<_>>());
    }
}

#[test]
fn a_newer_picture_is_a_new_check() {
    let work = tmp("newer_work");
    let frames = tmp("newer_frames");
    let first = frames.join("frame_0001.ppm");
    std::fs::write(&first, circle_ppm([255, 0, 0])).unwrap();
    seed(&work, &first, DEFAULT_QUESTION, 64, "No, it looks intact.");
    let (v, _, _) = outputs(&PrintWatchdogNode.cook(&ctx(&work), &[], &params(&frames)).unwrap());
    assert_eq!(v["verdict"], VERDICT_OK);
    std::fs::write(frames.join("frame_0002.ppm"), circle_ppm([0, 0, 255])).unwrap();
    let (v, _, m) = outputs(&PrintWatchdogNode.cook(&ctx(&work), &[], &params(&frames)).unwrap());
    assert_eq!(m["status"], "not_run", "the new picture has no answer yet");
    assert_eq!(v["verdict"], VERDICT_NONE);
    assert!(v["frame"].as_str().unwrap().ends_with("frame_0002.ppm"));
}

/// Checks the mechanism with the real model: the newest picture is the one asked about,
/// and a yes or a no becomes the right verdict. The pictures are drawn circles and the
/// question is about the circle's colour, which the model is known to answer. This says
/// nothing about how well the default question spots a failed print in a real photo.
#[test]
#[ignore = "needs the installed Qwen3-VL model, a CUDA GPU and WSL (set VERNON_AI3D_ROOT)"]
fn real_watchdog_checks_the_newest_picture() {
    let root = std::env::var(crate::bridge::ROOT_ENV).expect("VERNON_AI3D_ROOT");
    let dir = std::env::var("VERNON_AI_E2E_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("vernon_ai_e2e"))
        .join("watchdog");
    let timeout: i64 = std::env::var("VERNON_AI_E2E_TIMEOUT_S").ok().and_then(|v| v.parse().ok()).unwrap_or(1800);
    let (red, blue) = ([255u8, 0, 0], [0u8, 0, 255]);
    let question = "Is the circle in this picture red? Answer with the single word YES or NO first, then one short sentence.";
    for (name, older, newer, want) in [("newest_red", blue, red, VERDICT_FAILURE), ("newest_blue", red, blue, VERDICT_OK)] {
        let frames = dir.join(name);
        std::fs::create_dir_all(&frames).unwrap();
        std::fs::write(frames.join("frame_0001.ppm"), circle_ppm(older)).unwrap();
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
        std::fs::File::options().write(true).open(frames.join("frame_0001.ppm")).unwrap().set_modified(past).unwrap();
        std::fs::write(frames.join("frame_0002.ppm"), circle_ppm(newer)).unwrap();
        let mut p = params(&frames);
        p.set("ai_root", ParamValue::Str(root.clone()));
        p.set("question", ParamValue::Str(question.into()));
        p.set("timeout_s", ParamValue::Int(timeout));
        p.set("run", ParamValue::Bool(true));
        let t0 = std::time::Instant::now();
        let (v, _, m) = outputs(&PrintWatchdogNode.cook(&ctx(&dir), &[], &p).unwrap());
        println!("{name}: wall {:.1}s\n{}\n{}", t0.elapsed().as_secs_f64(), serde_json::to_string_pretty(&v).unwrap(), serde_json::to_string_pretty(&m).unwrap());
        assert_eq!(m["status"], "complete");
        assert_eq!(v["advisory"], true);
        assert!(v["frame"].as_str().unwrap().ends_with("frame_0002.ppm"), "newest picture: {}", v["frame"]);
        assert_eq!(v["verdict"], want, "{name}: {}", v["answer"]);
    }
}

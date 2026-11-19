use std::path::{Path, PathBuf};

use serde_json::json;
use vernon_graph::{CookContext, Data, NodeOp, ParamValue, Params};

use super::*;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vernon_bodyfit_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ctx(dir: &Path) -> CookContext {
    CookContext { node_name: "t".into(), work_dir: dir.to_path_buf(), frame: 0.0 }
}

fn params() -> Params {
    Params::from_specs(&BodyFitNode.info().params)
}

fn with_image(p: &mut Params, path: &Path) {
    p.set("image", ParamValue::Str(path.to_string_lossy().into()));
}

fn cook_err(dir: &Path, p: &Params) -> String {
    BodyFitNode.cook(&ctx(dir), &[], p).unwrap_err().to_string()
}

/// A stand-in photo (the bytes only name the work folder) and a saved result for it: a
/// 1.7 m tall tetrahedron in the model's camera frame, with a nose and two ankles.
fn seed(dir: &Path, name: &str, vertices_claimed: usize) -> PathBuf {
    let img = dir.join(name);
    std::fs::write(&img, format!("not a real picture: {name}")).unwrap();
    let work = dir.join(format!("bodyfit_{:016x}", work_key(&std::fs::read(&img).unwrap())));
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("body.obj"), "v 0 0 0\nv 0.4 0 0\nv 0 0 0.3\nv 0.2 -1.7 0.1\nf 1 2 3\nf 1 4 2\nf 2 4 3\nf 3 4 1\n").unwrap();
    let res = json!({
        "model": "facebook/sam-3d-body-dinov3", "revision": "rev", "code_commit": "c", "backbone_commit": "b", "device": "cuda",
        "image_size": [100, 200], "mesh": "body.obj", "vertices": vertices_claimed, "triangles": 4,
        "keypoints": [
            {"name": "nose", "xyz_m": [0.2, -1.6, 0.1], "uv_px": [50.0, 20.0]},
            {"name": "left-ankle", "xyz_m": [0.3, -0.1, 0.1], "uv_px": [60.0, 190.0]},
            {"name": "right-ankle", "xyz_m": [0.1, -0.1, 0.1], "uv_px": [40.0, 190.0]},
        ],
        "seconds_load": 1.0, "seconds_fit": 2.0, "peak_vram_GB": 3.0, "peak_rss_GB": 4.0,
    });
    std::fs::write(work.join("result.json"), serde_json::to_vec(&res).unwrap()).unwrap();
    img
}

fn outputs(out: &[Data]) -> (&vernon_core::Geometry, &serde_json::Value, &vernon_core::Report, &serde_json::Value) {
    let (Data::Geometry(g), Data::Json(pose), Data::Report(r), Data::Json(m)) = (&out[0], &out[1], &out[2], &out[3]) else { panic!("wrong output kinds") };
    (g, pose, r, m)
}

fn bounds(g: &vernon_core::Geometry) -> (DVec3, DVec3) {
    g.points.iter().fold((DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)), |(lo, hi), q| (lo.min(*q), hi.max(*q)))
}

#[test]
fn node_info_is_complete() {
    let i = BodyFitNode.info();
    assert_eq!(i.id, "capture.ai_body_fit");
    assert_eq!(i.label, "AI Body Fit from Photo");
    assert!(i.params.iter().all(|p| !p.help.is_empty()), "every param has help");
    let kinds: Vec<_> = i.outputs.iter().map(|o| o.kind).collect();
    assert_eq!(kinds, [DataKind::Geometry, DataKind::Json, DataKind::Report, DataKind::Json]);
    let p = params();
    assert_eq!(p.str("wsl_distro").unwrap(), "AI-3D");
    assert!(!p.bool("licence_accepted").unwrap(), "the licence is never accepted by default");
    assert_eq!(p.float("known_height_mm").unwrap(), 0.0);
    assert!(!p.bool("run").unwrap());
}

#[test]
fn camera_frame_becomes_z_up_millimetres() {
    // Up in the picture is -y in the camera frame; away from the camera is +z.
    assert_eq!(to_app_mm(DVec3::new(0.0, -1.0, 0.0)), DVec3::new(0.0, 0.0, 1000.0));
    assert_eq!(to_app_mm(DVec3::new(0.5, 0.0, 0.0)), DVec3::new(500.0, 0.0, 0.0));
    assert_eq!(to_app_mm(DVec3::new(0.0, 0.0, 2.0)), DVec3::new(0.0, 2000.0, 0.0));
    // Still a right-handed frame.
    let (x, y, z) = (to_app_mm(DVec3::X), to_app_mm(DVec3::Y), to_app_mm(DVec3::Z));
    assert!(x.cross(y).dot(z) > 0.0);
}

#[test]
fn placement_centres_floors_and_scales() {
    let pts = [DVec3::new(10.0, 20.0, 100.0), DVec3::new(30.0, 60.0, 1100.0)];
    let own = placement(&pts, 0.0).unwrap();
    assert_eq!(own.scale, 1.0);
    assert_eq!(own.model_height_mm, 1000.0);
    assert_eq!(own.apply(pts[0]), DVec3::new(-10.0, -20.0, 0.0));
    assert_eq!(own.apply(pts[1]), DVec3::new(10.0, 20.0, 1000.0));
    let known = placement(&pts, 1800.0).unwrap();
    assert!((known.scale - 1.8).abs() < 1e-12);
    assert!((known.apply(pts[1]).z - 1800.0).abs() < 1e-9);
    assert_eq!(known.model_height_mm, 1000.0);
    assert!(placement(&[], 0.0).unwrap_err().contains("no points"));
    assert!(placement(&[DVec3::ZERO, DVec3::X], 0.0).unwrap_err().contains("no height"));
    assert!(placement(&[DVec3::ZERO, DVec3::new(0.0, 0.0, f64::NAN)], 0.0).is_err());
}

#[test]
fn keypoints_are_parsed_and_checked() {
    let good = json!({"keypoints": [{"name": "nose", "xyz_m": [1.0, 2.0, 3.0], "uv_px": [4.0, 5.0]}]});
    let k = parse_keypoints(&good).unwrap();
    assert_eq!(k, vec![KeyPoint { name: "nose".into(), xyz_m: DVec3::new(1.0, 2.0, 3.0), uv_px: [4.0, 5.0] }]);
    assert!(parse_keypoints(&json!({})).unwrap_err().contains("no 'keypoints'"));
    assert!(parse_keypoints(&json!({"keypoints": []})).unwrap_err().contains("empty"));
    assert!(parse_keypoints(&json!({"keypoints": [{"name": "a", "xyz_m": [1.0, 2.0], "uv_px": [0.0, 0.0]}]})).unwrap_err().contains("xyz_m"));
    assert!(parse_keypoints(&json!({"keypoints": [{"name": "a", "xyz_m": [1.0, 2.0, 3.0], "uv_px": [0.0]}]})).unwrap_err().contains("uv_px"));
    assert!(parse_keypoints(&json!({"keypoints": [{"xyz_m": [1.0, 2.0, 3.0], "uv_px": [0.0, 0.0]}]})).unwrap_err().contains("no name"));
}

#[test]
fn work_key_follows_the_photo() {
    assert_eq!(work_key(b"abc"), work_key(b"abc"));
    assert_ne!(work_key(b"abc"), work_key(b"abd"));
}

#[test]
fn no_photo_is_idle_and_bad_photos_are_errors() {
    let dir = tmp("idle");
    let out = BodyFitNode.cook(&ctx(&dir), &[], &params()).unwrap();
    let (g, pose, _, m) = outputs(&out);
    assert_eq!(m["status"], "no_image");
    assert!(g.points.is_empty());
    assert_eq!(pose["advisory"], true);
    let mut p = params();
    with_image(&mut p, &dir.join("missing.jpg"));
    assert!(cook_err(&dir, &p).contains("photo not found"));
    let txt = dir.join("a.txt");
    std::fs::write(&txt, "x").unwrap();
    with_image(&mut p, &txt);
    assert!(cook_err(&dir, &p).contains("unsupported image format"));
}

#[test]
fn nothing_starts_without_run_or_without_the_licence() {
    let dir = tmp("gate");
    let img = dir.join("a.jpg");
    std::fs::write(&img, "x").unwrap();
    let mut p = params();
    with_image(&mut p, &img);
    let out = BodyFitNode.cook(&ctx(&dir), &[], &p).unwrap();
    let (_, _, rep, m) = outputs(&out);
    assert_eq!(m["status"], "not_run");
    assert!(m["licence"].as_str().unwrap().contains("SAM License"));
    assert!(rep.notes.iter().any(|(_, t)| t.contains("opt-in")));
    // Run asked for, licence not ticked: refused before any install is looked for.
    p.set("run", ParamValue::Bool(true));
    p.set("ai_root", ParamValue::Str(dir.join("no_such_root").to_string_lossy().into()));
    let e = cook_err(&dir, &p);
    assert!(e.contains("I accept the model's licence") && e.contains("military"), "{e}");
    assert!(!dir.join(format!("bodyfit_{:016x}", work_key(b"x"))).exists(), "no work folder is made");
}

#[test]
fn a_saved_result_loads_in_the_app_frame() {
    let dir = tmp("cached");
    let img = seed(&dir, "p.jpg", 4);
    let mut p = params();
    with_image(&mut p, &img);
    let out = BodyFitNode.cook(&ctx(&dir), &[], &p).unwrap();
    let (g, pose, rep, m) = outputs(&out);
    assert_eq!(m["status"], "complete");
    assert_eq!(m["advisory"], true);
    assert_eq!((g.points.len(), g.prims.len()), (4, 4));
    let (lo, hi) = bounds(g);
    assert!((lo - DVec3::new(-200.0, -150.0, 0.0)).length() < 1e-6, "{lo:?}");
    assert!((hi - DVec3::new(200.0, 150.0, 1700.0)).length() < 1e-6, "{hi:?}");
    assert!((m["model_height_mm"].as_f64().unwrap() - 1700.0).abs() < 1e-6);
    assert_eq!(m["scale"], 1.0);
    assert_eq!(m["open_edges"], 0);
    let nose = &pose["keypoints"][0];
    assert_eq!(nose["name"], "nose");
    let q: Vec<f64> = nose["xyz_mm"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert!((q[0] - 0.0).abs() < 1e-6 && (q[1] + 50.0).abs() < 1e-6 && (q[2] - 1600.0).abs() < 1e-6, "{q:?}");
    assert_eq!(nose["uv_px"], json!([50.0, 20.0]));
    assert_eq!(rep.title, BODY_LABEL);
    assert!(rep.notes.iter().any(|(s, t)| *s == Severity::Warning && t.contains("model's own guess")));
    // A known height rescales the same saved result; the model is not needed.
    p.set("known_height_mm", ParamValue::Float(1800.0));
    let out = BodyFitNode.cook(&ctx(&dir), &[], &p).unwrap();
    let (g, pose, rep, m) = outputs(&out);
    let (lo, hi) = bounds(g);
    assert!(lo.z.abs() < 1e-6 && (hi.z - 1800.0).abs() < 1e-6);
    assert!((m["scale"].as_f64().unwrap() - 1800.0 / 1700.0).abs() < 1e-9);
    assert!((pose["keypoints"][0]["xyz_mm"][2].as_f64().unwrap() - 1600.0 * 1800.0 / 1700.0).abs() < 1e-6);
    assert!(rep.notes.iter().any(|(_, t)| t.contains("stands upright")));
}

#[test]
fn a_saved_result_that_disagrees_with_its_mesh_is_refused() {
    let dir = tmp("mismatch");
    let img = seed(&dir, "p.jpg", 5);
    let mut p = params();
    with_image(&mut p, &img);
    let e = cook_err(&dir, &p);
    assert!(e.contains("4 points") && e.contains("says 5"), "{e}");
}

fn key_mm(pose: &serde_json::Value, name: &str) -> (DVec3, [f64; 2]) {
    let k = pose["keypoints"].as_array().unwrap().iter().find(|k| k["name"] == name).unwrap_or_else(|| panic!("no key point '{name}'"));
    let f = |v: &serde_json::Value, i: usize| v[i].as_f64().unwrap();
    (DVec3::new(f(&k["xyz_mm"], 0), f(&k["xyz_mm"], 1), f(&k["xyz_mm"], 2)), [f(&k["uv_px"], 0), f(&k["uv_px"], 1)])
}

/// Runs the real model on two photos of a standing person: arms at the sides
/// (`arms_down.jpg`) and arms overhead (`arms_up.jpg`), from VERNON_BODYFIT_PHOTOS_DIR.
/// Needs VERNON_AI3D_ROOT and the BodyFit install. Running it is accepting the licence.
#[test]
#[ignore = "needs the opt-in BodyFit install (VERNON_AI3D_ROOT) and VERNON_BODYFIT_PHOTOS_DIR"]
fn real_body_fit_follows_the_pose() {
    let root = std::env::var("VERNON_AI3D_ROOT").expect("set VERNON_AI3D_ROOT");
    let photos = PathBuf::from(std::env::var("VERNON_BODYFIT_PHOTOS_DIR").expect("set VERNON_BODYFIT_PHOTOS_DIR"));
    let dir = std::env::var("VERNON_AI_E2E_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("vernon_bodyfit_real"));
    std::fs::create_dir_all(&dir).unwrap();
    for (file, arms_up) in [("arms_down.jpg", false), ("arms_up.jpg", true)] {
        let mut p = params();
        p.set("ai_root", ParamValue::Str(root.clone()));
        with_image(&mut p, &photos.join(file));
        p.set("licence_accepted", ParamValue::Bool(true));
        p.set("run", ParamValue::Bool(true));
        if let Some(t) = std::env::var("VERNON_AI_E2E_TIMEOUT_S").ok().and_then(|v| v.parse::<i64>().ok()) {
            p.set("timeout_s", ParamValue::Int(t));
        }
        let t = std::time::Instant::now();
        let out = body_fit(&ctx(&dir), &p).unwrap();
        let wall = t.elapsed().as_secs_f64();
        let (g, pose, _, m) = outputs(&out);
        let (lo, hi) = bounds(g);
        let own = m["model_height_mm"].as_f64().unwrap();
        println!(
            "BODY {file} | wall {wall:.0}s | points {} tris {} open {} comps {} | own height {own:.0} mm | box {:.0} x {:.0} x {:.0} mm | vram {} rss {} load {} fit {}",
            m["points"], m["triangles"], m["open_edges"], m["components"], hi.x - lo.x, hi.y - lo.y, hi.z - lo.z, m["peak_vram_GB"], m["peak_rss_GB"], m["seconds_load"], m["seconds_fit"]
        );
        assert!(g.points.iter().all(|q| q.is_finite()), "every point is finite");
        assert!(g.points.len() > 5_000 && m["triangles"].as_u64().unwrap() > 10_000, "a full body surface");
        let (nose, nose_uv) = key_mm(pose, "nose");
        // The model's own names use hyphens (sam_3d_body.metadata.mhr70).
        let (ls, _) = key_mm(pose, "left-shoulder");
        let (rs, _) = key_mm(pose, "right-shoulder");
        let (lw, lw_uv) = key_mm(pose, "left-wrist");
        let (rw, rw_uv) = key_mm(pose, "right-wrist");
        let (la, _) = key_mm(pose, "left-ankle");
        let (ra, _) = key_mm(pose, "right-ankle");
        println!("KEYS {file} | nose z {:.0} | shoulders z {:.0} {:.0} | wrists z {:.0} {:.0} | ankles z {:.0} {:.0} | nose px {:.0},{:.0}", nose.z, ls.z, rs.z, lw.z, rw.z, la.z, ra.z, nose_uv[0], nose_uv[1]);
        // The bar, fixed before the first run.
        assert!(nose.z - la.z.max(ra.z) > 1000.0, "the head is more than a metre above the ankles");
        let size = m["image"]["size"].as_array().unwrap();
        let (w, h) = (size[0].as_f64().unwrap(), size[1].as_f64().unwrap());
        assert!(nose_uv[0] >= 0.0 && nose_uv[0] <= w && nose_uv[1] >= 0.0 && nose_uv[1] <= h * 0.5, "the nose is in the top half of the picture");
        if arms_up {
            assert!(lw.z > nose.z && rw.z > nose.z, "arms overhead: both wrists above the nose");
            assert!(lw_uv[1] < nose_uv[1] && rw_uv[1] < nose_uv[1], "arms overhead in the picture too");
            // Hands overhead add to the top-to-bottom extent.
            assert!((1700.0..=2700.0).contains(&own), "own height {own:.0} mm with arms overhead");
        } else {
            assert!(lw.z < ls.z.min(rs.z) && rw.z < ls.z.min(rs.z), "arms down: both wrists below the shoulders");
            assert!((1400.0..=2100.0).contains(&own), "own height {own:.0} mm for a standing adult");
        }
        // A known height rescales the saved result without the model.
        p.set("run", ParamValue::Bool(false));
        p.set("known_height_mm", ParamValue::Float(1700.0));
        let out = body_fit(&ctx(&dir), &p).unwrap();
        let (g, _, _, _) = outputs(&out);
        let (lo, hi) = bounds(g);
        assert!(lo.z.abs() < 1e-6 && (hi.z - 1700.0).abs() < 1e-6, "known height applied");
    }
}

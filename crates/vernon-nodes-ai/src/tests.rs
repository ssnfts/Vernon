use std::path::{Path, PathBuf};
use std::time::Duration;

use glam::DVec3;
use serde_json::json;
use vernon_core::report::Severity;
use vernon_core::Geometry;
use vernon_graph::{CookContext, CookError, Data, Graph, NodeOp, ParamValue, Params, Registry};

use crate::aero::{self, AeroPreviewNode};
use crate::bridge::{self, Event, Install, Resources};
use crate::compare::{self, AeroCompareNode};
use crate::parts::{self, LoadedPart, PartCrafterNode};
use crate::prep::{self, Domain, Placement, PrepOptions};
use crate::{AERO_LABEL, CFD_VALIDATION};

static PROCESS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("vernon_ai_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ctx(dir: &Path) -> CookContext {
    CookContext { node_name: "t".into(), work_dir: dir.to_path_buf(), frame: 0.0 }
}

fn params_for(op: &dyn NodeOp) -> Params {
    Params::from_specs(&op.info().params)
}

/// A closed box in millimetres, centred at `c`.
fn boxed(size: DVec3, c: DVec3) -> Geometry {
    let mut g = Geometry::make_box(size);
    g.transform(glam::DMat4::from_translation(c));
    g
}

/// 4.6 x 1.9 x 1.4 m box standing in for a car.
fn car_box() -> Geometry {
    boxed(DVec3::new(4600.0, 1900.0, 1400.0), DVec3::new(0.0, 0.0, 700.0))
}

// ---------------- bridge ----------------

#[test]
fn parses_bridge_lines() {
    assert_eq!(
        bridge::parse_line(r#"{"event":"progress","stage":"load","fraction":0.5,"message":"m"}"#),
        Some(Event::Progress(bridge::Progress { stage: "load".into(), fraction: 0.5, message: "m".into() }))
    );
    assert!(matches!(bridge::parse_line(r#"{"event":"result","x":1}"#), Some(Event::Result(_))));
    assert_eq!(
        bridge::parse_line(r#"{"event":"error","code":"out_of_memory","message":"gpu"}"#),
        Some(Event::Error { code: "out_of_memory".into(), message: "gpu".into() })
    );
    assert_eq!(bridge::parse_line("Decoding: 100%|####|"), None);
    assert_eq!(bridge::parse_line(r#"{"event":"mystery"}"#), None);
    // fractions are clamped
    if let Some(Event::Progress(p)) = bridge::parse_line(r#"{"event":"progress","fraction":7}"#) {
        assert_eq!(p.fraction, 1.0);
    } else {
        panic!();
    }
}

#[test]
fn converts_windows_paths_for_wsl() {
    assert_eq!(bridge::to_wsl_path(r"E:\AI-3D\DoMINO\x.stl"), "/mnt/e/AI-3D/DoMINO/x.stl");
    assert_eq!(bridge::to_wsl_path("/opt/ai3d/x"), "/opt/ai3d/x");
}

#[test]
fn parses_resource_queries() {
    assert_eq!(bridge::parse_nvidia_smi("5470, 16376\n"), Some((5470, 16376)));
    assert_eq!(bridge::parse_nvidia_smi("garbage"), None);
    let free = "              total        used        free      shared  buff/cache   available\nMem:          26000        3000       20000          10        3000       22000\nSwap:          8192           0        8192\n";
    assert_eq!(bridge::parse_free_m(free), Some(22000));
    assert_eq!(bridge::parse_free_m("nothing"), None);
}

#[test]
fn refuses_when_memory_is_short() {
    let need = bridge::with_margin(4.31, 6.0);
    assert!(need.vram_mib > 4400 && need.vram_mib < 6500);
    let ok = Resources { vram_free_mib: 12000, vram_total_mib: 16376, ram_available_mib: 20000 };
    assert!(bridge::check_resources(need, ok, "X").is_ok());
    let low_vram = Resources { vram_free_mib: 2000, ..ok };
    let e = bridge::check_resources(need, low_vram, "X").unwrap_err().to_string();
    assert!(e.contains("GPU memory") && e.contains("2000"), "{e}");
    let low_ram = Resources { ram_available_mib: 1000, ..ok };
    assert!(bridge::check_resources(need, low_ram, "X").unwrap_err().to_string().contains("RAM"));
}

#[test]
fn install_resolution_errors_are_clear() {
    // blank + no env var
    std::env::remove_var(bridge::ROOT_ENV);
    let e = Install::resolve("", "AI-3D").unwrap_err().to_string();
    assert!(e.contains("VERNON_AI3D_ROOT"), "{e}");
    let e = Install::resolve("Z:/definitely/not/here", "AI-3D").unwrap_err().to_string();
    assert!(e.contains("not found"), "{e}");
    let d = tmp("install");
    let i = Install::resolve(d.to_str().unwrap(), "AI-3D").unwrap();
    assert!(i.require_model_dir("DoMINO").unwrap_err().to_string().contains("not installed"));
    std::fs::create_dir_all(d.join("DoMINO")).unwrap();
    assert!(i.require_model_dir("DoMINO").is_ok());
    assert_eq!(i.python("DoMINO"), "/opt/ai3d/DoMINO/bin/python");
}

fn script(dir: &Path, name: &str, body_win: &str, body_unix: &str) -> std::process::Command {
    if cfg!(windows) {
        let p = dir.join(format!("{name}.bat"));
        std::fs::write(&p, format!("@echo off\r\n{body_win}\r\n")).unwrap();
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(p);
        c
    } else {
        let p = dir.join(format!("{name}.sh"));
        std::fs::write(&p, format!("{body_unix}\n")).unwrap();
        let mut c = std::process::Command::new("sh");
        c.arg(p);
        c
    }
}

#[test]
fn process_result_error_and_protocol_paths() {
    let _g = PROCESS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let d = tmp("proc");
    let ok = script(
        &d,
        "ok",
        "echo {\"event\":\"progress\",\"stage\":\"a\",\"fraction\":0.5,\"message\":\"m\"}\r\necho chatter\r\necho {\"event\":\"result\",\"value\":42}",
        "echo '{\"event\":\"progress\",\"stage\":\"a\",\"fraction\":0.5,\"message\":\"m\"}'; echo chatter; echo '{\"event\":\"result\",\"value\":42}'",
    );
    let v = bridge::run_process(ok, &d, Duration::from_secs(30), &|| {}).unwrap();
    assert_eq!(v["value"], 42);
    assert!(std::fs::read_to_string(d.join("progress.jsonl")).unwrap().contains("\"fraction\""));

    let bad = script(&d, "bad", "echo {\"event\":\"error\",\"code\":\"out_of_memory\",\"message\":\"gpu full\"}\r\nexit /b 2", "echo '{\"event\":\"error\",\"code\":\"out_of_memory\",\"message\":\"gpu full\"}'; exit 2");
    match bridge::run_process(bad, &d, Duration::from_secs(30), &|| {}) {
        Err(bridge::BridgeError::Model { code, message }) => {
            assert_eq!(code, "out_of_memory");
            assert!(message.contains("gpu full"));
        }
        other => panic!("{other:?}"),
    }

    let none = script(&d, "none", "echo just text", "echo just text");
    assert!(matches!(bridge::run_process(none, &d, Duration::from_secs(30), &|| {}), Err(bridge::BridgeError::Protocol(_))));
}

#[test]
fn process_timeout_and_cancel_kill_the_child() {
    let _g = PROCESS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let d = tmp("kill");
    let slow = || script(&d, "slow", "ping -n 30 127.0.0.1 >nul", "sleep 30");
    let t0 = std::time::Instant::now();
    let r = bridge::run_process(slow(), &d, Duration::from_millis(500), &|| {});
    assert!(matches!(r, Err(bridge::BridgeError::Timeout(_))), "{r:?}");
    assert!(t0.elapsed() < Duration::from_secs(10));
    // cancel from another thread
    let h = std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(800));
        bridge::request_cancel();
    });
    let t0 = std::time::Instant::now();
    let r = bridge::run_process(slow(), &d, Duration::from_secs(60), &|| {});
    h.join().unwrap();
    assert_eq!(r, Err(bridge::BridgeError::Cancelled));
    assert!(t0.elapsed() < Duration::from_secs(10));
    bridge::clear_cancel();
}

// ---------------- preparation ----------------

#[test]
fn car_sized_box_snaps_into_the_training_span() {
    let o = PrepOptions { placement: Placement::SnapToTrainingFrame, ..Default::default() };
    let p = prep::prepare(&car_box(), &o).unwrap();
    assert_eq!(p.open_edges, 0);
    assert_eq!(p.components, 1);
    assert!((p.min_m.x - prep::TRAIN_MIN.x).abs() < 1e-9);
    assert!((p.min_m.z - prep::TRAIN_MIN.z).abs() < 1e-9);
    assert!((p.min_m.y + p.max_m.y).abs() < 1e-9);
    // nose-to-tail 4.6 m is inside the box and the training span
    assert_eq!(p.domain, Domain::WithinTrainingSpan);
    assert!(p.blocking().is_none());
    // scene coordinates round-trip
    let back = p.to_scene_mm(p.tris[0][0]);
    let orig = car_box().points.iter().any(|q| (*q - back).length() < 1e-6);
    assert!(orig);
}

#[test]
fn small_or_huge_geometry_is_flagged_before_inference() {
    // as-is car box centred on the origin: z -? it spans y +-0.95 and x +-2.3: nose at -2.3 is outside the surface box
    let p = prep::prepare(&car_box(), &PrepOptions::default()).unwrap();
    assert_eq!(p.domain, Domain::Outside);
    assert!(p.blocking().is_some());
    let allow = PrepOptions { allow_outside: true, ..Default::default() };
    let p = prep::prepare(&car_box(), &allow).unwrap();
    assert!(p.blocking().is_none());
    assert!(p.checks.iter().any(|c| c.id == "outside_domain" && c.severity == Severity::Warning));
    // a 100 mm cube inside the box is far smaller than any training car
    let cube = boxed(DVec3::splat(100.0), DVec3::new(1000.0, 0.0, 100.0));
    let p = prep::prepare(&cube, &PrepOptions::default()).unwrap();
    assert_eq!(p.domain, Domain::OutOfDistribution);
    assert!(p.blocking().is_none());
    // a 20 m long box
    let big = boxed(DVec3::new(20000.0, 2000.0, 1500.0), DVec3::new(0.0, 0.0, 700.0));
    let o = PrepOptions { placement: Placement::SnapToTrainingFrame, ..Default::default() };
    assert_eq!(prep::prepare(&big, &o).unwrap().domain, Domain::Outside);
}

#[test]
fn rejects_unusable_geometry() {
    let mut tiny = Geometry::new();
    for p in [DVec3::ZERO, DVec3::X, DVec3::Y] {
        tiny.add_point(p);
    }
    tiny.add_prim(vernon_core::Prim::polygon(vec![0, 1, 2]));
    assert!(prep::prepare(&tiny, &PrepOptions::default()).unwrap_err().contains("at least 8"));
    let mut g = car_box();
    g.points[0].x = f64::NAN;
    assert!(prep::prepare(&g, &PrepOptions::default()).unwrap_err().contains("non-finite"));
    let o = PrepOptions { max_triangles: 4, ..Default::default() };
    assert!(prep::prepare(&car_box(), &o).unwrap_err().contains("decimate"));
}

#[test]
fn topology_counts_open_edges_and_pieces() {
    let g = car_box();
    let tris: Vec<[DVec3; 3]> = g.triangles().iter().map(|(_, t)| [g.points[t[0] as usize], g.points[t[1] as usize], g.points[t[2] as usize]]).collect();
    assert_eq!(prep::topology(&tris), (0, 0, 1));
    let open = &tris[..tris.len() - 2]; // drop one quad's two triangles
    let (o, nm, c) = prep::topology(open);
    assert_eq!((nm, c), (0, 1));
    assert_eq!(o, 4);
    let mut two = tris.clone();
    two.extend(tris.iter().map(|t| [t[0] + DVec3::splat(100000.0), t[1] + DVec3::splat(100000.0), t[2] + DVec3::splat(100000.0)]));
    assert_eq!(prep::topology(&two).2, 2);
}

#[test]
fn open_mesh_gets_a_watertight_warning() {
    let mut g = car_box();
    g.prims.pop();
    let o = PrepOptions { placement: Placement::SnapToTrainingFrame, ..Default::default() };
    let p = prep::prepare(&g, &o).unwrap();
    assert!(p.open_edges > 0);
    assert!(p.checks.iter().any(|c| c.id == "not_watertight"));
}

#[test]
fn stl_has_the_binary_layout() {
    let p = prep::prepare(&car_box(), &PrepOptions { placement: Placement::SnapToTrainingFrame, ..Default::default() }).unwrap();
    let b = prep::stl_bytes(&p.tris);
    assert_eq!(b.len(), 84 + 50 * p.tris.len());
    assert_eq!(u32::from_le_bytes([b[80], b[81], b[82], b[83]]) as usize, p.tris.len());
}

#[test]
fn slice_points_skip_the_body_only_when_watertight() {
    let o = PrepOptions { placement: Placement::SnapToTrainingFrame, ..Default::default() };
    let p = prep::prepare(&car_box(), &o).unwrap();
    let (pts, idx, masked) = prep::slice_points(&p, 0.0, (-2.0, 6.0), (-0.3, 2.0), 40, 20);
    assert!(masked);
    assert!(pts.len() < 40 * 20 && !pts.is_empty());
    assert_eq!(pts.len(), idx.len());
    assert!(pts.iter().all(|q| !prep::inside_mesh(&p.tris, *q)));
    let mut g = car_box();
    g.prims.pop();
    let p2 = prep::prepare(&g, &o).unwrap();
    let (pts2, _, masked2) = prep::slice_points(&p2, 0.0, (-2.0, 6.0), (-0.3, 2.0), 40, 20);
    assert!(!masked2);
    assert_eq!(pts2.len(), 800);
}

// ---------------- output parsing ----------------

fn prepared_car() -> (Geometry, prep::Prepared) {
    let g = car_box();
    let p = prep::prepare(&g, &PrepOptions { placement: Placement::SnapToTrainingFrame, ..Default::default() }).unwrap();
    (g, p)
}

#[test]
fn surface_prediction_is_painted_with_labels_and_ranges() {
    let (g, p) = prepared_car();
    let n = p.tris.len();
    let fields: Vec<f32> = (0..n).flat_map(|i| [i as f32 / n as f32 - 0.5, 0.001, 0.0, 0.0]).collect();
    let (out, st, (lo, hi)) = aero::build_surface(&g, &p, &fields, "pressure", Some((vec![-1.0, -1.0, -1.0, -1.0], vec![0.4, 1.0, 1.0, 1.0]))).unwrap();
    assert_eq!(st.triangles, n);
    assert!(hi > lo);
    assert_eq!(out.prim_attribs.floats("ai_p").unwrap().len(), g.prims.len());
    assert_eq!(out.prim_attribs.vec3s("ai_tau").unwrap().len(), g.prims.len());
    let cd = out.point_attribs.vec3s("Cd").unwrap();
    assert_eq!(cd.len(), g.points.len());
    assert!(cd.iter().all(|c| c.min_element() >= 0.0 && c.max_element() <= 1.0));
    assert_eq!(out.detail_attribs.strs("ai_label").unwrap()[0], AERO_LABEL);
    assert!((st.area_m2 - 2.0 * (4.6 * 1.9 + 4.6 * 1.4 + 1.9 * 1.4)).abs() < 1e-6);
    // 0.4..0.5 is out of the stated range: top fifth of the triangles
    assert!(st.out_of_range_fraction > 0.05 && st.out_of_range_fraction < 0.3, "{}", st.out_of_range_fraction);
    assert!(st.p_min < st.p_mean && st.p_mean < st.p_max);
}

#[test]
fn bad_surface_output_is_rejected() {
    let (g, p) = prepared_car();
    let n = p.tris.len();
    assert!(aero::build_surface(&g, &p, &vec![0.0; n * 4 - 1], "pressure", None).unwrap_err().contains("expected"));
    let mut f = vec![0.0f32; n * 4];
    f[3] = f32::NAN;
    assert!(aero::build_surface(&g, &p, &f, "pressure", None).unwrap_err().contains("non-finite"));
}

#[test]
fn slice_prediction_maps_back_to_scene_millimetres() {
    let (_, p) = prepared_car();
    let (pts, _, _) = prep::slice_points(&p, 0.0, (-2.0, 6.0), (-0.3, 2.0), 10, 6);
    let n = pts.len();
    let f: Vec<f32> = (0..n).flat_map(|i| [1.0 + i as f32 * 0.01, 0.0, 0.0, -0.1, 1e-5]).collect();
    let (g, st, _) = aero::build_slice(&p, &pts, &f, 60 - n, None).unwrap();
    assert_eq!(g.points.len(), n);
    assert_eq!(st.points, n);
    assert_eq!(st.masked, 60 - n);
    assert!((g.points[0] - p.to_scene_mm(pts[0])).length() < 1e-9);
    assert_eq!(g.point_attribs.floats("ai_speed").unwrap().len(), n);
    assert!(g.point_attribs.contains("Cd"));
    assert!(aero::build_slice(&p, &pts, &f[1..], 0, None).is_err());
}

#[test]
fn out_of_range_fraction_counts_rows() {
    let f = [0.0f32, 0.0, 5.0, 0.0, 0.0, 0.0];
    assert!((aero::out_of_range_fraction(&f, 3, &[-1.0; 3], &[1.0; 3]) - 0.5).abs() < 1e-12);
    assert_eq!(aero::out_of_range_fraction(&f, 3, &[], &[]), 0.0);
}

// ---------------- parts ----------------

fn two_parts() -> Vec<LoadedPart> {
    vec![
        LoadedPart { index: 0, name: "part_00".into(), geometry: Geometry::make_box(DVec3::new(2.0, 1.0, 1.0)) },
        LoadedPart { index: 1, name: "part_01".into(), geometry: boxed(DVec3::splat(0.5), DVec3::new(2.0, 0.0, 0.0)) },
    ]
}

#[test]
fn parts_are_scaled_named_and_selectable() {
    let g = parts::assemble(&two_parts(), 1000.0).unwrap();
    let b = g.bounds().size();
    assert!((b.max_element() - 1000.0).abs() < 1e-6);
    assert!((g.bounds().center().x).abs() < 1e-6);
    assert!(g.bounds().size().x > 0.0);
    assert_eq!(g.prim_groups["part_00"].len(), 6);
    assert_eq!(g.prim_groups["part_01"].len(), 6);
    let ids = g.prim_attribs.ints("part").unwrap();
    assert_eq!(ids.iter().filter(|&&i| i == 1).count(), 6);
    let (one, name) = parts::extract_part(&g, 1).unwrap();
    assert_eq!(name, "part_01");
    assert_eq!(one.prims.len(), 6);
    assert_eq!(one.points.len(), 8);
    one.validate().unwrap();
    let e = parts::extract_part(&g, 7).unwrap_err();
    assert!(e.contains("available parts"), "{e}");
    assert!(parts::extract_part(&Geometry::make_box(DVec3::ONE), 0).unwrap_err().contains("no 'part'"));
    assert!(parts::assemble(&[], 1000.0).is_err());
}

// ---------------- comparison ----------------

fn summary(p_mean: f64, complete: bool, domain: &str) -> serde_json::Value {
    json!({"status": if complete {"complete"} else {"not_run"}, "domain": domain,
           "surface": {"pressure_mean": p_mean, "pressure_min": -0.4, "pressure_max": 0.6, "shear_mean": 0.001},
           "slice": {"speed_mean": 0.9, "pressure_mean": -0.1, "plane": {"y_m": 0.0}}})
}

#[test]
fn comparison_reports_only_shared_supported_quantities() {
    let d = compare::deltas(&summary(-0.10, true, "within"), &summary(-0.14, true, "within"));
    assert_eq!(d.len(), 6);
    let sp = d.iter().find(|x| x.0 == "surface_pressure_mean").unwrap();
    assert!((sp.3 + 0.04).abs() < 1e-12);
    let mut b = summary(-0.1, true, "within");
    b.as_object_mut().unwrap().remove("slice");
    assert_eq!(compare::deltas(&summary(-0.1, true, "within"), &b).len(), 4);
    // no drag-like quantity can appear
    assert!(d.iter().all(|x| !x.0.contains("drag") && !x.0.contains("force") && !x.0.contains("cd")));
}

#[test]
fn comparison_node_refuses_incomplete_runs_and_labels_everything() {
    let dir = tmp("cmp");
    let n = AeroCompareNode;
    let ins = |a, b| vec![Some(Data::json(a)), Some(Data::json(b))];
    let out = n.cook(&ctx(&dir), &ins(summary(0.0, true, "within"), summary(0.0, false, "within")), &params_for(&n)).unwrap();
    let Data::Report(r) = &out[0] else { panic!() };
    assert!(r.notes.iter().any(|(s, t)| *s == Severity::Error && t.contains("variant B")));
    assert!(r.metrics.is_empty());
    let out = n.cook(&ctx(&dir), &ins(summary(-0.1, true, "within"), summary(-0.2, true, "OUT OF TRAINING DISTRIBUTION")), &params_for(&n)).unwrap();
    let Data::Report(r) = &out[0] else { panic!() };
    assert!(r.notes.iter().any(|(_, t)| t.contains(AERO_LABEL)));
    assert!(r.notes.iter().any(|(s, t)| *s == Severity::Warning && t.contains("variant B domain")));
    assert!(r.metrics.keys().all(|k| k.starts_with("ai_")));
    let Data::Json(j) = &out[1] else { panic!() };
    assert_eq!(j["cfd_validation"], CFD_VALIDATION);
    let missing = n.cook(&ctx(&dir), &[None, None], &params_for(&n)).unwrap();
    let Data::Report(r) = &missing[0] else { panic!() };
    assert!(!r.passes());
}

// ---------------- nodes in the graph ----------------

fn registry() -> Registry {
    let mut r = Registry::new();
    crate::register(&mut r);
    r
}

#[test]
fn nodes_are_registered_with_safe_defaults() {
    let r = registry();
    for id in ["geo.ai_partcrafter", "geo.ai_select_part", "cfd.ai_aero_preview", "cfd.ai_aero_compare"] {
        let info = r.info(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(!info.whitebox, "{id}");
        assert!(!info.help.is_empty());
        for p in &info.params {
            if p.name == "run" {
                assert!(p.action, "run must be a one-shot action");
                assert_eq!(p.default_value(), ParamValue::Bool(false));
            }
        }
    }
    // no trademark of the clean-room list leaks into user-visible strings
    for info in r.types() {
        for s in info.visible_strings() {
            let l = s.to_lowercase();
            for banned in ["houdini", "sidefx", "autodesk", "ansys", "simscale", "airshaper"] {
                assert!(!l.contains(banned), "{} contains {banned}", info.id);
            }
        }
    }
}

#[test]
fn aero_preview_checks_geometry_without_running_the_model() {
    let dir = tmp("aero_nodrun");
    let n = AeroPreviewNode;
    let out = n.cook(&ctx(&dir), &[Some(Data::geometry(car_box()))], &params_for(&n)).unwrap();
    assert_eq!(out.len(), 4);
    let Data::Report(r) = &out[2] else { panic!() };
    assert!(r.notes.iter().any(|(_, t)| t.contains(AERO_LABEL)));
    assert!(r.metrics.keys().all(|k| k.starts_with("ai_")), "{:?}", r.metrics.keys().collect::<Vec<_>>());
    // snap placement is the default, so the car box is within span and simply "not run"
    assert!(r.notes.iter().any(|(_, t)| t.contains("not run")));
    let Data::Json(j) = &out[3] else { panic!() };
    assert_eq!(j["status"], "not_run");
    assert_eq!(j["cfd_validation"], "DEFERRED");
    assert_eq!(j["operating_point"]["inlet_speed_mps"], 30.0);
    assert!(!dir.read_dir().unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with("aero_")), "no artifacts without a run");
}

#[test]
fn aero_preview_blocks_out_of_domain_geometry_and_never_launches() {
    let dir = tmp("aero_block");
    let n = AeroPreviewNode;
    let mut p = params_for(&n);
    p.set("placement", ParamValue::Str("as_is".into()));
    p.set("run", ParamValue::Bool(true));
    p.set("ai_root", ParamValue::Str("Z:/no/such/folder".into()));
    let out = n.cook(&ctx(&dir), &[Some(Data::geometry(car_box()))], &p).unwrap();
    let Data::Report(r) = &out[2] else { panic!() };
    assert!(r.notes.iter().any(|(s, t)| *s == Severity::Error && t.contains("inference not started")));
    assert!(!r.passes());
    let Data::Json(j) = &out[3] else { panic!() };
    assert_eq!(j["status"], "blocked");
}

#[test]
fn aero_preview_run_without_install_fails_with_a_clear_message() {
    let dir = tmp("aero_noinstall");
    let n = AeroPreviewNode;
    let mut p = params_for(&n);
    p.set("run", ParamValue::Bool(true));
    p.set("ai_root", ParamValue::Str("Z:/no/such/folder".into()));
    let e = n.cook(&ctx(&dir), &[Some(Data::geometry(car_box()))], &p).unwrap_err();
    assert!(matches!(e, CookError::Failed { .. }));
    assert!(e.to_string().contains("not found"), "{e}");
}

#[test]
fn aero_preview_loads_a_saved_result_for_identical_inputs() {
    // Simulate the bridge output on disk and check the node parses it with no model process.
    let dir = tmp("aero_cached");
    let n = AeroPreviewNode;
    let p = params_for(&n);
    let geo = car_box();
    // first cook only to learn the artifact folder name
    let first = n.cook(&ctx(&dir), &[Some(Data::geometry(geo.clone()))], &p).unwrap();
    let Data::Json(j0) = &first[3] else { panic!() };
    assert_eq!(j0["status"], "not_run");
    let art = PathBuf::from(j0["artifact_dir"].as_str().unwrap());
    std::fs::create_dir_all(&art).unwrap();
    let prepared = prep::prepare(&geo, &PrepOptions { placement: Placement::SnapToTrainingFrame, ..Default::default() }).unwrap();
    let fields: Vec<u8> = (0..prepared.tris.len() * 4).flat_map(|i| (if i % 4 == 0 { -0.2f32 } else { 0.001 }).to_le_bytes()).collect();
    std::fs::write(art.join("surface_fields.f32"), fields).unwrap();
    std::fs::write(
        art.join("result.json"),
        serde_json::to_vec(&json!({"model": "nvidia/domino_drivaerml", "checkpoint": "0.501", "manifest": {"revision": "abc"},
            "runs": [{"mode": "surface", "training_min": [-2.5, -0.02, -0.02, -0.02], "training_max": [0.9, 0.02, 0.02, 0.02]}]})).unwrap(),
    )
    .unwrap();
    let out = n.cook(&ctx(&dir), &[Some(Data::geometry(geo.clone()))], &p).unwrap();
    let Data::Geometry(surf) = &out[0] else { panic!() };
    assert!(surf.prim_attribs.contains("ai_p"));
    let Data::Report(r) = &out[2] else { panic!() };
    let m = &r.metrics["ai_surface_pressure_mean"];
    assert!((m.value + 0.2).abs() < 1e-5, "{}", m.value);
    assert_eq!(m.unit, "model-units");
    assert!(r.metrics.contains_key("ai_surface_out_of_range_fraction"));
    assert!(r.notes.iter().any(|(_, t)| t.contains("no drag or lift force")));
    let Data::Json(j) = &out[3] else { panic!() };
    assert_eq!(j["status"], "complete");
    assert_eq!(j["label"], AERO_LABEL);
    assert_eq!(j["weights"]["revision"], "abc");
    // a different placement is a different artifact folder: not run again
    let mut p2 = p.clone();
    p2.set("placement", ParamValue::Str("as_is".into()));
    p2.set("allow_outside", ParamValue::Bool(true));
    let out2 = n.cook(&ctx(&dir), &[Some(Data::geometry(geo))], &p2).unwrap();
    let Data::Json(j2) = &out2[3] else { panic!() };
    assert_eq!(j2["status"], "not_run");
    // a truncated field file is an error, not a silent partial result
    std::fs::write(art.join("surface_fields.f32"), [0u8; 8]).unwrap();
    assert!(n.cook(&ctx(&dir), &[Some(Data::geometry(car_box()))], &p).is_err());
}

#[test]
fn partcrafter_node_without_image_or_run_does_nothing() {
    let dir = tmp("pc_idle");
    let n = PartCrafterNode;
    let out = n.cook(&ctx(&dir), &[], &params_for(&n)).unwrap();
    let Data::Json(j) = &out[2] else { panic!() };
    assert_eq!(j["status"], "no_image");
    let img = dir.join("a.png");
    std::fs::write(&img, b"not really a png").unwrap();
    let mut p = params_for(&n);
    p.set("image", ParamValue::Str(img.to_string_lossy().into()));
    let out = n.cook(&ctx(&dir), &[], &p).unwrap();
    let Data::Json(j) = &out[2] else { panic!() };
    assert_eq!(j["status"], "not_run");
    p.set("image", ParamValue::Str(dir.join("missing.png").to_string_lossy().into()));
    assert!(n.cook(&ctx(&dir), &[], &p).unwrap_err().to_string().contains("cannot read image"));
}

#[test]
fn partcrafter_node_imports_saved_parts_with_provenance() {
    let dir = tmp("pc_cached");
    let n = PartCrafterNode;
    let img = dir.join("in.png");
    std::fs::write(&img, b"png-bytes").unwrap();
    let mut p = params_for(&n);
    p.set("image", ParamValue::Str(img.to_string_lossy().into()));
    p.set("seed", ParamValue::Int(7));
    let first = n.cook(&ctx(&dir), &[], &p).unwrap();
    let Data::Json(j) = &first[2] else { panic!() };
    let art = PathBuf::from(j["artifact_dir"].as_str().unwrap());
    std::fs::create_dir_all(&art).unwrap();
    let obj = |c: f64| {
        let mut s = String::new();
        for (x, y, z) in [(0., 0., 0.), (1., 0., 0.), (1., 1., 0.), (0., 1., 0.), (0., 0., 1.), (1., 0., 1.), (1., 1., 1.), (0., 1., 1.)] {
            s += &format!("v {} {} {}\n", x + c, y, z);
        }
        for f in [[1, 4, 3, 2], [5, 6, 7, 8], [1, 2, 6, 5], [2, 3, 7, 6], [3, 4, 8, 7], [4, 1, 5, 8]] {
            s += &format!("f {} {} {} {}\n", f[0], f[1], f[2], f[3]);
        }
        s
    };
    std::fs::write(art.join("part_00.obj"), obj(0.0)).unwrap();
    std::fs::write(art.join("part_01.obj"), obj(3.0)).unwrap();
    std::fs::write(
        art.join("result.json"),
        serde_json::to_vec(&json!({"model": "PartCrafter", "weights": {"revision": "69a0ffc"}, "seed": 7, "steps": 10, "tokens": 512, "image_sha256": "abc",
            "peak_vram_GB": 4.3, "seconds_generate": 8.0, "seconds_load": 2.0, "skipped_empty_parts": [2],
            "parts": [{"index": 0, "name": "part_00", "file": "part_00.obj"}, {"index": 1, "name": "part_01", "file": "part_01.obj"}]})).unwrap(),
    )
    .unwrap();
    let out = n.cook(&ctx(&dir), &[], &p).unwrap();
    let Data::Geometry(g) = &out[0] else { panic!() };
    assert_eq!(g.prim_groups.len(), 2);
    assert!((g.bounds().size().max_element() - 1000.0).abs() < 1e-6);
    let Data::Report(r) = &out[1] else { panic!() };
    assert_eq!(r.metrics["ai_part_count"].value, 2.0);
    assert_eq!(r.metrics["ai_part_00_open_edges"].value, 0.0);
    assert!(r.notes.iter().any(|(s, t)| *s == Severity::Warning && t.contains("empty")));
    assert!(r.notes.iter().any(|(_, t)| t.contains("simulation-ready")));
    let Data::Json(m) = &out[2] else { panic!() };
    assert_eq!(m["seed"], 7);
    assert_eq!(m["weights"]["revision"], "69a0ffc");
    assert_eq!(m["image"]["sha256"], "abc");
    assert!(art.join("manifest.json").is_file());
}

#[test]
fn graph_chain_parts_to_select_to_aero() {
    let reg = registry();
    let mut g = Graph::new();
    let pc = g.add_node(&reg, "geo.ai_partcrafter").unwrap();
    let sel = g.add_node(&reg, "geo.ai_select_part").unwrap();
    let aero = g.add_node(&reg, "cfd.ai_aero_preview").unwrap();
    let cmp = g.add_node(&reg, "cfd.ai_aero_compare").unwrap();
    g.connect_checked(&reg, pc, 0, sel, 0).unwrap();
    g.connect_checked(&reg, sel, 0, aero, 0).unwrap();
    g.connect_checked(&reg, aero, 3, cmp, 0).unwrap();
    g.connect_checked(&reg, aero, 3, cmp, 1).unwrap();
    // geometry cannot go into a Json port
    assert!(g.connect_checked(&reg, aero, 0, cmp, 0).is_err());
    // without an image the chain reports a clear error at Select AI Part (empty geometry, no 'part' attribute)
    let e = g.cook(&reg, sel).unwrap_err().to_string();
    assert!(e.contains("no 'part'"), "{e}");
    // the graph round-trips through .vrn JSON with the new nodes
    let back = Graph::from_json(&g.to_json()).unwrap();
    assert_eq!(back.nodes.len(), 4);
}

// ---------------- real models (opt in) ----------------

/// Closed ellipsoid with triangle caps, millimetres. A stand-in shape, not a car.
fn ellipsoid(nu: usize, nv: usize, radii: DVec3, center: DVec3) -> Geometry {
    let mut pts = vec![center + DVec3::new(0.0, 0.0, radii.z)];
    for j in 1..nv {
        let th = std::f64::consts::PI * j as f64 / nv as f64;
        for i in 0..nu {
            let ph = 2.0 * std::f64::consts::PI * i as f64 / nu as f64;
            pts.push(center + DVec3::new(radii.x * th.sin() * ph.cos(), radii.y * th.sin() * ph.sin(), radii.z * th.cos()));
        }
    }
    pts.push(center - DVec3::new(0.0, 0.0, radii.z));
    let ring = |j: usize, i: usize| (1 + (j - 1) * nu + i % nu) as u32;
    let (top, bot) = (0u32, (pts.len() - 1) as u32);
    let mut faces = Vec::new();
    for i in 0..nu {
        faces.push(vec![top, ring(1, i + 1), ring(1, i)]);
        faces.push(vec![bot, ring(nv - 1, i), ring(nv - 1, i + 1)]);
    }
    for j in 1..nv - 1 {
        for i in 0..nu {
            faces.push(vec![ring(j, i), ring(j, i + 1), ring(j + 1, i + 1), ring(j + 1, i)]);
        }
    }
    Geometry::from_polygons(pts, faces)
}

fn e2e_dir(name: &str) -> PathBuf {
    let base = std::env::var("VERNON_AI_E2E_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("vernon_ai_e2e"));
    let d = base.join(name);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn real_params(op: &dyn NodeOp) -> Params {
    let mut p = params_for(op);
    p.set("run", ParamValue::Bool(true));
    p
}

#[test]
#[ignore = "needs the installed DoMINO model, a CUDA GPU and WSL (set VERNON_AI3D_ROOT)"]
fn real_domino_surface_and_volume_on_standins() {
    let dir = e2e_dir("domino");
    let n = AeroPreviewNode;
    let a = ellipsoid(64, 40, DVec3::new(2300.0, 950.0, 780.0), DVec3::new(0.0, 0.0, 780.0));
    let mut p = real_params(&n);
    p.set("mode", ParamValue::Str("both".into()));
    let t0 = std::time::Instant::now();
    let out = n.cook(&ctx(&dir), &[Some(Data::geometry(a.clone()))], &p).unwrap();
    let wall = t0.elapsed().as_secs_f64();
    let Data::Report(r) = &out[2] else { panic!() };
    println!("wall {wall:.1}s\n{r:#?}");
    let Data::Json(j) = &out[3] else { panic!() };
    println!("{}", serde_json::to_string_pretty(&**j).unwrap());
    assert_eq!(j["status"], "complete");
    assert!(r.passes(), "{r:?}");
    let Data::Geometry(surf) = &out[0] else { panic!() };
    let Data::Geometry(slice) = &out[1] else { panic!() };
    assert_eq!(surf.prim_attribs.floats("ai_p").unwrap().len(), a.prims.len());
    assert!(slice.points.len() > 100);
    assert!(surf.prim_attribs.floats("ai_p").unwrap().iter().all(|v| v.is_finite()));
    // A second variant (wider) through the compare node.
    let b = ellipsoid(64, 40, DVec3::new(2300.0, 1000.0, 700.0), DVec3::new(0.0, 0.0, 700.0));
    let outb = n.cook(&ctx(&dir), &[Some(Data::geometry(b))], &p).unwrap();
    let Data::Json(jb) = &outb[3] else { panic!() };
    let c = AeroCompareNode;
    let cmp = c.cook(&ctx(&dir), &[Some(Data::Json(j.clone())), Some(Data::Json(jb.clone()))], &params_for(&c)).unwrap();
    let Data::Report(cr) = &cmp[0] else { panic!() };
    println!("{cr:#?}");
    assert!(cr.metrics.contains_key("ai_delta_surface_pressure_mean"));
}

#[test]
#[ignore = "needs the installed DoMINO model, a CUDA GPU and WSL (set VERNON_AI3D_ROOT)"]
fn real_domino_large_mesh_memory() {
    let dir = e2e_dir("domino_large");
    let n = AeroPreviewNode;
    let nu: usize = std::env::var("VERNON_AI_E2E_NU").ok().and_then(|v| v.parse().ok()).unwrap_or(100);
    let nv: usize = std::env::var("VERNON_AI_E2E_NV").ok().and_then(|v| v.parse().ok()).unwrap_or(100);
    let big = ellipsoid(nu, nv, DVec3::new(2300.0, 950.0, 780.0), DVec3::new(0.0, 0.0, 780.0));
    println!("triangles ~ {}", big.triangles().len());
    let mut p = real_params(&n);
    p.set("mode", ParamValue::Str(std::env::var("VERNON_AI_E2E_MODE").unwrap_or_else(|_| "surface".into())));
    let t0 = std::time::Instant::now();
    let out = n.cook(&ctx(&dir), &[Some(Data::geometry(big))], &p).unwrap();
    println!("wall {:.1}s", t0.elapsed().as_secs_f64());
    let Data::Json(j) = &out[3] else { panic!() };
    println!("{}", serde_json::to_string_pretty(&**j).unwrap());
    assert_eq!(j["status"], "complete");
}

#[test]
#[ignore = "needs the installed PartCrafter model, a CUDA GPU and WSL (set VERNON_AI3D_ROOT)"]
fn real_partcrafter_object_to_parts() {
    let dir = e2e_dir("partcrafter");
    let root = std::env::var(bridge::ROOT_ENV).expect("VERNON_AI3D_ROOT");
    let image = PathBuf::from(&root).join("PartCrafter/source/assets/images/np3_2f6ab901c5a84ed6bbdf85a67b22a2ee.png");
    let n = PartCrafterNode;
    let mut p = real_params(&n);
    p.set("image", ParamValue::Str(image.to_string_lossy().into()));
    let t0 = std::time::Instant::now();
    let out = n.cook(&ctx(&dir), &[], &p).unwrap();
    println!("wall {:.1}s", t0.elapsed().as_secs_f64());
    let Data::Report(r) = &out[1] else { panic!() };
    println!("{r:#?}");
    let Data::Json(m) = &out[2] else { panic!() };
    println!("{}", serde_json::to_string_pretty(&**m).unwrap());
    assert_eq!(m["status"], "complete");
    let Data::Geometry(g) = &out[0] else { panic!() };
    assert_eq!(g.prim_groups.len(), 3);
    // each part can be selected and then checked for the aero preview (it is not a car: expect a domain warning)
    let sel = parts::extract_part(g, 1).unwrap().0;
    let a = AeroPreviewNode;
    let rep = a.cook(&ctx(&dir), &[Some(Data::geometry(sel))], &params_for(&a)).unwrap();
    let Data::Report(ar) = &rep[2] else { panic!() };
    println!("{ar:#?}");
    assert!(ar.notes.iter().any(|(s, _)| *s >= Severity::Warning));
}

#[test]
fn surface_memory_model_matches_the_measurements() {
    // measured on the project's 16 GB GPU: 4,992 triangles -> 1.29 GB, 19,800 -> 6.35 GB
    assert!((aero::surface_peak_vram_gb(4992) - 1.29).abs() < 0.05);
    assert!((aero::surface_peak_vram_gb(19800) - 6.35).abs() < 0.1);
    // 33,540 triangles failed on that GPU and the model says it needs more than 16 GB
    assert!(aero::surface_peak_vram_gb(33540) > 16.0);
    assert_eq!(prep::MAX_TRIANGLES, 20_000);
    let big = ellipsoid(130, 130, DVec3::new(2300.0, 950.0, 780.0), DVec3::new(0.0, 0.0, 780.0));
    assert!(prep::prepare(&big, &PrepOptions::default()).unwrap_err().contains("decimate"));
    let ok = ellipsoid(100, 100, DVec3::new(2300.0, 950.0, 780.0), DVec3::new(0.0, 0.0, 780.0));
    assert!(prep::prepare(&ok, &PrepOptions { allow_outside: true, ..Default::default() }).is_ok());
}

#[test]
fn the_apps_cook_cancel_flag_stops_a_model_job() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let _g = PROCESS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let d = tmp("appcancel");
    let flag = Arc::new(AtomicBool::new(false));
    let f2 = flag.clone();
    let h = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(700));
        f2.store(true, Ordering::Relaxed);
    });
    let slow = script(&d, "slow2", "ping -n 30 127.0.0.1 >nul", "sleep 30");
    let t0 = std::time::Instant::now();
    let r = vernon_core::process::with_cancel(flag, || bridge::run_process(slow, &d, Duration::from_secs(60), &|| {}));
    h.join().unwrap();
    assert_eq!(r, Err(bridge::BridgeError::Cancelled));
    assert!(t0.elapsed() < Duration::from_secs(10));
}

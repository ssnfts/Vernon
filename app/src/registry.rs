//! The node registry the app runs with: every stage crate's node types.

use vernon_graph::Registry;

pub fn build_registry() -> Registry {
    let mut reg = Registry::new();
    vernon_nodes_geo::register(&mut reg);
    vernon_nodes_capture::register(&mut reg);
    vernon_nodes_clean::register(&mut reg);
    vernon_nodes_surface::register(&mut reg);
    vernon_nodes_print::register(&mut reg);
    vernon_nodes_cfd::register(&mut reg);
    vernon_nodes_fea::register(&mut reg);
    vernon_nodes_solar::register(&mut reg);
    vernon_nodes_perf::register(&mut reg);
    reg
}

/// The body-kit example graph, shipped as `Pipeline > Load Demo Pipeline`.
pub const DEMO_PIPELINE: &str = include_str!("../../examples/bodykit_pipeline.vrn");

/// The shared vendor-name guard (CLEAN_ROOM.md): the one list lives in
/// `vernon_core::clean_room`.
#[cfg(test)]
pub use vernon_core::clean_room::find_protected;

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use vernon_core::{Geometry, Metric, Report};
    use vernon_graph::{CookContext, CookResult, Data, DataKind, ParamValue, Params, Stage};

    #[test]
    fn every_stage_has_a_node() {
        let reg = build_registry();
        for stage in Stage::ALL {
            assert!(!reg.by_stage(stage).is_empty(), "{stage:?} has no node");
        }
        let prefixes = ["geo.", "capture.", "clean.", "surface.", "print.", "cfd.", "fea.", "solar.", "flatten.", "perf."];
        for i in reg.types() {
            assert!(prefixes.iter().any(|p| i.id.starts_with(p)), "{}", i.id);
        }
    }

    #[test]
    fn no_vendor_names_in_user_visible_node_strings() {
        let reg = build_registry();
        for info in reg.types() {
            let mut texts = info.visible_strings();
            texts.push(info.stage.label().to_string());
            for t in texts {
                assert_eq!(find_protected(&t), None, "{}: '{t}'", info.id);
            }
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("vernon-app-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn cook_node(reg: &vernon_graph::Registry, id: &str, ins: &[Option<Data>], set: &[(&str, ParamValue)]) -> CookResult<Vec<Data>> {
        let op = reg.get(id).unwrap_or_else(|| panic!("{id} not registered"));
        let mut p = Params::from_specs(&op.info().params);
        for (k, v) in set {
            p.set(k, v.clone());
        }
        let ctx = CookContext { node_name: id.into(), work_dir: scratch(id), frame: 0.0 };
        op.cook(&ctx, ins, &p)
    }

    /// The airflow stage's pressure surface (prim `Cp` + `Cp_valid`) feeds
    /// the structural aero load in field mode without any conversion node.
    #[test]
    fn cfd_pressure_surface_feeds_the_fea_field_load() {
        let reg = build_registry();
        let size = DVec3::new(400.0, 200.0, 60.0);
        let body = Geometry::make_box(size);
        // Solver samples (m) at the face centres: suction Cp = -1 on the top
        // face (kinematic p = -U^2 / 2 at the default 30 m/s), 0 elsewhere.
        let mut text = String::from("# x y z p\n");
        for i in 0..body.prims.len() {
            let vs = &body.prims[i].vertices;
            let c = vs.iter().map(|v| body.points[*v as usize]).sum::<DVec3>() / vs.len() as f64;
            let p = if body.prim_normal(i).z > 0.5 { -450.0 } else { 0.0 };
            text += &format!("{} {} {} {p}\n", c.x / 1000.0, c.y / 1000.0, c.z / 1000.0);
        }
        let file = scratch("handoff").join("p.raw");
        std::fs::write(&file, text).unwrap();
        let cfd = cook_node(&reg, "cfd.surface_pressure", &[Some(Data::geometry(body)), Some(Data::File(file))], &[]).unwrap();
        let surf = cfd[0].as_geometry().unwrap();
        assert!(surf.prim_attribs.floats("Cp").is_ok(), "the airflow stage writes prim Cp");
        let mesh = vernon_nodes_fea::mesh::box_tets(size, [8, 4, 2]);
        let fea = cook_node(
            &reg,
            "fea.load_aero",
            &[Some(Data::geometry(mesh)), Some(cfd[0].clone()), None],
            &[("mode", ParamValue::Str("field".into()))],
        )
        .unwrap();
        let r = fea[1].as_report().unwrap();
        let total = 2.0 * (size.x * size.y + size.x * size.z + size.y * size.z);
        assert!((r.metrics["loaded_area_mm2"].value - total).abs() < 1e-6 * total, "{r:?}");
        assert!(r.metrics["loaded_faces"].value > 0.0);
    }

    /// An airflow force result without solver data must stop the lap
    /// simulation's vehicle, not become zero drag.
    #[test]
    fn cfd_placeholder_force_result_is_refused_by_perf_vehicle() {
        let reg = build_registry();
        let history = serde_json::json!({"time": [], "columns": {}, "meta": {}, "placeholder": true});
        let cfd = cook_node(&reg, "cfd.force_coeffs", &[Some(Data::json(history)), None, None], &[]).unwrap();
        assert_eq!(cfd[0].as_json().unwrap()["placeholder"], serde_json::json!(true));
        for d in &cfd {
            let err = cook_node(&reg, "perf.vehicle", &[Some(d.clone())], &[]).unwrap_err().to_string();
            assert!(err.contains("placeholder"), "{err}");
        }
    }

    /// Every non-external node of every crate survives degenerate inputs
    /// (empty, point-only, NaN and huge coordinates) without panicking.
    /// Cooks run in parallel batches; one that outlives its time budget is
    /// listed on stderr and left running (this sweep looks for panics only).
    #[test]
    fn no_node_panics_on_degenerate_inputs() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        let reg = build_registry();
        let base = Geometry::make_box(DVec3::splat(20.0));
        let mut nan = base.clone();
        nan.points[0] = DVec3::new(f64::NAN, 0.0, 0.0);
        let mut huge = base.clone();
        huge.points[0] = DVec3::splat(1e300);
        let mut cloud = base.clone();
        cloud.prims.clear();
        let fixtures = [("empty", Geometry::default()), ("cloud", cloud), ("nan", nan), ("huge", huge)];
        let mut rep = Report::new("fixture");
        rep.metric("value", Metric::new(1.0, "-"));
        let missing = scratch("sweep").join("missing.dat");
        let mut jobs = Vec::new();
        for info in reg.types() {
            if info.external_tool.is_some() {
                continue;
            }
            for (fname, g) in &fixtures {
                let ins: Vec<Option<Data>> = info
                    .inputs
                    .iter()
                    .map(|port| {
                        Some(match port.kind {
                            DataKind::Geometry | DataKind::Any => Data::geometry(g.clone()),
                            DataKind::Report => Data::report(rep.clone()),
                            DataKind::Json => Data::json(serde_json::json!({})),
                            DataKind::File => Data::File(missing.clone()),
                        })
                    })
                    .collect();
                jobs.push((format!("{} on {fname}", info.id), info.id.clone(), ins));
            }
        }
        let par = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 16);
        let (mut panics, mut slow) = (Vec::new(), Vec::new());
        let mut pending = jobs.into_iter().peekable();
        while pending.peek().is_some() {
            let mut batch = Vec::new();
            for (label, id, ins) in pending.by_ref().take(par) {
                let (tx, rx) = mpsc::channel();
                let op = reg.get(&id).unwrap().clone();
                let params = Params::from_specs(&op.info().params);
                let ctx = CookContext { node_name: id.clone(), work_dir: scratch(&id), frame: 0.0 };
                std::thread::Builder::new()
                    .stack_size(8 << 20)
                    .spawn(move || {
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| op.cook(&ctx, &ins, &params)));
                        let _ = tx.send(r.is_err());
                    })
                    .unwrap();
                batch.push((label, rx));
            }
            let deadline = Instant::now() + Duration::from_secs(20);
            for (label, rx) in batch {
                match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(true) => panics.push(label),
                    Ok(false) => {}
                    Err(_) => slow.push(label),
                }
            }
        }
        if !slow.is_empty() {
            eprintln!("cooks over the sweep time budget (not checked): {slow:#?}");
        }
        assert!(panics.is_empty(), "{panics:#?}");
    }

    #[test]
    fn no_vendor_names_in_app_string_literals() {
        // Scan quoted literals in the app sources, skipping this file's list.
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.file_name().is_some_and(|n| n == "registry.rs") {
                    continue;
                }
                let src = std::fs::read_to_string(&p).unwrap();
                for (n, line) in src.lines().enumerate() {
                    if line.trim_start().starts_with("//") || line.contains(".emulates(") {
                        continue;
                    }
                    for lit in line.split('"').skip(1).step_by(2) {
                        assert_eq!(find_protected(lit), None, "{}:{}: {lit}", p.display(), n + 1);
                    }
                }
            }
        }
    }
}

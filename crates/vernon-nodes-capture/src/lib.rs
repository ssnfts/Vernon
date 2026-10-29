//! Stage 1 Capture: scan import, photogrammetry (COLMAP), scale correction, multi-scan registration.
//!
//! Every tool in the capture step is a node here. The numeric hand-off to
//! stage 2 is the `Report` each node emits (scale uncertainty, drift budget,
//! registration RMS) together with the `metric` detail attribute on the
//! geometry, which stays `0` until a scale node has run.

use vernon_graph::Registry;

pub mod accuracy;
pub mod colmap;
pub mod colmap_model;
pub mod import;
pub mod linalg;
pub mod registration;
mod global_registration;
mod pose_graph;
pub mod scale;
pub mod spatial;
pub mod tables;
pub mod triangulate;
pub mod units;
pub mod util;

/// Register every node type in this crate.
pub fn register(reg: &mut Registry) {
    // Import and units.
    reg.add(import::ImportScan);
    reg.add(import::ImportPhoneScan);
    reg.add(import::SetUnits);
    reg.add(import::MarkerTable);
    // Photogrammetry drivers.
    reg.add(colmap::SfmFeaturesMatch);
    reg.add(colmap::SfmReconstruct);
    reg.add(colmap::MvsDense);
    reg.add(colmap::MvsMesh);
    reg.add(triangulate::MarkerTriangulate);
    // Scale and marker alignment.
    reg.add(scale::ScaleFromDistance);
    reg.add(scale::ScaleBarSolve);
    reg.add(scale::SimilarityAlign);
    // Multi-scan registration.
    reg.add(registration::IcpPair);
    reg.add(registration::GlobalAlign);
    reg.add(registration::MultiwayAlign);
    reg.add(registration::MergeScans);
    reg.add(registration::MarkerMerge);
    // Error model and verification.
    reg.add(accuracy::ScannerProfile);
    reg.add(accuracy::DriftReport);
    reg.add(accuracy::ErrorBudget);
    reg.add(accuracy::LengthCheck);
    reg.add(accuracy::ScanQuality);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use glam::DVec3;
    use vernon_core::{AttribArray, Geometry, Severity};
    use vernon_graph::{CookContext, Data, DataKind, Params, Registry, Stage};

    use super::*;

    fn registry() -> Registry {
        let mut r = Registry::new();
        register(&mut r);
        r
    }

    /// Words that must never reach a user-visible string (CLEAN_ROOM.md).
    const VENDOR_WORDS: &[&str] = &[
        "rhino", "fusion", "autodesk", "3ds max", "quicksurface", "geomagic", "zeiss", "airshaper", "simscale", "ansys",
        "optimumlap", "plasticity", "polycam", "ladybug", "cloudcompare", "houdini", "revopoint", "einscan", "creality",
        "open3d", "colmap", "shining", "miraco", "trackit",
    ];

    fn visible_strings(info: &vernon_graph::NodeTypeInfo) -> Vec<String> {
        let mut v = vec![info.label.clone(), info.help.clone()];
        for p in info.inputs.iter().chain(info.outputs.iter()) {
            v.push(p.name.clone());
        }
        for p in &info.params {
            v.push(p.name.clone());
            v.push(p.label.clone());
            v.push(p.help.clone());
            if let Some(u) = &p.unit {
                v.push(u.clone());
            }
            if let vernon_graph::ParamKind::Enum { options, .. } = &p.kind {
                v.extend(options.iter().cloned());
            }
        }
        v
    }

    #[test]
    fn registration_is_complete_unique_and_documented() {
        let reg = registry();
        let expected = [
            "capture.import_scan",
            "capture.import_phone_scan",
            "capture.set_units",
            "capture.marker_table",
            "capture.sfm_features_match",
            "capture.sfm_reconstruct",
            "capture.mvs_dense",
            "capture.mvs_mesh",
            "capture.marker_triangulate",
            "capture.scale_from_distance",
            "capture.scale_bar_solve",
            "capture.similarity_align",
            "capture.icp_pair",
            "capture.global_align",
            "capture.multiway_align",
            "capture.merge_scans",
            "capture.marker_merge",
            "capture.scanner_profile",
            "capture.drift_report",
            "capture.error_budget",
            "capture.length_check",
            "capture.scan_quality",
        ];
        assert_eq!(reg.len(), expected.len());
        for id in expected {
            assert!(reg.info(id).is_some(), "missing {id}");
        }
        for info in reg.types() {
            assert!(info.id.starts_with("capture."), "{}", info.id);
            assert_eq!(info.stage, Stage::Capture);
            assert!(!info.help.is_empty(), "{} has no help", info.id);
            let expected_spec=if matches!(info.id.as_str(),"capture.global_align"|"capture.multiway_align") {
                "docs/specs/native-scan-registration.md"
            } else {"docs/specs/capture-scanning.md"};
            assert_eq!(info.spec.as_deref(), Some(expected_spec), "{}", info.id);
            assert!(!info.emulates.is_empty(), "{} has no emulates", info.id);
            assert!(!info.outputs.is_empty(), "{} has no outputs", info.id);
            for p in &info.params {
                assert!(!p.label.is_empty(), "{}.{} has no label", info.id, p.name);
            }
            // No vendor or program names in anything the user can read.
            for s in visible_strings(info) {
                let low = s.to_lowercase();
                for w in VENDOR_WORDS {
                    assert!(!low.contains(w), "{}: '{}' contains vendor word '{}'", info.id, s, w);
                }
            }
        }
    }

    #[test]
    fn white_box_and_external_flags_are_consistent() {
        let reg = registry();
        for id in [
            "capture.sfm_features_match",
            "capture.sfm_reconstruct",
            "capture.mvs_dense",
            "capture.mvs_mesh",
        ] {
            assert_eq!(reg.info(id).unwrap().external_tool.as_deref(), Some("COLMAP"), "{id}");
            assert!(reg.info(id).unwrap().params.iter().any(|p| p.name == "run"), "{id} needs a run switch");
        }
        for id in ["capture.mvs_mesh", "capture.sfm_features_match"] {
            assert!(reg.info(id).unwrap().whitebox, "{id} is a white-box node");
        }
        for id in [
            "capture.global_align",
            "capture.multiway_align",
            "capture.import_scan",
            "capture.scale_bar_solve",
            "capture.icp_pair",
            "capture.merge_scans",
            "capture.error_budget",
            "capture.length_check",
            "capture.marker_triangulate",
        ] {
            assert!(!reg.info(id).unwrap().whitebox, "{id} is implemented");
        }
    }

    fn ctx(name: &str) -> CookContext {
        let dir = std::env::temp_dir().join(format!("vernon-capture-smoke-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        CookContext {
            node_name: name.into(),
            work_dir: dir,
            frame: 0.0,
        }
    }

    /// A small curved patch with ids, normals and a scan tag, ~1 m across.
    fn synthetic_scan(offset: f64) -> Geometry {
        let mut pts = Vec::new();
        for i in 0..24 {
            for j in 0..16 {
                let (x, y) = (i as f64 * 25.0 + offset, j as f64 * 25.0);
                pts.push(DVec3::new(x, y, 30.0 * (x / 300.0).sin() * (y / 200.0).cos()));
            }
        }
        let mut g = Geometry::from_points(pts);
        let ids: Vec<String> = (0..g.point_count()).map(|i| i.to_string()).collect();
        g.point_attribs.set("id", AttribArray::Str(ids));
        util::set_metric_flag(&mut g, true);
        g
    }

    /// Plausible inputs for each port of a node, or `None` to leave it open.
    fn make_input(kind: DataKind, port: &str, index: usize) -> Option<Data> {
        match kind {
            DataKind::Geometry => Some(Data::geometry(synthetic_scan(index as f64 * 3.0))),
            DataKind::Report => None,
            DataKind::File => Some(Data::File(PathBuf::from("sparse/0"))),
            DataKind::Json | DataKind::Any => {
                let _ = port;
                None
            }
        }
    }

    #[test]
    fn every_node_cooks_on_synthetic_input_with_default_params() {
        let reg = registry();
        for info in reg.types().cloned().collect::<Vec<_>>() {
            let op = reg.get(&info.id).unwrap();
            let params = Params::from_specs(&info.params);
            let inputs: Vec<Option<Data>> = info
                .inputs
                .iter()
                .enumerate()
                .map(|(i, p)| if p.required || p.kind == DataKind::Geometry { make_input(p.kind, &p.name, i) } else { None })
                .collect();
            let c = ctx(&info.id.replace('.', "_"));
            match op.cook(&c, &inputs, &params) {
                Ok(out) => {
                    assert_eq!(out.len(), info.outputs.len(), "{}: output count", info.id);
                    for (d, spec) in out.iter().zip(&info.outputs) {
                        assert_eq!(d.kind(), spec.kind, "{}: output '{}' kind", info.id, spec.name);
                    }
                    for d in &out {
                        if let Data::Geometry(g) = d {
                            g.validate().unwrap_or_else(|e| panic!("{}: invalid geometry: {e}", info.id));
                        }
                    }
                    if info.whitebox {
                        let has_note = out.iter().any(|d| {
                            d.as_report().is_some_and(|r| {
                                r.notes.iter().any(|(s, m)| *s == Severity::Warning && (m.starts_with("white-box placeholder") || m.contains("run is off")))
                            })
                        });
                        assert!(has_note, "{}: white-box node must warn", info.id);
                    }
                }
                Err(e) => {
                    // A clean error is acceptable (missing data, nothing to do); a panic is not.
                    let msg = e.to_string();
                    assert!(!msg.is_empty(), "{}: empty error", info.id);
                }
            }
        }
    }

    #[test]
    fn scan_through_scale_to_budget_chain() {
        // Import-like cloud (non-metric) -> scale from a known distance -> budget.
        let reg = registry();
        let mut raw = synthetic_scan(0.0);
        util::set_metric_flag(&mut raw, false);
        let mut picks = Geometry::from_points(vec![DVec3::ZERO, DVec3::new(100.0, 0.0, 0.0)]);
        util::set_metric_flag(&mut picks, false);
        let op = reg.get("capture.scale_from_distance").unwrap();
        let info = reg.info("capture.scale_from_distance").unwrap();
        let mut p = Params::from_specs(&info.params);
        p.set("true_length", vernon_graph::ParamValue::Float(150.0));
        p.set("uncertainty", vernon_graph::ParamValue::Float(0.02));
        let out = op
            .cook(&ctx("chain1"), &[Some(Data::geometry(raw)), Some(Data::geometry(picks))], &p)
            .unwrap();
        let scaled = out[0].as_geometry().unwrap();
        assert_eq!(util::metric_flag(scaled), Some(true));
        let scale_rep = out[1].as_report().unwrap().clone();
        assert!((scale_rep.metrics["scale_factor"].value - 1.5).abs() < 1e-12);
        let budget = reg.get("capture.error_budget").unwrap();
        let binfo = reg.info("capture.error_budget").unwrap();
        let out = budget
            .cook(
                &ctx("chain2"),
                &[None, Some(Data::Report(scale_rep)), None],
                &Params::from_specs(&binfo.params),
            )
            .unwrap();
        let rep = out[0].as_report().unwrap();
        // 20 ppm x 4.5 m = 0.09 mm from the scale term.
        assert!(rep.metrics["e_scale"].value > 0.0 && rep.metrics["e_total"].value > 0.2);
    }
}

//! Registration and smoke tests covering every node type of the crate.

use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};

use glam::{DMat3, DVec3};
use serde_json::json;
use vernon_core::{AttribArray, Geometry};
use vernon_graph::{Data, DataKind, NodeTypeInfo, ParamKind, Params, Registry, Stage};

use crate::testkit::{ctx, ellipsoid_cloud, sphere_geo};
use crate::util::{transform_json, SPEC_ALIGN, SPEC_CAPTURE, SPEC_MESH};

fn registry() -> Registry {
    let mut r = Registry::new();
    crate::register(&mut r);
    r
}

const EXPECTED: &[&str] = &[
    "clean.pair_align",
    "clean.auto_prealign",
    "clean.icp_align",
    "clean.apply_transform",
    "clean.sample_surface",
    "clean.cloud_distance",
    "clean.mesh_distance",
    "clean.deviation_stats",
    "clean.deviation_histogram",
    "clean.deviation_colors",
    "clean.tolerance_gate",
    "clean.outlier_remove",
    "clean.radius_filter",
    "clean.voxel_subsample",
    "clean.random_subsample",
    "clean.denoise_points",
    "clean.estimate_normals",
    "clean.decimate",
    "clean.decimate_adaptive",
    "clean.fill_boundary_holes",
    "clean.smooth_mesh",
    "clean.isotropic_remesh",
    "clean.watertight_wrap",
    "clean.alpha_wrap",
    "clean.symmetry_detect",
    "clean.mirror_complete",
];

/// Vendor and product names that must stay out of user-visible text.
const TRADEMARKS: &[&str] = &[
    "rhino", "fusion", "3ds", "quicksurface", "geomagic", "zeiss", "airshaper", "simscale", "ansys", "optimumlap", "plasticity", "polycam", "ladybug", "cloudcompare", "houdini",
    "autodesk", "blender", "solidworks", "sidefx", "mcneel", "cgal", "meshlab", "open3d",
];

fn assert_clean(what: &str, text: &str) {
    let low = text.to_lowercase();
    for t in TRADEMARKS {
        assert!(!low.contains(t), "{what} contains '{t}': {text}");
    }
}

#[test]
fn registration_is_complete_unique_and_documented() {
    let reg = registry();
    assert_eq!(reg.len(), EXPECTED.len(), "node count");
    let mut ids = HashSet::new();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for info in reg.types() {
        assert!(ids.insert(info.id.clone()), "duplicate id {}", info.id);
        assert!(info.id.starts_with("clean."), "{} lacks the clean. prefix", info.id);
        assert_eq!(info.stage, Stage::Clean, "{}", info.id);
        assert!(!info.help.trim().is_empty(), "{} has no help", info.id);
        assert!(!info.label.trim().is_empty(), "{} has no label", info.id);
        assert!(!info.emulates.is_empty(), "{} has no emulates", info.id);
        assert!(!info.outputs.is_empty(), "{} has no outputs", info.id);
        let spec = info.spec.as_deref().unwrap_or_else(|| panic!("{} has no spec", info.id));
        assert!([SPEC_ALIGN, SPEC_MESH, SPEC_CAPTURE].contains(&spec), "{} spec {spec}", info.id);
        assert!(root.join(spec).is_file(), "spec file {spec} of {} is missing", info.id);
        assert_clean(&info.id, &info.label);
        assert_clean(&info.id, &info.help);
        let mut names = HashSet::new();
        for p in &info.params {
            assert!(names.insert(p.name.clone()), "{} duplicate param {}", info.id, p.name);
            assert!(!p.help.trim().is_empty(), "{}.{} has no help", info.id, p.name);
            assert_clean(&info.id, &p.label);
            assert_clean(&info.id, &p.help);
            match &p.kind {
                ParamKind::Enum { default, options } => {
                    assert!(options.contains(default), "{}.{} default not an option", info.id, p.name);
                    options.iter().for_each(|o| assert_clean(&info.id, o));
                }
                ParamKind::Float { default, min, max } => {
                    assert!(min <= default && default <= max, "{}.{} default out of range", info.id, p.name)
                }
                ParamKind::Int { default, min, max } => {
                    assert!(min <= default && default <= max, "{}.{} default out of range", info.id, p.name)
                }
                _ => {}
            }
        }
        for port in info.inputs.iter().chain(&info.outputs) {
            assert_clean(&info.id, &port.name);
        }
    }
    for id in EXPECTED {
        assert!(ids.contains(*id), "missing {id}");
    }
    // only the placeholder is a white-box stub; everything else is real
    let wb: Vec<&str> = reg.types().filter(|i| i.whitebox).map(|i| i.id.as_str()).collect();
    assert_eq!(wb, vec!["clean.alpha_wrap"]);
}

// ------------------------------------------------------------------ smoke

fn with_dist(mut g: Geometry) -> Geometry {
    let d: Vec<f64> = (0..g.points.len()).map(|i| ((i % 11) as f64 - 5.0) * 0.1).collect();
    g.point_attribs.set("dist", AttribArray::Float(d));
    g
}

fn input_sets() -> Vec<(&'static str, Geometry)> {
    vec![
        ("mesh", sphere_geo(50.0, 24, 25)),
        ("cloud", ellipsoid_cloud(40.0, 25.0, 15.0, 1500, 3)),
        ("dist_cloud", with_dist(ellipsoid_cloud(40.0, 25.0, 15.0, 400, 4))),
        ("dist_mesh", with_dist(sphere_geo(30.0, 12, 13))),
        ("empty", Geometry::new()),
        ("single_point", Geometry::from_points(vec![DVec3::new(1.0, 2.0, 3.0)])),
    ]
}

fn json_for(port: &str) -> Option<Data> {
    match port {
        "pairs" => Some(Data::json(json!([
            [[0.0, 0.0, 0.0], [0.0, 0.0, 0.0]],
            [[10.0, 0.0, 0.0], [10.0, 0.0, 0.0]],
            [[0.0, 10.0, 0.0], [0.0, 10.0, 0.0]],
            [[0.0, 0.0, 10.0], [0.0, 0.0, 10.0]]
        ]))),
        "transform" => Some(Data::json(transform_json("test", DMat3::IDENTITY, DVec3::new(1.0, 0.0, 0.0), 1.0))),
        "plane" => Some(Data::json(json!({"schema": "plane.v1", "origin": [0.0, 0.0, 0.0], "normal": [1.0, 0.0, 0.0]}))),
        "stats" => {
            // a real statistics record for the gate
            let reg = registry();
            let info = reg.info("clean.deviation_stats").unwrap().clone();
            let g = with_dist(ellipsoid_cloud(40.0, 25.0, 15.0, 400, 4));
            let out = reg
                .get("clean.deviation_stats")
                .unwrap()
                .cook(&ctx("smoke_stats"), &[Some(Data::geometry(g))], &Params::from_specs(&info.params))
                .unwrap();
            Some(out[1].clone())
        }
        _ => None,
    }
}

fn cook_all(info: &NodeTypeInfo, reg: &Registry, g: &Geometry) -> std::thread::Result<vernon_graph::CookResult<Vec<Data>>> {
    let ins: Vec<Option<Data>> = info
        .inputs
        .iter()
        .map(|p| match p.kind {
            DataKind::Geometry | DataKind::Any => Some(Data::geometry(g.clone())),
            DataKind::Json => json_for(&p.name),
            _ => None,
        })
        .collect();
    let op = reg.get(&info.id).unwrap().clone();
    let params = Params::from_specs(&info.params);
    let c = ctx(&format!("smoke_{}", info.id));
    catch_unwind(AssertUnwindSafe(|| op.cook(&c, &ins, &params)))
}

#[test]
fn smoke_every_node_cooks_with_defaults_or_fails_cleanly() {
    let reg = registry();
    for info in reg.types() {
        let mut cooked_ok = 0;
        for (name, g) in input_sets() {
            match cook_all(info, &reg, &g) {
                Err(_) => panic!("{} panicked on input '{name}'", info.id),
                Ok(Ok(out)) => {
                    cooked_ok += 1;
                    assert_eq!(out.len(), info.outputs.len(), "{} on {name}: output count", info.id);
                    for (d, port) in out.iter().zip(&info.outputs) {
                        assert_eq!(d.kind(), port.kind, "{} on {name}: port {}", info.id, port.name);
                        if let Data::Geometry(g) = d {
                            g.validate().unwrap_or_else(|e| panic!("{} on {name}: {e}", info.id));
                            assert!(g.points.iter().all(|p| p.is_finite()), "{} on {name}: non-finite point", info.id);
                        }
                    }
                }
                Ok(Err(e)) => {
                    // a clean error must carry a message
                    assert!(!e.to_string().trim().is_empty(), "{} on {name}: empty error", info.id);
                }
            }
        }
        assert!(cooked_ok >= 1, "{} cooked on none of the synthetic inputs", info.id);
    }
}

#[test]
fn every_implemented_node_reports_something_numeric_or_geometric() {
    // each node's report output (when present) is a Report with a title
    let reg = registry();
    for info in reg.types() {
        let Some(ri) = info.outputs.iter().position(|o| o.kind == DataKind::Report) else { continue };
        for (name, g) in input_sets() {
            if let Ok(Ok(out)) = cook_all(info, &reg, &g) {
                let r = out[ri].as_report().unwrap();
                assert!(!r.title.trim().is_empty(), "{} on {name}: untitled report", info.id);
                break;
            }
        }
    }
}

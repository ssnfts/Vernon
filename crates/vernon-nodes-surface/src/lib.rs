//! Stage 3 Mesh to Surface: quad remesh, SubD, auto-surfacing, mesh sections, curve fit, loft, STEP hand-off.
//!
//! Node groups (all ids start with `surface.`):
//! - sections: `section_curves`, `fit_section_curves`, `loft_sections`, `curve_network_patch`
//! - regions and primitives: `segment_regions`, `refine_regions`, `fit_primitives`
//! - freeform: `fit_patch`, `auto_surface`
//! - subdivision: `mesh_to_subd`, `subd_refine`, `subd_to_nurbs`, `subd_deviation`, `mesh_to_g2_surface`
//! - quads: `quad_remesh`, `quad_mesh_report`
//! - solids and edits: `convert_faceted`, `convert_prismatic`, `sew`, `mirror_surface`, `thicken_shell`
//! - hand-off and gate: `deviation_check`, `write_step`
//!
//! Patch sets and curve sets travel between nodes as `Data::Json` using the
//! `nurbs_patch_set` and `nurbs_curve_set` schemas of [`nurbs`]. Every stage node
//! reports numbers the next stage can gate on (`dev_p95_mm`, `watertight`, ...).

#![allow(clippy::needless_range_loop, clippy::too_many_arguments, clippy::type_complexity)]

pub mod bvh;
pub mod convert;
pub mod curvefit;
pub mod deviation;
pub mod fit;
mod field_remesh;
mod remesh_fields;
mod remesh_symmetry;
mod patch_network;
mod autosurface;
pub mod linalg;
pub mod nurbs;
pub mod patchnodes;
pub mod patchops;
pub mod quad;
pub mod sections;
pub mod segment;
pub mod step;
pub mod subd;
pub mod subd_nurbs;
#[cfg(test)]
pub mod testutil;
pub mod topology;
pub mod util;

use vernon_graph::Registry;

/// Register every node type in this crate.
pub fn register(reg: &mut Registry) {
    reg.add(sections::SectionCurvesNode);
    reg.add(curvefit::FitSectionCurvesNode);
    reg.add(convert::ConvertFacetedNode);
    reg.add(convert::ConvertPrismaticNode);
    reg.add(convert::SewNode);
    reg.add(convert::MirrorSurfaceNode);
    reg.add(convert::ThickenShellNode);
    reg.add(deviation::DeviationCheckNode);
    reg.add(patchnodes::FitPatchNode);
    reg.add(patchnodes::AutoSurfaceNode);
    reg.add(patchnodes::CurveNetworkPatchNode);
    reg.add(patchnodes::LoftSectionsNode);
    reg.add(segment::SegmentRegionsNode);
    reg.add(segment::RefineRegionsNode);
    reg.add(segment::FitPrimitivesNode);
    reg.add(quad::QuadRemeshNode);
    reg.add(quad::QuadMeshReportNode);
    reg.add(step::WriteStepNode);
    reg.add(subd::MeshToSubdNode);
    reg.add(subd::SubdRefineNode);
    reg.add(subd_nurbs::SubdToNurbsNode);
    reg.add(subd_nurbs::SubdDeviationNode);
    reg.add(subd_nurbs::MeshToG2SurfaceNode);
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::Path;

    use glam::DVec3;
    use vernon_core::Geometry;
    use vernon_graph::{Data, DataKind, NodeTypeInfo, Stage};

    use crate::nurbs::{CurveEntry, CurveSet, NurbsCurve, NurbsSurface, PatchSet};
    use crate::testutil::{cook, registry, some, sphere};

    /// Trademarks and program names that must never reach user-visible text.
    const VENDOR_WORDS: &[&str] = &[
        "rhino",
        "fusion",
        "autodesk",
        "3ds max",
        "quicksurface",
        "geomagic",
        "design x",
        "revo design",
        "zeiss",
        "airshaper",
        "simscale",
        "ansys",
        "optimumlap",
        "plasticity",
        "polycam",
        "ladybug",
        "cloudcompare",
        "houdini",
        "parasolid",
        "solidworks",
        "instant meshes",
        "quadriflow",
        "mcneel",
        "colmap",
        "openfoam",
        "calculix",
        "orcaslicer",
    ];

    const EXPECTED: &[&str] = &[
        "surface.section_curves",
        "surface.fit_section_curves",
        "surface.loft_sections",
        "surface.segment_regions",
        "surface.refine_regions",
        "surface.fit_primitives",
        "surface.fit_patch",
        "surface.curve_network_patch",
        "surface.auto_surface",
        "surface.quad_remesh",
        "surface.quad_mesh_report",
        "surface.mesh_to_subd",
        "surface.subd_refine",
        "surface.subd_to_nurbs",
        "surface.subd_deviation",
        "surface.mesh_to_g2_surface",
        "surface.convert_faceted",
        "surface.convert_prismatic",
        "surface.sew",
        "surface.mirror_surface",
        "surface.thicken_shell",
        "surface.deviation_check",
        "surface.write_step",
    ];

    fn visible_strings(info: &NodeTypeInfo) -> Vec<String> {
        let mut v = vec![info.label.clone(), info.help.clone()];
        for p in &info.params {
            v.push(p.label.clone());
            v.push(p.help.clone());
            if let vernon_graph::ParamKind::Enum { options, .. } = &p.kind {
                v.extend(options.clone());
            }
        }
        for p in info.inputs.iter().chain(&info.outputs) {
            v.push(p.name.clone());
        }
        v
    }

    #[test]
    fn registration_is_complete_unique_and_documented() {
        let reg = registry();
        assert_eq!(reg.len(), EXPECTED.len());
        for id in EXPECTED {
            assert!(reg.info(id).is_some(), "missing {id}");
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let mut seen = HashSet::new();
        for info in reg.types() {
            assert!(seen.insert(info.id.clone()), "duplicate {}", info.id);
            assert!(info.id.starts_with("surface."), "{}", info.id);
            assert_eq!(info.stage, Stage::Surface, "{}", info.id);
            let tail = &info.id["surface.".len()..];
            assert!(!tail.is_empty() && tail.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'), "bad id {}", info.id);
            assert!(!info.label.is_empty() && !info.help.is_empty(), "{} lacks label or help", info.id);
            assert!(!info.emulates.is_empty(), "{} has no emulates", info.id);
            assert!(!info.outputs.is_empty(), "{} has no outputs", info.id);
            let spec = info.spec.as_deref().unwrap_or_else(|| panic!("{} has no spec", info.id));
            assert!(root.join(spec).exists(), "{}: spec {} does not exist", info.id, spec);
            for p in &info.params {
                assert!(!p.label.is_empty() && !p.help.is_empty(), "{}.{} lacks label or help", info.id, p.name);
            }
            let names: HashSet<&str> = info.params.iter().map(|p| p.name.as_str()).collect();
            assert_eq!(names.len(), info.params.len(), "{} has duplicate parameter names", info.id);
            for s in visible_strings(info) {
                let low = s.to_lowercase();
                for w in VENDOR_WORDS {
                    assert!(!low.contains(w), "{}: '{}' contains vendor word '{}'", info.id, s, w);
                }
            }
        }
    }

    #[test]
    fn white_box_flags_are_honest() {
        let reg = registry();
        for id in [
            "surface.mesh_to_g2_surface",
            "surface.convert_prismatic",
            "surface.write_step",
        ] {
            assert!(reg.info(id).unwrap().whitebox, "{id} contains a placeholder");
        }
        for id in [
            "surface.section_curves",
            "surface.fit_section_curves",
            "surface.loft_sections",
            "surface.curve_network_patch",
            "surface.segment_regions",
            "surface.fit_patch",
            "surface.quad_remesh",
            "surface.auto_surface",
            "surface.refine_regions",
            "surface.mesh_to_subd",
            "surface.subd_refine",
            "surface.subd_to_nurbs",
            "surface.convert_faceted",
            "surface.sew",
            "surface.deviation_check",
        ] {
            assert!(!reg.info(id).unwrap().whitebox, "{id} is implemented");
        }
    }

    fn patch_set() -> PatchSet {
        let sheet = |x0: f64| {
            let mut pts = Vec::new();
            for i in 0..4 {
                for j in 0..4 {
                    pts.push(DVec3::new(x0 + 10.0 * i as f64 / 3.0, 10.0 * j as f64 / 3.0, ((i + j) as f64).sin()));
                }
            }
            NurbsSurface::clamped_uniform(3, 3, 4, 4, pts)
        };
        PatchSet::new(vec![sheet(0.0), sheet(10.0)])
    }

    fn curve(id: usize, section: i64, a: DVec3, b: DVec3) -> CurveEntry {
        CurveEntry { id, section_id: section, loop_id: 0, kind: "line".into(), closed: false, max_dev_mm: 0.0, nurbs: NurbsCurve::clamped_uniform(vec![a, b], 1), params: serde_json::Value::Null }
    }

    /// Natural inputs for a node, built from its port names.
    fn natural_inputs(info: &NodeTypeInfo) -> Vec<Option<Data>> {
        let ball = sphere(20.0, 16, 8);
        info.inputs
            .iter()
            .map(|p| match (p.kind, p.name.as_str()) {
                (DataKind::Geometry, "regions") => {
                    static CELL: std::sync::OnceLock<Data> = std::sync::OnceLock::new();
                    Some(CELL.get_or_init(|| cook("surface.segment_regions", &[some(&sphere(20.0, 16, 8))], &[]).unwrap()[0].clone()).clone())
                }
                (DataKind::Geometry, "sections") => {
                    static CELL: std::sync::OnceLock<Data> = std::sync::OnceLock::new();
                    Some(CELL.get_or_init(|| cook("surface.section_curves", &[some(&sphere(20.0, 16, 8))], &[]).unwrap()[0].clone()).clone())
                }
                (DataKind::Geometry, "cage") => some(&Geometry::make_box(DVec3::splat(20.0))),
                (DataKind::Geometry, "guides") => None,
                (DataKind::Geometry, _) if !p.required => None,
                (DataKind::Geometry, _) if info.id == "surface.fit_patch" => some(&crate::testutil::grid(20, 20, 100.0, 100.0, |x, y| 3.0 * (x / 30.0).sin() + 0.02 * y)),
                (DataKind::Geometry, _) => some(&ball),
                (DataKind::Json, "curves") if info.id == "surface.curve_network_patch" => {
                    let (a, b, c, d) = (DVec3::ZERO, DVec3::new(10.0, 0.0, 1.0), DVec3::new(10.0, 8.0, 0.0), DVec3::new(0.0, 8.0, 2.0));
                    Some(Data::json(CurveSet::new(vec![curve(0, 0, a, b), curve(1, 0, b, c), curve(2, 0, d, c), curve(3, 0, a, d)]).to_json()))
                }
                (DataKind::Json, "curves") => Some(Data::json(CurveSet::new(vec![curve(0, 0, DVec3::ZERO, DVec3::X * 10.0), curve(1, 1, DVec3::Z * 5.0, DVec3::new(10.0, 0.0, 5.0))]).to_json())),
                (DataKind::Json, _) => Some(Data::json(patch_set().to_json())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_node_cooks_on_synthetic_input_with_default_params() {
        let reg = registry();
        for info in reg.types() {
            let inputs = natural_inputs(info);
            let out = cook(&info.id, &inputs, &[]).unwrap_or_else(|e| panic!("{} failed on its natural input: {e}", info.id));
            assert_eq!(out.len(), info.outputs.len(), "{}: wrong output count", info.id);
            for (d, port) in out.iter().zip(&info.outputs) {
                assert_eq!(d.kind(), port.kind, "{}.{}: wrong output kind", info.id, port.name);
                if let Some(r) = d.as_report() {
                    for (name, m) in &r.metrics {
                        assert!(m.value.is_finite(), "{}: metric {name} is not finite", info.id);
                    }
                }
                if let Some(g) = d.as_geometry() {
                    g.validate().unwrap_or_else(|e| panic!("{}: invalid geometry output: {e}", info.id));
                    assert!(g.points.iter().all(|p| p.is_finite()), "{}: non-finite output points", info.id);
                }
            }
            if info.whitebox {
                // white-box nodes announce themselves in at least one report
                let announced = out.iter().filter_map(Data::as_report).any(|r| r.notes.iter().any(|(_, t)| t.starts_with("white-box placeholder")));
                let partial = ["surface.refine_regions"];
                assert!(announced || partial.contains(&info.id.as_str()), "{} is white-box but its report has no placeholder note", info.id);
            }
        }
    }

    #[test]
    fn unconnected_inputs_give_clean_errors_not_panics() {
        let reg = registry();
        for info in reg.types() {
            let none: Vec<Option<Data>> = vec![None; info.inputs.len()];
            // either a clean error or, for nodes that accept it, a result: never a panic
            let _ = cook(&info.id, &none, &[]);
        }
    }

    #[test]
    fn non_finite_geometry_is_rejected() {
        let reg = registry();
        let mut bad = sphere(20.0, 8, 4);
        bad.points[3].y = f64::NAN;
        for info in reg.types() {
            if !info.inputs.iter().any(|p| p.kind == DataKind::Geometry && p.required) {
                continue;
            }
            if matches!(info.id.as_str(), "surface.refine_regions" | "surface.convert_prismatic") {
                continue;
            }
            let mut inputs = natural_inputs(info);
            for (slot, port) in inputs.iter_mut().zip(&info.inputs) {
                if port.kind == DataKind::Geometry && port.required {
                    *slot = some(&bad);
                }
            }
            assert!(cook(&info.id, &inputs, &[]).is_err(), "{} accepted NaN coordinates", info.id);
        }
    }

    #[test]
    fn cooking_is_deterministic() {
        for id in ["surface.segment_regions", "surface.subd_refine", "surface.quad_remesh", "surface.auto_surface", "surface.deviation_check", "surface.fit_patch", "surface.subd_to_nurbs"] {
            let reg = registry();
            let inputs = natural_inputs(reg.info(id).unwrap());
            let a = cook(id, &inputs, &[]).unwrap();
            let b = cook(id, &inputs, &[]).unwrap();
            for (x, y) in a.iter().zip(&b) {
                match (x, y) {
                    (Data::Geometry(g1), Data::Geometry(g2)) => assert_eq!(**g1, **g2, "{id}: geometry differs between runs"),
                    (Data::Json(j1), Data::Json(j2)) => assert_eq!(**j1, **j2, "{id}: json differs between runs"),
                    (Data::Report(r1), Data::Report(r2)) => assert_eq!(**r1, **r2, "{id}: report differs between runs"),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn odd_geometry_never_panics() {
        use vernon_core::Prim;
        let tri = Geometry::from_polygons(vec![DVec3::ZERO, DVec3::X, DVec3::Y], vec![vec![0, 1, 2]]);
        let flat = Geometry::from_polygons(vec![DVec3::ZERO, DVec3::X, DVec3::X * 2.0], vec![vec![0, 1, 2]]);
        let dup = Geometry::from_polygons(vec![DVec3::ZERO, DVec3::X, DVec3::Y], vec![vec![0, 1, 2], vec![0, 1, 2]]);
        let pinched = Geometry::from_polygons(vec![DVec3::ZERO, DVec3::X, DVec3::Y, DVec3::ONE], vec![vec![0, 1, 1, 2], vec![0, 0, 0]]);
        let cloud = Geometry::from_points(vec![DVec3::ZERO, DVec3::X, DVec3::Y, DVec3::Z]);
        let lines = {
            let mut g = Geometry::from_points(vec![DVec3::ZERO, DVec3::X, DVec3::Y]);
            g.prims.push(Prim::polyline(vec![0, 1, 2]));
            g
        };
        let samples = [Geometry::new(), tri, flat, dup, pinched, cloud, lines];
        let reg = registry();
        for info in reg.types() {
            for g in &samples {
                let mut inputs = natural_inputs(info);
                for (slot, port) in inputs.iter_mut().zip(&info.inputs) {
                    if port.kind == DataKind::Geometry && port.name != "regions" && port.name != "sections" {
                        *slot = some(g);
                    }
                }
                let _ = cook(&info.id, &inputs, &[]);
            }
        }
    }

    #[test]
    fn odd_json_never_panics() {
        let empty_patches = PatchSet::new(vec![]).to_json();
        let empty_curves = CurveSet::new(vec![]).to_json();
        let one_pt_curve = CurveSet::new(vec![curve(0, 0, DVec3::ZERO, DVec3::ZERO)]).to_json();
        let samples = [serde_json::Value::Null, serde_json::json!({}), serde_json::json!({"schema": "nurbs_patch_set"}), empty_patches, empty_curves, one_pt_curve];
        let reg = registry();
        for info in reg.types() {
            for j in &samples {
                let mut inputs = natural_inputs(info);
                for (slot, port) in inputs.iter_mut().zip(&info.inputs) {
                    if port.kind == DataKind::Json {
                        *slot = Some(Data::json(j.clone()));
                    }
                }
                let _ = cook(&info.id, &inputs, &[]);
            }
        }
    }
}

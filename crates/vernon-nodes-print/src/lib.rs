//! Stage 4 Split/Print: body-line splitting, grid split, dovetail/dowel keys, bed fit, 3MF, slicer hand-off.

// The workspace targets Rust 1.80; newer clippy suggests APIs from later releases.
#![allow(unknown_lints)]
#![allow(clippy::unnecessary_map_or, clippy::manual_is_multiple_of, clippy::chunks_exact_to_as_chunks)]

pub mod common;
pub mod cut;
pub mod fit;
pub mod gcode;
pub mod hierarchy;
pub mod joints;
pub mod mesh;
pub mod nodes_cut;
pub mod nodes_fit;
pub mod nodes_joints;
pub mod nodes_pack;
pub mod nodes_split;
pub mod orient;
pub mod partition_search;
pub mod package;
pub mod poly;
pub mod polyline_project;
pub mod profile_schema;
pub mod script_bridge;
pub mod script_template;
pub mod slicer;
pub mod snap_geometry;
pub mod upload;
mod curtain;
mod gcode_check;
mod snap;
mod partition;
mod profiles;

use vernon_graph::Registry;

/// Register every node type in this crate.
pub fn register(reg: &mut Registry) {
    // cut and partition
    reg.add(nodes_cut::PlaneCut);
    reg.add(nodes_cut::PolylineCut);
    reg.add(nodes_cut::CutRegionSelect);
    reg.add(nodes_split::GridPartition);
    reg.add(nodes_split::SmartPartition);
    reg.add(nodes_split::Connectivity);
    reg.add(nodes_split::ByAttribute);
    reg.add(nodes_split::PartExtract);
    reg.add(nodes_split::PartLabel);
    // joints
    reg.add(nodes_joints::PlugJoint);
    reg.add(nodes_joints::DowelJoint);
    reg.add(nodes_joints::SnapJoint);
    reg.add(nodes_joints::DovetailJoint);
    // fit and orientation
    reg.add(nodes_fit::BuildFit);
    reg.add(nodes_fit::OrientScoreNode);
    reg.add(nodes_fit::DirectionalStrength);
    reg.add(nodes_fit::HeatMargin);
    reg.add(nodes_fit::StairStep);
    // package and hand-off
    reg.add(nodes_pack::PackageWrite);
    reg.add(nodes_pack::PackageRead);
    reg.add(nodes_pack::ProfileBuild);
    reg.add(nodes_pack::SliceExternal);
    reg.add(nodes_pack::GcodeSummaryNode);
    reg.add(nodes_pack::FlavourCheck);
    reg.add(nodes_pack::RemoteUpload);
    reg.add(nodes_pack::ScriptBridge);
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use glam::DVec3;
    use vernon_core::Geometry;
    use vernon_graph::{CookContext, Data, DataKind, Params, Registry, Stage};

    use crate::cut::{cut_all, Keep, Plane};
    use crate::mesh::testutil::tri_box;

    fn registry() -> Registry {
        let mut r = Registry::new();
        super::register(&mut r);
        r
    }

    #[test]
    fn registration_ids_prefixes_help_and_spec() {
        let reg = registry();
        let mut seen = HashSet::new();
        for info in reg.types() {
            assert!(seen.insert(info.id.clone()), "duplicate id {}", info.id);
            assert!(info.id.starts_with("print."), "prefix of {}", info.id);
            assert_eq!(info.stage, Stage::Print, "{}", info.id);
            assert!(!info.help.is_empty(), "help of {}", info.id);
            assert!(!info.emulates.is_empty(), "emulates of {}", info.id);
            assert_eq!(info.spec.as_deref(), Some("docs/specs/slicing-printing.md"), "spec of {}", info.id);
            assert!(!info.outputs.is_empty(), "outputs of {}", info.id);
            for pspec in &info.params {
                assert!(!pspec.help.is_empty(), "param help {}.{}", info.id, pspec.name);
            }
        }
        assert_eq!(reg.len(), 26);
    }

    #[test]
    fn user_visible_strings_carry_no_vendor_names() {
        let banned = ["rhino", "fusion", "autodesk", "3ds max", "quicksurface", "geomagic", "zeiss", "airshaper", "simscale", "ansys", "optimumlap", "plasticity", "polycam", "ladybug", "cloudcompare", "houdini", "orca", "prusa", "bambu"];
        for info in registry().types() {
            let mut texts = vec![info.label.clone(), info.help.clone()];
            for p in &info.params {
                texts.push(p.label.clone());
                texts.push(p.help.clone());
            }
            for t in texts {
                let low = t.to_ascii_lowercase();
                for b in banned {
                    assert!(!low.contains(b), "'{b}' in user-visible text of {}: {t}", info.id);
                }
            }
        }
    }

    fn capped_halves() -> Geometry {
        let cube = tri_box(DVec3::ZERO, DVec3::splat(100.0));
        let plane = Plane::new(DVec3::new(0.0, 0.0, 50.0), DVec3::Z).unwrap();
        cut_all(&cube, &[plane], 1e-6, true, Keep::Both).unwrap().geo
    }

    /// Every node cooks on a small input with default parameters: either the
    /// declared number of outputs or a clean error, never a panic.
    #[test]
    fn smoke_every_node_cooks_with_defaults() {
        let reg = registry();
        let dir = std::env::temp_dir().join(format!("vernon-print-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let gcode = dir.join("sample.gcode");
        std::fs::write(&gcode, "M83\n;LAYER_CHANGE\nG1 Z0.2\nG1 X10 E1\n").unwrap();
        let pkg = dir.join("sample.3mf");
        crate::package::write_package(&tri_box(DVec3::ZERO, DVec3::splat(10.0)), &pkg, vernon_io::FileUnit::Millimetre, true).unwrap();
        // The attribute splitter needs a numeric key attribute that a plain cut does not carry.
        let expected_errors = ["print.by_attribute"];
        let mut ok = 0;
        for info in reg.types() {
            let op = reg.get(&info.id).unwrap();
            let inputs: Vec<Option<Data>> = info
                .inputs
                .iter()
                .map(|port| match port.kind {
                    DataKind::Json if !port.required => None,
                    DataKind::Geometry if port.name == "guide" => {
                        let mut guide=Geometry::from_points(vec![DVec3::new(25.0,0.0,-50.0),DVec3::new(25.0,0.0,150.0)]);
                        guide.prims.push(vernon_core::Prim::polyline(vec![0,1]));
                        Some(Data::geometry(guide))
                    },
                    DataKind::Geometry => Some(Data::geometry(capped_halves())),
                    DataKind::Report => Some(Data::report(vernon_core::Report::new("in"))),
                    DataKind::File if port.name == "package" => Some(Data::File(pkg.clone())),
                    DataKind::File => Some(Data::File(gcode.clone())),
                    DataKind::Json => Some(Data::json(serde_json::json!({}))),
                    DataKind::Any => None,
                })
                .collect();
            let ctx = CookContext { node_name: info.id.clone(), work_dir: dir.join(info.id.replace('.', "_")), frame: 0.0 };
            let params = Params::from_specs(&info.params);
            match op.cook(&ctx, &inputs, &params) {
                Ok(out) => {
                    assert_eq!(out.len(), info.outputs.len(), "output count of {}", info.id);
                    for (d, port) in out.iter().zip(&info.outputs) {
                        assert_eq!(d.kind(), port.kind, "output kind of {} port {}", info.id, port.name);
                    }
                    ok += 1;
                }
                Err(e) => {
                    assert!(expected_errors.contains(&info.id.as_str()), "{} failed on its default input: {e}", info.id);
                }
            }
        }
        assert_eq!(ok, reg.len() - expected_errors.len());
    }
}

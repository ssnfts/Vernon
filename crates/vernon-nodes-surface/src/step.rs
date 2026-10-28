//! STEP export node, confined file effects and optional independent import test.
use crate::nurbs::PatchSet;
pub use crate::step_format::{write_step, Schema, StepOptions, StepSummary};
use crate::util::{check_finite, json_err, json_input, node_info, SPEC_RE};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use vernon_core::{Metric, Report, Severity};
use vernon_graph::{
    CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec,
};

/// `surface.write_step`
pub struct WriteStepNode;

impl NodeOp for WriteStepNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.write_step", "Write STEP", SPEC_RE)
            .input(PortSpec::optional("patches", DataKind::Json))
            .input(PortSpec::optional("mesh", DataKind::Geometry))
            .output(PortSpec::new("file", DataKind::File))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("schema", "Schema", "ap214", &["ap214", "ap242"]).help("Application protocol named in the file header."))
            .param(ParamSpec::string("name", "Part name", "vernon_surface").help("Name stored in the file and used for the file name."))
            .param(ParamSpec::path("save_to", "Save to", &["stp", "step"], true).help("Optional additional copy inside the node work directory. Relative paths are resolved there; outside paths are rejected."))
            .param(ParamSpec::int("max_faces", "Face limit", 500_000, 100, 50_000_000).help("Refuse faceted meshes with more faces than this."))
            .help("Writes AP214/AP242 STEP in millimetres, with planar faceted shells and whole-edge connected untrimmed B-spline faces. Structural validation checks references and topology before writing. The optional import test runs with: cargo test -p vernon-nodes-surface freecad_import -- --ignored; set FREECADCMD to the command-line importer executable.")
            .emulates("STEP export of surfaces and solids")
            .implemented()
    }

    fn cook(
        &self,
        ctx: &CookContext,
        inputs: &[Option<Data>],
        params: &Params,
    ) -> CookResult<Vec<Data>> {
        let mesh = vernon_graph::node::inputs::opt_geometry(inputs, 1);
        let pj = json_input(inputs, 0);
        if let Some(m) = &mesh {
            check_finite(ctx, m)?;
        }
        let ps = match pj {
            Some(j) => {
                Some(PatchSet::from_json(&j).map_err(|e| json_err(ctx, "patch set input", e))?)
            }
            None => None,
        };
        if let Some(p) = &ps {
            for patch in &p.patches {
                if patch.surface.control_points.iter().any(|c| !c.is_finite()) {
                    return Err(ctx.fail("a patch has non-finite control points"));
                }
            }
        }
        let opts = StepOptions {
            schema: if params.str("schema")? == "ap242" {
                Schema::Ap242
            } else {
                Schema::Ap214
            },
            name: params.str("name")?.trim().to_string(),
            max_faces: params.float("max_faces")? as usize,
        };
        if opts.name.is_empty() {
            return Err(ctx.fail("the part name must not be empty"));
        }
        let (content, sum) =
            write_step(mesh.as_deref(), ps.as_ref(), &opts).map_err(|e| ctx.fail(e))?;
        let validation = crate::step_validate::validate(&content)
            .map_err(|e| ctx.fail(format!("STEP structural validation failed: {e}")))?;
        if validation.entities != sum.entities {
            return Err(ctx.fail("STEP entity count does not match the writer summary"));
        }
        std::fs::create_dir_all(&ctx.work_dir)
            .map_err(|e| ctx.fail(format!("cannot create the work directory: {e}")))?;
        let file_name: String = opts
            .name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path =
            confined_target(&ctx.work_dir, &format!("{file_name}.stp")).map_err(|e| ctx.fail(e))?;
        let save = params.str("save_to")?.trim();
        let target = if save.is_empty() {
            None
        } else {
            Some(confined_target(&ctx.work_dir, save).map_err(|e| ctx.fail(e))?)
        };
        write_atomic(&path, &content)
            .map_err(|e| ctx.fail(format!("cannot write {}: {e}", path.display())))?;
        let mut report = Report::new("STEP export");
        report.metric("faces", Metric::new(sum.faces as f64, "count"));
        report.metric("patches", Metric::new(sum.patches as f64, "count"));
        report.metric("entities", Metric::new(sum.entities as f64, "count"));
        report.metric("file_bytes", Metric::new(sum.bytes as f64, "bytes"));
        report.metric("closed_shell", Metric::new(f64::from(sum.closed_shell), ""));
        let size = sum.bbox_max - sum.bbox_min;
        report.metric("bbox_x_mm", Metric::new(size.x, "mm"));
        report.metric("bbox_y_mm", Metric::new(size.y, "mm"));
        report.metric("bbox_z_mm", Metric::new(size.z, "mm"));
        if sum.patches > 0 {
            report.note(Severity::Info, "patches retain untrimmed parameter boundaries; compatible whole edges share topology, while partial-edge seams remain separate");
        }
        report.metric(
            "solid_shells",
            Metric::new(sum.solid_shells as f64, "count"),
        );
        report.metric("open_shells", Metric::new(sum.open_shells as f64, "count"));
        report.metric(
            "shared_patch_edges",
            Metric::new(sum.shared_edges as f64, "count"),
        );
        report.metric(
            "validated_entities",
            Metric::new(validation.entities as f64, "count"),
        );
        report.metric(
            "validated_references",
            Metric::new(validation.references as f64, "count"),
        );
        report.note(Severity::Info,"Structural subset validation passed. It does not certify all EXPRESS rules or replace an independent CAD import test.");
        if let Some(target) = target {
            write_atomic(&target, &content)
                .map_err(|e| ctx.fail(format!("cannot save to {}: {e}", target.display())))?;
        }
        Ok(vec![Data::File(path), Data::report(report)])
    }
}

fn confined_target(work: &Path, value: &str) -> Result<PathBuf, String> {
    let root =
        std::fs::canonicalize(work).map_err(|e| format!("cannot resolve work directory: {e}"))?;
    let raw = Path::new(value);
    if raw.components().any(|c| matches!(c, Component::ParentDir))
        || (!raw.is_absolute()
            && raw
                .components()
                .any(|c| matches!(c, Component::Prefix(_) | Component::RootDir)))
    {
        return Err(
            "save path must stay inside the node work directory without parent traversal".into(),
        );
    }
    let path = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        root.join(raw)
    };
    let parent = path.parent().ok_or("save path has no parent")?;
    let name = path.file_name().ok_or("save path has no file name")?;
    let mut ancestor = parent;
    while !ancestor.exists() {
        ancestor = ancestor
            .parent()
            .ok_or("save path has no existing ancestor")?;
    }
    if !std::fs::canonicalize(ancestor)
        .map_err(|e| format!("cannot resolve save path: {e}"))?
        .starts_with(&root)
    {
        return Err("save path is outside the node work directory".into());
    }
    std::fs::create_dir_all(parent).map_err(|e| format!("cannot create save directory: {e}"))?;
    let parent =
        std::fs::canonicalize(parent).map_err(|e| format!("cannot resolve save directory: {e}"))?;
    if !parent.starts_with(&root) {
        return Err("save directory resolves outside the node work directory".into());
    }
    Ok(parent.join(name))
}

fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = PathBuf::from(tmp);
    write_atomic_at(path, &tmp, content)
}

fn write_atomic_at(path: &Path, tmp: &Path, content: &str) -> std::io::Result<()> {
    // Cleanup starts only after this call owns the new temporary file.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp)?;
    let result = (|| {
        file.write_all(content.as_bytes())?;
        drop(file);
        std::fs::rename(tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::NurbsSurface;
    use crate::testutil::{cook, f, grid, report_of, s, some};
    use glam::DVec3;
    use vernon_core::Geometry;

    #[test]
    fn wp4_writer_is_implemented_and_has_no_placeholder() {
        assert!(!WriteStepNode.info().whitebox);
        let g = Geometry::make_box(DVec3::splat(10.0));
        let out = cook("surface.write_step", &[None, some(&g)], &[]).unwrap();
        assert!(!report_of(&out[1])
            .notes
            .iter()
            .any(|(_, n)| n.starts_with("white-box placeholder")));
    }

    #[test]
    fn wp4_oversized_patch_metadata_returns_error_without_panicking() {
        let base = PatchSet::new(vec![sheet(0.0)]).to_json();
        for field in ["degree_u", "degree_v", "nu", "nv"] {
            let mut data = base.clone();
            data["patches"][0][field] = serde_json::json!(usize::MAX);
            let result = std::panic::catch_unwind(|| {
                cook("surface.write_step", &[Some(Data::json(data)), None], &[])
            });
            assert!(result.is_ok(), "{field} overflowed during validation");
            assert!(result.unwrap().is_err());
        }
    }

    #[test]
    fn wp4_atomic_write_preserves_an_existing_temporary_file() {
        let ctx = crate::testutil::ctx("wp4_atomic_collision");
        std::fs::create_dir_all(&ctx.work_dir).unwrap();
        let path = ctx.work_dir.join("part.stp");
        let tmp = ctx.work_dir.join("existing.tmp");
        std::fs::write(&path, "previous STEP").unwrap();
        std::fs::write(&tmp, "unrelated file").unwrap();
        let error = write_atomic_at(&path, &tmp, "replacement").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "previous STEP");
        assert_eq!(std::fs::read_to_string(&tmp).unwrap(), "unrelated file");
    }

    fn count(content: &str, kind: &str) -> usize {
        content
            .lines()
            .filter(|l| {
                l.split_once('=')
                    .is_some_and(|(_, r)| r.starts_with(&format!("{kind}(")))
            })
            .count()
    }

    fn points(content: &str) -> Vec<DVec3> {
        content
            .lines()
            .filter_map(|l| {
                let r = l.split_once('=')?.1;
                let inner = r
                    .strip_prefix("CARTESIAN_POINT('',(")?
                    .strip_suffix("));")?;
                let v: Vec<f64> = inner.split(',').map(|x| x.parse().unwrap()).collect();
                (v.len() == 3).then(|| DVec3::new(v[0], v[1], v[2]))
            })
            .collect()
    }

    fn sheet(x0: f64) -> NurbsSurface {
        let mut pts = Vec::new();
        for i in 0..4 {
            for j in 0..4 {
                pts.push(DVec3::new(
                    x0 + 10.0 * i as f64 / 3.0,
                    10.0 * j as f64 / 3.0,
                    ((i * j) as f64).sin(),
                ));
            }
        }
        NurbsSurface::clamped_uniform(3, 3, 4, 4, pts)
    }

    #[test]
    fn faceted_box_is_a_closed_brep_in_millimetres() {
        let g = Geometry::make_box(DVec3::new(10.0, 20.0, 30.0));
        let (txt, sum) = write_step(Some(&g), None, &StepOptions::default()).unwrap();
        assert!(txt.starts_with("ISO-10303-21;\n"));
        assert!(txt.trim_end().ends_with("END-ISO-10303-21;"));
        assert!(txt.contains("SI_UNIT(.MILLI.,.METRE.)"));
        assert!(txt.contains("FILE_SCHEMA(('AUTOMOTIVE_DESIGN"));
        assert_eq!(count(&txt, "FACE_SURFACE"), 6);
        assert_eq!(count(&txt, "FACETED_BREP"), 1);
        assert_eq!(count(&txt, "CLOSED_SHELL"), 1);
        assert!(sum.closed_shell);
        assert!(txt.is_ascii());
        assert!((sum.bbox_max - sum.bbox_min - DVec3::new(10.0, 20.0, 30.0)).length() < 1e-9);
    }

    #[test]
    fn open_mesh_is_an_open_shell_and_ap242_changes_the_header() {
        let g = grid(2, 2, 10.0, 10.0, |_, _| 0.0);
        let (txt, sum) = write_step(
            Some(&g),
            None,
            &StepOptions {
                schema: Schema::Ap242,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(txt.contains("AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF"));
        assert_eq!(count(&txt, "OPEN_SHELL"), 1);
        assert_eq!(count(&txt, "FACETED_BREP"), 0);
        assert!(!sum.closed_shell);
        assert_eq!(count(&txt, "FACE_SURFACE"), 4);
    }

    #[test]
    fn two_patches_give_two_faces_and_the_input_bounding_box() {
        // spec test 15 (our own parse instead of an external reader)
        let ps = PatchSet::new(vec![sheet(0.0), sheet(10.0)]);
        let (txt, sum) = write_step(None, Some(&ps), &StepOptions::default()).unwrap();
        assert_eq!(count(&txt, "ADVANCED_FACE"), 2);
        assert_eq!(count(&txt, "B_SPLINE_SURFACE_WITH_KNOTS"), 2);
        assert_eq!(count(&txt, "EDGE_CURVE"), 8);
        let cps: Vec<DVec3> = ps
            .patches
            .iter()
            .flat_map(|p| p.surface.control_points.clone())
            .collect();
        let (lo, hi) = cps.iter().fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(l, h), p| (l.min(*p), h.max(*p)),
        );
        assert!((sum.bbox_min - lo).length() < 1e-4 && (sum.bbox_max - hi).length() < 1e-4);
        // distinct knots plus multiplicities: (4,4) and (0,1)
        assert!(
            txt.contains("(4,4),(4,4)"),
            "{}",
            txt.lines()
                .find(|l| l.contains("B_SPLINE_SURFACE_WITH_KNOTS"))
                .unwrap()
        );
        // all CARTESIAN_POINT entries parse as three numbers
        assert!(points(&txt).len() > 32);
        assert_eq!(
            crate::step_validate::validate(&txt).unwrap().entities,
            sum.entities
        );
    }

    #[test]
    fn rational_patches_use_the_complex_entity() {
        let mut s0 = sheet(0.0);
        s0.weights = Some(vec![1.0; 16]);
        let (txt, _) = write_step(
            None,
            Some(&PatchSet::new(vec![s0])),
            &StepOptions::default(),
        )
        .unwrap();
        assert!(txt.contains("RATIONAL_B_SPLINE_SURFACE"));
    }

    #[test]
    fn node_writes_a_file_and_recreates_it() {
        let g = Geometry::make_box(DVec3::splat(10.0));
        let out = cook(
            "surface.write_step",
            &[None, some(&g)],
            &[("name", s("box part"))],
        )
        .unwrap();
        let path = out[0].as_file().unwrap().clone();
        assert!(path.exists());
        assert!(path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("box_part"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("FACETED_BREP"));
        std::fs::remove_file(&path).unwrap();
        let again = cook(
            "surface.write_step",
            &[None, some(&g)],
            &[("name", s("box part"))],
        )
        .unwrap();
        assert!(again[0].as_file().unwrap().exists());
        assert_eq!(report_of(&out[1]).metrics["faces"].value, 6.0);
        // an unwritable extra copy is an error, not a panic
        let bad = cook(
            "surface.write_step",
            &[None, some(&g)],
            &[("save_to", s("Z:/no/such/dir/out.stp"))],
        );
        assert!(bad.is_err());
        // limits and empty inputs
        assert!(cook("surface.write_step", &[None, None], &[]).is_err());
        assert!(cook(
            "surface.write_step",
            &[None, some(&g)],
            &[("max_faces", f(100.0)), ("name", s(""))]
        )
        .is_err());
    }

    #[test]
    fn wp4_node_paths_are_confined_and_output_replaces_atomically() {
        let g = Geometry::make_box(DVec3::splat(10.0));
        let out = cook(
            "surface.write_step",
            &[None, some(&g)],
            &[("name", s("wp4_paths")), ("save_to", s("copies/part.step"))],
        )
        .unwrap();
        let main = out[0].as_file().unwrap();
        let original = std::fs::read_to_string(main).unwrap();
        assert_eq!(
            std::fs::read_to_string(main.parent().unwrap().join("copies/part.step")).unwrap(),
            original
        );
        let again = cook(
            "surface.write_step",
            &[None, some(&g)],
            &[("name", s("wp4_paths"))],
        )
        .unwrap();
        assert_eq!(again[0].as_file().unwrap(), main);
        assert_eq!(std::fs::read_to_string(main).unwrap(), original);
        for path in ["../outside.step", "C:/outside.step"] {
            assert!(cook(
                "surface.write_step",
                &[None, some(&g)],
                &[("name", s("wp4_paths")), ("save_to", s(path))]
            )
            .is_err());
            assert_eq!(std::fs::read_to_string(main).unwrap(), original);
        }
        let mut bad = g;
        bad.points[0].x = f64::NAN;
        assert!(cook(
            "surface.write_step",
            &[None, some(&bad)],
            &[("name", s("wp4_paths"))]
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(main).unwrap(), original);
    }

    #[test]
    #[ignore = "needs FreeCAD"]
    fn freecad_import_validates_facets_shared_patches_and_rational_surface() {
        use vernon_core::process::{run, Exit, RunOptions};
        let work = crate::testutil::ctx("wp4_freecad_import").work_dir;
        std::fs::create_dir_all(&work).unwrap();
        let box_mesh = Geometry::make_box(DVec3::new(10.0, 20.0, 30.0));
        let flat = |x: f64| {
            NurbsSurface::clamped_uniform(
                3,
                3,
                4,
                4,
                (0..4)
                    .flat_map(|i| {
                        (0..4).map(move |j| {
                            DVec3::new(x + 10.0 * i as f64 / 3.0, 10.0 * j as f64 / 3.0, 0.0)
                        })
                    })
                    .collect(),
            )
        };
        let joined = PatchSet::new(vec![flat(0.0), flat(10.0)]);
        let arc = crate::nurbs::arc_nurbs(
            DVec3::ZERO,
            DVec3::X,
            DVec3::Z,
            10.0,
            0.0,
            std::f64::consts::FRAC_PI_2,
        );
        let mut cylinder = NurbsSurface::clamped_uniform(
            2,
            1,
            3,
            2,
            arc.control_points
                .iter()
                .flat_map(|p| [*p, *p + DVec3::Y * 7.0])
                .collect(),
        );
        cylinder.weights = Some(arc.weights.unwrap().iter().flat_map(|&w| [w, w]).collect());
        let rational = PatchSet::new(vec![cylinder]);
        let mut cases = Vec::new();
        for (name, mesh, patches, schema, dimensions, solids, area) in [
            (
                "box_ap214",
                Some(&box_mesh),
                None,
                Schema::Ap214,
                [10.0, 20.0, 30.0],
                1,
                None,
            ),
            (
                "box_ap242",
                Some(&box_mesh),
                None,
                Schema::Ap242,
                [10.0, 20.0, 30.0],
                1,
                None,
            ),
            (
                "joined",
                None,
                Some(&joined),
                Schema::Ap214,
                [20.0, 10.0, 0.0],
                0,
                Some(200.0),
            ),
            (
                "rational",
                None,
                Some(&rational),
                Schema::Ap242,
                [10.0, 7.0, 10.0],
                0,
                Some(35.0 * std::f64::consts::PI),
            ),
        ] {
            let (content, _) = write_step(
                mesh,
                patches,
                &StepOptions {
                    schema,
                    name: name.into(),
                    ..Default::default()
                },
            )
            .unwrap();
            crate::step_validate::validate(&content).unwrap();
            let path = work.join(format!("{name}.step"));
            std::fs::write(&path, content).unwrap();
            cases.push(serde_json::json!({"path":path,"dimensions":dimensions,"solids":solids,"area":area}));
        }
        let data = work.join("cases.json");
        std::fs::write(&data, serde_json::to_string(&cases).unwrap()).unwrap();
        let script = work.join("check_import.py");
        let code=format!("import json\nimport Part\nwith open({}, encoding='utf-8') as f:\n    cases=json.load(f)\nfor case in cases:\n    shape=Part.read(case['path'])\n    assert not shape.isNull(), case['path']\n    assert shape.isValid(), case['path']\n    assert len(shape.Solids)==case['solids'], case['path']\n    dims=[shape.BoundBox.XLength,shape.BoundBox.YLength,shape.BoundBox.ZLength]\n    assert all(abs(a-b)<1e-4 for a,b in zip(dims,case['dimensions'])), (case['path'],dims)\n    if case['area'] is not None:\n        assert abs(shape.Area-case['area'])<1e-3, (case['path'],shape.Area)\nprint('VERNON_STEP_IMPORT_OK',len(cases))\n",serde_json::to_string(&data.to_string_lossy()).unwrap());
        std::fs::write(&script, code).unwrap();
        let exe = std::env::var_os("FREECADCMD").unwrap_or_else(|| "FreeCADCmd".into());
        let mut command = std::process::Command::new(exe);
        command
            .arg("--user-cfg")
            .arg(work.join("user.cfg"))
            .arg("--system-cfg")
            .arg(work.join("system.cfg"))
            .arg(&script)
            .current_dir(&work)
            .stdin(std::process::Stdio::null());
        let log = work.join("import.log");
        let exit = run(
            &mut command,
            &log,
            &RunOptions::with_timeout(std::time::Duration::from_secs(60)),
        )
        .expect("set FREECADCMD to an existing FreeCAD command-line executable");
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(exit, Exit::Code(0), "{text}");
        assert!(text.contains("VERNON_STEP_IMPORT_OK 4"), "{text}");
    }
    #[test]
    fn non_finite_coordinates_are_rejected() {
        let mut g = Geometry::make_box(DVec3::splat(10.0));
        g.points[0].x = f64::NAN;
        assert!(cook("surface.write_step", &[None, some(&g)], &[]).is_err());
    }
}

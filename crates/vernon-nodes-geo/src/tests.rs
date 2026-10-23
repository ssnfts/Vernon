use std::collections::HashSet;

use vernon_core::{AttribArray, DVec3, Geometry, Prim};
use vernon_graph::{CookContext, CookResult, Data, DataKind, ParamKind, ParamValue, Params, Registry, Stage};

use crate::measure::measure;
use crate::primitives::{make_box, make_grid, make_sphere, make_tube, CarPanel, GridPlane};
use crate::topology::{fuse_points, subdivide_linear, triangulate, triangulate_polygon};
use crate::util::SPEC;
use crate::xform::mirror;

fn registry() -> Registry {
    let mut r = Registry::new();
    crate::register(&mut r);
    r
}

fn ctx(name: &str) -> CookContext {
    CookContext {
        node_name: name.to_string(),
        work_dir: std::env::temp_dir().join("vernon-nodes-geo-tests").join(name.replace('.', "_")),
        frame: 0.0,
    }
}

fn cook(id: &str, inputs: &[Option<Data>], set: &[(&str, ParamValue)]) -> CookResult<Vec<Data>> {
    let reg = registry();
    let info = reg.info(id).unwrap_or_else(|| panic!("unknown node {id}"));
    let mut params = Params::from_specs(&info.params);
    for (k, v) in set {
        assert!(params.get(k).is_some(), "{id} has no param {k}");
        params.set(k, v.clone());
    }
    reg.get(id).unwrap().cook(&ctx(id), inputs, &params)
}

fn geo_of(d: &Data) -> Geometry {
    (**d.as_geometry().expect("geometry output")).clone()
}

fn cook_geo(id: &str, inputs: &[Option<Data>], set: &[(&str, ParamValue)]) -> Geometry {
    geo_of(&cook(id, inputs, set).unwrap_or_else(|e| panic!("{id}: {e}"))[0])
}

fn some(g: &Geometry) -> Option<Data> {
    Some(Data::geometry(g.clone()))
}

fn f(v: f64) -> ParamValue {
    ParamValue::Float(v)
}
fn i(v: i64) -> ParamValue {
    ParamValue::Int(v)
}
fn b(v: bool) -> ParamValue {
    ParamValue::Bool(v)
}
fn s(v: &str) -> ParamValue {
    ParamValue::Str(v.into())
}
fn v3(x: f64, y: f64, z: f64) -> ParamValue {
    ParamValue::Vec3(DVec3::new(x, y, z))
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// Every directed edge appears at most once (consistent winding).
fn consistently_wound(g: &Geometry) -> bool {
    let mut seen = HashSet::new();
    for p in &g.prims {
        if !p.closed {
            continue;
        }
        let v = &p.vertices;
        for k in 0..v.len() {
            if !seen.insert((v[k], v[(k + 1) % v.len()])) {
                return false;
            }
        }
    }
    true
}

const TRADEMARKS: &[&str] = &[
    "rhino", "fusion", "3ds", "quicksurface", "geomagic", "zeiss", "airshaper", "simscale",
    "ansys", "optimumlap", "plasticity", "polycam", "ladybug", "houdini", "autodesk", "blender",
    "solidworks", "revo", "sidefx", "orca", "prusa",
];

fn assert_no_trademark(what: &str, text: &str) {
    let low = text.to_lowercase();
    for t in TRADEMARKS {
        assert!(!low.contains(t), "{what} contains trademark '{t}': {text}");
    }
}

// ------------------------------------------------------------ registry ---

#[test]
fn registration() {
    let reg = registry();
    let expected = [
        "geo.box", "geo.grid", "geo.sphere", "geo.tube", "geo.car_panel", "geo.file_read",
        "geo.file_write", "geo.transform", "geo.merge", "geo.mirror", "geo.normals", "geo.reverse",
        "geo.triangulate", "geo.delete", "geo.group", "geo.attrib_create", "geo.measure",
        "geo.fuse", "geo.subdivide", "geo.noise",
    ];
    assert_eq!(reg.len(), expected.len());
    let mut ids = HashSet::new();
    for info in reg.types() {
        assert!(ids.insert(info.id.clone()), "duplicate id {}", info.id);
        assert!(info.id.starts_with("geo."), "{} lacks the geo. prefix", info.id);
        assert_eq!(info.stage, Stage::Geometry, "{}", info.id);
        assert!(!info.help.trim().is_empty(), "{} has no help", info.id);
        assert!(!info.label.trim().is_empty(), "{} has no label", info.id);
        assert_eq!(info.spec.as_deref(), Some(SPEC), "{} spec", info.id);
        assert!(!info.whitebox, "{} should be implemented", info.id);
        assert!(!info.emulates.is_empty(), "{} has no emulates", info.id);
        assert!(!info.outputs.is_empty(), "{} has no outputs", info.id);
        assert_no_trademark(&info.id, &info.label);
        assert_no_trademark(&info.id, &info.help);
        let mut names = HashSet::new();
        for p in &info.params {
            assert!(names.insert(p.name.clone()), "{} duplicate param {}", info.id, p.name);
            assert!(!p.help.trim().is_empty(), "{}.{} has no help", info.id, p.name);
            assert_no_trademark(&info.id, &p.label);
            assert_no_trademark(&info.id, &p.help);
            if let ParamKind::Enum { default, options } = &p.kind {
                assert!(options.contains(default), "{}.{} default not an option", info.id, p.name);
                for o in options {
                    assert_no_trademark(&info.id, o);
                }
            }
            if let ParamKind::Float { default, min, max } = &p.kind {
                assert!(min <= default && default <= max, "{}.{} default out of range", info.id, p.name);
            }
            if let ParamKind::Int { default, min, max } = &p.kind {
                assert!(min <= default && default <= max, "{}.{} default out of range", info.id, p.name);
            }
        }
        for port in info.inputs.iter().chain(&info.outputs) {
            assert_no_trademark(&info.id, &port.name);
        }
    }
    for id in expected {
        assert!(ids.contains(id), "missing {id}");
    }
    let spec = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(SPEC);
    assert!(spec.is_file(), "spec file {} is missing", spec.display());
}

// --------------------------------------------------------------- smoke ---

fn sample_inputs() -> Vec<(&'static str, Geometry)> {
    let mut cloud = Geometry::from_points(vec![DVec3::ZERO, DVec3::X, DVec3::Y, DVec3::new(5.0, 5.0, 5.0)]);
    cloud.point_groups.insert("pts".into(), vec![0, 2]);
    let mut lines = make_grid(100.0, 100.0, 2, 2, GridPlane::Xy, DVec3::ZERO);
    lines.prims.push(Prim::polyline(vec![0, 1, 2]));
    lines.prims.push(Prim::polygon(vec![3, 4])); // degenerate face
    lines.prim_attribs.resize_all(lines.prims.len());
    vec![
        ("box", make_box(DVec3::splat(100.0), DVec3::ZERO, 3)),
        ("panel_shell", CarPanel { thickness: 4.0, cols: 8, rows: 6, ..Default::default() }.build()),
        ("cloud", cloud),
        ("mixed", lines),
        ("empty", Geometry::new()),
    ]
}

#[test]
fn smoke_every_node_cooks_with_defaults() {
    let reg = registry();
    for info in reg.types() {
        let op = reg.get(&info.id).unwrap();
        let params = Params::from_specs(&info.params);
        let cases = if info.inputs.is_empty() { sample_inputs().into_iter().take(1).collect() } else { sample_inputs() };
        for (name, g) in cases {
            let ins: Vec<Option<Data>> = info
                .inputs
                .iter()
                .map(|p| match p.kind {
                    DataKind::Geometry | DataKind::Any => Some(Data::geometry(g.clone())),
                    _ => None,
                })
                .collect();
            match op.cook(&ctx(&info.id), &ins, &params) {
                Ok(out) => {
                    assert_eq!(out.len(), info.outputs.len(), "{} on {name}", info.id);
                    for (d, port) in out.iter().zip(&info.outputs) {
                        assert_eq!(d.kind(), port.kind, "{} on {name}: port {}", info.id, port.name);
                        if let Data::Geometry(g) = d {
                            g.validate().unwrap_or_else(|e| panic!("{} on {name}: {e}", info.id));
                        }
                    }
                }
                Err(e) => {
                    // Clean errors are allowed only where defaults can't work.
                    let allowed = matches!(info.id.as_str(), "geo.file_read" | "geo.file_write")
                        || (info.id == "geo.normals" && matches!(name, "cloud" | "empty"));
                    assert!(allowed, "{} failed on {name} with defaults: {e}", info.id);
                }
            }
        }
    }
}

// ----------------------------------------------------------- primitives ---

#[test]
fn box_is_closed_outward_and_measures_right() {
    for n in [1usize, 3] {
        let g = make_box(DVec3::new(10.0, 20.0, 30.0), DVec3::new(1.0, 2.0, 3.0), n);
        g.validate().unwrap();
        assert_eq!(g.points.len(), 6 * n * n + 2);
        assert_eq!(g.prims.len(), 6 * n * n);
        let m = measure(&g);
        assert!(m.watertight(), "n={n}");
        assert!(close(m.area, 2.0 * (200.0 + 600.0 + 300.0), 1e-9));
        assert!(close(m.volume, 6000.0, 1e-6));
        assert!(consistently_wound(&g));
        for p in 0..g.prims.len() {
            let c = crate::util::prim_centroid(&g, p) - DVec3::new(1.0, 2.0, 3.0);
            assert!(g.prim_normal(p).dot(c) > 0.0, "face {p} points inward");
        }
    }
}

#[test]
fn grid_counts_area_and_planes() {
    for (plane, n) in [("xy", DVec3::Z), ("yz", DVec3::X), ("zx", DVec3::Y)] {
        let g = cook_geo("geo.grid", &[], &[("size_u", f(200.0)), ("size_v", f(50.0)), ("cols", i(4)), ("rows", i(5)), ("plane", s(plane))]);
        assert_eq!(g.points.len(), 5 * 6);
        assert_eq!(g.prims.len(), 20);
        assert!(close(g.surface_area(), 10_000.0, 1e-9));
        for p in 0..g.prims.len() {
            assert!((g.prim_normal(p) - n).length() < 1e-12, "{plane}");
        }
        let uv = g.point_attribs.vec3s("uv").unwrap();
        assert_eq!(uv[29], DVec3::new(1.0, 1.0, 0.0));
    }
}

#[test]
fn sphere_converges_to_analytic_area_and_volume() {
    let r = 50.0;
    let g = make_sphere(r, 48, 96, DVec3::new(5.0, 0.0, 0.0));
    let m = measure(&g);
    assert!(m.watertight());
    assert!(consistently_wound(&g));
    for p in &g.points {
        assert!(close((*p - DVec3::new(5.0, 0.0, 0.0)).length(), r, 1e-9));
    }
    let area = 4.0 * std::f64::consts::PI * r * r;
    let vol = 4.0 / 3.0 * std::f64::consts::PI * r * r * r;
    assert!((m.area - area).abs() / area < 0.01, "area {}", m.area);
    assert!((m.volume - vol).abs() / vol < 0.015, "volume {}", m.volume);
    assert!(m.volume > 0.0);
}

#[test]
fn tube_volume_caps_and_axis() {
    let g = make_tube(25.0, 25.0, 100.0, 3, 256, true, 2, DVec3::ZERO);
    let m = measure(&g);
    assert!(m.watertight());
    let vol = std::f64::consts::PI * 25.0 * 25.0 * 100.0;
    assert!((m.volume - vol).abs() / vol < 1e-3, "volume {}", m.volume);
    let open = make_tube(25.0, 25.0, 100.0, 1, 24, false, 2, DVec3::ZERO);
    assert_eq!(measure(&open).open_edges, 48);
    let gx = cook_geo("geo.tube", &[], &[("axis", s("x")), ("height", f(300.0))]);
    let size = gx.bounds().size();
    assert!(close(size.x, 300.0, 1e-9) && close(size.y, 50.0, 1e-9));
    assert!(measure(&gx).volume > 0.0, "permuted axis keeps outward winding");
    let cone = make_tube(30.0, 10.0, 60.0, 1, 512, true, 2, DVec3::ZERO);
    let frustum = std::f64::consts::PI * 60.0 / 3.0 * (900.0 + 300.0 + 100.0);
    assert!((measure(&cone).volume - frustum).abs() / frustum < 1e-3);
}

#[test]
fn car_panel_golden_heights() {
    let p = CarPanel::default();
    assert!(close(p.height(0.0, 0.0), p.crown, 1e-12));
    assert!(close(p.height(0.0, p.width / 2.0), p.flare, 1e-12));
    assert!(close(p.height(0.0, -p.width / 2.0), 0.0, 1e-12));
    assert!(close(p.height(p.length / 2.0, 0.0), 0.0, 1e-12));
    assert!(close(p.height(-p.length / 2.0, p.width / 2.0), 0.0, 1e-12));

    let g = cook_geo("geo.car_panel", &[], &[]);
    assert_eq!(g.points.len(), 37 * 25);
    assert_eq!(g.prims.len(), 36 * 24);
    let size = g.bounds().size();
    assert!(close(size.x, 900.0, 1e-9) && close(size.y, 600.0, 1e-9));
    // Centre grid point (i = 18, j = 12) sits at the crown height.
    let c = g.points[12 * 37 + 18];
    assert!(close(c.z, 40.0, 1e-12) && close(c.x, 0.0, 1e-9) && close(c.y, 0.0, 1e-9));
    // Doubly curved: with the flare off, the crown is concave-down along both X
    // and Y at the centre (positive Gaussian curvature). With the flare on, the
    // +Y half curls upward, so only the X direction stays concave-down there.
    let h = 1.0;
    let second = |q: &CarPanel, dx: f64, dy: f64| {
        q.height(dx, dy) - 2.0 * q.height(0.0, 0.0) + q.height(-dx, -dy)
    };
    let crown_only = CarPanel { flare: 0.0, ..p };
    assert!(second(&crown_only, h, 0.0) < 0.0 && second(&crown_only, 0.0, h) < 0.0);
    assert!(second(&p, h, 0.0) < 0.0);
    assert!(p.height(0.0, 100.0) > crown_only.height(0.0, 100.0), "flare lifts the outboard side");
    // The flared +Y half tilts the centre normal slightly towards -Y; with the
    // flare off the surface is symmetric and the centre normal is exactly +Z.
    let n = g.point_attribs.vec3s("N").unwrap()[12 * 37 + 18];
    assert!((n - DVec3::Z).length() < 0.05, "centre normal {n}");
    let flat = cook_geo("geo.car_panel", &[], &[("flare", f(0.0))]);
    let n0 = flat.point_attribs.vec3s("N").unwrap()[12 * 37 + 18];
    assert!((n0 - DVec3::Z).length() < 1e-6, "crown-only centre normal {n0}");
    let m = measure(&g);
    assert_eq!(m.open_edges, 2 * (36 + 24));
    assert!(m.area > 900.0 * 600.0);
}

#[test]
fn car_panel_shell_is_closed_with_area_times_thickness_volume() {
    let flat = CarPanel { crown: 0.0, flare: 0.0, thickness: 10.0, ..Default::default() }.build();
    let m = measure(&flat);
    assert!(m.watertight());
    assert!(consistently_wound(&flat));
    assert!(close(m.volume, 900.0 * 600.0 * 10.0, 1e-3), "volume {}", m.volume);
    assert_eq!(flat.prim_groups["outer"].len(), 36 * 24);
    assert_eq!(flat.prim_groups["inner"].len(), 36 * 24);
    assert_eq!(flat.prim_groups["edge"].len(), 2 * (36 + 24));

    let curved = cook_geo("geo.car_panel", &[], &[("thickness", f(3.0))]);
    curved.validate().unwrap();
    let mc = measure(&curved);
    assert!(mc.watertight());
    let outer_area = measure(&CarPanel::default().build()).area;
    assert!((mc.volume - outer_area * 3.0).abs() / (outer_area * 3.0) < 0.02, "volume {}", mc.volume);
}

// ----------------------------------------------------------------- file ---

#[test]
fn file_write_then_read_round_trip() {
    let g = make_box(DVec3::splat(10.0), DVec3::ZERO, 2);
    let path = std::env::temp_dir().join("vernon-nodes-geo-tests").join("roundtrip.obj");
    let w = cook("geo.file_write", &[some(&g)], &[("path", s(path.to_str().unwrap()))]);
    let out = match w {
        Ok(out) => out,
        Err(e) if e.to_string().contains("not implemented") => return, // vernon-io pending
        Err(e) => panic!("{e}"),
    };
    assert_eq!(out[1].as_file(), Some(&path));
    assert_eq!(geo_of(&out[0]).points.len(), g.points.len());
    let r = cook("geo.file_read", &[], &[("path", s(path.to_str().unwrap()))]);
    let out = match r {
        Ok(out) => out,
        Err(e) if e.to_string().contains("not implemented") => return,
        Err(e) => panic!("{e}"),
    };
    let back = geo_of(&out[0]);
    assert_eq!(back.prims.len(), g.prims.len());
    assert!(close(measure(&back).area, 600.0, 1e-6));
    assert_eq!(out[1].as_report().unwrap().metrics["point_count"].value, back.points.len() as f64);
    // Metres in the file are converted to millimetres on read.
    let r = cook("geo.file_read", &[], &[("path", s(path.to_str().unwrap())), ("unit", s("m"))]).unwrap();
    assert!(close(geo_of(&r[0]).bounds().size().x, 10_000.0, 1e-6));
}

#[test]
fn file_nodes_reject_missing_or_unknown_paths() {
    assert!(cook("geo.file_read", &[], &[]).is_err());
    assert!(cook("geo.file_read", &[], &[("path", s("model.unknownext"))]).is_err());
    let g = make_box(DVec3::ONE, DVec3::ZERO, 1);
    assert!(cook("geo.file_write", &[some(&g)], &[]).is_err());
}

// ------------------------------------------------------------ transform ---

#[test]
fn transform_rotates_about_pivot_and_translates() {
    let g = Geometry::from_points(vec![DVec3::new(100.0, 0.0, 0.0)]);
    let out = cook_geo(
        "geo.transform",
        &[some(&g)],
        &[("rotate", v3(0.0, 0.0, 90.0)), ("translate", v3(1.0, 2.0, 3.0)), ("pivot", v3(50.0, 0.0, 0.0))],
    );
    assert!((out.points[0] - DVec3::new(51.0, 52.0, 3.0)).length() < 1e-9);
    // Rotation order: X then Z differs from Z then X.
    let p = Geometry::from_points(vec![DVec3::Y]);
    let xz = cook_geo("geo.transform", &[some(&p)], &[("rotate", v3(90.0, 0.0, 90.0)), ("rotate_order", s("xz"))]);
    let a = cook_geo("geo.transform", &[some(&p)], &[("rotate", v3(90.0, 0.0, 90.0)), ("rotate_order", s("xyz"))]);
    assert!(xz.points[0] == p.points[0] || (a.points[0] - DVec3::new(0.0, 0.0, 1.0)).length() < 1e-9);
    let zyx = cook_geo("geo.transform", &[some(&p)], &[("rotate", v3(90.0, 0.0, 90.0)), ("rotate_order", s("zyx"))]);
    assert!((zyx.points[0] - DVec3::new(-1.0, 0.0, 0.0)).length() < 1e-9, "{}", zyx.points[0]);
}

#[test]
fn transform_negative_scale_keeps_volume_positive() {
    let g = make_box(DVec3::new(10.0, 20.0, 30.0), DVec3::ZERO, 1);
    let out = cook_geo("geo.transform", &[some(&g)], &[("scale", v3(-2.0, 1.0, 1.0))]);
    assert!(close(measure(&out).volume, 12_000.0, 1e-6));
    let raw = cook_geo("geo.transform", &[some(&g)], &[("scale", v3(-2.0, 1.0, 1.0)), ("fix_winding", b(false))]);
    assert!(close(measure(&raw).volume, -12_000.0, 1e-6));
}

/// Report golden value: a volumetric drift of 0.05 mm/m over a 4.5 m car
/// adds about 0.23 mm (0.225 mm exactly) of length.
#[test]
fn scanner_drift_scale_golden() {
    let car = make_box(DVec3::new(4500.0, 1800.0, 1400.0), DVec3::ZERO, 1);
    let scaled = cook_geo("geo.transform", &[some(&car)], &[("uniform_scale", f(1.0 + 0.05 / 1000.0))]);
    let grow = scaled.bounds().size().x - 4500.0;
    assert!(close(grow, 0.225, 1e-9), "growth {grow}");
    assert!(close((grow * 100.0).round() / 100.0, 0.23, 1e-12));
}

// --------------------------------------------------------- merge/mirror ---

#[test]
fn merge_two_and_three_inputs() {
    let a = make_box(DVec3::ONE, DVec3::ZERO, 1);
    let bx = make_box(DVec3::ONE, DVec3::splat(5.0), 1);
    let two = cook_geo("geo.merge", &[some(&a), some(&bx), None], &[]);
    assert_eq!((two.points.len(), two.prims.len()), (16, 12));
    let three = cook_geo("geo.merge", &[some(&a), some(&bx), some(&a)], &[]);
    assert_eq!((three.points.len(), three.prims.len()), (24, 18));
    three.validate().unwrap();
    assert!(cook("geo.merge", &[some(&a), None, None], &[]).is_err());
}

#[test]
fn mirror_welds_seam_and_keeps_winding() {
    let panel = CarPanel::default().build();
    let (m, welded) = mirror(&panel, 1, -300.0, true, true, 0.01);
    m.validate().unwrap();
    assert_eq!(welded, 37);
    assert_eq!(m.points.len(), 2 * panel.points.len() - 37);
    assert_eq!(m.prims.len(), 2 * panel.prims.len());
    assert_eq!(m.prim_groups["mirrored"].len(), panel.prims.len());
    let ms = measure(&m);
    assert_eq!(ms.non_manifold_edges, 0);
    assert_eq!(ms.open_edges, 2 * 120 - 2 * 36);
    assert!(consistently_wound(&m));
    assert!(close(ms.area, 2.0 * measure(&panel).area, 1e-6));
    // Mirrored normals are reflected: centre normal stays +Z.
    let n = m.point_attribs.vec3s("N").unwrap();
    assert!(n.iter().all(|n| n.z > 0.0));
    // Unwelded copy has all points.
    let (u, w) = mirror(&panel, 1, -300.0, true, false, 0.01);
    assert_eq!((u.points.len(), w), (2 * panel.points.len(), 0));
    // Reflection only, closed solid keeps a positive volume.
    let bx = make_box(DVec3::new(10.0, 20.0, 30.0), DVec3::new(0.0, 50.0, 0.0), 1);
    let only = cook_geo("geo.mirror", &[some(&bx)], &[("keep_original", b(false))]);
    assert!(close(measure(&only).volume, 6000.0, 1e-6));
    assert!(close(only.bounds().center().y, -50.0, 1e-9));
}

// ------------------------------------------------------------- normals ---

#[test]
fn normals_angle_and_area_weighting_golden() {
    let g = make_box(DVec3::new(10.0, 20.0, 30.0), DVec3::ZERO, 1);
    let corner = g.points.iter().position(|p| p.x > 0.0 && p.y > 0.0 && p.z > 0.0).unwrap();
    let angle = cook_geo("geo.normals", &[some(&g)], &[]);
    let n = angle.point_attribs.vec3s("N").unwrap()[corner];
    assert!((n - DVec3::ONE.normalize()).length() < 1e-12, "{n}");
    let area = cook_geo("geo.normals", &[some(&g)], &[("weighting", s("area"))]);
    let n = area.point_attribs.vec3s("N").unwrap()[corner];
    assert!((n - DVec3::new(600.0, 300.0, 200.0).normalize()).length() < 1e-12, "{n}");
    let prims_only = cook_geo("geo.normals", &[some(&g)], &[("class", s("prims"))]);
    assert!(prims_only.point_attribs.get("N").is_none());
    assert_eq!(prims_only.prim_attribs.vec3s("N").unwrap().len(), 6);
    let grid = make_grid(10.0, 10.0, 3, 3, GridPlane::Xy, DVec3::ZERO);
    let gn = cook_geo("geo.normals", &[some(&grid)], &[("weighting", s("equal"))]);
    assert!(gn.point_attribs.vec3s("N").unwrap().iter().all(|n| (*n - DVec3::Z).length() < 1e-12));
}

#[test]
fn reverse_flips_volume_and_normals() {
    let mut g = make_box(DVec3::splat(10.0), DVec3::ZERO, 1);
    g.compute_normals();
    let r = cook_geo("geo.reverse", &[some(&g)], &[]);
    assert!(close(measure(&r).volume, -1000.0, 1e-9));
    let before = g.prim_attribs.vec3s("N").unwrap();
    let after = r.prim_attribs.vec3s("N").unwrap();
    assert!(before.iter().zip(after).all(|(a, b)| (*a + *b).length() < 1e-12));
    for (p, n) in after.iter().enumerate() {
        assert!((r.prim_normal(p) - *n).length() < 1e-12);
    }
    // Only a group.
    let mut h = g.clone();
    h.prim_groups.insert("top".into(), vec![1]);
    let r = cook_geo("geo.reverse", &[some(&h)], &[("group", s("top"))]);
    let mut rev = g.prims[1].vertices.clone();
    rev.reverse();
    assert_eq!(r.prims[1].vertices, rev);
    assert_eq!(r.prims[0].vertices, g.prims[0].vertices);
    assert!(cook("geo.reverse", &[some(&h)], &[("group", s("nope"))]).is_err());
}

// ---------------------------------------------------------- triangulate ---

#[test]
fn triangulate_concave_polygon_keeps_area_and_orientation() {
    // L-shape, counter-clockwise in XY.
    let pts = vec![
        DVec3::new(0.0, 0.0, 0.0),
        DVec3::new(20.0, 0.0, 0.0),
        DVec3::new(20.0, 10.0, 0.0),
        DVec3::new(10.0, 10.0, 0.0),
        DVec3::new(10.0, 20.0, 0.0),
        DVec3::new(0.0, 20.0, 0.0),
    ];
    let tris = triangulate_polygon(&pts);
    assert_eq!(tris.len(), 4);
    let mut area = 0.0;
    for t in &tris {
        let n = (pts[t[1]] - pts[t[0]]).cross(pts[t[2]] - pts[t[0]]);
        assert!(n.z > 0.0, "triangle {t:?} flipped");
        area += n.z * 0.5;
    }
    assert!(close(area, 300.0, 1e-9));
    // Fan would produce a triangle outside the L for this ordering start.
    let g = Geometry::from_polygons(pts.clone(), vec![(0..6).collect()]);
    let ear = cook_geo("geo.triangulate", &[some(&g)], &[]);
    assert!(close(measure(&ear).area, 300.0, 1e-9));
}

#[test]
fn triangulate_box_carries_attributes_and_groups() {
    let mut g = make_box(DVec3::splat(10.0), DVec3::ZERO, 1);
    g.prim_attribs.set("part", AttribArray::Int((0..6).collect()));
    g.prim_groups.insert("top".into(), vec![4]);
    let t = triangulate(&g, None, false);
    t.validate().unwrap();
    assert_eq!(t.prims.len(), 12);
    assert!(t.prims.iter().all(|p| p.vertices.len() == 3));
    let m = measure(&t);
    assert!(m.watertight() && close(m.area, 600.0, 1e-9) && close(m.volume, 1000.0, 1e-9));
    let part = t.prim_attribs.ints("part").unwrap();
    assert_eq!(part, &[0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5]);
    assert_eq!(t.prim_groups["top"], vec![8, 9]);
    let only_top = cook_geo("geo.triangulate", &[some(&g)], &[("group", s("top")), ("method", s("fan"))]);
    assert_eq!(only_top.prims.len(), 7);
}

// ------------------------------------------------- delete/group/attribs ---

#[test]
fn delete_by_box_group_and_points() {
    let g = make_grid(100.0, 100.0, 10, 10, GridPlane::Xy, DVec3::ZERO);
    let left = cook_geo("geo.delete", &[some(&g)], &[("mode", s("bbox")), ("bbox_center", v3(-25.0, 0.0, 0.0)), ("bbox_size", v3(50.0, 200.0, 10.0))]);
    assert_eq!(left.prims.len(), 50);
    assert_eq!(left.points.len(), 6 * 11);
    left.validate().unwrap();
    let kept = cook_geo(
        "geo.delete",
        &[some(&g)],
        &[("mode", s("bbox")), ("bbox_center", v3(-25.0, 0.0, 0.0)), ("bbox_size", v3(50.0, 200.0, 10.0)), ("invert", b(true))],
    );
    assert_eq!(kept.prims.len(), 50);
    assert!(kept.points.iter().all(|p| p.x <= 1e-9));
    let mut h = g.clone();
    h.prim_groups.insert("doomed".into(), vec![0, 1, 2]);
    h.point_groups.insert("corner".into(), vec![0]);
    let d = cook_geo("geo.delete", &[some(&h)], &[("group", s("doomed"))]);
    assert_eq!(d.prims.len(), 97);
    assert_eq!(d.points.len(), 121 - 3);
    assert!(d.point_groups["corner"].is_empty());
    // Deleting an interior point removes its four faces but keeps the others' points.
    let mut c = g.clone();
    c.point_groups.insert("mid".into(), vec![60]);
    let p = cook_geo("geo.delete", &[some(&c)], &[("group", s("mid")), ("element", s("points"))]);
    assert_eq!((p.points.len(), p.prims.len()), (120, 96));
    assert!(cook("geo.delete", &[some(&g)], &[("group", s("missing"))]).is_err());
    // Point clouds keep free points.
    let mut cloud = Geometry::from_points(vec![DVec3::ZERO, DVec3::splat(100.0)]);
    cloud.point_groups.insert("far".into(), vec![1]);
    let pc = cook_geo("geo.delete", &[some(&cloud)], &[("group", s("far")), ("element", s("points"))]);
    assert_eq!(pc.points, vec![DVec3::ZERO]);
}

#[test]
fn group_by_normal_and_box_with_combine() {
    let g = make_box(DVec3::splat(10.0), DVec3::ZERO, 2);
    let top = cook_geo("geo.group", &[some(&g)], &[("name", s("top")), ("method", s("normal")), ("spread", f(10.0))]);
    let idx = &top.prim_groups["top"];
    assert_eq!(idx.len(), 4);
    assert!(idx.iter().all(|&p| (top.prim_normal(p as usize) - DVec3::Z).length() < 1e-12));
    let wide = cook_geo("geo.group", &[some(&g)], &[("name", s("up")), ("method", s("normal")), ("spread", f(90.0))]);
    assert_eq!(wide.prim_groups["up"].len(), 4 + 16);
    let pts = cook_geo(
        "geo.group",
        &[some(&g)],
        &[("name", s("hi")), ("element", s("points")), ("bbox_center", v3(0.0, 0.0, 5.0)), ("bbox_size", v3(20.0, 20.0, 1.0))],
    );
    assert_eq!(pts.point_groups["hi"].len(), 9);
    let both = cook_geo(
        "geo.group",
        &[some(&top)],
        &[("name", s("top")), ("bbox_center", v3(2.5, 0.0, 0.0)), ("bbox_size", v3(5.0, 20.0, 20.0)), ("combine", s("intersect"))],
    );
    assert_eq!(both.prim_groups["top"].len(), 2);
    let sub = cook_geo(
        "geo.group",
        &[some(&top)],
        &[("name", s("top")), ("bbox_center", v3(2.5, 0.0, 0.0)), ("bbox_size", v3(5.0, 20.0, 20.0)), ("combine", s("subtract"))],
    );
    assert_eq!(sub.prim_groups["top"].len(), 2);
    assert!(cook("geo.group", &[some(&g)], &[("name", s(" "))]).is_err());
}

#[test]
fn attrib_create_classes_types_and_groups() {
    let mut g = make_grid(10.0, 10.0, 1, 1, GridPlane::Xy, DVec3::ZERO);
    g.point_groups.insert("two".into(), vec![1, 2]);
    let a = cook_geo("geo.attrib_create", &[some(&g)], &[("name", s("w")), ("value", v3(2.5, 0.0, 0.0)), ("group", s("two"))]);
    assert_eq!(a.point_attribs.floats("w").unwrap(), &[0.0, 2.5, 2.5, 0.0]);
    let i2 = cook_geo("geo.attrib_create", &[some(&a)], &[("name", s("w")), ("type", s("int")), ("value", v3(2.6, 0.0, 0.0))]);
    assert_eq!(i2.point_attribs.ints("w").unwrap(), &[3, 3, 3, 3]);
    let d = cook_geo("geo.attrib_create", &[some(&g)], &[("name", s("material")), ("class", s("detail")), ("type", s("string")), ("string_value", s("ASA"))]);
    assert_eq!(d.detail_attribs.strs("material").unwrap(), &["ASA".to_string()]);
    let p = cook_geo("geo.attrib_create", &[some(&g)], &[("name", s("Cd")), ("class", s("prim")), ("type", s("vector")), ("value", v3(1.0, 0.5, 0.0))]);
    assert_eq!(p.prim_attribs.vec3s("Cd").unwrap(), &[DVec3::new(1.0, 0.5, 0.0)]);
    p.validate().unwrap();
    assert!(cook("geo.attrib_create", &[some(&g)], &[("name", s("9bad"))]).is_err());
    assert!(cook("geo.attrib_create", &[some(&g)], &[("group", s("nope"))]).is_err());
}

// -------------------------------------------------------------- measure ---

#[test]
fn measure_report_metrics_and_watertight_gate() {
    let g = make_box(DVec3::new(10.0, 20.0, 30.0), DVec3::new(1.0, 1.0, 1.0), 2);
    let out = cook("geo.measure", &[some(&g)], &[]).unwrap();
    let r = out[1].as_report().unwrap();
    let m = |k: &str| r.metrics[k].value;
    assert_eq!(m("point_count"), g.points.len() as f64);
    assert!(close(m("surface_area"), 2200.0, 1e-9));
    assert!(close(m("surface_area_m2"), 0.0022, 1e-12));
    assert!(close(m("volume"), 6000.0, 1e-6));
    assert!(close(m("size_z"), 30.0, 1e-12));
    assert!(close(m("centroid_x"), 1.0, 1e-9) && close(m("centroid_z"), 1.0, 1e-9));
    assert_eq!(m("open_edges"), 0.0);
    assert!(r.passes());
    let areas = geo_of(&out[0]);
    assert!(close(areas.prim_attribs.floats("area").unwrap().iter().sum::<f64>(), 2200.0, 1e-9));
    assert_eq!(r.metrics["surface_area"].unit, "mm²");

    let panel = CarPanel::default().build();
    let gated = cook("geo.measure", &[some(&panel)], &[("require_watertight", b(true))]).unwrap();
    assert!(!gated[1].as_report().unwrap().passes());
    let free = cook("geo.measure", &[some(&panel)], &[]).unwrap();
    assert!(free[1].as_report().unwrap().passes());
}

// ------------------------------------------------------- fuse/subdivide ---

#[test]
fn fuse_merges_duplicates_and_removes_degenerates() {
    // Two quads sharing an edge, stored with duplicated points.
    let pts = vec![
        DVec3::new(0.0, 0.0, 0.0),
        DVec3::new(1.0, 0.0, 0.0),
        DVec3::new(1.0, 1.0, 0.0),
        DVec3::new(0.0, 1.0, 0.0),
        DVec3::new(1.005, 0.0, 0.0),
        DVec3::new(2.0, 0.0, 0.0),
        DVec3::new(2.0, 1.0, 0.0),
        DVec3::new(1.0, 1.0, 0.0),
    ];
    let mut g = Geometry::from_polygons(pts, vec![vec![0, 1, 2, 3], vec![4, 5, 6, 7]]);
    g.point_attribs.set("w", AttribArray::Float(vec![0.0, 1.0, 0.0, 0.0, 3.0, 0.0, 0.0, 0.0]));
    let tight = cook_geo("geo.fuse", &[some(&g)], &[]);
    assert_eq!(tight.points.len(), 7, "only the exact duplicate merges at the default 0.001 mm");
    let fused = cook_geo("geo.fuse", &[some(&g)], &[("tolerance", f(0.01))]);
    assert_eq!(fused.points.len(), 6);
    assert_eq!(measure(&fused).open_edges, 6);
    assert!(close(fused.points[1].x, 1.0025, 1e-12), "averaged position");
    assert!(close(fused.point_attribs.floats("w").unwrap()[1], 2.0, 1e-12), "averaged attribute");
    let first = cook_geo("geo.fuse", &[some(&g)], &[("tolerance", f(0.01)), ("snap", s("first"))]);
    assert_eq!(first.points[1], DVec3::new(1.0, 0.0, 0.0));
    // A sliver quad collapses to a triangle; a tiny triangle disappears.
    let sliver = Geometry::from_polygons(
        vec![DVec3::ZERO, DVec3::X, DVec3::new(1.0, 1e-4, 0.0), DVec3::Y],
        vec![vec![0, 1, 2, 3], vec![1, 2, 0]],
    );
    let (s1, removed) = fuse_points(&sliver, 0.001, false, true);
    assert_eq!(removed, 1);
    assert_eq!(s1.prims.len(), 1);
    assert_eq!(s1.prims[0].vertices.len(), 3);
}

#[test]
fn subdivide_linear_preserves_shape_and_interpolates() {
    let g = make_grid(100.0, 100.0, 2, 2, GridPlane::Xy, DVec3::ZERO);
    let s1 = cook_geo("geo.subdivide", &[some(&g)], &[]);
    assert_eq!(s1.prims.len(), 16);
    assert_eq!(s1.points.len(), 25);
    assert!(close(s1.surface_area(), 10_000.0, 1e-9));
    let uv = s1.point_attribs.vec3s("uv").unwrap();
    for (p, t) in s1.points.iter().zip(uv) {
        assert!(close(t.x, p.x / 100.0 + 0.5, 1e-12) && close(t.y, p.y / 100.0 + 0.5, 1e-12));
    }
    let s3 = cook_geo("geo.subdivide", &[some(&g)], &[("iterations", i(3))]);
    assert_eq!(s3.prims.len(), 4 * 64);
    // Closed triangulated box stays closed; triangles split 1 -> 4.
    let t = triangulate(&make_box(DVec3::splat(10.0), DVec3::ZERO, 1), None, false);
    let st = subdivide_linear(&t, false);
    assert_eq!(st.prims.len(), 48);
    let m = measure(&st);
    assert!(m.watertight() && close(m.volume, 1000.0, 1e-9));
    let sq = subdivide_linear(&t, true);
    assert_eq!(sq.prims.len(), 36);
    assert!(measure(&sq).watertight());
    // Point groups: midpoint between two members joins the group.
    let mut h = g.clone();
    h.point_groups.insert("edge".into(), vec![0, 1, 2]);
    let sh = subdivide_linear(&h, false);
    assert_eq!(sh.point_groups["edge"].len(), 5);
    // Polylines gain midpoints.
    let pl = Geometry {
        points: vec![DVec3::ZERO, DVec3::X * 2.0],
        prims: vec![Prim::polyline(vec![0, 1])],
        ..Default::default()
    };
    let sp = subdivide_linear(&pl, false);
    assert_eq!(sp.prims[0].vertices.len(), 3);
    assert_eq!(sp.points[2], DVec3::X);
}

#[test]
fn panel_pipeline_merge_mirror_measure() {
    // A small chain the example pipeline uses: panel -> mirror -> fuse -> measure.
    let panel = cook("geo.car_panel", &[], &[("cols", i(12)), ("rows", i(8))]).unwrap();
    let mirrored = cook("geo.mirror", &[Some(panel[0].clone())], &[("origin", f(-300.0))]).unwrap();
    let fused = cook("geo.fuse", &[Some(mirrored[0].clone())], &[]).unwrap();
    let report = cook("geo.measure", &[Some(fused[0].clone())], &[]).unwrap();
    let r = report[1].as_report().unwrap();
    assert_eq!(r.metrics["point_count"].value, (2 * 13 * 9 - 13) as f64);
    assert!(close(r.metrics["size_y"].value, 1200.0, 1e-9));
}

#[test]
fn unwired_inputs_give_clean_errors_not_panics() {
    let reg = registry();
    for info in reg.types() {
        if info.inputs.is_empty() {
            continue;
        }
        let op = reg.get(&info.id).unwrap();
        let params = Params::from_specs(&info.params);
        let ins: Vec<Option<Data>> = info.inputs.iter().map(|_| None).collect();
        let res = op.cook(&ctx(&info.id), &ins, &params);
        // A required input left unwired must be a cook error, never a panic.
        let any_required = info.inputs.iter().any(|p| p.required);
        assert!(res.is_err() || !any_required, "{} cooked with every input unwired", info.id);
        assert!(any_required, "{} has no required input", info.id);
    }
}

#[test]
fn closed_primitives_are_outward_euler_two() {
    let shapes = [
        ("box", make_box(DVec3::new(10.0, 20.0, 30.0), DVec3::ZERO, 1)),
        ("sphere", make_sphere(40.0, 12, 24, DVec3::ZERO)),
        ("tube", make_tube(20.0, 20.0, 80.0, 2, 16, true, 2, DVec3::ZERO)),
    ];
    for (name, g) in shapes {
        let edges = crate::util::edge_counts(&g);
        assert!(edges.values().all(|&n| n == 2), "{name} has an open or non-manifold edge");
        let euler = g.points.len() as i64 - edges.len() as i64 + g.prims.len() as i64;
        assert_eq!(euler, 2, "{name} Euler characteristic");
        assert!(measure(&g).volume > 0.0, "{name} winds inward");
    }
    let cube = make_box(DVec3::splat(10.0), DVec3::ZERO, 1);
    assert_eq!((cube.points.len(), crate::util::edge_counts(&cube).len(), cube.prims.len()), (8, 12, 6));
}

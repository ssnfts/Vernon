//! Tier A: analytic and textbook references. Always run; need no external program.

use std::f64::consts::PI;

use vernon_core::{DMat4, DVec3, Geometry};
use vernon_nodes_fea::linalg::von_mises;
use vernon_nodes_fea::mesh::{boundary_faces, box_tets, tet4_to_tet10};
use vernon_nodes_fea::refsolve::{solve, RefInput};
use vernon_nodes_fea::types::{FacePressure, LoadSet, MountSet, MountSite, OrientationCard};

use crate::fea::*;
use crate::harness::*;
use crate::meshes::*;

const TIER: &str = "A";

pub fn run() -> Vec<Case> {
    vec![
        beam_deflection(),
        beam_stress(),
        patch_test(),
        primitive_sphere(),
        primitive_plane_cylinder(),
        icp_known_transform(),
        qem_bounds(),
        flatten_developable(),
        flatten_negative_control(),
        snap_fit(),
        unit_round_trips(),
        file_round_trips(),
        ahmed_published_window(),
    ]
}

const BEAM_REF: &str = "Euler-Bernoulli cantilever tip deflection P L^3/(3 E I) (Roark, Formulas for Stress and Strain, Table 8.1 case 1a); Timoshenko shear adds ~0.8% at this L/h";

fn beam_deflection() -> Case {
    run_case("A.fea.beam_deflection", TIER, "Cantilever tip deflection vs beam theory (C3D10, 200x20x20 mm, E=2 GPa, 100 N)", BEAM_REF, |ck| {
        let mesh = mesh_tet10(2);
        let o = solve_cantilever(&mesh, 300_000)?;
        let d = tip_deflection(&mesh, &o.disp);
        ck.close("tip deflection (coarse, 2 elem/side)", d, beam_theory(), 0.03, 0.0, "mm");
        let fine = mesh_tet10(4);
        let df = tip_deflection(&fine, &solve_cantilever(&fine, 300_000)?.disp);
        ck.close("tip deflection (fine, 4 elem/side) vs beam+shear 1.008x", df, 1.008 * beam_theory(), 0.01, 0.0, "mm");
        let r = o.reactions["MOUNT1"];
        ck.close("root reaction Fz equals applied load", r[2], LOAD_N, 1e-3, 0.0, "N");
        Ok(())
    })
}

fn beam_stress() -> Case {
    run_case("A.fea.beam_stress", TIER, "Cantilever bending stress at mid-span vs sigma = M c / I", "Flexure formula sigma = M y / I with M = P (L/2) at mid-span (any strength-of-materials text)", |ck| {
        let mesh = mesh_tet10(2);
        let o = solve_cantilever(&mesh, 300_000)?;
        let want = LOAD_N * (LENGTH / 2.0) * (SIDE / 2.0) / (SIDE.powi(4) / 12.0);
        let peak = nodes_at(&mesh, 0.0).iter().map(|&n| von_mises(&o.node_stress[n as usize])).fold(0.0, f64::max);
        ck.close("mid-span outer-fibre von Mises", peak, want, 0.10, 0.0, "MPa");
        Ok(())
    })
}

fn patch_test() -> Case {
    run_case("A.fea.patch_uniaxial", TIER, "Uniform end pressure: reaction and stress are exact (patch test)", "Zienkiewicz & Taylor, patch test: a constant stress state must be reproduced by consistent elements", |ck| {
        let mesh = tet4_to_tet10(&box_tets(DVec3::new(40.0, 20.0, 20.0), [2, 1, 1]))?;
        let pressures = boundary_faces(&mesh).iter().filter(|b| b.normal.x > 0.99).map(|b| FacePressure { elem: b.elem as u32, face: b.face, p_mpa: -5.0 }).collect();
        let site = MountSite { name: "end".into(), center: [-20.0, 0.0, 0.0], axis: [-1.0, 0.0, 0.0], nodes: nodes_at(&mesh, -20.0), model: "spring_pad".into(), r_w_mm: 1000.0, avg_radius_mm: 5.0, bolt_d_mm: 5.0, pad_e_mpa: 1e6, pad_t_mm: 1.0 };
        let (c, id) = (card(), OrientationCard::identity());
        let o = solve(&RefInput { mesh: &mesh, card: &c, orient: &id, mounts: &MountSet { sites: vec![site] }, load: &LoadSet { pressures, ..Default::default() }, max_dof: 60000, tol: 1e-12 })?;
        ck.close("reaction = p A = 5 MPa x 400 mm2", -o.reactions["MOUNT1"][0], 2000.0, 1e-6, 0.0, "N");
        let (mut sum, mut n) = (0.0, 0.0);
        for (e, st) in o.elem_stress.iter().enumerate() {
            if vernon_nodes_fea::mesh::element_centroid(&mesh, e).x > 10.0 {
                sum += st[0];
                n += 1.0;
            }
        }
        ck.close("mean sigma_x away from support", sum / n, 5.0, 0.02, 0.0, "MPa");
        Ok(())
    })
}

fn measure(g: &Geometry) -> Result<(f64, f64), String> {
    let r = out_report(&cook("geo.measure", &[geo_in(g)], &[])?)?;
    Ok((metric(&r, "surface_area")?, metric(&r, "volume")?))
}

fn primitive_sphere() -> Case {
    run_case("A.geo.sphere_area_volume", TIER, "UV sphere area and volume vs 4 pi r^2, 4/3 pi r^3, with convergence", "Closed-form sphere; inscribed UV polyhedron error shrinks with refinement", |ck| {
        let r = 50.0;
        let mut prev = f64::MAX;
        for (rows, cols) in [(16, 32), (32, 64), (64, 128)] {
            let g = out_geo(&cook("geo.sphere", &[], &[("radius", f(r)), ("rows", i(rows)), ("cols", i(cols))])?, 0)?;
            let (a, v) = measure(&g)?;
            let ea = (a - 4.0 * PI * r * r).abs() / (4.0 * PI * r * r);
            let ev = (v - 4.0 / 3.0 * PI * r.powi(3)).abs() / (4.0 / 3.0 * PI * r.powi(3));
            ck.note(format!("{rows}x{cols}: area error {:.4}%, volume error {:.4}%", ea * 100.0, ev * 100.0));
            ck.at_most(&format!("{rows}x{cols}: area error shrinks with refinement"), ea, prev, "");
            prev = ea;
            if rows == 64 {
                ck.close("area at 64x128", a, 4.0 * PI * r * r, 0.002, 0.0, "mm2");
                ck.close("volume at 64x128", v, 4.0 / 3.0 * PI * r.powi(3), 0.003, 0.0, "mm3");
            }
        }
        Ok(())
    })
}

fn primitive_plane_cylinder() -> Case {
    run_case("A.geo.plane_cylinder", TIER, "Plane area; cylinder area and volume vs the exact n-gon prism", "Plane A = a b; regular n-gon: area 1/2 n r^2 sin(2 pi/n), edge 2 r sin(pi/n); exact for the tessellation", |ck| {
        let g = out_geo(&cook("geo.grid", &[], &[("size_u", f(300.0)), ("size_v", f(120.0)), ("cols", i(7)), ("rows", i(5))])?, 0)?;
        ck.close("plane area 300x120", measure(&g)?.0, 36000.0, 1e-9, 0.0, "mm2");
        let (r, h, n) = (25.0_f64, 100.0_f64, 128_i64);
        let g = out_geo(&cook("geo.tube", &[], &[("radius_bottom", f(r)), ("radius_top", f(r)), ("height", f(h)), ("cols", i(n)), ("caps", b(true))])?, 0)?;
        let (a, v) = measure(&g)?;
        let nf = n as f64;
        let side = 2.0 * r * (PI / nf).sin();
        let cap = 0.5 * nf * r * r * (2.0 * PI / nf).sin();
        ck.close("cylinder area vs exact 128-gon prism", a, nf * side * h + 2.0 * cap, 1e-9, 0.0, "mm2");
        ck.close("cylinder volume vs exact 128-gon prism", v, cap * h, 1e-9, 0.0, "mm3");
        ck.close("cylinder area vs true cylinder", a, 2.0 * PI * r * h + 2.0 * PI * r * r, 5e-4, 0.0, "mm2");
        Ok(())
    })
}

fn icp_known_transform() -> Case {
    run_case("A.clean.icp_known_transform", TIER, "ICP recovers a known rigid transform", "Besl & McKay 1992 / Chen & Medioni 1992; ground truth is the applied transform", |ck| {
        let reference = heightfield(20_000, 200.0);
        let truth = DMat4::from_translation(DVec3::new(2.0, -1.5, 1.0)) * DMat4::from_rotation_z(5f64.to_radians()) * DMat4::from_rotation_x(-3f64.to_radians());
        let mut moving = reference.clone();
        moving.transform(truth);
        for metric_name in ["point_to_plane", "point_to_point"] {
            let o = cook("clean.icp_align", &[geo_in(&moving), geo_in(&reference)], &[("metric", s(metric_name)), ("max_iterations", i(200)), ("sample_limit", i(100_000))])?;
            let aligned = out_geo(&o, 0)?;
            let worst = max_point_distance(&aligned, &reference);
            let tol = if metric_name == "point_to_plane" { 0.01 } else { 0.05 };
            ck.at_most(&format!("{metric_name}: worst vertex error after alignment"), worst, tol, "mm");
        }
        Ok(())
    })
}

fn qem_bounds() -> Case {
    run_case("A.clean.qem_bounds", TIER, "Quadric decimation stays within its error bound on a sphere", "Garland & Heckbert 1997; independent check against the analytic sphere and the input mesh", |ck| {
        let r = 50.0;
        let g = out_geo(&cook("geo.sphere", &[], &[("radius", f(r)), ("rows", i(48)), ("cols", i(96))])?, 0)?;
        let n_in = g.triangulated().len() as f64;
        let o = cook("clean.decimate", &[geo_in(&g)], &[("mode", s("ratio")), ("ratio", f(0.25))])?;
        let d = out_geo(&o, 0)?;
        let n_out = d.triangulated().len() as f64;
        ck.close("triangle count ratio", n_out / n_in, 0.25, 0.05, 0.0, "");
        let radial = d.points.iter().map(|p| (p.length() - r).abs()).fold(0.0, f64::max);
        ck.at_most("vertex distance from the analytic sphere (2% of r)", radial, 0.02 * r, "mm");
        ck.close("volume vs original sphere", measure(&d)?.1, measure(&g)?.1, 0.05, 0.0, "mm3");
        let bound = 0.4;
        let o2 = cook("clean.decimate", &[geo_in(&g)], &[("mode", s("max_error")), ("max_error", f(bound))])?;
        let d2 = out_geo(&o2, 0)?;
        let dist = cook("clean.mesh_distance", &[geo_in(&d2), geo_in(&g)], &[("signed", b(false))])?;
        let dv = out_geo(&dist, 0)?.point_attribs.floats("dist").map(|v| v.iter().fold(0.0f64, |m, x| m.max(x.abs()))).unwrap_or(f64::NAN);
        ck.at_most("max_error mode: measured vertex-to-input distance", dv, bound, "mm");
        Ok(())
    })
}

fn flatten(g: &Geometry) -> Result<(Geometry, vernon_core::Report), String> {
    let o = cook("flatten.conformal_map", &[geo_in(g), None], &[])?;
    Ok((out_geo(&o, 1)?, out_report(&o)?))
}

fn flatten_developable() -> Case {
    run_case("A.flatten.developable_cylinder", TIER, "Developable cylinder strip flattens isometrically", "A cylinder is developable (Gaussian curvature 0): the flat pattern is arc length x width with zero strain (do Carmo, Differential Geometry of Curves and Surfaces)", |ck| {
        let (r, ang, w) = (100.0, 90f64.to_radians(), 200.0);
        let g = cylinder_strip(r, ang, w, 60, 40);
        let (pat, rep) = flatten(&g)?;
        let size = pat.bounds().size();
        let mut dims = [size.x, size.y, size.z];
        dims.sort_by(|a, b| b.total_cmp(a));
        ck.close("pattern long side = strip width", dims[0], w, 0.01, 0.0, "mm");
        ck.close("pattern short side = arc length r*theta", dims[1], r * ang, 0.01, 0.0, "mm");
        ck.at_most("worst length change", metric(&rep, "strain_max_pct")?, 0.5, "%");
        Ok(())
    })
}

fn flatten_negative_control() -> Case {
    run_case("A.flatten.sphere_cap_distorts", TIER, "Negative control: a sphere cap cannot flatten without distortion", "Gauss's Theorema Egregium: K != 0 means no isometric flat map exists", |ck| {
        let g = sphere_cap(100.0, 70f64.to_radians(), 48, 20);
        let (_, rep) = flatten(&g)?;
        ck.at_least("worst length change is reported (> 1%)", metric(&rep, "strain_max_pct")?, 1.0, "%");
        Ok(())
    })
}

fn snap_fit() -> Case {
    run_case("A.print.snap_fit_beam", TIER, "Snap-fit cantilever strain eps = 1.5 t y / L^2", "Rectangular constant-section cantilever: curvature at root gives eps = c kappa with y = kappa L^2 / 3, so eps = 1.5 t y / L^2 (standard snap-fit design relation)", |ck| {
        use vernon_nodes_print::snap::HookDimensions;
        for (t, y, l) in [(1.2_f64, 0.6_f64, 8.0_f64), (2.0, 0.5, 15.0), (1.5, 1.0, 12.0)] {
            let h = HookDimensions { width: 6.0, thickness: t, length: l, undercut: y, lead_angle_deg: 35.0, flex_room: y.max(0.6), clearance: 0.15 };
            h.validate()?;
            ck.close(&format!("strain t={t} y={y} L={l}"), h.strain_pct(), 100.0 * 1.5 * t * y / (l * l), 1e-12, 0.0, "%");
        }
        let base = HookDimensions { width: 6.0, thickness: 1.2, length: 8.0, undercut: 0.5, lead_angle_deg: 35.0, flex_room: 0.6, clearance: 0.15 };
        let longer = HookDimensions { length: 16.0, ..base };
        let deeper = HookDimensions { undercut: 1.0, flex_room: 1.0, ..base };
        ck.close("2x length -> 1/4 strain", longer.strain_pct() / base.strain_pct(), 0.25, 1e-12, 0.0, "");
        ck.close("2x undercut -> 2x strain", deeper.strain_pct() / base.strain_pct(), 2.0, 1e-12, 0.0, "");
        Ok(())
    })
}

fn unit_round_trips() -> Case {
    run_case("A.units.set_units", TIER, "Unit conversion scales exactly and round-trips", "Exact definitions: 1 cm = 10 mm, 1 m = 1000 mm, 1 in = 25.4 mm (NIST SP 811)", |ck| {
        let g = heightfield(2_000, 1.0);
        let size = g.bounds().size();
        for (unit, factor) in [("cm", 10.0), ("m", 1000.0), ("in", 25.4), ("mm", 1.0)] {
            let o = cook("capture.set_units", &[geo_in(&g)], &[("from_unit", s(unit))])?;
            let big = out_geo(&o, 0)?;
            ck.close(&format!("{unit}->mm bbox x scale"), big.bounds().size().x / size.x, factor, 1e-12, 0.0, "");
            let mut back = big.clone();
            back.transform(DMat4::from_scale(DVec3::splat(1.0 / factor)));
            ck.at_most(&format!("{unit} round trip worst vertex error"), max_point_distance(&back, &g), 1e-12 * factor.max(1.0), "mm");
        }
        Ok(())
    })
}

fn file_round_trips() -> Case {
    run_case("A.io.file_round_trip", TIER, "Mesh file write/read round trips in millimetres", "Format definitions: ASCII OBJ/PLY keep double-precision text, binary STL stores 32-bit floats", |ck| {
        let g = heightfield(2_000, 300.0);
        let dir = scratch("io_round_trip");
        for (ext, tol) in [("obj", 1e-6), ("ply", 1e-6), ("stl", 5e-3)] {
            let path = dir.join(format!("rt.{ext}"));
            vernon_io::write(&path, &g, &Default::default()).map_err(|e| e.to_string())?;
            let back = vernon_io::read(&path, &Default::default()).map_err(|e| e.to_string())?;
            ck.close(&format!("{ext}: triangle count"), back.triangulated().len() as f64, g.triangulated().len() as f64, 0.0, 0.0, "");
            let bb = (back.bounds().size() - g.bounds().size()).abs().max_element();
            ck.at_most(&format!("{ext}: bounding-box size error"), bb, tol, "mm");
        }
        Ok(())
    })
}

fn ahmed_published_window() -> Case {
    const REF: &str = "Ahmed, Ramm & Faltin 1984, SAE 840300; ERCOFTAC case AC 1-05 https://kbwiki.ercoftac.org/w/index.php/AC_1-05 ; reference values 0.299 (25 deg) and 0.279 (35 deg) from a secondary table https://skill-lync.com/student-projects/project-1-flow-over-ahmed-body-7 (NOT the primary paper; confirm before quoting)";
    let dir = std::env::var_os("VERNON_AHMED_RESULTS").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(r"C:\Users\mabdu\REA\vernon\scratch\wp9-ahmed"));
    if !dir.is_dir() {
        return skipped("A.cfd.ahmed_drag_window", TIER, "Ahmed body drag within published window", REF, "no completed Ahmed-body run on this machine (set VERNON_AHMED_RESULTS)");
    }
    run_case("A.cfd.ahmed_drag_window", TIER, "Ahmed body drag within published window (existing OpenFOAM runs, read-only)", REF, |ck| {
        let mut any = false;
        for (label, angle, reference) in [("a25-g3", 25.0, 0.299), ("a35-g3", 35.0, 0.279)] {
            let Ok(text) = std::fs::read_to_string(dir.join(format!("{label}.json"))) else { continue };
            let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let cd = v["cd"].as_f64().ok_or("no cd")?;
            let converged = v["converged"].as_bool().unwrap_or(false);
            any = true;
            ck.note(format!("{label}: {} cells, {} iterations, converged={converged}", v["cells"], v["iterations"]));
            ck.close(&format!("Cd at {angle} deg, finest mesh, vs published ±15%"), cd, reference, 0.15, 0.0, "");
            ck.close(&format!("{label}: solver reported convergence (1 = yes)"), if converged { 1.0 } else { 0.0 }, 1.0, 0.0, 0.0, "bool");
        }
        if !any {
            ck.skip("result files a25-g3/a35-g3 not found");
        }
        Ok(())
    })
}

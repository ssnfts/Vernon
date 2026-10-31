//! Acceptance of the reference solver: a cantilever 200 x 20 x 20 mm, E = 2 GPa,
//! tip load 100 N, against beam theory `PL^3 / 3EI` = 10.000 mm.
//!
//! Shear adds about 0.8% to the slender-beam value and the clamped root takes a
//! little away, so C3D10 should land just above it. C3D4 is the constant-strain
//! element: it locks in bending and always comes out stiff (low), which is why
//! it gets a looser tolerance and a finer mesh.

use std::time::Duration;

use glam::DVec3;
use vernon_core::Geometry;

use crate::deck::{write_deck, DeckInput, DeckOptions};
use crate::frd::{dense_tensor6, dense_vec3, parse_frd};
use crate::linalg::von_mises;
use crate::material::MaterialCard;
use crate::mesh::{box_tets, tet4_to_tet10};
use crate::refsolve::{solve, RefInput, RefOutput};
use crate::solver::{find_executable, run_with_timeout, RunStatus};
use crate::types::{FacePressure, LoadSet, MountSet, MountSite, OrientationCard};

const LENGTH: f64 = 200.0;
const SIDE: f64 = 20.0;
const E: f64 = 2000.0;
const NU: f64 = 0.3;
const LOAD: f64 = 100.0;

fn beam_theory() -> f64 {
    LOAD * LENGTH.powi(3) / (3.0 * E * SIDE.powi(4) / 12.0)
}

fn card() -> MaterialCard {
    MaterialCard::trans_iso("iso", E, E, NU, NU, E / (2.0 * (1.0 + NU)))
}

fn nodes_at(mesh: &Geometry, x: f64) -> Vec<u32> {
    (0..mesh.points.len() as u32).filter(|&i| (mesh.points[i as usize].x - x).abs() < 1e-9).collect()
}

fn mounts(mesh: &Geometry) -> MountSet {
    let site = MountSite { name: "root".into(), center: [-LENGTH / 2.0, 0.0, 0.0], axis: [-1.0, 0.0, 0.0], nodes: nodes_at(mesh, -LENGTH / 2.0), model: "fixed_patch".into(), r_w_mm: 5.0, avg_radius_mm: 5.0, bolt_d_mm: 5.0, pad_e_mpa: 5.0, pad_t_mm: 2.0 };
    MountSet { sites: vec![site] }
}

/// The tip load as a uniform downward traction over the end face, in consistent nodal forces.
fn tip_load(mesh: &Geometry) -> LoadSet {
    let traction = LOAD / (SIDE * SIDE);
    let mut forces = Vec::new();
    for b in crate::mesh::boundary_faces(mesh).iter().filter(|b| b.normal.x > 0.99) {
        // a unit pressure gives the nodes and the share of the area; only the direction changes
        let unit = FacePressure { elem: b.elem as u32, face: b.face, p_mpa: 1.0 };
        for (node, f) in crate::loads::face_nodal_forces(mesh, &unit) {
            forces.push(crate::types::NodalForce { node, f_n: [0.0, 0.0, -traction * f.length()] });
        }
    }
    LoadSet { forces, ..Default::default() }
}

fn run(mesh: &Geometry) -> RefOutput {
    let (c, id, m, l) = (card(), OrientationCard::identity(), mounts(mesh), tip_load(mesh));
    solve(&RefInput { mesh, card: &c, orient: &id, mounts: &m, load: &l, max_dof: 300_000, tol: 1e-10 }).unwrap()
}

fn tip_deflection(mesh: &Geometry, disp: &[DVec3]) -> f64 {
    let tip = nodes_at(mesh, LENGTH / 2.0);
    tip.iter().map(|&n| -disp[n as usize].z).sum::<f64>() / tip.len() as f64
}

fn mesh_tet4(n: usize) -> Geometry {
    box_tets(DVec3::new(LENGTH, SIDE, SIDE), [10 * n, n, n])
}

fn mesh_tet10(n: usize) -> Geometry {
    tet4_to_tet10(&mesh_tet4(n)).unwrap()
}

#[test]
fn c3d10_is_within_3_percent_of_beam_theory() {
    let mesh = mesh_tet10(2);
    let o = run(&mesh);
    let d = tip_deflection(&mesh, &o.disp);
    let err = (d - beam_theory()) / beam_theory();
    println!("C3D10 tip {d:.4} mm, beam {:.4} mm, error {:+.2}%, {} dof, {} iterations, {}", beam_theory(), 100.0 * err, 3 * mesh.points.len(), o.iterations, o.preconditioner);
    assert!(err.abs() < 0.03, "{}", 100.0 * err);
    // the supports carry the load
    let r = o.reactions["MOUNT1"];
    assert!((r[2] - LOAD).abs() < 1e-3 * LOAD && r[0].abs() < 1e-3 * LOAD, "{r:?}");
    // bending stress PLy/I at the fibre, half way along
    let want = LOAD * (LENGTH / 2.0) * (SIDE / 2.0) / (SIDE.powi(4) / 12.0);
    let peak = nodes_at(&mesh, 0.0).iter().map(|&n| von_mises(&o.node_stress[n as usize])).fold(0.0, f64::max);
    println!("C3D10 mid-span fibre von Mises {peak:.3} MPa, beam {want:.3} MPa");
    assert!((peak - want).abs() < 0.10 * want, "{peak} {want}");
    assert_eq!(o.elem_vm().len(), mesh.prims.len());
    // refining moves it towards the value with shear included, about +0.8%
    let fine = mesh_tet10(4);
    let f = tip_deflection(&fine, &run(&fine).disp);
    println!("C3D10 refined tip {f:.4} mm, error {:+.2}%", 100.0 * (f - beam_theory()) / beam_theory());
    assert!(f > d && (f - 1.008 * beam_theory()).abs() < 0.01 * beam_theory(), "{f}");
}

#[test]
fn c3d4_is_within_15_percent_of_beam_theory_and_stiff() {
    let mesh = mesh_tet4(8);
    let o = run(&mesh);
    let d = tip_deflection(&mesh, &o.disp);
    let err = (d - beam_theory()) / beam_theory();
    println!("C3D4 tip {d:.4} mm, beam {:.4} mm, error {:+.2}%, {} dof, {} iterations, {}", beam_theory(), 100.0 * err, 3 * mesh.points.len(), o.iterations, o.preconditioner);
    assert!(err.abs() < 0.15, "{}", 100.0 * err);
    assert!(err < 0.0, "linear tetrahedra must come out stiff: {}", 100.0 * err);
    // the same coarse mesh shows why quadratic elements are the structural default
    let (coarse4, coarse10) = (mesh_tet4(2), mesh_tet10(2));
    let (a, b) = (tip_deflection(&coarse4, &run(&coarse4).disp), tip_deflection(&coarse10, &run(&coarse10).disp));
    println!("coarse mesh: C3D4 {a:.4} mm, C3D10 {b:.4} mm");
    assert!(a < 0.75 * b, "{a} {b}");
}

#[test]
fn quadratic_elements_on_stiff_pads_carry_an_end_pressure() {
    let mesh = tet4_to_tet10(&box_tets(DVec3::new(40.0, 20.0, 20.0), [2, 1, 1])).unwrap();
    let pressures = crate::mesh::boundary_faces(&mesh).iter().filter(|b| b.normal.x > 0.99).map(|b| FacePressure { elem: b.elem as u32, face: b.face, p_mpa: -5.0 }).collect();
    let nodes = nodes_at(&mesh, -20.0);
    let (c, id) = (card(), OrientationCard::identity());
    let site = MountSite { name: "end".into(), center: [-20.0, 0.0, 0.0], axis: [-1.0, 0.0, 0.0], nodes, model: "spring_pad".into(), r_w_mm: 1000.0, avg_radius_mm: 5.0, bolt_d_mm: 5.0, pad_e_mpa: 1e6, pad_t_mm: 1.0 };
    let o = solve(&RefInput { mesh: &mesh, card: &c, orient: &id, mounts: &MountSet { sites: vec![site] }, load: &LoadSet { pressures, ..Default::default() }, max_dof: 60000, tol: 1e-12 }).unwrap();
    let r = o.reactions["MOUNT1"];
    assert!((r[0] + 5.0 * 400.0).abs() < 1e-6 * 2000.0, "{r:?}");
    // away from the supported end the section carries the 5 MPa
    let last = o.elem_stress.iter().enumerate().filter(|(e, _)| crate::mesh::element_centroid(&mesh, *e).x > 10.0);
    let (mut sum, mut count) = (0.0, 0.0);
    for (_, s) in last {
        sum += s[0];
        count += 1.0;
    }
    assert!((sum / count - 5.0).abs() < 0.1, "{}", sum / count);
}

#[test]
fn gravity_is_distributed_consistently_on_quadratic_elements() {
    let mesh = mesh_tet10(1);
    let (c, id, m) = (card(), OrientationCard::identity(), mounts(&mesh));
    let g = crate::types::Gravity { accel_mm_s2: 9810.0, dir: [0.0, 0.0, -1.0] };
    let o = solve(&RefInput { mesh: &mesh, card: &c, orient: &id, mounts: &m, load: &LoadSet { gravity: Some(g), ..Default::default() }, max_dof: 60000, tol: 1e-10 }).unwrap();
    let weight = c.density_t_mm3 * LENGTH * SIDE * SIDE * 9810.0;
    assert!((o.reactions["MOUNT1"][2] - weight).abs() < 1e-3 * weight, "{:?} {weight}", o.reactions);
}

/// The same deck through the external solver. Set `VERNON_CCX` or put it on the program path.
fn external(mesh: &Geometry, tag: &str) -> (Vec<DVec3>, Vec<[f64; 6]>) {
    let exe = find_executable(&std::env::var("VERNON_CCX").unwrap_or_default(), &["ccx", "ccx_2.21", "ccx_2.20", "ccx_static", "ccx_MT"]).expect("the external solver is not installed");
    let dir = std::env::temp_dir().join(format!("vernon_fea_cantilever_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (c, id, m, l) = (card(), OrientationCard::identity(), mounts(mesh), tip_load(mesh));
    let opts = DeckOptions { job: "cantilever".into(), allow_linear_tet: true, ..Default::default() };
    let deck = write_deck(&DeckInput { mesh, card: &c, orient: &id, mounts: &m, load: &l, opts: &opts }).unwrap();
    std::fs::write(dir.join("cantilever.inp"), &deck.text).unwrap();
    let st = run_with_timeout(&exe, &["-i".to_string(), "cantilever".to_string()], &dir, &dir.join("cantilever.log"), Duration::from_secs(600), &[]).unwrap();
    assert!(matches!(st, RunStatus::Exited(0)), "the solver failed, see {}", dir.display());
    let frd = parse_frd(&std::fs::read_to_string(dir.join("cantilever.frd")).unwrap()).unwrap();
    let n = mesh.points.len();
    let needed = vec![true; n];
    (dense_vec3(frd.dataset("DISP").unwrap(), n, &needed).unwrap(), dense_tensor6(frd.dataset("STRESS").unwrap(), n, &needed).unwrap())
}

fn assert_agrees(mesh: &Geometry, tag: &str) {
    let o = run(mesh);
    let (u, s) = external(mesh, tag);
    let (ours, theirs) = (tip_deflection(mesh, &o.disp), tip_deflection(mesh, &u));
    println!("{tag}: tip {ours:.4} mm here, {theirs:.4} mm external");
    assert!((ours - theirs).abs() < 0.02 * theirs.abs(), "tip {ours} {theirs}");
    let peak = |v: &[[f64; 6]]| nodes_at(mesh, 0.0).iter().map(|&n| von_mises(&v[n as usize])).fold(0.0, f64::max);
    let (ours, theirs) = (peak(&o.node_stress), peak(&s));
    println!("{tag}: mid-span von Mises {ours:.3} MPa here, {theirs:.3} MPa external");
    assert!((ours - theirs).abs() < 0.02 * theirs.abs(), "von Mises {ours} {theirs}");
}

#[test]
#[ignore = "needs CalculiX"]
fn c3d10_agrees_with_the_external_solver_within_2_percent() {
    assert_agrees(&mesh_tet10(2), "c3d10");
}

#[test]
#[ignore = "needs CalculiX"]
fn c3d4_agrees_with_the_external_solver_within_2_percent() {
    assert_agrees(&mesh_tet4(8), "c3d4");
}

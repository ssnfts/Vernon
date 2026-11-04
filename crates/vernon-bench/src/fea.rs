//! Cantilever and patch-test models for the built-in linear solver, mirroring `vernon-nodes-fea`'s acceptance fixtures.

use vernon_core::{DVec3, Geometry};
use vernon_nodes_fea::material::MaterialCard;
use vernon_nodes_fea::mesh::{boundary_faces, box_tets, tet4_to_tet10};
use vernon_nodes_fea::refsolve::{solve, RefInput, RefOutput};
use vernon_nodes_fea::types::{FacePressure, LoadSet, MountSet, MountSite, NodalForce, OrientationCard};

pub const LENGTH: f64 = 200.0;
pub const SIDE: f64 = 20.0;
pub const E_MPA: f64 = 2000.0;
pub const NU: f64 = 0.3;
pub const LOAD_N: f64 = 100.0;

pub fn card() -> MaterialCard {
    MaterialCard::trans_iso("iso", E_MPA, E_MPA, NU, NU, E_MPA / (2.0 * (1.0 + NU)))
}
pub fn nodes_at(mesh: &Geometry, x: f64) -> Vec<u32> {
    (0..mesh.points.len() as u32).filter(|&i| (mesh.points[i as usize].x - x).abs() < 1e-9).collect()
}
pub fn mesh_tet4(n: usize) -> Geometry {
    box_tets(DVec3::new(LENGTH, SIDE, SIDE), [10 * n, n, n])
}
pub fn mesh_tet10(n: usize) -> Geometry {
    tet4_to_tet10(&mesh_tet4(n)).unwrap()
}
pub fn root_mount(mesh: &Geometry) -> MountSet {
    MountSet { sites: vec![MountSite { name: "root".into(), center: [-LENGTH / 2.0, 0.0, 0.0], axis: [-1.0, 0.0, 0.0], nodes: nodes_at(mesh, -LENGTH / 2.0), model: "fixed_patch".into(), r_w_mm: 5.0, avg_radius_mm: 5.0, bolt_d_mm: 5.0, pad_e_mpa: 5.0, pad_t_mm: 2.0 }] }
}
/// Uniform downward traction over the +X end face, total `LOAD_N`.
pub fn tip_load(mesh: &Geometry) -> LoadSet {
    let traction = LOAD_N / (SIDE * SIDE);
    let mut forces = Vec::new();
    for bf in boundary_faces(mesh).iter().filter(|b| b.normal.x > 0.99) {
        let unit = FacePressure { elem: bf.elem as u32, face: bf.face, p_mpa: 1.0 };
        for (node, f) in vernon_nodes_fea::loads::face_nodal_forces(mesh, &unit) {
            forces.push(NodalForce { node, f_n: [0.0, 0.0, -traction * f.length()] });
        }
    }
    LoadSet { forces, ..Default::default() }
}
pub fn solve_cantilever(mesh: &Geometry, max_dof: usize) -> Result<RefOutput, String> {
    let (c, id, m, l) = (card(), OrientationCard::identity(), root_mount(mesh), tip_load(mesh));
    solve(&RefInput { mesh, card: &c, orient: &id, mounts: &m, load: &l, max_dof, tol: 1e-10 })
}
pub fn tip_deflection(mesh: &Geometry, disp: &[DVec3]) -> f64 {
    let tip = nodes_at(mesh, LENGTH / 2.0);
    tip.iter().map(|&n| -disp[n as usize].z).sum::<f64>() / tip.len() as f64
}
/// Euler-Bernoulli tip deflection P L^3 / (3 E I).
pub fn beam_theory() -> f64 {
    LOAD_N * LENGTH.powi(3) / (3.0 * E_MPA * SIDE.powi(4) / 12.0)
}

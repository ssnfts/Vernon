//! Native reference solver: linear static analysis with four-node (C3D4) and
//! ten-node (C3D10) tetrahedra, assembled in CSR and solved by preconditioned
//! conjugate gradient ([`crate::sparse`]).
//!
//! It runs the chain end to end without the external solver and gives tests an
//! oracle. C3D4 has constant strain and is stiff in bending: a cantilever needs
//! a fine mesh to come near beam theory (see `cantilever.rs`). C3D10 uses the
//! four-point rule, so a straight-sided element is integrated exactly.
//!
//! Stress components are `[xx, yy, zz, xy, yz, zx]`, strain is engineering.

#![allow(clippy::needless_range_loop)]

use std::collections::BTreeMap;
use std::fmt::Write as _;

use glam::{DMat3, DVec3};
use vernon_core::Geometry;

use crate::frd::format_frd_value;
use crate::linalg::{rotate_stiffness_to_global, von_mises, M6};
use crate::loads::face_nodal_forces;
use crate::material::{MaterialCard, ROOM_C};
use crate::mesh::{corners, element_size, EDGES10};
use crate::sparse::{pcg, Csr};
use crate::types::{LoadSet, MountSet, OrientationCard};

pub struct RefInput<'a> {
    pub mesh: &'a Geometry,
    pub card: &'a MaterialCard,
    pub orient: &'a OrientationCard,
    pub mounts: &'a MountSet,
    pub load: &'a LoadSet,
    pub max_dof: usize,
    pub tol: f64,
}

#[derive(Debug, Clone)]
pub struct RefOutput {
    /// Displacement of every mesh point (mm).
    pub disp: Vec<DVec3>,
    /// Stress at the integration points of every element (MPa): one for C3D4, four for C3D10.
    pub ip_stress: Vec<Vec<[f64; 6]>>,
    /// Volume-averaged stress of every element (MPa).
    pub elem_stress: Vec<[f64; 6]>,
    /// Volume-averaged stress at every mesh point, extrapolated from the integration points (MPa).
    pub node_stress: Vec<[f64; 6]>,
    /// Support reaction totals per mount, keyed `MOUNT<k>` (N).
    pub reactions: BTreeMap<String, [f64; 3]>,
    pub volume: f64,
    pub iterations: usize,
    pub residual: f64,
    pub preconditioner: &'static str,
}

impl RefOutput {
    pub fn max_disp(&self) -> f64 {
        self.disp.iter().map(|d| d.length()).fold(0.0, f64::max)
    }

    pub fn max_vm(&self) -> f64 {
        self.node_stress.iter().map(von_mises).fold(0.0, f64::max)
    }

    /// Von Mises stress of every element's mean stress (MPa).
    pub fn elem_vm(&self) -> Vec<f64> {
        self.elem_stress.iter().map(von_mises).collect()
    }
}

/// Small and large barycentric coordinate of the four-point rule for tetrahedra (degree 2).
const IP_A: f64 = 0.138_196_601_125_010_5;
const IP_B: f64 = 0.585_410_196_624_968_5;

/// Barycentric coordinates of the integration points; every point has the same weight.
fn integration_points(nen: usize) -> Vec<[f64; 4]> {
    if nen == 4 {
        vec![[0.25; 4]]
    } else {
        (0..4).map(|i| std::array::from_fn(|k| if k == i { IP_B } else { IP_A })).collect()
    }
}

/// Shape function values and their derivatives with respect to the barycentric coordinates.
fn shape(nen: usize, l: &[f64; 4]) -> (Vec<f64>, Vec<[f64; 4]>) {
    let mut n = vec![0.0; nen];
    let mut d = vec![[0.0; 4]; nen];
    if nen == 4 {
        for a in 0..4 {
            n[a] = l[a];
            d[a][a] = 1.0;
        }
    } else {
        for a in 0..4 {
            n[a] = l[a] * (2.0 * l[a] - 1.0);
            d[a][a] = 4.0 * l[a] - 1.0;
        }
        for (k, &(a, b)) in EDGES10.iter().enumerate() {
            n[4 + k] = 4.0 * l[a] * l[b];
            d[4 + k][a] = 4.0 * l[b];
            d[4 + k][b] = 4.0 * l[a];
        }
    }
    (n, d)
}

/// Strain-displacement matrix (6 rows of `3 nen`, row-major) and the Jacobian
/// determinant at one point; `None` when the mapping is inverted.
fn b_matrix(x: &[DVec3], dl: &[[f64; 4]]) -> Option<(Vec<f64>, f64)> {
    let mut cols = [DVec3::ZERO; 3];
    for (p, d) in x.iter().zip(dl) {
        for k in 0..3 {
            cols[k] += *p * (d[k + 1] - d[0]);
        }
    }
    let j = DMat3::from_cols(cols[0], cols[1], cols[2]);
    let det = j.determinant();
    if det <= 1e-300 {
        return None;
    }
    let inv = j.inverse();
    // the rows of the inverse Jacobian are the gradients of the three independent coordinates
    let rows: [DVec3; 3] = std::array::from_fn(|k| DVec3::new(inv.col(0)[k], inv.col(1)[k], inv.col(2)[k]));
    let nd = 3 * x.len();
    let mut b = vec![0.0; 6 * nd];
    for (a, d) in dl.iter().enumerate() {
        let gr = rows[0] * (d[1] - d[0]) + rows[1] * (d[2] - d[0]) + rows[2] * (d[3] - d[0]);
        let c = 3 * a;
        b[c] = gr.x;
        b[nd + c + 1] = gr.y;
        b[2 * nd + c + 2] = gr.z;
        b[3 * nd + c] = gr.y;
        b[3 * nd + c + 1] = gr.x;
        b[4 * nd + c + 1] = gr.z;
        b[4 * nd + c + 2] = gr.y;
        b[5 * nd + c] = gr.z;
        b[5 * nd + c + 2] = gr.x;
    }
    Some((b, det))
}

struct Elem {
    /// Compact node numbers.
    n: Vec<usize>,
    /// Stress of unit nodal displacements, `D B`, per integration point (6 rows of `3 nen`).
    sb: Vec<Vec<f64>>,
    vol: f64,
}

/// Solve one load case.
pub fn solve(inp: &RefInput) -> Result<RefOutput, String> {
    let mesh = inp.mesh;
    let nen = element_size(mesh)?;
    let nd = 3 * nen;
    let mut compact: Vec<Option<usize>> = vec![None; mesh.points.len()];
    let mut used: Vec<u32> = Vec::new();
    for prim in &mesh.prims {
        for &c in &prim.vertices {
            if compact[c as usize].is_none() {
                compact[c as usize] = Some(used.len());
                used.push(c);
            }
        }
    }
    let ndof = 3 * used.len();
    if ndof > inp.max_dof {
        return Err(format!("{ndof} unknowns exceed the limit of {} for the reference solver: use the external solver or a coarser mesh", inp.max_dof));
    }
    if inp.mounts.sites.is_empty() {
        return Err("no mounts: the body would be free to move".into());
    }
    let q = if inp.card.mode == "isotropic_z" { crate::linalg::IDENTITY3 } else { inp.orient.axes };
    let d_mat: M6 = inp.card.stiffness().ok_or("the material stiffness is singular")?;
    let d_glob = rotate_stiffness_to_global(&d_mat, &q).ok_or("cannot rotate the stiffness into the model frame")?;
    let temps = mesh.point_attribs.floats("temp_c").ok();
    let accel = inp.load.gravity.map(|g| DVec3::from(g.dir) * g.accel_mm_s2);

    // element stiffness, assembled into CSR; loads from the body force on the way
    let ips = integration_points(nen);
    let shapes: Vec<_> = ips.iter().map(|l| shape(nen, l)).collect();
    let elem_nodes: Vec<Vec<usize>> = mesh.prims.iter().map(|p| p.vertices.iter().map(|&v| compact[v as usize].unwrap_or(0)).collect()).collect();
    let mut adjacent: Vec<Vec<u32>> = vec![Vec::new(); used.len()];
    for n in &elem_nodes {
        for &a in n {
            adjacent[a].extend(n.iter().map(|&b| b as u32));
        }
    }
    let rows = adjacent
        .iter_mut()
        .flat_map(|adj| {
            adj.sort_unstable();
            adj.dedup();
            let row: Vec<u32> = adj.iter().flat_map(|&b| (0..3).map(move |d| 3 * b + d)).collect();
            [row.clone(), row.clone(), row]
        })
        .collect();
    let mut k = Csr::from_pattern(rows);
    let mut f = vec![0.0; ndof];
    let mut elems = Vec::with_capacity(mesh.prims.len());
    let mut ke = vec![0.0; nd * nd];
    for (e, prim) in mesh.prims.iter().enumerate() {
        let x: Vec<DVec3> = prim.vertices.iter().map(|&v| mesh.points[v as usize]).collect();
        let c = corners(mesh, e);
        ke.iter_mut().for_each(|v| *v = 0.0);
        let mut sb_all = Vec::with_capacity(ips.len());
        let mut vol = 0.0;
        let n = &elem_nodes[e];
        for (l, (sf, dl)) in ips.iter().zip(&shapes) {
            let (b, det) = b_matrix(&x, dl).ok_or_else(|| format!("element {} has a non-positive volume", e + 1))?;
            let dv = det / 6.0 / ips.len() as f64;
            vol += dv;
            let t = temps.map_or(ROOM_C, |t| (0..4).map(|a| l[a] * t[c[a] as usize]).sum::<f64>());
            let r = inp.card.retention_at(t);
            let mut sb = vec![0.0; 6 * nd];
            for i in 0..6 {
                for j in 0..nd {
                    sb[i * nd + j] = r * (0..6).map(|m| d_glob[i][m] * b[m * nd + j]).sum::<f64>();
                }
            }
            for i in 0..nd {
                for j in 0..nd {
                    ke[nd * i + j] += dv * (0..6).map(|m| b[m * nd + i] * sb[m * nd + j]).sum::<f64>();
                }
            }
            if let Some(a) = accel {
                for (&node, s) in n.iter().zip(sf) {
                    let fe = a * (inp.card.density_t_mm3 * dv * s);
                    f[3 * node] += fe.x;
                    f[3 * node + 1] += fe.y;
                    f[3 * node + 2] += fe.z;
                }
            }
            sb_all.push(sb);
        }
        for i in 0..nd {
            for j in 0..nd {
                k.add(3 * n[i / 3] + i % 3, 3 * n[j / 3] + j % 3, ke[nd * i + j]);
            }
        }
        elems.push(Elem { n: n.clone(), sb: sb_all, vol });
    }

    // constraints and springs; a rigid washer whose reference node is held is a fixed patch
    let mut fixed = vec![false; ndof];
    let mut springs = vec![0.0; ndof];
    let mut mount_dofs: Vec<Vec<usize>> = Vec::new();
    for site in &inp.mounts.sites {
        let nodes: std::collections::BTreeSet<usize> = site.nodes.iter().filter_map(|&n| compact.get(n as usize).copied().flatten()).collect();
        if nodes.is_empty() {
            return Err(format!("mount '{}' has no nodes in the mesh", site.name));
        }
        let mut dofs = Vec::new();
        let k_node = (site.model == "spring_pad").then(|| {
            let area = std::f64::consts::PI * site.r_w_mm * site.r_w_mm;
            site.pad_e_mpa * area / site.pad_t_mm.max(1e-6) / nodes.len() as f64
        });
        for &n in &nodes {
            for d in 0..3 {
                match k_node {
                    Some(s) => {
                        springs[3 * n + d] += s;
                        k.add(3 * n + d, 3 * n + d, s);
                    }
                    None => fixed[3 * n + d] = true,
                }
                dofs.push(3 * n + d);
            }
        }
        mount_dofs.push(dofs);
    }

    // surface loads use flat faces, as the nodal-force form of the deck does
    let mut add_force = |node: usize, v: [f64; 3]| {
        for d in 0..3 {
            f[3 * node + d] += v[d];
        }
    };
    for fp in &inp.load.pressures {
        for (node, v) in face_nodal_forces(mesh, fp) {
            add_force(compact[node as usize].unwrap_or(0), v.into());
        }
    }
    for nf in &inp.load.forces {
        let p = mesh.points.get(nf.node as usize).ok_or("a nodal force refers to a node outside the mesh")?;
        // a force on a point outside every element goes to the nearest node
        let node = match compact[nf.node as usize] {
            Some(c) => c,
            None => (0..used.len())
                .min_by(|&a, &b| (mesh.points[used[a] as usize] - *p).length_squared().total_cmp(&(mesh.points[used[b] as usize] - *p).length_squared()))
                .ok_or("empty mesh")?,
        };
        add_force(node, nf.f_n);
    }

    // conjugate gradient on the free dofs
    let free: Vec<bool> = fixed.iter().map(|x| !x).collect();
    let reduced = k.submatrix(&free);
    let b: Vec<f64> = f.iter().zip(&free).filter(|(_, fr)| **fr).map(|(v, _)| *v).collect();
    let sol = pcg(&reduced, &b, inp.tol)?;
    let mut u = vec![0.0; ndof];
    let mut xs = sol.x.iter();
    for (v, fr) in u.iter_mut().zip(&free) {
        if *fr {
            *v = *xs.next().unwrap_or(&0.0);
        }
    }

    // reactions: support force on the body = K u - f at the constrained dofs (springs: -k u)
    let mut ku = vec![0.0; ndof];
    k.mul(&u, &mut ku);
    let mut reactions = BTreeMap::new();
    for (m, dofs) in mount_dofs.iter().enumerate() {
        let mut r = [0.0; 3];
        for &d in dofs {
            if fixed[d] {
                r[d % 3] += ku[d] - f[d];
            } else {
                r[d % 3] += -springs[d] * u[d];
            }
        }
        reactions.insert(format!("MOUNT{}", m + 1), r);
    }

    // element and nodal results
    let mut ip_stress = Vec::with_capacity(elems.len());
    let mut elem_stress = Vec::with_capacity(elems.len());
    let mut acc = vec![([0.0f64; 6], 0.0f64); mesh.points.len()];
    for (el, prim) in elems.iter().zip(&mesh.prims) {
        let mut ue = vec![0.0; nd];
        for (a, &n) in el.n.iter().enumerate() {
            ue[3 * a..3 * a + 3].copy_from_slice(&u[3 * n..3 * n + 3]);
        }
        let at_ips: Vec<[f64; 6]> = el
            .sb
            .iter()
            .map(|sb| std::array::from_fn(|i| sb[i * nd..(i + 1) * nd].iter().zip(&ue).map(|(a, b)| a * b).sum()))
            .collect();
        // linear field through the four points: corner j = sum (delta_ij - a) / (b - a) s_i
        let at_corner = |j: usize| -> [f64; 6] {
            if at_ips.len() == 1 {
                return at_ips[0];
            }
            std::array::from_fn(|c| (0..4).map(|i| (f64::from(u8::from(i == j)) - IP_A) / (IP_B - IP_A) * at_ips[i][c]).sum())
        };
        let cs: [[f64; 6]; 4] = std::array::from_fn(at_corner);
        for (a, &v) in prim.vertices.iter().enumerate() {
            let s: [f64; 6] = if a < 4 {
                cs[a]
            } else {
                let (p, q) = EDGES10[a - 4];
                std::array::from_fn(|c| 0.5 * (cs[p][c] + cs[q][c]))
            };
            let t = &mut acc[v as usize];
            for (x, y) in t.0.iter_mut().zip(s.iter()) {
                *x += y * el.vol;
            }
            t.1 += el.vol;
        }
        let w = 1.0 / at_ips.len() as f64;
        elem_stress.push(std::array::from_fn(|c| w * at_ips.iter().map(|s| s[c]).sum::<f64>()));
        ip_stress.push(at_ips);
    }
    let mut disp = vec![DVec3::ZERO; mesh.points.len()];
    for (m, &c) in used.iter().enumerate() {
        disp[c as usize] = DVec3::new(u[3 * m], u[3 * m + 1], u[3 * m + 2]);
    }
    let node_stress = acc.iter().map(|(s, w)| if *w > 0.0 { s.map(|v| v / w) } else { [0.0; 6] }).collect();
    let volume = elems.iter().map(|e| e.vol).sum();
    Ok(RefOutput { disp, ip_stress, elem_stress, node_stress, reactions, volume, iterations: sol.iterations, residual: sol.residual, preconditioner: sol.preconditioner })
}

/// The `.dat` text in the layout the solver writes, so the same reader serves both.
pub fn dat_text(out: &RefOutput) -> String {
    let mut s = String::new();
    for (name, r) in &out.reactions {
        let _ = writeln!(s, " total force (fx,fy,fz) for set {name} and time  0.1000000E+01\n");
        let _ = writeln!(s, "  {}  {}  {}\n", format_frd_value(r[0]), format_frd_value(r[1]), format_frd_value(r[2]));
    }
    let _ = writeln!(s, " stresses (elem, integ.pnt.,sxx,syy,szz,sxy,sxz,syz) for set EALL and time  0.1000000E+01\n");
    for (e, ips) in out.ip_stress.iter().enumerate() {
        for (ip, t) in ips.iter().enumerate() {
            let _ = writeln!(s, "{:>10}{:>5}  {}  {}  {}  {}  {}  {}", e + 1, ip + 1, format_frd_value(t[0]), format_frd_value(t[1]), format_frd_value(t[2]), format_frd_value(t[3]), format_frd_value(t[5]), format_frd_value(t[4]));
        }
    }
    let _ = writeln!(s, "\n total volume for set EALL and time  0.1000000E+01\n\n  {}", format_frd_value(out.volume));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frd::{dense_tensor6, dense_vec3, parse_dat, parse_frd, write_frd};
    use crate::mesh::{box_tets, tet4_to_tet10};
    use crate::types::{FacePressure, MountSite};

    /// Bar along X, 100 x 10 x 10 mm, one end clamped, the other loaded by a pressure.
    fn bar(card: &MaterialCard, orient: &OrientationCard, p_mpa: f64) -> RefOutput {
        let mesh = tet4_to_tet10(&box_tets(DVec3::new(100.0, 10.0, 10.0), [10, 1, 1])).unwrap();
        let left: Vec<u32> = (0..mesh.points.len() as u32).filter(|&i| (mesh.points[i as usize].x + 50.0).abs() < 1e-9).collect();
        let faces = crate::mesh::boundary_faces(&mesh);
        let pressures = faces.iter().filter(|b| b.normal.x > 0.99).map(|b| FacePressure { elem: b.elem as u32, face: b.face, p_mpa: -p_mpa }).collect();
        let site = MountSite { name: "m".into(), center: [-50.0, 0.0, 0.0], axis: [-1.0, 0.0, 0.0], nodes: left, model: "fixed_patch".into(), r_w_mm: 5.0, avg_radius_mm: 5.0, bolt_d_mm: 5.0, pad_e_mpa: 5.0, pad_t_mm: 2.0 };
        let mounts = MountSet { sites: vec![site] };
        let load = LoadSet { pressures, ..Default::default() };
        solve(&RefInput { mesh: &mesh, card, orient, mounts: &mounts, load: &load, max_dof: 60000, tol: 1e-12 }).unwrap()
    }

    fn card() -> MaterialCard {
        MaterialCard::trans_iso("t", 2500.0, 2000.0, 0.35, 0.30, 800.0)
    }

    /// Elongation of the 80 mm length that starts 20 mm from the clamped end
    /// (clear of the clamping disturbance), scaled to the 100 mm bar.
    fn elongation(o: &RefOutput) -> f64 {
        let mesh = tet4_to_tet10(&box_tets(DVec3::new(100.0, 10.0, 10.0), [10, 1, 1])).unwrap();
        let at = |x: f64| {
            let v: Vec<f64> = (0..mesh.points.len()).filter(|&i| (mesh.points[i].x - x).abs() < 1e-9).map(|i| o.disp[i].x).collect();
            v.iter().sum::<f64>() / v.len() as f64
        };
        (at(50.0) - at(-30.0)) / 80.0 * 100.0
    }

    #[test]
    fn g5_uniaxial_bar_elongations_follow_the_material_axes() {
        // 10 MPa pull, free lateral contraction: 100 mm / 2500 * 10 = 0.4 mm along axis 1
        let c = card();
        let id = OrientationCard::identity();
        let o = bar(&c, &id, 10.0);
        assert!((elongation(&o) - 0.4).abs() / 0.4 < 5e-3, "{}", elongation(&o));
        // axis 3 (stacking) lying along the bar: 100 / 2000 * 10 = 0.5 mm
        let rot = OrientationCard { name: "OR2".into(), axes: [[0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]], origin: [0.0; 3], rot_deg: [0.0; 3] };
        let o3 = bar(&c, &rot, 10.0);
        assert!((elongation(&o3) - 0.5).abs() / 0.5 < 5e-3, "{}", elongation(&o3));
        // axis 2 along the bar behaves like axis 1
        let rot2 = OrientationCard { name: "OR3".into(), axes: [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], origin: [0.0; 3], rot_deg: [0.0; 3] };
        let o2 = bar(&c, &rot2, 10.0);
        assert!((elongation(&o2) - 0.4).abs() / 0.4 < 5e-3, "{}", elongation(&o2));
        // mid-bar stress is the applied 10 MPa, with little lateral stress
        let mid = o.elem_stress.len() / 2;
        assert!((o.elem_stress[mid][0] - 10.0).abs() < 0.05 && o.elem_stress[mid][1].abs() < 0.05, "{:?}", o.elem_stress[mid]);
    }

    #[test]
    fn g9_reactions_balance_the_applied_load() {
        let o = bar(&card(), &OrientationCard::identity(), 10.0);
        let r = o.reactions["MOUNT1"];
        // end area 100 mm2 at 10 MPa: 1000 N pulling along +X, the support pulls back
        assert!((r[0] + 1000.0).abs() < 1e-3 * 1000.0, "{r:?}");
        assert!(r[1].abs() < 1.0 && r[2].abs() < 1.0);
        assert!((o.volume - 10000.0).abs() < 1e-6);
    }

    #[test]
    fn the_dat_and_frd_writers_round_trip_through_the_readers() {
        let o = bar(&card(), &OrientationCard::identity(), 10.0);
        let mesh = tet4_to_tet10(&box_tets(DVec3::new(100.0, 10.0, 10.0), [10, 1, 1])).unwrap();
        let dat = parse_dat(&dat_text(&o));
        assert_eq!(dat.ip_stress.len(), 4 * o.elem_stress.len());
        assert!((dat.ip_stress[0].2[0] - o.ip_stress[0][0][0]).abs() < 1e-4);
        assert!((dat.totals["MOUNT1"][0] - o.reactions["MOUNT1"][0]).abs() < 1e-2);
        assert!((dat.volume.unwrap() - 10000.0).abs() < 1e-2);
        let frd = parse_frd(&write_frd(&mesh.points, Some(&o.disp), Some(&o.node_stress))).unwrap();
        let n = mesh.points.len();
        let u = dense_vec3(frd.dataset("DISP").unwrap(), n, &vec![true; n]).unwrap();
        let s = dense_tensor6(frd.dataset("STRESS").unwrap(), n, &vec![true; n]).unwrap();
        let tip = o.disp.iter().map(|d| d.x).fold(f64::NEG_INFINITY, f64::max);
        assert!((u.iter().map(|d| d.x).fold(f64::NEG_INFINITY, f64::max) - tip).abs() < 1e-4);
        assert!((s[0][0] - o.node_stress[0][0]).abs() < 1e-3 * o.node_stress[0][0].abs().max(1.0));
    }

    #[test]
    fn g3_cantilever_is_stiffer_than_beam_theory_with_four_node_tetrahedra() {
        // L 100, b = h = 10, E 2500, tip load 10 N: beam theory 1.6000 mm (1.613 with shear).
        // Four-node tetrahedra lock in bending, so the solver must come out stiffer
        // (smaller deflection) but not absurdly so.
        let mesh = box_tets(DVec3::new(100.0, 10.0, 10.0), [40, 4, 4]);
        let left: Vec<u32> = (0..mesh.points.len() as u32).filter(|&i| (mesh.points[i as usize].x + 50.0).abs() < 1e-9).collect();
        let tip: Vec<u32> = (0..mesh.points.len() as u32).filter(|&i| (mesh.points[i as usize].x - 50.0).abs() < 1e-9).collect();
        let forces = tip.iter().map(|&n| crate::types::NodalForce { node: n, f_n: [0.0, 0.0, -10.0 / tip.len() as f64] }).collect();
        let site = MountSite { name: "m".into(), center: [-50.0, 0.0, 0.0], axis: [-1.0, 0.0, 0.0], nodes: left, model: "fixed_patch".into(), r_w_mm: 5.0, avg_radius_mm: 5.0, bolt_d_mm: 5.0, pad_e_mpa: 5.0, pad_t_mm: 2.0 };
        let c = MaterialCard::trans_iso("iso", 2500.0, 2500.0, 0.35, 0.35, 2500.0 / 2.7);
        let o = solve(&RefInput { mesh: &mesh, card: &c, orient: &OrientationCard::identity(), mounts: &MountSet { sites: vec![site] }, load: &LoadSet { forces, ..Default::default() }, max_dof: 60000, tol: 1e-10 }).unwrap();
        let defl = tip.iter().map(|&n| -o.disp[n as usize].z).sum::<f64>() / tip.len() as f64;
        assert!(defl > 0.3 && defl < 1.65, "{defl}");
        // the net reaction balances the 10 N tip load
        assert!((o.reactions["MOUNT1"][2] - 10.0).abs() < 0.05, "{:?}", o.reactions);
    }

    #[test]
    fn an_unconstrained_body_is_refused() {
        let mesh = tet4_to_tet10(&box_tets(DVec3::splat(10.0), [1, 1, 1])).unwrap();
        let load = LoadSet::default();
        let mounts = MountSet::default();
        let c = card();
        let id = OrientationCard::identity();
        assert!(solve(&RefInput { mesh: &mesh, card: &c, orient: &id, mounts: &mounts, load: &load, max_dof: 60000, tol: 1e-10 }).is_err());
    }
}

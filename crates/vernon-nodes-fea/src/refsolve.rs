//! White-box reference solver: linear static analysis with four-node
//! tetrahedra and a Jacobi-preconditioned conjugate-gradient solve.
//!
//! It exists so the chain runs end to end without the external solver and so
//! tests have an oracle. Four-node tetrahedra are stiff in bending: use the
//! external solver for numbers that gate a design. Quadratic meshes are
//! handled by using their corner nodes only; mid-edge nodes receive the
//! average of their edge ends.
//!
//! Stress components are `[xx, yy, zz, xy, yz, zx]`, strain is engineering.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use glam::{DMat3, DVec3};
use vernon_core::Geometry;

use crate::frd::format_frd_value;
use crate::linalg::{rotate_stiffness_to_global, von_mises, M6};
use crate::material::{MaterialCard, ROOM_C};
use crate::mesh::{corners, element_size, element_volume, EDGES10, FACES};
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
    /// Constant stress of every element (MPa).
    pub elem_stress: Vec<[f64; 6]>,
    /// Volume-averaged stress at every mesh point (MPa).
    pub node_stress: Vec<[f64; 6]>,
    /// Support reaction totals per mount, keyed `MOUNT<k>` (N).
    pub reactions: BTreeMap<String, [f64; 3]>,
    pub volume: f64,
    pub iterations: usize,
    pub residual: f64,
}

impl RefOutput {
    pub fn max_disp(&self) -> f64 {
        self.disp.iter().map(|d| d.length()).fold(0.0, f64::max)
    }

    pub fn max_vm(&self) -> f64 {
        self.node_stress.iter().map(von_mises).fold(0.0, f64::max)
    }
}

struct Elem {
    /// Compact node numbers of the four corners.
    n: [usize; 4],
    ke: Vec<f64>,
    /// Stress of unit nodal displacements, `D B`.
    sb: [[f64; 12]; 6],
    vol: f64,
}

fn b_matrix(g: [DVec3; 4]) -> Option<([[f64; 12]; 6], f64)> {
    let j = DMat3::from_cols(g[1] - g[0], g[2] - g[0], g[3] - g[0]);
    let det = j.determinant();
    if det <= 1e-300 {
        return None;
    }
    let inv = j.inverse();
    // gradients of the shape functions 1..3 are the rows of the inverse Jacobian
    let mut grad = [DVec3::ZERO; 4];
    for (k, gr) in grad.iter_mut().enumerate().skip(1) {
        *gr = DVec3::new(inv.col(0)[k - 1], inv.col(1)[k - 1], inv.col(2)[k - 1]);
    }
    grad[0] = -(grad[1] + grad[2] + grad[3]);
    let mut b = [[0.0; 12]; 6];
    for (a, gr) in grad.iter().enumerate() {
        let c = 3 * a;
        b[0][c] = gr.x;
        b[1][c + 1] = gr.y;
        b[2][c + 2] = gr.z;
        b[3][c] = gr.y;
        b[3][c + 1] = gr.x;
        b[4][c + 1] = gr.z;
        b[4][c + 2] = gr.y;
        b[5][c] = gr.z;
        b[5][c + 2] = gr.x;
    }
    Some((b, det / 6.0))
}

fn matvec(elems: &[Elem], springs: &[f64], x: &[f64], y: &mut [f64]) {
    y.iter_mut().for_each(|v| *v = 0.0);
    for e in elems {
        let mut xe = [0.0; 12];
        for (a, &n) in e.n.iter().enumerate() {
            xe[3 * a..3 * a + 3].copy_from_slice(&x[3 * n..3 * n + 3]);
        }
        for i in 0..12 {
            let row = &e.ke[12 * i..12 * i + 12];
            let s: f64 = row.iter().zip(xe.iter()).map(|(a, b)| a * b).sum();
            y[3 * e.n[i / 3] + i % 3] += s;
        }
    }
    for (i, k) in springs.iter().enumerate() {
        y[i] += k * x[i];
    }
}

/// Solve one load case.
pub fn solve(inp: &RefInput) -> Result<RefOutput, String> {
    let mesh = inp.mesh;
    let np = element_size(mesh)?;
    // corner nodes only
    let mut compact: Vec<Option<usize>> = vec![None; mesh.points.len()];
    let mut used: Vec<u32> = Vec::new();
    for e in 0..mesh.prims.len() {
        for c in corners(mesh, e) {
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
    let mut elems = Vec::with_capacity(mesh.prims.len());
    for e in 0..mesh.prims.len() {
        let c = corners(mesh, e);
        let pts = c.map(|i| mesh.points[i as usize]);
        let (b, vol) = b_matrix(pts).ok_or_else(|| format!("element {} has a non-positive volume", e + 1))?;
        let t = temps.map_or(ROOM_C, |t| c.iter().map(|&i| t[i as usize]).sum::<f64>() / 4.0);
        let r = inp.card.retention_at(t);
        let mut sb = [[0.0; 12]; 6];
        for i in 0..6 {
            for j in 0..12 {
                sb[i][j] = r * (0..6).map(|k| d_glob[i][k] * b[k][j]).sum::<f64>();
            }
        }
        let mut ke = vec![0.0; 144];
        for i in 0..12 {
            for j in 0..12 {
                ke[12 * i + j] = vol * (0..6).map(|k| b[k][i] * sb[k][j]).sum::<f64>();
            }
        }
        let n = c.map(|i| compact[i as usize].unwrap_or(0));
        elems.push(Elem { n, ke, sb, vol });
    }

    // constraints and springs
    let mut fixed = vec![false; ndof];
    let mut springs = vec![0.0; ndof];
    let mut mount_dofs: Vec<Vec<usize>> = Vec::new();
    // a constrained mid-edge node constrains both ends of its edge (the corners are the only unknowns here)
    let mut mid_ends: std::collections::HashMap<u32, [u32; 2]> = std::collections::HashMap::new();
    if np == 10 {
        for (e, prim) in mesh.prims.iter().enumerate() {
            let c = corners(mesh, e);
            for (k, (a, b)) in EDGES10.iter().enumerate() {
                mid_ends.insert(prim.vertices[4 + k], [c[*a], c[*b]]);
            }
        }
    }
    for site in &inp.mounts.sites {
        let mut ids: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        for &n in &site.nodes {
            match mid_ends.get(&n) {
                Some(ends) => ids.extend(ends.iter().filter_map(|&c| compact[c as usize])),
                None => ids.extend(compact.get(n as usize).copied().flatten()),
            }
        }
        let nodes: Vec<usize> = ids.into_iter().collect();
        if nodes.is_empty() {
            return Err(format!("mount '{}' has no corner nodes in the mesh", site.name));
        }
        let mut dofs = Vec::new();
        if site.model == "spring_pad" {
            let area = std::f64::consts::PI * site.r_w_mm * site.r_w_mm;
            let k_node = site.pad_e_mpa * area / site.pad_t_mm.max(1e-6) / nodes.len() as f64;
            for &n in &nodes {
                for d in 0..3 {
                    springs[3 * n + d] += k_node;
                    dofs.push(3 * n + d);
                }
            }
        } else {
            for &n in &nodes {
                for d in 0..3 {
                    fixed[3 * n + d] = true;
                    dofs.push(3 * n + d);
                }
            }
        }
        mount_dofs.push(dofs);
    }

    // loads on the corner nodes
    let mut f = vec![0.0; ndof];
    for fp in &inp.load.pressures {
        let e = fp.elem as usize;
        let c = corners(mesh, e);
        let (area, n) = crate::loads::face_geometry(mesh, e, fp.face);
        let force = -n * (fp.p_mpa * area / 3.0);
        for &li in &FACES[fp.face as usize - 1] {
            let k = compact[c[li] as usize].unwrap_or(0);
            f[3 * k] += force.x;
            f[3 * k + 1] += force.y;
            f[3 * k + 2] += force.z;
        }
    }
    if let Some(gr) = inp.load.gravity {
        let a = DVec3::from(gr.dir) * gr.accel_mm_s2;
        for el in &elems {
            let fe = a * (inp.card.density_t_mm3 * el.vol / 4.0);
            for &k in &el.n {
                f[3 * k] += fe.x;
                f[3 * k + 1] += fe.y;
                f[3 * k + 2] += fe.z;
            }
        }
    }
    for nf in &inp.load.forces {
        let p = mesh.points.get(nf.node as usize).ok_or("a nodal force refers to a node outside the mesh")?;
        let k = (0..used.len())
            .min_by(|&a, &b| (mesh.points[used[a] as usize] - *p).length_squared().total_cmp(&(mesh.points[used[b] as usize] - *p).length_squared()))
            .ok_or("empty mesh")?;
        for d in 0..3 {
            f[3 * k + d] += nf.f_n[d];
        }
    }

    // conjugate gradient on the free dofs
    let free: Vec<bool> = fixed.iter().map(|x| !x).collect();
    let mut diag = vec![0.0; ndof];
    for e in &elems {
        for i in 0..12 {
            diag[3 * e.n[i / 3] + i % 3] += e.ke[13 * i];
        }
    }
    for (d, s) in diag.iter_mut().zip(springs.iter()) {
        *d += s;
    }
    let inv_diag: Vec<f64> = diag.iter().zip(free.iter()).map(|(d, fr)| if *fr && *d > 0.0 { 1.0 / d } else { 0.0 }).collect();
    let mask = |v: &mut [f64]| v.iter_mut().zip(free.iter()).for_each(|(x, fr)| *x = if *fr { *x } else { 0.0 });
    let mut b = f.clone();
    mask(&mut b);
    let bnorm = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    let mut u = vec![0.0; ndof];
    let mut iterations = 0;
    let mut residual = 0.0;
    if bnorm > 0.0 {
        let mut r = b.clone();
        let mut z: Vec<f64> = r.iter().zip(inv_diag.iter()).map(|(a, d)| a * d).collect();
        let mut p = z.clone();
        let mut rz: f64 = r.iter().zip(z.iter()).map(|(a, b)| a * b).sum();
        let mut ap = vec![0.0; ndof];
        let max_it = (10 * ndof).clamp(200, 20000);
        while iterations < max_it {
            matvec(&elems, &springs, &p, &mut ap);
            mask(&mut ap);
            let pap: f64 = p.iter().zip(ap.iter()).map(|(a, b)| a * b).sum();
            if pap <= 0.0 {
                return Err("the stiffness matrix is not positive definite: the body is under-constrained".into());
            }
            let alpha = rz / pap;
            for i in 0..ndof {
                u[i] += alpha * p[i];
                r[i] -= alpha * ap[i];
            }
            iterations += 1;
            residual = r.iter().map(|x| x * x).sum::<f64>().sqrt() / bnorm;
            if residual < inp.tol {
                break;
            }
            for i in 0..ndof {
                z[i] = r[i] * inv_diag[i];
            }
            let rz_new: f64 = r.iter().zip(z.iter()).map(|(a, b)| a * b).sum();
            let beta = rz_new / rz;
            rz = rz_new;
            for i in 0..ndof {
                p[i] = z[i] + beta * p[i];
            }
        }
        if residual >= inp.tol.max(1e-6) {
            return Err(format!("the solver did not converge (relative residual {residual:.2e}): the body may be under-constrained"));
        }
    }

    // reactions: support force on the body = K u - f at the constrained dofs (springs: -k u)
    let mut ku = vec![0.0; ndof];
    matvec(&elems, &springs, &u, &mut ku);
    let mut reactions = BTreeMap::new();
    for (k, dofs) in mount_dofs.iter().enumerate() {
        let mut r = [0.0; 3];
        for &d in dofs {
            if fixed[d] {
                r[d % 3] += ku[d] - f[d];
            } else {
                r[d % 3] += -springs[d] * u[d];
            }
        }
        reactions.insert(format!("MOUNT{}", k + 1), r);
    }

    // element and nodal results
    let mut elem_stress = Vec::with_capacity(elems.len());
    let mut acc = vec![([0.0f64; 6], 0.0f64); mesh.points.len()];
    for (ei, el) in elems.iter().enumerate() {
        let mut ue = [0.0; 12];
        for (a, &n) in el.n.iter().enumerate() {
            ue[3 * a..3 * a + 3].copy_from_slice(&u[3 * n..3 * n + 3]);
        }
        let mut s = [0.0; 6];
        for (i, v) in s.iter_mut().enumerate() {
            *v = el.sb[i].iter().zip(ue.iter()).map(|(a, b)| a * b).sum();
        }
        for &c in &corners(mesh, ei) {
            let a = &mut acc[c as usize];
            for (v, x) in a.0.iter_mut().zip(s.iter()) {
                *v += x * el.vol;
            }
            a.1 += el.vol;
        }
        elem_stress.push(s);
    }
    let mut disp = vec![DVec3::ZERO; mesh.points.len()];
    for (k, &c) in used.iter().enumerate() {
        disp[c as usize] = DVec3::new(u[3 * k], u[3 * k + 1], u[3 * k + 2]);
    }
    let mut node_stress = vec![[0.0; 6]; mesh.points.len()];
    for (i, (s, w)) in acc.iter().enumerate() {
        if *w > 0.0 {
            node_stress[i] = s.map(|v| v / w);
        }
    }
    if np == 10 {
        for (e, prim) in mesh.prims.iter().enumerate() {
            let c = corners(mesh, e);
            for (k, (a, b)) in EDGES10.iter().enumerate() {
                let mid = prim.vertices[4 + k] as usize;
                disp[mid] = 0.5 * (disp[c[*a] as usize] + disp[c[*b] as usize]);
                let (sa, sb) = (node_stress[c[*a] as usize], node_stress[c[*b] as usize]);
                node_stress[mid] = std::array::from_fn(|i| 0.5 * (sa[i] + sb[i]));
            }
        }
    }
    let volume = (0..mesh.prims.len()).map(|e| element_volume(mesh, e)).sum();
    Ok(RefOutput { disp, elem_stress, node_stress, reactions, volume, iterations, residual })
}

/// The `.dat` text in the layout the solver writes, so the same reader serves both.
pub fn dat_text(out: &RefOutput) -> String {
    let mut s = String::new();
    for (name, r) in &out.reactions {
        let _ = writeln!(s, " total force (fx,fy,fz) for set {name} and time  0.1000000E+01\n");
        let _ = writeln!(s, "  {}  {}  {}\n", format_frd_value(r[0]), format_frd_value(r[1]), format_frd_value(r[2]));
    }
    let _ = writeln!(s, " stresses (elem, integ.pnt.,sxx,syy,szz,sxy,sxz,syz) for set EALL and time  0.1000000E+01\n");
    for (e, t) in out.elem_stress.iter().enumerate() {
        let _ = writeln!(s, "{:>10}{:>5}  {}  {}  {}  {}  {}  {}", e + 1, 1, format_frd_value(t[0]), format_frd_value(t[1]), format_frd_value(t[2]), format_frd_value(t[3]), format_frd_value(t[5]), format_frd_value(t[4]));
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
        assert_eq!(dat.ip_stress.len(), o.elem_stress.len());
        assert!((dat.ip_stress[0].2[0] - o.elem_stress[0][0]).abs() < 1e-4);
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
        // Four-node tetrahedra lock in bending, so the reference solver must come out stiffer
        // (smaller deflection) but not absurdly so; this is why its report never passes a gate.
        let mesh = tet4_to_tet10(&box_tets(DVec3::new(100.0, 10.0, 10.0), [40, 4, 4])).unwrap();
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

//! Load calculator: dynamic pressure, net aero force, pressure fields mapped
//! onto the solid mesh, consistent nodal forces, bump acceleration.
//!
//! Sign convention of the solver (and of every pressure here): a positive
//! pressure pushes *into* the body, a negative one (suction) pulls out.

use std::collections::BTreeMap;

use glam::DVec3;
use vernon_core::Geometry;

use crate::mesh::{boundary_faces, corners, element_volume, BFace, EDGES10, FACES};
use crate::spatial::TriGrid;
use crate::types::{FacePressure, NodalForce};

/// Standard gravity in the deck's unit system (mm/s2).
pub const G_MM_S2: f64 = 9810.0;

/// q = 1/2 rho v^2 in Pa for a density in kg/m3 and a speed in km/h.
pub fn dynamic_pressure_pa(rho_kg_m3: f64, v_kmh: f64) -> f64 {
    let v = v_kmh / 3.6;
    0.5 * rho_kg_m3 * v * v
}

/// Pa to MPa (= N/mm2).
pub fn pa_to_mpa(p_pa: f64) -> f64 {
    p_pa * 1e-6
}

/// Area (mm2) and outward unit normal of face `face` (1..=4) of element `elem`.
pub fn face_geometry(g: &Geometry, elem: usize, face: u8) -> (f64, DVec3) {
    let c = corners(g, elem);
    let f = FACES[face as usize - 1];
    let p = f.map(|i| g.points[c[i] as usize]);
    let cr = (p[1] - p[0]).cross(p[2] - p[0]);
    let opp = (0..4).find(|i| !f.contains(i)).unwrap_or(3);
    let centroid = (p[0] + p[1] + p[2]) / 3.0;
    let mut n = cr.normalize_or_zero();
    if n.dot(g.points[c[opp] as usize] - centroid) > 0.0 {
        n = -n;
    }
    (0.5 * cr.length(), n)
}

/// Resultant force (N) of a set of face pressures: sum of p A (-n).
pub fn pressure_resultant(g: &Geometry, ps: &[FacePressure]) -> DVec3 {
    ps.iter()
        .map(|fp| {
            let (a, n) = face_geometry(g, fp.elem as usize, fp.face);
            -n * (fp.p_mpa * a)
        })
        .sum()
}

/// Consistent nodal forces of one uniform face pressure. A quadratic
/// tetrahedron gets nothing on the corners and p A / 3 on each of the three
/// mid-edge nodes; a linear one gets p A / 3 on each corner.
pub fn face_nodal_forces(g: &Geometry, fp: &FacePressure) -> Vec<(u32, DVec3)> {
    let (a, n) = face_geometry(g, fp.elem as usize, fp.face);
    let f = -n * (fp.p_mpa * a / 3.0);
    let v = &g.prims[fp.elem as usize].vertices;
    let loc = FACES[fp.face as usize - 1];
    if v.len() == 10 {
        EDGES10
            .iter()
            .enumerate()
            .filter(|(_, (x, y))| loc.contains(x) && loc.contains(y))
            .map(|(k, _)| (v[4 + k], f))
            .collect()
    } else {
        loc.iter().map(|&i| (v[i], f)).collect()
    }
}

/// Sum the consistent nodal forces of many face pressures per node.
pub fn pressure_nodal_forces(g: &Geometry, ps: &[FacePressure]) -> Vec<NodalForce> {
    let mut acc: BTreeMap<u32, DVec3> = BTreeMap::new();
    for fp in ps {
        for (n, f) in face_nodal_forces(g, fp) {
            *acc.entry(n).or_insert(DVec3::ZERO) += f;
        }
    }
    acc.into_iter().map(|(node, f)| NodalForce { node, f_n: f.into() }).collect()
}

/// Replace the pressures by at most `bins` classes: equal-width bins over the
/// pressure range, each face getting the area-weighted mean of its bin, so
/// the integral of p over every bin is unchanged.
pub fn bin_pressures(g: &Geometry, ps: &mut [FacePressure], bins: usize) {
    if bins == 0 || ps.is_empty() {
        return;
    }
    let lo = ps.iter().map(|p| p.p_mpa).fold(f64::INFINITY, f64::min);
    let hi = ps.iter().map(|p| p.p_mpa).fold(f64::NEG_INFINITY, f64::max);
    if hi - lo < 1e-15 {
        return;
    }
    let class = |p: f64| (((p - lo) / (hi - lo) * bins as f64) as usize).min(bins - 1);
    let mut sums = vec![(0.0, 0.0); bins];
    for fp in ps.iter() {
        let a = face_geometry(g, fp.elem as usize, fp.face).0;
        let s = &mut sums[class(fp.p_mpa)];
        s.0 += fp.p_mpa * a;
        s.1 += a;
    }
    for fp in ps.iter_mut() {
        let s = sums[class(fp.p_mpa)];
        if s.1 > 0.0 {
            fp.p_mpa = s.0 / s.1;
        }
    }
}

/// Uniform pressure on the faces that look into `dir` so that the force
/// along `dir` equals `force_n` (a negative force flips the direction).
pub struct NetForce {
    pub pressures: Vec<FacePressure>,
    pub p_mpa: f64,
    pub projected_area_mm2: f64,
}

pub fn net_force_pressures(g: &Geometry, dir: DVec3, force_n: f64, min_cos: f64) -> Result<NetForce, String> {
    let mut d = dir.normalize_or_zero();
    if d == DVec3::ZERO {
        return Err("the load direction is a zero vector".into());
    }
    let mut f = force_n;
    if f < 0.0 {
        d = -d;
        f = -f;
    }
    let faces = boundary_faces(g);
    let sel: Vec<(&BFace, f64)> = faces.iter().map(|b| (b, -b.normal.dot(d))).filter(|(_, c)| *c > min_cos).collect();
    let proj: f64 = sel.iter().map(|(b, c)| b.area * c).sum();
    if proj <= 0.0 {
        return Err("no boundary face looks into the load direction".into());
    }
    let p = f / proj;
    let pressures = sel.iter().map(|(b, _)| FacePressure { elem: b.elem as u32, face: b.face, p_mpa: p }).collect();
    Ok(NetForce { pressures, p_mpa: p, projected_area_mm2: proj })
}

/// Pressure field mapped from a surface carrying a point attribute `cp`.
pub struct FieldLoad {
    pub pressures: Vec<FacePressure>,
    pub resultant_fe: DVec3,
    pub resultant_surface: DVec3,
    pub loaded_faces: usize,
    pub loaded_area_mm2: f64,
}

/// Map `cp` from `surf` onto the boundary faces of the solid within
/// `snap_mm` (nearest point, barycentric interpolation) and compare the
/// integrals over the footprint (conservation gate).
pub fn field_pressures(g: &Geometry, surf: &Geometry, q_pa: f64, cp_in: f64, snap_mm: f64) -> Result<FieldLoad, String> {
    let cp = surf.point_attribs.floats("cp").map_err(|_| "the pressure surface has no point attribute 'cp'".to_string())?;
    let tri_ids: Vec<[u32; 3]> = surf.triangles().into_iter().map(|(_, t)| t).collect();
    if tri_ids.is_empty() {
        return Err("the pressure surface has no triangles".into());
    }
    let sgrid = TriGrid::new(tri_ids.iter().map(|t| t.map(|i| surf.points[i as usize])).collect());
    let faces = boundary_faces(g);
    let mut pressures = Vec::new();
    let mut fe = DVec3::ZERO;
    let mut area = 0.0;
    for b in &faces {
        let Some(n) = sgrid.nearest(b.centroid, snap_mm) else { continue };
        let t = tri_ids[n.tri];
        let c = n.bary[0] * cp[t[0] as usize] + n.bary[1] * cp[t[1] as usize] + n.bary[2] * cp[t[2] as usize];
        let p_mpa = crate::loads::pa_to_mpa((c - cp_in) * q_pa);
        fe += -b.normal * (p_mpa * b.area);
        area += b.area;
        pressures.push(FacePressure { elem: b.elem as u32, face: b.face, p_mpa });
    }
    // Integral over the same footprint on the pressure surface itself, with
    // its triangle normals turned to agree with the solid's outward normals.
    let fgrid = TriGrid::new(
        faces
            .iter()
            .map(|b| b.nodes.map(|i| g.points[i as usize]))
            .collect(),
    );
    let mut sf = DVec3::ZERO;
    for t in &tri_ids {
        let p = t.map(|i| surf.points[i as usize]);
        let cr = (p[1] - p[0]).cross(p[2] - p[0]);
        let centroid = (p[0] + p[1] + p[2]) / 3.0;
        let Some(n) = fgrid.nearest(centroid, snap_mm) else { continue };
        let mut nrm = cr.normalize_or_zero();
        if nrm.dot(faces[n.tri].normal) < 0.0 {
            nrm = -nrm;
        }
        let cmean = (cp[t[0] as usize] + cp[t[1] as usize] + cp[t[2] as usize]) / 3.0;
        sf += -nrm * (pa_to_mpa((cmean - cp_in) * q_pa) * 0.5 * cr.length());
    }
    if pressures.is_empty() {
        return Err(format!("no boundary face lies within {snap_mm} mm of the pressure surface"));
    }
    let loaded_faces = pressures.len();
    Ok(FieldLoad { pressures, resultant_fe: fe, resultant_surface: sf, loaded_faces, loaded_area_mm2: area })
}

/// Mass (tonne) of a solid mesh at density `rho` (tonne/mm3).
pub fn mesh_mass_tonne(g: &Geometry, rho_t_mm3: f64) -> f64 {
    (0..g.prims.len()).map(|e| element_volume(g, e).abs()).sum::<f64>() * rho_t_mm3
}

/// Index of the mesh node nearest to `p`.
pub fn nearest_node(g: &Geometry, p: DVec3) -> Option<u32> {
    g.points
        .iter()
        .enumerate()
        .min_by(|a, b| (*a.1 - p).length_squared().total_cmp(&(*b.1 - p).length_squared()))
        .map(|(i, _)| i as u32)
}

/// Acceleration field (mm/s2) of a bump of `bump_g` along `axis`, plus 1 g
/// downward (-Z) when `include_gravity`.
pub fn accel_field(bump_g: f64, axis: DVec3, include_gravity: bool) -> DVec3 {
    let mut a = axis.normalize_or_zero() * (bump_g * G_MM_S2);
    if include_gravity {
        a += DVec3::new(0.0, 0.0, -G_MM_S2);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{box_tets, solid_from_tets, tet4_to_tet10};

    fn unit_tet10() -> Geometry {
        let pts = vec![DVec3::ZERO, DVec3::new(10.0, 0.0, 0.0), DVec3::new(0.0, 10.0, 0.0), DVec3::new(0.0, 0.0, 10.0)];
        tet4_to_tet10(&solid_from_tets(pts, &[[0, 1, 2, 3]])).unwrap()
    }

    #[test]
    fn g1_aero_numbers() {
        let q200 = dynamic_pressure_pa(1.11, 200.0);
        assert!((q200 - 1712.96).abs() < 0.01, "{q200}");
        assert!((pa_to_mpa(q200) - 1.71296e-3).abs() < 1e-8);
        assert!((q200 * 0.5 - 856.5).abs() < 0.1);
        let q250 = dynamic_pressure_pa(1.11, 250.0);
        assert!((q250 - 2676.50).abs() < 0.01);
        assert!((q250 * 0.5 - 1338.3).abs() < 0.1);
        assert!((q250 / q200 - 1.5625).abs() < 1e-12);
    }

    #[test]
    fn g4_face_pressure_nodal_forces() {
        let g = unit_tet10();
        let fp = FacePressure { elem: 0, face: 1, p_mpa: 0.01 };
        let f = face_nodal_forces(&g, &fp);
        assert_eq!(f.len(), 3);
        let total: DVec3 = f.iter().map(|x| x.1).sum();
        // face 1 (z = 0) has outward normal -z, so the pressure pushes +z
        assert!((total - DVec3::new(0.0, 0.0, 0.5)).length() < 1e-12);
        for (n, v) in &f {
            assert!(*n >= 4, "no force on corner nodes");
            assert!((v.length() - 0.5 / 3.0).abs() < 1e-12, "{v:?}");
        }
        let r = pressure_resultant(&g, &[fp]);
        assert!((r - total).length() < 1e-12);
        let all = pressure_nodal_forces(&g, &[fp]);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn net_force_hits_the_requested_resultant() {
        let g = box_tets(DVec3::new(40.0, 20.0, 6.0), [8, 4, 2]);
        let nf = net_force_pressures(&g, DVec3::NEG_Z, 856.5, 0.05).unwrap();
        assert!((nf.projected_area_mm2 - 800.0).abs() < 1e-6);
        let r = pressure_resultant(&g, &nf.pressures);
        assert!((r.z + 856.5).abs() < 1e-6 && r.x.abs() < 1e-6 && r.y.abs() < 1e-6, "{r:?}");
        // a negative force pushes the other way
        let up = net_force_pressures(&g, DVec3::NEG_Z, -100.0, 0.05).unwrap();
        assert!((pressure_resultant(&g, &up.pressures).z - 100.0).abs() < 1e-6);
        assert!(net_force_pressures(&g, DVec3::ZERO, 1.0, 0.05).is_err());
    }

    #[test]
    fn binning_preserves_the_integral() {
        let g = box_tets(DVec3::new(40.0, 20.0, 6.0), [8, 4, 2]);
        let mut ps: Vec<FacePressure> = boundary_faces(&g)
            .iter()
            .map(|b| FacePressure { elem: b.elem as u32, face: b.face, p_mpa: 1e-3 * (1.0 + b.centroid.x / 20.0) })
            .collect();
        bin_pressures(&g, &mut ps, 8);
        let mut distinct: Vec<f64> = ps.iter().map(|p| (p.p_mpa * 1e12).round()).collect();
        distinct.sort_by(|a, b| a.total_cmp(b));
        distinct.dedup();
        assert!(distinct.len() <= 8);
        // the box is closed, so the resultant is ~0 either way; compare the unsigned integral
        let abs_int = |ps: &[FacePressure]| ps.iter().map(|p| p.p_mpa * face_geometry(&g, p.elem as usize, p.face).0).sum::<f64>();
        let mut orig = ps.clone();
        for (o, b) in orig.iter_mut().zip(boundary_faces(&g)) {
            o.p_mpa = 1e-3 * (1.0 + b.centroid.x / 20.0);
        }
        assert!((abs_int(&orig) - abs_int(&ps)).abs() < 1e-9);
    }

    #[test]
    fn field_mapping_conserves_a_uniform_cp() {
        let g = box_tets(DVec3::new(40.0, 20.0, 6.0), [8, 4, 2]);
        let mut surf = Geometry::make_box(DVec3::new(40.0, 20.0, 6.0));
        let n = surf.points.len();
        surf.point_attribs.set("cp", vernon_core::AttribArray::Float(vec![-0.8; n]));
        let q = dynamic_pressure_pa(1.11, 200.0);
        let f = field_pressures(&g, &surf, q, 0.0, 2.0).unwrap();
        // suction everywhere: a closed box under uniform suction has no net force
        assert!(f.pressures.iter().all(|p| p.p_mpa < 0.0));
        assert!((f.pressures[0].p_mpa + pa_to_mpa(0.8 * q)).abs() < 1e-12);
        assert!(f.resultant_fe.length() < 1e-6 && f.resultant_surface.length() < 1e-6);
        assert!((f.loaded_area_mm2 - 2.0 * (800.0 + 240.0 + 120.0)).abs() < 1e-6);
    }

    #[test]
    fn field_mapping_matches_the_surface_resultant_for_a_top_face_load() {
        let g = box_tets(DVec3::new(40.0, 20.0, 6.0), [8, 4, 2]);
        // a flat plate just above the top face, facing up, with Cp = 1 (stagnation)
        let pts = vec![DVec3::new(-20.0, -10.0, 3.0), DVec3::new(20.0, -10.0, 3.0), DVec3::new(20.0, 10.0, 3.0), DVec3::new(-20.0, 10.0, 3.0)];
        let mut surf = Geometry::from_polygons(pts, vec![vec![0, 1, 2, 3]]);
        surf.point_attribs.set("cp", vernon_core::AttribArray::Float(vec![1.0; 4]));
        let q = dynamic_pressure_pa(1.11, 200.0);
        let f = field_pressures(&g, &surf, q, 0.0, 0.5).unwrap();
        assert!((f.loaded_area_mm2 - 800.0).abs() < 1e-6, "{}", f.loaded_area_mm2);
        let expect = pa_to_mpa(q) * 800.0;
        assert!((f.resultant_fe.z + expect).abs() < 1e-9, "{:?}", f.resultant_fe);
        assert!((f.resultant_surface.z + expect).abs() < 1e-9, "{:?}", f.resultant_surface);
        assert!(field_pressures(&g, &Geometry::make_box(DVec3::ONE), q, 0.0, 0.5).is_err());
    }

    #[test]
    fn weight_of_a_body_is_rho_v_g() {
        let g = box_tets(DVec3::new(40.0, 20.0, 10.0), [8, 4, 2]);
        let rho = 1.24e-9;
        let m = mesh_mass_tonne(&g, rho);
        assert!((m - 8000.0 * rho).abs() < 1e-18);
        let a = accel_field(0.0, DVec3::NEG_Z, true);
        assert!((m * a.length() - 8000.0 * rho * 9810.0).abs() < 1e-12);
        let bump = accel_field(4.0, DVec3::NEG_Z, true);
        assert!((bump.z + 5.0 * 9810.0).abs() < 1e-9);
        assert_eq!(nearest_node(&g, DVec3::new(-20.0, -10.0, -5.0)).map(|i| g.points[i as usize]), Some(DVec3::new(-20.0, -10.0, -5.0)));
    }
}

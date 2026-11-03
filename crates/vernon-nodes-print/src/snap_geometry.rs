//! Cantilever snap-fit design from standard beam theory, plus a watertight mesh.
//!
//! Formulas (classic snap-fit design guides, e.g. Bayer/BASF "Snap-Fit Design Manual";
//! exact edition UNVERIFIED, the equations themselves are textbook beam bending):
//! * maximum tip deflection of a constant-section cantilever:
//!   `y = 0.67 * eps * L^2 / h` (rectangular section, strain eps at the root);
//! * second moment `I = b h^3 / 12`, stiffness `k = 3 E I / L^3`, deflection force `P = k y`;
//! * inclined-plane force with friction mu and face angle `a` measured from the
//!   insertion direction: `W = P (mu + tan a) / (1 - mu tan a)`. The lead face gives the
//!   insertion force; the return face (angle `b`) gives the retention force. When
//!   `mu tan b >= 1` the joint is self-locking and no finite retention force exists.
//!
//! Units: mm, N, MPa (= N/mm^2). The arm lies along +X, deflects along Z and is extruded
//! along +Y by the width. The permissible strain is derated by `orientation_derate`
//! (layer lines across the arm weaken printed parts; 1.0 = no derate). The derate value is
//! the caller's responsibility (UNVERIFIED as a physical constant).

/// Design inputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapInput {
    pub length_mm: f64,
    pub thickness_mm: f64,
    pub width_mm: f64,
    /// Hook height = deflection needed to assemble.
    pub undercut_mm: f64,
    /// Permissible strain of the material (fraction, e.g. 0.02).
    pub permissible_strain: f64,
    pub modulus_mpa: f64,
    /// Multiplies the permissible strain, in (0, 1].
    pub orientation_derate: f64,
    /// Lead face angle from the insertion direction, degrees in (0, 90).
    pub lead_angle_deg: f64,
    /// Return face angle from the insertion direction, degrees in (0, 90].
    pub return_angle_deg: f64,
    pub friction: f64,
    /// Extra pocket depth below the arm beyond the undercut.
    pub clearance_mm: f64,
}

/// Numbers and mesh of a snap design.
#[derive(Debug, Clone)]
pub struct SnapDesign {
    pub vertices: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
    /// `0.67 * eps_allowed * L^2 / h`.
    pub max_deflection_mm: f64,
    /// Root strain at the required deflection `y`.
    pub strain: f64,
    /// Permissible strain after the derate.
    pub allowed_strain: f64,
    /// `1 - strain / allowed_strain`; negative when over-strained.
    pub strain_margin: f64,
    pub over_strain: bool,
    pub stiffness_n_per_mm: f64,
    /// Deflection force `P = k y` (N).
    pub assembly_force_n: f64,
    /// Force to push the lead face over the mating edge (N).
    pub insertion_force_n: f64,
    /// Force to pull the return face back out (N); `None` when self-locking.
    pub retention_force_n: Option<f64>,
}

type P2 = [f64; 2];

fn cross(o: P2, a: P2, b: P2) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Ear-clipping triangulation of a simple counter-clockwise polygon.
fn triangulate(poly: &[P2]) -> Result<Vec<[usize; 3]>, String> {
    let mut idx: Vec<usize> = (0..poly.len()).collect();
    let mut out = Vec::new();
    while idx.len() > 3 {
        let n = idx.len();
        let ear = (0..n).find(|&i| {
            let (a, b, c) = (idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]);
            if cross(poly[a], poly[b], poly[c]) <= 1e-12 {
                return false;
            }
            !idx.iter().any(|&p| {
                if p == a || p == b || p == c || poly[p] == poly[a] || poly[p] == poly[b] || poly[p] == poly[c] {
                    return false;
                }
                cross(poly[a], poly[b], poly[p]) >= -1e-12
                    && cross(poly[b], poly[c], poly[p]) >= -1e-12
                    && cross(poly[c], poly[a], poly[p]) >= -1e-12
            })
        });
        let i = ear.ok_or("polygon could not be triangulated")?;
        out.push([idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]]);
        idx.remove(i);
    }
    out.push([idx[0], idx[1], idx[2]]);
    Ok(out)
}

/// Cross-section in (x, z): wall + slab with a pocket under the arm, arm, and hook.
fn profile(i: &SnapInput, lead_run: f64, ret_run: f64) -> Vec<P2> {
    let (l, h, y) = (i.length_mm, i.thickness_mm, i.undercut_mm);
    let g = y + i.clearance_mm; // pocket depth
    let slab = 2.0; // material under the pocket
    let wall = h.max(2.0); // wall behind the arm root
    let big_h = g + slab;
    vec![
        [-wall, -big_h],
        [l, -big_h],
        [l, -g],
        [0.0, -g],
        [0.0, 0.0],
        [l, 0.0],
        [l, h],
        [l - lead_run, h + y],
        [l - lead_run - ret_run, h],
        [-wall, h],
    ]
}

fn finite_pos(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

/// Compute the design numbers and build the mesh.
pub fn design(i: &SnapInput) -> Result<SnapDesign, String> {
    for (name, v) in [
        ("length", i.length_mm),
        ("thickness", i.thickness_mm),
        ("width", i.width_mm),
        ("undercut", i.undercut_mm),
        ("permissible strain", i.permissible_strain),
        ("modulus", i.modulus_mpa),
    ] {
        if !finite_pos(v) {
            return Err(format!("{name} must be positive and finite"));
        }
    }
    if !(i.orientation_derate > 0.0 && i.orientation_derate <= 1.0) {
        return Err("orientation derate must be in (0, 1]".into());
    }
    if !(i.lead_angle_deg > 0.0 && i.lead_angle_deg < 90.0) {
        return Err("lead angle must be in (0, 90) degrees".into());
    }
    if !(i.return_angle_deg > 0.0 && i.return_angle_deg <= 90.0) {
        return Err("return angle must be in (0, 90] degrees".into());
    }
    if !(i.friction.is_finite() && i.friction >= 0.0) || !(i.clearance_mm.is_finite() && i.clearance_mm >= 0.0) {
        return Err("friction and clearance must be finite and non-negative".into());
    }
    let (l, h, b, y) = (i.length_mm, i.thickness_mm, i.width_mm, i.undercut_mm);
    let allowed = i.permissible_strain * i.orientation_derate;
    let max_defl = 0.67 * allowed * l * l / h;
    let strain = y * h / (0.67 * l * l);
    let inertia = b * h.powi(3) / 12.0;
    let k = 3.0 * i.modulus_mpa * inertia / l.powi(3);
    let p = k * y;
    let (ta, tb) = (i.lead_angle_deg.to_radians().tan(), i.return_angle_deg.to_radians().tan());
    let wedge = |t: f64| (i.friction + t) / (1.0 - i.friction * t);
    if 1.0 - i.friction * ta <= 0.0 {
        return Err("lead face is self-locking for this friction: no insertion is possible".into());
    }
    let retention = if 1.0 - i.friction * tb > 0.0 { Some(p * wedge(tb)) } else { None };

    let lead_run = y / ta;
    let ret_run = if i.return_angle_deg >= 90.0 { 0.0 } else { y / tb };
    if lead_run + ret_run > l {
        return Err("hook (lead + return runs) is longer than the arm".into());
    }
    let poly = profile(i, lead_run, ret_run);
    let tris2 = triangulate(&poly)?;
    let n = poly.len() as u32;
    let mut vertices: Vec<[f64; 3]> = poly.iter().map(|q| [q[0], 0.0, q[1]]).collect();
    vertices.extend(poly.iter().map(|q| [q[0], b, q[1]]));
    let mut triangles = Vec::new();
    for t in &tris2 {
        // Counter-clockwise in (x, z) has normal -Y: front cap as is, back cap reversed.
        triangles.push([t[0] as u32, t[1] as u32, t[2] as u32]);
        triangles.push([t[0] as u32 + n, t[2] as u32 + n, t[1] as u32 + n]);
    }
    for a in 0..n {
        let c = (a + 1) % n;
        triangles.push([a, a + n, c + n]);
        triangles.push([a, c + n, c]);
    }
    Ok(SnapDesign {
        vertices,
        triangles,
        max_deflection_mm: max_defl,
        strain,
        allowed_strain: allowed,
        strain_margin: 1.0 - strain / allowed,
        over_strain: strain > allowed,
        stiffness_n_per_mm: k,
        assembly_force_n: p,
        insertion_force_n: p * wedge(ta),
        retention_force_n: retention,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn input(y: f64) -> SnapInput {
        SnapInput {
            length_mm: 20.0,
            thickness_mm: 2.0,
            width_mm: 6.0,
            undercut_mm: y,
            permissible_strain: 0.02,
            modulus_mpa: 2000.0,
            orientation_derate: 0.5,
            lead_angle_deg: 30.0,
            return_angle_deg: 60.0,
            friction: 0.3,
            clearance_mm: 0.3,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 0.01 * b.abs()
    }

    fn volume(d: &SnapDesign) -> f64 {
        d.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| d.vertices[i as usize]);
                (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                    + a[2] * (b[0] * c[1] - b[1] * c[0]))
                    / 6.0
            })
            .sum()
    }

    #[test]
    fn snap_geometry_beam_numbers_match_hand_calculation() {
        // y_max = 0.67*0.01*400/2 = 1.34; I = 6*8/12 = 4; k = 3*2000*4/8000 = 3 N/mm.
        let d = design(&input(1.0)).unwrap();
        assert!(close(d.max_deflection_mm, 1.34));
        assert!(close(d.stiffness_n_per_mm, 3.0));
        assert!(close(d.assembly_force_n, 3.0));
        // strain = 1*2/(0.67*400) = 0.0074627; margin = 1 - 0.74627 = 0.25373.
        assert!(close(d.strain, 0.0074627));
        assert!(close(d.strain_margin, 0.25373));
        assert!(!d.over_strain);
        // W = 3*(0.3+tan30)/(1-0.3 tan30) = 3.1834; retention with 60 deg = 12.690.
        assert!(close(d.insertion_force_n, 3.1834));
        assert!(close(d.retention_force_n.unwrap(), 12.690));
    }

    #[test]
    fn snap_geometry_mesh_is_watertight_with_expected_volume() {
        let d = design(&input(1.0)).unwrap();
        let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
        for t in &d.triangles {
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        assert!(edges.values().all(|&c| c == 2), "open or non-manifold edge");
        // Hand area: wall 2*5.3 + slab 20*2 + arm 20*2 + hook 0.5*(1.73205+0.57735)*1 = 91.7547.
        assert!(close(volume(&d), 91.7547 * 6.0), "{}", volume(&d));
    }

    #[test]
    fn snap_geometry_over_strain_is_flagged() {
        // strain = 1.5*2/268 = 0.011194 > 0.01 allowed.
        let d = design(&input(1.5)).unwrap();
        assert!(d.over_strain);
        assert!(d.strain_margin < 0.0 && close(d.strain_margin, 1.0 - 0.011194 / 0.01));
    }

    #[test]
    fn snap_geometry_vertical_return_face_is_self_locking() {
        let mut i = input(1.0);
        i.return_angle_deg = 90.0;
        let d = design(&i).unwrap();
        assert_eq!(d.retention_force_n, None);
        let mut e: HashMap<(u32, u32), u32> = HashMap::new();
        for t in &d.triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                *e.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        assert!(e.values().all(|&c| c == 2));
    }

    #[test]
    fn snap_geometry_rejects_bad_inputs() {
        let mut i = input(1.0);
        i.thickness_mm = 0.0;
        assert!(design(&i).is_err());
        let mut i = input(1.0);
        i.orientation_derate = 1.5;
        assert!(design(&i).is_err());
        let mut i = input(1.0);
        i.length_mm = 2.0; // hook runs longer than the arm
        assert!(design(&i).is_err());
        let mut i = input(1.0);
        i.friction = 2.0; // mu*tan30 > ... lead 30: 2*0.577 > 1
        assert!(design(&i).is_err());
    }
}

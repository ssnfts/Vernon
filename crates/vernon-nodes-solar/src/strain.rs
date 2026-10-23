//! Per-triangle strain between a 3-D surface and its flat pattern.

use std::f64::consts::PI;

use glam::{DVec2, DVec3};
use vernon_core::Geometry;

use crate::lscm::local_frame;
use crate::topo::TopoInfo;

#[derive(Clone, Debug)]
pub struct StrainOpts {
    /// Strains below this (percent length change) do not count as affected.
    pub deadband_pct: f64,
    pub max_markers: usize,
}

impl Default for StrainOpts {
    fn default() -> Self {
        StrainOpts {
            deadband_pct: 0.01,
            max_markers: 10,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct StrainStats {
    /// Area change of the whole pattern, percent.
    pub area_change_pct: f64,
    pub stretch_avg_pct: f64,
    pub stretch_max_pct: f64,
    /// Share of the 3-D area with stretch above the dead-band, percent.
    pub stretch_share_pct: f64,
    pub compress_avg_pct: f64,
    pub compress_max_pct: f64,
    pub compress_share_pct: f64,
    /// Worst absolute length change, percent.
    pub max_length_change_pct: f64,
    pub flipped: usize,
    pub degenerate: usize,
    pub triangles: usize,
}

#[derive(Clone, Debug)]
pub struct StrainResult {
    /// Fan triangles `(prim, vertices)` in the order used below.
    pub tris: Vec<(usize, [u32; 3])>,
    pub tri_stretch: Vec<f64>,
    pub tri_compress: Vec<f64>,
    pub tri_area_change: Vec<f64>,
    pub prim_stretch: Vec<f64>,
    pub prim_compress: Vec<f64>,
    pub prim_area_change: Vec<f64>,
    /// Signed worst length change per primitive: positive stretch, negative compression.
    pub prim_strain: Vec<f64>,
    /// Marker positions (3-D triangle centroids) and signed percent values.
    pub markers: Vec<(DVec3, f64)>,
    pub stats: StrainStats,
}

/// Singular values of the Jacobian from the 3-D triangle to the 2-D one and
/// the signed determinant. `None` for a degenerate 3-D triangle.
pub fn tri_jacobian(p: [DVec3; 3], u: [DVec2; 3]) -> Option<(f64, f64, f64)> {
    let x = local_frame(p)?;
    let (l1, cx, cy) = (x[1].x, x[2].x, x[2].y);
    let d1 = u[1] - u[0];
    let d2 = u[2] - u[0];
    let a = d1.x / l1;
    let c = d1.y / l1;
    let b = -d1.x * cx / (l1 * cy) + d2.x / cy;
    let d = -d1.y * cx / (l1 * cy) + d2.y / cy;
    let q = (((a + d) * 0.5).powi(2) + ((c - b) * 0.5).powi(2)).sqrt();
    let r = (((a - d) * 0.5).powi(2) + ((c + b) * 0.5).powi(2)).sqrt();
    Some((q + r, q - r, a * d - b * c))
}

/// Strain of the pattern `uv` (indexed like `g.points`) against `g`.
pub fn compute_strain(g: &Geometry, uv: &[DVec2], o: &StrainOpts) -> StrainResult {
    let tris = g.triangles();
    let np = g.prims.len();
    let mut tri_stretch = vec![0.0; tris.len()];
    let mut tri_compress = vec![0.0; tris.len()];
    let mut tri_area = vec![0.0; tris.len()];
    let mut a3 = vec![0.0; tris.len()];
    let mut a2 = vec![0.0; tris.len()];
    let mut st = StrainStats {
        triangles: tris.len(),
        ..Default::default()
    };
    let (mut sum3, mut sum2) = (0.0, 0.0);
    let mut acc = [[0.0f64; 3]; 2]; // [stretch, compress] x [sum area*s, sum area affected, max]
    let mut worst: Vec<(f64, usize)> = Vec::new();
    let dead = o.deadband_pct;
    for (ti, (_, t)) in tris.iter().enumerate() {
        let p = [
            g.points[t[0] as usize],
            g.points[t[1] as usize],
            g.points[t[2] as usize],
        ];
        let u = [uv[t[0] as usize], uv[t[1] as usize], uv[t[2] as usize]];
        let area = (p[1] - p[0]).cross(p[2] - p[0]).length() * 0.5;
        a3[ti] = area;
        a2[ti] = 0.5 * (u[1] - u[0]).perp_dot(u[2] - u[0]);
        let Some((s1, s2, det)) = tri_jacobian(p, u) else {
            st.degenerate += 1;
            continue;
        };
        if det < 0.0 {
            st.flipped += 1;
        }
        let stretch = ((s1 - 1.0).max(0.0)) * 100.0;
        let compress = ((1.0 - s2).max(0.0)) * 100.0;
        tri_stretch[ti] = stretch;
        tri_compress[ti] = compress;
        tri_area[ti] = (det - 1.0) * 100.0;
        sum3 += area;
        sum2 += a2[ti];
        if stretch > dead {
            acc[0][0] += area * stretch;
            acc[0][1] += area;
        }
        if compress > dead {
            acc[1][0] += area * compress;
            acc[1][1] += area;
        }
        acc[0][2] = acc[0][2].max(stretch);
        acc[1][2] = acc[1][2].max(compress);
        worst.push((stretch.max(compress), ti));
    }
    if sum3 > 0.0 {
        st.area_change_pct = (sum2 / sum3 - 1.0) * 100.0;
        st.stretch_share_pct = 100.0 * acc[0][1] / sum3;
        st.compress_share_pct = 100.0 * acc[1][1] / sum3;
    }
    st.stretch_avg_pct = if acc[0][1] > 0.0 {
        acc[0][0] / acc[0][1]
    } else {
        0.0
    };
    st.compress_avg_pct = if acc[1][1] > 0.0 {
        acc[1][0] / acc[1][1]
    } else {
        0.0
    };
    st.stretch_max_pct = acc[0][2];
    st.compress_max_pct = acc[1][2];
    st.max_length_change_pct = acc[0][2].max(acc[1][2]);
    worst.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let markers = worst
        .iter()
        .take(o.max_markers)
        .filter(|(w, _)| *w > dead)
        .map(|&(_, ti)| {
            let t = tris[ti].1;
            let c =
                (g.points[t[0] as usize] + g.points[t[1] as usize] + g.points[t[2] as usize]) / 3.0;
            let v = if tri_stretch[ti] >= tri_compress[ti] {
                tri_stretch[ti]
            } else {
                -tri_compress[ti]
            };
            (c, v)
        })
        .collect();
    let mut prim_stretch = vec![0.0; np];
    let mut prim_compress = vec![0.0; np];
    let (mut s3, mut s2p) = (vec![0.0; np], vec![0.0; np]);
    for (ti, (pi, _)) in tris.iter().enumerate() {
        prim_stretch[*pi] = f64::max(prim_stretch[*pi], tri_stretch[ti]);
        prim_compress[*pi] = f64::max(prim_compress[*pi], tri_compress[ti]);
        s3[*pi] += a3[ti];
        s2p[*pi] += a2[ti];
    }
    let prim_area_change: Vec<f64> = (0..np)
        .map(|i| {
            if s3[i] > 0.0 {
                (s2p[i] / s3[i] - 1.0) * 100.0
            } else {
                0.0
            }
        })
        .collect();
    let prim_strain: Vec<f64> = (0..np)
        .map(|i| {
            if prim_stretch[i] >= prim_compress[i] {
                prim_stretch[i]
            } else {
                -prim_compress[i]
            }
        })
        .collect();
    StrainResult {
        tris,
        tri_stretch,
        tri_compress,
        tri_area_change: tri_area,
        prim_stretch,
        prim_compress,
        prim_area_change,
        prim_strain,
        markers,
        stats: st,
    }
}

/// Sum of |angle defect| over interior vertices, radians: zero for a
/// developable surface.
pub fn angle_defect_sum(g: &Geometry, topo: &TopoInfo) -> f64 {
    let mut sum = vec![0.0f64; g.points.len()];
    let mut used = vec![false; g.points.len()];
    for (_, t) in g.triangles() {
        let p = [
            g.points[t[0] as usize],
            g.points[t[1] as usize],
            g.points[t[2] as usize],
        ];
        for k in 0..3 {
            let (a, b) = (p[(k + 1) % 3] - p[k], p[(k + 2) % 3] - p[k]);
            let ang = a.cross(b).length().atan2(a.dot(b));
            sum[t[k] as usize] += ang;
            used[t[k] as usize] = true;
        }
    }
    let mut boundary = vec![false; g.points.len()];
    for lp in &topo.loops {
        for &v in lp {
            boundary[v as usize] = true;
        }
    }
    (0..g.points.len())
        .filter(|&i| used[i] && !boundary[i])
        .map(|i| (2.0 * PI - sum[i]).abs())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri_pair() -> (Geometry, Vec<DVec2>) {
        let g = Geometry::from_polygons(
            vec![
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(10.0, 0.0, 0.0),
                DVec3::new(10.0, 10.0, 0.0),
                DVec3::new(0.0, 10.0, 0.0),
            ],
            vec![vec![0, 1, 2], vec![0, 2, 3]],
        );
        let uv = g.points.iter().map(|p| DVec2::new(p.x, p.y)).collect();
        (g, uv)
    }

    #[test]
    fn identity_has_no_strain() {
        let (g, uv) = tri_pair();
        let s = compute_strain(&g, &uv, &StrainOpts::default());
        assert!(s.stats.max_length_change_pct < 1e-9);
        assert!(s.stats.area_change_pct.abs() < 1e-9);
        assert_eq!(s.stats.flipped, 0);
        assert!(s.markers.is_empty());
    }

    #[test]
    fn uniform_scale_matches_the_synthetic_report() {
        let (g, uv) = tri_pair();
        let big: Vec<DVec2> = uv.iter().map(|u| *u * 1.003).collect();
        let s = compute_strain(&g, &big, &StrainOpts::default()).stats;
        assert!((s.stretch_avg_pct - 0.3).abs() < 1e-9 && (s.stretch_max_pct - 0.3).abs() < 1e-9);
        assert!((s.stretch_share_pct - 100.0).abs() < 1e-9);
        assert!((s.area_change_pct - 0.6009).abs() < 1e-9);
        assert_eq!(s.compress_max_pct, 0.0);
        let small: Vec<DVec2> = uv.iter().map(|u| *u * 0.9995).collect();
        let s = compute_strain(&g, &small, &StrainOpts::default()).stats;
        assert!((s.compress_avg_pct - 0.05).abs() < 1e-9);
        assert!((s.area_change_pct + 0.09997500).abs() < 1e-6);
    }

    #[test]
    fn mirrored_triangle_is_flagged() {
        let (g, mut uv) = tri_pair();
        uv[3] = DVec2::new(10.0, -10.0); // flips the second triangle
        let s = compute_strain(&g, &uv, &StrainOpts::default());
        assert_eq!(s.stats.flipped, 1);
    }

    #[test]
    fn anisotropic_stretch_uses_singular_values() {
        let (g, uv) = tri_pair();
        let st: Vec<DVec2> = uv.iter().map(|u| DVec2::new(u.x * 1.1, u.y)).collect();
        let s = compute_strain(&g, &st, &StrainOpts::default());
        assert!((s.stats.stretch_max_pct - 10.0).abs() < 1e-9);
        assert!(s.stats.compress_max_pct < 1e-9);
        assert!((s.stats.area_change_pct - 10.0).abs() < 1e-9);
    }
}

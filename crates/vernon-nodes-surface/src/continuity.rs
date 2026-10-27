//! Sampled geometric continuity across explicitly expected surface seams.
use crate::nurbs::NurbsSurface;
use glam::DVec3;

#[derive(Debug, Clone, Copy)]
pub struct SeamRef {
    pub a: usize,
    pub side_a: usize,
    pub b: usize,
    pub side_b: usize,
    pub reversed: bool,
}
#[derive(Debug, Clone, Default)]
pub struct ContinuityStats {
    pub max_gap_mm: f64,
    pub max_normal_deg: f64,
    pub max_curvature_jump_inv_mm: f64,
    pub g1_fraction: f64,
    pub g2_fraction: f64,
    pub invalid_samples: usize,
    pub expected_seams: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::{arc_nurbs, PatchSet};
    fn parabola(x0: f64, a: f64, domain: (f64, f64)) -> NurbsSurface {
        let mut s = NurbsSurface::clamped_uniform(
            3,
            3,
            4,
            4,
            (0..4)
                .flat_map(|i| {
                    (0..4).map(move |j| {
                        let q = i as f64;
                        let z = a * (x0 * x0 + 2.0 * x0 * q / 3.0 + q * (q - 1.0) / 6.0);
                        DVec3::new(x0 + q / 3.0, j as f64 / 3.0, z)
                    })
                })
                .collect(),
        );
        for k in &mut s.knots_u {
            *k = domain.0 + *k * (domain.1 - domain.0);
        }
        s
    }
    fn seam() -> SeamRef {
        SeamRef {
            a: 0,
            side_a: 1,
            b: 1,
            side_b: 3,
            reversed: false,
        }
    }
    #[test]
    fn shape_operator_is_invariant_to_parameter_domains() {
        let ps = PatchSet::new(vec![
            parabola(-1.0, 0.2, (2.0, 5.0)),
            parabola(0.0, 0.2, (8.0, 20.0)),
        ]);
        let r = measure(&ps, &[seam()], 37);
        assert!(r.max_gap_mm < 1e-10);
        assert!(r.max_normal_deg < 1e-5);
        assert!(r.max_curvature_jump_inv_mm < 1e-10);
        assert_eq!(r.g2_fraction, 1.0);
        assert_eq!(r.invalid_samples, 0);
    }
    #[test]
    fn c1_join_with_a_curvature_jump_is_not_g2() {
        let ps = PatchSet::new(vec![
            parabola(-1.0, 0.1, (0.0, 1.0)),
            parabola(0.0, 0.4, (0.0, 1.0)),
        ]);
        let r = measure(&ps, &[seam()], 29);
        assert_eq!(r.g1_fraction, 1.0);
        assert_eq!(r.g2_fraction, 0.0);
        assert!((r.max_curvature_jump_inv_mm - 0.6).abs() < 1e-10, "{r:?}");
    }

    #[test]
    fn reversed_parameters_and_opposed_normals_preserve_geometric_g2() {
        let a = parabola(-1.0, 0.2, (2.0, 5.0));
        let mut b = parabola(0.0, 0.2, (8.0, 20.0));
        for row in b.control_points.chunks_mut(b.nv) {
            row.reverse();
        }
        let ps = PatchSet::new(vec![a, b]);
        let r = measure(
            &ps,
            &[SeamRef {
                reversed: true,
                ..seam()
            }],
            43,
        );
        assert_eq!(r.invalid_samples, 0);
        assert!(
            r.max_gap_mm < 1e-10 && r.max_curvature_jump_inv_mm < 1e-10,
            "{r:?}"
        );
        assert_eq!(r.g2_fraction, 1.0);
        let wrong = measure(&ps, &[seam()], 43);
        assert_eq!(wrong.g2_fraction, 0.0);
        assert!(wrong.max_gap_mm > 0.9);
    }
    #[test]
    fn rational_cylinder_curvature_matches_across_arc_patches() {
        let make = |a: f64, b: f64| {
            let c = arc_nurbs(DVec3::ZERO, DVec3::X, DVec3::Z, 10.0, a, b);
            let mut s = NurbsSurface::clamped_uniform(
                2,
                1,
                3,
                2,
                c.control_points
                    .iter()
                    .flat_map(|p| [*p, *p + DVec3::Y * 7.0])
                    .collect(),
            );
            s.weights = Some(c.weights.unwrap().iter().flat_map(|&w| [w, w]).collect());
            s
        };
        let ps = PatchSet::new(vec![
            make(0.0, std::f64::consts::FRAC_PI_4),
            make(std::f64::consts::FRAC_PI_4, std::f64::consts::FRAC_PI_2),
        ]);
        let r = measure(&ps, &[seam()], 31);
        assert_eq!(r.g2_fraction, 1.0);
        assert!(r.max_curvature_jump_inv_mm < 1e-10, "{r:?}");
    }
    #[test]
    fn missing_gap_and_singular_samples_cannot_certify_continuity() {
        let mut b = parabola(0.0, 0.2, (0.0, 1.0));
        for p in &mut b.control_points {
            p.z += 0.1;
        }
        let ps = PatchSet::new(vec![parabola(-1.0, 0.2, (0.0, 1.0)), b]);
        let r = measure(&ps, &[seam()], 19);
        assert_eq!(r.expected_seams, 1);
        assert!(r.max_gap_mm >= 0.099);
        assert_eq!(r.g2_fraction, 0.0);
        let r = measure(&ps, &[], 19);
        assert_eq!(r.g2_fraction, 0.0);
        let zero = NurbsSurface::clamped_uniform(3, 3, 4, 4, vec![DVec3::ZERO; 16]);
        let r = measure(&PatchSet::new(vec![zero.clone(), zero]), &[seam()], 19);
        assert!(r.invalid_samples > 0);
        assert_eq!(r.g1_fraction, 0.0);
        assert_eq!(r.g2_fraction, 0.0);
        let r = measure(&ps, &[SeamRef { a: 7, ..seam() }], 19);
        assert!(r.invalid_samples > 0);
    }
}

#[derive(Clone, Copy)]
struct Jet {
    p: DVec3,
    du: DVec3,
    dv: DVec3,
    duu: DVec3,
    duv: DVec3,
    dvv: DVec3,
    n: DVec3,
}

/// Rational quotient jets, including exact second derivatives. Parameters may
/// have any positive domain; curvature is measured in a physical tangent frame.
fn jet(s: &NurbsSurface, u: f64, v: f64) -> Option<Jet> {
    use crate::nurbs::{ders_basis_funs, find_span};
    let su = find_span(s.nu, s.degree_u, u, &s.knots_u);
    let sv = find_span(s.nv, s.degree_v, v, &s.knots_v);
    let bu = ders_basis_funs(su, u, s.degree_u, s.degree_u.min(2), &s.knots_u);
    let bv = ders_basis_funs(sv, v, s.degree_v, s.degree_v.min(2), &s.knots_v);
    let pairs = [(0, 0), (1, 0), (0, 1), (2, 0), (1, 1), (0, 2)];
    let mut a = [DVec3::ZERO; 6];
    let mut w = [0.0; 6];
    for i in 0..=s.degree_u {
        for j in 0..=s.degree_v {
            let index = (su - s.degree_u + i) * s.nv + sv - s.degree_v + j;
            let weight = s.weights.as_ref().map_or(1.0, |w| w[index]);
            let cp = s.control_points[index];
            for (k, &(du, dv)) in pairs.iter().enumerate() {
                let b =
                    bu.get(du).map_or(0.0, |b| b[i]) * bv.get(dv).map_or(0.0, |b| b[j]) * weight;
                a[k] += cp * b;
                w[k] += b;
            }
        }
    }
    if !w[0].is_finite() || w[0] <= 1e-30 {
        return None;
    }
    let p = a[0] / w[0];
    let du = (a[1] - p * w[1]) / w[0];
    let dv = (a[2] - p * w[2]) / w[0];
    let duu = (a[3] - du * (2.0 * w[1]) - p * w[3]) / w[0];
    let duv = (a[4] - dv * w[1] - du * w[2] - p * w[4]) / w[0];
    let dvv = (a[5] - dv * (2.0 * w[2]) - p * w[5]) / w[0];
    let n = du.cross(dv);
    let scale = du.length_squared() * dv.length_squared();
    if [p, du, dv, duu, duv, dvv].iter().any(|v| !v.is_finite())
        || scale <= 1e-60
        || n.length_squared() <= 1e-20 * scale
    {
        return None;
    }
    Some(Jet {
        p,
        du,
        dv,
        duu,
        duv,
        dvv,
        n: n.normalize(),
    })
}

fn normal_form(j: Jet, e: DVec3, f: DVec3, sign: f64) -> f64 {
    let (ee, ff, gg) = (j.du.length_squared(), j.du.dot(j.dv), j.dv.length_squared());
    let det = ee * gg - ff * ff;
    let coefficients = |v: DVec3| {
        (
            (v.dot(j.du) * gg - v.dot(j.dv) * ff) / det,
            (v.dot(j.dv) * ee - v.dot(j.du) * ff) / det,
        )
    };
    let (a, b) = coefficients(e);
    let (c, d) = coefficients(f);
    sign * (j.n.dot(j.duu) * a * c + j.n.dot(j.duv) * (a * d + b * c) + j.n.dot(j.dvv) * b * d)
}
fn side_jet(s: &NurbsSurface, side: usize, t: f64) -> Option<Jet> {
    let (a, b) = s.domain_u();
    let (c, d) = s.domain_v();
    let (u, v) = match side {
        0 => (a + (b - a) * t, c),
        1 => (b, c + (d - c) * t),
        2 => (a + (b - a) * t, d),
        3 => (a, c + (d - c) * t),
        _ => return None,
    };
    jet(s, u, v)
}

/// Every expected seam stays in the denominator, even if its gap prevents a
/// geometric matcher from finding it. Degenerate samples count as failures.
/// Fractions count complete seams passing 0.01 mm / 1 degree / 0.001 mm^-1
/// sampled thresholds. They do not certify exact mathematical continuity.
pub fn measure(ps: &crate::nurbs::PatchSet, seams: &[SeamRef], samples: usize) -> ContinuityStats {
    let count = samples.clamp(2, 1024);
    let mut out = ContinuityStats {
        expected_seams: seams.len(),
        ..Default::default()
    };
    let (mut g1, mut g2) = (0, 0);
    for seam in seams {
        let (mut good1, mut good2) = (true, true);
        let surfaces = ps
            .patches
            .get(seam.a)
            .zip(ps.patches.get(seam.b))
            .filter(|(a, b)| a.surface.validate().is_ok() && b.surface.validate().is_ok());
        for k in 0..=count {
            let t = k as f64 / count as f64;
            let pair = surfaces.and_then(|(a, b)| {
                side_jet(&a.surface, seam.side_a, t).zip(side_jet(
                    &b.surface,
                    seam.side_b,
                    if seam.reversed { 1.0 - t } else { t },
                ))
            });
            let Some((a, b)) = pair else {
                out.invalid_samples += 1;
                out.max_normal_deg = 180.0;
                out.max_curvature_jump_inv_mm = 1e12;
                good1 = false;
                good2 = false;
                continue;
            };
            let sign = if a.n.dot(b.n) < 0.0 { -1.0 } else { 1.0 };
            let n = (a.n + b.n * sign).normalize_or_zero();
            let tangent = if seam.side_a.checked_rem(2) == Some(0) {
                a.du
            } else {
                a.dv
            };
            let e1 = (tangent - n * tangent.dot(n)).normalize_or_zero();
            let e2 = n.cross(e1).normalize_or_zero();
            if e1.length_squared() < 0.5 || e2.length_squared() < 0.5 {
                out.invalid_samples += 1;
                out.max_normal_deg = 180.0;
                out.max_curvature_jump_inv_mm = 1e12;
                good1 = false;
                good2 = false;
                continue;
            }
            let gap = a.p.distance(b.p);
            let angle = a.n.dot(b.n).abs().clamp(0.0, 1.0).acos().to_degrees();
            let aa = normal_form(a, e1, e1, 1.0) - normal_form(b, e1, e1, sign);
            let ab = normal_form(a, e1, e2, 1.0) - normal_form(b, e1, e2, sign);
            let bb = normal_form(a, e2, e2, 1.0) - normal_form(b, e2, e2, sign);
            let jump = (aa * aa + 2.0 * ab * ab + bb * bb).sqrt();
            if !gap.is_finite() || !angle.is_finite() || !jump.is_finite() {
                out.invalid_samples += 1;
                out.max_normal_deg = 180.0;
                out.max_curvature_jump_inv_mm = 1e12;
                good1 = false;
                good2 = false;
                continue;
            }
            out.max_gap_mm = out.max_gap_mm.max(gap);
            out.max_normal_deg = out.max_normal_deg.max(angle);
            out.max_curvature_jump_inv_mm = out.max_curvature_jump_inv_mm.max(jump);
            good1 &= gap <= 0.01 && angle <= 1.0;
            good2 &= gap <= 0.01 && angle <= 1.0 && jump <= 0.001;
        }
        g1 += usize::from(good1);
        g2 += usize::from(good2);
    }
    if !seams.is_empty() {
        out.g1_fraction = g1 as f64 / seams.len() as f64;
        out.g2_fraction = g2 as f64 / seams.len() as f64;
    }
    out
}

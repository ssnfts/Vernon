//! Exact tensor splitting and measured whole-edge patch reconciliation.
use crate::nurbs::{NurbsCurve, NurbsSurface, PatchSet};
use glam::DVec3;

#[cfg(test)]
mod tests {
    use super::*;
    fn sheet(x: f64, reverse: bool) -> NurbsSurface {
        NurbsSurface::clamped_uniform(
            3,
            3,
            4,
            4,
            (0..4)
                .flat_map(|i| {
                    (0..4).map(move |j| {
                        let y = if reverse { 3 - j } else { j };
                        DVec3::new(x + i as f64, y as f64, 0.1 * (i * i) as f64)
                    })
                })
                .collect(),
        )
    }
    #[test]
    fn exact_split_preserves_shape_and_derivatives() {
        let s = sheet(0.0, false);
        for axis in 0..2 {
            let (a, b) = split_surface(&s, axis, 0.37).unwrap();
            for i in 0..37 {
                for j in 0..29 {
                    let u = i as f64 / 36.0;
                    let v = j as f64 / 28.0;
                    let p = if [u, v][axis] <= 0.37 { &a } else { &b };
                    let (q, du, dv) = s.derivs(u, v);
                    let (r, eu, ev) = p.derivs(u, v);
                    assert!((q - r).length() < 1e-11);
                    assert!((du - eu).length() < 1e-10);
                    assert!((dv - ev).length() < 1e-10);
                }
            }
        }
        assert!(split_surface(&s, 2, 0.5).is_err());
        assert!(split_surface(&s, 0, 0.0).is_err());
    }
    #[test]
    fn reversed_unequal_nets_reconcile_position_and_tangent() {
        let a = sheet(0.0, false);
        let mut b = sheet(3.001, true);
        for p in &mut b.control_points {
            p.z += 0.9;
        }
        let b = refine_axis(&b, 1, 0.41);
        let mut ps = PatchSet::new(vec![a, b]);
        let st = reconcile(&mut ps, true, 1.0);
        assert_eq!(st.seams, 1);
        assert!(st.max_gap_mm < 1e-9, "{:?}", st);
        assert!(st.max_normal_deg < 1e-4, "{:?}", st);
        assert_eq!(ps.patches[0].boundary_ids[1], ps.patches[1].boundary_ids[0]);
    }
    #[test]
    fn ambiguous_and_degenerate_edges_are_not_successes() {
        let a = sheet(0.0, false);
        let b = sheet(3.0, false);
        let mut ps = PatchSet::new(vec![a, b.clone(), b]);
        assert_eq!(reconcile(&mut ps, false, 0.01).seams, 0);
        let mut s = sheet(0.0, false);
        s.control_points.fill(DVec3::ZERO);
        let mut ps = PatchSet::new(vec![s.clone(), s]);
        let st = reconcile(&mut ps, true, 0.01);
        assert_eq!(st.seams, 0);
        assert_eq!(st.unmatched_edges, 8);
    }
}

#[cfg(test)]
mod junction_tests {
    use super::*;
    #[test]
    fn four_way_corner_and_knot_propagation() {
        let mut surfaces = Vec::new();
        for x in 0..2 {
            for y in 0..2 {
                let cps = (0..4)
                    .flat_map(|i| {
                        (0..4).map(move |j| {
                            DVec3::new(
                                x as f64 * 3.0 + i as f64,
                                y as f64 * 3.0 + j as f64,
                                0.001 * (x + y) as f64,
                            )
                        })
                    })
                    .collect();
                let mut s = NurbsSurface::clamped_uniform(3, 3, 4, 4, cps);
                if x == 1 {
                    s = refine_axis(&s, 1, 0.31);
                }
                if y == 1 {
                    s = refine_axis(&s, 0, 0.63);
                }
                surfaces.push(s);
            }
        }
        let mut ps = PatchSet::new(surfaces);
        let st = reconcile(&mut ps, true, 0.01);
        assert_eq!(st.seams, 4);
        assert!(st.max_gap_mm < 1e-8, "{st:?}");
        assert!(st.max_normal_deg < 1e-3, "{st:?}");
    }
    #[test]
    fn split_at_existing_nonuniform_knots_and_invalid_rational() {
        let cps = (0..4)
            .flat_map(|i| (0..4).map(move |j| DVec3::new(i as f64, j as f64, (i * j) as f64)))
            .collect();
        let s = NurbsSurface::clamped_uniform(3, 3, 4, 4, cps);
        let s = refine_axis(&refine_axis(&s, 0, 0.23), 0, 0.71);
        let (a, b) = split_surface(&s, 0, 0.23).unwrap();
        for i in 0..101 {
            let u = i as f64 / 100.0;
            let q = if u <= 0.23 { &a } else { &b };
            assert!((q.point(u, 0.43) - s.point(u, 0.43)).length() < 1e-11);
        }
        let mut rational = s.clone();
        rational.weights = Some(vec![1.0; s.control_points.len()]);
        assert!(split_surface(&rational, 0, 0.4).is_err());
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn split_near_existing_knot() {
        let cp = (0..4)
            .flat_map(|i| (0..4).map(move |j| DVec3::new(i as f64, j as f64, (i * j) as f64)))
            .collect();
        let s = refine_axis(&NurbsSurface::clamped_uniform(3, 3, 4, 4, cp), 0, 0.5);
        for t in [0.5 - 1e-14, 0.5 + 1e-14] {
            let (a, b) = split_surface(&s, 0, t).unwrap();
            assert_eq!(a.domain_u().1, 0.5);
            assert_eq!(b.domain_u().0, 0.5);
        }
    }
    #[test]
    fn knot_union_resource_limit_rolls_back() {
        let plane = |nv, x| {
            let k = crate::nurbs::clamped_uniform_knots(nv, 3);
            let cp = (0..4)
                .flat_map(|i| {
                    (0..nv).map({
                        let k = &k;
                        move |j| {
                            DVec3::new(x + i as f64, (k[j + 1] + k[j + 2] + k[j + 3]) / 3.0, 0.0)
                        }
                    })
                })
                .collect();
            NurbsSurface::new(
                3,
                3,
                vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
                k,
                4,
                nv,
                cp,
            )
        };
        let mut ps = PatchSet::new(vec![plane(70, 0.0), plane(71, 3.0)]);
        let before = ps
            .patches
            .iter()
            .map(|p| p.surface.clone())
            .collect::<Vec<_>>();
        let st = reconcile(&mut ps, true, 0.001);
        assert_eq!(st.seams, 0);
        assert!(!st.max_gap_mm.is_finite());
        assert!(!st.max_normal_deg.is_finite());
        assert_eq!(
            before,
            ps.patches
                .iter()
                .map(|p| p.surface.clone())
                .collect::<Vec<_>>()
        );
    }
}

/// Independent final measurements; invalid normals are reported as infinity.
#[derive(Debug, Clone, Copy)]
pub struct SeamStats {
    pub max_gap_mm: f64,
    pub max_normal_deg: f64,
    pub seams: usize,
    pub unmatched_edges: usize,
}

fn axis_data(s: &NurbsSurface, axis: usize) -> (usize, usize, &[f64]) {
    if axis == 0 {
        (s.nu, s.degree_u, &s.knots_u)
    } else {
        (s.nv, s.degree_v, &s.knots_v)
    }
}
fn refine_axis(s: &NurbsSurface, axis: usize, t: f64) -> NurbsSurface {
    let (n, p, k) = axis_data(s, axis);
    let other = if axis == 0 { s.nv } else { s.nu };
    let mut out = s.clone();
    let mut cp = vec![DVec3::ZERO; (n + 1) * other];
    let mut knots = Vec::new();
    for j in 0..other {
        let pts = (0..n)
            .map(|i| if axis == 0 { s.cp(i, j) } else { s.cp(j, i) })
            .collect();
        let c = NurbsCurve {
            degree: p,
            knots: k.to_vec(),
            control_points: pts,
            weights: None,
        }
        .insert_knot(t, 1);
        knots = c.knots;
        for (i, q) in c.control_points.into_iter().enumerate() {
            let idx = if axis == 0 {
                i * other + j
            } else {
                j * (n + 1) + i
            };
            cp[idx] = q;
        }
    }
    if axis == 0 {
        out.nu += 1;
        out.knots_u = knots;
    } else {
        out.nv += 1;
        out.knots_v = knots;
    }
    out.control_points = cp;
    out
}
/// Split at an interior parameter, retaining the original parameter domains.
pub fn split_surface(
    s: &NurbsSurface,
    axis: usize,
    t: f64,
) -> Result<(NurbsSurface, NurbsSurface), String> {
    s.validate()?;
    if axis > 1 || s.weights.is_some() {
        return Err("split requires a non-rational surface and axis 0 or 1".into());
    }
    let (_, p, k) = axis_data(s, axis);
    // Match the knot validator's compression tolerance: inserting a distinct
    // floating-point value inside that tolerance creates invalid multiplicities.
    let t = k
        .iter()
        .copied()
        .find(|&x| (x - t).abs() <= 1e-12 * (1.0 + x.abs()))
        .unwrap_or(t);
    let lo = k[p];
    let hi = k[k.len() - p - 1];
    if p == 0 || !t.is_finite() || t <= lo || t >= hi {
        return Err("split parameter must be strictly inside a positive-degree domain".into());
    }
    let mut r = s.clone();
    let mult = k.iter().filter(|&&x| x == t).count();
    for _ in mult..p {
        r = refine_axis(&r, axis, t);
    }
    let (n, _, k) = axis_data(&r, axis);
    let first = k.iter().position(|&x| x == t).unwrap();
    let cut = first - 1;
    let mut ka = k[..first + p].to_vec();
    ka.push(t);
    let mut kb = vec![t];
    kb.extend_from_slice(&k[first..]);
    let mut a = r.clone();
    let mut b = r.clone();
    let other = if axis == 0 { r.nv } else { r.nu };
    let extract = |start: usize, end: usize| {
        let mut q = Vec::new();
        if axis == 0 {
            for i in start..end {
                for j in 0..other {
                    q.push(r.cp(i, j));
                }
            }
        } else {
            for j in 0..other {
                for i in start..end {
                    q.push(r.cp(j, i));
                }
            }
        }
        q
    };
    a.control_points = extract(0, cut + 1);
    b.control_points = extract(cut, n);
    if axis == 0 {
        a.nu = cut + 1;
        b.nu = n - cut;
        a.knots_u = ka;
        b.knots_u = kb;
    } else {
        a.nv = cut + 1;
        b.nv = n - cut;
        a.knots_v = ka;
        b.knots_v = kb;
    }
    a.validate()?;
    b.validate()?;
    Ok((a, b))
}
fn uv(s: &NurbsSurface, e: usize, t: f64) -> (f64, f64) {
    let (a, b) = s.domain_u();
    let (c, d) = s.domain_v();
    match e {
        0 => (a, c + (d - c) * t),
        1 => (b, c + (d - c) * t),
        2 => (a + (b - a) * t, c),
        _ => (a + (b - a) * t, d),
    }
}
fn edge_sample(s: &NurbsSurface, e: usize, t: f64) -> (DVec3, DVec3) {
    let (u, v) = uv(s, e, t);
    let (p, a, b) = s.derivs(u, v);
    (p, a.cross(b))
}
fn idx(s: &NurbsSurface, e: usize, i: usize, inner: bool) -> usize {
    let r = usize::from(inner);
    match e {
        0 => r * s.nv + i,
        1 => (s.nu - 1 - r) * s.nv + i,
        2 => i * s.nv + r,
        _ => i * s.nv + s.nv - 1 - r,
    }
}
fn edge_len(s: &NurbsSurface, e: usize) -> usize {
    if e < 2 {
        s.nv
    } else {
        s.nu
    }
}
fn scale(s: &NurbsSurface, e: usize) -> f64 {
    let (n, p, k) = axis_data(s, usize::from(e >= 2));
    if e.is_multiple_of(2) {
        p as f64 / (k[p + 1] - k[p])
    } else {
        p as f64 / (k[n] - k[n - 1])
    }
}
fn normal_knots(s: &NurbsSurface, e: usize, rev: bool) -> Vec<f64> {
    let (_, p, k) = axis_data(s, usize::from(e < 2));
    let a = k[p];
    let b = k[k.len() - p - 1];
    let mut out: Vec<_> = k
        .iter()
        .map(|x| {
            if rev {
                (b - x) / (b - a)
            } else {
                (x - a) / (b - a)
            }
        })
        .collect();
    if rev {
        out.reverse();
    }
    out
}
fn compatible(a: &mut NurbsSurface, ea: usize, b: &mut NurbsSurface, eb: usize, rev: bool) -> bool {
    let ka = normal_knots(a, ea, false);
    let kb = normal_knots(b, eb, rev);
    let mut vals = ka.clone();
    vals.extend(&kb);
    vals.sort_by(f64::total_cmp);
    vals.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
    let mut extra_a = 0;
    let mut extra_b = 0;
    for &t in &vals {
        if t <= 1e-12 || t >= 1.0 - 1e-12 {
            continue;
        }
        let ma = ka.iter().filter(|x| (**x - t).abs() < 1e-12).count();
        let mb = kb.iter().filter(|x| (**x - t).abs() < 1e-12).count();
        extra_a += mb.saturating_sub(ma);
        extra_b += ma.saturating_sub(mb);
    }
    if edge_len(a, ea) + extra_a > 128 || edge_len(b, eb) + extra_b > 128 {
        return false;
    }
    for t in vals {
        if t <= 1e-12 || t >= 1.0 - 1e-12 {
            continue;
        }
        let ma = ka.iter().filter(|x| (**x - t).abs() < 1e-12).count();
        let mb = kb.iter().filter(|x| (**x - t).abs() < 1e-12).count();
        for _ in ma..mb {
            let ax = usize::from(ea < 2);
            let (_, p, k) = axis_data(a, ax);
            let q = k[p] + t * (k[k.len() - p - 1] - k[p]);
            *a = refine_axis(a, ax, q);
        }
        for _ in mb..ma {
            let ax = usize::from(eb < 2);
            let (_, p, k) = axis_data(b, ax);
            let q = k[p] + (if rev { 1.0 - t } else { t }) * (k[k.len() - p - 1] - k[p]);
            *b = refine_axis(b, ax, q);
        }
    }
    true
}
/// Match unambiguous whole edges, refine their knot union and reconcile them.
/// Unmatched edges include exterior boundaries and unsupported degree pairs.
pub fn reconcile(ps: &mut PatchSet, g1: bool, match_tol: f64) -> SeamStats {
    let n = ps.patches.len() * 4;
    for (i, p) in ps.patches.iter_mut().enumerate() {
        p.boundary_ids = (i * 4..i * 4 + 4).collect();
    }
    let failure = SeamStats {
        max_gap_mm: f64::INFINITY,
        max_normal_deg: f64::INFINITY,
        seams: 0,
        unmatched_edges: n,
    };
    if ps.patches.len() > 256
        || ps
            .patches
            .iter()
            .any(|p| p.surface.nu > 128 || p.surface.nv > 128)
    {
        return failure;
    }
    let mut candidates = vec![Vec::new(); n];
    if match_tol.is_finite() && match_tol >= 0.0 {
        for a in 0..n {
            for b in a + 1..n {
                if a / 4 == b / 4 {
                    continue;
                }
                let (sa, sb) = (&ps.patches[a / 4].surface, &ps.patches[b / 4].surface);
                let (ea, eb) = (a % 4, b % 4);
                if sa.validate().is_err()
                    || sb.validate().is_err()
                    || sa.weights.is_some()
                    || sb.weights.is_some()
                    || sa.degree_u == 0
                    || sa.degree_v == 0
                    || sb.degree_u == 0
                    || sb.degree_v == 0
                {
                    continue;
                }
                if axis_data(sa, usize::from(ea < 2)).1 != axis_data(sb, usize::from(eb < 2)).1 {
                    continue;
                }
                for rev in [false, true] {
                    let mut good = true;
                    let mut length = 0.0;
                    let mut prev: Option<DVec3> = None;
                    for i in 0..=32 {
                        let t = i as f64 / 32.0;
                        let (pa, na) = edge_sample(sa, ea, t);
                        let (pb, nb) = edge_sample(sb, eb, if rev { 1.0 - t } else { t });
                        if !pa.is_finite()
                            || !pb.is_finite()
                            || na.length_squared() < 1e-24
                            || nb.length_squared() < 1e-24
                            || (pa - pb).length() > match_tol
                        {
                            good = false;
                            break;
                        }
                        if let Some(p) = prev {
                            length += (pa - p).length();
                        }
                        prev = Some(pa);
                    }
                    if good
                        && length > match_tol.max(1e-8) * 2.0
                        && opposite_interiors(sa, ea, sb, eb)
                    {
                        candidates[a].push((b, rev));
                        candidates[b].push((a, rev));
                    }
                }
            }
        }
    }
    let seams: Vec<_> = (0..n)
        .filter_map(|a| {
            if candidates[a].len() == 1 {
                let (b, r) = candidates[a][0];
                if b > a && candidates[b].len() == 1 {
                    Some((a, b, r))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();
    let original = ps.clone();
    for _ in 0..n.max(1) {
        let before: usize = ps
            .patches
            .iter()
            .map(|p| p.surface.control_points.len())
            .sum();
        for &(a, b, r) in &seams {
            let (left, right) = ps.patches.split_at_mut(b / 4);
            if !compatible(
                &mut left[a / 4].surface,
                a % 4,
                &mut right[0].surface,
                b % 4,
                r,
            ) {
                *ps = original;
                return failure;
            }
            right[0].boundary_ids[b % 4] = a;
        }
        let after: usize = ps
            .patches
            .iter()
            .map(|p| p.surface.control_points.len())
            .sum();
        if before == after {
            break;
        }
    }
    // Alternating projections let constraints sharing corner/derivative points converge.
    for _ in 0..128 {
        for &(a, b, rev) in &seams {
            let (left, right) = ps.patches.split_at_mut(b / 4);
            let (sa, sb) = (&mut left[a / 4].surface, &mut right[0].surface);
            let (ea, eb) = (a % 4, b % 4);
            let count = edge_len(sa, ea);
            let (wa, wb) = (scale(sa, ea), scale(sb, eb));
            for i in 0..count {
                let j = if rev { count - 1 - i } else { i };
                let (ia, ib) = (idx(sa, ea, i, false), idx(sb, eb, j, false));
                let qa = sa.control_points[ia];
                let qb = sb.control_points[ib];
                let q = (qa + qb) * 0.5;
                if g1 {
                    let (ja, jb) = (idx(sa, ea, i, true), idx(sb, eb, j, true));
                    let da = (sa.control_points[ja] - qa) * wa;
                    let db = (sb.control_points[jb] - qb) * wb;
                    let d = (da - db) * 0.5;
                    sa.control_points[ja] = q + d / wa;
                    sb.control_points[jb] = q - d / wb;
                }
                sa.control_points[ia] = q;
                sb.control_points[ib] = q;
            }
        }
    }
    let mut st = SeamStats {
        max_gap_mm: 0.0,
        max_normal_deg: 0.0,
        seams: seams.len(),
        unmatched_edges: n - seams.len() * 2,
    };
    for (a, b, r) in seams {
        for i in 0..=128 {
            let t = i as f64 / 128.0;
            let (pa, na) = edge_sample(&ps.patches[a / 4].surface, a % 4, t);
            let (pb, nb) = edge_sample(
                &ps.patches[b / 4].surface,
                b % 4,
                if r { 1.0 - t } else { t },
            );
            st.max_gap_mm = st.max_gap_mm.max((pa - pb).length());
            let angle = if na.length_squared() < 1e-24
                || nb.length_squared() < 1e-24
                || !na.is_finite()
                || !nb.is_finite()
            {
                f64::INFINITY
            } else {
                na.normalize()
                    .dot(nb.normalize())
                    .abs()
                    .clamp(0.0, 1.0)
                    .acos()
                    .to_degrees()
            };
            st.max_normal_deg = st.max_normal_deg.max(angle);
        }
    }
    st
}

fn opposite_interiors(a: &NurbsSurface, ea: usize, b: &NurbsSurface, eb: usize) -> bool {
    let inward = |s: &NurbsSurface, e: usize| {
        let (u, v) = uv(s, e, 0.5);
        let (_, du, dv) = s.derivs(u, v);
        let tangent = if e < 2 { dv } else { du };
        let d = if e < 2 { du } else { dv } * if e.is_multiple_of(2) { 1.0 } else { -1.0 };
        let t = tangent.normalize_or_zero();
        (d - t * d.dot(t)).normalize_or_zero()
    };
    let x = inward(a, ea);
    let y = inward(b, eb);
    x.length_squared() > 0.5 && y.length_squared() > 0.5 && x.dot(y) < 0.5
}

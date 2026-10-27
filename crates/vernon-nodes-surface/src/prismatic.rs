//! Deterministic sample-consensus surface fitting, following the hypothesis,
//! support scoring and refitting pattern of Schnabel, Wahl & Klein (2007),
//! Efficient RANSAC for Point-Cloud Shape Detection. No external source used.
use crate::{
    fit::TriMesh,
    linalg::{scatter, solve_dense, sym_eigen3},
};
use glam::DVec3;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Debug)]
pub(crate) enum Shape {
    Plane {
        origin: DVec3,
        normal: DVec3,
    },
    Sphere {
        center: DVec3,
        radius: f64,
    },
    Cylinder {
        origin: DVec3,
        axis: DVec3,
        radius: f64,
    },
    Cone {
        apex: DVec3,
        axis: DVec3,
        angle: f64,
    },
}
impl Shape {
    pub fn distance(&self, p: DVec3) -> f64 {
        match self {
            Self::Plane { origin, normal } => (p - *origin).dot(*normal).abs(),
            Self::Sphere { center, radius } => ((p - *center).length() - radius).abs(),
            Self::Cylinder {
                origin,
                axis,
                radius,
            } => {
                let q = p - *origin;
                ((q - *axis * q.dot(*axis)).length() - radius).abs()
            }
            Self::Cone { apex, axis, angle } => {
                let q = p - *apex;
                let h = q.dot(*axis);
                if h < 0.0 {
                    q.length()
                } else {
                    ((q - *axis * h).length() * angle.cos() - h * angle.sin()).abs()
                }
            }
        }
    }
    fn normal(&self, p: DVec3) -> DVec3 {
        match self {
            Self::Plane { normal, .. } => *normal,
            Self::Sphere { center, .. } => (p - *center).normalize_or_zero(),
            Self::Cylinder { origin, axis, .. } => {
                let q = p - *origin;
                (q - *axis * q.dot(*axis)).normalize_or_zero()
            }
            Self::Cone { apex, axis, angle } => {
                let q = p - *apex;
                (q - *axis * q.dot(*axis)).normalize_or_zero() * angle.cos() - *axis * angle.sin()
            }
        }
    }
    pub fn json(&self) -> Value {
        match self {
            Self::Plane { origin, normal } => {
                json!({"type":"plane","origin":origin.to_array(),"normal":normal.to_array()})
            }
            Self::Sphere { center, radius } => {
                json!({"type":"sphere","center":center.to_array(),"radius":radius})
            }
            Self::Cylinder {
                origin,
                axis,
                radius,
            } => {
                json!({"type":"cylinder","origin":origin.to_array(),"axis":axis.to_array(),"radius":radius})
            }
            Self::Cone { apex, axis, angle } => {
                json!({"type":"cone","apex":apex.to_array(),"axis":axis.to_array(),"half_angle_rad":angle})
            }
        }
    }
}
pub(crate) struct FitGroup {
    pub id: i64,
    pub tris: Vec<u32>,
    pub shape: Option<Shape>,
    pub rms: f64,
    pub max_dev: f64,
}
fn ls(rows: &[Vec<f64>], rhs: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut a = vec![0.; n * n];
    let mut b = vec![0.; n];
    for (r, y) in rows.iter().zip(rhs) {
        for i in 0..n {
            b[i] += r[i] * y;
            for j in 0..n {
                a[i * n + j] += r[i] * r[j];
            }
        }
    }
    let x = solve_dense(&a, n, &b)?;
    x.iter().all(|v| v.is_finite()).then_some(x)
}
fn candidates(m: &TriMesh, ts: &[u32]) -> Vec<Shape> {
    let ids: BTreeSet<_> = ts.iter().flat_map(|&t| m.tris[t as usize]).collect();
    let p: Vec<_> = ids.iter().map(|&i| m.points[i as usize]).collect();
    if p.len() < 3 {
        return vec![];
    }
    let (c, cov) = scatter(&p, None);
    let (e, v) = sym_eigen3(cov);
    if e[1] <= 1e-14 * e[2].max(1.) {
        return vec![];
    }
    let mut out = vec![Shape::Plane {
        origin: c,
        normal: v[0],
    }];
    let normals: Vec<_> = ts
        .iter()
        .map(|&t| {
            let t = m.tris[t as usize];
            let a = m.points[t[0] as usize];
            let b = m.points[t[1] as usize];
            let c = m.points[t[2] as usize];
            ((a + b + c) / 3., (b - a).cross(c - a).normalize_or_zero())
        })
        .collect();
    if ts.len() < 6 {
        return out;
    }
    let rows: Vec<_> = p
        .iter()
        .map(|q| {
            let q = *q - c;
            vec![2. * q.x, 2. * q.y, 2. * q.z, 1.]
        })
        .collect();
    let rhs: Vec<_> = p.iter().map(|q| (*q - c).length_squared()).collect();
    if let Some(x) = ls(&rows, &rhs, 4) {
        let d = DVec3::new(x[0], x[1], x[2]);
        let r = x[3] + d.length_squared();
        if r > 0. {
            out.push(Shape::Sphere {
                center: c + d,
                radius: r.sqrt(),
            });
        }
    }
    let ns: Vec<_> = normals.iter().map(|(_, n)| *n).collect();
    let mut nc = [[0.; 3]; 3];
    for n in &ns {
        for i in 0..3 {
            for j in 0..3 {
                nc[i][j] += n[i] * n[j];
            }
        }
    }
    let (_, nv) = sym_eigen3(nc);
    let axis = nv[0];
    let ex = axis.any_orthonormal_vector();
    let ey = axis.cross(ex);
    let rows: Vec<_> = p
        .iter()
        .map(|q| {
            let d = *q - c;
            vec![2. * d.dot(ex), 2. * d.dot(ey), 1.]
        })
        .collect();
    let rhs: Vec<_> = p
        .iter()
        .map(|q| {
            let d = *q - c;
            d.dot(ex).powi(2) + d.dot(ey).powi(2)
        })
        .collect();
    if let Some(x) = ls(&rows, &rhs, 3) {
        let r = x[2] + x[0] * x[0] + x[1] * x[1];
        if r > 0. {
            out.push(Shape::Cylinder {
                origin: c + ex * x[0] + ey * x[1],
                axis,
                radius: r.sqrt(),
            });
        }
    }
    // Tangent planes of a cone intersect at the apex. Centred normal covariance
    // has the cone axis as its null direction. This seed is refined geometrically.
    let rows: Vec<_> = normals.iter().map(|(_, n)| n.to_array().to_vec()).collect();
    let rhs: Vec<_> = normals.iter().map(|(q, n)| (*q - c).dot(*n)).collect();
    if let Some(x) = ls(&rows, &rhs, 3) {
        let apex = c + DVec3::new(x[0], x[1], x[2]);
        let (_, cov) = scatter(&ns, None);
        let (_, nv) = sym_eigen3(cov);
        let mut axis = nv[0];
        if (c - apex).dot(axis) < 0. {
            axis = -axis;
        }
        let angle = p
            .iter()
            .map(|q| {
                let d = *q - apex;
                let h = d.dot(axis);
                (d - axis * h).length().atan2(h)
            })
            .sum::<f64>()
            / p.len() as f64;
        if angle > 0.01 && angle < 1.5 {
            out.push(Shape::Cone { apex, axis, angle });
        }
    }
    out
}
// Bounded finite-difference Gauss-Newton on geometric point distances. The
// ridge resolves redundant axis/origin parameters without moving along them.
fn refine(shape: Shape, points: &[DVec3]) -> Shape {
    let (mut x, kind) = match &shape {
        Shape::Plane { origin, normal } => (
            [origin.to_array().as_slice(), normal.to_array().as_slice()].concat(),
            0,
        ),
        Shape::Sphere { center, radius } => {
            let mut x = center.to_array().to_vec();
            x.push(*radius);
            (x, 1)
        }
        Shape::Cylinder {
            origin,
            axis,
            radius,
        } => {
            let mut x = [origin.to_array().as_slice(), axis.to_array().as_slice()].concat();
            x.push(*radius);
            (x, 2)
        }
        Shape::Cone { apex, axis, angle } => {
            let mut x = [apex.to_array().as_slice(), axis.to_array().as_slice()].concat();
            x.push(*angle);
            (x, 3)
        }
    };
    let decode = |x: &[f64]| {
        let p = DVec3::new(x[0], x[1], x[2]);
        if kind == 1 {
            return Shape::Sphere {
                center: p,
                radius: x[3].abs(),
            };
        }
        let n = DVec3::new(x[3], x[4], x[5]).normalize_or_zero();
        match kind {
            0 => Shape::Plane {
                origin: p,
                normal: n,
            },
            2 => Shape::Cylinder {
                origin: p,
                axis: n,
                radius: x[6].abs(),
            },
            _ => Shape::Cone {
                apex: p,
                axis: n,
                angle: x[6].clamp(0.001, 1.56),
            },
        }
    };
    let energy = |s: &Shape| points.iter().map(|&p| s.distance(p).powi(2)).sum::<f64>();
    let mut best = shape;
    let mut e = energy(&best);
    for _ in 0..8 {
        let n = x.len();
        let mut a = vec![0.; n * n];
        let mut b = vec![0.; n];
        for &p in points.iter().step_by(points.len().div_ceil(1000).max(1)) {
            let d = best.distance(p);
            let mut jac = vec![0.; n];
            for j in 0..n {
                let step = 1e-6 * x[j].abs().max(1.);
                let mut y = x.clone();
                y[j] += step;
                let hi = decode(&y).distance(p);
                y[j] -= 2. * step;
                jac[j] = (hi - decode(&y).distance(p)) / (2. * step);
            }
            for i in 0..n {
                b[i] -= jac[i] * d;
                for j in 0..n {
                    a[i * n + j] += jac[i] * jac[j];
                }
            }
        }
        let ridge = (0..n).map(|i| a[i * n + i]).fold(1e-12, f64::max) * 1e-8;
        for i in 0..n {
            a[i * n + i] += ridge;
        }
        let Some(dx) = solve_dense(&a, n, &b) else {
            break;
        };
        let mut accepted = false;
        for scale in [1., 0.5, 0.25, 0.125] {
            let y: Vec<_> = x.iter().zip(&dx).map(|(x, d)| x + scale * d).collect();
            let next = decode(&y);
            let ne = energy(&next);
            if ne < e && ne.is_finite() {
                x = y;
                best = next;
                e = ne;
                accepted = true;
                break;
            }
        }
        if !accepted {
            break;
        }
    }
    best
}
fn residual(m: &TriMesh, t: u32, s: &Shape) -> f64 {
    let t = m.tris[t as usize];
    t.iter()
        .map(|&i| s.distance(m.points[i as usize]))
        .fold(0., f64::max)
}
fn support(m: &TriMesh, ts: &[u32], s: &Shape, tol: f64) -> Vec<u32> {
    ts.iter()
        .copied()
        .filter(|&t| {
            if residual(m, t, s) > tol {
                return false;
            }
            let t = m.tris[t as usize];
            let a = m.points[t[0] as usize];
            let b = m.points[t[1] as usize];
            let c = m.points[t[2] as usize];
            let n = (b - a).cross(c - a).normalize_or_zero();
            n.dot(s.normal((a + b + c) / 3.)).abs() > 0.94
        })
        .collect()
}
const MAX_TRIANGLES: usize = 20_000;
const MAX_WORK: usize = 20_000_000;
fn charge(budget: &mut usize, work: usize) -> Result<(), String> {
    if work > *budget {
        return Err(
            "primitive recognition work budget exceeded; reduce the mesh or split the input".into(),
        );
    }
    *budget -= work;
    Ok(())
}
fn best(
    m: &TriMesh,
    ts: &[u32],
    tol: f64,
    budget: &mut usize,
) -> Result<Option<(Shape, Vec<u32>)>, String> {
    // Charge global/local fit and normal-pair candidate preparation as well as
    // support evaluations. This budget is shared by all labels and extractions.
    charge(budget, ts.len().saturating_mul(128))?;
    let mut hypotheses = candidates(m, ts);
    let mut seed = 0x5eeda11u64;
    for &t in ts.iter().step_by(ts.len().div_ceil(128).max(1)) {
        hypotheses.extend(candidates(m, &[t]));
    }
    // Local normal pairs supply cylinder axes even when the global support mixes
    // several planes and a hole. Refit all normals perpendicular to that axis.
    for k in 0..64.min(ts.len()) {
        let a = ts[k * ts.len() / 64.min(ts.len())];
        let b = ts[(k * 37 + 17) % ts.len()];
        let normal = |t: u32| {
            let t = m.tris[t as usize];
            (m.points[t[1] as usize] - m.points[t[0] as usize])
                .cross(m.points[t[2] as usize] - m.points[t[0] as usize])
                .normalize_or_zero()
        };
        let axis = normal(a).cross(normal(b)).normalize_or_zero();
        if axis.length_squared() < 0.5 {
            continue;
        }
        let subset: Vec<_> = ts
            .iter()
            .copied()
            .filter(|&t| normal(t).dot(axis).abs() < 0.01)
            .collect();
        hypotheses.extend(candidates(m, &subset));
    }

    // Fixed budget and seed make ids and work reproducible. This is a bounded
    // RANSAC variant, without the paper's octree acceleration/probability claim.
    for _ in 0..96 {
        let mut sample = BTreeSet::new();
        for _ in 0..12.min(ts.len()) {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            sample.insert(ts[(seed >> 32) as usize % ts.len()]);
        }
        hypotheses.extend(candidates(m, &sample.into_iter().collect::<Vec<_>>()));
    }
    let mut winner = None;
    let mut count = 0;
    let mut err = f64::INFINITY;
    for s in hypotheses {
        charge(budget, ts.len())?;
        let ss = support(m, ts, &s, tol);
        let min = if matches!(s, Shape::Plane { .. }) {
            2
        } else {
            6
        };
        if ss.len() < min {
            continue;
        }
        let e = ss.iter().map(|&t| residual(m, t, &s).powi(2)).sum::<f64>();
        if ss.len() > count
            || (ss.len() == count
                && (e + tol * tol * ss.len() as f64 * 1e-8 < err
                    || ((e - err).abs() < tol * tol * ss.len() as f64 * 1e-8
                        && matches!(s, Shape::Plane { .. }))))
        {
            count = ss.len();
            err = e;
            winner = Some((s, ss));
        }
    }
    let Some((mut s, mut ss)) = winner else {
        return Ok(None);
    };
    for _ in 0..3 {
        let kind = std::mem::discriminant(&s);
        if let Some(refit) = candidates(m, &ss)
            .into_iter()
            .filter(|s| std::mem::discriminant(s) == kind)
            .min_by(|a, b| {
                ss.iter()
                    .map(|&t| residual(m, t, a).powi(2))
                    .sum::<f64>()
                    .total_cmp(&ss.iter().map(|&t| residual(m, t, b).powi(2)).sum::<f64>())
            })
        {
            charge(budget, ts.len())?;
            let next = support(m, ts, &refit, tol);
            if next.len() >= ss.len() {
                s = refit;
                ss = next;
            }
        }
    }
    let ids: BTreeSet<_> = ss.iter().flat_map(|&t| m.tris[t as usize]).collect();
    let points: Vec<_> = ids.iter().map(|&i| m.points[i as usize]).collect();
    let refined = refine(s.clone(), &points);
    if ss.iter().all(|&t| residual(m, t, &refined) <= tol) {
        s = refined;
    }
    Ok(Some((s, ss)))
}
pub(crate) fn recognize(
    m: &TriMesh,
    labels: Option<&[i64]>,
    tol: f64,
) -> Result<Vec<FitGroup>, String> {
    recognize_budget(m, labels, tol, MAX_WORK)
}
fn recognize_budget(
    m: &TriMesh,
    labels: Option<&[i64]>,
    tol: f64,
    mut budget: usize,
) -> Result<Vec<FitGroup>, String> {
    if m.tris.len() > MAX_TRIANGLES {
        return Err(format!("primitive recognition supports at most {MAX_TRIANGLES} triangles; reduce the mesh or split the input"));
    }
    if labels.is_some_and(|l| l.len() != m.tris.len()) {
        return Err("primitive label count does not match triangle count".into());
    }
    let mut buckets: BTreeMap<i64, Vec<u32>> = BTreeMap::new();
    for t in 0..m.tris.len() {
        let tri = m.tris[t];
        let a = m.points[tri[0] as usize];
        let b = m.points[tri[1] as usize];
        let c = m.points[tri[2] as usize];
        if (b - a).cross(c - a).length_squared() > 1e-24 {
            buckets
                .entry(labels.map_or(0, |l| l[t]))
                .or_default()
                .push(t as u32);
        }
    }
    let mut groups = vec![];
    let bucket_count = buckets.len();
    for (bucket_index, (region, mut remaining)) in buckets.into_iter().enumerate() {
        let buckets_remaining = bucket_index + 1 < bucket_count;
        while !remaining.is_empty() {
            let fit = if region < 0 {
                None
            } else {
                best(m, &remaining, tol, &mut budget)?
            };
            let (shape, tris) = match fit {
                Some((s, t)) => (Some(s), t),
                None => (None, remaining.clone()),
            };
            let mut sum = 0.;
            let mut max: f64 = 0.;
            let mut n = 0;
            for &t in &tris {
                for &v in &m.tris[t as usize] {
                    let d = shape
                        .as_ref()
                        .map_or(0., |s| s.distance(m.points[v as usize]));
                    sum += d * d;
                    max = max.max(d);
                    n += 1;
                }
            }
            let set: BTreeSet<_> = tris.iter().copied().collect();
            remaining.retain(|t| !set.contains(t));
            groups.push(FitGroup {
                id: groups.len() as i64,
                tris,
                shape,
                rms: (sum / n.max(1) as f64).sqrt(),
                max_dev: max,
            });
            if groups.len() >= 512 && (!remaining.is_empty() || buckets_remaining) {
                return Err("primitive recognition exceeds the global 512 group limit; reduce labels or split the input".into());
            }
        }
    }
    Ok(groups)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        segment::seg_mesh,
        testutil::{b, cook, f, geo_of, report_of, some, sphere, tube},
    };
    use vernon_core::{Geometry, Prim};
    fn drilled() -> Geometry {
        let mut g = Geometry::new();
        let n = 64u32;
        for z in [-5., 5.] {
            for outer in [true, false] {
                for i in 0..n {
                    let a = std::f64::consts::TAU * (i as f64 + 0.5) / n as f64;
                    let (x, y) = (a.cos(), a.sin());
                    let r = if outer {
                        10. / x.abs().max(y.abs())
                    } else {
                        3.
                    };
                    g.points.push(DVec3::new(r * x, r * y, z));
                }
            }
        }
        for i in 0..n {
            let j = (i + 1) % n;
            for q in [
                vec![i, j, 2 * n + j, 2 * n + i],
                vec![n + i, 3 * n + i, 3 * n + j, n + j],
                vec![i, n + i, n + j, j],
                vec![2 * n + i, 2 * n + j, 3 * n + j, 3 * n + i],
            ] {
                g.prims.push(Prim::polygon(q));
            }
        }
        // Insert exact corner samples so square side strips never cross a corner.
        // Angles above include no corner: use offset zero instead, with 64/8 integral.
        for p in &mut g.points {
            let a = p.y.atan2(p.x) - std::f64::consts::PI / 64.;
            let radius = if p.x.abs().max(p.y.abs()) > 9. {
                10. / a.cos().abs().max(a.sin().abs())
            } else {
                3.
            };
            p.x = radius * a.cos();
            p.y = radius * a.sin();
        }
        g
    }
    #[test]
    fn drilled_block_automatic_has_seven_analytic_faces() {
        let g = drilled();
        let o = cook("surface.convert_prismatic", &[some(&g)], &[]).unwrap();
        let d = o[0].as_json().unwrap();
        let fs = d["faces"].as_array().unwrap();
        assert_eq!(fs.len(), 7, "{fs:?}");
        assert_eq!(fs.iter().filter(|f| f["type"] == "plane").count(), 6);
        let c = fs.iter().find(|f| f["type"] == "cylinder").unwrap();
        assert!((c["radius"].as_f64().unwrap() / 3. - 1.).abs() < 0.01);
        let r = report_of(&o[1]);
        assert_eq!(r.metrics["watertight"].value, 1.);
        assert!(r.metrics["volume_mm3"].value > 3500.);
        assert_eq!(geo_of(&o[2]).prims.len(), 512);
        assert_eq!(geo_of(&o[2]).prim_attribs.ints("part").unwrap().len(), 512);
        let again = cook("surface.convert_prismatic", &[some(&g)], &[]).unwrap();
        assert_eq!(d, again[0].as_json().unwrap());
    }
    #[test]
    fn transformed_noisy_curved_shapes() {
        let mut cone = Geometry::new();
        for z in [4., 8., 12., 16.] {
            for i in 0..64 {
                let a = std::f64::consts::TAU * i as f64 / 64.;
                cone.points
                    .push(DVec3::new(z * 0.5 * a.cos(), z * 0.5 * a.sin(), z));
            }
        }
        for j in 0..3u32 {
            for i in 0..64u32 {
                let a = j * 64 + i;
                let b = j * 64 + (i + 1) % 64;
                cone.prims.push(Prim::polygon(vec![a, b, b + 64, a + 64]));
            }
        }
        let mut cyl = tube(7., 3., 20., 64);
        cyl.prims.truncate(64);
        cyl.points.truncate(128);
        for (mut g, kind) in [
            (cyl, "cylinder"),
            (sphere(8., 32, 16), "sphere"),
            (cone, "cone"),
        ] {
            let rot = glam::DQuat::from_euler(glam::EulerRot::XYZ, 0.5, 0.9, -0.2);
            for (i, p) in g.points.iter_mut().enumerate() {
                *p = rot * *p
                    + DVec3::new(30., -12., 7.)
                    + DVec3::splat((i as f64 * 1.37).sin() * 0.001);
            }
            let o = cook(
                "surface.convert_prismatic",
                &[some(&g)],
                &[("tol_mm", f(0.02))],
            )
            .unwrap();
            let d = o[0].as_json().unwrap();
            assert_eq!(d["faces"].as_array().unwrap().len(), 1, "{kind}: {d}");
            assert_eq!(d["faces"][0]["type"], kind);
            if kind == "cone" {
                assert!(
                    (d["faces"][0]["half_angle_rad"].as_f64().unwrap() - 0.5f64.atan()).abs()
                        < 0.002
                );
            }
            if kind == "cylinder" {
                assert!((d["faces"][0]["radius"].as_f64().unwrap() - 7.).abs() < 0.01);
            }
            if kind == "sphere" {
                assert!((d["faces"][0]["radius"].as_f64().unwrap() - 8.).abs() < 0.01);
            }
            assert!(d["faces"][0]["max_deviation_mm"].as_f64().unwrap() < 0.02);
        }
    }
    #[test]
    fn malformed_attributes_are_rejected_before_recognition() {
        for attr in [
            vernon_core::AttribArray::Int(vec![-1]),
            vernon_core::AttribArray::Float(vec![0.; 6]),
        ] {
            let mut g = Geometry::make_box(DVec3::splat(10.));
            g.prim_attribs.set("region_id", attr);
            assert!(cook("surface.convert_prismatic", &[some(&g)], &[]).is_err());
        }
        let mut g = Geometry::make_box(DVec3::splat(10.));
        g.point_attribs
            .set("bad", vernon_core::AttribArray::Float(vec![0.]));
        assert!(cook("surface.convert_prismatic", &[some(&g)], &[]).is_err());
    }
    #[test]
    fn work_and_group_limits_apply_across_label_buckets() {
        let m = seg_mesh(&Geometry::make_box(DVec3::splat(10.))).mesh;
        let labels: Vec<_> = (0..m.tris.len()).map(|i| (i / 2) as i64).collect();
        assert!(recognize_budget(&m, Some(&labels), 0.05, 600)
            .err()
            .unwrap()
            .contains("budget"));
        let mut large = m.clone();
        large.tris = vec![m.tris[0]; MAX_TRIANGLES + 1];
        assert!(recognize(&large, None, 0.05)
            .err()
            .unwrap()
            .contains("at most"));
        large.tris = vec![m.tris[0]; 513];
        let labels: Vec<_> = (0..513).map(|i| -(i as i64) - 1).collect();
        assert!(recognize(&large, Some(&labels), 0.05)
            .err()
            .unwrap()
            .contains("global 512"));
    }
    #[test]
    fn insufficient_curved_support_stays_faceted() {
        let g = Geometry::from_polygons(
            vec![DVec3::ZERO, DVec3::X * 3., DVec3::Y * 4., DVec3::Z * 5.],
            vec![vec![0, 2, 1], vec![0, 1, 3], vec![0, 3, 2], vec![1, 2, 3]],
        );
        let o = cook("surface.convert_prismatic", &[some(&g)], &[]).unwrap();
        assert_eq!(report_of(&o[1]).metrics["analytic_faces"].value, 0.);
        assert_eq!(geo_of(&o[2]).prims.len(), 4);
        let o = cook(
            "surface.convert_prismatic",
            &[some(&g)],
            &[("keep_unfit_as_facets", b(false))],
        )
        .unwrap();
        assert!(geo_of(&o[2]).prims.is_empty());
    }
    #[test]
    fn invalid_and_negative_labels_fallback() {
        let mut g = Geometry::make_box(DVec3::splat(10.));
        g.prim_attribs
            .set("region_id", vernon_core::AttribArray::Int(vec![-1; 6]));
        let a = cook("surface.convert_prismatic", &[some(&g)], &[]).unwrap();
        assert_eq!(report_of(&a[1]).metrics["faceted_triangles"].value, 12.);
        let a = cook(
            "surface.convert_prismatic",
            &[some(&g)],
            &[("keep_unfit_as_facets", b(false))],
        )
        .unwrap();
        assert!(geo_of(&a[2]).prims.is_empty());
        assert_eq!(report_of(&a[1]).metrics["watertight"].value, 0.);
        g.points[0].x = f64::NAN;
        assert!(cook("surface.convert_prismatic", &[some(&g)], &[]).is_err());
        let line = Geometry::from_polygons(
            vec![DVec3::ZERO, DVec3::X, DVec3::X * 2.],
            vec![vec![0, 1, 2]],
        );
        assert!(cook("surface.convert_prismatic", &[some(&line)], &[]).is_err());
        let m = seg_mesh(&Geometry::new());
        assert!(recognize(&m.mesh, None, 0.05).unwrap().is_empty());
    }
}

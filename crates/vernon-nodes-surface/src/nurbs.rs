//! B-spline / NURBS curves and surfaces, plus the JSON schemas that carry them
//! between nodes (`nurbs_patch_set`, `nurbs_curve_set`).
//!
//! Conventions: knot vectors are stored expanded (with multiplicities), curves
//! and surfaces produced by Vernon are clamped, surface control points are
//! stored `u`-major (`index = iu * nv + iv`).

use glam::DVec3;
use serde::{Deserialize, Serialize};
use vernon_core::{AttribArray, Geometry, Prim};

/// Knot span index containing `u` (NURBS-book algorithm A2.1).
pub fn find_span(n_ctrl: usize, p: usize, u: f64, knots: &[f64]) -> usize {
    let n = n_ctrl - 1;
    if u >= knots[n + 1] {
        return n;
    }
    if u <= knots[p] {
        return p;
    }
    let (mut lo, mut hi) = (p, n + 1);
    let mut mid = (lo + hi) / 2;
    while u < knots[mid] || u >= knots[mid + 1] {
        if u < knots[mid] {
            hi = mid;
        } else {
            lo = mid;
        }
        mid = (lo + hi) / 2;
    }
    mid
}

/// The `p + 1` non-zero basis functions at `u` (A2.2).
pub fn basis_funs(span: usize, u: f64, p: usize, knots: &[f64]) -> Vec<f64> {
    let mut n = vec![0.0; p + 1];
    let mut left = vec![0.0; p + 1];
    let mut right = vec![0.0; p + 1];
    n[0] = 1.0;
    for j in 1..=p {
        left[j] = u - knots[span + 1 - j];
        right[j] = knots[span + j] - u;
        let mut saved = 0.0;
        for r in 0..j {
            let temp = n[r] / (right[r + 1] + left[j - r]);
            n[r] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        n[j] = saved;
    }
    n
}

/// Basis functions and their derivatives up to order `nd <= p` (A2.3).
/// `result[k][j]` is the k-th derivative of basis function `span - p + j`.
pub fn ders_basis_funs(span: usize, u: f64, p: usize, nd: usize, knots: &[f64]) -> Vec<Vec<f64>> {
    let nd = nd.min(p);
    let mut ndu = vec![vec![0.0; p + 1]; p + 1];
    let mut left = vec![0.0; p + 1];
    let mut right = vec![0.0; p + 1];
    ndu[0][0] = 1.0;
    for j in 1..=p {
        left[j] = u - knots[span + 1 - j];
        right[j] = knots[span + j] - u;
        let mut saved = 0.0;
        for r in 0..j {
            ndu[j][r] = right[r + 1] + left[j - r];
            let temp = ndu[r][j - 1] / ndu[j][r];
            ndu[r][j] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        ndu[j][j] = saved;
    }
    let mut ders = vec![vec![0.0; p + 1]; nd + 1];
    for j in 0..=p {
        ders[0][j] = ndu[j][p];
    }
    let mut a = vec![vec![0.0; p + 1]; 2];
    for r in 0..=p {
        let (mut s1, mut s2) = (0usize, 1usize);
        a[0][0] = 1.0;
        for k in 1..=nd {
            let mut d = 0.0;
            let rk = r as isize - k as isize;
            let pk = p as isize - k as isize;
            if r >= k {
                a[s2][0] = a[s1][0] / ndu[(pk + 1) as usize][rk as usize];
                d = a[s2][0] * ndu[rk as usize][pk as usize];
            }
            let j1 = if rk >= -1 { 1 } else { (-rk) as usize };
            let j2 = if (r as isize - 1) <= pk { k - 1 } else { p - r };
            for j in j1..=j2 {
                let idx = (rk + j as isize) as usize;
                a[s2][j] = (a[s1][j] - a[s1][j - 1]) / ndu[(pk + 1) as usize][idx];
                d += a[s2][j] * ndu[idx][pk as usize];
            }
            if (r as isize) <= pk {
                a[s2][k] = -a[s1][k - 1] / ndu[(pk + 1) as usize][r];
                d += a[s2][k] * ndu[r][pk as usize];
            }
            ders[k][r] = d;
            std::mem::swap(&mut s1, &mut s2);
        }
    }
    let mut factor = p as f64;
    for k in 1..=nd {
        for j in 0..=p {
            ders[k][j] *= factor;
        }
        factor *= (p as isize - k as isize) as f64;
    }
    ders
}

/// Clamped uniform knot vector for `n` control points and degree `p`.
pub fn clamped_uniform_knots(n: usize, p: usize) -> Vec<f64> {
    let spans = n - p;
    let mut k = vec![0.0; p + 1];
    for i in 1..spans {
        k.push(i as f64 / spans as f64);
    }
    k.extend(std::iter::repeat_n(1.0, p + 1));
    k
}

/// Distinct knot values and their multiplicities (as STEP stores them).
pub fn compress_knots(knots: &[f64]) -> (Vec<f64>, Vec<usize>) {
    let mut vals: Vec<f64> = Vec::new();
    let mut mult: Vec<usize> = Vec::new();
    for &k in knots {
        match vals.last() {
            Some(&l) if (k - l).abs() <= 1e-12 * (1.0 + l.abs()) => *mult.last_mut().expect("non-empty") += 1,
            _ => {
                vals.push(k);
                mult.push(1);
            }
        }
    }
    (vals, mult)
}

/// Inverse of [`compress_knots`].
pub fn expand_knots(vals: &[f64], mult: &[usize]) -> Vec<f64> {
    let mut out = Vec::new();
    for (v, m) in vals.iter().zip(mult) {
        out.extend(std::iter::repeat_n(*v, *m));
    }
    out
}

fn check_knots(knots: &[f64], n_ctrl: usize, p: usize, what: &str) -> Result<(), String> {
    if n_ctrl < p + 1 {
        return Err(format!("{what}: {n_ctrl} control points is too few for degree {p}"));
    }
    if knots.len() != n_ctrl + p + 1 {
        return Err(format!("{what}: knot count {} != points {} + degree {} + 1", knots.len(), n_ctrl, p));
    }
    if knots.windows(2).any(|w| w[1] < w[0]) {
        return Err(format!("{what}: knots are not non-decreasing"));
    }
    if knots.iter().any(|k| !k.is_finite()) {
        return Err(format!("{what}: non-finite knot"));
    }
    let (vals, mult) = compress_knots(knots);
    if mult[0] != p + 1 || *mult.last().expect("non-empty") != p + 1 {
        return Err(format!("{what}: knot vector is not clamped (end multiplicity must be degree + 1)"));
    }
    if mult[1..mult.len() - 1].iter().any(|&m| m > p) {
        return Err(format!("{what}: interior knot multiplicity exceeds the degree"));
    }
    if vals.len() < 2 {
        return Err(format!("{what}: empty parameter domain"));
    }
    Ok(())
}

/// A (possibly rational) B-spline curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NurbsCurve {
    pub degree: usize,
    /// Expanded knot vector, `control_points.len() + degree + 1` values.
    pub knots: Vec<f64>,
    pub control_points: Vec<DVec3>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weights: Option<Vec<f64>>,
}

impl NurbsCurve {
    /// Clamped uniform curve over `points`.
    pub fn clamped_uniform(points: Vec<DVec3>, degree: usize) -> Self {
        let degree = degree.min(points.len().saturating_sub(1)).max(1);
        Self { degree, knots: clamped_uniform_knots(points.len(), degree), control_points: points, weights: None }
    }

    pub fn domain(&self) -> (f64, f64) {
        (self.knots[self.degree], self.knots[self.control_points.len()])
    }

    pub fn validate(&self) -> Result<(), String> {
        check_knots(&self.knots, self.control_points.len(), self.degree, "curve")?;
        if let Some(w) = &self.weights {
            if w.len() != self.control_points.len() || w.iter().any(|&x| x <= 0.0 || !x.is_finite()) {
                return Err("curve: weights must be positive and match the control points".into());
            }
        }
        if self.control_points.iter().any(|p| !p.is_finite()) {
            return Err("curve: non-finite control point".into());
        }
        Ok(())
    }

    pub fn point(&self, u: f64) -> DVec3 {
        let n = self.control_points.len();
        let u = u.clamp(self.domain().0, self.domain().1);
        let span = find_span(n, self.degree, u, &self.knots);
        let b = basis_funs(span, u, self.degree, &self.knots);
        let mut num = DVec3::ZERO;
        let mut den = 0.0;
        for (j, bj) in b.iter().enumerate() {
            let i = span - self.degree + j;
            let w = self.weights.as_ref().map_or(1.0, |w| w[i]);
            num += self.control_points[i] * (bj * w);
            den += bj * w;
        }
        num / den
    }

    /// First derivative (analytic for non-rational curves, central difference
    /// for rational ones).
    pub fn derivative(&self, u: f64) -> DVec3 {
        if self.weights.is_some() || self.degree < 1 {
            let (a, b) = self.domain();
            let h = (b - a) * 1e-6;
            let (u0, u1) = ((u - h).max(a), (u + h).min(b));
            return (self.point(u1) - self.point(u0)) / (u1 - u0);
        }
        let n = self.control_points.len();
        let u = u.clamp(self.domain().0, self.domain().1);
        let span = find_span(n, self.degree, u, &self.knots);
        let d = ders_basis_funs(span, u, self.degree, 1, &self.knots);
        let mut s = DVec3::ZERO;
        for j in 0..=self.degree {
            s += self.control_points[span - self.degree + j] * d[1][j];
        }
        s
    }

    pub fn second_derivative(&self, u: f64) -> DVec3 {
        if self.weights.is_some() || self.degree < 2 {
            let (a, b) = self.domain();
            let h = (b - a) * 1e-4;
            let (u0, u1) = ((u - h).max(a), (u + h).min(b));
            return (self.derivative(u1) - self.derivative(u0)) / (u1 - u0);
        }
        let n = self.control_points.len();
        let u = u.clamp(self.domain().0, self.domain().1);
        let span = find_span(n, self.degree, u, &self.knots);
        let d = ders_basis_funs(span, u, self.degree, 2, &self.knots);
        let mut s = DVec3::ZERO;
        for j in 0..=self.degree {
            s += self.control_points[span - self.degree + j] * d[2][j];
        }
        s
    }

    /// `count` points evenly spaced in parameter (including both ends).
    pub fn sample(&self, count: usize) -> Vec<DVec3> {
        let (a, b) = self.domain();
        let count = count.max(2);
        (0..count).map(|i| self.point(a + (b - a) * i as f64 / (count - 1) as f64)).collect()
    }

    /// Samples per knot span (denser than `sample` for the same total).
    pub fn sample_spans(&self, per_span: usize) -> Vec<(f64, DVec3)> {
        let (vals, _) = compress_knots(&self.knots);
        let mut out = Vec::new();
        for w in vals.windows(2) {
            for k in 0..per_span {
                let u = w[0] + (w[1] - w[0]) * k as f64 / per_span as f64;
                out.push((u, self.point(u)));
            }
        }
        let end = self.domain().1;
        out.push((end, self.point(end)));
        out
    }

    pub fn length(&self) -> f64 {
        let pts = self.sample_spans(8);
        pts.windows(2).map(|w| (w[1].1 - w[0].1).length()).sum()
    }

    pub fn reversed(&self) -> NurbsCurve {
        let (a, b) = self.domain();
        let knots: Vec<f64> = self.knots.iter().rev().map(|k| a + b - k).collect();
        NurbsCurve {
            degree: self.degree,
            knots,
            control_points: self.control_points.iter().rev().copied().collect(),
            weights: self.weights.as_ref().map(|w| w.iter().rev().copied().collect()),
        }
    }

    /// Foot point of `q`: (parameter, point, distance).
    pub fn project(&self, q: DVec3) -> (f64, DVec3, f64) {
        let (a, b) = self.domain();
        // coarse search over every span
        let per = (self.degree * 4).max(8);
        let mut best = (a, f64::INFINITY);
        for (u, p) in self.sample_spans(per) {
            let d = (p - q).length_squared();
            if d < best.1 {
                best = (u, d);
            }
        }
        let mut u = best.0;
        for _ in 0..30 {
            let c = self.point(u);
            let d1 = self.derivative(u);
            let d2 = self.second_derivative(u);
            let f = (c - q).dot(d1);
            let fp = d1.dot(d1) + (c - q).dot(d2);
            if fp.abs() < 1e-300 {
                break;
            }
            let step = (f / fp).clamp(-(b - a) * 0.25, (b - a) * 0.25);
            let un = (u - step).clamp(a, b);
            if (un - u).abs() < 1e-14 * (b - a) {
                u = un;
                break;
            }
            u = un;
        }
        let p = self.point(u);
        // never worse than the coarse hit
        let coarse = self.point(best.0);
        if (coarse - q).length_squared() < (p - q).length_squared() {
            return (best.0, coarse, (coarse - q).length());
        }
        (u, p, (p - q).length())
    }

    /// Local Newton foot-point search starting at `u0` (cheap; may land in a
    /// local minimum, unlike [`NurbsCurve::project`]).
    pub fn project_from(&self, q: DVec3, u0: f64) -> (f64, f64) {
        let (a, b) = self.domain();
        let mut u = u0.clamp(a, b);
        for _ in 0..12 {
            let c = self.point(u);
            let d1 = self.derivative(u);
            let d2 = self.second_derivative(u);
            let f = (c - q).dot(d1);
            let fp = d1.dot(d1) + (c - q).dot(d2);
            let step = if fp > 1e-300 { f / fp } else { f / d1.dot(d1).max(1e-300) };
            let un = (u - step.clamp(-(b - a) * 0.1, (b - a) * 0.1)).clamp(a, b);
            if (un - u).abs() < 1e-12 * (b - a) {
                u = un;
                break;
            }
            u = un;
        }
        (u, (self.point(u) - q).length())
    }

    /// Insert knot `u` `times` times (Boehm). Non-rational curves only.
    pub fn insert_knot(&self, u: f64, times: usize) -> NurbsCurve {
        let mut knots = self.knots.clone();
        let mut pts = self.control_points.clone();
        let p = self.degree;
        for _ in 0..times {
            let n = pts.len();
            let span = find_span(n, p, u, &knots);
            let mut new_pts = Vec::with_capacity(n + 1);
            for i in 0..=n {
                if i <= span - p {
                    new_pts.push(pts[i]);
                } else if i > span {
                    new_pts.push(pts[i - 1]);
                } else {
                    let a = (u - knots[i]) / (knots[i + p] - knots[i]);
                    new_pts.push(pts[i - 1] * (1.0 - a) + pts[i] * a);
                }
            }
            knots.insert(span + 1, u);
            pts = new_pts;
        }
        NurbsCurve { degree: p, knots, control_points: pts, weights: None }
    }

    pub fn transformed(&self, f: impl Fn(DVec3) -> DVec3) -> NurbsCurve {
        NurbsCurve {
            degree: self.degree,
            knots: self.knots.clone(),
            control_points: self.control_points.iter().map(|&p| f(p)).collect(),
            weights: self.weights.clone(),
        }
    }
}

/// Rational quadratic NURBS arc of a circle (angles in radians, `a1 > a0`).
pub fn arc_nurbs(center: DVec3, x_axis: DVec3, y_axis: DVec3, radius: f64, a0: f64, a1: f64) -> NurbsCurve {
    let theta = a1 - a0;
    let narcs = ((theta / std::f64::consts::FRAC_PI_2).ceil() as usize).clamp(1, 16);
    let dtheta = theta / narcs as f64;
    let w1 = (dtheta / 2.0).cos();
    let at = |a: f64| center + (x_axis * a.cos() + y_axis * a.sin()) * radius;
    let tan_at = |a: f64| -x_axis * a.sin() + y_axis * a.cos();
    let mut pts = vec![at(a0)];
    let mut weights = vec![1.0];
    for k in 0..narcs {
        let ang0 = a0 + dtheta * k as f64;
        let ang1 = ang0 + dtheta;
        pts.push(at(ang0) + tan_at(ang0) * (radius * (dtheta / 2.0).tan()));
        weights.push(w1);
        pts.push(at(ang1));
        weights.push(1.0);
    }
    let mut knots = vec![0.0; 3];
    for k in 1..narcs {
        let v = k as f64 / narcs as f64;
        knots.push(v);
        knots.push(v);
    }
    knots.extend([1.0, 1.0, 1.0]);
    NurbsCurve { degree: 2, knots, control_points: pts, weights: Some(weights) }
}

/// A tensor-product B-spline surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NurbsSurface {
    pub degree_u: usize,
    pub degree_v: usize,
    pub knots_u: Vec<f64>,
    pub knots_v: Vec<f64>,
    pub nu: usize,
    pub nv: usize,
    /// `u`-major control net: index `iu * nv + iv`.
    pub control_points: Vec<DVec3>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weights: Option<Vec<f64>>,
}

/// Quad tessellation of one surface.
pub struct Tessellation {
    pub points: Vec<DVec3>,
    pub quads: Vec<[u32; 4]>,
    pub nu: usize,
    pub nv: usize,
}

impl NurbsSurface {
    pub fn new(
        degree_u: usize,
        degree_v: usize,
        knots_u: Vec<f64>,
        knots_v: Vec<f64>,
        nu: usize,
        nv: usize,
        control_points: Vec<DVec3>,
    ) -> Self {
        Self { degree_u, degree_v, knots_u, knots_v, nu, nv, control_points, weights: None }
    }

    /// Clamped uniform surface over an `nu x nv` net.
    pub fn clamped_uniform(degree_u: usize, degree_v: usize, nu: usize, nv: usize, cps: Vec<DVec3>) -> Self {
        Self::new(degree_u, degree_v, clamped_uniform_knots(nu, degree_u), clamped_uniform_knots(nv, degree_v), nu, nv, cps)
    }

    pub fn cp(&self, iu: usize, iv: usize) -> DVec3 {
        self.control_points[iu * self.nv + iv]
    }

    pub fn domain_u(&self) -> (f64, f64) {
        (self.knots_u[self.degree_u], self.knots_u[self.nu])
    }

    pub fn domain_v(&self) -> (f64, f64) {
        (self.knots_v[self.degree_v], self.knots_v[self.nv])
    }

    pub fn validate(&self) -> Result<(), String> {
        check_knots(&self.knots_u, self.nu, self.degree_u, "surface u")?;
        check_knots(&self.knots_v, self.nv, self.degree_v, "surface v")?;
        if self.control_points.len() != self.nu * self.nv {
            return Err(format!("surface: {} control points, expected {}", self.control_points.len(), self.nu * self.nv));
        }
        if let Some(w) = &self.weights {
            if w.len() != self.control_points.len() || w.iter().any(|&x| x <= 0.0 || !x.is_finite()) {
                return Err("surface: weights must be positive and match the control points".into());
            }
        }
        if self.control_points.iter().any(|p| !p.is_finite()) {
            return Err("surface: non-finite control point".into());
        }
        Ok(())
    }

    pub fn point(&self, u: f64, v: f64) -> DVec3 {
        let (ua, ub) = self.domain_u();
        let (va, vb) = self.domain_v();
        let (u, v) = (u.clamp(ua, ub), v.clamp(va, vb));
        let su = find_span(self.nu, self.degree_u, u, &self.knots_u);
        let sv = find_span(self.nv, self.degree_v, v, &self.knots_v);
        let bu = basis_funs(su, u, self.degree_u, &self.knots_u);
        let bv = basis_funs(sv, v, self.degree_v, &self.knots_v);
        let mut num = DVec3::ZERO;
        let mut den = 0.0;
        for (a, ba) in bu.iter().enumerate() {
            for (b, bb) in bv.iter().enumerate() {
                let idx = (su - self.degree_u + a) * self.nv + (sv - self.degree_v + b);
                let w = self.weights.as_ref().map_or(1.0, |w| w[idx]);
                num += self.control_points[idx] * (ba * bb * w);
                den += ba * bb * w;
            }
        }
        num / den
    }

    /// Point and first partial derivatives `(S, Su, Sv)`.
    pub fn derivs(&self, u: f64, v: f64) -> (DVec3, DVec3, DVec3) {
        let (ua, ub) = self.domain_u();
        let (va, vb) = self.domain_v();
        let (u, v) = (u.clamp(ua, ub), v.clamp(va, vb));
        if self.weights.is_some() {
            let (hu, hv) = ((ub - ua) * 1e-6, (vb - va) * 1e-6);
            let (u0, u1) = ((u - hu).max(ua), (u + hu).min(ub));
            let (v0, v1) = ((v - hv).max(va), (v + hv).min(vb));
            return (
                self.point(u, v),
                (self.point(u1, v) - self.point(u0, v)) / (u1 - u0),
                (self.point(u, v1) - self.point(u, v0)) / (v1 - v0),
            );
        }
        let su = find_span(self.nu, self.degree_u, u, &self.knots_u);
        let sv = find_span(self.nv, self.degree_v, v, &self.knots_v);
        let du = ders_basis_funs(su, u, self.degree_u, 1, &self.knots_u);
        let dv = ders_basis_funs(sv, v, self.degree_v, 1, &self.knots_v);
        let (mut s, mut s_u, mut s_v) = (DVec3::ZERO, DVec3::ZERO, DVec3::ZERO);
        for a in 0..=self.degree_u {
            for b in 0..=self.degree_v {
                let c = self.cp(su - self.degree_u + a, sv - self.degree_v + b);
                s += c * (du[0][a] * dv[0][b]);
                s_u += c * (du[1][a] * dv[0][b]);
                s_v += c * (du[0][a] * dv[1][b]);
            }
        }
        (s, s_u, s_v)
    }

    pub fn normal(&self, u: f64, v: f64) -> DVec3 {
        let (_, su, sv) = self.derivs(u, v);
        let n = su.cross(sv);
        if n.length_squared() > 1e-24 {
            return n.normalize();
        }
        // degenerate corner: nudge inward
        let (ua, ub) = self.domain_u();
        let (va, vb) = self.domain_v();
        let (eu, ev) = ((ub - ua) * 1e-4, (vb - va) * 1e-4);
        let uu = if u + eu <= ub { u + eu } else { u - eu };
        let vv = if v + ev <= vb { v + ev } else { v - ev };
        let (_, su, sv) = self.derivs(uu, vv);
        su.cross(sv).normalize_or_zero()
    }

    /// Gauss-Newton foot-point refinement from a starting `(u, v)`.
    pub fn foot_point(&self, q: DVec3, u0: f64, v0: f64, iters: usize) -> (f64, f64, f64) {
        let (ua, ub) = self.domain_u();
        let (va, vb) = self.domain_v();
        let (mut u, mut v) = (u0.clamp(ua, ub), v0.clamp(va, vb));
        for _ in 0..iters {
            let (s, su, sv) = self.derivs(u, v);
            let r = s - q;
            let (a, b, c) = (su.dot(su), su.dot(sv), sv.dot(sv));
            let (g1, g2) = (su.dot(r), sv.dot(r));
            let det = a * c - b * b;
            if det.abs() < 1e-18 * (a * c).abs().max(1e-300) {
                break;
            }
            let du = (c * g1 - b * g2) / det;
            let dv = (a * g2 - b * g1) / det;
            let un = (u - du).clamp(ua, ub);
            let vn = (v - dv).clamp(va, vb);
            let moved = (un - u).abs() / (ub - ua) + (vn - v).abs() / (vb - va);
            u = un;
            v = vn;
            if moved < 1e-13 {
                break;
            }
        }
        (u, v, (self.point(u, v) - q).length())
    }

    /// Evaluate on a rectangular parameter grid (`us` x `vs`, u-major).
    pub fn eval_grid(&self, us: &[f64], vs: &[f64]) -> Vec<DVec3> {
        let prep = |vals: &[f64], n: usize, p: usize, k: &[f64], dom: (f64, f64)| -> Vec<(usize, Vec<f64>)> {
            vals.iter()
                .map(|&t| {
                    let t = t.clamp(dom.0, dom.1);
                    let s = find_span(n, p, t, k);
                    (s, basis_funs(s, t, p, k))
                })
                .collect()
        };
        let pu = prep(us, self.nu, self.degree_u, &self.knots_u, self.domain_u());
        let pv = prep(vs, self.nv, self.degree_v, &self.knots_v, self.domain_v());
        let mut out = Vec::with_capacity(us.len() * vs.len());
        for (su, bu) in &pu {
            for (sv, bv) in &pv {
                let mut num = DVec3::ZERO;
                let mut den = 0.0;
                for (a, ba) in bu.iter().enumerate() {
                    for (b, bb) in bv.iter().enumerate() {
                        let idx = (su - self.degree_u + a) * self.nv + (sv - self.degree_v + b);
                        let w = self.weights.as_ref().map_or(1.0, |w| w[idx]);
                        num += self.control_points[idx] * (ba * bb * w);
                        den += ba * bb * w;
                    }
                }
                out.push(num / den);
            }
        }
        out
    }

    /// Boundary curve: 0 = `v` min (along u), 1 = `u` max (along v),
    /// 2 = `v` max (along u), 3 = `u` min (along v). Valid for clamped surfaces.
    pub fn boundary_curve(&self, side: usize) -> NurbsCurve {
        let (pts, deg, knots, w): (Vec<DVec3>, usize, &Vec<f64>, Option<Vec<f64>>) = match side % 4 {
            0 => self.row_u(0),
            1 => self.row_v(self.nu - 1),
            2 => self.row_u(self.nv - 1),
            _ => self.row_v(0),
        };
        NurbsCurve { degree: deg, knots: knots.clone(), control_points: pts, weights: w }
    }

    fn row_u(&self, iv: usize) -> (Vec<DVec3>, usize, &Vec<f64>, Option<Vec<f64>>) {
        let pts = (0..self.nu).map(|i| self.cp(i, iv)).collect();
        let w = self.weights.as_ref().map(|w| (0..self.nu).map(|i| w[i * self.nv + iv]).collect());
        (pts, self.degree_u, &self.knots_u, w)
    }

    fn row_v(&self, iu: usize) -> (Vec<DVec3>, usize, &Vec<f64>, Option<Vec<f64>>) {
        let pts = (0..self.nv).map(|j| self.cp(iu, j)).collect();
        let w = self.weights.as_ref().map(|w| (0..self.nv).map(|j| w[iu * self.nv + j]).collect());
        (pts, self.degree_v, &self.knots_v, w)
    }

    /// Reflect across the plane (`origin`, unit `normal`), reversing the u
    /// direction so the surface normal keeps pointing to the same material side.
    pub fn mirrored(&self, origin: DVec3, normal: DVec3) -> NurbsSurface {
        let n = normal.normalize_or_zero();
        let refl = |p: DVec3| p - n * (2.0 * (p - origin).dot(n));
        let (a, b) = self.domain_u();
        let mut cps = Vec::with_capacity(self.control_points.len());
        for i in 0..self.nu {
            for j in 0..self.nv {
                cps.push(refl(self.cp(self.nu - 1 - i, j)));
            }
        }
        let weights = self.weights.as_ref().map(|w| {
            let mut out = Vec::with_capacity(w.len());
            for i in 0..self.nu {
                for j in 0..self.nv {
                    out.push(w[(self.nu - 1 - i) * self.nv + j]);
                }
            }
            out
        });
        NurbsSurface {
            degree_u: self.degree_u,
            degree_v: self.degree_v,
            knots_u: self.knots_u.iter().rev().map(|k| a + b - k).collect(),
            knots_v: self.knots_v.clone(),
            nu: self.nu,
            nv: self.nv,
            control_points: cps,
            weights,
        }
    }

    pub fn aabb(&self) -> (DVec3, DVec3) {
        let mut lo = DVec3::splat(f64::INFINITY);
        let mut hi = DVec3::splat(f64::NEG_INFINITY);
        for p in &self.control_points {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        (lo, hi)
    }

    /// Adaptive quad tessellation: the grid doubles until the surface point at
    /// every cell centre is within `tol` of the bilinear cell interpolation.
    pub fn tessellate(&self, tol: f64) -> Tessellation {
        let (ua, ub) = self.domain_u();
        let (va, vb) = self.domain_v();
        let spans_u = compress_knots(&self.knots_u).0.len() - 1;
        let spans_v = compress_knots(&self.knots_v).0.len() - 1;
        let mut cu = (spans_u * 2).max(2);
        let mut cv = (spans_v * 2).max(2);
        loop {
            let us: Vec<f64> = (0..=cu).map(|i| ua + (ub - ua) * i as f64 / cu as f64).collect();
            let vs: Vec<f64> = (0..=cv).map(|j| va + (vb - va) * j as f64 / cv as f64).collect();
            let pts = self.eval_grid(&us, &vs);
            let ncols = cv + 1;
            let mut worst_u: f64 = 0.0;
            let mut worst_v: f64 = 0.0;
            // centre deviation, attributed to the direction with the larger edge-midpoint sag
            let cmid_u: Vec<f64> = (0..cu).map(|i| 0.5 * (us[i] + us[i + 1])).collect();
            let cmid_v: Vec<f64> = (0..cv).map(|j| 0.5 * (vs[j] + vs[j + 1])).collect();
            let centres = self.eval_grid(&cmid_u, &cmid_v);
            let eu = self.eval_grid(&cmid_u, &vs);
            let ev = self.eval_grid(&us, &cmid_v);
            for i in 0..cu {
                for j in 0..cv {
                    let a = pts[i * ncols + j];
                    let b = pts[(i + 1) * ncols + j];
                    let c = pts[(i + 1) * ncols + j + 1];
                    let d = pts[i * ncols + j + 1];
                    let bil = (a + b + c + d) * 0.25;
                    let dev = (centres[i * cv + j] - bil).length();
                    let sag_u = (eu[i * ncols + j] - (a + b) * 0.5).length().max((eu[i * ncols + j + 1] - (d + c) * 0.5).length());
                    let sag_v = (ev[i * cv + j] - (a + d) * 0.5).length().max((ev[(i + 1) * cv + j] - (b + c) * 0.5).length());
                    worst_u = worst_u.max(sag_u.max(dev));
                    worst_v = worst_v.max(sag_v.max(dev));
                }
            }
            let ok_u = worst_u <= tol || cu >= 256;
            let ok_v = worst_v <= tol || cv >= 256;
            if (ok_u && ok_v) || (cu + 1) * (cv + 1) > 1_000_000 {
                let mut quads = Vec::with_capacity(cu * cv);
                for i in 0..cu {
                    for j in 0..cv {
                        let a = (i * ncols + j) as u32;
                        quads.push([a, a + ncols as u32, a + ncols as u32 + 1, a + 1]);
                    }
                }
                return Tessellation { points: pts, quads, nu: cu + 1, nv: cv + 1 };
            }
            if !ok_u {
                cu *= 2;
            }
            if !ok_v {
                cv *= 2;
            }
        }
    }
}

/// One surface of a patch set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Patch {
    pub id: usize,
    #[serde(flatten)]
    pub surface: NurbsSurface,
    #[serde(default)]
    pub boundary_ids: Vec<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region_id: Option<i64>,
}

/// The `nurbs_patch_set` JSON schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatchSet {
    pub schema: String,
    pub version: u32,
    pub units: String,
    pub patches: Vec<Patch>,
    #[serde(default)]
    pub analytic_faces: Vec<serde_json::Value>,
    #[serde(default)]
    pub closed: bool,
    #[serde(default)]
    pub solid: bool,
}

pub const PATCH_SET_SCHEMA: &str = "nurbs_patch_set";
pub const CURVE_SET_SCHEMA: &str = "nurbs_curve_set";

impl PatchSet {
    pub fn new(surfaces: Vec<NurbsSurface>) -> Self {
        Self {
            schema: PATCH_SET_SCHEMA.into(),
            version: 1,
            units: "mm".into(),
            patches: surfaces
                .into_iter()
                .enumerate()
                .map(|(id, surface)| Patch { id, surface, boundary_ids: vec![], region_id: None })
                .collect(),
            analytic_faces: vec![],
            closed: false,
            solid: false,
        }
    }

    pub fn from_json(v: &serde_json::Value) -> Result<PatchSet, String> {
        let ps: PatchSet = serde_json::from_value(v.clone()).map_err(|e| format!("not a nurbs_patch_set: {e}"))?;
        if ps.schema != PATCH_SET_SCHEMA {
            return Err(format!("expected schema '{PATCH_SET_SCHEMA}', found '{}'", ps.schema));
        }
        for p in &ps.patches {
            p.surface.validate().map_err(|e| format!("patch {}: {e}", p.id))?;
        }
        Ok(ps)
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("patch set serialises")
    }

    /// Tessellate every patch into one quad `Geometry` (prim attribute `patch_id`).
    /// Patches are not welded together; each keeps its own points.
    pub fn tessellate(&self, tol: f64) -> Geometry {
        let mut g = Geometry::new();
        let mut ids = Vec::new();
        for p in &self.patches {
            let t = p.surface.tessellate(tol);
            let base = g.points.len() as u32;
            g.points.extend_from_slice(&t.points);
            for q in &t.quads {
                g.prims.push(Prim::polygon(q.iter().map(|&i| i + base).collect()));
                ids.push(p.id as i64);
            }
        }
        g.point_attribs.resize_all(g.points.len());
        g.prim_attribs.set("patch_id", AttribArray::Int(ids));
        g
    }

    pub fn control_point_count(&self) -> usize {
        self.patches.iter().map(|p| p.surface.control_points.len()).sum()
    }
}

/// One entry of a `nurbs_curve_set`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurveEntry {
    pub id: usize,
    #[serde(default)]
    pub section_id: i64,
    #[serde(default)]
    pub loop_id: i64,
    /// `line`, `arc`, `circle`, `ellipse`, `spline` or `closed_spline`.
    pub kind: String,
    pub closed: bool,
    #[serde(default)]
    pub max_dev_mm: f64,
    pub nurbs: NurbsCurve,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// The `nurbs_curve_set` JSON schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurveSet {
    pub schema: String,
    pub version: u32,
    pub units: String,
    pub curves: Vec<CurveEntry>,
}

impl CurveSet {
    pub fn new(curves: Vec<CurveEntry>) -> Self {
        Self { schema: CURVE_SET_SCHEMA.into(), version: 1, units: "mm".into(), curves }
    }

    pub fn from_json(v: &serde_json::Value) -> Result<CurveSet, String> {
        let cs: CurveSet = serde_json::from_value(v.clone()).map_err(|e| format!("not a nurbs_curve_set: {e}"))?;
        if cs.schema != CURVE_SET_SCHEMA {
            return Err(format!("expected schema '{CURVE_SET_SCHEMA}', found '{}'", cs.schema));
        }
        for c in &cs.curves {
            c.nurbs.validate().map_err(|e| format!("curve {}: {e}", c.id))?;
        }
        Ok(cs)
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("curve set serialises")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cubic() -> NurbsCurve {
        NurbsCurve::clamped_uniform(
            vec![
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(1.0, 2.0, 0.0),
                DVec3::new(3.0, 3.0, 1.0),
                DVec3::new(5.0, 1.0, 0.0),
                DVec3::new(6.0, 0.0, 2.0),
                DVec3::new(8.0, 2.0, 1.0),
            ],
            3,
        )
    }

    #[test]
    fn wrong_knot_count_rejected() {
        let mut c = cubic();
        c.knots.pop();
        assert!(c.validate().is_err());
        let mut c = cubic();
        c.knots[0] = 0.1;
        assert!(c.validate().is_err(), "unclamped start");
        assert!(cubic().validate().is_ok());
    }

    #[test]
    fn expand_compress_roundtrip() {
        let k = cubic().knots;
        let (v, m) = compress_knots(&k);
        assert_eq!(expand_knots(&v, &m), k);
        assert_eq!(m[0], 4);
    }

    #[test]
    fn partition_of_unity_and_endpoints() {
        let c = cubic();
        for i in 0..=40 {
            let u = i as f64 / 40.0;
            let span = find_span(c.control_points.len(), 3, u, &c.knots);
            let s: f64 = basis_funs(span, u, 3, &c.knots).iter().sum();
            assert!((s - 1.0).abs() < 1e-12);
        }
        assert!((c.point(0.0) - c.control_points[0]).length() < 1e-12);
        assert!((c.point(1.0) - *c.control_points.last().unwrap()).length() < 1e-12);
    }

    #[test]
    fn derivative_matches_finite_difference() {
        let c = cubic();
        for &u in &[0.1, 0.37, 0.8] {
            let h = 1e-6;
            let fd = (c.point(u + h) - c.point(u - h)) / (2.0 * h);
            assert!((c.derivative(u) - fd).length() < 1e-5);
            let fd2 = (c.derivative(u + h) - c.derivative(u - h)) / (2.0 * h);
            assert!((c.second_derivative(u) - fd2).length() < 1e-4);
        }
    }

    #[test]
    fn knot_insertion_keeps_shape() {
        let c = cubic();
        let d = c.insert_knot(0.45, 2);
        assert_eq!(d.control_points.len(), c.control_points.len() + 2);
        d.validate().unwrap();
        for i in 0..=30 {
            let u = i as f64 / 30.0;
            assert!((c.point(u) - d.point(u)).length() < 1e-12);
        }
    }

    #[test]
    fn projection_finds_foot_point() {
        let c = cubic();
        let on = c.point(0.6);
        let q = on + c.derivative(0.6).cross(DVec3::Z).normalize() * 0.3;
        let (u, p, d) = c.project(q);
        assert!((u - 0.6).abs() < 1e-6, "u={u}");
        assert!((p - on).length() < 1e-5);
        assert!((d - 0.3).abs() < 1e-5);
    }

    #[test]
    fn circle_arc_is_exact() {
        let a = arc_nurbs(DVec3::new(1.0, 2.0, 3.0), DVec3::X, DVec3::Y, 25.0, 0.0, std::f64::consts::TAU);
        a.validate().unwrap();
        for i in 0..=100 {
            let u = i as f64 / 100.0;
            let r = (a.point(u) - DVec3::new(1.0, 2.0, 3.0)).length();
            assert!((r - 25.0).abs() < 1e-9);
        }
    }

    fn bicubic_sheet() -> NurbsSurface {
        let (nu, nv) = (5, 4);
        let mut cps = Vec::new();
        for i in 0..nu {
            for j in 0..nv {
                cps.push(DVec3::new(i as f64 * 10.0, j as f64 * 7.0, ((i * j) as f64).sin() * 3.0));
            }
        }
        NurbsSurface::clamped_uniform(3, 3, nu, nv, cps)
    }

    #[test]
    fn surface_derivatives_and_corners() {
        let s = bicubic_sheet();
        s.validate().unwrap();
        assert!((s.point(0.0, 0.0) - s.cp(0, 0)).length() < 1e-12);
        assert!((s.point(1.0, 1.0) - s.cp(4, 3)).length() < 1e-12);
        let (u, v) = (0.31, 0.64);
        let h = 1e-6;
        let (_, su, sv) = s.derivs(u, v);
        let fu = (s.point(u + h, v) - s.point(u - h, v)) / (2.0 * h);
        let fv = (s.point(u, v + h) - s.point(u, v - h)) / (2.0 * h);
        assert!((su - fu).length() < 1e-5 && (sv - fv).length() < 1e-5);
    }

    #[test]
    fn boundary_curve_matches_surface_edge() {
        let s = bicubic_sheet();
        for side in 0..4 {
            let c = s.boundary_curve(side);
            for &t in &[0.0, 0.3, 0.77, 1.0] {
                let on_surface = match side {
                    0 => s.point(t, 0.0),
                    1 => s.point(1.0, t),
                    2 => s.point(t, 1.0),
                    _ => s.point(0.0, t),
                };
                assert!((c.point(t) - on_surface).length() < 1e-12);
            }
        }
    }

    #[test]
    fn mirror_reverses_and_reflects() {
        let s = bicubic_sheet();
        let m = s.mirrored(DVec3::ZERO, DVec3::X);
        m.validate().unwrap();
        let (u, v) = (0.2, 0.7);
        let p = s.point(u, v);
        let q = m.point(1.0 - u, v);
        assert!((q - DVec3::new(-p.x, p.y, p.z)).length() < 1e-12);
    }

    #[test]
    fn tessellation_is_within_tolerance() {
        let s = bicubic_sheet();
        let t = s.tessellate(0.01);
        assert!(t.quads.len() >= 4);
        // sample quad centres vs surface
        let mut worst: f64 = 0.0;
        for q in &t.quads {
            let c = (t.points[q[0] as usize] + t.points[q[1] as usize] + t.points[q[2] as usize] + t.points[q[3] as usize]) * 0.25;
            let (_, _, d) = s.foot_point(c, 0.5, 0.5, 30);
            worst = worst.max(d);
        }
        assert!(worst < 0.02, "worst {worst}");
    }

    #[test]
    fn patch_set_json_roundtrip() {
        let ps = PatchSet::new(vec![bicubic_sheet()]);
        let j = ps.to_json();
        let back = PatchSet::from_json(&j).unwrap();
        assert_eq!(back.patches.len(), 1);
        assert_eq!(back.patches[0].surface, ps.patches[0].surface);
        assert!(PatchSet::from_json(&serde_json::json!({"schema": "x"})).is_err());
    }
}

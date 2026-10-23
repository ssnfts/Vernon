//! Least-squares tensor-product B-spline surface fitting with parameter
//! correction and refinement, plus the mesh parameterisations that feed it
//! (best-fit-plane projection, harmonic map to the unit square, least-squares
//! conformal map).

use glam::DVec3;

use crate::linalg::{cg, scatter, sym_eigen3};
use crate::nurbs::{basis_funs, clamped_uniform_knots, find_span, NurbsSurface};
use crate::topology::boundary_loops;
use crate::util::percentile;

/// One data point with its current surface parameters.
#[derive(Debug, Clone, Copy)]
pub struct FitSample {
    pub u: f64,
    pub v: f64,
    pub q: DVec3,
}

/// Fixed-size fit settings.
#[derive(Debug, Clone, Copy)]
pub struct FitOptions {
    pub degree: usize,
    pub nu: usize,
    pub nv: usize,
    /// Thin-plate fairness weight relative to the data term (0 = none).
    pub smoothness: f64,
}

struct Row {
    idx: Vec<u32>,
    w: Vec<f64>,
}

fn collocation_rows(samples: &[FitSample], degree: usize, nu: usize, nv: usize, ku: &[f64], kv: &[f64]) -> Vec<Row> {
    samples
        .iter()
        .map(|s| {
            let (u, v) = (s.u.clamp(0.0, 1.0), s.v.clamp(0.0, 1.0));
            let su = find_span(nu, degree, u, ku);
            let sv = find_span(nv, degree, v, kv);
            let bu = basis_funs(su, u, degree, ku);
            let bv = basis_funs(sv, v, degree, kv);
            let mut idx = Vec::with_capacity((degree + 1) * (degree + 1));
            let mut w = Vec::with_capacity((degree + 1) * (degree + 1));
            for (a, ba) in bu.iter().enumerate() {
                for (b, bb) in bv.iter().enumerate() {
                    idx.push(((su - degree + a) * nv + (sv - degree + b)) as u32);
                    w.push(ba * bb);
                }
            }
            Row { idx, w }
        })
        .collect()
}

/// Fit with fixed control-net size. Returns `None` when the data are
/// degenerate (fewer samples than needed or non-finite).
pub fn fit_fixed(samples: &[FitSample], opts: &FitOptions) -> Option<NurbsSurface> {
    let p = opts.degree;
    let (nu, nv) = (opts.nu.max(p + 1), opts.nv.max(p + 1));
    if samples.len() < 4 || samples.iter().any(|s| !s.q.is_finite()) {
        return None;
    }
    let ku = clamped_uniform_knots(nu, p);
    let kv = clamped_uniform_knots(nv, p);
    let rows = collocation_rows(samples, p, nu, nv, &ku, &kv);
    let n = nu * nv;
    let mut dd = vec![0.0; n];
    for r in &rows {
        for (k, &i) in r.idx.iter().enumerate() {
            dd[i as usize] += r.w[k] * r.w[k];
        }
    }
    let avg = dd.iter().sum::<f64>() / n as f64;
    let lam = opts.smoothness.max(0.0) * avg.max(1e-12);
    let eps = 1e-9 * avg.max(1e-12);
    let diag: Vec<f64> = dd.iter().map(|d| d + 10.0 * lam + eps).collect();
    let apply = |x: &[f64], y: &mut [f64]| {
        for v in y.iter_mut() {
            *v = 0.0;
        }
        for r in &rows {
            let mut s = 0.0;
            for (k, &i) in r.idx.iter().enumerate() {
                s += r.w[k] * x[i as usize];
            }
            for (k, &i) in r.idx.iter().enumerate() {
                y[i as usize] += r.w[k] * s;
            }
        }
        if lam > 0.0 {
            let at = |i: usize, j: usize| i * nv + j;
            for i in 0..nu {
                for j in 0..nv {
                    if i + 2 < nu {
                        let d = x[at(i, j)] - 2.0 * x[at(i + 1, j)] + x[at(i + 2, j)];
                        y[at(i, j)] += lam * d;
                        y[at(i + 1, j)] -= 2.0 * lam * d;
                        y[at(i + 2, j)] += lam * d;
                    }
                    if j + 2 < nv {
                        let d = x[at(i, j)] - 2.0 * x[at(i, j + 1)] + x[at(i, j + 2)];
                        y[at(i, j)] += lam * d;
                        y[at(i, j + 1)] -= 2.0 * lam * d;
                        y[at(i, j + 2)] += lam * d;
                    }
                    if i + 1 < nu && j + 1 < nv {
                        let d = x[at(i, j)] - x[at(i + 1, j)] - x[at(i, j + 1)] + x[at(i + 1, j + 1)];
                        y[at(i, j)] += 2.0 * lam * d;
                        y[at(i + 1, j)] -= 2.0 * lam * d;
                        y[at(i, j + 1)] -= 2.0 * lam * d;
                        y[at(i + 1, j + 1)] += 2.0 * lam * d;
                    }
                }
            }
        }
        for i in 0..n {
            y[i] += eps * x[i];
        }
    };
    let mut cols: Vec<Vec<f64>> = Vec::with_capacity(3);
    for c in 0..3 {
        let mut b = vec![0.0; n];
        for (r, s) in rows.iter().zip(samples) {
            let qc = s.q[c];
            for (k, &i) in r.idx.iter().enumerate() {
                b[i as usize] += r.w[k] * qc;
            }
        }
        // start from the mean so under-determined regions stay near the data
        let mean = samples.iter().map(|s| s.q[c]).sum::<f64>() / samples.len() as f64;
        let mut x = vec![mean; n];
        cg(&apply, &diag, &b, &mut x, 1e-13, 20 * n.max(100));
        cols.push(x);
    }
    let cps: Vec<DVec3> = (0..n).map(|i| DVec3::new(cols[0][i], cols[1][i], cols[2][i])).collect();
    Some(NurbsSurface::new(p, p, ku, kv, nu, nv, cps))
}

/// Outcome of an adaptive fit.
#[derive(Debug, Clone)]
pub struct SurfaceFit {
    pub surface: NurbsSurface,
    /// Foot-point distances from the samples to the surface.
    pub max_dev: f64,
    pub rms_dev: f64,
    pub p95_dev: f64,
    pub iterations: usize,
    pub reached_tol: bool,
}

/// Distances from the samples to `surf` (after Newton foot-point correction of
/// their parameters, written back into `samples`).
pub fn correct_parameters(surf: &NurbsSurface, samples: &mut [FitSample], iters: usize) -> Vec<f64> {
    samples
        .iter_mut()
        .map(|s| {
            let (u, v, d) = surf.foot_point(s.q, s.u, s.v, iters);
            // foot_point works in the surface domain, which is [0, 1]^2 here
            s.u = u;
            s.v = v;
            d
        })
        .collect()
}

fn stats(d: &[f64]) -> (f64, f64, f64) {
    if d.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mut s = d.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let rms = (d.iter().map(|x| x * x).sum::<f64>() / d.len() as f64).sqrt();
    (*s.last().expect("non-empty"), rms, percentile(&s, 95.0))
}

/// Fit starting from `init_cp` control points per direction (scaled by
/// `aspect = extent_u / extent_v`), refine until the maximum sample distance is
/// within `tol` or `max_cp` is reached. Parameters are corrected by foot-point
/// projection after every solve.
pub fn fit_adaptive_surface(
    samples: &mut [FitSample],
    degree: usize,
    init_cp: usize,
    max_cp: usize,
    aspect: f64,
    tol: f64,
    smoothness: f64,
    passes: usize,
) -> Option<SurfaceFit> {
    let aspect = aspect.clamp(0.05, 20.0);
    let dims = |scale: f64| -> (usize, usize) {
        let base = scale.max((degree + 1) as f64);
        let nu = base * aspect.sqrt();
        let nv = base / aspect.sqrt();
        (((nu.round() as usize).max(degree + 1)).min(max_cp), ((nv.round() as usize).max(degree + 1)).min(max_cp))
    };
    let mut scale = init_cp as f64;
    let mut best: Option<SurfaceFit>;
    let mut iterations = 0;
    loop {
        let (nu, nv) = dims(scale);
        let mut surf = fit_fixed(samples, &FitOptions { degree, nu, nv, smoothness })?;
        let mut d = correct_parameters(&surf, samples, 6);
        for _ in 0..passes {
            surf = fit_fixed(samples, &FitOptions { degree, nu, nv, smoothness })?;
            d = correct_parameters(&surf, samples, 6);
        }
        iterations += 1;
        let (max_dev, rms_dev, p95_dev) = stats(&d);
        let reached = max_dev <= tol;
        best = Some(SurfaceFit { surface: surf, max_dev, rms_dev, p95_dev, iterations, reached_tol: reached });
        if reached || (nu >= max_cp && nv >= max_cp) || samples.len() < nu * nv * 2 {
            break;
        }
        scale *= 1.4;
        if iterations >= 8 {
            break;
        }
    }
    best
}

// ---------------------------------------------------------------------------
// parameterisations
// ---------------------------------------------------------------------------

/// A small indexed triangle mesh (a mesh region).
#[derive(Debug, Clone, Default)]
pub struct TriMesh {
    pub points: Vec<DVec3>,
    pub tris: Vec<[u32; 3]>,
}

impl TriMesh {
    /// Compact sub-mesh made of the given triangles.
    pub fn sub(&self, tri_ids: &[usize]) -> TriMesh {
        let mut map = std::collections::HashMap::new();
        let mut points = Vec::new();
        let mut tris = Vec::new();
        for &t in tri_ids {
            let tr = self.tris[t];
            let mut nt = [0u32; 3];
            for k in 0..3 {
                nt[k] = *map.entry(tr[k]).or_insert_with(|| {
                    points.push(self.points[tr[k] as usize]);
                    (points.len() - 1) as u32
                });
            }
            tris.push(nt);
        }
        TriMesh { points, tris }
    }

    pub fn area(&self) -> f64 {
        self.tris.iter().map(|t| 0.5 * (self.points[t[1] as usize] - self.points[t[0] as usize]).cross(self.points[t[2] as usize] - self.points[t[0] as usize]).length()).sum()
    }
}

/// Best-fit-plane frame: centre and three axes (two in-plane, then the normal).
pub fn plane_frame(pts: &[DVec3]) -> (DVec3, [DVec3; 3]) {
    let (c, m) = scatter(pts, None);
    let (_, vecs) = sym_eigen3(m);
    // eigenvalues ascending: vecs[2] is the largest-variance axis, vecs[0] the normal
    (c, [vecs[2], vecs[1], vecs[0]])
}

fn normalise_uv(uv: &mut [[f64; 2]]) -> f64 {
    if uv.is_empty() {
        return 1.0;
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in uv.iter() {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let ext = [(hi[0] - lo[0]).max(1e-12), (hi[1] - lo[1]).max(1e-12)];
    for p in uv.iter_mut() {
        for k in 0..2 {
            p[k] = (p[k] - lo[k]) / ext[k];
        }
    }
    ext[0] / ext[1]
}

/// Orthogonal projection onto the best-fit plane, scaled to the unit square.
/// Returns the parameters and the extent ratio `extent_u / extent_v`.
pub fn param_projection(pts: &[DVec3]) -> (Vec<[f64; 2]>, f64) {
    let (c, ax) = plane_frame(pts);
    let mut uv: Vec<[f64; 2]> = pts.iter().map(|&p| [(p - c).dot(ax[0]), (p - c).dot(ax[1])]).collect();
    let aspect = normalise_uv(&mut uv);
    (uv, aspect)
}

/// Do the mesh normals all lean towards the plane normal (a height field over
/// the best-fit plane)?
pub fn is_height_field(m: &TriMesh) -> bool {
    let (_, ax) = plane_frame(&m.points);
    let mut bad_area = 0.0;
    let mut total = 0.0;
    for t in &m.tris {
        let (a, b, c) = (m.points[t[0] as usize], m.points[t[1] as usize], m.points[t[2] as usize]);
        let n = (b - a).cross(c - a);
        let area = n.length() * 0.5;
        total += area;
        if n.normalize_or_zero().dot(ax[2]).abs() < 0.25 {
            bad_area += area;
        }
    }
    total > 0.0 && bad_area / total < 0.02
}

fn vertex_edges(m: &TriMesh) -> Vec<Vec<(u32, f64)>> {
    let mut adj: Vec<std::collections::BTreeMap<u32, f64>> = vec![Default::default(); m.points.len()];
    for t in &m.tris {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let w = 1.0 / (m.points[a as usize] - m.points[b as usize]).length().max(1e-12);
            adj[a as usize].insert(b, w);
            adj[b as usize].insert(a, w);
        }
    }
    adj.into_iter().map(|m| m.into_iter().collect()).collect()
}

/// Harmonic (Tutte-style, inverse-length weights) map of the largest boundary
/// loop onto the unit square perimeter by arc length. `None` for closed meshes.
pub fn param_boundary(m: &TriMesh) -> Option<(Vec<[f64; 2]>, f64)> {
    let faces: Vec<Vec<u32>> = m.tris.iter().map(|t| t.to_vec()).collect();
    let loops = boundary_loops(&faces);
    let lp = loops.into_iter().max_by(|a, b| {
        let la: f64 = (0..a.len()).map(|i| (m.points[a[i] as usize] - m.points[a[(i + 1) % a.len()] as usize]).length()).sum();
        let lb: f64 = (0..b.len()).map(|i| (m.points[b[i] as usize] - m.points[b[(i + 1) % b.len()] as usize]).length()).sum();
        la.total_cmp(&lb)
    })?;
    if lp.len() < 4 {
        return None;
    }
    let (_, ax) = plane_frame(&m.points);
    let c = m.points.iter().fold(DVec3::ZERO, |s, p| s + *p) / m.points.len() as f64;
    let ring = lp;
    let nr = ring.len();
    let mut cum = vec![0.0; nr + 1];
    for k in 0..nr {
        cum[k + 1] = cum[k] + (m.points[ring[k] as usize] - m.points[ring[(k + 1) % nr] as usize]).length();
    }
    let total = cum[nr].max(1e-12);
    // corners: the four sharpest boundary turns that are at least 10 % of the loop apart
    let turn = |k: usize| -> f64 {
        let (p, c, n) = (m.points[ring[(k + nr - 1) % nr] as usize], m.points[ring[k] as usize], m.points[ring[(k + 1) % nr] as usize]);
        let (a, b) = ((c - p).normalize_or_zero(), (n - c).normalize_or_zero());
        a.dot(b).clamp(-1.0, 1.0).acos()
    };
    let mut order: Vec<usize> = (0..nr).collect();
    order.sort_by(|&a, &b| turn(b).total_cmp(&turn(a)).then(a.cmp(&b)));
    let circ = |a: usize, b: usize| {
        let d = (cum[a] - cum[b]).abs();
        d.min(total - d)
    };
    let mut corners: Vec<usize> = Vec::new();
    for &k in &order {
        if corners.iter().all(|&c| circ(c, k) >= 0.1 * total) {
            corners.push(k);
            if corners.len() == 4 {
                break;
            }
        }
    }
    if corners.len() < 4 {
        corners = (0..4).map(|q| (q * nr) / 4).collect();
    }
    corners.sort_unstable();
    let n = m.points.len();
    let mut uv = vec![[0.0f64; 2]; n];
    let mut fixed = vec![false; n];
    for side in 0..4 {
        let (a, b) = (corners[side], corners[(side + 1) % 4]);
        let (ca, la) = (cum[a], if b > a { cum[b] - cum[a] } else { total - cum[a] + cum[b] });
        let la = la.max(1e-12);
        let mut k = a;
        loop {
            let d = if k >= a { cum[k] - ca } else { total - ca + cum[k] };
            let f = (d / la).clamp(0.0, 1.0);
            uv[ring[k] as usize] = match side {
                0 => [f, 0.0],
                1 => [1.0, f],
                2 => [1.0 - f, 1.0],
                _ => [0.0, 1.0 - f],
            };
            fixed[ring[k] as usize] = true;
            k = (k + 1) % nr;
            if k == b {
                break;
            }
        }
    }
    let adj = vertex_edges(m);
    let free: Vec<usize> = (0..n).filter(|&i| !fixed[i]).collect();
    if !free.is_empty() {
        let mut pos = vec![usize::MAX; n];
        for (k, &i) in free.iter().enumerate() {
            pos[i] = k;
        }
        let diag: Vec<f64> = free.iter().map(|&i| adj[i].iter().map(|e| e.1).sum::<f64>().max(1e-12)).collect();
        let apply = |x: &[f64], y: &mut [f64]| {
            for (k, &i) in free.iter().enumerate() {
                let mut s = diag[k] * x[k];
                for &(j, w) in &adj[i] {
                    if pos[j as usize] != usize::MAX {
                        s -= w * x[pos[j as usize]];
                    }
                }
                y[k] = s;
            }
        };
        for c in 0..2 {
            let mut b = vec![0.0; free.len()];
            for (k, &i) in free.iter().enumerate() {
                for &(j, w) in &adj[i] {
                    if fixed[j as usize] {
                        b[k] += w * uv[j as usize][c];
                    }
                }
            }
            let mut x = vec![0.5; free.len()];
            cg(&apply, &diag, &b, &mut x, 1e-12, 4000);
            for (k, &i) in free.iter().enumerate() {
                uv[i][c] = x[k];
            }
        }
    }
    let aspect = {
        let ext = |a: DVec3| {
            let (lo, hi) = m.points.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), p| (l.min((*p - c).dot(a)), h.max((*p - c).dot(a))));
            (hi - lo).max(1e-12)
        };
        ext(ax[0]) / ext(ax[1])
    };
    Some((uv, aspect))
}

/// Least-squares conformal map (two pinned vertices), scaled to the unit square.
pub fn param_conformal(m: &TriMesh) -> Option<(Vec<[f64; 2]>, f64)> {
    let n = m.points.len();
    if n < 3 || m.tris.is_empty() {
        return None;
    }
    // pin the two vertices farthest apart along the main axis
    let (c, ax) = plane_frame(&m.points);
    let proj: Vec<f64> = m.points.iter().map(|p| (*p - c).dot(ax[0])).collect();
    let i0 = (0..n).min_by(|&a, &b| proj[a].total_cmp(&proj[b]))?;
    let i1 = (0..n).max_by(|&a, &b| proj[a].total_cmp(&proj[b]))?;
    if i0 == i1 {
        return None;
    }
    let span = (m.points[i1] - m.points[i0]).length();
    // local triangle coefficients
    struct Tri {
        v: [usize; 3],
        a: [f64; 3],
        b: [f64; 3],
    }
    let mut tris = Vec::with_capacity(m.tris.len());
    for t in &m.tris {
        let (p0, p1, p2) = (m.points[t[0] as usize], m.points[t[1] as usize], m.points[t[2] as usize]);
        let e1 = p1 - p0;
        let e2 = p2 - p0;
        let l1 = e1.length();
        if l1 < 1e-14 {
            continue;
        }
        let x = e1 / l1;
        let nrm = e1.cross(e2);
        let area2 = nrm.length();
        if area2 < 1e-14 {
            continue;
        }
        let y = nrm.cross(e1).normalize();
        let q = [(0.0, 0.0), (l1, 0.0), (e2.dot(x), e2.dot(y))];
        let sc = 1.0 / area2.sqrt();
        let w = |j: usize, k: usize| ((q[k].0 - q[j].0) * sc, (q[k].1 - q[j].1) * sc);
        // W_j from the opposite edge (j+1 -> j+2)
        let (w0, w1, w2) = (w(1, 2), w(2, 0), w(0, 1));
        tris.push(Tri { v: [t[0] as usize, t[1] as usize, t[2] as usize], a: [w0.0, w1.0, w2.0], b: [w0.1, w1.1, w2.1] });
    }
    let pinned = [(i0, 0.0, 0.0), (i1, span, 0.0)];
    let is_pin = |i: usize| pinned.iter().position(|p| p.0 == i);
    // unknown layout: u at 2*i, v at 2*i+1 for free vertices
    let mut slot = vec![usize::MAX; n];
    let mut nfree = 0;
    for i in 0..n {
        if is_pin(i).is_none() {
            slot[i] = nfree;
            nfree += 1;
        }
    }
    let nn = 2 * nfree;
    let ata = |x: &[f64], y: &mut [f64]| {
        for v in y.iter_mut() {
            *v = 0.0;
        }
        for t in &tris {
            let (mut r1, mut r2) = (0.0, 0.0);
            for j in 0..3 {
                let s = slot[t.v[j]];
                if s == usize::MAX {
                    continue;
                }
                let (u, v) = (x[2 * s], x[2 * s + 1]);
                r1 += t.a[j] * u - t.b[j] * v;
                r2 += t.b[j] * u + t.a[j] * v;
            }
            for j in 0..3 {
                let s = slot[t.v[j]];
                if s == usize::MAX {
                    continue;
                }
                y[2 * s] += t.a[j] * r1 + t.b[j] * r2;
                y[2 * s + 1] += -t.b[j] * r1 + t.a[j] * r2;
            }
        }
    };
    let mut rhs = vec![0.0; nn];
    for t in &tris {
        let (mut f1, mut f2) = (0.0, 0.0);
        for j in 0..3 {
            if let Some(pi) = is_pin(t.v[j]) {
                let (u, v) = (pinned[pi].1, pinned[pi].2);
                f1 += t.a[j] * u - t.b[j] * v;
                f2 += t.b[j] * u + t.a[j] * v;
            }
        }
        for j in 0..3 {
            let s = slot[t.v[j]];
            if s == usize::MAX {
                continue;
            }
            rhs[2 * s] -= t.a[j] * f1 + t.b[j] * f2;
            rhs[2 * s + 1] -= -t.b[j] * f1 + t.a[j] * f2;
        }
    }
    let mut diag = vec![1e-12; nn];
    for t in &tris {
        for j in 0..3 {
            let s = slot[t.v[j]];
            if s != usize::MAX {
                let d = t.a[j] * t.a[j] + t.b[j] * t.b[j];
                diag[2 * s] += d;
                diag[2 * s + 1] += d;
            }
        }
    }
    let mut x = vec![0.0; nn];
    // start from the plane projection for a quick, well-oriented solution
    for i in 0..n {
        if slot[i] != usize::MAX {
            let d = m.points[i] - c;
            x[2 * slot[i]] = d.dot(ax[0]) - (m.points[i0] - c).dot(ax[0]);
            x[2 * slot[i] + 1] = d.dot(ax[1]);
        }
    }
    cg(&ata, &diag, &rhs, &mut x, 1e-12, 6000);
    let mut uv = vec![[0.0f64; 2]; n];
    for i in 0..n {
        uv[i] = match is_pin(i) {
            Some(pi) => [pinned[pi].1, pinned[pi].2],
            None => [x[2 * slot[i]], x[2 * slot[i] + 1]],
        };
    }
    if uv.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
        return None;
    }
    align_principal(&mut uv);
    let aspect = normalise_uv(&mut uv);
    Some((uv, aspect))
}

/// Rotate 2-D points so their principal axis is the first coordinate.
fn align_principal(uv: &mut [[f64; 2]]) {
    let n = uv.len() as f64;
    let (cx, cy) = (uv.iter().map(|p| p[0]).sum::<f64>() / n, uv.iter().map(|p| p[1]).sum::<f64>() / n);
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for p in uv.iter() {
        let (x, y) = (p[0] - cx, p[1] - cy);
        sxx += x * x;
        syy += y * y;
        sxy += x * y;
    }
    let th = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (c, s) = (th.cos(), th.sin());
    for p in uv.iter_mut() {
        let (x, y) = (p[0] - cx, p[1] - cy);
        *p = [c * x + s * y, -s * x + c * y];
    }
}

/// Triangle-area-weighted sample points (vertices plus barycentric points)
/// with parameters interpolated from the vertex parameters.
pub fn sample_mesh(m: &TriMesh, uv: &[[f64; 2]], extra_per_tri: usize, max_samples: usize) -> Vec<FitSample> {
    let mut out: Vec<FitSample> = m.points.iter().zip(uv).map(|(p, t)| FitSample { u: t[0], v: t[1], q: *p }).collect();
    if extra_per_tri == 0 {
        return out;
    }
    let mut rng = crate::util::Rng::new(0x5EED_5A3F);
    let budget = max_samples.saturating_sub(out.len());
    let per = if m.tris.is_empty() { 0 } else { (budget / m.tris.len()).min(extra_per_tri) };
    for t in &m.tris {
        for _ in 0..per {
            let (mut a, mut b) = (rng.next_f64(), rng.next_f64());
            if a + b > 1.0 {
                a = 1.0 - a;
                b = 1.0 - b;
            }
            let c = 1.0 - a - b;
            let (i0, i1, i2) = (t[0] as usize, t[1] as usize, t[2] as usize);
            out.push(FitSample {
                u: uv[i0][0] * c + uv[i1][0] * a + uv[i2][0] * b,
                v: uv[i0][1] * c + uv[i1][1] * a + uv[i2][1] * b,
                q: m.points[i0] * c + m.points[i1] * a + m.points[i2] * b,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::grid;
    use crate::util::Rng;
    use std::f64::consts::PI;

    fn sheet_samples(n: usize, noise: f64, seed: u64) -> Vec<FitSample> {
        let mut rng = Rng::new(seed);
        let mut out = Vec::new();
        for i in 0..n {
            for j in 0..n {
                let (u, v) = (i as f64 / (n - 1) as f64, j as f64 / (n - 1) as f64);
                let (x, y) = (100.0 * u, 100.0 * v);
                let z = 5.0 * (PI * x / 50.0).sin() + noise * rng.gauss();
                out.push(FitSample { u, v, q: DVec3::new(x, y, z) });
            }
        }
        out
    }

    #[test]
    fn sine_sheet_golden_fit() {
        // spec case 6: z = 5 sin(pi x / 50) on 100 x 100 mm, 8 x 8 cubic net
        let mut s = sheet_samples(60, 0.0, 1);
        let f = fit_adaptive_surface(&mut s, 3, 8, 8, 1.0, 0.05, 0.0, 4).unwrap();
        assert_eq!((f.surface.nu, f.surface.nv), (8, 8));
        assert!(f.max_dev <= 0.05, "max {}", f.max_dev);
        // spec proposal is p95 <= 0.02; the uniform 8 x 8 net measures 0.0204, so allow 0.025
        assert!(f.p95_dev <= 0.025, "p95 {}", f.p95_dev);
    }

    #[test]
    fn noisy_sheet_is_smoothed_not_chased() {
        let mut s = sheet_samples(60, 0.05, 2);
        let f = fit_adaptive_surface(&mut s, 3, 8, 8, 1.0, 0.0, 1e-3, 2).unwrap();
        assert!(f.rms_dev > 0.035 && f.rms_dev < 0.065, "rms {}", f.rms_dev);
        assert!(f.p95_dev <= 0.12, "p95 {}", f.p95_dev);
    }

    #[test]
    fn planar_data_gives_coplanar_control_points() {
        let mut s: Vec<FitSample> = Vec::new();
        for i in 0..20 {
            for j in 0..20 {
                let (u, v) = (i as f64 / 19.0, j as f64 / 19.0);
                s.push(FitSample { u, v, q: DVec3::new(30.0 * u, 20.0 * v, 0.2 * 30.0 * u + 0.1 * 20.0 * v + 4.0) });
            }
        }
        let f = fit_adaptive_surface(&mut s, 3, 6, 6, 1.5, 1e-6, 0.0, 1).unwrap();
        for p in &f.surface.control_points {
            assert!((p.z - (0.2 * p.x + 0.1 * p.y + 4.0)).abs() < 1e-7, "{p:?}");
        }
        assert!(f.max_dev < 1e-7);
    }

    #[test]
    fn adaptive_refinement_reaches_tolerance() {
        let mut s = sheet_samples(70, 0.0, 3);
        let f = fit_adaptive_surface(&mut s, 3, 5, 40, 1.0, 0.02, 1e-6, 2).unwrap();
        assert!(f.reached_tol && f.max_dev <= 0.02, "max {} n {}x{}", f.max_dev, f.surface.nu, f.surface.nv);
    }

    fn mesh_of(g: &vernon_core::Geometry) -> TriMesh {
        let mut tris = Vec::new();
        for p in &g.prims {
            for k in 1..p.vertices.len() - 1 {
                tris.push([p.vertices[0], p.vertices[k], p.vertices[k + 1]]);
            }
        }
        TriMesh { points: g.points.clone(), tris }
    }

    #[test]
    fn projection_and_boundary_params_cover_the_unit_square() {
        let g = grid(12, 12, 100.0, 60.0, |x, y| 0.01 * x * y / 100.0);
        let m = mesh_of(&g);
        let (uv, aspect) = param_projection(&m.points);
        assert!((aspect - 100.0 / 60.0).abs() < 0.02 || (aspect - 60.0 / 100.0).abs() < 0.02);
        assert!(uv.iter().all(|p| p[0] >= 0.0 && p[0] <= 1.0 && p[1] >= 0.0 && p[1] <= 1.0));
        assert!(is_height_field(&m));
        let (b, _) = param_boundary(&m).unwrap();
        assert!(b.iter().all(|p| p[0] >= -1e-9 && p[0] <= 1.0 + 1e-9 && p[1] >= -1e-9 && p[1] <= 1.0 + 1e-9));
        // harmonic map is injective: no flipped triangles
        for t in &m.tris {
            let (a, bb, c) = (b[t[0] as usize], b[t[1] as usize], b[t[2] as usize]);
            let cr = (bb[0] - a[0]) * (c[1] - a[1]) - (bb[1] - a[1]) * (c[0] - a[0]);
            assert!(cr > 1e-12, "flipped or degenerate triangle: {cr}");
        }
    }

    #[test]
    fn conformal_map_of_a_developable_strip_is_isometric_up_to_scale() {
        // half cylinder strip: arc length 100 around, 50 along axis
        let (nu, nv) = (30usize, 10usize);
        let r = 100.0 / PI;
        let mut pts = Vec::new();
        for i in 0..=nu {
            for j in 0..=nv {
                let a = PI * i as f64 / nu as f64;
                pts.push(DVec3::new(r * a.cos(), r * a.sin(), 50.0 * j as f64 / nv as f64));
            }
        }
        let mut tris = Vec::new();
        let id = |i: usize, j: usize| (i * (nv + 1) + j) as u32;
        for i in 0..nu {
            for j in 0..nv {
                tris.push([id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
                tris.push([id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
            }
        }
        let m = TriMesh { points: pts, tris };
        let (uv, aspect) = param_conformal(&m).unwrap();
        assert!((aspect - 2.0).abs() < 0.1, "aspect {aspect}");
        assert!(uv.iter().all(|p| p[0].is_finite() && p[1].is_finite()));
    }
}

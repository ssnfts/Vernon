//! Stretch-aware flattening constraints: a local/global solver whose local
//! step clamps the singular values of every triangle's 2x2 Jacobian, with
//! per-triangle rigid/floppy weights and a fold-free line search.
//!
//! Standalone functions over plain slices (`[[f64; 3]]` positions in mm,
//! `[[usize; 3]]` triangles, `[[f64; 2]]` flat coordinates in mm).
//!
//! Method, from the papers (equation numbers are not quoted):
//! * Liu, Zhang, Xu, Liu and Gortler, "A Local/Global Approach to Mesh
//!   Parameterization", SGP 2008: alternate between a *local* step that finds,
//!   per triangle, the target matrix `L_t` closest to the current Jacobian
//!   `J_t` (here: same rotation frame, singular values clamped) and a *global*
//!   step that minimises `sum_t w_t A_t |J_t - L_t|_F^2`, a sparse linear
//!   system with the cotangent Laplacian of the triangles' own isometric
//!   frames. The identity `A |M|_F^2 = 1/2 sum_e cot(theta_e) |M e|^2` for a
//!   linear map `M` on a triangle turns the energy into edge form.
//! * Smith and Schaefer, "Bijective Parameterization with Free Boundaries",
//!   SIGGRAPH 2015: pick the step length so no triangle reaches zero signed
//!   area. A triangle's signed area along the step is an exact quadratic in
//!   the step length, so the largest safe step is the smallest positive root;
//!   the step actually taken is a fraction of it. (The paper's additional
//!   distortion-energy backtracking is not implemented -- UNVERIFIED which
//!   details of the paper beyond the root-finding idea this matches.)
//!
//! Constraint modes act on the singular values `s1 >= s2 > 0` of `J_t` in the
//! local step by clamping, never by a penalty weight: `Rigid` sets both to 1
//! (isometry), `StretchOnly` raises values below 1 to 1 (the sheet may
//! stretch but not compress), `CompressOnly` lowers values above 1 to 1.
//! The one-sided modes are targets of the least-squares fit, which cannot
//! guarantee that every edge ends up on the right side; a final uniform
//! scale about the pinned vertex removes any remaining violation (this makes
//! the one-sided guarantee exact at the cost of a global scale) and is
//! reported in [`Output::final_scale`].
//!
//! Orientation: triangles must be counter-clockwise seen from the side the
//! normal `(v1-v0) x (v2-v0)` points to, and the start layout must have
//! positive signed area everywhere; the solver keeps it that way.

/// Which singular values the local step may keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Both singular values target 1 (length preserving).
    Rigid,
    /// Singular values below 1 are raised to 1; larger ones are kept.
    StretchOnly,
    /// Singular values above 1 are lowered to 1; smaller ones are kept.
    CompressOnly,
}

/// Solver settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    pub mode: Mode,
    pub max_iterations: usize,
    /// Stop when the largest vertex move of an iteration is below
    /// `tolerance * bounding box diagonal` of the layout.
    pub tolerance: f64,
    /// Fraction of the largest fold-free step actually taken, in (0, 1).
    pub step_fraction: f64,
    /// Vertex held at its start position (removes the translation freedom).
    pub pin: usize,
    /// Relative edge-length tolerance of the one-sided modes' final scale.
    pub edge_tolerance: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            mode: Mode::Rigid,
            max_iterations: 200,
            tolerance: 1e-10,
            step_fraction: 0.8,
            pin: 0,
            edge_tolerance: 1e-9,
        }
    }
}

/// Result of [`flatten_constrained`].
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub uv: Vec<[f64; 2]>,
    pub iterations: usize,
    /// Singular values `[s1, s2]` of every triangle's Jacobian in the result
    /// (`s1 >= s2`; zero-area triangles report `[1, 1]`).
    pub sigma: Vec<[f64; 2]>,
    /// Smallest and largest edge length ratio (flat / 3-D) over all edges.
    pub min_edge_ratio: f64,
    pub max_edge_ratio: f64,
    /// Uniform scale applied last by the one-sided modes (1 when none).
    pub final_scale: f64,
    /// Triangles with non-positive signed area (0 on success).
    pub flipped: usize,
}

/// Weights for rigid (1) and floppy (`floppy_weight`) triangles.
pub fn floppy_weights(n_tris: usize, floppy: &[usize], floppy_weight: f64) -> Vec<f64> {
    let mut w = vec![1.0; n_tris];
    for &t in floppy {
        if t < n_tris {
            w[t] = floppy_weight;
        }
    }
    w
}

// --- 2x2 helpers ---------------------------------------------------------------

type M2 = [[f64; 2]; 2];

fn rot(a: f64) -> M2 {
    let (s, c) = a.sin_cos();
    [[c, -s], [s, c]]
}

fn mul(a: &M2, b: &M2) -> M2 {
    let mut r = [[0.0; 2]; 2];
    for i in 0..2 {
        for j in 0..2 {
            r[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j];
        }
    }
    r
}

/// `J = Rot(phi) diag(s1, s2) Rot(theta)` with `s2` negative for a reflection
/// (the closed form for 2x2 matrices). Returns `(phi, theta, s1, s2)`.
fn svd2(j: &M2) -> (f64, f64, f64, f64) {
    let e = 0.5 * (j[0][0] + j[1][1]);
    let f = 0.5 * (j[0][0] - j[1][1]);
    let g = 0.5 * (j[1][0] + j[0][1]);
    let h = 0.5 * (j[1][0] - j[0][1]);
    let q = e.hypot(h);
    let r = f.hypot(g);
    let a1 = g.atan2(f);
    let a2 = h.atan2(e);
    (0.5 * (a2 + a1), 0.5 * (a2 - a1), q + r, q - r)
}

/// Singular values `[s1, s2]` (both non-negative, `s1 >= s2`) of a 2x2 matrix.
pub fn singular_values(j: &[[f64; 2]; 2]) -> [f64; 2] {
    let (_, _, s1, s2) = svd2(j);
    [s1.abs().max(s2.abs()), s1.abs().min(s2.abs())]
}

/// The singular values after the mode's clamp.
pub fn clamp_singular_values(s: [f64; 2], mode: Mode) -> [f64; 2] {
    let c = |v: f64| match mode {
        Mode::Rigid => 1.0,
        Mode::StretchOnly => v.max(1.0),
        Mode::CompressOnly => v.min(1.0),
    };
    [c(s[0]), c(s[1])]
}

// --- triangle frames -------------------------------------------------------------

/// A triangle's isometric 2-D frame: `x0 = (0,0)`, `x1 = (l, 0)`, `x2 = (p, q)`.
#[derive(Clone, Copy)]
struct Frame {
    l: f64,
    p: f64,
    q: f64,
}

fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn frame(pos: &[[f64; 3]], t: &[usize; 3]) -> Option<Frame> {
    let a = sub3(pos[t[1]], pos[t[0]]);
    let b = sub3(pos[t[2]], pos[t[0]]);
    let l = dot3(a, a).sqrt();
    let n = cross3(a, b);
    let area2 = dot3(n, n).sqrt();
    if l <= 0.0 || area2 <= 1e-12 * l * l.max(1.0) {
        return None;
    }
    Some(Frame { l, p: dot3(a, b) / l, q: area2 / l })
}

impl Frame {
    fn x(&self) -> [[f64; 2]; 3] {
        [[0.0, 0.0], [self.l, 0.0], [self.p, self.q]]
    }

    /// Jacobian of the map from this frame to the flat triangle `u`.
    fn jacobian(&self, u: [[f64; 2]; 3]) -> M2 {
        let d1 = [u[1][0] - u[0][0], u[1][1] - u[0][1]];
        let d2 = [u[2][0] - u[0][0], u[2][1] - u[0][1]];
        let (l, p, q) = (self.l, self.p, self.q);
        [
            [d1[0] / l, (-d1[0] * p + d2[0] * l) / (l * q)],
            [d1[1] / l, (-d1[1] * p + d2[1] * l) / (l * q)],
        ]
    }
}

/// Signed area times two of a flat triangle.
fn area2(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

// --- fold-free line search ---------------------------------------------------------

/// Largest `alpha > 0` such that no triangle of `uv + alpha * step` has
/// reached zero signed area; `f64::INFINITY` when none ever does. Triangles
/// that are not positively oriented at `alpha = 0` give 0.
pub fn max_fold_free_step(tris: &[[usize; 3]], uv: &[[f64; 2]], step: &[[f64; 2]]) -> f64 {
    let mut best = f64::INFINITY;
    for t in tris {
        let e1 = [uv[t[1]][0] - uv[t[0]][0], uv[t[1]][1] - uv[t[0]][1]];
        let e2 = [uv[t[2]][0] - uv[t[0]][0], uv[t[2]][1] - uv[t[0]][1]];
        let d1 = [step[t[1]][0] - step[t[0]][0], step[t[1]][1] - step[t[0]][1]];
        let d2 = [step[t[2]][0] - step[t[0]][0], step[t[2]][1] - step[t[0]][1]];
        let cr = |a: [f64; 2], b: [f64; 2]| a[0] * b[1] - a[1] * b[0];
        // 2A(alpha) = c0 + c1 alpha + c2 alpha^2
        let (c0, c1, c2) = (cr(e1, e2), cr(e1, d2) + cr(d1, e2), cr(d1, d2));
        if c0 <= 0.0 {
            return 0.0;
        }
        let root = smallest_positive_root(c0, c1, c2);
        best = best.min(root);
    }
    best
}

/// Smallest `alpha > 0` with `c0 + c1 alpha + c2 alpha^2 = 0`, given `c0 > 0`.
fn smallest_positive_root(c0: f64, c1: f64, c2: f64) -> f64 {
    let scale = c0.abs().max(c1.abs()).max(c2.abs());
    if c2.abs() <= 1e-14 * scale {
        return if c1 < 0.0 { -c0 / c1 } else { f64::INFINITY };
    }
    let disc = c1 * c1 - 4.0 * c2 * c0;
    if disc < 0.0 {
        return f64::INFINITY;
    }
    // numerically stable pair of roots
    let sq = disc.sqrt();
    let qv = -0.5 * (c1 + if c1 >= 0.0 { sq } else { -sq });
    let r1 = qv / c2;
    let r2 = if qv != 0.0 { c0 / qv } else { f64::INFINITY };
    [r1, r2].into_iter().filter(|r| *r > 0.0).fold(f64::INFINITY, f64::min)
}

// --- sparse symmetric system ---------------------------------------------------------

struct Csr {
    row_ptr: Vec<usize>,
    col: Vec<usize>,
    val: Vec<f64>,
    diag: Vec<f64>,
}

impl Csr {
    fn from_triplets(n: usize, mut t: Vec<(usize, usize, f64)>) -> Self {
        t.sort_by_key(|e| (e.0, e.1));
        let (mut row_ptr, mut col, mut val) = (vec![0usize; n + 1], Vec::new(), Vec::new());
        let mut diag = vec![0.0; n];
        let mut k = 0;
        while k < t.len() {
            let (r, c) = (t[k].0, t[k].1);
            let mut s = 0.0;
            while k < t.len() && t[k].0 == r && t[k].1 == c {
                s += t[k].2;
                k += 1;
            }
            col.push(c);
            val.push(s);
            row_ptr[r + 1] += 1;
            if r == c {
                diag[r] = s;
            }
        }
        for i in 0..n {
            row_ptr[i + 1] += row_ptr[i];
        }
        Self { row_ptr, col, val, diag }
    }

    fn mul(&self, x: &[f64], y: &mut [f64]) {
        for (i, yi) in y.iter_mut().enumerate() {
            *yi = (self.row_ptr[i]..self.row_ptr[i + 1]).map(|k| self.val[k] * x[self.col[k]]).sum();
        }
    }
}

/// Jacobi-preconditioned conjugate gradients on the free unknowns; entries
/// with `free[i] == false` keep their value in `x`.
fn conjugate_gradient(a: &Csr, b: &[f64], x: &mut [f64], free: &[bool]) {
    let n = b.len();
    let mut ax = vec![0.0; n];
    a.mul(x, &mut ax);
    let mut r: Vec<f64> = (0..n).map(|i| if free[i] { b[i] - ax[i] } else { 0.0 }).collect();
    let prec = |i: usize| if a.diag[i] > 1e-300 { 1.0 / a.diag[i] } else { 1.0 };
    let mut z: Vec<f64> = (0..n).map(|i| r[i] * prec(i)).collect();
    let mut p = z.clone();
    let mut rz: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
    let b_norm = b.iter().zip(free).filter(|(_, f)| **f).map(|(v, _)| v * v).sum::<f64>().sqrt().max(1e-300);
    let mut ap = vec![0.0; n];
    for _ in 0..(10 * n + 100) {
        if r.iter().map(|v| v * v).sum::<f64>().sqrt() <= 1e-13 * b_norm {
            break;
        }
        a.mul(&p, &mut ap);
        for i in 0..n {
            if !free[i] {
                ap[i] = 0.0;
            }
        }
        let pap: f64 = p.iter().zip(&ap).map(|(a, b)| a * b).sum();
        if pap.abs() < 1e-300 {
            break;
        }
        let alpha = rz / pap;
        for i in 0..n {
            x[i] += alpha * p[i];
            r[i] -= alpha * ap[i];
        }
        for i in 0..n {
            z[i] = r[i] * prec(i);
        }
        let rz_new: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
        let beta = rz_new / rz;
        rz = rz_new;
        for i in 0..n {
            p[i] = z[i] + beta * p[i];
            if !free[i] {
                p[i] = 0.0;
            }
        }
    }
}

// --- the solver ------------------------------------------------------------------------

/// Flatten with constrained singular values. `uv0` is the start layout (must
/// have positive signed area in every triangle; e.g. a conformal map);
/// `weights` holds one weight per triangle in `(0, 1]` (an empty slice means
/// all rigid, values below 1e-6 are raised to 1e-6 to keep the system
/// solvable).
pub fn flatten_constrained(
    pos: &[[f64; 3]],
    tris: &[[usize; 3]],
    uv0: &[[f64; 2]],
    weights: &[f64],
    p: &Params,
) -> Result<Output, String> {
    let n = pos.len();
    if uv0.len() != n {
        return Err(format!("{} start coordinates for {n} vertices", uv0.len()));
    }
    if !weights.is_empty() && weights.len() != tris.len() {
        return Err(format!("{} weights for {} triangles", weights.len(), tris.len()));
    }
    if p.pin >= n {
        return Err("pin vertex out of range".into());
    }
    if let Some(t) = tris.iter().find(|t| t.iter().any(|&v| v >= n)) {
        return Err(format!("triangle {t:?} refers to a missing vertex"));
    }
    let frames: Vec<Option<Frame>> = tris.iter().map(|t| frame(pos, t)).collect();
    let w = |ti: usize| weights.get(ti).copied().unwrap_or(1.0).max(1e-6);

    // Constant system matrix: edge form of the weighted Dirichlet energy.
    let mut trip = Vec::new();
    for (ti, t) in tris.iter().enumerate() {
        let Some(f) = frames[ti] else { continue };
        let x = f.x();
        for k in 0..3 {
            let (i, j) = ((k + 1) % 3, (k + 2) % 3);
            let (a, b) = (sub2(x[i], x[k]), sub2(x[j], x[k]));
            let cot = (a[0] * b[0] + a[1] * b[1]) / (a[0] * b[1] - a[1] * b[0]).abs();
            let c = 0.5 * w(ti) * cot;
            let (vi, vj) = (t[i], t[j]);
            trip.extend([(vi, vi, c), (vj, vj, c), (vi, vj, -c), (vj, vi, -c)]);
        }
    }
    let a = Csr::from_triplets(n, trip);
    let mut free = vec![true; n];
    free[p.pin] = false;

    let mut uv = uv0.to_vec();
    if frames.iter().zip(tris).any(|(f, t)| f.is_some() && area2(uv[t[0]], uv[t[1]], uv[t[2]]) <= 0.0) {
        return Err("the start layout has a flipped or degenerate triangle".into());
    }
    let diag = bbox_diag(&uv).max(1e-300);
    let mut iterations = 0;
    for it in 0..p.max_iterations {
        iterations = it + 1;
        // local step (inside `rhs`): clamp the singular values, keep the rotation frames
        let (bx, by) = rhs(tris, &frames, &uv, p.mode, &w, n);
        // global step
        let mut gx: Vec<f64> = uv.iter().map(|q| q[0]).collect();
        let mut gy: Vec<f64> = uv.iter().map(|q| q[1]).collect();
        conjugate_gradient(&a, &bx, &mut gx, &free);
        conjugate_gradient(&a, &by, &mut gy, &free);
        let step: Vec<[f64; 2]> = (0..n).map(|i| [gx[i] - uv[i][0], gy[i] - uv[i][1]]).collect();
        // fold-free line search
        let amax = max_fold_free_step(tris_ok(tris, &frames).as_slice(), &uv, &step);
        let alpha = if amax.is_finite() { (p.step_fraction * amax).min(1.0) } else { 1.0 };
        let mut moved = 0.0f64;
        for i in 0..n {
            uv[i][0] += alpha * step[i][0];
            uv[i][1] += alpha * step[i][1];
            moved = moved.max(alpha * step[i][0].hypot(step[i][1]));
        }
        if moved <= p.tolerance * diag {
            break;
        }
    }

    // One-sided modes: remove any remaining edge violation by a uniform scale.
    let (mut lo, mut hi) = edge_ratio_range(pos, tris, &uv);
    let mut final_scale = 1.0;
    let scale = match p.mode {
        Mode::StretchOnly if lo < 1.0 - p.edge_tolerance && lo > 0.0 => Some(1.0 / lo),
        Mode::CompressOnly if hi > 1.0 + p.edge_tolerance => Some(1.0 / hi),
        _ => None,
    };
    if let Some(s) = scale {
        let o = uv[p.pin];
        for q in uv.iter_mut() {
            q[0] = o[0] + s * (q[0] - o[0]);
            q[1] = o[1] + s * (q[1] - o[1]);
        }
        final_scale = s;
        (lo, hi) = (lo * s, hi * s);
    }

    let sigma = tris
        .iter()
        .zip(&frames)
        .map(|(t, f)| match f {
            Some(f) => singular_values(&f.jacobian([uv[t[0]], uv[t[1]], uv[t[2]]])),
            None => [1.0, 1.0],
        })
        .collect();
    let flipped = tris
        .iter()
        .zip(&frames)
        .filter(|(t, f)| f.is_some() && area2(uv[t[0]], uv[t[1]], uv[t[2]]) <= 0.0)
        .count();
    Ok(Output { uv, iterations, sigma, min_edge_ratio: lo, max_edge_ratio: hi, final_scale, flipped })
}

fn sub2(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn tris_ok(tris: &[[usize; 3]], frames: &[Option<Frame>]) -> Vec<[usize; 3]> {
    tris.iter().zip(frames).filter(|(_, f)| f.is_some()).map(|(t, _)| *t).collect()
}

fn bbox_diag(uv: &[[f64; 2]]) -> f64 {
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for q in uv {
        for d in 0..2 {
            lo[d] = lo[d].min(q[d]);
            hi[d] = hi[d].max(q[d]);
        }
    }
    (hi[0] - lo[0]).hypot(hi[1] - lo[1])
}

/// Right-hand sides of the global step for the current layout.
fn rhs(
    tris: &[[usize; 3]],
    frames: &[Option<Frame>],
    uv: &[[f64; 2]],
    mode: Mode,
    w: &dyn Fn(usize) -> f64,
    n: usize,
) -> (Vec<f64>, Vec<f64>) {
    let (mut bx, mut by) = (vec![0.0; n], vec![0.0; n]);
    for (ti, t) in tris.iter().enumerate() {
        let Some(f) = frames[ti] else { continue };
        let j = f.jacobian([uv[t[0]], uv[t[1]], uv[t[2]]]);
        let (phi, theta, s1, s2) = svd2(&j);
        let target = clamp_singular_values([s1.abs(), s2.abs()], mode);
        let l = mul(&mul(&rot(phi), &[[target[0], 0.0], [0.0, target[1]]]), &rot(theta));
        let x = f.x();
        for k in 0..3 {
            let (i, jn) = ((k + 1) % 3, (k + 2) % 3);
            let (xa, xb) = (sub2(x[i], x[k]), sub2(x[jn], x[k]));
            let cot = (xa[0] * xb[0] + xa[1] * xb[1]) / (xa[0] * xb[1] - xa[1] * xb[0]).abs();
            let c = 0.5 * w(ti) * cot;
            let e = sub2(x[i], x[jn]);
            let le = [l[0][0] * e[0] + l[0][1] * e[1], l[1][0] * e[0] + l[1][1] * e[1]];
            bx[t[i]] += c * le[0];
            by[t[i]] += c * le[1];
            bx[t[jn]] -= c * le[0];
            by[t[jn]] -= c * le[1];
        }
    }
    (bx, by)
}

/// Smallest and largest `flat edge length / 3-D edge length` over all triangle edges.
pub fn edge_ratio_range(pos: &[[f64; 3]], tris: &[[usize; 3]], uv: &[[f64; 2]]) -> (f64, f64) {
    let (mut lo, mut hi) = (f64::INFINITY, 0.0f64);
    for t in tris {
        for k in 0..3 {
            let (i, j) = (t[k], t[(k + 1) % 3]);
            let d3 = dot3(sub3(pos[i], pos[j]), sub3(pos[i], pos[j])).sqrt();
            if d3 > 0.0 {
                let r = (uv[i][0] - uv[j][0]).hypot(uv[i][1] - uv[j][1]) / d3;
                lo = lo.min(r);
                hi = hi.max(r);
            }
        }
    }
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// `(nx+1) x (ny+1)` grid; `f(i, j)` gives the 3-D position.
    fn grid(nx: usize, ny: usize, f: impl Fn(usize, usize) -> [f64; 3]) -> (Vec<[f64; 3]>, Vec<[usize; 3]>) {
        let id = |i: usize, j: usize| j * (nx + 1) + i;
        let pos = (0..=ny).flat_map(|j| (0..=nx).map(move |i| (i, j))).map(|(i, j)| f(i, j)).collect();
        let mut tris = Vec::new();
        for j in 0..ny {
            for i in 0..nx {
                tris.push([id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
                tris.push([id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
            }
        }
        (pos, tris)
    }

    fn worst_sigma(o: &Output) -> f64 {
        o.sigma.iter().flat_map(|s| s.iter()).map(|v| (v - 1.0).abs()).fold(0.0, f64::max)
    }

    #[test]
    fn flatten_constraints_singular_values_closed_form() {
        let s = singular_values(&[[3.0, 0.0], [0.0, 2.0]]);
        assert!((s[0] - 3.0).abs() < 1e-12 && (s[1] - 2.0).abs() < 1e-12);
        let s = singular_values(&[[0.0, -2.0], [2.0, 0.0]]);
        assert!((s[0] - 2.0).abs() < 1e-12 && (s[1] - 2.0).abs() < 1e-12);
        // shear [[1,1],[0,1]]: singular values are the golden ratio and its inverse
        let phi = (1.0 + 5f64.sqrt()) / 2.0;
        let s = singular_values(&[[1.0, 1.0], [0.0, 1.0]]);
        assert!((s[0] - phi).abs() < 1e-12 && (s[1] - 1.0 / phi).abs() < 1e-12);
    }

    #[test]
    fn flatten_constraints_clamp_acts_on_singular_values() {
        assert_eq!(clamp_singular_values([2.0, 0.5], Mode::Rigid), [1.0, 1.0]);
        assert_eq!(clamp_singular_values([2.0, 0.5], Mode::StretchOnly), [2.0, 1.0]);
        assert_eq!(clamp_singular_values([2.0, 0.5], Mode::CompressOnly), [1.0, 0.5]);
    }

    #[test]
    fn flatten_constraints_floppy_weights_mark_triangles() {
        assert_eq!(floppy_weights(4, &[1, 3, 9], 0.01), vec![1.0, 0.01, 1.0, 0.01]);
    }

    #[test]
    fn flatten_constraints_line_search_finds_the_exact_fold_step() {
        // Move vertex 2 of (0,0),(1,0),(0,1) down by 2: the area is zero at alpha = 0.5.
        let uv = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let step = [[0.0, 0.0], [0.0, 0.0], [0.0, -2.0]];
        let a = max_fold_free_step(&[[0, 1, 2]], &uv, &step);
        assert!((a - 0.5).abs() < 1e-12, "{a}");
        // uniform shrink by 0.5 per unit step: both legs hit zero together at alpha = 2
        let shrink = [[0.0, 0.0], [-1.0, 0.0], [0.0, -1.0]];
        assert!((max_fold_free_step(&[[0, 1, 2]], &uv, &shrink) - 1.0).abs() < 1e-12);
        // a step that only enlarges the triangle never folds it
        let grow = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        assert!(max_fold_free_step(&[[0, 1, 2]], &uv, &grow).is_infinite());
    }

    #[test]
    fn flatten_constraints_flat_grid_maps_to_itself() {
        let (pos, tris) = grid(5, 4, |i, j| [2.0 * i as f64, 2.0 * j as f64, 0.0]);
        // start: rotated by 30 deg, scaled by 1.3 and shifted
        let (s, c) = 30f64.to_radians().sin_cos();
        let uv0: Vec<[f64; 2]> = pos
            .iter()
            .map(|q| [1.3 * (c * q[0] - s * q[1]) + 5.0, 1.3 * (s * q[0] + c * q[1]) - 2.0])
            .collect();
        let o = flatten_constrained(&pos, &tris, &uv0, &[], &Params::default()).unwrap();
        assert!(worst_sigma(&o) < 1e-9, "{}", worst_sigma(&o));
        assert_eq!(o.flipped, 0);
        assert!((o.min_edge_ratio - 1.0).abs() < 1e-9 && (o.max_edge_ratio - 1.0).abs() < 1e-9);
    }

    #[test]
    fn flatten_constraints_cylinder_patch_flattens_without_stretch() {
        // 90 degree strip of a radius-20 cylinder, rulings along z: a polyhedral
        // prism, exactly developable. Start from a sheared, shrunk layout.
        let (r, n) = (20.0f64, 8usize);
        let dphi = 0.5 * PI / n as f64;
        let chord = 2.0 * r * (dphi / 2.0).sin();
        let (pos, tris) = grid(n, 6, |i, j| [r * (i as f64 * dphi).cos(), r * (i as f64 * dphi).sin(), 5.0 * j as f64]);
        let uv0: Vec<[f64; 2]> = (0..pos.len())
            .map(|v| {
                let (i, j) = ((v % (n + 1)) as f64, (v / (n + 1)) as f64);
                [0.9 * i * chord + 0.05 * 5.0 * j, 0.9 * 5.0 * j]
            })
            .collect();
        let o = flatten_constrained(&pos, &tris, &uv0, &[], &Params::default()).unwrap();
        assert!(worst_sigma(&o) < 1e-6, "{}", worst_sigma(&o));
        assert_eq!(o.flipped, 0);
    }

    type CapMesh = (Vec<[f64; 3]>, Vec<[usize; 3]>, Vec<[f64; 2]>);

    /// Cap of a sphere of radius 10 mm up to polar angle `max_theta`, a pole
    /// vertex (index 0) and `rings` rings of `sectors` vertices; start layout
    /// is the azimuthal equidistant projection.
    fn cap(rings: usize, sectors: usize, max_theta: f64) -> CapMesh {
        let r = 10.0;
        let mut pos = vec![[0.0, 0.0, r]];
        let mut uv = vec![[0.0, 0.0]];
        for k in 1..=rings {
            let th = max_theta * k as f64 / rings as f64;
            for s in 0..sectors {
                let ph = 2.0 * PI * s as f64 / sectors as f64;
                pos.push([r * th.sin() * ph.cos(), r * th.sin() * ph.sin(), r * th.cos()]);
                uv.push([r * th * ph.cos(), r * th * ph.sin()]);
            }
        }
        let ring = |k: usize, s: usize| 1 + (k - 1) * sectors + s % sectors;
        let mut tris = Vec::new();
        for s in 0..sectors {
            tris.push([0, ring(1, s), ring(1, s + 1)]);
        }
        for k in 1..rings {
            for s in 0..sectors {
                tris.push([ring(k, s), ring(k + 1, s), ring(k + 1, s + 1)]);
                tris.push([ring(k, s), ring(k + 1, s + 1), ring(k, s + 1)]);
            }
        }
        (pos, tris, uv)
    }

    #[test]
    fn flatten_constraints_hemisphere_patch_never_flips() {
        let (pos, tris, uv0) = cap(6, 12, 80f64.to_radians());
        // start from a lopsided but valid layout: squash the equidistant map
        let uv0: Vec<[f64; 2]> = uv0.iter().map(|q| [0.6 * q[0], q[1]]).collect();
        let o = flatten_constrained(&pos, &tris, &uv0, &[], &Params { pin: 0, ..Params::default() }).unwrap();
        assert_eq!(o.flipped, 0);
        assert!(tris.iter().all(|t| area2(o.uv[t[0]], o.uv[t[1]], o.uv[t[2]]) > 0.0));
        // a sphere cannot be flattened without distortion: it must be reported
        assert!(worst_sigma(&o) > 1e-3);
        // and the rigid fit stays close to an isometry on average
        let mean: f64 = o.sigma.iter().map(|s| (s[0] - 1.0).abs() + (s[1] - 1.0).abs()).sum::<f64>() / (2.0 * o.sigma.len() as f64);
        assert!(mean < 0.15, "{mean}");
    }

    #[test]
    fn flatten_constraints_stretch_only_leaves_no_compressed_edge() {
        let (pos, tris, uv0) = cap(6, 12, 80f64.to_radians());
        let p = Params { mode: Mode::StretchOnly, ..Params::default() };
        let o = flatten_constrained(&pos, &tris, &uv0, &[], &p).unwrap();
        assert_eq!(o.flipped, 0);
        assert!(o.min_edge_ratio >= 1.0 - 1e-9, "min ratio {}", o.min_edge_ratio);
        let (lo, _) = edge_ratio_range(&pos, &tris, &o.uv);
        assert!(lo >= 1.0 - 1e-9, "recomputed {lo}");
        assert!(o.sigma.iter().all(|s| s[1] >= 1.0 - 1e-6 || o.final_scale > 1.0));
    }

    #[test]
    fn flatten_constraints_compress_only_leaves_no_stretched_edge() {
        let (pos, tris, uv0) = cap(6, 12, 80f64.to_radians());
        let p = Params { mode: Mode::CompressOnly, ..Params::default() };
        let o = flatten_constrained(&pos, &tris, &uv0, &[], &p).unwrap();
        assert_eq!(o.flipped, 0);
        assert!(o.max_edge_ratio <= 1.0 + 1e-9, "max ratio {}", o.max_edge_ratio);
    }

    #[test]
    fn flatten_constraints_rejects_bad_input() {
        let (pos, tris) = grid(1, 1, |i, j| [i as f64, j as f64, 0.0]);
        let flipped: Vec<[f64; 2]> = pos.iter().map(|q| [-q[0], q[1]]).collect();
        assert!(flatten_constrained(&pos, &tris, &flipped, &[], &Params::default()).is_err());
        let ok: Vec<[f64; 2]> = pos.iter().map(|q| [q[0], q[1]]).collect();
        assert!(flatten_constrained(&pos, &tris, &ok, &[1.0], &Params::default()).is_err());
        assert!(flatten_constrained(&pos, &tris, &ok[..2], &[], &Params::default()).is_err());
    }
}

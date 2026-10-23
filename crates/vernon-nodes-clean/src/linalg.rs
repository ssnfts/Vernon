//! Small dense linear algebra: symmetric Jacobi eigen solver (up to 7x7),
//! 3x3 one-sided Jacobi SVD, pseudo-inverse solve with condition number, and
//! closed-form rigid / similarity fits from paired points (Horn 1987,
//! Umeyama 1991, Kabsch 1976). No external dependencies.

use glam::{DMat3, DQuat, DVec3};

pub const MAXN: usize = 7;
pub type Mat = [[f64; MAXN]; MAXN];

/// Eigen-decomposition of a symmetric matrix. `vectors[k]` is the k-th
/// eigenvector (length `n`), eigenvalues are sorted descending.
#[derive(Clone, Debug)]
pub struct Eigen {
    pub n: usize,
    pub values: [f64; MAXN],
    pub vectors: [[f64; MAXN]; MAXN],
}

/// Cyclic Jacobi rotations on the leading `n x n` block of `a_in`.
pub fn sym_eigen(n: usize, a_in: &Mat) -> Eigen {
    assert!(n <= MAXN);
    let mut a = *a_in;
    let mut v: Mat = [[0.0; MAXN]; MAXN];
    for i in 0..n {
        v[i][i] = 1.0;
    }
    let mut frob = 0.0;
    for i in 0..n {
        for j in 0..n {
            frob += a[i][j] * a[i][j];
        }
    }
    for _ in 0..100 {
        let mut off = 0.0;
        for i in 0..n {
            for j in (i + 1)..n {
                off += a[i][j] * a[i][j];
            }
        }
        if off <= 1e-32 * frob {
            break;
        }
        for p in 0..n {
            for q in (p + 1)..n {
                let apq = a[p][q];
                if apq == 0.0 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * apq);
                let sgn = if theta >= 0.0 { 1.0 } else { -1.0 };
                let t = sgn / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let akp = a[k][p];
                    let akq = a[k][q];
                    a[k][p] = c * akp - s * akq;
                    a[k][q] = s * akp + c * akq;
                }
                for k in 0..n {
                    let apk = a[p][k];
                    let aqk = a[q][k];
                    a[p][k] = c * apk - s * aqk;
                    a[q][k] = s * apk + c * aqk;
                }
                for k in 0..n {
                    let vkp = v[k][p];
                    let vkq = v[k][q];
                    v[k][p] = c * vkp - s * vkq;
                    v[k][q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut order = [0usize; MAXN];
    for (i, o) in order.iter_mut().enumerate().take(n) {
        *o = i;
    }
    // insertion sort, descending eigenvalue
    for i in 1..n {
        let mut j = i;
        while j > 0 && a[order[j]][order[j]] > a[order[j - 1]][order[j - 1]] {
            order.swap(j, j - 1);
            j -= 1;
        }
    }
    let mut out = Eigen {
        n,
        values: [0.0; MAXN],
        vectors: [[0.0; MAXN]; MAXN],
    };
    for k in 0..n {
        out.values[k] = a[order[k]][order[k]];
        for r in 0..n {
            out.vectors[k][r] = v[r][order[k]];
        }
    }
    out
}

/// Eigen-decomposition of a symmetric 3x3 matrix (descending eigenvalues,
/// unit eigenvectors).
pub fn eigen3(m: &DMat3) -> ([f64; 3], [DVec3; 3]) {
    let mut a: Mat = [[0.0; MAXN]; MAXN];
    let cols = [m.x_axis, m.y_axis, m.z_axis];
    for i in 0..3 {
        for j in 0..3 {
            a[i][j] = cols[j][i];
        }
    }
    let e = sym_eigen(3, &a);
    (
        [e.values[0], e.values[1], e.values[2]],
        [
            DVec3::new(e.vectors[0][0], e.vectors[0][1], e.vectors[0][2]),
            DVec3::new(e.vectors[1][0], e.vectors[1][1], e.vectors[1][2]),
            DVec3::new(e.vectors[2][0], e.vectors[2][1], e.vectors[2][2]),
        ],
    )
}

/// Result of a pseudo-inverse solve of a symmetric positive semi-definite
/// system.
#[derive(Clone, Debug)]
pub struct PinvSolution {
    pub x: [f64; MAXN],
    /// lambda_max / lambda_min of the system matrix (capped at 1e30).
    pub cond: f64,
    /// Eigenvectors of the directions that were dropped (not solved for).
    pub free: Vec<[f64; MAXN]>,
}

/// Solve `A x = b` for symmetric PSD `A` (leading `n x n`), dropping
/// eigen-directions whose eigenvalue is below `lambda_max / max_cond`.
pub fn solve_pinv(n: usize, a: &Mat, b: &[f64; MAXN], max_cond: f64) -> PinvSolution {
    let e = sym_eigen(n, a);
    let lmax = e.values[0].max(0.0);
    let lmin = e.values[n - 1];
    let cond = if lmax <= 0.0 || lmin <= lmax * 1e-30 {
        1e30
    } else {
        (lmax / lmin).min(1e30)
    };
    let mut x = [0.0; MAXN];
    let mut free = Vec::new();
    for k in 0..n {
        if e.values[k] > lmax / max_cond && e.values[k] > 0.0 {
            let mut dot = 0.0;
            for r in 0..n {
                dot += e.vectors[k][r] * b[r];
            }
            let c = dot / e.values[k];
            for r in 0..n {
                x[r] += c * e.vectors[k][r];
            }
        } else {
            free.push(e.vectors[k]);
        }
    }
    PinvSolution { x, cond, free }
}

/// 3x3 SVD by one-sided Jacobi: `m = U * diag(S) * V^T`, singular values
/// descending. Columns of `U` belonging to (near-)zero singular values are
/// completed to an orthonormal basis.
pub fn svd3(m: DMat3) -> (DMat3, DVec3, DMat3) {
    let mut u = [m.x_axis, m.y_axis, m.z_axis];
    let mut v = [DVec3::X, DVec3::Y, DVec3::Z];
    for _ in 0..60 {
        let mut rotated = false;
        for p in 0..2 {
            for q in (p + 1)..3 {
                let alpha = u[p].length_squared();
                let beta = u[q].length_squared();
                let gamma = u[p].dot(u[q]);
                if gamma == 0.0 || gamma.abs() <= 1e-15 * (alpha * beta).sqrt() {
                    continue;
                }
                rotated = true;
                let zeta = (beta - alpha) / (2.0 * gamma);
                let sgn = if zeta >= 0.0 { 1.0 } else { -1.0 };
                let t = sgn / (zeta.abs() + (1.0 + zeta * zeta).sqrt());
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = c * t;
                let (up, uq) = (u[p], u[q]);
                u[p] = up * c - uq * s;
                u[q] = up * s + uq * c;
                let (vp, vq) = (v[p], v[q]);
                v[p] = vp * c - vq * s;
                v[q] = vp * s + vq * c;
            }
        }
        if !rotated {
            break;
        }
    }
    let mut idx = [0usize, 1, 2];
    let sv = [u[0].length(), u[1].length(), u[2].length()];
    idx.sort_by(|&a, &b| sv[b].total_cmp(&sv[a]));
    let s = DVec3::new(sv[idx[0]], sv[idx[1]], sv[idx[2]]);
    let vmax = s.x.max(1e-300);
    let mut uc = [DVec3::ZERO; 3];
    let mut ok = [false; 3];
    for k in 0..3 {
        let sk = sv[idx[k]];
        if sk > 1e-12 * vmax {
            uc[k] = u[idx[k]] / sk;
            ok[k] = true;
        }
    }
    if !ok[0] {
        uc[0] = DVec3::X;
        ok[0] = true;
    }
    if !ok[1] {
        let helper = if uc[0].x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
        uc[1] = (helper - uc[0] * helper.dot(uc[0])).normalize_or_zero();
        ok[1] = true;
    }
    if !ok[2] {
        uc[2] = uc[0].cross(uc[1]).normalize_or_zero();
    }
    (
        DMat3::from_cols(uc[0], uc[1], uc[2]),
        s,
        DMat3::from_cols(v[idx[0]], v[idx[1]], v[idx[2]]),
    )
}

/// Errors from the closed-form fits.
#[derive(Debug, Clone, thiserror::Error)]
pub enum FitError {
    #[error("need at least 3 point pairs, got {0}")]
    TooFew(usize),
    #[error("point lists have different lengths ({0} vs {1})")]
    Mismatch(usize, usize),
    #[error("point pairs are collinear or coincident, so rotation about the line is undetermined")]
    Degenerate,
}

/// A fitted similarity transform `x -> s * R * x + t`.
#[derive(Debug, Clone, Copy)]
pub struct RigidFit {
    pub r: DMat3,
    pub t: DVec3,
    pub s: f64,
    pub rms: f64,
}

impl RigidFit {
    pub fn apply(&self, p: DVec3) -> DVec3 {
        self.s * (self.r * p) + self.t
    }
}

fn weight(w: Option<&[f64]>, i: usize) -> f64 {
    w.map_or(1.0, |w| w[i])
}

fn rank_ok(pc: &[DVec3], w: Option<&[f64]>, wsum: f64) -> bool {
    let mut cov = DMat3::ZERO;
    for (i, p) in pc.iter().enumerate() {
        let wi = weight(w, i);
        cov += DMat3::from_cols(*p * p.x, *p * p.y, *p * p.z) * wi;
    }
    let (ev, _) = eigen3(&(cov * (1.0 / wsum)));
    ev[0] > 0.0 && ev[1] > 1e-12 * ev[0]
}

fn centroids(p: &[DVec3], q: &[DVec3], w: Option<&[f64]>) -> (DVec3, DVec3, f64) {
    let mut pc = DVec3::ZERO;
    let mut qc = DVec3::ZERO;
    let mut ws = 0.0;
    for i in 0..p.len() {
        let wi = weight(w, i);
        pc += p[i] * wi;
        qc += q[i] * wi;
        ws += wi;
    }
    (pc / ws, qc / ws, ws)
}

fn finish(p: &[DVec3], q: &[DVec3], w: Option<&[f64]>, r: DMat3, s: f64, pc: DVec3, qc: DVec3, ws: f64) -> RigidFit {
    let t = qc - s * (r * pc);
    let mut e2 = 0.0;
    for i in 0..p.len() {
        let d = s * (r * p[i]) + t - q[i];
        e2 += weight(w, i) * d.length_squared();
    }
    RigidFit {
        r,
        t,
        s,
        rms: (e2 / ws).sqrt(),
    }
}

fn prepare(p: &[DVec3], q: &[DVec3], w: Option<&[f64]>) -> Result<(DVec3, DVec3, f64, Vec<DVec3>, Vec<DVec3>), FitError> {
    if p.len() != q.len() {
        return Err(FitError::Mismatch(p.len(), q.len()));
    }
    if p.len() < 3 {
        return Err(FitError::TooFew(p.len()));
    }
    let (pc, qc, ws) = centroids(p, q, w);
    let pcs: Vec<DVec3> = p.iter().map(|x| *x - pc).collect();
    let qcs: Vec<DVec3> = q.iter().map(|x| *x - qc).collect();
    if (ws.is_nan() || ws <= 0.0) || !rank_ok(&pcs, w, ws) || !rank_ok(&qcs, w, ws) {
        return Err(FitError::Degenerate);
    }
    Ok((pc, qc, ws, pcs, qcs))
}

/// Horn's quaternion method. With `with_scale` the optimal uniform scale
/// (Umeyama) is returned too; otherwise `s == 1`.
pub fn fit_rigid(p: &[DVec3], q: &[DVec3], w: Option<&[f64]>, with_scale: bool) -> Result<RigidFit, FitError> {
    let (pc, qc, ws, pcs, qcs) = prepare(p, q, w)?;
    // S[a][b] = sum w * p_a * q_b
    let mut s = [[0.0f64; 3]; 3];
    for i in 0..pcs.len() {
        let wi = weight(w, i);
        let (a, b) = (pcs[i].to_array(), qcs[i].to_array());
        for r in 0..3 {
            for c in 0..3 {
                s[r][c] += wi * a[r] * b[c];
            }
        }
    }
    let (sxx, sxy, sxz) = (s[0][0], s[0][1], s[0][2]);
    let (syx, syy, syz) = (s[1][0], s[1][1], s[1][2]);
    let (szx, szy, szz) = (s[2][0], s[2][1], s[2][2]);
    let mut n: Mat = [[0.0; MAXN]; MAXN];
    let rows = [
        [sxx + syy + szz, syz - szy, szx - sxz, sxy - syx],
        [syz - szy, sxx - syy - szz, sxy + syx, szx + sxz],
        [szx - sxz, sxy + syx, -sxx + syy - szz, syz + szy],
        [sxy - syx, szx + sxz, syz + szy, -sxx - syy + szz],
    ];
    for i in 0..4 {
        for j in 0..4 {
            n[i][j] = rows[i][j];
        }
    }
    let e = sym_eigen(4, &n);
    let qv = e.vectors[0];
    let quat = DQuat::from_xyzw(qv[1], qv[2], qv[3], qv[0]).normalize();
    let r = DMat3::from_quat(quat);
    let scale = if with_scale {
        let mut num = 0.0;
        let mut den = 0.0;
        for i in 0..pcs.len() {
            let wi = weight(w, i);
            num += wi * qcs[i].dot(r * pcs[i]);
            den += wi * pcs[i].length_squared();
        }
        if den > 0.0 {
            num / den
        } else {
            1.0
        }
    } else {
        1.0
    };
    Ok(finish(p, q, w, r, scale, pc, qc, ws))
}

/// Kabsch / SVD route to the same rigid fit (used to cross-check Horn).
pub fn fit_rigid_svd(p: &[DVec3], q: &[DVec3], w: Option<&[f64]>) -> Result<RigidFit, FitError> {
    let (pc, qc, ws, pcs, qcs) = prepare(p, q, w)?;
    let mut h = DMat3::ZERO;
    for i in 0..pcs.len() {
        let wi = weight(w, i);
        // H = sum w p q^T
        let (a, b) = (pcs[i], qcs[i]);
        h += DMat3::from_cols(a * b.x, a * b.y, a * b.z) * wi;
    }
    let (u, _s, v) = svd3(h);
    let d = (v * u.transpose()).determinant().signum();
    let r = v * DMat3::from_diagonal(DVec3::new(1.0, 1.0, d)) * u.transpose();
    Ok(finish(p, q, w, r, 1.0, pc, qc, ws))
}

/// Rotation matrix for a small rotation vector (Rodrigues).
pub fn rodrigues(r: DVec3) -> DMat3 {
    let angle = r.length();
    if angle < 1e-300 {
        DMat3::IDENTITY
    } else {
        DMat3::from_axis_angle(r / angle, angle)
    }
}

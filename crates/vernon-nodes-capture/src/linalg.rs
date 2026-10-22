//! Small dense linear algebra: symmetric eigen-solver (cyclic Jacobi), a
//! pivoted linear solve and the closed-form similarity fit.
//!
//! The similarity fit is Umeyama's least-squares optimum (scale `c`,
//! rotation `R`, translation `t`). The rotation is found through Horn's
//! quaternion eigenproblem, which reaches the same optimum but stays well
//! defined for planar point sets (three markers always span a plane).

use glam::{DMat4, DQuat, DVec3};

/// Eigen-decomposition of a symmetric `n x n` row-major matrix.
/// Returns `(eigenvalues, vectors)`; `vectors[i * n + k]` is component `i`
/// of eigenvector `k`.
pub fn sym_eigen(n: usize, a: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let mut a = a.to_vec();
    let mut v = vec![0.0; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    for _sweep in 0..100 {
        let mut off = 0.0;
        let mut diag = 0.0;
        for p in 0..n {
            diag += a[p * n + p] * a[p * n + p];
            for q in (p + 1)..n {
                off += a[p * n + q] * a[p * n + q];
            }
        }
        if off <= 1e-30 * (diag + off) || off < 1e-300 {
            break;
        }
        for p in 0..n.saturating_sub(1) {
            for q in (p + 1)..n {
                let apq = a[p * n + q];
                if apq.abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q * n + q] - a[p * n + p]) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let akp = a[k * n + p];
                    let akq = a[k * n + q];
                    a[k * n + p] = c * akp - s * akq;
                    a[k * n + q] = s * akp + c * akq;
                }
                for k in 0..n {
                    let apk = a[p * n + k];
                    let aqk = a[q * n + k];
                    a[p * n + k] = c * apk - s * aqk;
                    a[q * n + k] = s * apk + c * aqk;
                }
                for k in 0..n {
                    let vkp = v[k * n + p];
                    let vkq = v[k * n + q];
                    v[k * n + p] = c * vkp - s * vkq;
                    v[k * n + q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let vals = (0..n).map(|i| a[i * n + i]).collect();
    (vals, v)
}

/// Solve `A x = b` (row-major `n x n`) by Gaussian elimination with partial
/// pivoting. `None` when the matrix is numerically singular.
pub fn solve(n: usize, a: &[f64], b: &[f64]) -> Option<Vec<f64>> {
    let mut m = a.to_vec();
    let mut x = b.to_vec();
    let scale = m.iter().fold(0.0_f64, |s, v| s.max(v.abs()));
    if scale == 0.0 {
        return None;
    }
    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if m[r * n + col].abs() > m[piv * n + col].abs() {
                piv = r;
            }
        }
        if m[piv * n + col].abs() < 1e-14 * scale {
            return None;
        }
        if piv != col {
            for k in 0..n {
                m.swap(col * n + k, piv * n + k);
            }
            x.swap(col, piv);
        }
        for r in (col + 1)..n {
            let f = m[r * n + col] / m[col * n + col];
            if f != 0.0 {
                for k in col..n {
                    m[r * n + k] -= f * m[col * n + k];
                }
                x[r] -= f * x[col];
            }
        }
    }
    for col in (0..n).rev() {
        let mut s = x[col];
        for k in (col + 1)..n {
            s -= m[col * n + k] * x[k];
        }
        x[col] = s / m[col * n + col];
    }
    Some(x)
}

/// Similarity transform `y = scale * R x + t`.
#[derive(Debug, Clone, Copy)]
pub struct Sim3 {
    pub scale: f64,
    pub rot: DQuat,
    pub trans: DVec3,
}

impl Sim3 {
    pub fn identity() -> Self {
        Sim3 {
            scale: 1.0,
            rot: DQuat::IDENTITY,
            trans: DVec3::ZERO,
        }
    }

    pub fn apply(&self, p: DVec3) -> DVec3 {
        self.scale * (self.rot * p) + self.trans
    }

    pub fn to_mat4(&self) -> DMat4 {
        DMat4::from_scale_rotation_translation(DVec3::splat(self.scale), self.rot, self.trans)
    }

    /// Rotation angle in degrees.
    pub fn angle_deg(&self) -> f64 {
        self.rot.to_axis_angle().1.to_degrees()
    }
}

/// Covariance spread test: `true` when the points do not span at least a
/// plane (all on one line, or all coincident).
pub fn is_collinear(points: &[DVec3]) -> bool {
    if points.len() < 3 {
        return true;
    }
    let c = crate::util::centroid(points);
    let mut m = [0.0; 9];
    for p in points {
        let d = *p - c;
        let dd = [d.x, d.y, d.z];
        for i in 0..3 {
            for j in 0..3 {
                m[i * 3 + j] += dd[i] * dd[j];
            }
        }
    }
    let (mut vals, _) = sym_eigen(3, &m);
    vals.sort_by(|a, b| b.total_cmp(a));
    vals[0] <= 0.0 || vals[1] <= 1e-9 * vals[0]
}

/// Least-squares similarity (or rigid, `with_scale = false`) transform that
/// maps `src[i]` onto `dst[i]`. Needs at least three non-collinear pairs.
pub fn fit_similarity(src: &[DVec3], dst: &[DVec3], with_scale: bool) -> Result<Sim3, String> {
    if src.len() != dst.len() {
        return Err(format!(
            "point lists differ in length ({} vs {})",
            src.len(),
            dst.len()
        ));
    }
    if src.len() < 3 {
        return Err("need at least 3 point pairs".into());
    }
    if is_collinear(src) || is_collinear(dst) {
        return Err("point pairs are collinear; need 3 or more non-collinear points".into());
    }
    let mu_x = crate::util::centroid(src);
    let mu_y = crate::util::centroid(dst);
    let mut s = [[0.0_f64; 3]; 3];
    let mut var_x = 0.0;
    for (x, y) in src.iter().zip(dst) {
        let a = (*x - mu_x).to_array();
        let b = (*y - mu_y).to_array();
        var_x += a[0] * a[0] + a[1] * a[1] + a[2] * a[2];
        for i in 0..3 {
            for j in 0..3 {
                s[i][j] += a[i] * b[j];
            }
        }
    }
    let (sxx, sxy, sxz) = (s[0][0], s[0][1], s[0][2]);
    let (syx, syy, syz) = (s[1][0], s[1][1], s[1][2]);
    let (szx, szy, szz) = (s[2][0], s[2][1], s[2][2]);
    #[rustfmt::skip]
    let n = [
        sxx + syy + szz, syz - szy,        szx - sxz,        sxy - syx,
        syz - szy,       sxx - syy - szz,  sxy + syx,        szx + sxz,
        szx - sxz,       sxy + syx,       -sxx + syy - szz,  syz + szy,
        sxy - syx,       szx + sxz,        syz + szy,       -sxx - syy + szz,
    ];
    let (vals, vecs) = sym_eigen(4, &n);
    let mut best = 0;
    for k in 1..4 {
        if vals[k] > vals[best] {
            best = k;
        }
    }
    let q = DQuat::from_xyzw(
        vecs[best + 4],
        vecs[2 * 4 + best],
        vecs[3 * 4 + best],
        vecs[best],
    )
    .normalize();
    let scale = if with_scale {
        let mut num = 0.0;
        for (x, y) in src.iter().zip(dst) {
            num += (*y - mu_y).dot(q * (*x - mu_x));
        }
        if var_x <= 0.0 {
            return Err("source points coincide".into());
        }
        let c = num / var_x;
        if !(c.is_finite() && c > 0.0) {
            return Err("fitted scale is not positive".into());
        }
        c
    } else {
        1.0
    };
    let trans = mu_y - scale * (q * mu_x);
    Ok(Sim3 {
        scale,
        rot: q,
        trans,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eigen_of_known_matrix() {
        // [[2,1],[1,2]] -> 3 and 1
        let (mut v, _) = sym_eigen(2, &[2.0, 1.0, 1.0, 2.0]);
        v.sort_by(|a, b| a.total_cmp(b));
        assert!((v[0] - 1.0).abs() < 1e-12 && (v[1] - 3.0).abs() < 1e-12);
    }

    #[test]
    fn eigen_vectors_are_orthonormal() {
        let a = [4.0, 1.0, 0.5, 1.0, 3.0, 0.2, 0.5, 0.2, 1.0];
        let (vals, v) = sym_eigen(3, &a);
        for k in 0..3 {
            for l in 0..3 {
                let dot: f64 = (0..3).map(|i| v[i * 3 + k] * v[i * 3 + l]).sum();
                let want = if k == l { 1.0 } else { 0.0 };
                assert!((dot - want).abs() < 1e-12);
            }
            // A v = lambda v
            for i in 0..3 {
                let av: f64 = (0..3).map(|j| a[i * 3 + j] * v[j * 3 + k]).sum();
                assert!((av - vals[k] * v[i * 3 + k]).abs() < 1e-10);
            }
        }
    }

    #[test]
    fn solve_matches_hand_result() {
        let x = solve(3, &[2.0, 1.0, 0.0, 1.0, 3.0, 1.0, 0.0, 1.0, 4.0], &[3.0, 5.0, 5.0]).unwrap();
        assert!((x[0] - 1.0).abs() < 1e-12 && (x[1] - 1.0).abs() < 1e-12 && (x[2] - 1.0).abs() < 1e-12);
        assert!(solve(2, &[1.0, 2.0, 2.0, 4.0], &[1.0, 2.0]).is_none());
    }

    #[test]
    fn similarity_recovers_known_transform() {
        let truth = Sim3 {
            scale: 1000.0,
            rot: DQuat::from_rotation_z(30f64.to_radians()),
            trans: DVec3::new(120.0, -40.0, 15.0),
        };
        let src = [
            DVec3::new(0.1, 0.2, 0.0),
            DVec3::new(2.0, 0.5, 0.3),
            DVec3::new(1.0, 3.0, 0.1),
            DVec3::new(4.0, 1.0, 0.9),
            DVec3::new(3.0, 3.5, 1.2),
            DVec3::new(0.5, 2.0, 0.7),
        ];
        let dst: Vec<DVec3> = src.iter().map(|p| truth.apply(*p)).collect();
        let fit = fit_similarity(&src, &dst, true).unwrap();
        assert!((fit.scale - 1000.0).abs() < 1e-9);
        assert!((fit.trans - truth.trans).length() < 1e-9);
        assert!(fit.rot.angle_between(truth.rot) < 1e-12);
        for (s, d) in src.iter().zip(&dst) {
            assert!((fit.apply(*s) - *d).length() < 1e-9);
        }
    }

    #[test]
    fn rigid_fit_handles_three_points_and_rejects_collinear() {
        let truth = Sim3 {
            scale: 1.0,
            rot: DQuat::from_axis_angle(DVec3::new(1.0, 2.0, 3.0).normalize(), 0.7),
            trans: DVec3::new(5.0, 6.0, 7.0),
        };
        let src = [DVec3::ZERO, DVec3::X * 100.0, DVec3::Y * 80.0];
        let dst: Vec<DVec3> = src.iter().map(|p| truth.apply(*p)).collect();
        let fit = fit_similarity(&src, &dst, false).unwrap();
        for (s, d) in src.iter().zip(&dst) {
            assert!((fit.apply(*s) - *d).length() < 1e-9);
        }
        let line = [DVec3::ZERO, DVec3::X, DVec3::X * 2.0];
        assert!(fit_similarity(&line, &line, false).is_err());
    }
}

//! Small dense and sparse solvers used by the fitting code (no external deps).

use glam::DVec3;

/// Solve `A x = b` for a symmetric positive (semi)definite matrix `a`
/// (row-major, `n x n`) and several right-hand sides at once. A tiny ridge is
/// added when the plain Cholesky factorisation breaks down. Returns `None`
/// when the system is hopeless.
pub fn solve_spd(a: &[f64], n: usize, rhs: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    if n == 0 {
        return Some(rhs.iter().map(|_| Vec::new()).collect());
    }
    let max_diag = (0..n).map(|i| a[i * n + i].abs()).fold(0.0, f64::max).max(1e-300);
    for attempt in 0..6 {
        let ridge = if attempt == 0 { 0.0 } else { max_diag * 10f64.powi(-12 + 2 * attempt) };
        let mut l = a.to_vec();
        for i in 0..n {
            l[i * n + i] += ridge;
        }
        if cholesky_in_place(&mut l, n, max_diag * 1e-14) {
            return Some(rhs.iter().map(|b| cholesky_solve(&l, n, b)).collect());
        }
    }
    None
}

fn cholesky_in_place(l: &mut [f64], n: usize, min_pivot: f64) -> bool {
    for j in 0..n {
        let mut d = l[j * n + j];
        for k in 0..j {
            d -= l[j * n + k] * l[j * n + k];
        }
        if d <= min_pivot {
            return false;
        }
        let d = d.sqrt();
        l[j * n + j] = d;
        for i in j + 1..n {
            let mut s = l[i * n + j];
            for k in 0..j {
                s -= l[i * n + k] * l[j * n + k];
            }
            l[i * n + j] = s / d;
        }
    }
    true
}

fn cholesky_solve(l: &[f64], n: usize, b: &[f64]) -> Vec<f64> {
    let mut y = b.to_vec();
    for i in 0..n {
        let mut s = y[i];
        for k in 0..i {
            s -= l[i * n + k] * y[k];
        }
        y[i] = s / l[i * n + i];
    }
    for i in (0..n).rev() {
        let mut s = y[i];
        for k in i + 1..n {
            s -= l[k * n + i] * y[k];
        }
        y[i] = s / l[i * n + i];
    }
    y
}

/// Gaussian elimination with partial pivoting for a general small system.
pub fn solve_dense(a: &[f64], n: usize, b: &[f64]) -> Option<Vec<f64>> {
    let mut m = a.to_vec();
    let mut x = b.to_vec();
    for col in 0..n {
        let mut piv = col;
        for r in col + 1..n {
            if m[r * n + col].abs() > m[piv * n + col].abs() {
                piv = r;
            }
        }
        if m[piv * n + col].abs() < 1e-300 {
            return None;
        }
        if piv != col {
            for k in 0..n {
                m.swap(col * n + k, piv * n + k);
            }
            x.swap(col, piv);
        }
        for r in col + 1..n {
            let f = m[r * n + col] / m[col * n + col];
            if f != 0.0 {
                for k in col..n {
                    m[r * n + k] -= f * m[col * n + k];
                }
                x[r] -= f * x[col];
            }
        }
    }
    for i in (0..n).rev() {
        let mut s = x[i];
        for k in i + 1..n {
            s -= m[i * n + k] * x[k];
        }
        x[i] = s / m[i * n + i];
    }
    Some(x)
}

/// Jacobi-preconditioned conjugate gradients for a symmetric positive
/// semi-definite operator given matrix-free. `x` holds the start vector on
/// entry and the solution on exit. Returns (iterations, relative residual).
pub fn cg(
    apply: &dyn Fn(&[f64], &mut [f64]),
    diag: &[f64],
    b: &[f64],
    x: &mut [f64],
    rel_tol: f64,
    max_iter: usize,
) -> (usize, f64) {
    let n = b.len();
    let mut r = vec![0.0; n];
    let mut ax = vec![0.0; n];
    apply(x, &mut ax);
    for i in 0..n {
        r[i] = b[i] - ax[i];
    }
    let bnorm = b.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
    let precond = |r: &[f64], z: &mut [f64]| {
        for i in 0..n {
            z[i] = if diag[i] > 1e-300 { r[i] / diag[i] } else { r[i] };
        }
    };
    let mut z = vec![0.0; n];
    precond(&r, &mut z);
    let mut p = z.clone();
    let mut rz: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
    let mut ap = vec![0.0; n];
    let mut rel = r.iter().map(|v| v * v).sum::<f64>().sqrt() / bnorm;
    let mut it = 0;
    while it < max_iter && rel > rel_tol {
        apply(&p, &mut ap);
        let pap: f64 = p.iter().zip(&ap).map(|(a, b)| a * b).sum();
        if pap.abs() < 1e-300 {
            break;
        }
        let alpha = rz / pap;
        for i in 0..n {
            x[i] += alpha * p[i];
            r[i] -= alpha * ap[i];
        }
        precond(&r, &mut z);
        let rz_new: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
        let beta = rz_new / rz.max(1e-300);
        for i in 0..n {
            p[i] = z[i] + beta * p[i];
        }
        rz = rz_new;
        rel = r.iter().map(|v| v * v).sum::<f64>().sqrt() / bnorm;
        it += 1;
    }
    (it, rel)
}

/// Eigen-decomposition of a symmetric 3x3 matrix by Jacobi rotations.
/// Eigenvalues are returned in ascending order with matching unit vectors.
pub fn sym_eigen3(m: [[f64; 3]; 3]) -> ([f64; 3], [DVec3; 3]) {
    let mut a = m;
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..64 {
        let off = a[0][1].abs() + a[0][2].abs() + a[1][2].abs();
        let scale = a[0][0].abs() + a[1][1].abs() + a[2][2].abs();
        if off <= 1e-300 || off < 1e-16 * scale {
            break;
        }
        for (p, q) in [(0usize, 1usize), (0, 2), (1, 2)] {
            if a[p][q].abs() < 1e-300 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            for k in 0..3 {
                let (akp, akq) = (a[k][p], a[k][q]);
                a[k][p] = c * akp - s * akq;
                a[k][q] = s * akp + c * akq;
            }
            for k in 0..3 {
                let (apk, aqk) = (a[p][k], a[q][k]);
                a[p][k] = c * apk - s * aqk;
                a[q][k] = s * apk + c * aqk;
            }
            for k in 0..3 {
                let (vkp, vkq) = (v[k][p], v[k][q]);
                v[k][p] = c * vkp - s * vkq;
                v[k][q] = s * vkp + c * vkq;
            }
        }
    }
    let mut idx = [0usize, 1, 2];
    idx.sort_by(|&i, &j| a[i][i].total_cmp(&a[j][j]));
    let vals = [a[idx[0]][idx[0]], a[idx[1]][idx[1]], a[idx[2]][idx[2]]];
    let vec_of = |c: usize| DVec3::new(v[0][c], v[1][c], v[2][c]).normalize_or_zero();
    (vals, [vec_of(idx[0]), vec_of(idx[1]), vec_of(idx[2])])
}

/// Covariance-style accumulation helper: returns the weighted centroid and the
/// 3x3 scatter matrix of `pts` about it.
pub fn scatter(pts: &[DVec3], weights: Option<&[f64]>) -> (DVec3, [[f64; 3]; 3]) {
    let w = |i: usize| weights.map_or(1.0, |w| w[i]);
    let wsum: f64 = (0..pts.len()).map(w).sum::<f64>().max(1e-300);
    let c = pts.iter().enumerate().fold(DVec3::ZERO, |acc, (i, p)| acc + *p * w(i)) / wsum;
    let mut m = [[0.0; 3]; 3];
    for (i, p) in pts.iter().enumerate() {
        let d = (*p - c).to_array();
        for r in 0..3 {
            for k in 0..3 {
                m[r][k] += w(i) * d[r] * d[k];
            }
        }
    }
    (c, m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eigen_known_matrix() {
        let (vals, vecs) = sym_eigen3([[2.0, 0.0, 0.0], [0.0, 3.0, 4.0], [0.0, 4.0, 9.0]]);
        assert!((vals[0] - 1.0).abs() < 1e-12);
        assert!((vals[1] - 2.0).abs() < 1e-12);
        assert!((vals[2] - 11.0).abs() < 1e-12);
        assert!((vecs[1].x.abs() - 1.0).abs() < 1e-12);
        // eigenvector of 1: (0, 2, -1)/sqrt5
        assert!((vecs[0].dot(DVec3::new(0.0, 2.0, -1.0).normalize()).abs() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn spd_and_dense_agree() {
        let a = [4.0, 1.0, 0.0, 1.0, 3.0, 1.0, 0.0, 1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        let x1 = solve_spd(&a, 3, std::slice::from_ref(&b)).unwrap().remove(0);
        let x2 = solve_dense(&a, 3, &b).unwrap();
        for i in 0..3 {
            assert!((x1[i] - x2[i]).abs() < 1e-12);
        }
    }

    #[test]
    fn cg_solves_tridiagonal() {
        let n = 50;
        let apply = |x: &[f64], y: &mut [f64]| {
            for i in 0..n {
                let mut s = 2.0 * x[i];
                if i > 0 {
                    s -= x[i - 1];
                }
                if i + 1 < n {
                    s -= x[i + 1];
                }
                y[i] = s + 0.01 * x[i];
            }
        };
        let diag = vec![2.01; n];
        let b = vec![1.0; n];
        let mut x = vec![0.0; n];
        let (_, rel) = cg(&apply, &diag, &b, &mut x, 1e-12, 500);
        assert!(rel < 1e-10);
    }
}

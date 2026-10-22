//! Small dense linear algebra for the structural nodes.
//!
//! Stress and strain vectors use the order `[xx, yy, zz, xy, yz, zx]` (the
//! order of the solver's result blocks); shear strains are engineering
//! strains (gamma = 2 epsilon).

#![allow(clippy::needless_range_loop)]

pub type M3 = [[f64; 3]; 3];
pub type M6 = [[f64; 6]; 6];
pub type V6 = [f64; 6];

pub const IDENTITY3: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Index pairs of the 6-vector order.
const PAIRS: [(usize, usize); 6] = [(0, 0), (1, 1), (2, 2), (0, 1), (1, 2), (2, 0)];

/// Gauss-Jordan inverse with partial pivoting; `None` when singular.
pub fn invert6(m: &M6) -> Option<M6> {
    let mut a = *m;
    let mut inv = [[0.0; 6]; 6];
    for (i, row) in inv.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for col in 0..6 {
        let mut piv = col;
        for r in col + 1..6 {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, piv);
        inv.swap(col, piv);
        let d = a[col][col];
        for k in 0..6 {
            a[col][k] /= d;
            inv[col][k] /= d;
        }
        for r in 0..6 {
            if r != col {
                let f = a[r][col];
                if f != 0.0 {
                    for k in 0..6 {
                        a[r][k] -= f * a[col][k];
                        inv[r][k] -= f * inv[col][k];
                    }
                }
            }
        }
    }
    Some(inv)
}

pub fn mul6(a: &M6, b: &M6) -> M6 {
    let mut c = [[0.0; 6]; 6];
    for i in 0..6 {
        for k in 0..6 {
            let aik = a[i][k];
            for j in 0..6 {
                c[i][j] += aik * b[k][j];
            }
        }
    }
    c
}

pub fn mulv6(a: &M6, v: &V6) -> V6 {
    let mut out = [0.0; 6];
    for i in 0..6 {
        for j in 0..6 {
            out[i] += a[i][j] * v[j];
        }
    }
    out
}

/// Stress transformation `s' = K s` for new axes whose directions in the old
/// frame are the rows of `q` (so `x' = q x`).
pub fn stress_transform(q: &M3) -> M6 {
    let mut k = [[0.0; 6]; 6];
    for (i6, &(i, j)) in PAIRS.iter().enumerate() {
        for (j6, &(a, b)) in PAIRS.iter().enumerate() {
            let mut v = q[i][a] * q[j][b];
            if a != b {
                v += q[i][b] * q[j][a];
            }
            k[i6][j6] = v;
        }
    }
    k
}

/// Engineering-strain transformation `e' = K e` for the same axes.
pub fn strain_transform(q: &M3) -> M6 {
    let mut k = stress_transform(q);
    for (i6, row) in k.iter_mut().enumerate() {
        for (j6, v) in row.iter_mut().enumerate() {
            if j6 >= 3 {
                *v *= 0.5;
            }
            if i6 >= 3 {
                *v *= 2.0;
            }
        }
    }
    k
}

/// Rotate a stiffness matrix given in the material frame (axes = rows of
/// `q` in global coordinates) into the global frame.
pub fn rotate_stiffness_to_global(d_mat: &M6, q: &M3) -> Option<M6> {
    let ks = invert6(&stress_transform(q))?;
    Some(mul6(&mul6(&ks, d_mat), &strain_transform(q)))
}

/// Rotate a stress vector with `s' = Q s Q^T`.
pub fn rotate_stress(s: &V6, q: &M3) -> V6 {
    mulv6(&stress_transform(q), s)
}

pub fn stress_to_tensor(s: &V6) -> M3 {
    [[s[0], s[3], s[5]], [s[3], s[1], s[4]], [s[5], s[4], s[2]]]
}

pub fn von_mises(s: &V6) -> f64 {
    (0.5 * ((s[0] - s[1]).powi(2) + (s[1] - s[2]).powi(2) + (s[2] - s[0]).powi(2))
        + 3.0 * (s[3] * s[3] + s[4] * s[4] + s[5] * s[5]))
        .sqrt()
}

/// Eigen-decomposition of a symmetric 3x3 matrix (Jacobi). Returns ascending
/// eigenvalues and the matching unit eigenvectors as rows.
pub fn eig_sym3(s: &M3) -> ([f64; 3], M3) {
    let mut a = *s;
    let mut v = IDENTITY3;
    for _ in 0..60 {
        let (mut p, mut q, mut big) = (0, 1, a[0][1].abs());
        if a[0][2].abs() > big {
            (p, q, big) = (0, 2, a[0][2].abs());
        }
        if a[1][2].abs() > big {
            (p, q, big) = (1, 2, a[1][2].abs());
        }
        let scale = a[0][0].abs() + a[1][1].abs() + a[2][2].abs() + 1e-300;
        if big < 1e-15 * scale {
            break;
        }
        let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
        let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
        let t = if theta == 0.0 { 1.0 } else { t };
        let c = 1.0 / (t * t + 1.0).sqrt();
        let sn = t * c;
        for k in 0..3 {
            let (akp, akq) = (a[k][p], a[k][q]);
            a[k][p] = c * akp - sn * akq;
            a[k][q] = sn * akp + c * akq;
        }
        for k in 0..3 {
            let (apk, aqk) = (a[p][k], a[q][k]);
            a[p][k] = c * apk - sn * aqk;
            a[q][k] = sn * apk + c * aqk;
        }
        for k in 0..3 {
            let (vkp, vkq) = (v[k][p], v[k][q]);
            v[k][p] = c * vkp - sn * vkq;
            v[k][q] = sn * vkp + c * vkq;
        }
    }
    let mut idx = [0usize, 1, 2];
    idx.sort_by(|&i, &j| a[i][i].total_cmp(&a[j][j]));
    let vals = [a[idx[0]][idx[0]], a[idx[1]][idx[1]], a[idx[2]][idx[2]]];
    let mut vecs = [[0.0; 3]; 3];
    for (r, &i) in idx.iter().enumerate() {
        vecs[r] = [v[0][i], v[1][i], v[2][i]];
    }
    (vals, vecs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_round_trip() {
        let mut m = [[0.0; 6]; 6];
        for i in 0..6 {
            for j in 0..6 {
                m[i][j] = if i == j { 4.0 + i as f64 } else { 0.1 * (i + j) as f64 };
            }
        }
        let inv = invert6(&m).unwrap();
        let id = mul6(&m, &inv);
        for i in 0..6 {
            for j in 0..6 {
                assert!((id[i][j] - if i == j { 1.0 } else { 0.0 }).abs() < 1e-12);
            }
        }
        assert!(invert6(&[[0.0; 6]; 6]).is_none());
    }

    #[test]
    fn eigen_known_matrix() {
        let (vals, vecs) = eig_sym3(&[[2.0, 0.0, 0.0], [0.0, 3.0, 4.0], [0.0, 4.0, 9.0]]);
        assert!((vals[0] - 1.0).abs() < 1e-12);
        assert!((vals[1] - 2.0).abs() < 1e-12);
        assert!((vals[2] - 11.0).abs() < 1e-12);
        let top = vecs[2];
        assert!((top[1] * top[1] + top[2] * top[2] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn rotation_preserves_von_mises() {
        let s = [10.0, -3.0, 4.0, 2.0, -1.0, 5.0];
        let c = 0.5f64.sqrt();
        let q = [[c, c, 0.0], [-c, c, 0.0], [0.0, 0.0, 1.0]];
        let r = rotate_stress(&s, &q);
        assert!((von_mises(&r) - von_mises(&s)).abs() < 1e-12);
    }

    #[test]
    fn stiffness_rotation_is_identity_for_isotropic() {
        let (e, nu) = (3000.0, 0.35);
        let l = e * nu / ((1.0 + nu) * (1.0 - 2.0 * nu));
        let mu = e / (2.0 * (1.0 + nu));
        let mut d = [[0.0; 6]; 6];
        for i in 0..3 {
            for j in 0..3 {
                d[i][j] = l + if i == j { 2.0 * mu } else { 0.0 };
            }
            d[i + 3][i + 3] = mu;
        }
        let c = 0.5f64.sqrt();
        let q = [[c, c, 0.0], [-c, c, 0.0], [0.0, 0.0, 1.0]];
        let g = rotate_stiffness_to_global(&d, &q).unwrap();
        for i in 0..6 {
            for j in 0..6 {
                assert!((g[i][j] - d[i][j]).abs() < 1e-8, "{i} {j}");
            }
        }
    }
}

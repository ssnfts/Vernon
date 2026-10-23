//! A small CSR matrix and a Jacobi-preconditioned conjugate gradient
//! solver for the symmetric positive definite systems of the flattener.
//! Everything is serial, so results do not depend on the thread count.

#[derive(Clone, Debug)]
pub struct Csr {
    pub n: usize,
    pub row_ptr: Vec<usize>,
    pub col: Vec<u32>,
    pub val: Vec<f64>,
}

impl Csr {
    /// Build from `(row, col, value)` triplets; duplicates are summed in a
    /// fixed order.
    pub fn from_triplets(n: usize, mut t: Vec<(u32, u32, f64)>) -> Csr {
        t.sort_by_key(|e| (e.0, e.1));
        let mut row_ptr = vec![0usize; n + 1];
        let mut col: Vec<u32> = Vec::with_capacity(t.len());
        let mut val: Vec<f64> = Vec::with_capacity(t.len());
        let mut last: Option<(u32, u32)> = None;
        for (r, c, v) in t {
            if last == Some((r, c)) {
                *val.last_mut().expect("entry exists") += v;
            } else {
                col.push(c);
                val.push(v);
                row_ptr[r as usize + 1] += 1;
                last = Some((r, c));
            }
        }
        for i in 0..n {
            row_ptr[i + 1] += row_ptr[i];
        }
        Csr {
            n,
            row_ptr,
            col,
            val,
        }
    }

    pub fn mul(&self, x: &[f64], y: &mut [f64]) {
        for (i, yi) in y.iter_mut().enumerate().take(self.n) {
            let mut s = 0.0;
            for k in self.row_ptr[i]..self.row_ptr[i + 1] {
                s += self.val[k] * x[self.col[k] as usize];
            }
            *yi = s;
        }
    }

    pub fn diag(&self) -> Vec<f64> {
        (0..self.n)
            .map(|i| {
                (self.row_ptr[i]..self.row_ptr[i + 1])
                    .find(|&k| self.col[k] as usize == i)
                    .map_or(0.0, |k| self.val[k])
            })
            .collect()
    }
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Solve `A x = b` for SPD `A`, starting from the contents of `x`.
/// Returns `(iterations, relative residual)`.
pub fn pcg(a: &Csr, b: &[f64], x: &mut [f64], tol: f64, max_iter: usize) -> (usize, f64) {
    let n = a.n;
    let dinv: Vec<f64> = a
        .diag()
        .iter()
        .map(|d| if d.abs() > 1e-300 { 1.0 / d } else { 1.0 })
        .collect();
    let bn = dot(b, b).sqrt().max(1e-300);
    let mut ax = vec![0.0; n];
    a.mul(x, &mut ax);
    let mut r: Vec<f64> = b.iter().zip(&ax).map(|(bi, ai)| bi - ai).collect();
    let mut z: Vec<f64> = r.iter().zip(&dinv).map(|(ri, di)| ri * di).collect();
    let mut p = z.clone();
    let mut rz = dot(&r, &z);
    let mut it = 0;
    let mut rel = dot(&r, &r).sqrt() / bn;
    let mut ap = vec![0.0; n];
    while it < max_iter && rel > tol {
        a.mul(&p, &mut ap);
        let pap = dot(&p, &ap);
        if pap.abs() < 1e-300 {
            break;
        }
        let alpha = rz / pap;
        for i in 0..n {
            x[i] += alpha * p[i];
            r[i] -= alpha * ap[i];
        }
        for i in 0..n {
            z[i] = r[i] * dinv[i];
        }
        let rz_new = dot(&r, &z);
        let beta = rz_new / rz;
        rz = rz_new;
        for i in 0..n {
            p[i] = z[i] + beta * p[i];
        }
        rel = dot(&r, &r).sqrt() / bn;
        it += 1;
    }
    (it, rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_a_grid_laplacian_like_a_dense_solve() {
        // 1-D Laplacian with Dirichlet ends on 30 unknowns.
        let n = 30;
        let mut t = Vec::new();
        for i in 0..n {
            t.push((i as u32, i as u32, 2.0));
            if i > 0 {
                t.push((i as u32, i as u32 - 1, -1.0));
            }
            if i + 1 < n {
                t.push((i as u32, i as u32 + 1, -1.0));
            }
        }
        let a = Csr::from_triplets(n, t);
        let b: Vec<f64> = (0..n).map(|i| (i as f64 * 0.37).sin()).collect();
        let mut x = vec![0.0; n];
        let (_, rel) = pcg(&a, &b, &mut x, 1e-12, 1000);
        assert!(rel < 1e-10);
        // residual check against a direct product
        let mut ax = vec![0.0; n];
        a.mul(&x, &mut ax);
        let err = ax
            .iter()
            .zip(&b)
            .map(|(p, q)| (p - q).abs())
            .fold(0.0, f64::max);
        assert!(err < 1e-8, "{err}");
        // dense Thomas solve for the same system
        let (mut c, mut d) = (vec![0.0; n], vec![0.0; n]);
        c[0] = -0.5;
        d[0] = b[0] / 2.0;
        for i in 1..n {
            let m = 2.0 + c[i - 1];
            c[i] = -1.0 / m;
            d[i] = (b[i] + d[i - 1]) / m;
        }
        let mut xs = vec![0.0; n];
        xs[n - 1] = d[n - 1];
        for i in (0..n - 1).rev() {
            xs[i] = d[i] - c[i] * xs[i + 1];
        }
        let diff = x
            .iter()
            .zip(&xs)
            .map(|(p, q)| (p - q).abs())
            .fold(0.0, f64::max);
        assert!(diff < 1e-8, "{diff}");
    }
}

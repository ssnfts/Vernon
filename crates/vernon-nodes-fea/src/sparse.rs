//! Compressed sparse row matrices and a preconditioned conjugate-gradient
//! solve for symmetric positive definite systems.
//!
//! The preconditioner is incomplete Cholesky with no fill (Meijerink and van
//! der Vorst 1977). Elasticity matrices are not M-matrices, so the
//! factorisation can break down; it is then retried with a growing diagonal
//! shift (Manteuffel 1980), and Jacobi is the last resort.

// a NaN pivot, curvature or residual must take the failure branch, so `!(x > y)` is meant
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use rayon::prelude::*;

#[derive(Debug, Clone)]
pub struct Csr {
    ptr: Vec<usize>,
    col: Vec<u32>,
    val: Vec<f64>,
}

impl Csr {
    /// Zero matrix with the given column indices per row (any order, repeats allowed).
    pub fn from_pattern(mut rows: Vec<Vec<u32>>) -> Self {
        let mut ptr = Vec::with_capacity(rows.len() + 1);
        ptr.push(0);
        let mut col = Vec::new();
        for r in &mut rows {
            r.sort_unstable();
            r.dedup();
            col.extend_from_slice(r);
            ptr.push(col.len());
        }
        let val = vec![0.0; col.len()];
        Csr { ptr, col, val }
    }

    pub fn n(&self) -> usize {
        self.ptr.len() - 1
    }

    pub fn nnz(&self) -> usize {
        self.col.len()
    }

    /// Add to an entry of the pattern; anything outside it is a programming error.
    pub fn add(&mut self, i: usize, j: usize, v: f64) {
        let row = self.ptr[i]..self.ptr[i + 1];
        let at = self.col[row.clone()].binary_search(&(j as u32)).expect("entry is in the sparsity pattern");
        self.val[row.start + at] += v;
    }

    /// `y = A x`. Rows are independent, so the result does not depend on the thread count.
    pub fn mul(&self, x: &[f64], y: &mut [f64]) {
        y.par_iter_mut().enumerate().for_each(|(i, out)| {
            let row = self.ptr[i]..self.ptr[i + 1];
            *out = self.col[row.clone()].iter().zip(&self.val[row]).map(|(&j, v)| v * x[j as usize]).sum();
        });
    }

    /// The rows and columns whose `keep` flag is set, renumbered in order.
    pub fn submatrix(&self, keep: &[bool]) -> Csr {
        let mut map = vec![u32::MAX; keep.len()];
        let mut next = 0;
        for (m, k) in map.iter_mut().zip(keep) {
            if *k {
                *m = next;
                next += 1;
            }
        }
        let mut out = Csr { ptr: vec![0], col: Vec::new(), val: Vec::new() };
        for i in (0..self.n()).filter(|&i| keep[i]) {
            for p in self.ptr[i]..self.ptr[i + 1] {
                let j = map[self.col[p] as usize];
                if j != u32::MAX {
                    out.col.push(j);
                    out.val.push(self.val[p]);
                }
            }
            out.ptr.push(out.col.len());
        }
        out
    }

    fn diagonal(&self) -> Vec<f64> {
        (0..self.n())
            .map(|i| {
                let row = self.ptr[i]..self.ptr[i + 1];
                self.col[row.clone()].binary_search(&(i as u32)).map_or(0.0, |at| self.val[row.start + at])
            })
            .collect()
    }

    /// Lower triangle (diagonal last in each row) of the IC(0) factor with the
    /// diagonal scaled by `1 + shift`; `None` on breakdown.
    fn ic0(&self, shift: f64) -> Option<Csr> {
        let mut l = Csr { ptr: vec![0], col: Vec::new(), val: Vec::new() };
        for i in 0..self.n() {
            for p in self.ptr[i]..self.ptr[i + 1] {
                if self.col[p] as usize <= i {
                    l.col.push(self.col[p]);
                    l.val.push(self.val[p]);
                }
            }
            l.ptr.push(l.col.len());
        }
        for i in 0..l.n() {
            let start = l.ptr[i];
            let end = l.ptr[i + 1];
            if end == start || l.col[end - 1] as usize != i {
                return None;
            }
            for p in start..end {
                let k = l.col[p] as usize;
                // sum of L(i,j) L(k,j) over the common columns j < k
                let (mut a, mut b) = (start, l.ptr[k]);
                let mut sum = 0.0;
                while a < p && l.col[b] < l.col[p] {
                    match l.col[a].cmp(&l.col[b]) {
                        std::cmp::Ordering::Less => a += 1,
                        std::cmp::Ordering::Greater => b += 1,
                        std::cmp::Ordering::Equal => {
                            sum += l.val[a] * l.val[b];
                            a += 1;
                            b += 1;
                        }
                    }
                }
                if k < i {
                    l.val[p] = (l.val[p] - sum) / l.val[l.ptr[k + 1] - 1];
                } else {
                    let pivot = l.val[p] * (1.0 + shift);
                    let rest = pivot - sum;
                    if !(rest > 1e-12 * pivot) {
                        return None;
                    }
                    l.val[p] = rest.sqrt();
                }
            }
        }
        Some(l)
    }
}

enum Precond {
    Ic0(Csr),
    Jacobi(Vec<f64>),
}

impl Precond {
    fn build(a: &Csr) -> Precond {
        for shift in [0.0, 1e-3, 1e-2, 1e-1] {
            if let Some(l) = a.ic0(shift) {
                return Precond::Ic0(l);
            }
        }
        Precond::Jacobi(a.diagonal().iter().map(|d| if *d > 0.0 { 1.0 / d } else { 0.0 }).collect())
    }

    /// `z = M^-1 r`.
    fn apply(&self, r: &[f64], z: &mut [f64]) {
        match self {
            Precond::Jacobi(inv) => z.iter_mut().zip(r).zip(inv).for_each(|((z, r), d)| *z = r * d),
            Precond::Ic0(l) => {
                for i in 0..l.n() {
                    let (start, last) = (l.ptr[i], l.ptr[i + 1] - 1);
                    let s: f64 = (start..last).map(|p| l.val[p] * z[l.col[p] as usize]).sum();
                    z[i] = (r[i] - s) / l.val[last];
                }
                for i in (0..l.n()).rev() {
                    let (start, last) = (l.ptr[i], l.ptr[i + 1] - 1);
                    z[i] /= l.val[last];
                    let zi = z[i];
                    for p in start..last {
                        z[l.col[p] as usize] -= l.val[p] * zi;
                    }
                }
            }
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Precond::Ic0(_) => "incomplete Cholesky",
            Precond::Jacobi(_) => "Jacobi",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PcgSolution {
    pub x: Vec<f64>,
    pub iterations: usize,
    /// Relative residual `|b - A x| / |b|` of the recurrence.
    pub residual: f64,
    pub preconditioner: &'static str,
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Solve `A x = b` from a zero start. A curvature that is not positive, or a
/// residual still above `max(tol, 1e-6)` at the iteration limit, is an error:
/// both mean the system is singular or close to it.
pub fn pcg(a: &Csr, b: &[f64], tol: f64) -> Result<PcgSolution, String> {
    let n = a.n();
    let precond = Precond::build(a);
    let mut x = vec![0.0; n];
    let bnorm = dot(b, b).sqrt();
    let (mut iterations, mut residual) = (0, 0.0);
    if bnorm > 0.0 {
        let mut r = b.to_vec();
        let mut z = vec![0.0; n];
        precond.apply(&r, &mut z);
        let mut p = z.clone();
        let mut rz = dot(&r, &z);
        let mut ap = vec![0.0; n];
        let max_it = (10 * n).clamp(200, 20000);
        while iterations < max_it {
            a.mul(&p, &mut ap);
            let pap = dot(&p, &ap);
            if !(pap > 0.0) {
                return Err("the stiffness matrix is not positive definite: the body is under-constrained".into());
            }
            let alpha = rz / pap;
            x.iter_mut().zip(&p).for_each(|(x, p)| *x += alpha * p);
            r.iter_mut().zip(&ap).for_each(|(r, ap)| *r -= alpha * ap);
            iterations += 1;
            residual = dot(&r, &r).sqrt() / bnorm;
            if residual < tol {
                break;
            }
            precond.apply(&r, &mut z);
            let rz_new = dot(&r, &z);
            let beta = rz_new / rz;
            rz = rz_new;
            p.iter_mut().zip(&z).for_each(|(p, z)| *p = z + beta * *p);
        }
        if !(residual < tol.max(1e-6)) {
            return Err(format!("the solver did not converge (relative residual {residual:.2e}): the body may be under-constrained"));
        }
    }
    Ok(PcgSolution { x, iterations, residual, preconditioner: precond.name() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1D Laplacian: a tridiagonal SPD matrix with a known solution.
    fn laplacian(n: usize) -> Csr {
        let rows = (0..n).map(|i| (i.saturating_sub(1)..(i + 2).min(n)).map(|j| j as u32).collect()).collect();
        let mut a = Csr::from_pattern(rows);
        for i in 0..n {
            a.add(i, i, 2.0);
            if i + 1 < n {
                a.add(i, i + 1, -1.0);
                a.add(i + 1, i, -1.0);
            }
        }
        a
    }

    #[test]
    fn ic0_is_exact_without_fill_and_converges_at_once() {
        // a tridiagonal factor has no fill, so the preconditioner is the exact inverse
        let a = laplacian(50);
        let want: Vec<f64> = (0..50).map(|i| (i as f64 * 0.3).sin()).collect();
        let mut b = vec![0.0; 50];
        a.mul(&want, &mut b);
        let s = pcg(&a, &b, 1e-12).unwrap();
        assert_eq!(s.preconditioner, "incomplete Cholesky");
        assert!(s.iterations <= 2, "{}", s.iterations);
        assert!(s.x.iter().zip(&want).all(|(x, w)| (x - w).abs() < 1e-9));
    }

    #[test]
    fn a_matrix_that_is_not_positive_definite_falls_back_and_is_refused() {
        let mut a = Csr::from_pattern(vec![vec![0, 1], vec![0, 1]]);
        a.add(0, 0, 1.0);
        a.add(1, 1, -1.0);
        assert!(a.ic0(0.0).is_none());
        assert!(pcg(&a, &[1.0, 1.0], 1e-10).is_err());
    }

    #[test]
    fn a_singular_system_is_refused() {
        // both rows the same: the right-hand side is not in the range
        let mut a = Csr::from_pattern(vec![vec![0, 1], vec![0, 1]]);
        for (i, j) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            a.add(i, j, 1.0);
        }
        assert!(pcg(&a, &[1.0, -1.0], 1e-10).is_err());
    }

    #[test]
    fn submatrix_keeps_the_selected_rows_and_columns() {
        let s = laplacian(4).submatrix(&[true, false, true, true]);
        assert_eq!((s.n(), s.nnz()), (3, 5));
        let mut y = [0.0; 3];
        s.mul(&[1.0, 1.0, 1.0], &mut y);
        assert_eq!(y, [2.0, 1.0, 1.0]);
    }
}

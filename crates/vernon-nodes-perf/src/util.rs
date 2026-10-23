//! Small shared helpers: tables, list parsing, dense solves, input/parameter plumbing.

use std::sync::Arc;

use serde_json::{json, Value};
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params};

use crate::physics::{Env, Vehicle};

/// Signature of a node's cook function.
pub type CookFn = fn(&CookContext, &[Option<Data>], &Params) -> CookResult<Vec<Data>>;

/// Node built from two plain functions, so each node is a few lines of boilerplate.
pub struct FnNode {
    info: fn() -> NodeTypeInfo,
    cook: CookFn,
}

impl FnNode {
    pub fn new(info: fn() -> NodeTypeInfo, cook: CookFn) -> Self {
        Self { info, cook }
    }
}

impl NodeOp for FnNode {
    fn info(&self) -> NodeTypeInfo {
        (self.info)()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        (self.cook)(ctx, inputs, params)
    }
}

pub const SPEC: &str = "docs/specs/performance-validation.md";

/// A table in the shared JSON form: named columns with units, rows of numbers.
pub fn table_json(columns: &[(&str, &str)], rows: Vec<Vec<f64>>) -> Value {
    json!({
        "kind": "table.v1",
        "columns": columns.iter().map(|(n, u)| json!({"name": n, "unit": u})).collect::<Vec<_>>(),
        "rows": rows,
    })
}

/// Parse a list of numbers separated by commas, semicolons or spaces.
pub fn parse_list(s: &str) -> Result<Vec<f64>, String> {
    s.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(|t| t.parse::<f64>().map_err(|_| format!("'{t}' is not a number")))
        .collect()
}

/// Solve `A x = b` by Gaussian elimination with partial pivoting.
pub fn lin_solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for c in 0..n {
        let p = (c..n).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))?;
        if a[p][c].abs() < 1e-300 {
            return None;
        }
        a.swap(c, p);
        b.swap(c, p);
        for r in c + 1..n {
            let f = a[r][c] / a[c][c];
            let pivot_row = a[c].clone();
            for (x, p) in a[r].iter_mut().zip(&pivot_row).skip(c) {
                *x -= f * p;
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let s: f64 = (r + 1..n).map(|k| a[r][k] * x[k]).sum();
        x[r] = (b[r] - s) / a[r][r];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Invert a small dense matrix.
pub fn mat_inv(a: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    let mut cols = Vec::with_capacity(n);
    for j in 0..n {
        let mut e = vec![0.0; n];
        e[j] = 1.0;
        cols.push(lin_solve(a.to_vec(), e)?);
    }
    Some((0..n).map(|i| (0..n).map(|j| cols[j][i]).collect()).collect())
}

pub fn mean(x: &[f64]) -> f64 {
    if x.is_empty() {
        0.0
    } else {
        x.iter().sum::<f64>() / x.len() as f64
    }
}

/// Sample standard deviation (n - 1).
pub fn sd(x: &[f64]) -> f64 {
    if x.len() < 2 {
        return 0.0;
    }
    let m = mean(x);
    (x.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (x.len() - 1) as f64).sqrt()
}

pub fn median(x: &[f64]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    let mut s = x.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let n = s.len();
    if n % 2 == 1 {
        s[n / 2]
    } else {
        0.5 * (s[n / 2 - 1] + s[n / 2])
    }
}

/// Two-sided 95 percent Student t quantile for `df` degrees of freedom.
pub fn t975(df: usize) -> f64 {
    const T: [f64; 30] = [
        12.706, 4.303, 3.182, 2.776, 2.571, 2.447, 2.365, 2.306, 2.262, 2.228, 2.201, 2.179, 2.160, 2.145, 2.131,
        2.120, 2.110, 2.101, 2.093, 2.086, 2.080, 2.074, 2.069, 2.064, 2.060, 2.056, 2.052, 2.048, 2.045, 2.042,
    ];
    match df {
        0 => f64::INFINITY,
        1..=30 => T[df - 1],
        31..=60 => 2.0,
        _ => 1.96,
    }
}

/// Fallback-density parameter shared by every node that needs air density.
pub fn rho_param() -> ParamSpec {
    ParamSpec::float("rho_kgm3", "Air density (no env input)", 1.225, 0.5, 1.5)
        .unit("kg/m3")
        .help("Density used when no air-state input is connected")
}

/// JSON value of a connected input, or `None` when unconnected.
pub fn json_in(inputs: &[Option<Data>], i: usize) -> CookResult<Option<Arc<Value>>> {
    match inputs.get(i).and_then(|d| d.as_ref()) {
        None => Ok(None),
        Some(Data::Json(v)) => Ok(Some(v.clone())),
        Some(other) => Err(CookError::WrongInput { index: i, expected: DataKind::Json, found: other.kind() }),
    }
}

/// Air state: the connected `env` input, else the `rho_kgm3` parameter.
pub fn env_in(ctx: &CookContext, inputs: &[Option<Data>], i: usize, p: &Params) -> CookResult<Env> {
    match json_in(inputs, i)? {
        Some(v) => Env::from_json(&v).map_err(|e| ctx.fail(e)),
        None => {
            let rho = p.float("rho_kgm3")?;
            if !(rho.is_finite() && rho > 0.0) {
                return Err(ctx.fail("air density must be positive"));
            }
            Ok(Env::from_rho(rho))
        }
    }
}

/// The connected `vehicle` input, if any.
pub fn vehicle_in(ctx: &CookContext, inputs: &[Option<Data>], i: usize) -> CookResult<Option<Vehicle>> {
    match json_in(inputs, i)? {
        Some(v) => Ok(Some(Vehicle::from_json(&v).map_err(|e| ctx.fail(e))?)),
        None => Ok(None),
    }
}

/// Convert a `Result<_, String>` from the numeric code into a cook failure.
pub trait OrFail<T> {
    fn or_fail(self, ctx: &CookContext) -> CookResult<T>;
}

impl<T> OrFail<T> for Result<T, String> {
    fn or_fail(self, ctx: &CookContext) -> CookResult<T> {
        self.map_err(|e| ctx.fail(e))
    }
}

/// Reject non-finite results with a clear message.
pub fn finite(ctx: &CookContext, what: &str, v: f64) -> CookResult<f64> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(ctx.fail(format!("{what} is not finite")))
    }
}

/// Read a text file, or fail with the path in the message.
pub fn read_text(ctx: &CookContext, path: &std::path::Path) -> CookResult<String> {
    std::fs::read_to_string(path).map_err(|e| ctx.fail(format!("cannot read {}: {e}", path.display())))
}

//! Stretch-aware flattening constraints: a local/global solver whose local
//! step blends per-material projections of every triangle's 2x2 Jacobian,
//! with hard one-sided (stretch-only / compress-only) and boundary-length
//! constraints and a fold-free line search on the global step.
//!
//! Standalone functions over plain slices (`[[f64; 3]]` positions in mm,
//! `[[usize; 3]]` triangles, `[[f64; 2]]` flat coordinates in mm).
//!
//! Method, from the papers (equation numbers are not quoted):
//! * Liu, Zhang, Xu, Liu and Gortler, "A Local/Global Approach to Mesh
//!   Parameterization", SGP 2008: alternate between a *local* step that finds,
//!   per triangle, a target matrix `L_t` from the current Jacobian `J_t` and a
//!   *global* step that minimises `sum_t w_t A_t |J_t - L_t|_F^2`, a sparse
//!   linear system with the cotangent Laplacian of the triangles' own
//!   isometric frames. The identity `A |M|_F^2 = 1/2 sum_e cot(theta_e) |M e|^2`
//!   for a linear map `M` on a triangle turns the energy into edge form.
//! * Bouaziz, Deuss, Schwartzburg, Weise and Pauly, "Shape-Up: Shaping
//!   Discrete Geometry with Projections", SGP 2012, and Bouaziz, Martin, Liu,
//!   Kavan and Pauly, "Projective Dynamics", SIGGRAPH 2014: several
//!   constraints, each with its own weight and its own projection, share one
//!   global solve; the target of the global step is the weighted mean of the
//!   projections. Used here for the material: the projection onto rotations
//!   (edge lengths kept), onto unit-determinant maps (area kept) and onto
//!   similarities (no shear), weighted per material; and for the boundary:
//!   each boundary edge is projected onto its 3-D length.
//! * Overby, Brown, Li and Narain, "ADMM ⊇ Projective Dynamics: Fast
//!   Simulation of Hyperelastic Models with Dynamic Constraints", IEEE TVCG
//!   2017: hard constraints inside a local/global loop through a scaled dual
//!   variable that is added to the projection target and updated with the
//!   remaining violation after every global step (augmented Lagrangian). Used
//!   here for the one-sided singular-value bounds and the boundary lengths,
//!   so they are constraints, not large weights.
//! * Smith and Schaefer, "Bijective Parameterization with Free Boundaries",
//!   SIGGRAPH 2015: pick the step length so no triangle reaches zero signed
//!   area. A triangle's signed area along the step is an exact quadratic in
//!   the step length, so the largest safe step is the smallest positive root;
//!   the step actually taken is the full step when that is safe, else a
//!   fraction of the root. (The paper's barrier energy and its free-boundary
//!   overlap check are not implemented.)
//!
//! The dual iteration drives the violations towards zero but cannot reach
//! exactly zero in finitely many steps, so a last uniform scale about the
//! pinned vertex makes the constraint exact: by `1 / min s2` (stretch-only),
//! `1 / max s1` (compress-only) or the 3-D over flat boundary length
//! (preserve boundary). A uniform scale multiplies every singular value by
//! the same factor, so after it the one-sided bound holds on every triangle
//! to rounding; the factor is reported in [`Output::final_scale`] and is
//! close to 1 when the dual iteration has converged.
//!
//! Stretch/compress factors ([`EdgeFactors`]) bias the edge term one way, so
//! a larger factor makes that one-sided deviation more expensive and it
//! shrinks. Without a one-sided mode the edge term is the global quadratic:
//! in every global step each triangle edge's weight is multiplied by the
//! stretch factor when the edge is currently longer than in 3-D and by the
//! compress factor when it is shorter (separately for boundary and interior
//! edges), an iteratively reweighted least-squares step as in the plain
//! as-rigid-as-possible map, with the cotangents clamped positive so every
//! weighted triangle stays positive semi-definite. With all factors at 1 the
//! weights are the exact cotangents and the matrix is built once.
//!
//! In a one-sided mode the factors are not applied. There the material
//! energy lives in the ADMM local step, so reweighting the global step only
//! changes the penalty metric, not the fixed point (measured: a boundary
//! stretch factor of 10 moved the F2 cap's boundary strain from 4.7440 % to
//! 4.7469 %). Weighting the local step's edge term instead left the bound
//! unmet within the iteration limit, and the final uniform scale then
//! stretched every triangle (interior stretch 1.35 % -> 2.28 % with an
//! interior stretch factor of 10, the opposite of the request).
//!
//! Orientation: triangles must be counter-clockwise seen from the side the
//! normal `(v1-v0) x (v2-v0)` points to, and the start layout must have
//! positive signed area everywhere (unless [`Params::allow_flipped_start`]);
//! the line search keeps every positive triangle positive.

/// Which side of the singular values is constrained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// No hard bound: the material target alone.
    Rigid,
    /// Hard bound `s2 >= 1` on every triangle: the sheet may stretch but not compress.
    StretchOnly,
    /// Hard bound `s1 <= 1` on every triangle: the sheet may compress but not stretch.
    CompressOnly,
}

/// Weights of the three projections blended in the local step (Projective
/// Dynamics style). Values are Vernon choices; the spec states only which
/// term dominates for each material.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaterialWeights {
    /// Projection onto rotations: keeps edge lengths (membrane strain).
    pub edge: f64,
    /// Projection onto unit-determinant maps: keeps triangle areas.
    pub area: f64,
    /// Projection onto similarities: removes shear (angle distortion).
    pub shear: f64,
}

impl MaterialWeights {
    /// Edge term only: the plain as-rigid-as-possible energy.
    pub const EDGE_ONLY: Self = Self { edge: 1.0, area: 0.0, shear: 0.0 };
    /// Stiff sheet (spec 3.5 step 4, Vernon numbers): area term 1, edge term 0.3.
    pub const RIGID: Self = Self { edge: 0.3, area: 1.0, shear: 0.0 };
    /// Pliable fabric (spec 3.5 step 4, Vernon numbers): edge term 1, area term
    /// 0.1 and a shear penalty of 0.3.
    pub const FLOPPY: Self = Self { edge: 1.0, area: 0.1, shear: 0.3 };

    fn total(&self) -> f64 {
        self.edge + self.area + self.shear
    }
}

/// Weights on the one-sided edge terms: how much more an edge that is
/// stretched (or compressed) costs than one at its 3-D length. All 1 is the
/// unbiased energy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeFactors {
    pub boundary_stretch: f64,
    pub boundary_compress: f64,
    pub interior_stretch: f64,
    pub interior_compress: f64,
}

impl EdgeFactors {
    pub const NONE: Self = Self { boundary_stretch: 1.0, boundary_compress: 1.0, interior_stretch: 1.0, interior_compress: 1.0 };

    /// True when every factor is 1.
    pub fn is_uniform(&self) -> bool {
        [self.boundary_stretch, self.boundary_compress, self.interior_stretch, self.interior_compress].iter().all(|f| *f == 1.0)
    }

    /// The factor for an edge of flat/3-D length ratio `r`.
    fn of(&self, boundary: bool, r: f64) -> f64 {
        match (boundary, r > 1.0) {
            (true, true) => self.boundary_stretch,
            (true, false) => self.boundary_compress,
            (false, true) => self.interior_stretch,
            (false, false) => self.interior_compress,
        }
    }
}

/// Solver settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    pub mode: Mode,
    pub material: MaterialWeights,
    /// Weight of the boundary-length constraint relative to the material
    /// terms; 0 leaves the boundary free. When it is on, the total boundary
    /// length is made exact by the final scale (unless a one-sided mode needs
    /// that scale).
    pub boundary_weight: f64,
    pub max_iterations: usize,
    /// Stop when the largest vertex move of an iteration is below
    /// `tolerance * bounding box diagonal` of the layout and the constraint
    /// violation is below `constraint_tolerance`.
    pub tolerance: f64,
    /// Largest remaining violation (singular value or relative boundary edge
    /// length) accepted as converged before the final scale.
    pub constraint_tolerance: f64,
    /// Fraction of the largest fold-free step taken when the full step would fold, in (0, 1).
    pub step_fraction: f64,
    /// Vertex held at its start position (removes the translation freedom).
    pub pin: usize,
    /// Relative tolerance below which the final scale is skipped.
    pub edge_tolerance: f64,
    /// Accept a start layout with flipped triangles: they are left out of the
    /// line search (and counted in [`Output::flipped`] if still flipped).
    pub allow_flipped_start: bool,
    /// Stretch/compress weights on the edge term; [`EdgeFactors::NONE`] leaves it
    /// unbiased. Not applied in a one-sided mode (see the module notes).
    pub edge_factors: EdgeFactors,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            mode: Mode::Rigid,
            material: MaterialWeights::EDGE_ONLY,
            boundary_weight: 0.0,
            max_iterations: 200,
            tolerance: 1e-10,
            constraint_tolerance: 1e-6,
            step_fraction: 0.8,
            pin: 0,
            edge_tolerance: 1e-9,
            allow_flipped_start: false,
            edge_factors: EdgeFactors::NONE,
        }
    }
}

/// Result of [`flatten_constrained`].
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub uv: Vec<[f64; 2]>,
    pub iterations: usize,
    /// The vertex moves and the constraint violation fell below the
    /// tolerances before the iteration limit.
    pub converged: bool,
    /// Material energy `sum_t w_t A_t |J_t - L_t|^2` of the last iteration
    /// (plus the boundary term when it is on), before the final scale.
    pub energy: f64,
    /// Singular values `[s1, s2]` of every triangle's Jacobian in the result
    /// (`s1 >= s2`; zero-area triangles report `[1, 1]`).
    pub sigma: Vec<[f64; 2]>,
    /// Smallest and largest edge length ratio (flat / 3-D) over all edges.
    pub min_edge_ratio: f64,
    pub max_edge_ratio: f64,
    /// Uniform scale applied last to make a hard constraint exact (1 when none).
    pub final_scale: f64,
    /// Largest violation left by the iteration, before the final scale.
    pub violation_before_scale: f64,
    /// Flat over 3-D boundary length after the final scale (1 for a mesh without boundary).
    pub boundary_ratio: f64,
    /// Iterations whose full global step would have folded a triangle, so a shorter step was taken.
    pub shortened_steps: usize,
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

/// Closest `[a, 1/a]` (unit determinant) to the singular values `[s1, s2]`:
/// the stationary point of `(a - s1)^2 + (1/a - s2)^2`, i.e. the root of
/// `a^4 - s1 a^3 + s2 a - 1 = 0`, by safeguarded Newton from `sqrt(s1/s2)`.
fn area_projection(s1: f64, s2: f64) -> [f64; 2] {
    let mut a = if s2 > 1e-9 { (s1 / s2).sqrt() } else { s1.max(1.0) };
    for _ in 0..30 {
        let f = a.powi(4) - s1 * a.powi(3) + s2 * a - 1.0;
        let df = 4.0 * a.powi(3) - 3.0 * s1 * a * a + s2;
        if df.abs() < 1e-300 {
            break;
        }
        let next = a - f / df;
        let next = if next > 0.0 { next } else { 0.5 * a };
        if (next - a).abs() <= 1e-15 * a {
            a = next;
            break;
        }
        a = next;
    }
    let a = a.max(1.0 / a); // s1 >= s2, so the larger value goes first
    [a, 1.0 / a]
}

/// The material target singular values for the current `[s1, s2]`: the
/// weighted mean of the three projections.
pub fn material_target(s: [f64; 2], m: &MaterialWeights) -> [f64; 2] {
    let w = m.total();
    if w <= 0.0 {
        return [1.0, 1.0];
    }
    let ar = if m.area > 0.0 { area_projection(s[0], s[1]) } else { [1.0, 1.0] };
    let mean = 0.5 * (s[0] + s[1]);
    [0, 1].map(|i| (m.edge + m.area * ar[i] + m.shear * mean) / w)
}

/// Largest `alpha > 0` such that every triangle of `uv + alpha * step` keeps
/// twice its signed area above `min_area2` (or above half its start value
/// when that is smaller); `f64::INFINITY` when none ever gets there.
/// Triangles that are not positive at `alpha = 0` give 0.
pub fn max_step_keeping_area(tris: &[[usize; 3]], uv: &[[f64; 2]], step: &[[f64; 2]], min_area2: f64) -> f64 {
    let mut best = f64::INFINITY;
    for t in tris {
        let e1 = sub2(uv[t[1]], uv[t[0]]);
        let e2 = sub2(uv[t[2]], uv[t[0]]);
        let d1 = sub2(step[t[1]], step[t[0]]);
        let d2 = sub2(step[t[2]], step[t[0]]);
        let cr = |a: [f64; 2], b: [f64; 2]| a[0] * b[1] - a[1] * b[0];
        let (c0, c1, c2) = (cr(e1, e2), cr(e1, d2) + cr(d1, e2), cr(d1, d2));
        if c0 <= 0.0 {
            return 0.0;
        }
        let floor = min_area2.min(0.5 * c0);
        best = best.min(smallest_positive_root(c0 - floor, c1, c2));
    }
    best
}

/// Flatten with a material energy and hard constraints. `uv0` is the start
/// layout (positive signed area in every triangle, e.g. a conformal map);
/// `weights` holds one weight per triangle in `(0, 1]` (an empty slice means
/// all 1; values below 1e-6 are raised to 1e-6 to keep the system solvable).
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
    if p.material.total() <= 0.0 || [p.material.edge, p.material.area, p.material.shear].iter().any(|w| *w < 0.0) {
        return Err("material weights must be non-negative with a positive sum".into());
    }
    let ef = p.edge_factors;
    if [ef.boundary_stretch, ef.boundary_compress, ef.interior_stretch, ef.interior_compress].iter().any(|f| !(f.is_finite() && *f > 0.0)) {
        return Err("stretch/compress factors must be positive and finite".into());
    }
    let biased = !ef.is_uniform();
    let one_sided = p.mode != Mode::Rigid;
    // reweight the global step only where the edge energy is the global quadratic
    let biased_global = biased && !one_sided;
    let frames: Vec<Option<Frame>> = tris.iter().map(|t| frame(pos, t)).collect();
    let wsum = p.material.total();
    let w = |ti: usize| weights.get(ti).copied().unwrap_or(1.0).max(1e-6) * wsum;

    let mut uv = uv0.to_vec();
    let positive: Vec<bool> = tris.iter().zip(&frames).map(|(t, f)| f.is_some() && area2(uv[t[0]], uv[t[1]], uv[t[2]]) > 0.0).collect();
    if !p.allow_flipped_start && frames.iter().zip(&positive).any(|(f, ok)| f.is_some() && !ok) {
        return Err("the start layout has a flipped or degenerate triangle".into());
    }
    // the line search guards the triangles that start positive
    let guarded: Vec<[usize; 3]> = tris.iter().zip(&positive).filter(|(_, ok)| **ok).map(|(t, _)| *t).collect();
    let mean_area2 = {
        let (s, c) = frames.iter().flatten().fold((0.0, 0usize), |(s, c), f| (s + f.l * f.q, c + 1));
        if c > 0 { s / c as f64 } else { 0.0 }
    };
    let min_area2 = 1e-6 * mean_area2;

    // boundary edges (used by one triangle) with their 3-D lengths
    let boundary: Vec<(usize, usize, f64)> = {
        let mut count = std::collections::BTreeMap::new();
        for (t, f) in tris.iter().zip(&frames) {
            if f.is_none() {
                continue;
            }
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                *count.entry((a.min(b), a.max(b))).or_insert(0usize) += 1;
            }
        }
        count
            .into_iter()
            .filter(|(_, c)| *c == 1)
            .map(|((a, b), _)| (a, b, dot3(sub3(pos[a], pos[b]), sub3(pos[a], pos[b])).sqrt()))
            .collect()
    };
    let wb = if p.boundary_weight > 0.0 { p.boundary_weight * wsum } else { 0.0 };
    let on_boundary: std::collections::BTreeSet<(usize, usize)> = boundary.iter().map(|&(a, b, _)| (a, b)).collect();
    // Edge `k` of a triangle is the one opposite corner `k`: whether it is on the boundary,
    // and its 3-D length.
    let edge_info: Vec<[(bool, f64); 3]> = tris
        .iter()
        .map(|t| {
            std::array::from_fn(|k| {
                let (a, b) = (t[(k + 1) % 3], t[(k + 2) % 3]);
                (on_boundary.contains(&(a.min(b), a.max(b))), dot3(sub3(pos[a], pos[b]), sub3(pos[a], pos[b])).sqrt())
            })
        })
        .collect();
    // Edge weights of the global step: the cotangents, times the one-sided factor of each
    // edge's current length when the factors are biased (cotangents clamped positive then).
    let edge_weights = |uv: &[[f64; 2]]| -> Vec<[f64; 3]> {
        tris.iter()
            .enumerate()
            .map(|(ti, t)| {
                let Some(f) = frames[ti] else { return [0.0; 3] };
                let x = f.x();
                std::array::from_fn(|k| {
                    let cot = cot_at(&x, k);
                    if !biased_global {
                        return 0.5 * w(ti) * cot;
                    }
                    let (bnd, l3) = edge_info[ti][k];
                    let d = sub2(uv[t[(k + 1) % 3]], uv[t[(k + 2) % 3]]);
                    let r = d[0].hypot(d[1]) / l3.max(1e-300);
                    0.5 * w(ti) * cot.max(1e-4) * ef.of(bnd, r)
                })
            })
            .collect()
    };
    let system = |cw: &[[f64; 3]]| -> Csr {
        let mut trip = Vec::new();
        for (ti, t) in tris.iter().enumerate() {
            if frames[ti].is_none() {
                continue;
            }
            for k in 0..3 {
                let (vi, vj) = (t[(k + 1) % 3], t[(k + 2) % 3]);
                let c = cw[ti][k];
                trip.extend([(vi, vi, c), (vj, vj, c), (vi, vj, -c), (vj, vi, -c)]);
            }
        }
        if wb > 0.0 {
            for &(a, b, _) in &boundary {
                trip.extend([(a, a, wb), (b, b, wb), (a, b, -wb), (b, a, -wb)]);
            }
        }
        Csr::from_triplets(n, trip)
    };
    // Unbiased: a constant system matrix, the edge form of the weighted Dirichlet energy plus
    // the boundary springs. Biased: rebuilt every iteration from the current edge lengths.
    let mut cw = edge_weights(&uv);
    let mut a = system(&cw);
    let mut free = vec![true; n];
    free[p.pin] = false;

    // scaled dual variables (Overby et al. 2017): a 2x2 matrix per triangle for a one-sided
    // mode (ADMM with the split Z_t = J_t), one scalar per boundary edge for the boundary length
    let mut dual_t = vec![[[0.0f64; 2]; 2]; tris.len()];
    let mut split_t: Vec<Option<M2>> = vec![None; tris.len()];
    let mut dual_b = vec![0.0f64; boundary.len()];
    let diag = bbox_diag(&uv).max(1e-300);
    let mut iterations = 0;
    let mut converged = false;
    let mut energy = 0.0;
    let mut shortened_steps = 0;
    let mut violation = 0.0;
    for it in 0..p.max_iterations {
        iterations = it + 1;
        if biased_global && it > 0 {
            cw = edge_weights(&uv);
            a = system(&cw);
        }
        // local step: targets for every triangle and boundary edge
        let targets: Vec<Option<M2>> = tris
            .iter()
            .enumerate()
            .map(|(ti, t)| {
                let f = frames[ti]?;
                let j = f.jacobian([uv[t[0]], uv[t[1]], uv[t[2]]]);
                if !one_sided {
                    // plain local step: the material target of the current Jacobian
                    let (phi, theta, s1, s2) = svd2(&j);
                    let b = material_target([s1.abs(), s2.abs()], &p.material);
                    return Some(compose(phi, theta, b));
                }
                // ADMM local step: Z = argmin material(Z) + rho/2 |Z - (J + Y)|^2 over the
                // admissible set, with the material projections taken at J + Y and rho equal
                // to the material weight; the global target is then Z - Y
                let y = dual_t[ti];
                let v = madd(&j, &y, 1.0);
                let (phi, theta, s1, s2) = svd2(&v);
                let sv = [s1.abs(), s2.abs()];
                let b = material_target(sv, &p.material);
                let z = [0, 1].map(|i| {
                    let m = 0.5 * (b[i] + sv[i]);
                    match p.mode {
                        Mode::StretchOnly => m.max(1.0),
                        _ => m.min(1.0),
                    }
                });
                let zm = compose(phi, theta, z);
                split_t[ti] = Some(zm);
                Some(madd(&zm, &y, -1.0))
            })
            .collect();
        let edge_targets: Vec<[f64; 2]> = boundary
            .iter()
            .zip(&dual_b)
            .map(|(&(i, j, l3), z)| {
                let d = sub2(uv[i], uv[j]);
                let len = d[0].hypot(d[1]);
                let u = if len > 1e-300 { [d[0] / len, d[1] / len] } else { [1.0, 0.0] };
                [(l3 + z) * u[0], (l3 + z) * u[1]]
            })
            .collect();
        // global step
        let (mut bx, mut by) = rhs(tris, &frames, &targets, &cw, n);
        if wb > 0.0 {
            for (&(i, j, _), d) in boundary.iter().zip(&edge_targets) {
                bx[i] += wb * d[0];
                by[i] += wb * d[1];
                bx[j] -= wb * d[0];
                by[j] -= wb * d[1];
            }
        }
        let mut gx: Vec<f64> = uv.iter().map(|q| q[0]).collect();
        let mut gy: Vec<f64> = uv.iter().map(|q| q[1]).collect();
        conjugate_gradient(&a, &bx, &mut gx, &free);
        conjugate_gradient(&a, &by, &mut gy, &free);
        let step: Vec<[f64; 2]> = (0..n).map(|i| [gx[i] - uv[i][0], gy[i] - uv[i][1]]).collect();
        // fold-free line search (Smith and Schaefer 2015): full step when it is safe
        let amax = max_step_keeping_area(&guarded, &uv, &step, min_area2);
        let alpha = if amax >= 1.0 {
            1.0
        } else {
            shortened_steps += 1;
            p.step_fraction * amax
        };
        let mut moved = 0.0f64;
        for i in 0..n {
            uv[i][0] += alpha * step[i][0];
            uv[i][1] += alpha * step[i][1];
            moved = moved.max(alpha * step[i][0].hypot(step[i][1]));
        }
        // energy with this iteration's targets
        energy = 0.0;
        for (ti, t) in tris.iter().enumerate() {
            let (Some(f), Some(l)) = (frames[ti], targets[ti]) else { continue };
            let x = f.x();
            for (k, &c) in cw[ti].iter().enumerate() {
                let (i, j) = ((k + 1) % 3, (k + 2) % 3);
                let e = sub2(x[i], x[j]);
                let le = [l[0][0] * e[0] + l[0][1] * e[1], l[1][0] * e[0] + l[1][1] * e[1]];
                let du = sub2(uv[t[i]], uv[t[j]]);
                energy += c * ((du[0] - le[0]).powi(2) + (du[1] - le[1]).powi(2));
            }
        }
        for (&(i, j, _), d) in boundary.iter().zip(&edge_targets) {
            let du = sub2(uv[i], uv[j]);
            energy += wb * ((du[0] - d[0]).powi(2) + (du[1] - d[1]).powi(2));
        }
        // dual update with the remaining violation
        violation = 0.0f64;
        if one_sided {
            for (ti, t) in tris.iter().enumerate() {
                let (Some(f), Some(z)) = (frames[ti], split_t[ti]) else { continue };
                let j = f.jacobian([uv[t[0]], uv[t[1]], uv[t[2]]]);
                // Y += J - Z; the primal residual |J - Z| and the bound on J both count
                let r = madd(&j, &z, -1.0);
                dual_t[ti] = madd(&dual_t[ti], &r, 1.0);
                let s = singular_values(&j);
                let bound = match p.mode {
                    Mode::StretchOnly => 1.0 - s[1],
                    _ => s[0] - 1.0,
                };
                let res = r.iter().flatten().fold(0.0f64, |m, v| m.max(v.abs()));
                violation = violation.max(bound).max(res);
            }
        }
        if wb > 0.0 {
            for (k, &(i, j, l3)) in boundary.iter().enumerate() {
                let d = sub2(uv[i], uv[j]);
                let len = d[0].hypot(d[1]);
                dual_b[k] += l3 - len;
                violation = violation.max((len / l3 - 1.0).abs());
            }
        }
        if moved <= p.tolerance * diag && violation <= p.constraint_tolerance {
            converged = true;
            break;
        }
    }

    // Make the hard constraint exact with one uniform scale about the pinned vertex.
    let sigma_of = |uv: &[[f64; 2]]| -> Vec<[f64; 2]> {
        tris.iter()
            .zip(&frames)
            .map(|(t, f)| match f {
                Some(f) => singular_values(&f.jacobian([uv[t[0]], uv[t[1]], uv[t[2]]])),
                None => [1.0, 1.0],
            })
            .collect()
    };
    let boundary_len = |uv: &[[f64; 2]]| -> (f64, f64) {
        boundary.iter().fold((0.0, 0.0), |(l2, l3), &(i, j, d3)| {
            let d = sub2(uv[i], uv[j]);
            (l2 + d[0].hypot(d[1]), l3 + d3)
        })
    };
    let sigma = sigma_of(&uv);
    let positive_sigma = || sigma.iter().zip(&frames).filter(|(_, f)| f.is_some());
    let scale = match p.mode {
        Mode::StretchOnly => {
            let lo = positive_sigma().map(|(s, _)| s[1]).fold(f64::INFINITY, f64::min);
            (lo.is_finite() && lo > 0.0 && lo < 1.0 - p.edge_tolerance).then(|| 1.0 / lo)
        }
        Mode::CompressOnly => {
            let hi = positive_sigma().map(|(s, _)| s[0]).fold(0.0, f64::max);
            (hi > 1.0 + p.edge_tolerance).then(|| 1.0 / hi)
        }
        Mode::Rigid if wb > 0.0 => {
            let (l2, l3) = boundary_len(&uv);
            (l2 > 0.0 && (l2 / l3 - 1.0).abs() > p.edge_tolerance).then(|| l3 / l2)
        }
        Mode::Rigid => None,
    };
    let mut final_scale = 1.0;
    if let Some(s) = scale {
        let o = uv[p.pin];
        for q in uv.iter_mut() {
            q[0] = o[0] + s * (q[0] - o[0]);
            q[1] = o[1] + s * (q[1] - o[1]);
        }
        final_scale = s;
    }
    let sigma = sigma_of(&uv);
    let (lo, hi) = edge_ratio_range(pos, tris, &uv);
    let (l2, l3) = boundary_len(&uv);
    let flipped = tris
        .iter()
        .zip(&frames)
        .filter(|(t, f)| f.is_some() && area2(uv[t[0]], uv[t[1]], uv[t[2]]) <= 0.0)
        .count();
    Ok(Output {
        uv,
        iterations,
        converged,
        energy,
        sigma,
        min_edge_ratio: lo,
        max_edge_ratio: hi,
        final_scale,
        violation_before_scale: violation,
        boundary_ratio: if l3 > 0.0 { l2 / l3 } else { 1.0 },
        shortened_steps,
        flipped,
    })
}

fn sub2(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

/// `a + k b` for 2x2 matrices.
fn madd(a: &M2, b: &M2, k: f64) -> M2 {
    [[a[0][0] + k * b[0][0], a[0][1] + k * b[0][1]], [a[1][0] + k * b[1][0], a[1][1] + k * b[1][1]]]
}

/// `Rot(phi) diag(s) Rot(theta)`.
fn compose(phi: f64, theta: f64, s: [f64; 2]) -> M2 {
    mul(&mul(&rot(phi), &[[s[0], 0.0], [0.0, s[1]]]), &rot(theta))
}

/// Cotangent of the angle at corner `k` of a frame triangle.
fn cot_at(x: &[[f64; 2]; 3], k: usize) -> f64 {
    let (a, b) = (sub2(x[(k + 1) % 3], x[k]), sub2(x[(k + 2) % 3], x[k]));
    (a[0] * b[0] + a[1] * b[1]) / (a[0] * b[1] - a[1] * b[0]).abs()
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

/// Right-hand sides of the global step for the given per-triangle targets.
fn rhs(
    tris: &[[usize; 3]],
    frames: &[Option<Frame>],
    targets: &[Option<M2>],
    cw: &[[f64; 3]],
    n: usize,
) -> (Vec<f64>, Vec<f64>) {
    let (mut bx, mut by) = (vec![0.0; n], vec![0.0; n]);
    for (ti, t) in tris.iter().enumerate() {
        let (Some(f), Some(l)) = (frames[ti], targets[ti]) else { continue };
        let x = f.x();
        for (k, &c) in cw[ti].iter().enumerate() {
            let (i, jn) = ((k + 1) % 3, (k + 2) % 3);
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

    #[test]
    fn flatten_constraints_material_targets() {
        // the area projection has unit determinant and is the closest such pair
        for s in [[1.3, 0.9], [2.0, 0.5], [1.0, 1.0], [0.8, 0.7]] {
            let a = area_projection(s[0], s[1]);
            assert!((a[0] * a[1] - 1.0).abs() < 1e-12, "{a:?}");
            let d = |x: f64| (x - s[0]).powi(2) + (1.0 / x - s[1]).powi(2);
            assert!(d(a[0]) <= d(a[0] * 1.001) && d(a[0]) <= d(a[0] * 0.999), "{s:?} -> {a:?}");
        }
        assert_eq!(material_target([1.3, 0.9], &MaterialWeights::EDGE_ONLY), [1.0, 1.0]);
        // a pure shear term targets the mean singular value on both axes
        let t = material_target([1.3, 0.9], &MaterialWeights { edge: 0.0, area: 0.0, shear: 1.0 });
        assert!((t[0] - 1.1).abs() < 1e-12 && (t[1] - 1.1).abs() < 1e-12);
    }

    #[test]
    fn flatten_constraints_boundary_constraint_keeps_boundary_edges() {
        let (pos, tris, uv0) = cap(6, 24, 50f64.to_radians());
        let p = Params { boundary_weight: 10.0, ..Params::default() };
        let o = flatten_constrained(&pos, &tris, &uv0, &[], &p).unwrap();
        assert_eq!(o.flipped, 0);
        assert!((o.boundary_ratio - 1.0).abs() < 1e-12, "{}", o.boundary_ratio);
        assert!((o.final_scale - 1.0).abs() < 1e-4, "the iteration did the work: scale {}", o.final_scale);
        // every boundary edge, not only the total, is at its 3-D length
        let ring = |s: usize| 1 + 5 * 24 + s % 24;
        for s in 0..24 {
            let (a, b) = (ring(s), ring(s + 1));
            let d3 = dot3(sub3(pos[a], pos[b]), sub3(pos[a], pos[b])).sqrt();
            let d2 = (o.uv[a][0] - o.uv[b][0]).hypot(o.uv[a][1] - o.uv[b][1]);
            assert!((d2 / d3 - 1.0).abs() < 1e-4, "edge {s}: {}", d2 / d3);
        }
    }
}

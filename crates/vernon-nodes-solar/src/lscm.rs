//! Flattening kernels: plane fit, least-squares conformal map (LSCM) and an
//! as-rigid-as-possible refinement with stretch/compression weighting.

use std::collections::BTreeMap;

use glam::{DVec2, DVec3};
use vernon_core::Geometry;

use crate::sparse::{pcg, Csr};
use crate::topo::ekey;

/// Triangles of a geometry on compacted point ids.
pub struct Compact {
    pub pts: Vec<DVec3>,
    pub tris: Vec<[u32; 3]>,
    pub tri_prim: Vec<usize>,
    /// Compact id to original point id.
    pub used: Vec<u32>,
    /// Original point id to compact id (`u32::MAX` if unused).
    pub map: Vec<u32>,
}

pub fn compact(g: &Geometry) -> Compact {
    let tri = g.triangles();
    let mut map = vec![u32::MAX; g.points.len()];
    let mut used = Vec::new();
    let mut tris = Vec::with_capacity(tri.len());
    let mut tri_prim = Vec::with_capacity(tri.len());
    for (pi, t) in &tri {
        let mut c = [0u32; 3];
        for (k, &v) in t.iter().enumerate() {
            if map[v as usize] == u32::MAX {
                map[v as usize] = used.len() as u32;
                used.push(v);
            }
            c[k] = map[v as usize];
        }
        tris.push(c);
        tri_prim.push(*pi);
    }
    let pts = used.iter().map(|&v| g.points[v as usize]).collect();
    Compact {
        pts,
        tris,
        tri_prim,
        used,
        map,
    }
}

/// Local 2-D coordinates of a 3-D triangle: first vertex at the origin, the
/// first edge on +x, the third vertex at positive y. `None` if degenerate.
pub fn local_frame(p: [DVec3; 3]) -> Option<[DVec2; 3]> {
    let e1 = p[1] - p[0];
    let e2 = p[2] - p[0];
    let l1 = e1.length();
    let cross = e1.cross(e2).length();
    if l1 < 1e-12 || cross < 1e-12 {
        return None;
    }
    Some([
        DVec2::ZERO,
        DVec2::new(l1, 0.0),
        DVec2::new(e1.dot(e2) / l1, cross / l1),
    ])
}

#[allow(clippy::needless_range_loop)]
fn eig_sym3(mut a: [[f64; 3]; 3]) -> ([f64; 3], [DVec3; 3]) {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..60 {
        let off = a[0][1].powi(2) + a[0][2].powi(2) + a[1][2].powi(2);
        if off < 1e-28 {
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
            for row in a.iter_mut() {
                let (akp, akq) = (row[p], row[q]);
                row[p] = c * akp - s * akq;
                row[q] = s * akp + c * akq;
            }
            for k in 0..3 {
                let (apk, aqk) = (a[p][k], a[q][k]);
                a[p][k] = c * apk - s * aqk;
                a[q][k] = s * apk + c * aqk;
            }
            for row in v.iter_mut() {
                let (vkp, vkq) = (row[p], row[q]);
                row[p] = c * vkp - s * vkq;
                row[q] = s * vkp + c * vkq;
            }
        }
    }
    let mut idx = [0usize, 1, 2];
    idx.sort_by(|&i, &j| a[i][i].total_cmp(&a[j][j]));
    let vals = [a[idx[0]][idx[0]], a[idx[1]][idx[1]], a[idx[2]][idx[2]]];
    let vecs = [0, 1, 2].map(|k| DVec3::new(v[0][idx[k]], v[1][idx[k]], v[2][idx[k]]));
    (vals, vecs)
}

/// Orthogonal projection onto the best-fit plane. Returns `None` for
/// collinear input.
pub fn plane_fit(c: &Compact) -> Option<Vec<DVec2>> {
    let n = c.pts.len();
    if n < 3 {
        return None;
    }
    let cen = c.pts.iter().copied().sum::<DVec3>() / n as f64;
    let mut cov = [[0.0f64; 3]; 3];
    for p in &c.pts {
        let d = (*p - cen).to_array();
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += d[i] * d[j];
            }
        }
    }
    let (vals, vecs) = eig_sym3(cov);
    if vals[1] <= 1e-12 * vals[2].max(1e-300) || vals[2] <= 0.0 {
        return None;
    }
    let mut normal = vecs[0];
    let mut mean_n = DVec3::ZERO;
    for t in &c.tris {
        let (a, b, d) = (
            c.pts[t[0] as usize],
            c.pts[t[1] as usize],
            c.pts[t[2] as usize],
        );
        mean_n += (b - a).cross(d - a);
    }
    if normal.dot(mean_n) < 0.0 {
        normal = -normal;
    }
    let e1 = vecs[2];
    let e2 = normal.cross(e1);
    Some(
        c.pts
            .iter()
            .map(|p| DVec2::new((*p - cen).dot(e1), (*p - cen).dot(e2)))
            .collect(),
    )
}

fn area3(c: &Compact) -> f64 {
    c.tris
        .iter()
        .map(|t| {
            let (a, b, d) = (
                c.pts[t[0] as usize],
                c.pts[t[1] as usize],
                c.pts[t[2] as usize],
            );
            (b - a).cross(d - a).length() * 0.5
        })
        .sum()
}

pub fn signed_area2(c: &Compact, uv: &[DVec2]) -> f64 {
    c.tris
        .iter()
        .map(|t| {
            let (a, b, d) = (uv[t[0] as usize], uv[t[1] as usize], uv[t[2] as usize]);
            0.5 * (b - a).perp_dot(d - a)
        })
        .sum()
}

/// Least-squares conformal map of a disk-topology mesh. `boundary` is the
/// boundary loop (compact ids). The result is a similarity-fixed map
/// rescaled so the pattern area equals the 3-D area. Returns the map, the
/// solver iterations and the relative residual.
pub fn lscm(
    c: &Compact,
    boundary: &[u32],
    tol: f64,
    max_iter: usize,
) -> Result<(Vec<DVec2>, usize, f64), String> {
    let n = c.pts.len();
    if boundary.len() < 3 || n < 3 {
        return Err("the mesh has no usable boundary".into());
    }
    let far = |from: u32| -> u32 {
        let pf = c.pts[from as usize];
        boundary
            .iter()
            .copied()
            .max_by(|&x, &y| {
                c.pts[x as usize]
                    .distance(pf)
                    .total_cmp(&c.pts[y as usize].distance(pf))
                    .then(y.cmp(&x))
            })
            .unwrap_or(from)
    };
    let pb = far(boundary[0]);
    let pa = far(pb);
    let d = c.pts[pa as usize].distance(c.pts[pb as usize]);
    if pa == pb || d < 1e-12 {
        return Err("cannot choose two distinct pin vertices".into());
    }
    let mut pin = vec![None::<DVec2>; n];
    pin[pa as usize] = Some(DVec2::ZERO);
    pin[pb as usize] = Some(DVec2::new(d, 0.0));
    let mut free_idx = vec![usize::MAX; n];
    let mut m = 0usize;
    for i in 0..n {
        if pin[i].is_none() {
            free_idx[i] = m;
            m += 1;
        }
    }
    let mut trip: Vec<(u32, u32, f64)> = Vec::with_capacity(c.tris.len() * 72);
    let mut rhs = vec![0.0; 2 * m];
    for t in &c.tris {
        let p = [
            c.pts[t[0] as usize],
            c.pts[t[1] as usize],
            c.pts[t[2] as usize],
        ];
        let Some(l) = local_frame(p) else { continue };
        let s = 1.0 / (l[1].x * l[2].y).sqrt();
        let w = [(l[2] - l[1]) * s, (l[0] - l[2]) * s, (l[1] - l[0]) * s];
        for row in 0..2 {
            let mut s_pin = 0.0;
            let mut ent: Vec<(usize, f64)> = Vec::with_capacity(6);
            for j in 0..3 {
                let (a, b) = (w[j].x, w[j].y);
                let (cu, cv) = if row == 0 { (a, -b) } else { (b, a) };
                let vid = t[j] as usize;
                match pin[vid] {
                    Some(pv) => s_pin += cu * pv.x + cv * pv.y,
                    None => {
                        ent.push((free_idx[vid], cu));
                        ent.push((m + free_idx[vid], cv));
                    }
                }
            }
            for &(fi, ci) in &ent {
                rhs[fi] -= ci * s_pin;
                for &(fj, cj) in &ent {
                    trip.push((fi as u32, fj as u32, ci * cj));
                }
            }
        }
    }
    let k = Csr::from_triplets(2 * m, trip);
    let mut x = vec![0.0; 2 * m];
    let (it, rel) = pcg(&k, &rhs, &mut x, tol, max_iter);
    let mut uv: Vec<DVec2> = (0..n)
        .map(|i| match pin[i] {
            Some(p) => p,
            None => DVec2::new(x[free_idx[i]], x[m + free_idx[i]]),
        })
        .collect();
    let a2 = signed_area2(c, &uv);
    if a2 <= 1e-18 {
        return Err("the conformal map collapsed (zero pattern area)".into());
    }
    let s = (area3(c) / a2).sqrt();
    for u in uv.iter_mut() {
        *u *= s;
    }
    Ok((uv, it, rel))
}

/// Rotate the pattern so its principal axis lies on +x, optionally mirror
/// it, and move its bounding box to the origin.
pub fn align_uv(uv: &mut [DVec2], mirror: bool) {
    if uv.is_empty() {
        return;
    }
    if mirror {
        for u in uv.iter_mut() {
            u.y = -u.y;
        }
    }
    let n = uv.len() as f64;
    let cen = uv.iter().copied().sum::<DVec2>() / n;
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for u in uv.iter() {
        let d = *u - cen;
        sxx += d.x * d.x;
        syy += d.y * d.y;
        sxy += d.x * d.y;
    }
    let ang = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (s, c) = (-ang).sin_cos();
    for u in uv.iter_mut() {
        let d = *u - cen;
        *u = DVec2::new(c * d.x - s * d.y, s * d.x + c * d.y);
    }
    let min = uv
        .iter()
        .fold(DVec2::splat(f64::INFINITY), |m, u| m.min(*u));
    for u in uv.iter_mut() {
        *u -= min;
    }
}

#[derive(Clone, Debug)]
pub struct ArapOpts {
    pub max_iter: usize,
    pub tol: f64,
    pub preserve_boundary: bool,
    pub base_stretch: f64,
    pub base_compress: f64,
    pub bnd_stretch: f64,
    pub bnd_compress: f64,
    pub int_stretch: f64,
    pub int_compress: f64,
}

impl Default for ArapOpts {
    fn default() -> Self {
        ArapOpts {
            max_iter: 200,
            tol: 1e-6,
            preserve_boundary: false,
            base_stretch: 1.0,
            base_compress: 1.0,
            bnd_stretch: 1.0,
            bnd_compress: 1.0,
            int_stretch: 1.0,
            int_compress: 1.0,
        }
    }
}

/// The result of the ARAP iterations.
pub struct ArapOut {
    pub uv: Vec<DVec2>,
    pub iterations: usize,
    pub energy: f64,
    /// The relative energy change fell below the tolerance (or the energy
    /// vanished) before the iteration limit.
    pub converged: bool,
    /// Triangles left out of the solve because they are degenerate.
    pub skipped: usize,
    /// Exact (signed) cotangent weights were used, i.e. the energy is the one
    /// of Liu et al.; otherwise the weights were clamped positive.
    pub exact_weights: bool,
}

/// A triangle that cannot carry a map: a non-finite corner, an area below
/// 1e-12 mm2, or a height below 1e-6 of its longest edge (a sliver whose
/// cotangent weights would swamp the system).
pub fn tri_degenerate(p: [DVec3; 3]) -> bool {
    if p.iter().any(|v| !v.is_finite()) {
        return true;
    }
    let (e1, e2, e3) = (p[1] - p[0], p[2] - p[1], p[0] - p[2]);
    let lmax2 = e1
        .length_squared()
        .max(e2.length_squared())
        .max(e3.length_squared());
    let twice_area = e1.cross(-e3).length();
    twice_area < 2e-12 || twice_area < 1e-6 * lmax2
}

/// Per-triangle degenerate flags of a compacted mesh (see [`tri_degenerate`]).
pub fn degenerate_tris(c: &Compact) -> Vec<bool> {
    c.tris
        .iter()
        .map(|t| {
            tri_degenerate([
                c.pts[t[0] as usize],
                c.pts[t[1] as usize],
                c.pts[t[2] as usize],
            ])
        })
        .collect()
}

struct ArapTri {
    v: [u32; 3],
    x: [DVec2; 3],
    /// Cotangent weights of the local edges (0,1), (1,2), (2,0).
    cot: [f64; 3],
    bnd: [bool; 3],
}

const EDGES: [(usize, usize); 3] = [(0, 1), (1, 2), (2, 0)];

/// Closest rotation to the Jacobian from the triangle's local 3-D frame to
/// the current pattern, as `(cos, sin)`. In 2-D the closest rotation of
/// `[[a, b], [c, d]]` has the angle `atan2(c - b, a + d)`, which is the
/// polar factor and needs no SVD; a reflected triangle gets its nearest
/// proper rotation.
fn rotation_of(t: &ArapTri, uv: &[DVec2]) -> (f64, f64) {
    let d1 = uv[t.v[1] as usize] - uv[t.v[0] as usize];
    let d2 = uv[t.v[2] as usize] - uv[t.v[0] as usize];
    let (l1, cx, cy) = (t.x[1].x, t.x[2].x, t.x[2].y);
    let j11 = d1.x / l1;
    let j21 = d1.y / l1;
    let j12 = -d1.x * cx / (l1 * cy) + d2.x / cy;
    let j22 = -d1.y * cx / (l1 * cy) + d2.y / cy;
    let ang = (j21 - j12).atan2(j11 + j22);
    (ang.cos(), ang.sin())
}

/// As-rigid-as-possible parameterisation by the local/global iteration of
/// Liu, Zhang, Xu, Gotsman and Gortler (2008). Local step: the closest
/// rotation of every triangle's Jacobian. Global step: the sparse linear
/// system for the pattern positions that minimises
/// `sum_t sum_(i,j) cot(theta_ij) |(u_i - u_j) - R_t (x_i - x_j)|^2`.
/// One vertex is pinned to fix the translation; the rotation is free.
///
/// With all stretch/compress factors at 1 the weights are the exact
/// (possibly negative) cotangents and the system matrix is constant, so it is
/// built once. With biased factors the edge weights are rescaled each
/// iteration by the stretch/compress factors (iteratively reweighted) and the
/// cotangents are clamped positive so every weighted triangle stays positive
/// semi-definite. Degenerate triangles are skipped and counted.
pub fn arap(c: &Compact, uv0: &[DVec2], o: &ArapOpts) -> ArapOut {
    let n = c.pts.len();
    let uniform = !o.preserve_boundary
        && [
            o.base_stretch,
            o.base_compress,
            o.bnd_stretch,
            o.bnd_compress,
            o.int_stretch,
            o.int_compress,
        ]
        .iter()
        .all(|&f| f == 1.0);
    let mut edge_count: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    for t in &c.tris {
        for (a, b) in EDGES {
            *edge_count.entry(ekey(t[a], t[b])).or_insert(0) += 1;
        }
    }
    let mut tris: Vec<ArapTri> = Vec::new();
    let mut skipped = 0usize;
    let mut total_area = 0.0;
    for t in &c.tris {
        let p = [
            c.pts[t[0] as usize],
            c.pts[t[1] as usize],
            c.pts[t[2] as usize],
        ];
        let x = if tri_degenerate(p) { None } else { local_frame(p) };
        let Some(x) = x else {
            skipped += 1;
            continue;
        };
        let area2 = x[1].x * x[2].y;
        total_area += 0.5 * area2;
        let mut cot = [0.0; 3];
        let mut bnd = [false; 3];
        for (k, &(a, b)) in EDGES.iter().enumerate() {
            let opp = 3 - a - b;
            let (ea, eb) = (x[a] - x[opp], x[b] - x[opp]);
            let raw = ea.dot(eb) / area2;
            cot[k] = if uniform { raw } else { raw.max(1e-4) };
            bnd[k] = edge_count[&ekey(t[a], t[b])] == 1;
        }
        tris.push(ArapTri { v: *t, x, cot, bnd });
    }
    let mut uv = uv0.to_vec();
    let Some(pin_v) = tris.first().map(|t| t.v[0] as usize) else {
        return ArapOut {
            uv,
            iterations: 0,
            energy: 0.0,
            converged: true,
            skipped,
            exact_weights: uniform,
        };
    };
    let pin_pos = uv[pin_v];
    let mut free_idx = vec![usize::MAX; n];
    let mut m = 0;
    for (i, f) in free_idx.iter_mut().enumerate() {
        if i != pin_v {
            *f = m;
            m += 1;
        }
    }
    let build = |weights: &[[f64; 3]]| -> Csr {
        let mut trip: Vec<(u32, u32, f64)> = Vec::with_capacity(tris.len() * 12);
        for (ti, t) in tris.iter().enumerate() {
            for (k, &(a, b)) in EDGES.iter().enumerate() {
                let (va, vb) = (t.v[a] as usize, t.v[b] as usize);
                let wk = weights[ti][k];
                for (row, col) in [(va, vb), (vb, va)] {
                    if row != pin_v {
                        let r = free_idx[row];
                        trip.push((r as u32, r as u32, wk));
                        if col != pin_v {
                            trip.push((r as u32, free_idx[col] as u32, -wk));
                        }
                    }
                }
            }
        }
        Csr::from_triplets(m, trip)
    };
    let mut fixed: Option<Csr> = None;
    let mut prev_e = f64::INFINITY;
    let mut energy = 0.0;
    let mut it = 0;
    let mut converged = false;
    while it < o.max_iter {
        // Local step: the rotation of every triangle.
        let rots: Vec<(f64, f64)> = tris.iter().map(|t| rotation_of(t, &uv)).collect();
        // Edge weights: exact cotangents, or reweighted by stretch/compress.
        let weights: Vec<[f64; 3]> = tris
            .iter()
            .map(|t| {
                let mut w = [0.0; 3];
                for (k, &(a, b)) in EDGES.iter().enumerate() {
                    if uniform {
                        w[k] = t.cot[k];
                        continue;
                    }
                    let len3 = t.x[a].distance(t.x[b]);
                    let r = uv[t.v[a] as usize].distance(uv[t.v[b] as usize]) / len3.max(1e-300);
                    let kk = if r > 1.0 {
                        o.base_stretch
                            * if t.bnd[k] {
                                o.bnd_stretch
                            } else {
                                o.int_stretch
                            }
                    } else {
                        o.base_compress
                            * if t.bnd[k] {
                                o.bnd_compress
                            } else {
                                o.int_compress
                            }
                    };
                    let pb = if o.preserve_boundary && t.bnd[k] {
                        10.0
                    } else {
                        1.0
                    };
                    w[k] = t.cot[k] * kk * pb;
                }
                w
            })
            .collect();
        // Global step: right-hand side from the rotated rest edges.
        let (mut bu, mut bv) = (vec![0.0; m], vec![0.0; m]);
        for (ti, t) in tris.iter().enumerate() {
            let (cs, sn) = rots[ti];
            for (k, &(a, b)) in EDGES.iter().enumerate() {
                let (va, vb) = (t.v[a] as usize, t.v[b] as usize);
                let wk = weights[ti][k];
                let dx = t.x[a] - t.x[b];
                let rd = DVec2::new(cs * dx.x - sn * dx.y, sn * dx.x + cs * dx.y);
                // Contribution to row va: +wk * R (x_a - x_b); to row vb: -.
                for (row, col, sign) in [(va, vb, 1.0), (vb, va, -1.0)] {
                    if row != pin_v {
                        let r = free_idx[row];
                        bu[r] += sign * wk * rd.x;
                        bv[r] += sign * wk * rd.y;
                        if col == pin_v {
                            bu[r] += wk * pin_pos.x;
                            bv[r] += wk * pin_pos.y;
                        }
                    }
                }
            }
        }
        let rebuilt;
        let l: &Csr = if uniform {
            fixed.get_or_insert_with(|| build(&weights))
        } else {
            rebuilt = build(&weights);
            &rebuilt
        };
        let mut xu: Vec<f64> = (0..n).filter(|&i| i != pin_v).map(|i| uv[i].x).collect();
        let mut xv: Vec<f64> = (0..n).filter(|&i| i != pin_v).map(|i| uv[i].y).collect();
        pcg(l, &bu, &mut xu, 1e-10, 4000);
        pcg(l, &bv, &mut xv, 1e-10, 4000);
        for i in 0..n {
            uv[i] = if i == pin_v {
                pin_pos
            } else {
                DVec2::new(xu[free_idx[i]], xv[free_idx[i]])
            };
        }
        // Energy with the same weights and the rotations of this iteration.
        energy = 0.0;
        for (ti, t) in tris.iter().enumerate() {
            let (cs, sn) = rots[ti];
            for (k, &(a, b)) in EDGES.iter().enumerate() {
                let dx = t.x[a] - t.x[b];
                let rd = DVec2::new(cs * dx.x - sn * dx.y, sn * dx.x + cs * dx.y);
                let du = uv[t.v[a] as usize] - uv[t.v[b] as usize];
                energy += weights[ti][k] * (du - rd).length_squared();
            }
        }
        it += 1;
        if energy.abs() <= 1e-20 * total_area.max(1e-30)
            || (prev_e - energy).abs() <= o.tol * energy.abs().max(1e-30)
        {
            converged = true;
            break;
        }
        prev_e = energy;
    }
    ArapOut {
        uv,
        iterations: it,
        energy,
        converged,
        skipped,
        exact_weights: uniform,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eigen_of_a_diagonal_and_a_rotated_matrix() {
        let (v, e) = eig_sym3([[3.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 2.0]]);
        assert!(
            (v[0] - 1.0).abs() < 1e-12 && (v[1] - 2.0).abs() < 1e-12 && (v[2] - 3.0).abs() < 1e-12
        );
        assert!(e[2].dot(DVec3::X).abs() > 0.999999);
        // [[2,1,0],[1,2,0],[0,0,5]] has eigenvalues 1, 3, 5
        let (v, _) = eig_sym3([[2.0, 1.0, 0.0], [1.0, 2.0, 0.0], [0.0, 0.0, 5.0]]);
        assert!(
            (v[0] - 1.0).abs() < 1e-10 && (v[1] - 3.0).abs() < 1e-10 && (v[2] - 5.0).abs() < 1e-10
        );
    }
}

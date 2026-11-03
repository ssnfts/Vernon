//! Woven-ply shear and locking analysis of a flattened mesh.
//!
//! Kinematic (pin-jointed net, "fishnet") draping view of a woven ply: the
//! yarns are inextensible lines that rotate freely at the crossings, so the
//! only way a fabric follows a doubly curved surface is by *shear*, the change
//! of the angle between the warp and weft yarns. This module judges a given
//! flattening: for every triangle it takes the warp and weft directions on the
//! 3-D surface, maps them to the flat pattern with the triangle's affine map
//! and reports the angle change and the length change of each yarn direction.
//!
//! Sources (textbook, cited from memory; all UNVERIFIED -- check before
//! quoting):
//! * Mack and Taylor, "The fitting of woven cloth to surfaces", Journal of the
//!   Textile Institute 47(9), 1956 -- the fishnet / pin-jointed net idea;
//! * Van Der Weeen (Van Der Ween), "Algorithms for draping fabrics on
//!   doubly-curved surfaces", International Journal for Numerical Methods in
//!   Engineering 31(7), 1991 -- the geometric draping algorithm.
//!
//! The shear angle of a triangle is `90 deg - (angle between the mapped warp
//! and weft)`, signed: positive when the yarns close up (the angle between
//! them shrinks), negative when it opens. The locking angle is the shear at
//! which the yarns jam and the ply starts to wrinkle instead of shearing; it
//! depends on the weave, yarn size and fabric tension, so
//! [`DEFAULT_LOCKING_DEG`] is only a placeholder to be replaced by a measured
//! value of the actual material.
//!
//! Units: positions in mm (only ratios matter), angles in degrees. Triangles
//! must be wound counter-clockwise about the surface normal and the flat
//! layout must keep that winding (positive signed area).

/// Shear angle, in degrees, beyond which a ply is assumed to lock. Material
/// dependent (weave, yarn spacing, tension); 45 deg is a common
/// rule-of-thumb order of magnitude, not a measured property.
pub const DEFAULT_LOCKING_DEG: f64 = 45.0;

/// Shear and stretch of one triangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriShear {
    /// False when the warp is (nearly) normal to the triangle, or the
    /// triangle is degenerate: all other fields are zero then.
    pub valid: bool,
    /// Image of the unit warp vector in the flat pattern (length = warp stretch).
    pub warp_2d: [f64; 2],
    /// Image of the unit weft vector in the flat pattern (length = weft stretch).
    pub weft_2d: [f64; 2],
    /// Signed shear angle, degrees (see the module docs for the sign).
    pub shear_deg: f64,
    pub warp_stretch: f64,
    pub weft_stretch: f64,
}

impl TriShear {
    const INVALID: TriShear = TriShear {
        valid: false,
        warp_2d: [0.0; 2],
        weft_2d: [0.0; 2],
        shear_deg: 0.0,
        warp_stretch: 0.0,
        weft_stretch: 0.0,
    };
}

/// Result of [`analyse`].
#[derive(Debug, Clone, PartialEq)]
pub struct ShearReport {
    pub per_tri: Vec<TriShear>,
    /// Largest `|shear|` over valid triangles, degrees.
    pub max_shear_deg: f64,
    /// Mean `|shear|` over valid triangles, degrees.
    pub mean_shear_deg: f64,
    /// Locking angle that was used, degrees.
    pub locking_deg: f64,
    /// Share of valid triangles with `|shear|` above the locking angle, in [0, 1].
    pub locked_fraction: f64,
    /// Triangles that could not be analysed.
    pub undefined: usize,
    /// Smallest and largest yarn stretch (flat length / 3-D length) over valid
    /// triangles and both directions.
    pub min_stretch: f64,
    pub max_stretch: f64,
}

impl ShearReport {
    /// Per-triangle `[signed shear deg, warp stretch, weft stretch]`.
    pub fn shear_vectors(&self) -> Vec<[f64; 3]> {
        self.per_tri.iter().map(|t| [t.shear_deg, t.warp_stretch, t.weft_stretch]).collect()
    }

    /// Per-triangle colour: green at no shear through yellow to red at the
    /// locking angle and beyond; grey for undefined triangles.
    pub fn colours(&self) -> Vec<[f32; 3]> {
        self.per_tri
            .iter()
            .map(|t| {
                if !t.valid {
                    return [0.5, 0.5, 0.5];
                }
                let f = (t.shear_deg.abs() / self.locking_deg).clamp(0.0, 1.0) as f32;
                [(2.0 * f).min(1.0), (2.0 - 2.0 * f).min(1.0), 0.0]
            })
            .collect()
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// Shear and stretch of the triangle `(p0, p1, p2)` mapped to `(q0, q1, q2)`
/// for the 3-D warp direction `warp` (need not be unit or tangent).
pub fn triangle_shear(p: [[f64; 3]; 3], q: [[f64; 2]; 3], warp: [f64; 3]) -> TriShear {
    let a = sub(p[1], p[0]);
    let b = sub(p[2], p[0]);
    let n = cross(a, b);
    let n_len = dot(n, n).sqrt();
    let wl = dot(warp, warp).sqrt();
    if n_len <= 1e-12 * (dot(a, a) + dot(b, b)).max(1e-300) || wl <= 0.0 {
        return TriShear::INVALID;
    }
    let n = scale(n, 1.0 / n_len);
    // tangent part of the warp
    let wt = sub(warp, scale(n, dot(warp, n)));
    let wt_len = dot(wt, wt).sqrt();
    if wt_len <= 1e-9 * wl {
        return TriShear::INVALID;
    }
    let warp3 = scale(wt, 1.0 / wt_len);
    let weft3 = cross(n, warp3);
    // affine map A of the tangent plane: A a = da, A b = db. A tangent vector
    // v = alpha a + beta b has (alpha, beta) from the Gram system.
    let (da, db) = ([q[1][0] - q[0][0], q[1][1] - q[0][1]], [q[2][0] - q[0][0], q[2][1] - q[0][1]]);
    let (gaa, gab, gbb) = (dot(a, a), dot(a, b), dot(b, b));
    let det = gaa * gbb - gab * gab;
    let image = |v: [f64; 3]| -> [f64; 2] {
        let (va, vb) = (dot(v, a), dot(v, b));
        let alpha = (va * gbb - vb * gab) / det;
        let beta = (vb * gaa - va * gab) / det;
        [alpha * da[0] + beta * db[0], alpha * da[1] + beta * db[1]]
    };
    let (w2, f2) = (image(warp3), image(weft3));
    let (lw, lf) = (w2[0].hypot(w2[1]), f2[0].hypot(f2[1]));
    // angle from warp to weft: +90 deg for an undistorted, positively wound triangle
    let angle = (w2[0] * f2[1] - w2[1] * f2[0]).atan2(w2[0] * f2[0] + w2[1] * f2[1]).to_degrees();
    TriShear { valid: true, warp_2d: w2, weft_2d: f2, shear_deg: 90.0 - angle, warp_stretch: lw, weft_stretch: lf }
}

/// Analyse every triangle. `locking_deg` is the material's locking angle
/// (see [`DEFAULT_LOCKING_DEG`]).
pub fn analyse(
    pos: &[[f64; 3]],
    tris: &[[usize; 3]],
    uv: &[[f64; 2]],
    warp: [f64; 3],
    locking_deg: f64,
) -> Result<ShearReport, String> {
    if uv.len() != pos.len() {
        return Err(format!("{} flat coordinates for {} vertices", uv.len(), pos.len()));
    }
    if let Some(t) = tris.iter().find(|t| t.iter().any(|&v| v >= pos.len())) {
        return Err(format!("triangle {t:?} refers to a missing vertex"));
    }
    if locking_deg <= 0.0 || !locking_deg.is_finite() {
        return Err("the locking angle must be a positive number of degrees".into());
    }
    let per_tri: Vec<TriShear> = tris
        .iter()
        .map(|t| triangle_shear([pos[t[0]], pos[t[1]], pos[t[2]]], [uv[t[0]], uv[t[1]], uv[t[2]]], warp))
        .collect();
    let valid: Vec<&TriShear> = per_tri.iter().filter(|t| t.valid).collect();
    let n = valid.len() as f64;
    let (mut max_shear, mut sum, mut locked) = (0.0f64, 0.0, 0usize);
    let (mut min_s, mut max_s) = (f64::INFINITY, 0.0f64);
    for t in &valid {
        let g = t.shear_deg.abs();
        max_shear = max_shear.max(g);
        sum += g;
        locked += usize::from(g > locking_deg);
        min_s = min_s.min(t.warp_stretch.min(t.weft_stretch));
        max_s = max_s.max(t.warp_stretch.max(t.weft_stretch));
    }
    Ok(ShearReport {
        undefined: per_tri.len() - valid.len(),
        max_shear_deg: max_shear,
        mean_shear_deg: if n > 0.0 { sum / n } else { 0.0 },
        locking_deg,
        locked_fraction: if n > 0.0 { locked as f64 / n } else { 0.0 },
        min_stretch: if n > 0.0 { min_s } else { 0.0 },
        max_stretch: max_s,
        per_tri,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn ply_shear_known_affine_map_gives_exact_shear() {
        // warp +x, weft +y. F = [[1, 0.5], [0, 1]]: warp -> (1,0), weft -> (0.5,1).
        let p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let f = |x: f64, y: f64| [x + 0.5 * y, y];
        let q = [f(0.0, 0.0), f(1.0, 0.0), f(0.0, 1.0)];
        let t = triangle_shear(p, q, [1.0, 0.0, 0.0]);
        assert!(t.valid);
        assert!((t.shear_deg - 0.5f64.atan().to_degrees()).abs() < 1e-9, "{}", t.shear_deg);
        assert!((t.warp_stretch - 1.0).abs() < 1e-12);
        assert!((t.weft_stretch - 1.25f64.sqrt()).abs() < 1e-12);
        // F = [[2, 1], [0, 3]]: warp -> (2,0), weft -> (1,3); shear = atan(1/3)
        let g = |x: f64, y: f64| [2.0 * x + y, 3.0 * y];
        let t = triangle_shear(p, [g(0.0, 0.0), g(1.0, 0.0), g(0.0, 1.0)], [5.0, 0.0, 0.0]);
        assert!((t.shear_deg - (1.0f64 / 3.0).atan().to_degrees()).abs() < 1e-9);
        assert!((t.warp_stretch - 2.0).abs() < 1e-12 && (t.weft_stretch - 10f64.sqrt()).abs() < 1e-12);
        // the opposite shear opens the angle: negative
        let h = |x: f64, y: f64| [x - 0.5 * y, y];
        let t = triangle_shear(p, [h(0.0, 0.0), h(1.0, 0.0), h(0.0, 1.0)], [1.0, 0.0, 0.0]);
        assert!((t.shear_deg + 0.5f64.atan().to_degrees()).abs() < 1e-9);
    }

    #[test]
    fn ply_shear_is_independent_of_pattern_position_and_rotation() {
        let p = [[1.0, 2.0, 3.0], [4.0, 2.0, 3.0], [1.0, 5.0, 3.0]];
        let (s, c) = 0.7f64.sin_cos();
        let m = |v: [f64; 3]| [c * (v[0] - 1.0) - s * (v[1] - 2.0) + 50.0, s * (v[0] - 1.0) + c * (v[1] - 2.0) - 9.0];
        let q = [m(p[0]), m(p[1]), m(p[2])];
        let t = triangle_shear(p, q, [0.3, 0.9, 0.0]);
        assert!(t.shear_deg.abs() < 1e-9);
        assert!((t.warp_stretch - 1.0).abs() < 1e-12 && (t.weft_stretch - 1.0).abs() < 1e-12);
    }

    #[test]
    fn ply_shear_warp_normal_to_the_triangle_is_undefined() {
        let p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let t = triangle_shear(p, [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]], [0.0, 0.0, 1.0]);
        assert!(!t.valid);
        let r = analyse(&p, &[[0, 1, 2]], &[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]], [0.0, 0.0, 1.0], 45.0).unwrap();
        assert_eq!(r.undefined, 1);
        assert_eq!(r.max_shear_deg, 0.0);
        assert_eq!(r.colours(), vec![[0.5, 0.5, 0.5]]);
    }

    type TestMesh = (Vec<[f64; 3]>, Vec<[usize; 3]>, Vec<[f64; 2]>);

    /// `(nx+1) x (ny+1)` vertices, two triangles per cell.
    fn grid(nx: usize, ny: usize, f: impl Fn(usize, usize) -> ([f64; 3], [f64; 2])) -> TestMesh {
        let id = |i: usize, j: usize| j * (nx + 1) + i;
        let (mut pos, mut uv) = (Vec::new(), Vec::new());
        for j in 0..=ny {
            for i in 0..=nx {
                let (p, q) = f(i, j);
                pos.push(p);
                uv.push(q);
            }
        }
        let mut tris = Vec::new();
        for j in 0..ny {
            for i in 0..nx {
                tris.push([id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
                tris.push([id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
            }
        }
        (pos, tris, uv)
    }

    #[test]
    fn ply_shear_unfolded_flat_patch_has_zero_shear() {
        let (pos, tris, uv) = grid(5, 4, |i, j| ([3.0 * i as f64, 2.0 * j as f64, 1.0], [3.0 * i as f64, 2.0 * j as f64]));
        let r = analyse(&pos, &tris, &uv, [1.0, 0.4, 0.0], DEFAULT_LOCKING_DEG).unwrap();
        assert_eq!(r.undefined, 0);
        assert!(r.max_shear_deg < 1e-9 && r.mean_shear_deg < 1e-9);
        assert_eq!(r.locked_fraction, 0.0);
        assert!((r.min_stretch - 1.0).abs() < 1e-9 && (r.max_stretch - 1.0).abs() < 1e-9);
    }

    #[test]
    fn ply_shear_cylinder_patch_has_zero_shear_for_any_warp() {
        // faceted cylinder (rulings along z) unrolled facet by facet: an isometry
        let (r, n, dphi) = (20.0f64, 8usize, 0.5 * PI / 8.0);
        let chord = 2.0 * r * (dphi / 2.0).sin();
        let (pos, tris, uv) = grid(n, 5, |i, j| {
            (
                [r * (i as f64 * dphi).cos(), r * (i as f64 * dphi).sin(), 4.0 * j as f64],
                [chord * i as f64, 4.0 * j as f64],
            )
        });
        for warp in [[0.0, 0.0, 1.0], [0.0, 0.6, 0.8]] {
            let rep = analyse(&pos, &tris, &uv, warp, DEFAULT_LOCKING_DEG).unwrap();
            assert_eq!(rep.undefined, 0);
            assert!(rep.max_shear_deg < 1e-9, "warp {warp:?}: {}", rep.max_shear_deg);
        }
    }

    /// Sphere cap radius 10, pole vertex 0, azimuthal-equidistant flat layout.
    fn cap(rings: usize, sectors: usize, max_theta: f64) -> TestMesh {
        let r = 10.0;
        let (mut pos, mut uv) = (vec![[0.0, 0.0, r]], vec![[0.0, 0.0]]);
        for k in 1..=rings {
            let th = max_theta * k as f64 / rings as f64;
            for s in 0..sectors {
                let ph = 2.0 * PI * s as f64 / sectors as f64;
                pos.push([r * th.sin() * ph.cos(), r * th.sin() * ph.sin(), r * th.cos()]);
                uv.push([r * th * ph.cos(), r * th * ph.sin()]);
            }
        }
        let ring = |k: usize, s: usize| 1 + (k - 1) * sectors + s % sectors;
        let mut tris: Vec<[usize; 3]> = (0..sectors).map(|s| [0, ring(1, s), ring(1, s + 1)]).collect();
        for k in 1..rings {
            for s in 0..sectors {
                tris.push([ring(k, s), ring(k + 1, s), ring(k + 1, s + 1)]);
                tris.push([ring(k, s), ring(k + 1, s + 1), ring(k, s + 1)]);
            }
        }
        (pos, tris, uv)
    }

    /// Analytic shear of the equidistant map at polar angle `th`, azimuth `ph`
    /// for the constant warp +x: the map keeps meridians and scales parallels
    /// by `k = th / sin th`; with warp components `a = cos th cos ph` along
    /// the meridian and `b = -sin ph` along the parallel,
    /// `tan(shear) = a b (k^2 - 1) / (k (a^2 + b^2))`.
    fn analytic_shear(th: f64, ph: f64) -> f64 {
        let k = th / th.sin();
        let (a, b) = (th.cos() * ph.cos(), -ph.sin());
        (a * b * (k * k - 1.0) / (k * (a * a + b * b))).atan().to_degrees()
    }

    #[test]
    fn ply_shear_sphere_patch_matches_the_analytic_value_and_grows_from_the_pole() {
        // hand value: th = 50 deg, ph = 45 deg -> k = 1.1393, a = 0.4545, b = -0.7071 -> -6.8 deg
        assert!((analytic_shear(50f64.to_radians(), 45f64.to_radians()) + 6.8).abs() < 0.05);

        let (rings, sectors) = (24usize, 96usize);
        let (pos, tris, uv) = cap(rings, sectors, 60f64.to_radians());
        let rep = analyse(&pos, &tris, &uv, [1.0, 0.0, 0.0], DEFAULT_LOCKING_DEG).unwrap();
        assert_eq!(rep.undefined, 0);
        let mut worst = 0.0f64;
        for (t, ts) in tris.iter().zip(&rep.per_tri) {
            if t.contains(&0) {
                continue; // the pole fan is too coarse for a point-wise comparison
            }
            let c: [f64; 3] = std::array::from_fn(|d| (pos[t[0]][d] + pos[t[1]][d] + pos[t[2]][d]) / 3.0);
            let th = (c[2] / dot(c, c).sqrt()).acos();
            let ph = c[1].atan2(c[0]);
            worst = worst.max((ts.shear_deg - analytic_shear(th, ph)).abs());
        }
        assert!(worst < 2.0, "worst deviation {worst} deg");
        // shear grows away from the pole: mean |shear| of each outer third exceeds the inner one
        let third = |lo: f64, hi: f64| {
            let v: Vec<f64> = tris
                .iter()
                .zip(&rep.per_tri)
                .filter(|(t, _)| !t.contains(&0))
                .filter(|(t, _)| {
                    let z = (pos[t[0]][2] + pos[t[1]][2] + pos[t[2]][2]) / 30.0;
                    let th = z.acos().to_degrees();
                    th >= lo && th < hi
                })
                .map(|(_, s)| s.shear_deg.abs())
                .collect();
            v.iter().sum::<f64>() / v.len() as f64
        };
        let (inner, mid, outer) = (third(0.0, 20.0), third(20.0, 40.0), third(40.0, 61.0));
        assert!(inner < mid && mid < outer, "{inner} {mid} {outer}");
    }

    #[test]
    fn ply_shear_locked_fraction_counts_triangles_beyond_the_angle() {
        // two triangles of a unit square, sheared so both have shear atan(1) = 45 deg and atan(0.5)
        let pos = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
        let uv = [[0.0, 0.0], [1.0, 0.0], [2.0, 1.0], [1.0, 1.0]]; // F = [[1,1],[0,1]]
        let tris = [[0, 1, 2], [0, 2, 3]];
        let r = analyse(&pos, &tris, &uv, [1.0, 0.0, 0.0], 40.0).unwrap();
        assert!((r.max_shear_deg - 45.0).abs() < 1e-9);
        assert_eq!(r.locked_fraction, 1.0);
        let r = analyse(&pos, &tris, &uv, [1.0, 0.0, 0.0], 50.0).unwrap();
        assert_eq!(r.locked_fraction, 0.0);
        assert!((r.colours()[0][0] - 1.0).abs() < 1e-6 && r.colours()[0][1] > 0.1);
        assert_eq!(r.shear_vectors().len(), 2);
        assert!(analyse(&pos, &tris, &uv[..3], [1.0, 0.0, 0.0], 45.0).is_err());
        assert!(analyse(&pos, &tris, &uv, [1.0, 0.0, 0.0], 0.0).is_err());
    }
}

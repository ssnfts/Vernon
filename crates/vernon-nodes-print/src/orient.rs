//! Build-orientation numbers (spec A5), directional strength (section 2.7)
//! and the panel heat balance (A6). Pure functions; the nodes live in
//! `nodes_fit`.

use std::collections::HashMap;

use glam::{DMat3, DQuat, DVec3};
use vernon_core::Geometry;

/// Faces flatter than this (degrees from a layer plane) are exact layer planes
/// and show no stair-step.
pub const FLAT_SLOPE_DEG: f64 = 0.5;

/// Default weights: overhang area fraction, stair-step, 1/r, height fraction.
pub const DEFAULT_WEIGHTS: [f64; 4] = [1.0, 0.5, 2.0, 0.2];

/// Triangle soup of one part with cached normals and areas.
#[derive(Debug, Clone, Default)]
pub struct Tris {
    pub v: Vec<[DVec3; 3]>,
    pub n: Vec<DVec3>,
    pub area: Vec<f64>,
}

impl Tris {
    pub fn new(geo: &Geometry) -> Tris {
        let mut t = Tris::default();
        for (_, [a, b, c]) in geo.triangles() {
            let (pa, pb, pc) = (geo.points[a as usize], geo.points[b as usize], geo.points[c as usize]);
            let cr = (pb - pa).cross(pc - pa);
            let len = cr.length();
            if len < 1e-18 {
                continue;
            }
            t.v.push([pa, pb, pc]);
            t.n.push(cr / len);
            t.area.push(0.5 * len);
        }
        t
    }

    pub fn total_area(&self) -> f64 {
        self.area.iter().sum()
    }

    /// Bounding-box diagonal.
    pub fn diagonal(&self) -> f64 {
        let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
        for t in &self.v {
            for p in t {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
        }
        if lo.x > hi.x {
            0.0
        } else {
            (hi - lo).length()
        }
    }
}

/// Allowable ratio r (relative to the in-plane allowable) for a load whose
/// direction makes the angle phi with the build direction:
/// 1/r^2 = sin^2 phi + cos^2 phi / k^2, with k the Z / in-plane ratio.
pub fn load_ratio(load: DVec3, up: DVec3, k: f64) -> f64 {
    let (l, u) = (load.normalize_or_zero(), up.normalize_or_zero());
    if l == DVec3::ZERO || u == DVec3::ZERO || k <= 0.0 {
        return 1.0;
    }
    let c2 = l.dot(u).powi(2).min(1.0);
    1.0 / ((1.0 - c2) + c2 / (k * k)).sqrt()
}

#[derive(Debug, Clone)]
pub struct OrientParams {
    pub layer_h: f64,
    /// Support threshold angle in degrees.
    pub theta_o_deg: f64,
    /// Load directions (any length; the direction is what matters).
    pub loads: Vec<DVec3>,
    /// Z / in-plane allowable ratio of the material.
    pub k: f64,
    pub weights: [f64; 4],
}

#[derive(Debug, Clone)]
pub struct OrientScore {
    pub up: DVec3,
    pub height: f64,
    pub layers: u64,
    pub overhang_area: f64,
    /// Area-weighted mean of |n.u| over faces that are not exact layer planes;
    /// the stair-step cusp is h times this.
    pub cusp_mean: f64,
    /// Sum of area * h * |n.u| (mm3 of stair-step error volume).
    pub cusp_volume: f64,
    pub r_min: f64,
    pub total: f64,
}

/// Number of layers for a part of height `h_total`.
pub fn layer_count(h_total: f64, layer_h: f64) -> u64 {
    if layer_h <= 0.0 {
        return 0;
    }
    (h_total / layer_h - 1e-9).ceil().max(0.0) as u64
}

/// Score one build direction. Lower totals are better.
pub fn score(tris: &Tris, diag: f64, up: DVec3, p: &OrientParams) -> OrientScore {
    let up = up.normalize_or_zero();
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for t in &tris.v {
        for q in t {
            let d = q.dot(up);
            lo = lo.min(d);
            hi = hi.max(d);
        }
    }
    let height = if lo.is_finite() { hi - lo } else { 0.0 };
    let bed_tol = 1e-6 * diag.max(1.0);
    let cos_o = p.theta_o_deg.to_radians().cos();
    let flat_cos = FLAT_SLOPE_DEG.to_radians().cos();
    let (mut a_o, mut cusp_w) = (0.0, 0.0);
    for i in 0..tris.v.len() {
        let nu = tris.n[i].dot(up);
        let a = tris.area[i];
        if nu.abs() < flat_cos {
            cusp_w += a * nu.abs();
        }
        if nu < -cos_o {
            // Facets lying on the bed need no support.
            let on_bed = tris.v[i].iter().all(|q| q.dot(up) - lo < bed_tol);
            if !on_bed {
                a_o += a;
            }
        }
    }
    let total_area = tris.total_area().max(1e-12);
    let cusp_mean = cusp_w / total_area;
    let r_min = p.loads.iter().map(|l| load_ratio(*l, up, p.k)).fold(1.0f64, f64::min);
    let w = p.weights;
    let total = w[0] * a_o / total_area + w[1] * cusp_mean + w[2] / r_min + w[3] * height / diag.max(1e-12);
    OrientScore {
        up,
        height,
        layers: layer_count(height, p.layer_h),
        overhang_area: a_o,
        cusp_mean,
        cusp_volume: cusp_w * p.layer_h,
        r_min,
        total,
    }
}

/// Stair-step cusp of a facet as a fraction of the layer height.
pub fn facet_cusp_fraction(n: DVec3, up: DVec3) -> f64 {
    n.dot(up.normalize_or_zero()).abs()
}

// ------------------------------------------------------------- candidates

/// The six axis directions, +Z first.
pub fn axis_ups() -> Vec<DVec3> {
    vec![DVec3::Z, DVec3::NEG_Z, DVec3::X, DVec3::NEG_X, DVec3::Y, DVec3::NEG_Y]
}

/// `n` roughly even directions on the sphere (Fibonacci lattice).
pub fn fibonacci_dirs(n: usize) -> Vec<DVec3> {
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    (0..n)
        .map(|i| {
            let z = 1.0 - 2.0 * (i as f64 + 0.5) / n as f64;
            let r = (1.0 - z * z).max(0.0).sqrt();
            let a = golden * i as f64;
            DVec3::new(r * a.cos(), r * a.sin(), z)
        })
        .collect()
}

type Cell = (i32, i32, i32);

/// Up directions that lay the largest flat-ish face groups on the bed: the
/// negated normals of the `max` biggest face clusters (areas summed per
/// quantised normal).
pub fn face_cluster_ups(tris: &Tris, max: usize) -> Vec<DVec3> {
    let mut cells: HashMap<Cell, (f64, DVec3)> = HashMap::new();
    for i in 0..tris.n.len() {
        let n = tris.n[i];
        let key = ((n.x * 20.0).round() as i32, (n.y * 20.0).round() as i32, (n.z * 20.0).round() as i32);
        let e = cells.entry(key).or_insert((0.0, DVec3::ZERO));
        e.0 += tris.area[i];
        e.1 += n * tris.area[i];
    }
    let mut v: Vec<(Cell, (f64, DVec3))> = cells.into_iter().collect();
    v.sort_by(|a, b| b.1 .0.total_cmp(&a.1 .0).then(a.0.cmp(&b.0)));
    v.into_iter().take(max).filter_map(|(_, (_, s))| (-s).try_normalize()).collect()
}

/// Remove directions closer than ~0.4 degrees to an earlier one.
pub fn dedup_dirs(dirs: Vec<DVec3>) -> Vec<DVec3> {
    let mut out: Vec<DVec3> = Vec::new();
    for d in dirs {
        let Some(d) = d.try_normalize() else { continue };
        if !out.iter().any(|o| o.dot(d) > 0.99998) {
            out.push(d);
        }
    }
    out
}

/// Which candidate families to try.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Search {
    Axis,
    Faces,
    Extended,
}

impl Search {
    pub fn parse(s: &str) -> Search {
        match s {
            "axis" => Search::Axis,
            "extended" => Search::Extended,
            _ => Search::Faces,
        }
    }
}

pub fn candidate_ups(tris: &Tris, search: Search) -> Vec<DVec3> {
    let mut v = axis_ups();
    if search != Search::Axis {
        v.extend(face_cluster_ups(tris, 8));
    }
    if search == Search::Extended {
        v.extend(fibonacci_dirs(64));
    }
    dedup_dirs(v)
}

/// Rotation that takes `up` to +Z.
pub fn rotation_to_z(up: DVec3) -> DMat3 {
    DMat3::from_quat(DQuat::from_rotation_arc(up.normalize(), DVec3::Z))
}

// ---------------------------------------------------------------- strength

/// Required cross-section for a tensile force with an in-plane allowable and
/// a Z allowable (spec golden 10).
#[derive(Debug, Clone, Copy)]
pub struct Strength {
    /// Z allowable / in-plane allowable.
    pub z_ratio: f64,
    pub sigma_xy: f64,
    pub sigma_z: f64,
    pub area_inplane: f64,
    pub area_z: f64,
    /// area_z / area_inplane.
    pub area_penalty: f64,
}

pub fn strength(force_n: f64, sf: f64, sigma_xy: f64, z_ratio: f64) -> Strength {
    let sigma_z = sigma_xy * z_ratio;
    let area_inplane = force_n * sf / sigma_xy;
    let area_z = if sigma_z > 0.0 { force_n * sf / sigma_z } else { f64::INFINITY };
    Strength {
        z_ratio,
        sigma_xy,
        sigma_z,
        area_inplane,
        area_z,
        area_penalty: if area_inplane > 0.0 { area_z / area_inplane } else { 1.0 / z_ratio.max(1e-12) },
    }
}

// -------------------------------------------------------------------- heat

const STEFAN_BOLTZMANN: f64 = 5.670_374_419e-8;

#[derive(Debug, Clone, Copy)]
pub struct HeatInput {
    pub ambient_c: f64,
    pub wind_ms: f64,
    pub absorptivity: f64,
    pub irradiance: f64,
    pub emissivity: f64,
    /// Sky temperature below ambient, K.
    pub sky_delta_k: f64,
}

impl Default for HeatInput {
    fn default() -> Self {
        HeatInput { ambient_c: 45.0, wind_ms: 0.0, absorptivity: 0.95, irradiance: 1000.0, emissivity: 0.9, sky_delta_k: 15.0 }
    }
}

/// Steady surface temperature (deg C) of a panel insulated at the back:
/// alpha G = h_c (Ts - Ta) + eps sigma (Ts^4 - Tsky^4), h_c = 5.7 + 3.8 V.
/// Solved by bisection.
pub fn surface_temp_c(h: &HeatInput) -> f64 {
    let ta = h.ambient_c + 273.15;
    let tsky = ta - h.sky_delta_k;
    let hc = 5.7 + 3.8 * h.wind_ms.max(0.0);
    let q_in = h.absorptivity * h.irradiance;
    let f = |ts: f64| hc * (ts - ta) + h.emissivity * STEFAN_BOLTZMANN * (ts.powi(4) - tsky.powi(4)) - q_in;
    let (mut lo, mut hi) = (150.0, 900.0);
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if f(mid) > 0.0 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    0.5 * (lo + hi) - 273.15
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::material;
    use crate::mesh::testutil::tri_box;

    fn params(h: f64, loads: Vec<DVec3>, k: f64) -> OrientParams {
        OrientParams { layer_h: h, theta_o_deg: 45.0, loads, k, weights: DEFAULT_WEIGHTS }
    }

    #[test]
    fn layer_counts_golden() {
        assert_eq!(layer_count(3.0, 0.2), 15);
        assert_eq!(layer_count(100.0, 0.2), 500);
        assert_eq!(layer_count(100.0, 0.12), 834);
    }

    #[test]
    fn strip_flat_vs_upright_golden() {
        let strip = Tris::new(&tri_box(DVec3::ZERO, DVec3::new(100.0, 20.0, 3.0)));
        let diag = strip.diagonal();
        let asa = material("asa");
        let p = params(0.2, vec![DVec3::X], asa.z_ratio);
        let flat = score(&strip, diag, DVec3::Z, &p);
        let upright = score(&strip, diag, DVec3::X, &p);
        assert_eq!(flat.layers, 15);
        assert_eq!(upright.layers, 500);
        assert!((flat.r_min - 1.0).abs() < 1e-12);
        // ASA upright strength is 29 % of flat (shared material table)
        assert!((upright.r_min - 0.29).abs() < 1e-12);
        assert!(flat.total < upright.total);
    }

    #[test]
    fn load_ratio_interpolates() {
        let k = 0.5;
        assert!((load_ratio(DVec3::Z, DVec3::Z, k) - k).abs() < 1e-12);
        assert!((load_ratio(DVec3::X, DVec3::Z, k) - 1.0).abs() < 1e-12);
        let m = load_ratio(DVec3::new(1.0, 0.0, 1.0), DVec3::Z, k);
        assert!(m > k && m < 1.0);
    }

    #[test]
    fn overhang_ignores_bed_contact() {
        let cube = Tris::new(&tri_box(DVec3::ZERO, DVec3::splat(10.0)));
        let s = score(&cube, cube.diagonal(), DVec3::Z, &params(0.2, vec![], 1.0));
        assert_eq!(s.overhang_area, 0.0);
        assert!((s.height - 10.0).abs() < 1e-12);
    }

    #[test]
    fn strength_golden_ppa_cf() {
        let m = material("ppa_cf");
        let s = strength(500.0, 2.0, m.sigma_xy_mpa, m.z_ratio);
        assert!((s.area_inplane - 5.952).abs() < 1e-3);
        assert!((s.area_z - 17.544).abs() < 1e-3);
        assert!((s.area_penalty - 168.0 / 57.0).abs() < 1e-9);
    }

    #[test]
    fn heat_golden_table() {
        let t = |a: f64, ta: f64, v: f64| surface_temp_c(&HeatInput { ambient_c: ta, wind_ms: v, absorptivity: a, ..Default::default() });
        assert!((t(0.95, 45.0, 0.0) - 104.7).abs() < 0.5, "{}", t(0.95, 45.0, 0.0));
        assert!((t(0.30, 45.0, 0.0) - 61.3).abs() < 0.5);
        assert!((t(0.60, 45.0, 0.0) - 82.6).abs() < 0.5);
        assert!((t(0.95, 50.0, 3.0) - 83.9).abs() < 0.5, "{}", t(0.95, 50.0, 3.0));
    }

    #[test]
    fn candidates_are_unique_and_include_axes() {
        let cube = Tris::new(&tri_box(DVec3::ZERO, DVec3::new(10.0, 20.0, 30.0)));
        let a = candidate_ups(&cube, Search::Axis);
        assert_eq!(a.len(), 6);
        let e = candidate_ups(&cube, Search::Extended);
        assert!(e.len() > 60);
        assert_eq!(e[0], DVec3::Z);
    }

    #[test]
    fn rotation_to_z_is_proper() {
        for up in [DVec3::X, DVec3::NEG_Z, DVec3::new(1.0, 2.0, 3.0).normalize()] {
            let r = rotation_to_z(up);
            assert!((r.determinant() - 1.0).abs() < 1e-12);
            assert!((r * up - DVec3::Z).length() < 1e-12);
        }
    }
}

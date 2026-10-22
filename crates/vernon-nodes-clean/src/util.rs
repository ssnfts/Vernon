//! Shared helpers: RNG, parameter checks, point filtering with attribute
//! bookkeeping, JSON conversions for transforms and planes, report helpers.

use glam::{DMat3, DMat4, DVec3};
use serde_json::{json, Value};
use vernon_core::{Geometry, Prim, Report, Severity};
use vernon_graph::{CookContext, CookError, CookResult, Params};

pub const SPEC_ALIGN: &str = "docs/specs/alignment-deviation.md";
pub const SPEC_MESH: &str = "docs/specs/rhino8-mesh-tools.md";

/// Small deterministic PRNG (SplitMix64); no external dependency.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in 0..n (n > 0).
    pub fn below(&mut self, n: usize) -> usize {
        ((self.next_u64() as u128 * n as u128) >> 64) as usize
    }
}

/// Partial Fisher-Yates: after the call `idx[..m]` is a uniform random
/// subset of the indices.
pub fn partial_shuffle(idx: &mut [u32], m: usize, rng: &mut Rng) {
    let n = idx.len();
    for i in 0..m.min(n) {
        let j = i + rng.below(n - i);
        idx.swap(i, j);
    }
}

/// Float parameter checked against its declared range.
pub fn pf(p: &Params, name: &str, min: f64, max: f64) -> CookResult<f64> {
    let v = p.float(name)?;
    if !v.is_finite() || v < min || v > max {
        return Err(CookError::Other(format!(
            "parameter '{name}' = {v} is outside {min}..{max}"
        )));
    }
    Ok(v)
}

/// Integer parameter checked against its declared range.
pub fn pi(p: &Params, name: &str, min: i64, max: i64) -> CookResult<i64> {
    let v = p.int(name)?;
    if v < min || v > max {
        return Err(CookError::Other(format!(
            "parameter '{name}' = {v} is outside {min}..{max}"
        )));
    }
    Ok(v)
}

/// Stage-2 nodes work in millimetres. A geometry whose `metric` detail
/// attribute is present and zero has unknown or arbitrary units (a raw
/// photogrammetry cloud), so cooking is refused until a scale step ran.
pub fn ensure_metric(ctx: &CookContext, g: &Geometry) -> CookResult<()> {
    if let Ok(v) = g.detail_attribs.ints("metric") {
        if v.first() == Some(&0) {
            return Err(ctx.fail(
                "input is not in millimetres (detail attribute 'metric' = 0); apply a scale or unit step first",
            ));
        }
    }
    Ok(())
}

/// Parse "1, 2.5; 3" style lists.
pub fn parse_list(s: &str) -> Vec<f64> {
    s.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse::<f64>().ok())
        .collect()
}

/// "none", "x", "xy"... -> which axes are selected.
pub fn axes_mask(s: &str) -> [bool; 3] {
    let l = s.to_ascii_lowercase();
    [l.contains('x'), l.contains('y'), l.contains('z')]
}

/// Keep the points listed in `idx` (original indices, ascending preferred).
/// Primitives that lose a vertex are dropped; attributes, prim attributes and
/// groups stay consistent.
pub fn gather_points(g: &Geometry, idx: &[usize]) -> Geometry {
    let mut map = vec![u32::MAX; g.points.len()];
    for (new, &old) in idx.iter().enumerate() {
        map[old] = new as u32;
    }
    let points: Vec<DVec3> = idx.iter().map(|&i| g.points[i]).collect();
    let mut prims = Vec::new();
    let mut prim_idx = Vec::new();
    let mut prim_map = vec![u32::MAX; g.prims.len()];
    for (pi, p) in g.prims.iter().enumerate() {
        if p.vertices.iter().all(|&v| map[v as usize] != u32::MAX) {
            prim_map[pi] = prims.len() as u32;
            prims.push(Prim {
                vertices: p.vertices.iter().map(|&v| map[v as usize]).collect(),
                closed: p.closed,
            });
            prim_idx.push(pi);
        }
    }
    let remap = |idx: &[u32], m: &[u32]| -> Vec<u32> {
        let mut v: Vec<u32> = idx
            .iter()
            .filter_map(|&i| m.get(i as usize).copied().filter(|&x| x != u32::MAX))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    Geometry {
        points,
        prims,
        point_attribs: g.point_attribs.gather_all(idx),
        prim_attribs: g.prim_attribs.gather_all(&prim_idx),
        detail_attribs: g.detail_attribs.clone(),
        point_groups: g.point_groups.iter().map(|(k, v)| (k.clone(), remap(v, &map))).collect(),
        prim_groups: g.prim_groups.iter().map(|(k, v)| (k.clone(), remap(v, &prim_map))).collect(),
    }
}

pub fn vec3_json(v: DVec3) -> Value {
    json!([v.x, v.y, v.z])
}

pub fn json_vec3(v: &Value) -> Option<DVec3> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some(DVec3::new(a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?))
}

/// 4x4 row-major matrix of `x -> s R x + t`.
pub fn matrix4(r: DMat3, t: DVec3, s: f64) -> DMat4 {
    let c = r * s;
    DMat4::from_cols(
        c.x_axis.extend(0.0),
        c.y_axis.extend(0.0),
        c.z_axis.extend(0.0),
        t.extend(1.0),
    )
}

/// Transform record exchanged between align nodes (`transform` Json port).
pub fn transform_json(kind: &str, r: DMat3, t: DVec3, s: f64) -> Value {
    let m = matrix4(r, t, s);
    let rows: Vec<Vec<f64>> = (0..4)
        .map(|row| (0..4).map(|col| m.col(col)[row]).collect())
        .collect();
    json!({
        "schema": "transform.v1",
        "kind": kind,
        "matrix": rows,
        "rotation": [r.x_axis.to_array(), r.y_axis.to_array(), r.z_axis.to_array()],
        "translation": vec3_json(t),
        "scale": s,
    })
}

/// Read back `(R, t, s)` from a `transform.v1` record. The rotation is
/// stored as three column vectors.
pub fn json_transform(v: &Value) -> Option<(DMat3, DVec3, f64)> {
    let rot = v.get("rotation")?.as_array()?;
    if rot.len() != 3 {
        return None;
    }
    let r = DMat3::from_cols(json_vec3(&rot[0])?, json_vec3(&rot[1])?, json_vec3(&rot[2])?);
    Some((r, json_vec3(v.get("translation")?)?, v.get("scale")?.as_f64()?))
}

/// Report whose only content is a white-box warning.
pub fn placeholder_report(title: &str, what: &str) -> Report {
    let mut r = Report::new(title);
    r.note(Severity::Warning, format!("white-box placeholder: {what}"));
    r
}

pub fn finite_or(v: f64, fallback: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

/// Point normals from the `N` attribute, if present with the right length.
pub fn point_normals(g: &Geometry) -> Option<&[DVec3]> {
    g.point_attribs
        .vec3s("N")
        .ok()
        .filter(|n| n.len() == g.points.len())
}

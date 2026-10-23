//! Surface-pressure taps to Cp, comparison with a simulated surface Cp, and tuft
//! classification. Spec sections 2.5, 2.6, 3.9 and 3.10.

use glam::DVec3;
use serde_json::{json, Value};
use vernon_core::Geometry;

use crate::util::mean;

/// Unit factor, 1 in. w.c. in Pa.
pub const INWC_PA: f64 = 249.09;

/// A raw tap reading.
#[derive(Debug, Clone, PartialEq)]
pub struct TapIn {
    pub id: String,
    /// Position in mm.
    pub pos: DVec3,
    pub dp_pa: f64,
    pub speed_ms: Option<f64>,
    pub rho: Option<f64>,
}

/// A converted tap.
#[derive(Debug, Clone, PartialEq)]
pub struct CpRow {
    pub id: String,
    pub pos: DVec3,
    pub dp_pa: f64,
    pub speed_ms: f64,
    pub rho: f64,
    pub q_pa: f64,
    pub cp: f64,
    pub cp_sd: f64,
}

/// Conversion settings.
#[derive(Debug, Clone, Copy)]
pub struct CpOpts {
    pub rho_default: Option<f64>,
    pub speed_default: Option<f64>,
    pub speed_sd_ms: f64,
    pub dp_sd_pa: f64,
    pub rho_sd_frac: f64,
}

/// Parse tap rows from a CSV: `id,x,y,z,dp_pa|dp_inwc,speed_ms|speed_kmh[,rho]`.
pub fn parse_taps_csv(text: &str) -> Result<Vec<TapIn>, String> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    let header = lines.next().ok_or("tap file is empty")?;
    let names: Vec<String> = header.split(',').map(|s| s.trim().to_ascii_lowercase()).collect();
    let col = |keys: &[&str]| names.iter().position(|n| keys.contains(&n.as_str()));
    let c_id = col(&["id", "tap", "name"]);
    let (cx, cy, cz) = (col(&["x", "x_mm"]), col(&["y", "y_mm"]), col(&["z", "z_mm"]));
    let c_dp = col(&["dp_pa", "dp"]);
    let c_dpi = col(&["dp_inwc", "dp_in_wc"]);
    let c_v = col(&["speed_ms", "v_ms", "speed"]);
    let c_vk = col(&["speed_kmh", "v_kmh"]);
    let c_rho = col(&["rho", "rho_kgm3"]);
    if c_dp.is_none() && c_dpi.is_none() {
        return Err("tap file needs a dp_pa (or dp_inwc) column".into());
    }
    let mut out = Vec::new();
    for (ln, l) in lines.enumerate() {
        let f: Vec<&str> = l.split(',').map(str::trim).collect();
        let num = |c: Option<usize>| c.and_then(|c| f.get(c)).and_then(|s| s.parse::<f64>().ok());
        let dp = match (num(c_dp), num(c_dpi)) {
            (Some(p), _) => p,
            (None, Some(i)) => i * INWC_PA,
            _ => return Err(format!("tap row {}: no pressure value", ln + 2)),
        };
        out.push(TapIn {
            id: c_id.and_then(|c| f.get(c)).map(|s| s.to_string()).unwrap_or_else(|| format!("tap{}", ln + 1)),
            pos: DVec3::new(num(cx).unwrap_or(0.0), num(cy).unwrap_or(0.0), num(cz).unwrap_or(0.0)),
            dp_pa: dp,
            speed_ms: num(c_v).or_else(|| num(c_vk).map(|k| k / 3.6)),
            rho: num(c_rho),
        });
    }
    if out.is_empty() {
        return Err("tap file has no rows".into());
    }
    Ok(out)
}

/// Parse tap rows from JSON: a list of row objects, or `{"taps": [...]}`.
pub fn parse_taps_json(v: &Value) -> Result<Vec<TapIn>, String> {
    let list = v.get("taps").and_then(Value::as_array).or_else(|| v.as_array()).ok_or("taps: expected a list of rows")?;
    let mut out = Vec::new();
    for (i, r) in list.iter().enumerate() {
        let g = |k: &str| r.get(k).and_then(Value::as_f64);
        let dp = match (g("dp_pa"), g("dp_inwc")) {
            (Some(p), _) => p,
            (None, Some(w)) => w * INWC_PA,
            _ => return Err(format!("tap {i}: needs dp_pa")),
        };
        out.push(TapIn {
            id: r.get("id").map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).unwrap_or_else(|| format!("tap{}", i + 1)),
            pos: DVec3::new(g("x").unwrap_or(0.0), g("y").unwrap_or(0.0), g("z").unwrap_or(0.0)),
            dp_pa: dp,
            speed_ms: g("speed_ms").or_else(|| g("speed_kmh").map(|k| k / 3.6)),
            rho: g("rho"),
        });
    }
    if out.is_empty() {
        return Err("taps: the list is empty".into());
    }
    Ok(out)
}

/// `Cp = dp / q` with the propagated uncertainty
/// `sigma_Cp^2 = (sigma_dp/q)^2 + Cp^2 [(sigma_rho/rho)^2 + (2 sigma_v/v)^2]`.
pub fn compute_cp(taps: &[TapIn], o: &CpOpts) -> Result<Vec<CpRow>, String> {
    let mut rows = Vec::with_capacity(taps.len());
    for t in taps {
        let v = t.speed_ms.or(o.speed_default).ok_or_else(|| format!("tap {}: no speed (column or parameter)", t.id))?;
        let rho = t.rho.or(o.rho_default).ok_or_else(|| format!("tap {}: no density (column, env input or parameter)", t.id))?;
        if !(v.is_finite() && v > 0.0 && rho.is_finite() && rho > 0.0) {
            return Err(format!("tap {}: speed and density must be positive", t.id));
        }
        let q = 0.5 * rho * v * v;
        let cp = t.dp_pa / q;
        let var = (o.dp_sd_pa / q).powi(2) + cp * cp * (o.rho_sd_frac.powi(2) + (2.0 * o.speed_sd_ms / v).powi(2));
        rows.push(CpRow { id: t.id.clone(), pos: t.pos, dp_pa: t.dp_pa, speed_ms: v, rho, q_pa: q, cp, cp_sd: var.sqrt() });
    }
    Ok(rows)
}

pub fn cp_table_json(rows: &[CpRow]) -> Value {
    json!({
        "kind": "cp_table.v1",
        "rows": rows.iter().map(|r| json!({
            "id": r.id, "x": r.pos.x, "y": r.pos.y, "z": r.pos.z, "dp_pa": r.dp_pa,
            "speed_ms": r.speed_ms, "rho": r.rho, "q_pa": r.q_pa, "cp": r.cp, "cp_sd": r.cp_sd,
        })).collect::<Vec<_>>(),
    })
}

pub fn cp_table_from_json(v: &Value) -> Result<Vec<CpRow>, String> {
    let list = v.get("rows").and_then(Value::as_array).ok_or("cp table: missing rows")?;
    let mut out = Vec::new();
    for (i, r) in list.iter().enumerate() {
        let g = |k: &str| r.get(k).and_then(Value::as_f64).ok_or_else(|| format!("cp row {i}: missing {k}"));
        out.push(CpRow {
            id: r.get("id").and_then(Value::as_str).map(String::from).unwrap_or_else(|| format!("tap{}", i + 1)),
            pos: DVec3::new(g("x")?, g("y")?, g("z")?),
            dp_pa: g("dp_pa").unwrap_or(f64::NAN),
            speed_ms: g("speed_ms").unwrap_or(f64::NAN),
            rho: g("rho").unwrap_or(f64::NAN),
            q_pa: g("q_pa").unwrap_or(f64::NAN),
            cp: g("cp")?,
            cp_sd: g("cp_sd").unwrap_or(0.0),
        });
    }
    if out.is_empty() {
        return Err("cp table has no rows".into());
    }
    Ok(out)
}

// ---------------------------------------------------------------- surface sampling

/// Closest point on triangle `abc` to `p`, with its barycentric coordinates.
pub fn closest_on_triangle(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> (DVec3, [f64; 3]) {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, [1.0, 0.0, 0.0]);
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return (b, [0.0, 1.0, 0.0]);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (a + ab * v, [1.0 - v, v, 0.0]);
    }
    let cp_ = p - c;
    let d5 = ab.dot(cp_);
    let d6 = ac.dot(cp_);
    if d6 >= 0.0 && d5 <= d6 {
        return (c, [0.0, 0.0, 1.0]);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (a + ac * w, [1.0 - w, 0.0, w]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (b + (c - b) * w, [0.0, 1.0 - w, w]);
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (a + ab * v + ac * w, [1.0 - v - w, v, w])
}

/// Where the surface Cp lives.
pub enum SurfaceCp<'a> {
    Point(&'a [f64]),
    Prim(&'a [f64]),
}

/// Find the surface Cp field: point attribute `Cp` first, then prim attribute `Cp`.
pub fn surface_cp(geo: &Geometry) -> Result<SurfaceCp<'_>, String> {
    if let Ok(v) = geo.point_attribs.floats("Cp") {
        return Ok(SurfaceCp::Point(v));
    }
    if let Ok(v) = geo.prim_attribs.floats("Cp") {
        return Ok(SurfaceCp::Prim(v));
    }
    Err("the surface has no float attribute 'Cp' on points or faces".into())
}

/// A sampled surface value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub cp: f64,
    /// Distance from the tap to the surface, mm.
    pub offset_mm: f64,
    pub triangles: usize,
}

/// Area-average the surface Cp over a disc of `radius_mm` about the tap's projection.
pub fn sample_surface(
    geo: &Geometry,
    tris: &[(usize, [u32; 3])],
    field: &SurfaceCp<'_>,
    tap: DVec3,
    radius_mm: f64,
) -> Option<Sample> {
    if tris.is_empty() {
        return None;
    }
    let pt = |i: u32| geo.points[i as usize];
    let mut best = (f64::MAX, DVec3::ZERO);
    for (_, t) in tris {
        let (q, _) = closest_on_triangle(tap, pt(t[0]), pt(t[1]), pt(t[2]));
        let d = q.distance(tap);
        if d < best.0 {
            best = (d, q);
        }
    }
    let proj = best.1;
    let (mut acc, mut wsum, mut cnt) = (0.0, 0.0, 0usize);
    let mut nearest = (f64::MAX, 0.0);
    for (pi, t) in tris {
        let (a, b, c) = (pt(t[0]), pt(t[1]), pt(t[2]));
        let (q, bary) = closest_on_triangle(proj, a, b, c);
        let d = q.distance(proj);
        let val = match field {
            SurfaceCp::Point(v) => {
                bary[0] * v[t[0] as usize] + bary[1] * v[t[1] as usize] + bary[2] * v[t[2] as usize]
            }
            SurfaceCp::Prim(v) => v[*pi],
        };
        if d < nearest.0 {
            nearest = (d, val);
        }
        if d <= radius_mm {
            let area = 0.5 * (b - a).cross(c - a).length();
            acc += area * val;
            wsum += area;
            cnt += 1;
        }
    }
    let cp = if wsum > 0.0 { acc / wsum } else { nearest.1 };
    Some(Sample { cp, offset_mm: best.0, triangles: cnt.max(1) })
}

/// Per-tap comparison row.
#[derive(Debug, Clone, PartialEq)]
pub struct CompareRow {
    pub id: String,
    pub cp_meas: f64,
    pub cp_sd: f64,
    pub cp_sim: f64,
    pub residual: f64,
    pub tol: f64,
    pub pass: bool,
    pub sign_ok: bool,
    pub offset_mm: f64,
}

/// Summary of the comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct CompareResult {
    pub rows: Vec<CompareRow>,
    pub bias: f64,
    pub rms: f64,
    pub max_abs: f64,
    pub sign_agree: usize,
    pub failed: usize,
}

/// Cp values closer to zero than this count as "no sign" (about 1 Pa at 100 km/h).
pub const SIGN_DEADBAND: f64 = 0.02;

/// Compare measured and simulated Cp: `residual = Cp_sim - Cp_meas`; a tap passes when
/// `|residual| <= max(tol, sigma_k * sigma_Cp)`.
pub fn compare(
    taps: &[CpRow],
    geo: &Geometry,
    radius_mm: f64,
    tol: f64,
    sigma_k: f64,
) -> Result<CompareResult, String> {
    let field = surface_cp(geo)?;
    let tris = geo.triangles();
    if tris.is_empty() {
        return Err("the surface has no polygon faces".into());
    }
    let mut rows = Vec::with_capacity(taps.len());
    for t in taps {
        let s = sample_surface(geo, &tris, &field, t.pos, radius_mm).ok_or("empty surface")?;
        let residual = s.cp - t.cp;
        let tol_i = tol.max(sigma_k * t.cp_sd);
        let sign_ok = (s.cp >= 0.0) == (t.cp >= 0.0) || (s.cp.abs() <= SIGN_DEADBAND && t.cp.abs() <= SIGN_DEADBAND);
        rows.push(CompareRow {
            id: t.id.clone(),
            cp_meas: t.cp,
            cp_sd: t.cp_sd,
            cp_sim: s.cp,
            residual,
            tol: tol_i,
            pass: residual.abs() <= tol_i,
            sign_ok,
            offset_mm: s.offset_mm,
        });
    }
    let res: Vec<f64> = rows.iter().map(|r| r.residual).collect();
    let n = rows.len().max(1) as f64;
    Ok(CompareResult {
        bias: mean(&res),
        rms: (res.iter().map(|r| r * r).sum::<f64>() / n).sqrt(),
        max_abs: res.iter().fold(0.0, |m, r| f64::max(m, r.abs())),
        sign_agree: rows.iter().filter(|r| r.sign_ok).count(),
        failed: rows.iter().filter(|r| !r.pass).count(),
        rows,
    })
}

// ---------------------------------------------------------------- tufts

/// Tuft behaviour classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuftClass {
    Attached = 0,
    Separated = 1,
    Reversed = 2,
}

impl TuftClass {
    pub fn name(self) -> &'static str {
        match self {
            TuftClass::Attached => "attached",
            TuftClass::Separated => "separated",
            TuftClass::Reversed => "reversed",
        }
    }
}

/// Thresholds in degrees.
#[derive(Debug, Clone, Copy)]
pub struct TuftThresholds {
    pub attached_deg: f64,
    pub reversed_deg: f64,
    pub sd_attached_deg: f64,
    pub sd_separated_deg: f64,
}

/// Circular mean and standard deviation of angles in degrees, `(mean, sd)`.
pub fn circular_stats_deg(angles: &[f64]) -> (f64, f64) {
    if angles.is_empty() {
        return (0.0, 0.0);
    }
    let n = angles.len() as f64;
    let c = angles.iter().map(|a| a.to_radians().cos()).sum::<f64>() / n;
    let s = angles.iter().map(|a| a.to_radians().sin()).sum::<f64>() / n;
    let r = (c * c + s * s).sqrt().clamp(1e-12, 1.0);
    (s.atan2(c).to_degrees(), (-2.0 * r.ln()).sqrt().to_degrees())
}

/// Smallest signed difference `a - b` wrapped to (-180, 180].
pub fn angle_diff_deg(a: f64, b: f64) -> f64 {
    let mut d = (a - b) % 360.0;
    if d > 180.0 {
        d -= 360.0;
    } else if d <= -180.0 {
        d += 360.0;
    }
    d
}

/// Classify one tuft from its angle series and the local flow direction.
/// Returns `(class, mean offset from the flow, sd)`.
pub fn classify_tuft(angles_deg: &[f64], flow_deg: f64, th: &TuftThresholds) -> (TuftClass, f64, f64) {
    let (mean_a, sdv) = circular_stats_deg(angles_deg);
    let off = angle_diff_deg(mean_a, flow_deg);
    let class = if off.abs() > th.reversed_deg {
        TuftClass::Reversed
    } else if sdv > th.sd_separated_deg {
        TuftClass::Separated
    } else if off.abs() <= th.attached_deg && sdv <= th.sd_attached_deg {
        TuftClass::Attached
    } else {
        TuftClass::Separated
    };
    (class, off, sdv)
}

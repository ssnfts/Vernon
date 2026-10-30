//! Steady surface-temperature balance, balance sweep, material heat table
//! and heat-deflection check.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use vernon_core::{keys, AttribArray, Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{
    CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params,
    PortSpec, Stage,
};

use crate::util::*;
use crate::weather::{ir_from_corr, tsky_from_ir, Weather, SIGMA};

const K0: f64 = 273.15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HcModel {
    McAdams,
    MowittWindward,
    MowittLeeward,
    Natural,
}

impl HcModel {
    pub fn parse(s: &str) -> Option<HcModel> {
        Some(match s {
            "mcadams" => HcModel::McAdams,
            "mowitt_windward" => HcModel::MowittWindward,
            "mowitt_leeward" => HcModel::MowittLeeward,
            "natural" => HcModel::Natural,
            _ => return None,
        })
    }
}

/// Convection coefficient, W/m2K. `dt` is |Ts - Ta| in kelvin.
pub fn hc(model: HcModel, wind: f64, dt: f64) -> f64 {
    let nat = 1.31 * dt.cbrt();
    match model {
        HcModel::McAdams => 5.7 + 3.8 * wind,
        HcModel::Natural => nat,
        HcModel::MowittWindward => {
            ((0.84 * dt.cbrt()).powi(2) + (3.26 * wind.powf(0.89)).powi(2)).sqrt()
        }
        HcModel::MowittLeeward => {
            ((0.84 * dt.cbrt()).powi(2) + (3.55 * wind.powf(0.617)).powi(2)).sqrt()
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Balance {
    pub alpha: f64,
    pub eps: f64,
    pub g: f64,
    pub ta_k: f64,
    pub tsky_k: f64,
    pub wind: f64,
    /// Cosine of the tilt from +Z (normal z component).
    pub cos_tilt: f64,
    pub tg_k: f64,
    pub hb: f64,
    pub tin_k: f64,
    pub model: HcModel,
}

impl Balance {
    fn residual(&self, ts: f64) -> f64 {
        let fsky = (1.0 + self.cos_tilt) * 0.5;
        let fg = 1.0 - fsky;
        let h = hc(self.model, self.wind, (ts - self.ta_k).abs());
        self.alpha * self.g
            - h * (ts - self.ta_k)
            - self.eps
                * SIGMA
                * (fsky * (ts.powi(4) - self.tsky_k.powi(4))
                    + fg * (ts.powi(4) - self.tg_k.powi(4)))
            - self.hb * (ts - self.tin_k)
    }
}

/// Solve the surface balance by bisection (the residual is strictly
/// decreasing in the surface temperature). Returns `(Ts in K, hc, converged)`.
pub fn solve_surface_temp(b: &Balance) -> (f64, f64, bool) {
    let (mut lo, mut hi) = (b.ta_k - 60.0, b.ta_k + 250.0);
    if b.residual(lo) < 0.0 {
        return (lo, hc(b.model, b.wind, (lo - b.ta_k).abs()), false);
    }
    if b.residual(hi) > 0.0 {
        return (hi, hc(b.model, b.wind, (hi - b.ta_k).abs()), false);
    }
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if b.residual(mid) > 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
        if hi - lo < 1e-7 {
            break;
        }
    }
    let ts = 0.5 * (lo + hi);
    (ts, hc(b.model, b.wind, (ts - b.ta_k).abs()), true)
}

// ---------------------------------------------------------------------------
// Materials
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Material {
    pub name: String,
    pub hdt_datasheet_c: Option<f64>,
    pub hdt_as_printed_c: Option<f64>,
    pub hdt_annealed_c: Option<f64>,
    pub tg_c: Option<f64>,
    pub water_abs_pct: Option<f64>,
    /// Strength kept after accelerated UV exposure, percent; `None` when no evidence exists.
    #[serde(default)]
    pub uv_retention_pct: Option<f64>,
    /// Colour change dE after the same exposure, when a number is known.
    #[serde(default)]
    pub uv_delta_e: Option<f64>,
    pub source: String,
    pub note: String,
}

fn mat(
    name: &str,
    ds: Option<f64>,
    asp: Option<f64>,
    ann: Option<f64>,
    tg: Option<f64>,
    w: Option<f64>,
    note: &str,
) -> Material {
    Material {
        name: name.into(),
        hdt_datasheet_c: ds,
        hdt_as_printed_c: asp,
        hdt_annealed_c: ann,
        tg_c: tg,
        water_abs_pct: w,
        uv_retention_pct: None,
        uv_delta_e: None,
        source: "rep".into(),
        note: note.into(),
    }
}

/// Heat-deflection figures at 0.45 MPa: the printable materials come from the
/// shared table (`vernon_core::materials`, also read by the print and
/// structural stages); two report-only rows follow.
pub fn builtin_materials() -> Vec<Material> {
    let mut rows: Vec<Material> = vernon_core::materials::PRINT_MATERIALS
        .iter()
        .map(|m| {
            let mut note = m.hdt_basis.to_string();
            if let Some(w) = m.water_abs_pct {
                note += &format!("; water uptake {w}% ({})", m.water_source.label());
            }
            let mut row = mat(
                m.name,
                Some(m.hdt_vendor_c.unwrap_or(m.hdt_045_c)),
                m.hdt_045_annealed_c.map(|_| m.hdt_045_c),
                m.hdt_045_annealed_c,
                None,
                m.water_abs_pct,
                &note,
            );
            if let Some(uv) = m.uv {
                row.uv_retention_pct = Some(uv.retention_pct);
                row.uv_delta_e = uv.delta_e;
                row.note += &format!("; UV: {} ({})", uv.basis, uv.source.label());
            }
            row
        })
        .collect();
    rows.extend([
        mat(
            "PA6-CF",
            None,
            None,
            None,
            None,
            Some(2.35),
            "no heat-deflection figure given",
        ),
        mat(
            "CF-ABS",
            None,
            None,
            None,
            Some(53.5),
            None,
            "measured glass transition of a failed part; test the real spool",
        ),
    ]);
    rows
}

impl Material {
    /// Effective heat-deflection temperature, degC.
    pub fn hdt_eff(&self, annealed: bool, use_tg: bool) -> Option<f64> {
        let base = if annealed {
            self.hdt_annealed_c.or(self.hdt_datasheet_c)
        } else {
            self.hdt_as_printed_c.or(self.hdt_datasheet_c)
        };
        let tg = if use_tg { self.tg_c } else { None };
        match (base, tg) {
            (Some(b), Some(t)) => Some(b.min(t)),
            (Some(b), None) => Some(b),
            (None, Some(t)) => Some(t),
            (None, None) => None,
        }
    }
}

pub fn materials_from_json(v: &Value) -> Result<Vec<Material>, String> {
    let rows = v.get("rows").ok_or("not a material table (no rows)")?;
    serde_json::from_value(rows.clone()).map_err(|e| format!("bad material rows: {e}"))
}

fn materials_json(rows: &[Material]) -> Value {
    json!({ "schema": "materials.v1", "rows": rows })
}

fn parse_csv_materials(text: &str) -> Result<Vec<Material>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        if line.trim().is_empty() || i == 0 && f[0].eq_ignore_ascii_case("name") {
            continue;
        }
        if f.len() < 2 {
            return Err(format!("line {}: need at least name and hdt_c", i + 1));
        }
        let num = |k: usize| f.get(k).and_then(|s| s.parse::<f64>().ok());
        out.push(Material {
            name: f[0].to_string(),
            hdt_datasheet_c: num(1),
            hdt_as_printed_c: None,
            hdt_annealed_c: num(2),
            tg_c: num(3),
            water_abs_pct: num(4),
            uv_retention_pct: None,
            uv_delta_e: None,
            source: f.get(5).copied().unwrap_or("user").to_string(),
            note: String::new(),
        });
    }
    if out.is_empty() {
        return Err("the file has no material rows".into());
    }
    Ok(out)
}

pub struct MaterialTableNode;

impl NodeOp for MaterialTableNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("solar.material_table", "Material Heat Table", Stage::Solar)
            .output(PortSpec::new("materials", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("preset", "Preset", "report_v1", &["report_v1"]).help("Built-in table of heat-deflection temperatures for common printing materials."))
            .param(ParamSpec::path("csv_path", "User table", &["csv"], false).help("Optional CSV (name, hdt_c, hdt_annealed_c, tg_c, water_abs_pct, source). Replaces the preset when set."))
            .help("Heat-deflection temperature, glass transition and water absorption of printing materials, from the built-in table or a user CSV.")
            .emulates("Material heat table of the report (datasheet values)")
            .spec(SOLAR_SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, _ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let path = p.str("csv_path")?;
        let rows = if path.trim().is_empty() {
            builtin_materials()
        } else {
            let text = std::fs::read_to_string(path)
                .map_err(|e| ctx.fail(format!("cannot read '{path}': {e}")))?;
            parse_csv_materials(&text).map_err(|e| ctx.fail(e))?
        };
        let mut r = Report::new("Material table");
        r.metric("materials", Metric::new(rows.len() as f64, "count"));
        r.note(
            Severity::Info,
            "datasheet values: test the actual spool; annealed values apply only after a post-print anneal",
        );
        Ok(vec![Data::json(materials_json(&rows)), Data::report(r)])
    }
}

// ---------------------------------------------------------------------------
// solar.surface_balance
// ---------------------------------------------------------------------------

pub struct SurfaceBalanceNode;

fn wind_at_height(v_met: f64, terrain: &str, z: f64) -> f64 {
    let (a, delta) = if terrain == "city" {
        (0.33, 460.0)
    } else {
        (0.14, 270.0)
    };
    v_met * (270.0f64 / 10.0).powf(0.14) * (z / delta).powf(a)
}

impl NodeOp for SurfaceBalanceNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("solar.surface_balance", "Surface Temperature Balance", Stage::Solar)
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .input(PortSpec::optional("weather", DataKind::Json))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("alpha", "Solar absorptance", 0.95, 0.0, 1.0).help("0.95 black or raw carbon fibre, 0.6 mid grey, 0.3 white. A per-face 'alpha' attribute overrides it."))
            .param(ParamSpec::float("emissivity", "Emissivity", 0.9, 0.0, 1.0).help("Long-wave emissivity of the surface, 0 to 1."))
            .param(ParamSpec::float("t_air_c", "Air temperature", 45.0, -60.0, 70.0).unit("degC").help("Used when no weather table is connected."))
            .param(ParamSpec::float("wind_m_s", "Wind speed", 0.0, 0.0, 40.0).unit("m/s").help("Speed of air relative to the panel. Used when no weather table is connected."))
            .param(ParamSpec::choice("hc_model", "Convection", "mcadams", &["mcadams", "mowitt_windward", "mowitt_leeward", "natural"]).help("Convection coefficient correlation. The default is 5.7 + 3.8 V."))
            .param(ParamSpec::choice("sky_model", "Sky temperature", "fixed_depression", &["fixed_depression", "epw"]).help("Fixed drop below air temperature, or from the weather row (measured infrared or correlation)."))
            .param(ParamSpec::float("depression_k", "Sky depression", 15.0, 0.0, 60.0).unit("K").help("How far the effective sky temperature lies below the air temperature, in kelvin."))
            .param(ParamSpec::float("ground_offset_k", "Ground above air", 0.0, -20.0, 60.0).unit("K").help("Ground temperature minus air temperature (a hot road is worse)."))
            .param(ParamSpec::float("back_h", "Back-face coefficient", 0.0, 0.0, 100.0).unit("W/m2K").help("0 means an insulated back: an upper bound."))
            .param(ParamSpec::float("t_inside_c", "Interior temperature", 45.0, -60.0, 120.0).unit("degC").help("Temperature on the inside of the panel, in degrees Celsius. Heat flows through the panel toward it."))
            .param(ParamSpec::float("g_fallback_w_m2", "Irradiance fallback", 1000.0, 0.0, 1400.0).unit("W/m2").help("Used when the mesh has no irr_w_m2 attribute."))
            .param(ParamSpec::bool("use_weather", "Use weather row", true).help("Take air temperature, wind and sky from the connected weather row."))
            .param(ParamSpec::int("weather_row", "Weather row", 0, 0, 8783).help("Row of the connected weather table to use, counted from 0."))
            .param(ParamSpec::bool("wind_height_scale", "Scale wind to panel height", false).help("Convert the weather-station wind speed to the height of the panel."))
            .param(ParamSpec::choice("terrain", "Terrain", "flat", &["flat", "city"]).help("Terrain class that scales the measured wind to the panel height: open flat ground or city."))
            .param(ParamSpec::float("panel_height_m", "Panel height", 0.5, 0.05, 10.0).unit("m").help("Height of the panel above the ground, in metres. With the terrain it scales the wind speed to the panel."))
            .help("Steady energy balance per face: absorbed sun equals convection plus radiation to sky and ground (and optional back loss). Writes the surface temperature per face and reports the hottest one.")
            .emulates("Peak-hour hand energy balance of the report")
            .spec(SOLAR_SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        if face_count(&g) == 0 {
            return Err(ctx.fail("the mesh has no faces"));
        }
        let model = HcModel::parse(p.str("hc_model")?)
            .ok_or_else(|| CookError::BadParam("hc_model".into()))?;
        let (mut ta_c, mut wind, mut sky_c) = (p.float("t_air_c")?, p.float("wind_m_s")?, None);
        let mut r = Report::new("Surface temperature");
        if p.bool("use_weather")? {
            if let Some(wv) = opt_json_in(ins, 1)? {
                let w = Weather::from_json(&wv).map_err(|e| ctx.fail(e))?;
                let idx =
                    (p.int("weather_row")?.max(0) as usize).min(w.rows.len().saturating_sub(1));
                let row = w
                    .rows
                    .get(idx)
                    .ok_or_else(|| ctx.fail("weather table has no rows"))?;
                ta_c = row.t_db;
                wind = row.wind;
                if p.bool("wind_height_scale")? {
                    wind = wind_at_height(wind, p.str("terrain")?, p.float("panel_height_m")?);
                }
                if p.str("sky_model")? == "epw" {
                    let ir = row
                        .ir
                        .unwrap_or_else(|| ir_from_corr(row.t_db, row.t_dp, row.opaque_cover));
                    sky_c = Some(tsky_from_ir(ir));
                }
            }
        }
        let sky_c = sky_c.unwrap_or(ta_c - p.float("depression_k")?);
        let g_attr = g.prim_attribs.floats("irr_w_m2").ok();
        if g_attr.is_none() {
            r.note(
                Severity::Info,
                "no irr_w_m2 attribute: the fallback irradiance is used for every face",
            );
        }
        let alpha_attr = g.prim_attribs.floats("alpha").ok();
        let np = g.prims.len();
        let areas = prim_areas(&g);
        let g_fb = p.float("g_fallback_w_m2")?;
        let alpha0 = p.float("alpha")?;
        let eps = p.float("emissivity")?;
        let (hb, tin) = (p.float("back_h")?, p.float("t_inside_c")? + K0);
        let tg = ta_c + K0 + p.float("ground_offset_k")?;
        let mut ts_c = vec![0.0; np];
        let mut hcs = vec![0.0; np];
        let mut bad = 0usize;
        let mut gmax = 0.0f64;
        for i in 0..np {
            if !(g.prims[i].closed && g.prims[i].vertices.len() >= 3) || areas[i] <= 1e-12 {
                ts_c[i] = ta_c;
                continue;
            }
            let n = g.prim_normal(i);
            let gi = g_attr.map_or(g_fb, |a| a[i]);
            gmax = gmax.max(gi);
            let b = Balance {
                alpha: alpha_attr.map_or(alpha0, |a| a[i]),
                eps,
                g: gi,
                ta_k: ta_c + K0,
                tsky_k: sky_c + K0,
                wind,
                cos_tilt: n.z,
                tg_k: tg,
                hb,
                tin_k: tin,
                model,
            };
            let (ts, h, ok) = solve_surface_temp(&b);
            ts_c[i] = ts - K0;
            hcs[i] = h;
            if !ok {
                bad += 1;
            }
        }
        let atot: f64 = areas.iter().sum();
        let mean = if atot > 0.0 {
            ts_c.iter().zip(&areas).map(|(t, a)| t * a).sum::<f64>() / atot
        } else {
            ta_c
        };
        let valid: Vec<f64> = (0..np)
            .filter(|&i| areas[i] > 1e-12)
            .map(|i| ts_c[i])
            .collect();
        let tmax = valid.iter().cloned().fold(f64::MIN, f64::max);
        let tmin = valid.iter().cloned().fold(f64::MAX, f64::min);
        r.metric("max_temp_c", Metric::new(tmax, "degC"));
        r.metric("mean_temp_c", Metric::new(mean, "degC"));
        r.metric("min_temp_c", Metric::new(tmin, "degC"));
        r.metric("peak_irradiance_w_m2", Metric::new(gmax, "W/m2"));
        r.metric("ambient_c", Metric::new(ta_c, "degC"));
        r.metric("sky_temp_c", Metric::new(sky_c, "degC"));
        r.metric("wind_m_s", Metric::new(wind, "m/s"));
        r.metric(
            "non_converged_faces",
            Metric::new(bad as f64, "count").with_limit(0.0, true),
        );
        let span = (tmax - tmin).max(1e-9);
        let mut out = (*g).clone();
        out.prim_attribs
            .set("temp_c", AttribArray::Float(ts_c.clone()));
        out.prim_attribs.set("hc_w_m2k", AttribArray::Float(hcs));
        out.prim_attribs.set(
            "Cd",
            AttribArray::Vec3(ts_c.iter().map(|t| ramp_seq((t - tmin) / span)).collect()),
        );
        let pt = prim_to_point(&g, &ts_c);
        out.point_attribs.set(
            "Cd",
            AttribArray::Vec3(pt.iter().map(|t| ramp_seq((t - tmin) / span)).collect()),
        );
        out.point_attribs.set("temp_c", AttribArray::Float(pt));
        Ok(vec![Data::geometry(out), Data::report(r)])
    }
}

// ---------------------------------------------------------------------------
// solar.balance_sweep
// ---------------------------------------------------------------------------

pub struct BalanceSweepNode;

impl NodeOp for BalanceSweepNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("solar.balance_sweep", "Balance Sweep Table", Stage::Solar)
            .output(PortSpec::new("table", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::string("alphas", "Absorptances", "0.95, 0.6, 0.3").help("Comma-separated list."))
            .param(ParamSpec::string("winds", "Wind speeds", "0, 1, 3").help("Comma-separated list, m/s."))
            .param(ParamSpec::string("t_airs", "Air temperatures", "45, 50").help("Comma-separated list, degC."))
            .param(ParamSpec::float("g_w_m2", "Irradiance", 1000.0, 0.0, 1400.0).unit("W/m2").help("Solar irradiance on the surface, in W/m2."))
            .param(ParamSpec::float("emissivity", "Emissivity", 0.9, 0.0, 1.0).help("Long-wave emissivity of the surface, 0 to 1."))
            .param(ParamSpec::float("depression_k", "Sky depression", 15.0, 0.0, 60.0).unit("K").help("How far the effective sky temperature lies below the air temperature, in kelvin."))
            .param(ParamSpec::float("tilt_deg", "Tilt from horizontal", 0.0, 0.0, 180.0).unit("deg").help("Tilt of the surface from horizontal: 0 faces up, 90 is vertical."))
            .param(ParamSpec::choice("hc_model", "Convection", "mcadams", &["mcadams", "mowitt_windward", "mowitt_leeward", "natural"]).help("Convection model for the surface-to-air heat transfer."))
            .help("Tabulates the steady surface temperature for every combination of absorptance, wind speed and air temperature: the design table of the thermal check.")
            .emulates("Result table of the report's hand model")
            .spec(SOLAR_SPEC)
            .implemented()
    }

    fn cook(&self, _ctx: &CookContext, _ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let alphas = parse_list(p.str("alphas")?, "alphas")?;
        let winds = parse_list(p.str("winds")?, "winds")?;
        let tairs = parse_list(p.str("t_airs")?, "t_airs")?;
        let model = HcModel::parse(p.str("hc_model")?)
            .ok_or_else(|| CookError::BadParam("hc_model".into()))?;
        let tilt = p.float("tilt_deg")?.to_radians();
        let dep = p.float("depression_k")?;
        let mut rows = Vec::new();
        let mut r = Report::new("Balance sweep");
        let mut tmax = f64::MIN;
        let mut bad = 0.0;
        for &a in &alphas {
            for &v in &winds {
                for &t in &tairs {
                    let b = Balance {
                        alpha: a,
                        eps: p.float("emissivity")?,
                        g: p.float("g_w_m2")?,
                        ta_k: t + K0,
                        tsky_k: t - dep + K0,
                        wind: v,
                        cos_tilt: tilt.cos(),
                        tg_k: t + K0,
                        hb: 0.0,
                        tin_k: t + K0,
                        model,
                    };
                    let (ts, h, ok) = solve_surface_temp(&b);
                    if !ok {
                        bad += 1.0;
                    }
                    tmax = tmax.max(ts - K0);
                    rows.push(json!({ "alpha": a, "wind_m_s": v, "t_air_c": t, "ts_c": ts - K0, "hc_w_m2k": h }));
                    r.metric(
                        format!("ts_c.alpha{a}.v{v}.ta{t}"),
                        Metric::new(ts - K0, "degC"),
                    );
                }
            }
        }
        r.metric("max_ts_c", Metric::new(tmax, "degC"));
        r.metric(
            "non_converged",
            Metric::new(bad, "count").with_limit(0.0, true),
        );
        Ok(vec![
            Data::json(json!({ "schema": "balance_sweep.v1", "rows": rows })),
            Data::report(r),
        ])
    }
}

// ---------------------------------------------------------------------------
// solar.hdt_check
// ---------------------------------------------------------------------------

pub struct HdtCheckNode;

/// Black-panel temperature of the SAE J2527 weathering cycle, degC.
pub const J2527_PANEL_C: f64 = 70.0;

fn resolve<'a>(table: &'a [Material], name: &str) -> Option<&'a Material> {
    table.iter().find(|m| m.name.eq_ignore_ascii_case(name))
}

impl NodeOp for HdtCheckNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("solar.hdt_check", "Heat Deflection Check", Stage::Solar)
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .input(PortSpec::optional("materials", DataKind::Json))
            .input(PortSpec::optional("material_report", DataKind::Report))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("material", "Material", "ASA", &["PLA", "PETG", "ASA", "PET-CF", "PAHT-CF", "PPA-CF", "PA6-CF", "CF-ABS"]).help("Used for every face unless the mesh has a per-face 'material' name attribute."))
            .param(ParamSpec::bool("annealed", "Annealed", false).help("Use the annealed heat-deflection value where the material has one."))
            .param(ParamSpec::float("margin_warn_k", "Warning margin", 10.0, 0.0, 100.0).unit("K").help("Faces within this margin below the limit get a warning."))
            .param(ParamSpec::bool("use_tg_cap", "Cap at glass transition", true).help("Limit the allowed temperature to the measured glass transition when known."))
            .help("Compares each face's surface temperature with the heat-deflection temperature of its material. Writes the margin and a verdict per face (0 ok, 1 warning, 2 fail) and reports the worst margin for the next stage. Materials whose as-printed and annealed limits differ show both in the report, and a peak above the 70 degC weathering-panel temperature adds a reminder that a UV test does not test heat.")
            .emulates("Heat-deflection comparison of the report")
            .spec(SOLAR_SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        let temps = g
            .prim_attribs
            .floats("temp_c")
            .map_err(|_| {
                ctx.fail("the mesh has no temp_c attribute; run Surface Temperature Balance first")
            })?
            .to_vec();
        let table = match opt_json_in(ins, 1)? {
            Some(v) => materials_from_json(&v).map_err(|e| ctx.fail(e))?,
            None => builtin_materials(),
        };
        // Material nodes of other stages name it hdt_045_c / hdt_18_c.
        let material_report = inputs::opt_report(ins, 2);
        let found = material_report.as_ref().and_then(|r| r.first_metric(&keys::HDT_ALIASES));
        let override_hdt = found.map(|(_, v)| v);
        let annealed = p.bool("annealed")?;
        let use_tg = p.bool("use_tg_cap")?;
        let warn = p.float("margin_warn_k")?;
        let default_name = p.str("material")?.to_string();
        let names = g.prim_attribs.strs("material").ok();
        let np = g.prims.len();
        let areas = prim_areas(&g);
        let mut margin = vec![0.0; np];
        let mut verdict = vec![0i64; np];
        let mut hdt_used = vec![0.0; np];
        let mut worst: Option<(f64, usize)> = None;
        for i in 0..np {
            if areas[i] <= 1e-12 {
                continue;
            }
            let hdt = match override_hdt {
                Some(h) => h,
                None => {
                    let name = names.map_or(default_name.as_str(), |n| n[i].as_str());
                    let name = if name.is_empty() {
                        default_name.as_str()
                    } else {
                        name
                    };
                    let m = resolve(&table, name)
                        .ok_or_else(|| ctx.fail(format!("unknown material '{name}'")))?;
                    m.hdt_eff(annealed, use_tg).ok_or_else(|| {
                        ctx.fail(format!("material '{name}' has no heat-deflection value"))
                    })?
                }
            };
            hdt_used[i] = hdt;
            margin[i] = hdt - temps[i];
            verdict[i] = if temps[i] > hdt {
                2
            } else if temps[i] > hdt - warn {
                1
            } else {
                0
            };
            if !matches!(worst, Some((m, _)) if margin[i] >= m) {
                worst = Some((margin[i], i));
            }
        }
        let atot: f64 = areas.iter().sum::<f64>().max(1e-12);
        let share = |v: i64| {
            100.0
                * (0..np)
                    .filter(|&i| verdict[i] == v && areas[i] > 1e-12)
                    .map(|i| areas[i])
                    .sum::<f64>()
                / atot
        };
        let (min_margin, worst_face) = worst.unwrap_or((0.0, 0));
        let max_temp = temps.iter().cloned().fold(f64::MIN, f64::max);
        let mut r = Report::new("Heat deflection check");
        r.metric(
            "min_margin_k",
            Metric::new(min_margin, "K").with_limit(0.0, false),
        );
        r.metric(
            "fail_area_pct",
            Metric::new(share(2), "%").with_limit(0.0, true),
        );
        r.metric("warn_area_pct", Metric::new(share(1), "%"));
        r.metric("ok_area_pct", Metric::new(share(0), "%"));
        r.metric("worst_face", Metric::new(worst_face as f64, "index"));
        r.metric("max_temp_c", Metric::new(max_temp, "degC"));
        // A connected material report sets the limit, so it also sets the treatment: state what
        // the limit really is rather than this node's own flag, which did not choose it.
        if override_hdt.is_some() {
            let treatment = |on: bool| if on { "annealed" } else { "as-printed" };
            let reported = material_report
                .as_ref()
                .and_then(|m| m.first_metric(&[keys::ANNEALED]))
                .map(|(_, v)| v >= 0.5);
            match reported {
                Some(on) => {
                    r.metric(keys::ANNEALED, Metric::new(f64::from(on), ""));
                    if on != annealed {
                        r.note(
                            Severity::Warning,
                            format!(
                                "this node's Annealed setting is ignored: the limit comes from the connected material report, which uses the {} value; set the treatment on the material node",
                                treatment(on)
                            ),
                        );
                    }
                }
                None if annealed => {
                    r.note(
                        Severity::Warning,
                        "this node's Annealed setting is ignored: the limit comes from the connected material report, which does not state a treatment",
                    );
                }
                None => {}
            }
        } else {
            r.metric(keys::ANNEALED, Metric::new(f64::from(annealed), ""));
        }
        // the treatment behind the limit is a metric, not only a note: some materials carry a
        // datasheet figure, an as-printed figure and an annealed figure that differ by 100 K
        if override_hdt.is_none() {
            if let Some(row) = resolve(&table, &default_name) {
                if let Some(v) = row.hdt_datasheet_c {
                    r.metric("hdt_datasheet_c", Metric::new(v, "degC"));
                }
                if let Some(v) = row.hdt_as_printed_c {
                    r.metric("hdt_as_printed_c", Metric::new(v, "degC"));
                }
                if let Some(v) = row.hdt_annealed_c {
                    r.metric("hdt_annealed_c", Metric::new(v, "degC"));
                }
                if let (Some(asp), Some(ann)) = (row.hdt_as_printed_c, row.hdt_annealed_c) {
                    r.metric("treatment_matters", Metric::new(1.0, ""));
                    let used = if annealed { ann } else { asp };
                    r.note(
                        Severity::Warning,
                        format!(
                            "{}: the limit used is the {} value {used:.1} degC (as printed {asp:.1}, annealed {ann:.1}{}); the verdict depends on the treatment, so state it before trusting a pass",
                            row.name,
                            if annealed { "annealed" } else { "as-printed" },
                            row.hdt_datasheet_c.map_or(String::new(), |d| format!(", datasheet {d:.1}")),
                        ),
                    );
                }
            }
        }
        if max_temp > J2527_PANEL_C {
            r.note(
                Severity::Info,
                format!(
                    "weathering tests of the SAE J2527 type hold a {J2527_PANEL_C:.0} degC black panel, below the {max_temp:.0} degC peak here: a weathering or UV pass says nothing about heat, so add a separate oven test at the design temperature"
                ),
            );
        }
        if let Some((key, h)) = found {
            r.metric(keys::HDT_C, Metric::new(h, "degC"));
            r.note(
                Severity::Info,
                format!("heat-deflection temperature taken from the connected material report ({key})"),
            );
        } else if let Some(&h) = hdt_used.iter().find(|h| **h != 0.0) {
            r.metric(keys::HDT_C, Metric::new(h, "degC"));
        }
        if material_report.is_some() && found.is_none() {
            r.note(
                Severity::Warning,
                format!(
                    "the connected material report has no heat-deflection metric ({}); the material table was used",
                    keys::HDT_ALIASES.join(", ")
                ),
            );
        }
        if min_margin < 0.0 {
            r.note(
                Severity::Error,
                format!(
                    "face {worst_face} exceeds the heat-deflection temperature by {:.1} K",
                    -min_margin
                ),
            );
        } else if min_margin < warn {
            r.note(
                Severity::Warning,
                format!("worst margin is only {min_margin:.1} K"),
            );
        }
        let mut out: Geometry = (*g).clone();
        out.prim_attribs
            .set("hdt_margin_k", AttribArray::Float(margin.clone()));
        out.prim_attribs
            .set("hdt_verdict", AttribArray::Int(verdict.clone()));
        let span = warn.max(1.0) * 2.0;
        out.prim_attribs.set(
            "Cd",
            AttribArray::Vec3(
                margin
                    .iter()
                    .map(|m| ramp_div(-(m / span).clamp(-1.0, 1.0)))
                    .collect(),
            ),
        );
        let pm = prim_to_point(&g, &margin);
        out.point_attribs.set(
            "Cd",
            AttribArray::Vec3(
                pm.iter()
                    .map(|m| ramp_div(-(m / span).clamp(-1.0, 1.0)))
                    .collect(),
            ),
        );
        out.point_attribs
            .set("hdt_margin_k", AttribArray::Float(pm));
        Ok(vec![Data::geometry(out), Data::report(r)])
    }
}

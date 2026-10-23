//! Site aero loads: forces, power and design pressure at a chosen air density and speed.

use serde::Serialize;
use serde_json::{json, Value};
use vernon_core::{Metric, Report, Severity};
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::air::{air_in, dynamic_pressure};
use crate::coeffs::ForceResult;
use crate::util::{fin, json_in, FnNode};

/// Loads at one speed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SitePoint {
    pub v_kmh: f64,
    pub v_ms: f64,
    pub q_pa: f64,
    /// Drag force, N.
    pub drag_n: f64,
    /// Downforce, N (negative = net lift).
    pub downforce_n: f64,
    /// Power absorbed by drag, kW.
    pub power_kw: f64,
}

/// `D = q CdA`, `F_down = -q ClA` (z up, so downforce has ClA < 0), `P = D v`.
pub fn site_loads(cda: f64, cla: f64, rho: f64, v_ms: f64) -> SitePoint {
    let q = dynamic_pressure(rho, v_ms);
    let drag = q * cda;
    SitePoint {
        v_kmh: v_ms * 3.6,
        v_ms,
        q_pa: q,
        drag_n: drag,
        downforce_n: -q * cla,
        power_kw: drag * v_ms / 1000.0,
    }
}

/// Comma-separated speeds in km/h.
pub fn parse_speeds_kmh(s: &str) -> Result<Vec<f64>, String> {
    let v: Vec<f64> = s
        .split([',', ';', ' '])
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.trim().parse::<f64>().map_err(|_| format!("'{t}' is not a speed")))
        .collect::<Result<_, _>>()?;
    if v.is_empty() || v.len() > 16 {
        return Err("give between 1 and 16 speeds".into());
    }
    if v.iter().any(|x| !x.is_finite() || *x <= 0.0 || *x > 600.0) {
        return Err("speeds must be between 0 and 600 km/h".into());
    }
    Ok(v)
}

fn info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.aero_loads", "Site Aero Loads", Stage::Cfd)
        .input(PortSpec::optional("result", DataKind::Json))
        .input(PortSpec::optional("air", DataKind::Json))
        .output(PortSpec::new("loads", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::float("cda_m2", "CdA (manual)", 0.64, 0.05, 3.0).unit("m2").help("Used when no force result is connected"))
        .param(ParamSpec::float("cla_m2", "ClA (manual)", -0.5, -3.0, 3.0).unit("m2").help("Negative = downforce. Used when no force result is connected"))
        .param(ParamSpec::float("rho", "Site air density", 1.11, 0.5, 1.4).unit("kg/m3").help("Overridden by a wired air state"))
        .param(ParamSpec::float("rho_run", "Density of the run", 1.225, 0.9, 1.3).unit("kg/m3").help("Only for the force scaling factor; a connected result carries its own"))
        .param(ParamSpec::string("speeds_kmh", "Speed table", "100, 150, 200, 250").unit("km/h").help("Comma-separated"))
        .param(ParamSpec::float("design_kmh", "Design speed", 250.0, 50.0, 400.0).unit("km/h"))
        .param(ParamSpec::float("load_factor", "Design load factor", 1.0, 1.0, 5.0).help("Safety factor on the structural design load"))
        .param(ParamSpec::float("panel_area_m2", "Loaded panel area", 0.0, 0.0, 10.0).unit("m2").help("Optional: turns the design downforce into a mean pressure; 0 = skip"))
        .help("Scales the measured CdA and ClA to the site: dynamic pressure, drag, downforce and drag power over a speed table, plus the design-speed load the structural stage applies. Coefficients are density independent; forces scale with density and speed squared.")
        .emulates("automotive wind-tunnel service: site-condition force scaling")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

fn cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let air = air_in(ins, 1)?;
    let rho = air.rho.unwrap_or(p.float("rho")?);
    let mut rep = Report::new("Site aero loads");
    let (cda, cla, rho_run, from_result) = match json_in(ins, 0)? {
        Some(v) => {
            let r = ForceResult::from_json(&v)?;
            if r.placeholder {
                rep.metric("data_available", Metric::new(0.0, "-").with_limit(1.0, false));
                rep.note(Severity::Warning, "the force result has no solver data yet; no loads are computed");
                return Ok(vec![Data::json(json!({"placeholder": true, "rows": []})), Data::report(rep)]);
            }
            let rho_run = if r.rho > 0.0 { r.rho } else { p.float("rho_run")? };
            (r.cda, r.cla, rho_run, true)
        }
        None => (p.float("cda_m2")?, p.float("cla_m2")?, p.float("rho_run")?, false),
    };
    let speeds = parse_speeds_kmh(p.str("speeds_kmh")?).map_err(|e| ctx.fail(e))?;
    let rows: Vec<SitePoint> = speeds.iter().map(|v| site_loads(cda, cla, rho, v / 3.6)).collect();
    let v_design = p.float("design_kmh")?;
    let factor = p.float("load_factor")?;
    let design = site_loads(cda, cla, rho, v_design / 3.6);
    let design_down = design.downforce_n * factor;
    let area = p.float("panel_area_m2")?;
    let density_factor = rho / rho_run;
    rep.metric("data_available", Metric::new(1.0, "-").with_limit(1.0, false))
        .metric("CdA", Metric::new(cda, "m2"))
        .metric("ClA", Metric::new(cla, "m2"))
        .metric("rho_site", Metric::new(fin(ctx, "rho", rho)?, "kg/m3"))
        .metric("density_factor", Metric::new(density_factor, "-"))
        .metric("design_speed", Metric::new(v_design, "km/h"))
        .metric("design_q", Metric::new(design.q_pa * factor, "Pa"))
        .metric("drag_design", Metric::new(design.drag_n, "N"))
        .metric("downforce_design", Metric::new(design_down, "N"))
        .metric("power_design", Metric::new(design.power_kw, "kW"));
    let mut design_pressure = Value::Null;
    if area > 0.0 {
        let pa = design_down / area;
        rep.metric("design_pressure", Metric::new(pa, "Pa"));
        design_pressure = json!(pa);
    }
    if cla > 0.0 {
        rep.note(Severity::Warning, "net lift (ClA > 0): the downforce columns are negative");
    }
    if !from_result {
        rep.note(Severity::Info, "coefficients entered by hand, not taken from a solved case");
    }
    rep.note(Severity::Info, "forces scale with density and speed squared; coefficients at matched Reynolds number do not");
    let out = json!({
        "placeholder": false, "cda": cda, "cla": cla, "rho": rho, "rho_run": rho_run,
        "density_factor": density_factor, "rows": rows,
        "design": {
            "v_kmh": v_design, "q_pa": design.q_pa, "load_factor": factor,
            "design_q_pa": design.q_pa * factor, "drag_n": design.drag_n,
            "downforce_n": design.downforce_n, "design_downforce_n": design_down,
            "power_kw": design.power_kw, "design_pressure_pa": design_pressure,
        },
    });
    if rows.iter().any(|r| !r.q_pa.is_finite()) {
        return Err(CookError::Other("non-finite dynamic pressure".into()));
    }
    Ok(vec![Data::json(out), Data::report(rep)])
}

pub(crate) fn register(reg: &mut Registry) {
    reg.add(FnNode::new(info, cook));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::ctx;
    use vernon_graph::ParamValue;

    fn pct(a: f64, b: f64) -> f64 {
        (a / b - 1.0).abs() * 100.0
    }

    #[test]
    fn golden_g3_two_hundred_kmh() {
        let s = site_loads(0.64, -0.5, 1.11, 200.0 / 3.6);
        assert!(pct(s.q_pa, 1713.0) < 0.2, "{}", s.q_pa);
        assert!(pct(s.drag_n, 1096.0) < 0.2, "{}", s.drag_n);
        assert!(pct(s.power_kw, 60.9) < 0.2, "{}", s.power_kw);
        assert!(pct(s.downforce_n, 856.0) < 0.2, "{}", s.downforce_n);
        assert!((s.v_kmh - 200.0).abs() < 1e-9);
    }

    #[test]
    fn golden_g4_two_fifty_and_square_law() {
        let s = site_loads(0.64, -0.5, 1.11, 250.0 / 3.6);
        assert!(pct(s.q_pa, 2677.0) < 0.5, "{}", s.q_pa);
        assert!(pct(s.downforce_n, 1338.0) < 0.5);
        let a = site_loads(0.64, -0.5, 1.11, 200.0 / 3.6);
        assert!((s.downforce_n / a.downforce_n - (250.0f64 / 200.0).powi(2)).abs() < 1e-12);
        // drag power goes with the cube of speed
        assert!((s.power_kw / a.power_kw - (250.0f64 / 200.0).powi(3)).abs() < 1e-12);
    }

    #[test]
    fn golden_g5_density_factor() {
        let mut p = Params::from_specs(&info().params);
        p.set("rho", ParamValue::Float(1.11));
        let out = cook(&ctx("loads-g5"), &[], &p).unwrap();
        let f = out[0].as_json().unwrap()["density_factor"].as_f64().unwrap();
        assert!((f - 0.906).abs() < 0.001, "{f}");
    }

    #[test]
    fn lift_has_negative_downforce() {
        assert!(site_loads(0.6, 0.1, 1.2, 50.0).downforce_n < 0.0);
    }

    #[test]
    fn node_design_load_and_pressure() {
        let mut p = Params::from_specs(&info().params);
        p.set("rho", ParamValue::Float(1.11));
        p.set("load_factor", ParamValue::Float(1.5));
        p.set("panel_area_m2", ParamValue::Float(0.5));
        let out = cook(&ctx("loads-node"), &[], &p).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(r.passes());
        let down = r.metrics["downforce_design"].value;
        assert!((down - 1.5 * 1338.0).abs() / down < 0.005, "{down}");
        assert!((r.metrics["design_pressure"].value - down / 0.5).abs() < 1e-9);
        assert_eq!(out[0].as_json().unwrap()["rows"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn wired_result_and_air_state_take_priority() {
        let res = ForceResult { cda: 0.7, cla: -0.2, rho: 1.225, ..ForceResult::default() };
        let air = json!({"rho": 1.098, "u_ms": 30.0});
        let out = cook(&ctx("loads-wired"), &[Some(Data::json(res.to_json())), Some(Data::json(air))], &Params::from_specs(&info().params)).unwrap();
        let j = out[0].as_json().unwrap();
        assert_eq!(j["cda"], 0.7);
        assert!((j["density_factor"].as_f64().unwrap() - 1.098 / 1.225).abs() < 1e-12);
        // placeholder results compute nothing
        let ph = ForceResult { placeholder: true, ..ForceResult::default() };
        let out = cook(&ctx("loads-ph"), &[Some(Data::json(ph.to_json()))], &Params::from_specs(&info().params)).unwrap();
        assert!(!out[1].as_report().unwrap().passes());
    }

    #[test]
    fn speed_list_parsing() {
        assert_eq!(parse_speeds_kmh("100, 200;250").unwrap(), vec![100.0, 200.0, 250.0]);
        assert!(parse_speeds_kmh("").is_err());
        assert!(parse_speeds_kmh("100, abc").is_err());
        assert!(parse_speeds_kmh("-5").is_err());
        let mut p = Params::from_specs(&info().params);
        p.set("speeds_kmh", ParamValue::Str("fast".into()));
        assert!(cook(&ctx("loads-bad"), &[], &p).is_err());
    }
}

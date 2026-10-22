//! Air state: moist-air density, Sutherland viscosity, dynamic pressure.

use serde_json::json;
use vernon_core::{Metric, Report, Severity};
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::util::{fin, jf, json_in, FnNode};

pub const R_DRY: f64 = 287.05;
pub const R_VAPOUR: f64 = 461.5;
pub const RHO_ISA: f64 = 1.225;

/// Thermodynamic state of the tunnel air.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Air {
    pub t_c: f64,
    pub p_pa: f64,
    pub rh_pct: f64,
    pub rho: f64,
    pub mu: f64,
    pub nu: f64,
}

/// Saturation vapour pressure (Magnus form), Pa.
pub fn p_sat_pa(t_c: f64) -> f64 {
    610.94 * (17.625 * t_c / (t_c + 243.04)).exp()
}

/// Sutherland dynamic viscosity, Pa s.
pub fn sutherland_mu(t_k: f64) -> f64 {
    1.458e-6 * t_k.powf(1.5) / (t_k + 110.4)
}

/// Moist-air state from temperature (deg C), total pressure (Pa) and relative humidity (%).
pub fn moist_air(t_c: f64, p_pa: f64, rh_pct: f64) -> Air {
    let t_k = t_c + 273.15;
    let p_v = (rh_pct / 100.0).clamp(0.0, 1.0) * p_sat_pa(t_c);
    let p_d = (p_pa - p_v).max(0.0);
    let rho = p_d / (R_DRY * t_k) + p_v / (R_VAPOUR * t_k);
    let mu = sutherland_mu(t_k);
    Air {
        t_c,
        p_pa,
        rh_pct,
        rho,
        mu,
        nu: mu / rho,
    }
}

/// Dynamic pressure, Pa.
pub fn dynamic_pressure(rho: f64, u: f64) -> f64 {
    0.5 * rho * u * u
}

/// Air state read from a hand-off Json (`rho`, `nu`, `u_ms`).
pub(crate) struct AirIn {
    pub rho: Option<f64>,
    pub nu: Option<f64>,
    pub mu: Option<f64>,
    pub u: Option<f64>,
}

pub(crate) fn air_in(inputs: &[Option<Data>], idx: usize) -> CookResult<AirIn> {
    let v = json_in(inputs, idx)?;
    Ok(AirIn {
        rho: v.as_ref().and_then(|v| jf(v, "rho")),
        nu: v.as_ref().and_then(|v| jf(v, "nu")),
        mu: v.as_ref().and_then(|v| jf(v, "mu")),
        u: v.as_ref().and_then(|v| jf(v, "u_ms")),
    })
}

fn info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.air_state", "Air State", Stage::Cfd)
        .output(PortSpec::new("air", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::float("t_c", "Temperature", 15.0, -30.0, 60.0).unit("°C"))
        .param(ParamSpec::float("p_pa", "Pressure", 101_325.0, 60_000.0, 110_000.0).unit("Pa"))
        .param(ParamSpec::float("rh_pct", "Relative humidity", 0.0, 0.0, 100.0).unit("%"))
        .param(ParamSpec::float("u_ms", "Speed", 30.0, 10.0, 100.0).unit("m/s"))
        .help("Moist-air density, viscosity and dynamic pressure from temperature, pressure and humidity. Feeds the case writer and the site-load scaling.")
        .emulates("automotive wind-tunnel service: site air density input")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

fn cook(ctx: &CookContext, _inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let u = p.float("u_ms")?;
    let air = moist_air(p.float("t_c")?, p.float("p_pa")?, p.float("rh_pct")?);
    let q = dynamic_pressure(air.rho, u);
    let mach = u / (1.4 * R_DRY * (air.t_c + 273.15)).sqrt();
    let mut r = Report::new("Air state");
    r.metric("rho", Metric::new(fin(ctx, "rho", air.rho)?, "kg/m3"))
        .metric("rho_vs_isa_pct", Metric::new((air.rho / RHO_ISA - 1.0) * 100.0, "%"))
        .metric("mu", Metric::new(air.mu, "Pa s"))
        .metric("nu", Metric::new(air.nu, "m2/s"))
        .metric("q", Metric::new(q, "Pa"))
        .metric("mach", Metric::new(mach, "-").with_limit(0.3, true));
    if mach > 0.3 {
        r.note(Severity::Warning, "Mach number above 0.3: the incompressible solver is no longer a good model");
    }
    let j = json!({"rho": air.rho, "mu": air.mu, "nu": air.nu, "q": q, "u_ms": u, "t_c": air.t_c, "p_pa": air.p_pa, "rh_pct": air.rh_pct});
    Ok(vec![Data::json(j), Data::report(r)])
}

pub(crate) fn register(reg: &mut Registry) {
    reg.add(FnNode::new(info, cook));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_dry_40c() {
        let a = moist_air(40.0, 101_325.0, 0.0);
        assert!((a.rho - 1.127).abs() < 0.001, "{}", a.rho);
        assert!(((a.rho / RHO_ISA - 1.0) * 100.0 + 8.0).abs() < 0.2);
    }

    #[test]
    fn golden_dubai_45c_30rh() {
        let a = moist_air(45.0, 101_325.0, 30.0);
        assert!((a.rho - 1.098).abs() < 0.002, "{}", a.rho);
        assert!((a.mu / 1.93e-5 - 1.0).abs() < 0.01, "{}", a.mu);
        assert!((a.nu / 1.76e-5 - 1.0).abs() < 0.01, "{}", a.nu);
    }

    #[test]
    fn isa_density_and_k2_check() {
        assert!((moist_air(15.0, 101_325.0, 0.0).rho - 1.225).abs() < 0.001);
        // 30 C dry: 1.164 kg/m3, mu 1.861e-5
        let a = moist_air(30.0, 101_325.0, 0.0);
        assert!((a.rho - 1.164).abs() < 0.001);
        assert!((a.mu / 1.861e-5 - 1.0).abs() < 0.005);
    }

    #[test]
    fn humid_air_is_lighter() {
        assert!(moist_air(30.0, 101_325.0, 90.0).rho < moist_air(30.0, 101_325.0, 0.0).rho);
    }

    #[test]
    fn node_reports_dynamic_pressure() {
        let ctx = crate::util::testutil::ctx("air");
        let params = Params::from_specs(&info().params);
        let out = cook(&ctx, &[], &params).unwrap();
        let r = out[1].as_report().unwrap();
        assert!((r.metrics["q"].value - 0.5 * 1.225 * 900.0).abs() < 1.0);
        assert!(r.passes());
    }
}

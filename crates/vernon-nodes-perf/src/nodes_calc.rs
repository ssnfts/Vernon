//! Calculator nodes: air density, aero loads, duct heat, top speed, vehicle definition,
//! grip envelope and corner-speed table. Spec section 4.

use vernon_core::{Metric, Report, Severity};
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Stage};

use crate::physics::{self, Env, Vehicle, G};
use crate::util::{env_in, finite, parse_list, rho_param, table_json, vehicle_in, FnNode, OrFail, SPEC};

fn node(id: &str, label: &str) -> NodeTypeInfo {
    NodeTypeInfo::new(id, label, Stage::Performance).spec(SPEC).implemented()
}

pub fn register(reg: &mut vernon_graph::Registry) {
    reg.add(FnNode::new(air_density_info, air_density_cook));
    reg.add(FnNode::new(aero_loads_info, aero_loads_cook));
    reg.add(FnNode::new(duct_heat_info, duct_heat_cook));
    reg.add(FnNode::new(top_speed_info, top_speed_cook));
    reg.add(FnNode::new(vehicle_info, vehicle_cook));
    reg.add(FnNode::new(gg_info, gg_cook));
    reg.add(FnNode::new(corner_info, corner_cook));
}

/// Parse an engine torque table written `rpm:Nm, rpm:Nm, ...`.
pub fn parse_curve(s: &str) -> Result<Vec<[f64; 2]>, String> {
    let mut out = Vec::new();
    for tok in s.split([',', ';']).map(str::trim).filter(|t| !t.is_empty()) {
        let (a, b) = tok.split_once(':').ok_or_else(|| format!("'{tok}' is not rpm:torque"))?;
        let rpm = a.trim().parse::<f64>().map_err(|_| format!("'{a}' is not a number"))?;
        let nm = b.trim().parse::<f64>().map_err(|_| format!("'{b}' is not a number"))?;
        out.push([rpm, nm]);
    }
    out.sort_by(|a, b| a[0].total_cmp(&b[0]));
    Ok(out)
}

// ------------------------------------------------------------------ air density

fn air_density_info() -> NodeTypeInfo {
    node("perf.air_density", "Air Density")
        .output(PortSpec::new("env", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::float("temp_c", "Temperature", 20.0, -40.0, 60.0).unit("C").help("Air temperature"))
        .param(ParamSpec::float("pressure_pa", "Pressure", 101325.0, 5.0e4, 1.1e5).unit("Pa").help("Static air pressure"))
        .param(ParamSpec::float("rel_humidity", "Relative humidity", 0.0, 0.0, 1.0).help("0 = dry, 1 = saturated"))
        .param(ParamSpec::float("dry_gas_const", "Dry-air gas constant", 287.058, 280.0, 295.0).unit("J/(kg K)").help("Specific gas constant of dry air"))
        .help("Moist-air density from temperature, pressure and humidity (saturation pressure by the Tetens form, ideal-gas mixture). Feeds every other stage-8 node as the air state.")
        .emulates("standard air-density relation used by the aero/performance calculators")
}

fn air_density_cook(ctx: &CookContext, _inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let (t, pr, rh, r) = (p.float("temp_c")?, p.float("pressure_pa")?, p.float("rel_humidity")?, p.float("dry_gas_const")?);
    let env = Env::from_state(t, pr, rh, r);
    finite(ctx, "density", env.rho)?;
    let mut rep = Report::new("Air state");
    rep.metric("rho", Metric::new(env.rho, "kg/m3"))
        .metric("rho_vs_isa_pct", Metric::new(100.0 * (env.rho / 1.225 - 1.0), "%"))
        .metric("sat_pressure", Metric::new(physics::sat_pressure_pa(t), "Pa"))
        .metric("vapour_pressure", Metric::new(rh * physics::sat_pressure_pa(t), "Pa"));
    if rh * physics::sat_pressure_pa(t) > pr {
        rep.note(Severity::Warning, "vapour pressure exceeds the total pressure; humidity was limited");
    }
    Ok(vec![Data::json(env.to_json()), Data::report(rep)])
}

// ------------------------------------------------------------------ aero loads

fn aero_loads_info() -> NodeTypeInfo {
    node("perf.aero_loads", "Aero Loads")
        .input(PortSpec::optional("env", DataKind::Json))
        .input(PortSpec::optional("vehicle", DataKind::Json))
        .output(PortSpec::new("table", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::bool("use_vehicle", "Use connected vehicle", true).help("Take CdA, ClA and mass from the connected vehicle; off uses the values below"))
        .param(ParamSpec::float("cda_m2", "CdA", 0.64, 0.05, 3.0).unit("m2").help("Drag area"))
        .param(ParamSpec::float("cla_m2", "ClA (downforce +)", 0.5, -1.0, 6.0).unit("m2").help("Downforce area, positive = downforce"))
        .param(ParamSpec::float("mass_kg", "Mass", 1400.0, 50.0, 6000.0).unit("kg").help("Vehicle mass for the braking column"))
        .param(ParamSpec::string("speeds_kmh", "Speeds", "50, 100, 150, 200, 250, 300").unit("km/h").help("Table speeds, comma separated (50-300)"))
        .param(ParamSpec::float("design_kmh", "Design speed", 200.0, 20.0, 400.0).unit("km/h").help("Speed whose loads are reported as metrics for the structural stage"))
        .param(ParamSpec::float("scale_from_rho", "Reference density", 0.0, 0.0, 1.5).unit("kg/m3").help("0 = off. Otherwise adds the loads at this density (for example a standard-atmosphere run) beside the site loads and reports the density ratio"))
        .param(ParamSpec::float("cla_front_m2", "Front axle ClA", 0.0, -1.0, 6.0).unit("m2").help("Optional per-axle downforce area; with the rear value adds an aero-balance column"))
        .param(ParamSpec::float("cla_rear_m2", "Rear axle ClA", 0.0, -1.0, 6.0).unit("m2").help("Optional per-axle downforce area"))
        .param(ParamSpec::float("brake_mu", "Braking friction", 1.0, 0.2, 3.0).help("Tyre friction coefficient for the braking-deceleration column"))
        .param(rho_param())
        .help("Dynamic pressure, drag, downforce, aero power, aero balance and braking deceleration over a speed table, plus the design-speed loads the structural stage needs.")
        .emulates("aero load calculator (q, drag, downforce, power, braking, balance)")
}

fn aero_loads_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let env = env_in(ctx, inputs, 0, p)?;
    let rho = env.rho;
    let veh = vehicle_in(ctx, inputs, 1)?;
    let (cda, cla, mass) = match (&veh, p.bool("use_vehicle")?) {
        (Some(v), true) => (v.cda_eff(), v.cla_m2, v.mass_kg),
        _ => (p.float("cda_m2")?, p.float("cla_m2")?, p.float("mass_kg")?),
    };
    if !(mass > 0.0 && cda >= 0.0) {
        return Err(ctx.fail("mass must be positive and CdA non-negative"));
    }
    let speeds = parse_list(p.str("speeds_kmh")?).or_fail(ctx)?;
    if speeds.is_empty() || speeds.iter().any(|v| !(*v > 0.0 && v.is_finite())) {
        return Err(ctx.fail("speeds must be a list of positive numbers"));
    }
    let (lf, lr) = (p.float("cla_front_m2")?, p.float("cla_rear_m2")?);
    let balance = lf != 0.0 || lr != 0.0;
    let ref_rho = p.float("scale_from_rho")?;
    let mu = p.float("brake_mu")?;
    let mut cols = vec![
        ("v_kmh", "km/h"),
        ("v_ms", "m/s"),
        ("q_pa", "Pa"),
        ("drag_n", "N"),
        ("downforce_n", "N"),
        ("downforce_kgf", "kgf"),
        ("p_aero_w", "W"),
    ];
    if balance {
        cols.push(("front_balance_pct", "%"));
    }
    cols.push(("brake_decel_ms2", "m/s2"));
    cols.push(("brake_decel_g", "g"));
    if ref_rho > 0.0 {
        cols.push(("drag_ref_n", "N"));
        cols.push(("downforce_ref_n", "N"));
    }
    let row = |kmh: f64| -> Vec<f64> {
        let v = kmh / 3.6;
        let q = physics::dyn_pressure(rho, v);
        let mut r = vec![kmh, v, q, q * cda, q * cla, q * cla / G, physics::aero_power(rho, cda, v)];
        if balance {
            r.push(physics::aero_balance_pct(lf, lr));
        }
        let a = physics::braking_decel(mu, rho, cda, cla, mass, v);
        r.push(a);
        r.push(a / G);
        if ref_rho > 0.0 {
            let qr = physics::dyn_pressure(ref_rho, v);
            r.push(qr * cda);
            r.push(qr * cla);
        }
        r
    };
    let rows: Vec<Vec<f64>> = speeds.iter().map(|&s| row(s)).collect();
    let design = row(p.float("design_kmh")?);
    let mut rep = Report::new("Aero loads");
    rep.metric("rho", Metric::new(rho, "kg/m3"))
        .metric("CdA", Metric::new(cda, "m2"))
        .metric("ClA", Metric::new(cla, "m2"))
        .metric("design_speed", Metric::new(design[0], "km/h"))
        .metric("design_q", Metric::new(design[2], "Pa"))
        .metric("drag_design", Metric::new(design[3], "N"))
        .metric("downforce_design", Metric::new(design[4], "N"))
        .metric("power_design", Metric::new(design[6] / 1000.0, "kW"));
    let k = if balance { 8 } else { 7 };
    rep.metric("brake_decel_design", Metric::new(design[k], "m/s2"));
    if balance {
        rep.metric("front_balance", Metric::new(design[7], "%"));
        if !(35.0..=65.0).contains(&design[7]) {
            rep.note(Severity::Warning, "front aero balance outside 35-65 percent (advisory: the point-mass lap model ignores balance)");
        }
    }
    if ref_rho > 0.0 {
        rep.metric("density_factor", Metric::new(rho / ref_rho, "-"));
    }
    if cla < 0.0 {
        rep.note(Severity::Warning, "net lift (ClA < 0): the downforce columns are negative");
    }
    // The design row the structural stage's aero load reads, so one
    // density and one dynamic pressure flow from the air state onwards.
    let mut table = table_json(&cols, rows);
    if let Some(obj) = table.as_object_mut() {
        obj.insert(
            "design".into(),
            serde_json::json!({
                "v_kmh": design[0], "rho": rho, "q_pa": design[2], "design_q_pa": design[2],
                "drag_n": design[3], "downforce_n": design[4], "design_downforce_n": design[4],
            }),
        );
    }
    Ok(vec![Data::json(table), Data::report(rep)])
}

// ------------------------------------------------------------------ duct heat

fn duct_heat_info() -> NodeTypeInfo {
    node("perf.duct_heat", "Duct Heat")
        .input(PortSpec::optional("env", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::float("duct_speed_ms", "Mean duct speed", 20.0, 0.0, 100.0).unit("m/s").help("Mean air speed through the duct"))
        .param(ParamSpec::float("duct_area_m2", "Duct area", 0.01, 1.0e-4, 1.0).unit("m2").help("Cross-section area"))
        .param(ParamSpec::float("delta_t_k", "Air temperature rise", 30.0, 0.0, 200.0).unit("K").help("Used when solving for heat"))
        .param(ParamSpec::float("heat_w", "Heat load", 6000.0, 0.0, 1.0e6).unit("W").help("Used when solving for temperature rise"))
        .param(ParamSpec::float("cp_j_kgk", "Specific heat", 1005.0, 900.0, 1100.0).unit("J/(kg K)").help("Specific heat of air"))
        .param(ParamSpec::choice("solve_for", "Solve for", "heat", &["heat", "delta_t"]).help("Heat load from the temperature rise, or the rise from a heat load"))
        .param(rho_param())
        .help("Duct mass flow (density x speed x area) and the heat it carries away, or the temperature rise for a given heat load.")
        .emulates("duct flow and heat calculator (mdot = rho V A, Q = mdot cp dT)")
}

fn duct_heat_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let rho = env_in(ctx, inputs, 0, p)?.rho;
    let (v, a, cp) = (p.float("duct_speed_ms")?, p.float("duct_area_m2")?, p.float("cp_j_kgk")?);
    let mdot = physics::duct_mass_flow(rho, v, a);
    let mut rep = Report::new("Duct heat");
    rep.metric("mdot", Metric::new(mdot, "kg/s"))
        .metric("volume_flow", Metric::new(v * a, "m3/s"));
    match p.str("solve_for")? {
        "heat" => {
            let dt = p.float("delta_t_k")?;
            rep.metric("delta_t", Metric::new(dt, "K")).metric("heat", Metric::new(mdot * cp * dt, "W"));
        }
        "delta_t" => {
            if mdot <= 0.0 {
                return Err(ctx.fail("no air flow (speed is zero): the temperature rise is unbounded"));
            }
            let q = p.float("heat_w")?;
            rep.metric("heat", Metric::new(q, "W")).metric("delta_t", Metric::new(q / (mdot * cp), "K"));
        }
        other => return Err(CookError::BadParam(format!("solve_for = {other}"))),
    }
    Ok(vec![Data::report(rep)])
}

// ------------------------------------------------------------------ top speed

fn top_speed_info() -> NodeTypeInfo {
    node("perf.top_speed", "Top Speed")
        .input(PortSpec::optional("env", DataKind::Json))
        .input(PortSpec::optional("vehicle", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::bool("use_vehicle", "Use connected vehicle", true).help("Take every value from the connected vehicle; off uses the values below"))
        .param(ParamSpec::float("power_w", "Wheel power", 150_000.0, 1.0e3, 2.0e6).unit("W").help("Power at the wheels"))
        .param(ParamSpec::float("mass_kg", "Mass", 1400.0, 50.0, 6000.0).unit("kg").help("Vehicle mass"))
        .param(ParamSpec::float("crr", "Rolling resistance", 0.012, 0.002, 0.05).help("Rolling-resistance coefficient"))
        .param(ParamSpec::bool("rr_includes_downforce", "Rolling on downforce too", false).help("Rolling resistance on weight only (off) or on weight plus downforce (on)"))
        .param(ParamSpec::float("cda_m2", "CdA", 0.64, 0.05, 3.0).unit("m2").help("Drag area"))
        .param(ParamSpec::float("cla_m2", "ClA (downforce +)", 0.0, -1.0, 6.0).unit("m2").help("Downforce area"))
        .param(ParamSpec::float("v_cap_ms", "Speed cap", 150.0, 10.0, 150.0).unit("m/s").help("Upper bound of the search"))
        .param(rho_param())
        .help("Steady top speed where wheel power equals aero plus rolling power, with the power split and the speed lost per square metre of extra drag area.")
        .emulates("top-speed solve from the aero/performance relations")
}

fn top_speed_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let rho = env_in(ctx, inputs, 0, p)?.rho;
    let veh = match (vehicle_in(ctx, inputs, 1)?, p.bool("use_vehicle")?) {
        (Some(v), true) => v,
        _ => Vehicle {
            power_w: p.float("power_w")?,
            mass_kg: p.float("mass_kg")?,
            crr: p.float("crr")?,
            rr_includes_downforce: p.bool("rr_includes_downforce")?,
            cda_m2: p.float("cda_m2")?,
            cla_m2: p.float("cla_m2")?,
            v_cap_ms: p.float("v_cap_ms")?,
            ..Vehicle::default()
        },
    };
    veh.validate().or_fail(ctx)?;
    let ts = veh.top_speed(rho);
    let h = 0.01;
    let mut up = veh.clone();
    up.cda_m2 += h;
    let mut dn = veh.clone();
    dn.cda_m2 = (dn.cda_m2 - h).max(0.0);
    let span = up.cda_m2 - dn.cda_m2;
    let dv = (up.top_speed(rho).v_ms - dn.top_speed(rho).v_ms) * 3.6 / span;
    let p_total = ts.p_aero_w + ts.p_roll_w;
    let mut rep = Report::new("Top speed");
    rep.metric("v_max", Metric::new(ts.v_ms * 3.6, "km/h"))
        .metric("v_max_ms", Metric::new(ts.v_ms, "m/s"))
        .metric("p_aero", Metric::new(ts.p_aero_w, "W"))
        .metric("p_roll", Metric::new(ts.p_roll_w, "W"))
        .metric("aero_share", Metric::new(if p_total > 0.0 { 100.0 * ts.p_aero_w / p_total } else { 0.0 }, "%"))
        .metric("dvmax_dcda_kmh_per_m2", Metric::new(dv, "km/h per m2"))
        .metric("rho", Metric::new(rho, "kg/m3"));
    if ts.limited_by_cap {
        rep.note(Severity::Warning, "the speed cap was reached before power ran out; raise the cap");
    }
    Ok(vec![Data::report(rep)])
}

// ------------------------------------------------------------------ vehicle

fn vehicle_info() -> NodeTypeInfo {
    let d = Vehicle::default();
    node("perf.vehicle", "Vehicle Definition")
        .input(PortSpec::optional("aero", DataKind::Any))
        .output(PortSpec::new("vehicle", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::float("mass_kg", "Mass", d.mass_kg, 50.0, 6000.0).unit("kg").help("Total mass including driver and fuel"))
        .param(ParamSpec::float("rot_inertia_frac", "Rotating inertia", d.rot_inertia_frac, 0.0, 0.15).help("Effective mass = m (1 + e) when accelerating and braking"))
        .param(ParamSpec::float("power_w", "Wheel power", d.power_w, 1.0e3, 2.0e6).unit("W").help("Power at the wheels, used when there is no engine curve"))
        .param(ParamSpec::float("power_density_exponent", "Power-density exponent", d.power_density_exponent, 0.0, 1.5).help("Power scales as (rho / reference rho)^k; 1 for a naturally aspirated engine"))
        .param(ParamSpec::float("rho_ref", "Power reference density", d.rho_ref, 0.5, 1.5).unit("kg/m3").help("Density at which the power was measured"))
        .param(ParamSpec::string("engine_curve", "Engine curve", "").help("Optional torque table written rpm:Nm, rpm:Nm; with gear ratios it replaces the wheel-power model"))
        .param(ParamSpec::string("gear_ratios", "Gear ratios", "").help("Optional comma-separated gear ratios (first gear first)"))
        .param(ParamSpec::float("final_drive", "Final drive", d.final_drive, 0.5, 15.0).help("Final-drive ratio"))
        .param(ParamSpec::float("wheel_radius_m", "Wheel radius", d.wheel_radius_m, 0.1, 0.6).unit("m").help("Loaded wheel radius"))
        .param(ParamSpec::float("driveline_eff", "Driveline efficiency", d.driveline_eff, 0.3, 1.0).help("Engine-to-wheel efficiency"))
        .param(ParamSpec::float("f_drive_max_n", "Drive-force cap", d.f_drive_max_n, 0.0, 1.0e5).unit("N").help("Largest force the driveline can put at the wheels, in newtons. 0 turns the cap off."))
        .param(ParamSpec::float("cda_m2", "CdA", d.cda_m2, 0.05, 3.0).unit("m2").help("Drag area; overridden by a connected aero result"))
        .param(ParamSpec::float("cla_m2", "ClA (downforce +)", d.cla_m2, -1.0, 6.0).unit("m2").help("Downforce area, positive = downforce; overridden by a connected aero result"))
        .param(ParamSpec::float("cda_offset_m2", "CdA calibration offset", d.cda_offset_m2, -0.5, 0.5).unit("m2").help("Additive correction from a road test; the simulated aero numbers are never rewritten"))
        .param(ParamSpec::float("mu_x", "Longitudinal friction", d.mu_x, 0.2, 3.0).help("Peak longitudinal tyre friction"))
        .param(ParamSpec::float("mu_y", "Lateral friction", d.mu_y, 0.2, 3.0).help("Peak lateral tyre friction"))
        .param(ParamSpec::float("mu_drop_per_kn", "Friction load sensitivity", d.mu_drop_per_kn, 0.0, 0.2).unit("1/kN").help("Linear drop of friction with normal load above the static weight"))
        .param(ParamSpec::float("crr", "Rolling resistance", d.crr, 0.002, 0.05).help("Rolling-resistance coefficient"))
        .param(ParamSpec::bool("rr_includes_downforce", "Rolling on downforce too", d.rr_includes_downforce).help("Rolling force on the normal load (on) or on the weight only (off)"))
        .param(ParamSpec::float("brake_max_g", "Brake cap", d.brake_max_g, 0.0, 5.0).unit("g").help("0 = limited by the tyres only"))
        .param(ParamSpec::float("v_cap_ms", "Speed cap", d.v_cap_ms, 10.0, 150.0).unit("m/s").help("Top-speed cap for straights"))
        .param(ParamSpec::float("aero_balance_front", "Front aero balance", d.aero_balance_front, 0.0, 1.0).help("Stored for reports only: the point-mass model ignores balance"))
        .param(ParamSpec::bool("lift_positive", "Aero input is lift-positive", true).help("The airflow stage reports ClA negative for downforce; on flips the sign of a connected aero result"))
        .help("Collects the car's mass, power, tyres, rolling resistance and aero numbers into one vehicle description. A connected aero result (CdA, ClA) overrides the manual aero values.")
        .emulates("vehicle definition (mass, power, tyres, aero) of a point-mass lap simulator")
}

/// CdA and raw ClA from an aero result: a JSON object or a report with `CdA` / `ClA` metrics.
/// A placeholder result (no solver data yet: `placeholder: true` in JSON, or
/// a report whose `data_available` is 0) is an error, never zero drag.
fn aero_from(d: &Data) -> Result<(f64, f64), String> {
    const NO_DATA: &str = "the aero input is a placeholder with no solver data; run the airflow case first";
    const NO_VALUES: &str = "the aero input holds no CdA / ClA values";
    let (cda, cla) = match d {
        Data::Json(v) => {
            if v.get("placeholder").and_then(|x| x.as_bool()) == Some(true) {
                return Err(NO_DATA.into());
            }
            let g = |names: &[&str]| names.iter().find_map(|k| v.get(*k).and_then(|x| x.as_f64()));
            (g(&vernon_core::keys::CDA_ALIASES), g(&vernon_core::keys::CLA_ALIASES))
        }
        Data::Report(r) => {
            if r.metrics.get("data_available").is_some_and(|m| m.value < 0.5) {
                return Err(NO_DATA.into());
            }
            let g = |names: &[&'static str]| r.first_metric(names).map(|(_, v)| v);
            (g(&vernon_core::keys::CDA_ALIASES), g(&vernon_core::keys::CLA_ALIASES))
        }
        _ => (None, None),
    };
    match (cda, cla) {
        (Some(cda), Some(cla)) if cda.is_finite() && cla.is_finite() => Ok((cda, cla)),
        _ => Err(NO_VALUES.into()),
    }
}

fn vehicle_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let curve = parse_curve(p.str("engine_curve")?).or_fail(ctx)?;
    let gears = if p.str("gear_ratios")?.trim().is_empty() { Vec::new() } else { parse_list(p.str("gear_ratios")?).or_fail(ctx)? };
    let mut v = Vehicle {
        mass_kg: p.float("mass_kg")?,
        rot_inertia_frac: p.float("rot_inertia_frac")?,
        power_w: p.float("power_w")?,
        power_density_exponent: p.float("power_density_exponent")?,
        rho_ref: p.float("rho_ref")?,
        engine_curve: curve,
        gear_ratios: gears,
        final_drive: p.float("final_drive")?,
        wheel_radius_m: p.float("wheel_radius_m")?,
        driveline_eff: p.float("driveline_eff")?,
        f_drive_max_n: p.float("f_drive_max_n")?,
        cda_m2: p.float("cda_m2")?,
        cla_m2: p.float("cla_m2")?,
        cda_offset_m2: p.float("cda_offset_m2")?,
        mu_x: p.float("mu_x")?,
        mu_y: p.float("mu_y")?,
        mu_drop_per_kn: p.float("mu_drop_per_kn")?,
        crr: p.float("crr")?,
        rr_includes_downforce: p.bool("rr_includes_downforce")?,
        brake_max_g: p.float("brake_max_g")?,
        v_cap_ms: p.float("v_cap_ms")?,
        aero_balance_front: p.float("aero_balance_front")?,
    };
    let mut rep = Report::new("Vehicle");
    if let Some(d) = inputs.first().and_then(|d| d.as_ref()) {
        let (cda, cla) = aero_from(d).map_err(|e| ctx.fail(e))?;
        v.cda_m2 = cda;
        v.cla_m2 = if p.bool("lift_positive")? { -cla } else { cla };
        rep.note(Severity::Info, format!("aero taken from the connected result: CdA {:.4} m2, downforce area {:.4} m2", v.cda_m2, v.cla_m2));
    }
    v.validate().or_fail(ctx)?;
    rep.metric("mass", Metric::new(v.mass_kg, "kg"))
        .metric("weight", Metric::new(v.weight(), "N"))
        .metric("power_to_weight", Metric::new(v.power_w / v.mass_kg, "W/kg"))
        .metric("CdA", Metric::new(v.cda_eff(), "m2"))
        .metric("downforce_area", Metric::new(v.cla_m2, "m2"));
    if v.is_geared() {
        rep.note(Severity::Info, format!("geared drivetrain with {} gears", v.gear_ratios.len()));
    } else if !v.engine_curve.is_empty() || !v.gear_ratios.is_empty() {
        rep.note(Severity::Warning, "engine curve and gear ratios must both be given; falling back to the wheel-power model");
    }
    if v.cla_m2 < 0.0 {
        rep.note(Severity::Warning, "negative downforce area (net lift)");
    }
    Ok(vec![Data::json(v.to_json()), Data::report(rep)])
}

// ------------------------------------------------------------------ grip envelope

fn gg_info() -> NodeTypeInfo {
    node("perf.gg_envelope", "Grip Envelope")
        .input(PortSpec::new("vehicle", DataKind::Json))
        .input(PortSpec::optional("env", DataKind::Json))
        .output(PortSpec::new("table", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::int("n_speeds", "Speeds", 20, 2, 200).help("Number of speeds from the first step up to the maximum"))
        .param(ParamSpec::float("v_max_ms", "Maximum speed", 0.0, 0.0, 150.0).unit("m/s").help("0 = the vehicle's speed cap"))
        .param(ParamSpec::int("ay_points", "Lateral steps", 21, 3, 101).help("Points from straight-line to the lateral limit"))
        .param(rho_param())
        .help("Combined-grip envelope: at each speed, the lateral limit and how much acceleration or braking is left at each lateral load (friction ellipse), including drag and rolling.")
        .emulates("g-g-v diagram of a quasi-steady lap simulator")
}

fn gg_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let veh = vehicle_in(ctx, inputs, 0)?.ok_or(CookError::MissingInput { index: 0, name: "vehicle".into() })?;
    let rho = env_in(ctx, inputs, 1, p)?.rho;
    let ns = p.int("n_speeds")?.clamp(2, 200) as usize;
    let na = p.int("ay_points")?.clamp(3, 101) as usize;
    let vmax = if p.float("v_max_ms")? > 0.0 { p.float("v_max_ms")? } else { veh.v_cap_ms };
    let cols = [
        ("v_ms", "m/s"),
        ("ay_frac", "-"),
        ("ay_ms2", "m/s2"),
        ("ax_grip_ms2", "m/s2"),
        ("ax_accel_ms2", "m/s2"),
        ("ax_brake_ms2", "m/s2"),
    ];
    let mut rows = Vec::with_capacity(ns * na);
    let mut peak_ay = 0.0f64;
    let mut peak_brake = 0.0f64;
    for i in 1..=ns {
        let v = vmax * i as f64 / ns as f64;
        let n = veh.normal_load(rho, v);
        let ay_max = veh.mu_y_at(n) * n / veh.mass_kg;
        peak_ay = peak_ay.max(ay_max);
        for j in 0..na {
            let frac = j as f64 / (na - 1) as f64;
            let ay = frac * ay_max;
            let kappa = ay / (v * v);
            let g = veh.grip(rho, v, kappa);
            let (a_acc, _, _) = veh.accel(rho, v, kappa);
            let (a_dec, _) = veh.decel(rho, v, kappa);
            if j == 0 {
                peak_brake = peak_brake.max(a_dec);
            }
            rows.push(vec![v, frac, ay, g.fx_max / veh.mass_kg, a_acc, a_dec]);
        }
    }
    let mut rep = Report::new("Grip envelope");
    rep.metric("peak_lateral_g", Metric::new(peak_ay / G, "g"))
        .metric("peak_brake_g", Metric::new(peak_brake / G, "g"))
        .metric("static_lateral_g", Metric::new(veh.mu_y_at(veh.weight()), "g"))
        .metric("rho", Metric::new(rho, "kg/m3"));
    Ok(vec![Data::json(table_json(&cols, rows)), Data::report(rep)])
}

// ------------------------------------------------------------------ corner limits

fn corner_info() -> NodeTypeInfo {
    node("perf.corner_limits", "Corner Speed Table")
        .input(PortSpec::new("vehicle", DataKind::Json))
        .input(PortSpec::optional("env", DataKind::Json))
        .output(PortSpec::new("table", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::string("radii_m", "Corner radii", "15, 25, 50, 100, 200, 500").unit("m").help("Corner radii, comma separated (15-500)"))
        .param(ParamSpec::bool("compare", "Compare a second ClA", false).help("Add the speed and gain for the alternative downforce area below"))
        .param(ParamSpec::float("compare_cla_m2", "Alternative ClA", 1.0, -1.0, 6.0).unit("m2").help("Downforce area for the comparison column"))
        .param(rho_param())
        .help("Steady cornering speed for a list of radii, with and without downforce, and the percentage speed gained from the downforce.")
        .emulates("corner-speed relation v = sqrt(mu g R / (1 - mu rho ClA R / 2m))")
}

fn corner_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let veh = vehicle_in(ctx, inputs, 0)?.ok_or(CookError::MissingInput { index: 0, name: "vehicle".into() })?;
    let rho = env_in(ctx, inputs, 1, p)?.rho;
    let radii = parse_list(p.str("radii_m")?).or_fail(ctx)?;
    if radii.is_empty() || radii.iter().any(|r| !(*r > 0.0 && r.is_finite())) {
        return Err(ctx.fail("radii must be a list of positive numbers"));
    }
    let compare = p.bool("compare")?;
    let alt = p.float("compare_cla_m2")?;
    let mut noaero = veh.clone();
    noaero.cla_m2 = 0.0;
    let mut alt_veh = veh.clone();
    alt_veh.cla_m2 = alt;
    let mut cols = vec![
        ("radius_m", "m"),
        ("k", "-"),
        ("v_lim_ms", "m/s"),
        ("v_lim_kmh", "km/h"),
        ("v_noaero_ms", "m/s"),
        ("gain_pct", "%"),
    ];
    if compare {
        cols.push(("v_alt_ms", "m/s"));
        cols.push(("gain_alt_pct", "%"));
    }
    let mut rows = Vec::new();
    let mut rep = Report::new("Corner speeds");
    for &r in &radii {
        let kappa = 1.0 / r;
        let v = veh.corner_limit(rho, kappa);
        let v0 = noaero.corner_limit(rho, kappa);
        let k = veh.mu_y * rho * veh.cla_m2 * r / (2.0 * veh.mass_kg);
        let mut row = vec![r, k, v, v * 3.6, v0, 100.0 * (v / v0 - 1.0)];
        if compare {
            let va = alt_veh.corner_limit(rho, kappa);
            row.push(va);
            row.push(100.0 * (va / v0 - 1.0));
        }
        rep.metric(format!("v_lim_R{r}"), Metric::new(v, "m/s"));
        rep.metric(format!("gain_R{r}"), Metric::new(100.0 * (v / v0 - 1.0), "%"));
        rows.push(row);
    }
    if veh.v_cap_ms <= rows.iter().map(|r| r[2]).fold(0.0, f64::max) + 1e-9 {
        rep.note(Severity::Info, "at least one radius is limited by the speed cap, not by grip");
    }
    Ok(vec![Data::json(table_json(&cols, rows)), Data::report(rep)])
}

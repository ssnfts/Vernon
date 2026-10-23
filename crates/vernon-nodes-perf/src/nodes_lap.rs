//! Lap nodes: lap simulation, parameter sweep with sensitivities, and A/B comparison.

use rayon::prelude::*;
use vernon_core::{Metric, Report, Severity};
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Stage};

use crate::lapsim::{self, default_step, LapOptions, SweepParam, TimeRule};
use crate::nodes_track::coloured_path;
use crate::physics::Vehicle;
use crate::track::Track;
use crate::util::{env_in, json_in, rho_param, table_json, vehicle_in, FnNode, OrFail, SPEC};

fn node(id: &str, label: &str) -> NodeTypeInfo {
    NodeTypeInfo::new(id, label, Stage::Performance).spec(SPEC).implemented()
}

pub fn register(reg: &mut vernon_graph::Registry) {
    reg.add(FnNode::new(lap_info, lap_cook));
    reg.add(FnNode::new(sweep_info, sweep_cook));
    reg.add(FnNode::new(delta_info, delta_cook));
}

fn need_vehicle(ctx: &CookContext, inputs: &[Option<Data>], i: usize, name: &str) -> CookResult<Vehicle> {
    vehicle_in(ctx, inputs, i)?.ok_or(CookError::MissingInput { index: i, name: name.into() })
}

fn need_track(ctx: &CookContext, inputs: &[Option<Data>], i: usize) -> CookResult<Track> {
    let v = json_in(inputs, i)?.ok_or(CookError::MissingInput { index: i, name: "track".into() })?;
    Track::from_json(&v).or_fail(ctx)
}

fn time_rule(p: &Params) -> CookResult<TimeRule> {
    match p.str("time_rule")? {
        "trapezoid" => Ok(TimeRule::Trapezoid),
        "sum_ds_over_v" => Ok(TimeRule::SumDsOverV),
        other => Err(CookError::BadParam(format!("time_rule = {other}"))),
    }
}

fn sim_params(info: NodeTypeInfo) -> NodeTypeInfo {
    info.param(ParamSpec::choice("time_rule", "Time rule", "trapezoid", &["trapezoid", "sum_ds_over_v"]).help("How lap time is summed: trapezoid per interval (exact for constant acceleration) or distance over node speed"))
        .param(ParamSpec::float("ds_max_m", "Largest step", 5.0, 0.1, 20.0).unit("m").help("Intervals longer than this are subdivided before solving; the step is also kept below 1/20 of the tightest corner radius"))
        .param(rho_param())
}

fn sim_opts(p: &Params) -> CookResult<LapOptions> {
    Ok(LapOptions { time_rule: time_rule(p)?, ds_max_m: p.float("ds_max_m")?, ..LapOptions::default() })
}

// ------------------------------------------------------------------ lap simulation

fn lap_info() -> NodeTypeInfo {
    sim_params(
        node("perf.lap_sim", "Lap Simulation")
            .input(PortSpec::new("vehicle", DataKind::Json))
            .input(PortSpec::new("track", DataKind::Json))
            .input(PortSpec::optional("env", DataKind::Json))
            .output(PortSpec::new("lap", DataKind::Json))
            .output(PortSpec::new("geometry", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report)),
    )
    .param(ParamSpec::float("start_speed_ms", "Start speed", 0.0, 0.0, 150.0).unit("m/s").help("Speed at the start of an open track (0 = standing start); a closed lap finds its own"))
    .param(ParamSpec::float("end_speed_ms", "End speed", -1.0, -1.0, 150.0).unit("m/s").help("Required speed at the end of an open track; negative = free"))
    .param(ParamSpec::bool("energy", "Energy use", true).help("Report the energy drawn over the lap"))
    .param(ParamSpec::float("regen_frac", "Braking recovery", 0.0, 0.0, 1.0).help("Fraction of braking energy recovered"))
    .param(ParamSpec::bool("hold_limit_speed", "Hold cornering limit", false).help("Hold exactly the limit speed on constant-radius arcs instead of letting drag sag it slightly"))
    .param(ParamSpec::float("kappa_straight", "Straight curvature", 0.001, 1.0e-5, 0.1).unit("1/m").help("Below this curvature a piece counts as straight in the event table"))
    .param(ParamSpec::float("min_seg_m", "Shortest segment", 3.0, 0.5, 100.0).unit("m").help("Shorter pieces merge into their neighbour in the event table"))
    .help("Point-mass quasi-steady lap simulation: cornering limits from the friction ellipse with downforce, then a forward (power, traction, drag) and a backward (braking) pass; gives lap time, speed trace, channels, apex and end-of-straight speeds and energy.")
    .emulates("point-mass quasi-steady lap simulation (OptimumLap-style)")
}

fn lap_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let veh = need_vehicle(ctx, inputs, 0, "vehicle")?;
    let track = need_track(ctx, inputs, 1)?;
    let rho = env_in(ctx, inputs, 2, p)?.rho;
    let mut o = sim_opts(p)?;
    o.start_speed = Some(p.float("start_speed_ms")?);
    o.end_speed = Some(p.float("end_speed_ms")?).filter(|e| *e >= 0.0);
    o.hold_limit_speed = p.bool("hold_limit_speed")?;
    o.regen_frac = p.float("regen_frac")?;
    o.kappa_straight = p.float("kappa_straight")?;
    o.min_seg_m = p.float("min_seg_m")?;
    let r = lapsim::solve(&veh, rho, &track, &o).or_fail(ctx)?;

    let mut rep = Report::new("Lap simulation");
    let apex = r.segments.iter().filter(|s| s.kind != crate::track::SegKind::Straight).map(|s| s.v_min).fold(f64::MAX, f64::min);
    let eos = r.segments.iter().filter(|s| s.kind == crate::track::SegKind::Straight).map(|s| s.v_max).fold(f64::MIN, f64::max);
    rep.metric("lap_time", Metric::new(r.lap_time_s, "s"))
        .metric("length", Metric::new(r.track.length(), "m"))
        .metric("v_max", Metric::new(r.v_max() * 3.6, "km/h"))
        .metric("v_min", Metric::new(r.v_min() * 3.6, "km/h"))
        .metric("avg_speed", Metric::new(r.track.length() / r.lap_time_s * 3.6, "km/h"))
        .metric("rho", Metric::new(rho, "kg/m3"))
        .metric("envelope_violations", Metric::new(r.violations as f64, "-").with_limit(0.0, true));
    if apex < f64::MAX {
        rep.metric("min_apex_speed", Metric::new(apex * 3.6, "km/h"));
    }
    if eos > f64::MIN {
        rep.metric("max_end_of_straight_speed", Metric::new(eos * 3.6, "km/h"));
    }
    if p.bool("energy")? {
        rep.metric("energy", Metric::new(r.energy_kwh(), "kWh"))
            .metric("wheel_energy", Metric::new(r.energy_wheel_j / 3.6e6, "kWh"));
    }
    if r.violations > 0 {
        rep.note(Severity::Warning, format!("{} interval(s) leave the grip envelope", r.violations));
    }
    rep.note(Severity::Info, format!("time rule: {}; point-mass model, flat road, no load transfer: use for differences between set-ups", r.time_rule.name()));
    let geo = coloured_path(&r.track, &r.v, "v_kmh", 3.6);
    Ok(vec![Data::json(r.to_json()), Data::geometry(geo), Data::report(rep)])
}

// ------------------------------------------------------------------ sweep

fn sweep_info() -> NodeTypeInfo {
    let names = SweepParam::NAMES;
    let mut opts: Vec<&str> = vec!["none"];
    opts.extend(names);
    sim_params(
        node("perf.lap_sweep", "Parameter Sweep")
            .input(PortSpec::new("vehicle", DataKind::Json))
            .input(PortSpec::new("track", DataKind::Json))
            .input(PortSpec::optional("env", DataKind::Json))
            .output(PortSpec::new("table", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report)),
    )
    .param(ParamSpec::choice("param", "Parameter", "cda_m2", &names).help("Quantity to vary (friction sets both directions; power at the wheels)"))
    .param(ParamSpec::float("lo", "From", 0.0, -1.0e7, 1.0e7).help("First value; with To equal to From the range is 80 to 120 percent of the base value (0 to 1 when the base is zero)"))
    .param(ParamSpec::float("hi", "To", 0.0, -1.0e7, 1.0e7).help("Last value"))
    .param(ParamSpec::int("n", "Points", 9, 2, 101).help("Number of lap solves along the range"))
    .param(ParamSpec::choice("second_param", "Second parameter", "none", &opts).help("Optional second quantity for a grid"))
    .param(ParamSpec::float("second_lo", "Second from", 0.0, -1.0e7, 1.0e7).help("First value of the second parameter (equal ends = 80 to 120 percent)"))
    .param(ParamSpec::float("second_hi", "Second to", 0.0, -1.0e7, 1.0e7).help("Last value of the second parameter"))
    .param(ParamSpec::int("second_n", "Second points", 5, 2, 51).help("Points of the second parameter"))
    .param(ParamSpec::float("step_pct", "Sensitivity step", 5.0, 0.1, 50.0).unit("%").help("Central-difference step as a percentage of the value"))
    .help("Re-runs the lap while one (or two) quantities vary, tabulates lap time, and reports lap-time sensitivities to drag area, downforce area, mass, friction, power and density, plus the break-even downforce per unit of extra drag.")
    .emulates("parameter sweeps and what-if comparisons of a lap simulator")
}

fn linspace(lo: f64, hi: f64, n: usize, base: f64) -> Vec<f64> {
    let (lo, hi) = if lo == hi {
        if base != 0.0 {
            (0.8 * base, 1.2 * base)
        } else {
            (0.0, 1.0)
        }
    } else {
        (lo, hi)
    };
    (0..n).map(|i| lo + (hi - lo) * i as f64 / (n - 1) as f64).collect()
}

fn sweep_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let veh = need_vehicle(ctx, inputs, 0, "vehicle")?;
    let track = need_track(ctx, inputs, 1)?;
    let rho = env_in(ctx, inputs, 2, p)?.rho;
    let o = sim_opts(p)?;
    let sp = SweepParam::parse(p.str("param")?).ok_or_else(|| CookError::BadParam("param".into()))?;
    let second = match p.str("second_param")? {
        "none" => None,
        s => Some(SweepParam::parse(s).ok_or_else(|| CookError::BadParam("second_param".into()))?),
    };
    let base = lapsim::lap_time(&veh, rho, &track, &o).or_fail(ctx)?;
    let xs = linspace(p.float("lo")?, p.float("hi")?, p.int("n")?.max(2) as usize, sp.value(&veh, rho));
    let eval2 = |a: f64, b: Option<(SweepParam, f64)>| -> Result<f64, String> {
        let mut v = veh.clone();
        let mut r = rho;
        sp.apply(&mut v, &mut r, a);
        if let Some((s2, x2)) = b {
            s2.apply(&mut v, &mut r, x2);
        }
        lapsim::lap_time(&v, r, &track, &o)
    };
    let mut rep = Report::new("Parameter sweep");
    let table = if let Some(s2) = second {
        let ys = linspace(p.float("second_lo")?, p.float("second_hi")?, p.int("second_n")?.max(2) as usize, s2.value(&veh, rho));
        let combos: Vec<(f64, f64)> = xs.iter().flat_map(|&x| ys.iter().map(move |&y| (x, y))).collect();
        let times: Vec<f64> = combos.par_iter().map(|&(x, y)| eval2(x, Some((s2, y)))).collect::<Result<_, String>>().or_fail(ctx)?;
        let rows = combos.iter().zip(&times).map(|(&(x, y), &t)| vec![x, y, t, t - base]).collect();
        rep.metric("lap_time_min", Metric::new(times.iter().copied().fold(f64::MAX, f64::min), "s"))
            .metric("lap_time_max", Metric::new(times.iter().copied().fold(f64::MIN, f64::max), "s"));
        table_json(&[(sp.name(), sp.unit()), (s2.name(), s2.unit()), ("lap_time_s", "s"), ("delta_vs_base_s", "s")], rows)
    } else {
        let times: Vec<f64> = xs.par_iter().map(|&x| eval2(x, None)).collect::<Result<_, String>>().or_fail(ctx)?;
        // expected direction of lap time with the parameter
        let sign = match sp {
            SweepParam::CdA | SweepParam::Mass => Some(1.0),
            SweepParam::ClA | SweepParam::Mu | SweepParam::Power => Some(-1.0),
            SweepParam::Rho => None,
        };
        let monotonic = sign.map(|s| times.windows(2).all(|w| s * (w[1] - w[0]) >= -1e-9));
        if let Some(m) = monotonic {
            rep.metric("monotonic", Metric::new(if m { 1.0 } else { 0.0 }, "-"));
            if !m {
                rep.note(Severity::Warning, "lap time does not move monotonically with the swept quantity; check the range and the track resolution");
            }
        }
        rep.metric("lap_time_min", Metric::new(times.iter().copied().fold(f64::MAX, f64::min), "s"))
            .metric("lap_time_max", Metric::new(times.iter().copied().fold(f64::MIN, f64::max), "s"));
        let rows = xs.iter().zip(&times).map(|(&x, &t)| vec![x, t, t - base]).collect();
        table_json(&[(sp.name(), sp.unit()), ("lap_time_s", "s"), ("delta_vs_base_s", "s")], rows)
    };
    // sensitivities at the base point
    let pct = p.float("step_pct")? / 100.0;
    let all = [SweepParam::CdA, SweepParam::ClA, SweepParam::Mass, SweepParam::Mu, SweepParam::Power, SweepParam::Rho];
    let sens: Vec<f64> = all
        .par_iter()
        .map(|&s| {
            let x = s.value(&veh, rho);
            let h = if x.abs() > 1e-12 { pct * x.abs() } else { default_step(0.0, 0.05) };
            lapsim::sensitivity(&veh, rho, &track, &o, s, h)
        })
        .collect::<Result<_, String>>()
        .or_fail(ctx)?;
    rep.metric("base_lap_time", Metric::new(base, "s"))
        .metric("dt_dcda", Metric::new(sens[0], "s/m2"))
        .metric("dt_dcda_per_0p01", Metric::new(sens[0] * 0.01, "s"))
        .metric("dt_dcla", Metric::new(sens[1], "s/m2"))
        .metric("dt_dcla_per_0p01", Metric::new(sens[1] * 0.01, "s"))
        .metric("dt_dmass", Metric::new(sens[2], "s/kg"))
        .metric("dt_dmu", Metric::new(sens[3], "s"))
        .metric("dt_dpower", Metric::new(sens[4] * 1000.0, "s/kW"))
        .metric("dt_drho", Metric::new(sens[5], "s per kg/m3"));
    if sens[1].abs() > 1e-12 {
        rep.metric("breakeven_dcla_per_dcda", Metric::new(-sens[0] / sens[1], "-"));
    }
    rep.note(Severity::Info, "sensitivities are central differences at the base vehicle; a downforce area that is bought with drag area pays off only above the break-even ratio");
    Ok(vec![Data::json(table), Data::report(rep)])
}

// ------------------------------------------------------------------ A/B delta

fn delta_info() -> NodeTypeInfo {
    sim_params(
        node("perf.lap_delta", "Lap A/B Delta")
            .input(PortSpec::new("vehicle_a", DataKind::Json))
            .input(PortSpec::new("vehicle_b", DataKind::Json))
            .input(PortSpec::new("track", DataKind::Json))
            .input(PortSpec::optional("env", DataKind::Json))
            .output(PortSpec::new("table", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report)),
    )
    .help("Runs two vehicles (for example without and with a body kit) round the same track and reports the lap-time difference, how it splits across straights and corners, and the change in top and minimum speed and energy.")
    .emulates("A/B lap-time comparison of two vehicle set-ups in a lap simulator")
}

fn delta_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let a = need_vehicle(ctx, inputs, 0, "vehicle_a")?;
    let b = need_vehicle(ctx, inputs, 1, "vehicle_b")?;
    let track = need_track(ctx, inputs, 2)?;
    let rho = env_in(ctx, inputs, 3, p)?.rho;
    let o = sim_opts(p)?;
    let ra = lapsim::solve(&a, rho, &track, &o).or_fail(ctx)?;
    let rb = lapsim::solve(&b, rho, &track, &o).or_fail(ctx)?;
    let rows: Vec<Vec<f64>> = ra
        .segments
        .iter()
        .zip(&rb.segments)
        .enumerate()
        .map(|(i, (sa, sb))| {
            let code = match sa.kind {
                crate::track::SegKind::Straight => 0.0,
                crate::track::SegKind::Left => 1.0,
                crate::track::SegKind::Right => 2.0,
            };
            vec![i as f64, code, sa.s_start_m, sa.length_m, sa.time_s, sb.time_s, sb.time_s - sa.time_s, sa.v_min * 3.6, sb.v_min * 3.6, sa.v_max * 3.6, sb.v_max * 3.6]
        })
        .collect();
    let cols = [
        ("segment", "-"),
        ("type_code", "0 straight, 1 left, 2 right"),
        ("s_start_m", "m"),
        ("length_m", "m"),
        ("time_a_s", "s"),
        ("time_b_s", "s"),
        ("delta_s", "s"),
        ("v_min_a_kmh", "km/h"),
        ("v_min_b_kmh", "km/h"),
        ("v_max_a_kmh", "km/h"),
        ("v_max_b_kmh", "km/h"),
    ];
    let d = rb.lap_time_s - ra.lap_time_s;
    let mut rep = Report::new("Lap A/B delta");
    rep.metric("lap_time_a", Metric::new(ra.lap_time_s, "s"))
        .metric("lap_time_b", Metric::new(rb.lap_time_s, "s"))
        .metric("delta_lap_time", Metric::new(d, "s"))
        .metric("delta_lap_time_pct", Metric::new(100.0 * d / ra.lap_time_s, "%"))
        .metric("delta_v_max", Metric::new((rb.v_max() - ra.v_max()) * 3.6, "km/h"))
        .metric("delta_v_min", Metric::new((rb.v_min() - ra.v_min()) * 3.6, "km/h"))
        .metric("delta_energy", Metric::new(rb.energy_kwh() - ra.energy_kwh(), "kWh"))
        .metric("delta_cda", Metric::new(b.cda_eff() - a.cda_eff(), "m2"))
        .metric("delta_cla", Metric::new(b.cla_m2 - a.cla_m2, "m2"));
    rep.note(Severity::Info, if d < 0.0 { "B is faster than A" } else { "B is not faster than A" });
    Ok(vec![Data::json(table_json(&cols, rows)), Data::report(rep)])
}

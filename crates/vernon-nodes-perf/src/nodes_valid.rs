//! Validation nodes: coastdown fit, pressure to Cp, Cp comparison, tuft analysis and the
//! validation gate.

use glam::DVec3;
use serde_json::{json, Value};
use vernon_core::{AttribArray, Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs as inp;
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Stage};

use crate::coast::{self, CoastOpts, Method};
use crate::cp::{self, CpOpts, TuftClass, TuftThresholds};
use crate::gate::{self, GateInput};
use crate::lapsim::{self, LapOptions};
use crate::physics::Vehicle;
use crate::track::Track;
use crate::tuft;
use crate::util::{env_in, json_in, read_text, rho_param, table_json, FnNode, OrFail, SPEC};

fn node(id: &str, label: &str) -> NodeTypeInfo {
    NodeTypeInfo::new(id, label, Stage::Performance).spec(SPEC).implemented()
}

pub fn register(reg: &mut vernon_graph::Registry) {
    reg.add(FnNode::new(coast_info, coast_cook));
    reg.add(FnNode::new(cp_info, cp_cook));
    reg.add(FnNode::new(compare_info, compare_cook));
    reg.add(FnNode::new(tuft_info, tuft_cook));
    reg.add(FnNode::new(gate_info, gate_cook));
}

// ------------------------------------------------------------------ coastdown

fn coast_info() -> NodeTypeInfo {
    node("perf.coastdown_fit", "Coastdown Fit")
        .input(PortSpec::optional("log", DataKind::Any))
        .input(PortSpec::optional("env", DataKind::Json))
        .output(PortSpec::new("fit", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::path("log_path", "Log file", &["csv"], false).help("CSV with t, v_ms (or v_kmh) and optional run, dir, wind_ms, grade, temp_c, p_pa, rh (fraction, or percent when above 1); used when no log is connected"))
        .param(ParamSpec::float("mass_kg", "Mass", 1400.0, 50.0, 6000.0).unit("kg").help("Vehicle mass during the runs"))
        .param(ParamSpec::float("rot_mass_kg", "Rotating mass", 0.0, 0.0, 500.0).unit("kg").help("Equivalent mass of rotating parts, added to the mass in the force balance"))
        .param(ParamSpec::choice("method", "Method", "integrated", &["integrated", "accel"]).help("Integrated: least-squares fit of the closed-form speed curve. Accel: regression of 1 s deceleration slopes"))
        .param(ParamSpec::float("speed_hi_kmh", "Window top speed", 120.0, 20.0, 300.0).unit("km/h").help("Upper edge of the speed window"))
        .param(ParamSpec::float("speed_lo_kmh", "Window bottom speed", 20.0, 2.0, 200.0).unit("km/h").help("Lower edge of the speed window"))
        .param(ParamSpec::int("min_runs", "Minimum valid runs", 8, 1, 100).help("The report fails below this many valid runs"))
        .param(ParamSpec::float("wind_max_ms", "Wind limit", 2.7, 0.0, 20.0).unit("m/s").help("Runs with a mean head-wind component above this are rejected; 0 disables the check"))
        .param(ParamSpec::float("outlier_sd", "Outlier limit", 2.0, 0.5, 5.0).help("Pairs further than this many standard deviations from the mean are dropped"))
        .param(ParamSpec::float("crr_fixed", "Fixed rolling resistance", 0.0, 0.0, 0.05).help("0 = fit it. A fixed value fits drag only and biases it if the value is wrong"))
        .param(ParamSpec::bool("hampel", "Despike speed", true).help("Remove speed spikes with a Hampel filter (3 s window)"))
        .param(rho_param())
        .help("Fits drag area and rolling resistance to logged coastdown runs: speed-spike filter, wind and density handling, opposite-direction pairing, outlier removal, standard errors, the drag/rolling correlation and the smallest change in drag area the test can detect. Heading-based yaw correction is not applied.")
        .emulates("coastdown regression of road-load coefficients from a speed log")
}

fn coast_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let runs = match inputs.first().and_then(|d| d.as_ref()) {
        Some(Data::File(f)) => {
            let cols = coast::parse_columns_csv(&read_text(ctx, f)?).or_fail(ctx)?;
            coast::runs_from_columns(&cols).or_fail(ctx)?
        }
        Some(Data::Json(v)) => coast::runs_from_json(v).or_fail(ctx)?,
        Some(other) => return Err(CookError::WrongInput { index: 0, expected: DataKind::Any, found: other.kind() }),
        None => {
            let path = p.str("log_path")?;
            if path.is_empty() {
                return Err(CookError::MissingInput { index: 0, name: "log".into() });
            }
            let cols = coast::parse_columns_csv(&read_text(ctx, std::path::Path::new(path))?).or_fail(ctx)?;
            coast::runs_from_columns(&cols).or_fail(ctx)?
        }
    };
    let env = env_in(ctx, inputs, 1, p)?;
    let mut o = CoastOpts::new(p.float("mass_kg")?);
    o.rot_mass_kg = p.float("rot_mass_kg")?;
    o.method = if p.str("method")? == "accel" { Method::Accel } else { Method::Integrated };
    o.v_hi_ms = p.float("speed_hi_kmh")? / 3.6;
    o.v_lo_ms = p.float("speed_lo_kmh")? / 3.6;
    o.min_runs = p.int("min_runs")?.max(1) as usize;
    o.wind_max_ms = p.float("wind_max_ms")?;
    o.outlier_sd = p.float("outlier_sd")?;
    o.crr_fixed = p.float("crr_fixed")?;
    o.hampel = p.bool("hampel")?;
    o.rho_default = Some(env.rho);
    let fit = coast::fit_coastdown(&runs, &o).or_fail(ctx)?;

    let mut rep = Report::new("Coastdown fit");
    rep.metric("CdA", Metric::new(fit.cda, "m2"))
        .metric("CdA_se", Metric::new(fit.cda_se, "m2"))
        .metric("CdA_ci95_lo", Metric::new(fit.cda_ci95.0, "m2"))
        .metric("CdA_ci95_hi", Metric::new(fit.cda_ci95.1, "m2"))
        .metric("Crr", Metric::new(fit.crr, "-"))
        .metric("Crr_se", Metric::new(fit.crr_se, "-"))
        .metric("valid_runs", Metric::new(fit.n_valid_runs as f64, "runs").with_limit(o.min_runs as f64, false))
        .metric("pairs_used", Metric::new(fit.n_pairs_used as f64, "pairs"))
        .metric("min_detectable_dcda", Metric::new(fit.min_detectable_dcda, "m2"))
        .metric("rho_mean", Metric::new(fit.rho_mean, "kg/m3"));
    if fit.corr_cda_crr.is_finite() {
        rep.metric("corr_cda_crr", Metric::new(fit.corr_cda_crr, "-"));
    }
    rep.notes.extend(fit.notes.iter().cloned());
    if o.crr_fixed > 0.0 {
        let mut o2 = o.clone();
        o2.crr_fixed = o.crr_fixed * 1.25;
        if let Ok(f2) = coast::fit_coastdown(&runs, &o2) {
            let shift = 100.0 * (f2.cda / fit.cda - 1.0);
            rep.metric("cda_shift_if_crr_plus25_pct", Metric::new(shift, "%"));
            rep.note(Severity::Warning, format!("rolling resistance is held fixed: if it is 25 percent too low, CdA moves by {shift:+.1} percent"));
        }
    }
    Ok(vec![Data::json(fit.to_json()), Data::report(rep)])
}

// ------------------------------------------------------------------ pressure to Cp

fn cp_info() -> NodeTypeInfo {
    node("perf.pressure_cp", "Pressure To Cp")
        .input(PortSpec::optional("taps", DataKind::Any))
        .input(PortSpec::optional("env", DataKind::Json))
        .output(PortSpec::new("cp", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::path("taps_path", "Tap file", &["csv"], false).help("CSV with id, x, y, z (mm), dp_pa (or dp_inwc), speed_ms (or speed_kmh) and optional rho; used when no taps are connected"))
        .param(ParamSpec::float("speed_ms", "Speed (all taps)", 0.0, 0.0, 150.0).unit("m/s").help("Used for rows without a speed; 0 = require a speed column"))
        .param(ParamSpec::float("rho_kgm3", "Density (no env input)", 0.0, 0.0, 1.5).unit("kg/m3").help("Used for rows without a density when no air state is connected; 0 = none"))
        .param(ParamSpec::float("gauge_range_pa", "Gauge range", 249.0, 10.0, 5000.0).unit("Pa").help("Full-scale range of the differential gauge (about 1 inch of water = 249 Pa)"))
        .param(ParamSpec::float("speed_sd_ms", "Speed uncertainty", 0.3, 0.0, 5.0).unit("m/s").help("One standard deviation of the speed reading"))
        .param(ParamSpec::float("dp_sd_pa", "Pressure uncertainty", 3.0, 0.0, 100.0).unit("Pa").help("One standard deviation of the gauge reading"))
        .param(ParamSpec::float("rho_sd_frac", "Density uncertainty", 0.01, 0.0, 0.2).help("Relative standard deviation of the density"))
        .param(ParamSpec::choice("ref_mode", "Reference", "static_port", &["static_port", "sealed"]).help("What the low side of the gauge is connected to"))
        .help("Converts surface-pressure readings against a static reference into pressure coefficients Cp = dp / q, with the propagated uncertainty and a check that the gauge range is large enough.")
        .emulates("on-road surface-pressure probe reduction to Cp")
}

fn load_taps(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<cp::TapIn>> {
    match inputs.first().and_then(|d| d.as_ref()) {
        Some(Data::File(f)) => cp::parse_taps_csv(&read_text(ctx, f)?).or_fail(ctx),
        Some(Data::Json(v)) => cp::parse_taps_json(v).or_fail(ctx),
        Some(other) => Err(CookError::WrongInput { index: 0, expected: DataKind::Any, found: other.kind() }),
        None => {
            let path = p.str("taps_path")?;
            if path.is_empty() {
                return Err(CookError::MissingInput { index: 0, name: "taps".into() });
            }
            cp::parse_taps_csv(&read_text(ctx, std::path::Path::new(path))?).or_fail(ctx)
        }
    }
}

fn cp_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let taps = load_taps(ctx, inputs, p)?;
    let rho_default = match json_in(inputs, 1)? {
        Some(v) => Some(crate::physics::Env::from_json(&v).or_fail(ctx)?.rho),
        None => Some(p.float("rho_kgm3")?).filter(|r| *r > 0.0),
    };
    let o = CpOpts {
        rho_default,
        speed_default: Some(p.float("speed_ms")?).filter(|v| *v > 0.0),
        speed_sd_ms: p.float("speed_sd_ms")?,
        dp_sd_pa: p.float("dp_sd_pa")?,
        rho_sd_frac: p.float("rho_sd_frac")?,
    };
    let rows = cp::compute_cp(&taps, &o).or_fail(ctx)?;
    let max_dp = rows.iter().fold(0.0f64, |m, r| m.max(r.dp_pa.abs()));
    let gauge = p.float("gauge_range_pa")?;
    let mut rep = Report::new("Pressure coefficients");
    rep.metric("taps", Metric::new(rows.len() as f64, "-"))
        .metric("cp_max", Metric::new(rows.iter().map(|r| r.cp).fold(f64::MIN, f64::max), "-"))
        .metric("cp_min", Metric::new(rows.iter().map(|r| r.cp).fold(f64::MAX, f64::min), "-"))
        .metric("cp_sd_mean", Metric::new(rows.iter().map(|r| r.cp_sd).sum::<f64>() / rows.len() as f64, "-"))
        .metric("max_abs_dp", Metric::new(max_dp, "Pa"));
    if max_dp > 0.0 {
        rep.metric("gauge_headroom", Metric::new(gauge / max_dp, "x").with_limit(1.5, false));
        if gauge < 1.5 * max_dp {
            rep.note(Severity::Warning, "gauge range is below 1.5 times the largest reading: use a larger range");
        }
    }
    if p.str("ref_mode")? == "sealed" {
        rep.note(Severity::Info, "sealed reference: re-zero before each run, the vessel pressure drifts with temperature and weather");
    }
    rep.note(Severity::Info, "prefer differences between set-ups at the same tap on the same day: sensor offsets cancel");
    Ok(vec![Data::json(cp::cp_table_json(&rows)), Data::report(rep)])
}

// ------------------------------------------------------------------ Cp comparison

fn compare_info() -> NodeTypeInfo {
    node("perf.cp_compare", "Cp Versus Simulation")
        .input(PortSpec::new("cp", DataKind::Json))
        .input(PortSpec::new("surface", DataKind::Geometry))
        .output(PortSpec::new("table", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::float("tap_radius_mm", "Tap radius", 20.0, 1.0, 100.0).unit("mm").help("The simulated Cp is area-averaged over a disc of this radius around the tap"))
        .param(ParamSpec::float("tol", "Tolerance", 0.05, 0.005, 0.5).help("Smallest allowed |Cp difference| (repeatability floor)"))
        .param(ParamSpec::float("sigma_k", "Uncertainty multiple", 2.0, 0.0, 5.0).help("A tap passes when its difference is within max(tolerance, this multiple of its own uncertainty)"))
        .param(ParamSpec::float("max_offset_mm", "Largest tap offset", 30.0, 1.0, 500.0).unit("mm").help("Warn when a tap is further than this from the surface"))
        .help("Compares measured Cp at each tap with the simulated surface Cp (a Cp attribute on the surface): bias, RMS, worst tap, sign agreement and a pass/fail per tap.")
        .emulates("point-by-point comparison of measured and simulated surface pressure")
}

fn compare_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let table = json_in(inputs, 0)?.ok_or(CookError::MissingInput { index: 0, name: "cp".into() })?;
    let taps = cp::cp_table_from_json(&table).or_fail(ctx)?;
    let surface = inp::geometry(inputs, 1)?;
    surface.validate()?;
    let res = cp::compare(&taps, &surface, p.float("tap_radius_mm")?, p.float("tol")?, p.float("sigma_k")?).or_fail(ctx)?;
    let n = res.rows.len();
    let disagree = n - res.sign_agree;
    let mut rep = Report::new("Cp comparison");
    rep.metric("taps", Metric::new(n as f64, "-"))
        .metric("bias_cp", Metric::new(res.bias, "-"))
        .metric("rms_cp", Metric::new(res.rms, "-"))
        .metric("max_abs_cp", Metric::new(res.max_abs, "-"))
        .metric("sign_agree", Metric::new(res.sign_agree as f64, "taps"))
        .metric("sign_disagree", Metric::new(disagree as f64, "taps").with_limit(0.0, true))
        .metric("taps_failed", Metric::new(res.failed as f64, "taps").with_limit(0.0, true));
    if disagree > 0 {
        rep.note(Severity::Warning, format!("sign agreement {}/{n}: a tap on the wrong side of zero matters more than the RMS", res.sign_agree));
    }
    if res.rms > 0.1 {
        rep.note(Severity::Warning, format!("RMS Cp residual {:.3} is above 0.1", res.rms));
    }
    let far = res.rows.iter().filter(|r| r.offset_mm > p.float("max_offset_mm").unwrap_or(30.0)).count();
    if far > 0 {
        rep.note(Severity::Warning, format!("{far} tap(s) lie far from the surface: check the tap positions and the units"));
    }
    let rows = res
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| vec![i as f64, r.cp_meas, r.cp_sd, r.cp_sim, r.residual, r.tol, f64::from(u8::from(r.pass)), f64::from(u8::from(r.sign_ok)), r.offset_mm])
        .collect();
    let mut t = table_json(
        &[("tap", "-"), ("cp_meas", "-"), ("cp_sd", "-"), ("cp_sim", "-"), ("residual", "-"), ("tol", "-"), ("pass", "0/1"), ("sign_ok", "0/1"), ("offset_mm", "mm")],
        rows,
    );
    if let Value::Object(m) = &mut t {
        m.insert("ids".into(), json!(res.rows.iter().map(|r| r.id.clone()).collect::<Vec<_>>()));
    }
    Ok(vec![Data::json(t), Data::report(rep)])
}

// ------------------------------------------------------------------ tufts

fn tuft_info() -> NodeTypeInfo {
    node("perf.tuft_analysis", "Tuft Analysis")
        .input(PortSpec::optional("images", DataKind::File))
        .input(PortSpec::optional("patch", DataKind::Json))
        .input(PortSpec::optional("tufts", DataKind::Any))
        .input(PortSpec::optional("surface", DataKind::Geometry))
        .output(PortSpec::new("table", DataKind::Json))
        .output(PortSpec::new("geometry", DataKind::Geometry))
        .output(PortSpec::new("report", DataKind::Report))
        .output(PortSpec::new("surface", DataKind::Geometry))
        .param(ParamSpec::float("attached_deg", "Attached limit", 30.0, 1.0, 90.0).unit("deg").help("Largest mean offset from the flow for an attached or unsteady tuft; a tuft beyond it is separated (or reversed, beyond the reversed limit)"))
        .param(ParamSpec::float("reversed_deg", "Reversed limit", 120.0, 90.0, 180.0).unit("deg").help("Mean offset beyond which a tuft counts as reversed (class 2): it points back upstream"))
        .param(ParamSpec::float("sd_attached_deg", "Steadiness limit", 15.0, 1.0, 90.0).unit("deg").help("Largest angle spread (std-dev) for an attached tuft; a tuft that follows the flow but flutters more is unsteady"))
        .param(ParamSpec::float("sd_separated_deg", "Fluttering limit", 30.0, 5.0, 120.0).unit("deg").help("Angle spread (std-dev) above which a tuft counts as separated"))
        .param(ParamSpec::path("csv_path", "Tuft CSV", &["csv"], false).help("CSV of tuft samples, used when nothing is connected to the tufts port. Columns x, y, z (mm) and angle_deg, optionally id and flow_deg (the local flow direction, default 0). One row per tuft per frame; rows with the same id are the samples of one tuft"))
        .param(ParamSpec::float("reach_mm", "Surface reach", 75.0, 1.0, 1000.0).unit("mm").help("A surface face takes the class of the nearest tuft within this distance; faces farther from every tuft stay unobserved (-1 in tuft_class, tuft_offset_deg, tuft_sd_deg and tuft_dist_mm). Tufts farther than this from the surface are reported as off it"))
        .param(ParamSpec::string("region_attrib", "Region attribute", "part").help("Face attribute of the surface that names its regions (for example the panel id); % separated is reported per region. Without it the whole surface is one region"))
        .help("Classifies tufts as attached, separated, reversed or unsteady from a CSV of tuft positions and measured angles: the offset of the mean angle from the flow direction and the angle spread (std-dev) are compared with the limits. A tuft that follows the flow steadily is attached (class 0); one that is off the flow or fluttering wildly is separated (1); one that points back upstream is reversed (2); one that follows the flow but flutters more than an attached tuft is unsteady (3). The classes are written onto the faces of a connected surface (tuft_class, colour) and the share of separated and of reversed tufts is reported per region. Faces with no tuft in reach get -1 in tuft_class, tuft_offset_deg, tuft_sd_deg and tuft_dist_mm: -1 means no tuft, not a perfect one (test tuft_class, since a real offset can also be -1 degree). Finding tufts in photographs is not implemented: with no CSV, or a photo input connected, the report says so and no numbers are made up.")
        .emulates("tuft flow-visualisation analysis (attached / separated / reversed / unsteady map)")
}

/// Marker and face colour of a class.
fn class_colour(c: TuftClass) -> DVec3 {
    match c {
        TuftClass::Attached => DVec3::new(0.1, 0.8, 0.2),
        TuftClass::Separated => DVec3::new(1.0, 0.8, 0.1),
        TuftClass::Reversed => DVec3::new(0.9, 0.1, 0.1),
        TuftClass::Unsteady => DVec3::new(1.0, 0.5, 0.1),
    }
}

/// The surface with the class of the nearest tuft on every face in reach. Faces out of reach
/// get -1 ("no tuft") in `tuft_class`, `tuft_offset_deg`, `tuft_sd_deg` and `tuft_dist_mm`, never
/// a number that could pass for a perfect tuft. An empty geometry when no surface is connected.
fn surface_output(surface: Option<&Geometry>, verdicts: &[tuft::Verdict], map: &tuft::SurfaceMap) -> Geometry {
    let Some(s) = surface else { return Geometry::new() };
    let mut g = s.clone();
    let n = g.prims.len();
    let (mut class, mut off, mut sd, mut dist) = (vec![-1i64; n], vec![-1.0; n], vec![-1.0; n], vec![-1.0; n]);
    let mut cd = vec![DVec3::splat(0.6); n];
    for (f, hit) in map.face_tuft.iter().enumerate() {
        if let Some((ti, d)) = hit {
            let v = &verdicts[*ti];
            class[f] = v.class as i64;
            off[f] = v.offset_deg;
            sd[f] = v.sd_deg;
            dist[f] = *d;
            cd[f] = class_colour(v.class);
        }
    }
    g.prim_attribs.set("tuft_class", AttribArray::Int(class));
    g.prim_attribs.set("tuft_offset_deg", AttribArray::Float(off));
    g.prim_attribs.set("tuft_sd_deg", AttribArray::Float(sd));
    g.prim_attribs.set("tuft_dist_mm", AttribArray::Float(dist));
    g.prim_attribs.set("Cd", AttribArray::Vec3(cd));
    g
}

fn tuft_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let th = TuftThresholds {
        attached_deg: p.float("attached_deg")?,
        reversed_deg: p.float("reversed_deg")?,
        sd_attached_deg: p.float("sd_attached_deg")?,
        sd_separated_deg: p.float("sd_separated_deg")?,
    };
    let reach = p.float("reach_mm")?;
    let mut rep = Report::new("Tuft analysis");
    let photo_route = inputs.first().is_some_and(Option::is_some);
    // Angles come from the tufts port (a CSV file or a JSON series) or the CSV path parameter.
    let parsed = match inputs.get(2).and_then(|d| d.as_ref()) {
        Some(Data::Json(v)) => Some(tuft::parse_json(v).or_fail(ctx)?),
        Some(Data::File(f)) => Some(tuft::parse_csv(&read_text(ctx, f)?).or_fail(ctx)?),
        Some(other) => return Err(CookError::WrongInput { index: 2, expected: DataKind::Any, found: other.kind() }),
        None => {
            let path = p.str("csv_path")?;
            if path.trim().is_empty() {
                None
            } else {
                Some(tuft::parse_csv(&read_text(ctx, std::path::Path::new(path))?).or_fail(ctx)?)
            }
        }
    };
    match (&parsed, photo_route) {
        (None, _) => {
            rep.note(
                Severity::Warning,
                "white-box placeholder: no tuft CSV is connected and finding tufts in photographs (ridge detection, skeleton, homography to the surface, comparison with the simulated streamline) is not implemented, so no tuft is classified and no numbers are produced. Supply a CSV of tuft positions (x, y, z in mm) and measured angles",
            );
        }
        (Some(_), true) => {
            rep.note(
                Severity::Warning,
                "white-box placeholder: finding tufts in the connected photographs is not implemented, so the photographs are ignored; only the supplied CSV angles are classified",
            );
        }
        (Some(_), false) => {}
    }
    let (tufts, parse_notes) = parsed.map_or((Vec::new(), Vec::new()), |pr| (pr.tufts, pr.notes));
    for n in parse_notes {
        rep.note(Severity::Warning, n);
    }
    if th.sd_attached_deg > th.sd_separated_deg {
        rep.note(
            Severity::Warning,
            "the steadiness limit is above the fluttering limit: a tuft with a spread between them counts as separated, and none is unsteady because of flutter",
        );
    }
    let verdicts: Vec<tuft::Verdict> = tufts.iter().map(|t| tuft::judge(t, &th)).collect();
    let surface = inp::opt_geometry(inputs, 3);
    let map = match &surface {
        Some(s) => tuft::map_to_surface(s, &tufts, reach),
        None => tuft::SurfaceMap { tuft_face: vec![None; tufts.len()], face_tuft: Vec::new() },
    };
    let region_attr = p.str("region_attrib")?.trim().to_string();
    let face_region = surface.as_ref().and_then(|s| if region_attr.is_empty() { None } else { tuft::face_regions(s, &region_attr) });
    let labels: Vec<Option<String>> = (0..tufts.len())
        .map(|i| match &face_region {
            Some(fr) => map.tuft_face[i].and_then(|(f, _)| fr.get(f).cloned()),
            None => Some("all".to_string()),
        })
        .collect();
    let regions = tuft::region_stats(&labels, &verdicts);
    let off_surface = map.tuft_face.iter().filter(|h| h.is_none()).count();
    if !tufts.is_empty() {
        match (&surface, &face_region) {
            (None, _) => {
                rep.note(Severity::Info, "no surface is connected: the classes are not written onto faces and % separated covers all tufts together");
            }
            (Some(_), None) => {
                rep.note(Severity::Info, format!("the surface has no face attribute '{region_attr}': % separated covers all tufts together"));
            }
            (Some(_), Some(_)) => {}
        }
        if surface.is_some() && off_surface > 0 {
            rep.note(
                Severity::Warning,
                format!(
                    "{off_surface} of {} tufts are more than {reach} mm from the surface; check that the tuft positions are in millimetres and in the surface's frame",
                    tufts.len()
                ),
            );
        }
    }

    let mut rows = Vec::new();
    let mut geo = Geometry::new();
    let (mut cd, mut class_attr, mut off_attr, mut sd_attr) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (i, (t, v)) in tufts.iter().zip(&verdicts).enumerate() {
        let face = map.tuft_face[i].map_or(-1.0, |(f, _)| f as f64);
        rows.push(vec![
            i as f64,
            t.pos.x,
            t.pos.y,
            t.pos.z,
            t.flow_deg,
            v.mean_deg,
            v.offset_deg,
            v.sd_deg,
            v.class as u8 as f64,
            t.angles.len() as f64,
            face,
        ]);
        geo.add_point(t.pos);
        cd.push(class_colour(v.class));
        class_attr.push(v.class as i64);
        off_attr.push(v.offset_deg);
        sd_attr.push(v.sd_deg);
    }
    geo.point_attribs.set("Cd", AttribArray::Vec3(cd));
    geo.point_attribs.set("class", AttribArray::Int(class_attr));
    geo.point_attribs.set("offset_deg", AttribArray::Float(off_attr));
    geo.point_attribs.set("sd_deg", AttribArray::Float(sd_attr));

    if !tufts.is_empty() {
        let n = tufts.len() as f64;
        let count = |c: TuftClass| verdicts.iter().filter(|v| v.class == c).count() as f64;
        rep.metric("tufts", Metric::new(n, "-"))
            .metric("attached_fraction", Metric::new(count(TuftClass::Attached) / n, "-"))
            .metric("unsteady_fraction", Metric::new(count(TuftClass::Unsteady) / n, "-"))
            .metric("separated_fraction", Metric::new(count(TuftClass::Separated) / n, "-"))
            .metric("reversed_fraction", Metric::new(count(TuftClass::Reversed) / n, "-"))
            .metric("separated_pct", Metric::new(100.0 * count(TuftClass::Separated) / n, "%"))
            .metric("reversed_pct", Metric::new(100.0 * count(TuftClass::Reversed) / n, "%"));
        if let Some(s) = &surface {
            let covered = map.face_tuft.iter().filter(|h| h.is_some()).count();
            rep.metric("tufts_on_surface", Metric::new(n - off_surface as f64, "-"))
                .metric("tufts_off_surface", Metric::new(off_surface as f64, "-"))
                .metric("faces_in_reach_pct", Metric::new(100.0 * covered as f64 / s.prims.len().max(1) as f64, "%"));
        }
        if face_region.is_some() {
            // region names that differ only in punctuation must not share a metric key
            let names: Vec<String> = regions.iter().map(|r| r.name.clone()).collect();
            let (labels, collisions) = tuft::metric_labels(&names);
            for w in collisions {
                rep.note(Severity::Warning, w);
            }
            for (r, label) in regions.iter().zip(&labels) {
                rep.metric(format!("separated_pct_region_{label}"), Metric::new(r.separated_pct(), "%"))
                    .metric(format!("reversed_pct_region_{label}"), Metric::new(r.reversed_pct(), "%"))
                    .metric(format!("tufts_region_{label}"), Metric::new(r.tufts as f64, "-"));
            }
        }
        rep.note(
            Severity::Info,
            "class codes: 0 attached, 1 separated, 2 reversed, 3 unsteady; separated_pct counts class 1 only and reversed_pct class 2, so a reversed tuft is in reversed_pct, not in separated_pct; thresholds are Vernon-proposed, confirm on real footage",
        );
        if surface.is_some() {
            rep.note(
                Severity::Info,
                format!("faces take the class of the nearest tuft within {reach} mm (-1 in tuft_class, tuft_offset_deg, tuft_sd_deg and tuft_dist_mm = no tuft in reach); no class is extrapolated beyond that"),
            );
        }
    }
    let mut t = table_json(
        &[
            ("tuft", "-"),
            ("x", "mm"),
            ("y", "mm"),
            ("z", "mm"),
            ("flow_deg", "deg"),
            ("mean_deg", "deg"),
            ("offset_deg", "deg"),
            ("sd_deg", "deg"),
            ("class", "code"),
            ("frames", "-"),
            ("face", "index"),
        ],
        rows,
    );
    if let Value::Object(m) = &mut t {
        m.insert("ids".into(), json!(tufts.iter().map(|x| x.id.clone()).collect::<Vec<_>>()));
        m.insert("region".into(), json!(labels));
        m.insert(
            "regions".into(),
            json!(regions
                .iter()
                .map(|r| json!({
                    "name": r.name,
                    "tufts": r.tufts,
                    "attached": r.attached,
                    "unsteady": r.unsteady,
                    "separated": r.separated,
                    "reversed": r.reversed,
                    "separated_pct": r.separated_pct(),
                    "reversed_pct": r.reversed_pct(),
                }))
                .collect::<Vec<_>>()),
        );
    }
    let surf = surface_output(surface.as_deref(), &verdicts, &map);
    Ok(vec![Data::json(t), Data::geometry(geo), Data::report(rep), Data::geometry(surf)])
}

// ------------------------------------------------------------------ gate

fn gate_info() -> NodeTypeInfo {
    node("perf.validation_gate", "Validation Gate")
        .input(PortSpec::optional("sweep", DataKind::Report))
        .input(PortSpec::optional("top_speed", DataKind::Report))
        .input(PortSpec::optional("fit", DataKind::Json))
        .input(PortSpec::optional("cp", DataKind::Report))
        .input(PortSpec::optional("vehicle", DataKind::Json))
        .input(PortSpec::optional("track", DataKind::Json))
        .input(PortSpec::optional("env", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::float("claimed_dcda_m2", "Claimed change in CdA", 0.0, -1.0, 1.0).unit("m2").help("The drag-area change being claimed (positive = more drag)"))
        .param(ParamSpec::float("claimed_dcla_m2", "Claimed change in ClA", 0.0, -3.0, 3.0).unit("m2").help("The downforce-area change being claimed (positive = more downforce)"))
        .param(ParamSpec::float("cfd_cda_m2", "Simulated CdA", 0.0, 0.0, 3.0).unit("m2").help("CdA from the airflow stage; 0 = take it from the connected vehicle"))
        .param(ParamSpec::float("cda_tol_m2", "CdA agreement", 0.05, 0.001, 0.5).unit("m2").help("How far the simulated CdA may sit outside the coastdown 95 percent interval"))
        .param(ParamSpec::float("cp_rms_limit", "Cp RMS limit", 0.1, 0.01, 1.0).help("Warn above this RMS Cp residual"))
        .param(ParamSpec::bool("fail_on_miss", "Fail on a CdA miss", false).help("Turn the simulated-versus-coastdown CdA check from a warning into a failure"))
        .param(ParamSpec::float("ds_max_m", "Largest lap step", 5.0, 0.1, 20.0).unit("m").help("Step of the lap solves used to value the claimed change"))
        .param(rho_param())
        .help("Final check of the performance stage: is the claimed aero change above the coastdown noise floor, does the simulated CdA agree with the road test, do the pressure taps agree with the simulation, and what is the claimed change worth in lap time and top speed.")
        .emulates("validation of a simulated aero change against road tests")
}

fn report_metric(r: &Option<std::sync::Arc<Report>>, name: &str) -> Option<f64> {
    r.as_ref().and_then(|r| r.metrics.get(name)).map(|m| m.value)
}

fn gate_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let (dcda, dcla) = (p.float("claimed_dcda_m2")?, p.float("claimed_dcla_m2")?);
    let sweep = inp::opt_report(inputs, 0);
    let top = inp::opt_report(inputs, 1);
    let cp_rep = inp::opt_report(inputs, 3);
    let veh = crate::util::vehicle_in(ctx, inputs, 4)?;
    let track = match json_in(inputs, 5)? {
        Some(v) => Some(Track::from_json(&v).or_fail(ctx)?),
        None => None,
    };
    let rho = env_in(ctx, inputs, 6, p)?.rho;
    let mut g = GateInput {
        claimed_dcda_m2: dcda,
        claimed_dcla_m2: dcla,
        cda_tol_m2: p.float("cda_tol_m2")?,
        cp_rms_limit: p.float("cp_rms_limit")?,
        fail_on_miss: p.bool("fail_on_miss")?,
        ..GateInput::default()
    };
    // value of the claimed change: exact when the vehicle and track are connected
    if let Some(v) = &veh {
        let mut claimed = v.clone();
        claimed.cda_m2 += dcda;
        claimed.cla_m2 += dcla;
        claimed.validate().or_fail(ctx)?;
        let ts0 = v.top_speed(rho).v_ms;
        let ts1 = claimed.top_speed(rho).v_ms;
        g.top_speed_value_kmh = Some((ts1 - ts0) * 3.6);
        if let Some(t) = &track {
            let o = LapOptions { ds_max_m: p.float("ds_max_m")?, ..LapOptions::default() };
            let a = lapsim::lap_time(v, rho, t, &o).or_fail(ctx)?;
            let b = lapsim::lap_time(&claimed, rho, t, &o).or_fail(ctx)?;
            g.lap_value_s = Some(b - a);
        }
    }
    if g.lap_value_s.is_none() {
        if let (Some(a), Some(b)) = (report_metric(&sweep, "dt_dcda"), report_metric(&sweep, "dt_dcla")) {
            g.lap_value_s = Some(a * dcda + b * dcla);
        }
    }
    if g.top_speed_value_kmh.is_none() {
        if let Some(s) = report_metric(&top, "dvmax_dcda_kmh_per_m2") {
            g.top_speed_value_kmh = Some(s * dcda);
        }
    }
    if let Some(f) = json_in(inputs, 2)? {
        g.min_detectable_dcda = f.get("min_detectable_dcda_m2").and_then(Value::as_f64);
        let cda = f.get("cda_m2").and_then(Value::as_f64);
        let ci = f.get("cda_ci95").and_then(Value::as_array).and_then(|a| Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?)));
        if let (Some(c), Some((lo, hi))) = (cda, ci) {
            g.coast_cda = Some((c, lo, hi));
        }
    }
    let cfd = p.float("cfd_cda_m2")?;
    g.cfd_cda = if cfd > 0.0 { Some(cfd) } else { veh.as_ref().map(Vehicle::cda_eff) };
    g.cp_rms = report_metric(&cp_rep, "rms_cp");
    g.cp_sign_disagree = report_metric(&cp_rep, "sign_disagree");
    let rep = gate::evaluate(&g);
    Ok(vec![Data::report(rep)])
}

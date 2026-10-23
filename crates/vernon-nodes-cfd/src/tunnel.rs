//! Virtual tunnel nodes: domain sizing, ground and wheel treatment, porous
//! radiator zones and the refinement plan.

use serde_json::json;
use vernon_core::{DMat4, DVec3, Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::air::air_in;
use crate::types::{domain_in, ref_in, BcSet, Domain, PorousZone, RefGeom, WheelBc};
use crate::util::{jf, json_in, FnNode};

/// Domain box from the reference geometry (spec 3.2). Ground is z = 0.
pub(crate) fn size_domain(
    rf: &RefGeom,
    blockage_pct: f64,
    up_l: f64,
    down_l: f64,
    aspect: f64,
    half: bool,
    yaw_deg: f64,
) -> Domain {
    let cross = rf.a_front_m2 / (blockage_pct / 100.0);
    let width = (cross * aspect).sqrt();
    let height = cross / width;
    let l = rf.length_m;
    let (y0, y1) = if half { (0.0, width / 2.0) } else { (-width / 2.0, width / 2.0) };
    Domain {
        min_m: [rf.nose_x_m - up_l * l, y0, 0.0],
        max_m: [rf.tail_x_m + down_l * l, y1, height],
        half_model: half,
        yaw_deg,
        blockage_pct,
        cross_section_m2: cross,
        ref_length_m: l,
        up_l,
        down_l,
    }
}

fn box_geometry_mm(min_m: [f64; 3], max_m: [f64; 3]) -> Geometry {
    let (lo, hi) = (DVec3::from_array(min_m) * 1e3, DVec3::from_array(max_m) * 1e3);
    let mut g = Geometry::make_box(hi - lo);
    g.transform(DMat4::from_translation((lo + hi) * 0.5));
    g
}

// ------------------------------------------------------------ tunnel domain

fn domain_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.tunnel_domain", "Virtual Tunnel", Stage::Cfd)
        .input(PortSpec::optional("reference", DataKind::Json))
        .output(PortSpec::new("domain", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .output(PortSpec::new("box", DataKind::Geometry))
        .param(ParamSpec::float("blockage_pct", "Blockage", 1.0, 0.5, 2.0).unit("%").help("Frontal area over tunnel cross-section; a numerical tunnel can be far emptier than a physical one"))
        .param(ParamSpec::float("up_l", "Upstream distance", 3.0, 2.0, 5.0).unit("car lengths"))
        .param(ParamSpec::float("down_l", "Downstream distance", 6.0, 5.0, 10.0).unit("car lengths"))
        .param(ParamSpec::float("aspect", "Width to height", 5.0 / 3.0, 1.0, 3.0))
        .param(ParamSpec::bool("half_model", "Half model (symmetry plane)", true))
        .param(ParamSpec::float("yaw_deg", "Yaw", 0.0, -10.0, 10.0).unit("deg").help("Non-zero yaw needs the full car"))
        .help("Sizes the virtual wind tunnel around the vehicle from a blockage target, with inlet and outlet distances in car lengths, and names the boundary patches.")
        .emulates("automotive wind-tunnel service: virtual tunnel sizing")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

fn domain_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let (rf, wired) = ref_in(ins, 0)?;
    let half = p.bool("half_model")?;
    let yaw = p.float("yaw_deg")?;
    if half && yaw.abs() > 1e-9 {
        return Err(ctx.fail("a half model is only valid at zero yaw; switch to the full car"));
    }
    let blockage = p.float("blockage_pct")?;
    let d = size_domain(&rf, blockage, p.float("up_l")?, p.float("down_l")?, p.float("aspect")?, half, yaw);
    let [lx, ly, lz] = d.size();
    let mut r = Report::new("Virtual tunnel");
    r.metric("blockage_pct", Metric::new(rf.a_front_m2 / d.cross_section_m2 * 100.0, "%").with_limit(blockage + 1e-9, true))
        .metric("cross_section_m2", Metric::new(d.cross_section_m2, "m2"))
        .metric("width_m", Metric::new(if half { 2.0 * ly } else { ly }, "m"))
        .metric("height_m", Metric::new(lz, "m"))
        .metric("length_m", Metric::new(lx, "m"));
    if !wired {
        r.note(Severity::Info, "no vehicle reference wired: sized for a default road-car reference");
    }
    let g = box_geometry_mm(d.min_m, d.max_m);
    Ok(vec![Data::json(d.to_json()), Data::report(r), Data::geometry(g)])
}

// ----------------------------------------------------------- ground + wheels

/// Signed angular speed about `axis` for a tyre rolling toward +x on the ground (spec 3.4).
pub fn wheel_omega(u: f64, r_dyn: f64, axis: DVec3) -> Option<f64> {
    let a = axis.normalize_or_zero();
    if a.y.abs() < 0.5 || r_dyn <= 0.0 {
        return None;
    }
    Some(-u / (a.y * r_dyn))
}

pub(crate) fn gw_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.ground_wheels", "Ground and Wheels", Stage::Cfd)
        .input(PortSpec::optional("domain", DataKind::Json))
        .input(PortSpec::optional("reference", DataKind::Json))
        .input(PortSpec::optional("air", DataKind::Json))
        .output(PortSpec::new("boundary", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::choice("ground_mode", "Ground", "moving", &["moving", "stationary", "slip"]).help("Moving belt at the free-stream speed is needed for splitters, skirts and diffusers"))
        .param(ParamSpec::choice("wheel_mode", "Wheels", "rotating", &["rotating", "mrf", "static"]).help("Rotating wall velocity, a rotating reference-frame zone, or fixed wheels"))
        .param(ParamSpec::float("u_ms", "Speed", 30.0, 10.0, 100.0).unit("m/s"))
        .param(ParamSpec::float("r_dyn_mm", "Rolling radius (0 = from vehicle)", 0.0, 0.0, 600.0).unit("mm"))
        .param(ParamSpec::vec3("axis", "Wheel axis", DVec3::Y))
        .param(ParamSpec::float("wheel_width_mm", "Wheel width (rotating-frame zone)", 250.0, 100.0, 500.0).unit("mm"))
        .param(ParamSpec::bool("belt_start", "Start the moving belt downstream of the inlet", false))
        .param(ParamSpec::float("belt_start_l", "Belt start from nose", -0.5, -3.0, 0.0).unit("car lengths"))
        .help("Boundary treatment for road and wheels: moving ground at the free-stream speed, wheels spinning at the rolling rate. Switching either off marks the run as not representative.")
        .emulates("CFD GUI front end: moving ground and rotating wheel boundary set")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

pub(crate) fn gw_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let (rf, _) = ref_in(ins, 1)?;
    let _domain = domain_in(ins, 0, &rf)?;
    let air = air_in(ins, 2)?;
    let u = air.u.unwrap_or(p.float("u_ms")?);
    let axis = p.vec3("axis")?;
    let r_dyn = match p.float("r_dyn_mm")? {
        v if v > 0.0 => v * 1e-3,
        _ => rf.wheel_radius_m,
    };
    let omega = wheel_omega(u, r_dyn, axis).ok_or_else(|| ctx.fail("wheel axis must be along y and the rolling radius positive"))?;
    let ground = p.str("ground_mode")?.to_string();
    let wheel = p.str("wheel_mode")?.to_string();
    let wheels: Vec<WheelBc> = if rf.wheel_centres_m.is_empty() {
        vec![]
    } else {
        rf.wheel_centres_m.iter().map(|(n, c)| WheelBc { name: n.clone(), centre_m: *c }).collect()
    };
    let belt = if p.bool("belt_start")? { Some(rf.nose_x_m + p.float("belt_start_l")? * rf.length_m) } else { None };
    let bc = BcSet {
        ground_mode: ground.clone(),
        wheel_mode: wheel.clone(),
        u_ms: u,
        omega_rad_s: omega,
        r_dyn_m: r_dyn,
        axis: axis.normalize().to_array(),
        wheel_width_m: p.float("wheel_width_mm")? * 1e-3,
        belt_start_x_m: belt,
        wheels,
    };
    let representative = ground == "moving" && wheel != "static";
    let mut r = Report::new("Ground and wheels");
    r.metric("omega_rad_s", Metric::new(omega.abs(), "rad/s"))
        .metric("u_ms", Metric::new(u, "m/s"))
        .metric("r_dyn_m", Metric::new(r_dyn, "m"))
        .metric("representative", Metric::new(if representative { 1.0 } else { 0.0 }, "-").with_limit(1.0, false));
    if !representative {
        r.note(Severity::Warning, "not representative of a moving road car: ground or wheel motion is switched off");
    }
    if wheel != "static" && ground != "moving" {
        r.note(Severity::Warning, "spinning wheels on a stationary ground is not a physical combination");
    }
    if wheel == "mrf" || wheel == "rotating" {
        r.note(Severity::Info, "lift from wall-velocity or rotating-frame wheels is moderate confidence; a sliding mesh is the only reliable option");
    }
    if bc.belt_start_x_m.is_some() {
        r.note(Severity::Info, "belt start is recorded in the boundary set; the upstream slip section needs a patch split that the case writer does not create yet");
    }
    Ok(vec![Data::json(serde_json::to_value(&bc).unwrap_or_default()), Data::report(r)])
}

// ------------------------------------------------------------- porous core

/// Least-squares fit of `dp = a v + b v^2` through the origin.
pub fn fit_dp_curve(v: &[f64], dp: &[f64]) -> Option<(f64, f64)> {
    if v.len() != dp.len() || v.len() < 2 {
        return None;
    }
    let (mut s2, mut s3, mut s4, mut r1, mut r2) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (&x, &y) in v.iter().zip(dp) {
        s2 += x * x;
        s3 += x * x * x;
        s4 += x * x * x * x;
        r1 += x * y;
        r2 += x * x * y;
    }
    let det = s2 * s4 - s3 * s3;
    if det.abs() < 1e-12 * (s2 * s4).abs().max(1e-300) {
        return None;
    }
    Some(((r1 * s4 - r2 * s3) / det, (s2 * r2 - s3 * r1) / det))
}

/// Darcy-Forchheimer coefficients (spec 3.6): returns (d_flow [1/m2], f_flow [1/m]).
pub fn darcy_forchheimer(a: f64, b: f64, thickness: f64, mu: f64, rho: f64) -> (f64, f64) {
    (a / (mu * thickness), 2.0 * b / (rho * thickness))
}

fn porous_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.porous_core", "Porous Core", Stage::Cfd)
        .input(PortSpec::optional("core", DataKind::Geometry))
        .input(PortSpec::optional("curve", DataKind::Json))
        .input(PortSpec::optional("zones_in", DataKind::Json))
        .input(PortSpec::optional("air", DataKind::Json))
        .output(PortSpec::new("zones", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::string("name", "Zone name", "radiator"))
        .param(ParamSpec::float("a", "Linear loss a (dp = a v + b v^2)", 20.0, 0.0, 1e5).unit("Pa s/m").help("Ignored when a pressure-drop curve is wired"))
        .param(ParamSpec::float("b", "Quadratic loss b", 15.0, 0.0, 1e5).unit("Pa s2/m2"))
        .param(ParamSpec::float("thickness_m", "Core thickness", 0.05, 0.005, 0.5).unit("m"))
        .param(ParamSpec::float("mu", "Dynamic viscosity", 1.8e-5, 1e-5, 3e-5).unit("Pa s"))
        .param(ParamSpec::float("rho", "Density", 1.1, 0.8, 1.4).unit("kg/m3"))
        .param(ParamSpec::vec3("flow_dir", "Through-flow direction", DVec3::X))
        .param(ParamSpec::float("k_lat", "Lateral resistance factor", 1000.0, 1.0, 1e6).help("Forces the air through the core instead of sideways"))
        .help("Radiator core as a Darcy-Forchheimer porous zone built from a pressure-drop curve. Zones accumulate along the chain so several heat exchangers can share one case.")
        .emulates("CFD GUI front end: porous zone from a loss curve")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

fn porous_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let air = air_in(ins, 3)?;
    let (mu, rho) = (air.mu.unwrap_or(p.float("mu")?), air.rho.unwrap_or(p.float("rho")?));
    let t = p.float("thickness_m")?;
    let mut r = Report::new("Porous core");
    let (a, b) = match json_in(ins, 1)? {
        Some(c) => {
            let col = |k: &str| -> Vec<f64> {
                c.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default()
            };
            let (v, dp) = (col("v"), col("dp"));
            let (a, b) = fit_dp_curve(&v, &dp).ok_or_else(|| ctx.fail("pressure-drop curve needs at least two distinct points under keys 'v' (m/s) and 'dp' (Pa)"))?;
            if a < 0.0 || b < 0.0 {
                r.note(Severity::Warning, "fitted loss coefficients are negative; check the curve");
            }
            (a, b)
        }
        None => (p.float("a")?, p.float("b")?),
    };
    let (d_flow, f_flow) = darcy_forchheimer(a, b, t, mu, rho);
    let k = p.float("k_lat")?;
    let flow = p.vec3("flow_dir")?.normalize_or_zero();
    if flow == DVec3::ZERO {
        return Err(ctx.fail("flow direction must be non-zero"));
    }
    let bbox = inputs::opt_geometry(ins, 0).filter(|g| !g.points.is_empty()).map(|g| {
        let b = g.bounds();
        [(b.min * 1e-3).to_array(), (b.max * 1e-3).to_array()]
    });
    if bbox.is_none() {
        r.note(Severity::Warning, "no core surface wired: the zone has no volume and the case writer will skip it");
    }
    let zone = PorousZone {
        name: p.str("name")?.to_string(),
        bbox_m: bbox,
        flow_dir: flow.to_array(),
        d: [d_flow, d_flow * k, d_flow * k],
        f: [f_flow, f_flow * k, f_flow * k],
    };
    let mut zones: Vec<serde_json::Value> = json_in(ins, 2)?
        .and_then(|v| v.get("zones").and_then(|z| z.as_array().cloned()))
        .unwrap_or_default();
    zones.retain(|z| z.get("name").and_then(|n| n.as_str()) != Some(zone.name.as_str()));
    zones.push(serde_json::to_value(&zone).unwrap_or_default());
    r.metric("d_flow_per_m2", Metric::new(d_flow, "1/m2"))
        .metric("f_flow_per_m", Metric::new(f_flow, "1/m"))
        .metric("d_lateral_per_m2", Metric::new(d_flow * k, "1/m2"))
        .metric("zone_count", Metric::new(zones.len() as f64, "count"));
    Ok(vec![Data::json(json!({ "zones": zones })), Data::report(r)])
}

/// A one-zone porous hand-off for the case-writer tests: a 100 x 400 x 300 mm core.
#[cfg(test)]
pub(crate) fn porous_cook_for_case_tests() -> serde_json::Value {
    let mut core = Geometry::make_box(DVec3::new(100.0, 400.0, 300.0));
    core.transform(DMat4::from_translation(DVec3::new(250.0, 300.0, 350.0)));
    let ctx = crate::util::testutil::ctx("porous-helper");
    let out = porous_cook(&ctx, &[Some(Data::geometry(core))], &Params::from_specs(&porous_info().params)).unwrap();
    out[0].as_json().unwrap().as_ref().clone()
}

// -------------------------------------------------------- refinement plan

/// Flat-plate skin friction `Cf = 0.026 Re^(-1/7)` and friction velocity.
pub fn flat_plate_u_tau(u: f64, l: f64, nu: f64) -> (f64, f64, f64) {
    let re = u * l / nu;
    let cf = 0.026 * re.powf(-1.0 / 7.0);
    (re, cf, u * (cf / 2.0).sqrt())
}

pub(crate) fn plan_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.refinement_plan", "Refinement Plan", Stage::Cfd)
        .input(PortSpec::optional("reference", DataKind::Json))
        .input(PortSpec::optional("domain", DataKind::Json))
        .input(PortSpec::optional("kit", DataKind::Geometry))
        .input(PortSpec::optional("air", DataKind::Json))
        .output(PortSpec::new("plan", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::float("dx0_m", "Base cell size", 0.32, 0.1, 1.0).unit("m").help("A level-k cell is this size divided by 2^k"))
        .param(ParamSpec::int("body_min", "Body level (min)", 4, 3, 7))
        .param(ParamSpec::int("body_max", "Body level (max)", 5, 3, 7))
        .param(ParamSpec::int("wheel_level", "Wheel level", 5, 3, 7))
        .param(ParamSpec::int("underbody_level", "Underbody level", 6, 3, 8))
        .param(ParamSpec::float("underbody_dist_m", "Underbody distance band", 0.1, 0.02, 0.5).unit("m"))
        .param(ParamSpec::float("underbody_height_m", "Underbody box height", 0.3, 0.1, 1.0).unit("m").help("Used when the vehicle has no separate underbody surface"))
        .param(ParamSpec::int("kit_level", "Kit box level (0 = body max + 1)", 0, 0, 8))
        .param(ParamSpec::float("kit_margin_m", "Kit box margin", 0.15, 0.0, 1.0).unit("m"))
        .param(ParamSpec::float("near_wake_l", "Near-wake length", 1.0, 0.5, 3.0).unit("car lengths"))
        .param(ParamSpec::int("near_wake_level", "Near-wake level", 4, 2, 6))
        .param(ParamSpec::float("far_wake_l", "Far-wake length", 2.0, 1.0, 5.0).unit("car lengths"))
        .param(ParamSpec::int("far_wake_level", "Far-wake level", 3, 2, 6))
        .param(ParamSpec::float("u_ms", "Speed", 30.0, 10.0, 100.0).unit("m/s"))
        .param(ParamSpec::float("nu", "Kinematic viscosity", 1.5e-5, 1.4e-5, 1.9e-5).unit("m2/s"))
        .param(ParamSpec::float("yplus_target", "Target y+", 30.0, 1.0, 300.0))
        .help("Refinement levels for the surfaces, boxes around the part under test, an underbody band and wake boxes, plus the first-layer height needed for a target y+ from a flat-plate friction estimate.")
        .emulates("CFD GUI front end: mesh refinement regions")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

pub(crate) fn plan_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let (rf, _) = ref_in(ins, 0)?;
    let dom = domain_in(ins, 1, &rf)?;
    let air = air_in(ins, 3)?;
    let u = air.u.unwrap_or(p.float("u_ms")?);
    let nu = air.nu.unwrap_or(p.float("nu")?);
    let dx0 = p.float("dx0_m")?;
    let (bmin, bmax) = (p.int("body_min")?, p.int("body_max")?);
    if bmin > bmax {
        return Err(ctx.fail("body minimum level exceeds the maximum"));
    }
    let wheel = p.int("wheel_level")?;
    let ub = p.int("underbody_level")?;
    let kit_level = match p.int("kit_level")? {
        0 => bmax + 1,
        k => k,
    };
    let l = rf.length_m;
    let (re, cf, u_tau) = flat_plate_u_tau(u, l, nu);
    let y1 = |yp: f64| yp * nu / u_tau;
    let cell = |lv: i64| dx0 / 2f64.powi(lv as i32);

    let clip = |min: [f64; 3], max: [f64; 3]| {
        let mut a = min;
        let mut b = max;
        for i in 0..3 {
            a[i] = a[i].max(dom.min_m[i]);
            b[i] = b[i].min(dom.max_m[i]);
        }
        (a, b)
    };
    let mut boxes = Vec::new();
    let mut r = Report::new("Refinement plan");
    let kit_box = inputs::opt_geometry(ins, 2)
        .filter(|g| !g.points.is_empty())
        .map(|g| {
            let b = g.bounds();
            [(b.min * 1e-3).to_array(), (b.max * 1e-3).to_array()]
        })
        .or(rf.kit_bbox_m);
    match kit_box {
        Some([lo, hi]) => {
            let m = p.float("kit_margin_m")?;
            let (a, b) = clip([lo[0] - m, lo[1] - m, lo[2] - m], [hi[0] + m, hi[1] + m, hi[2] + m]);
            boxes.push(json!({"name": "kitBox", "min": a, "max": b, "level": kit_level}));
        }
        None => {
            r.note(Severity::Info, "no kit surface: no kit refinement box (refinement boxes go around the part under test)");
        }
    }
    let half_w = rf.width_m / 2.0 + 0.2;
    let y_lo = if dom.half_model { 0.0 } else { -half_w };
    let (a, b) = clip(
        [rf.tail_x_m - 0.5, y_lo, 0.0],
        [rf.tail_x_m + p.float("near_wake_l")? * l, half_w, 1.3 * rf.height_m],
    );
    boxes.push(json!({"name": "nearWake", "min": a, "max": b, "level": p.int("near_wake_level")?}));
    let (a, b) = clip(
        [rf.tail_x_m - 0.5, y_lo, 0.0],
        [rf.tail_x_m + p.float("far_wake_l")? * l, half_w + 0.5, 1.6 * rf.height_m],
    );
    boxes.push(json!({"name": "farWake", "min": a, "max": b, "level": p.int("far_wake_level")?}));
    let underbody_surface = rf.patches.iter().any(|n| n == "underbody");
    if !underbody_surface {
        let (a, b) = clip(
            [rf.nose_x_m, y_lo, 0.0],
            [rf.tail_x_m, rf.width_m / 2.0, p.float("underbody_height_m")?],
        );
        boxes.push(json!({"name": "underbodyBox", "min": a, "max": b, "level": ub}));
    }
    let plan = json!({
        "dx0_m": dx0,
        "levels": {"body": [bmin, bmax], "wheel": wheel, "underbody": ub, "kit": kit_level,
                   "near_wake": p.int("near_wake_level")?, "far_wake": p.int("far_wake_level")?},
        "boxes": boxes,
        "underbody_mode": if underbody_surface {"distance"} else {"box"},
        "underbody_dist_m": p.float("underbody_dist_m")?,
        "u_ms": u, "nu": nu, "re_l": re, "cf": cf, "u_tau": u_tau,
        "yplus_target": p.float("yplus_target")?,
        "first_layer_m_target": y1(p.float("yplus_target")?),
        "finest_cell_m": cell(bmax.max(wheel).max(ub).max(kit_level)),
        "patches": rf.patches,
        "half_model": dom.half_model,
        "reference": rf.to_json(),
    });
    r.metric("re_l", Metric::new(re, "-"))
        .metric("cf", Metric::new(cf, "-"))
        .metric("u_tau", Metric::new(u_tau, "m/s"))
        .metric("y1_mm_yplus1", Metric::new(y1(1.0) * 1e3, "mm"))
        .metric("y1_mm_yplus30", Metric::new(y1(30.0) * 1e3, "mm"))
        .metric("y1_mm_yplus100", Metric::new(y1(100.0) * 1e3, "mm"))
        .metric("y1_mm_target", Metric::new(y1(p.float("yplus_target")?) * 1e3, "mm"))
        .metric("body_cell_mm", Metric::new(cell(bmax) * 1e3, "mm"))
        .metric("finest_cell_mm", Metric::new(jf(&plan, "finest_cell_m").unwrap_or(0.0) * 1e3, "mm"));
    if kit_level < bmax + 1 && kit_box.is_some() {
        r.note(Severity::Warning, "kit box is not finer than the body: the part under test is under-resolved");
    }
    Ok(vec![Data::json(plan), Data::report(r)])
}

pub(crate) fn register(reg: &mut Registry) {
    reg.add(FnNode::new(domain_info, domain_cook));
    reg.add(FnNode::new(gw_info, gw_cook));
    reg.add(FnNode::new(porous_info, porous_cook));
    reg.add(FnNode::new(plan_info, plan_cook));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::ctx;
    use vernon_graph::ParamValue;

    fn run(info: fn() -> NodeTypeInfo, cook: crate::util::CookFn, ins: &[Option<Data>], set: &[(&str, ParamValue)]) -> Vec<Data> {
        let mut p = Params::from_specs(&info().params);
        for (k, v) in set {
            p.set(k, v.clone());
        }
        cook(&ctx("tunnel"), ins, &p).unwrap()
    }

    fn ref_json(a: f64, l: f64) -> Data {
        let mut rf = RefGeom::reference_car();
        rf.a_front_m2 = a;
        rf.length_m = l;
        rf.tail_x_m = l;
        Data::json(rf.to_json())
    }

    #[test]
    fn g6_domain_sizes() {
        let rf = RefGeom { a_front_m2: 2.16, ..RefGeom::reference_car() };
        let d = size_domain(&rf, 1.0, 3.0, 6.0, 5.0 / 3.0, true, 0.0);
        assert!((d.cross_section_m2 - 216.0).abs() < 1e-9);
        let [lx, ly, lz] = d.size();
        assert!((2.0 * ly - 18.97).abs() < 0.01, "{ly}");
        assert!((ly - 9.49).abs() < 0.01);
        assert!((lz - 11.38).abs() < 0.01, "{lz}");
        assert!((lx - 46.0).abs() < 1e-9, "{lx}");
        assert_eq!(d.patch_names().len(), 6);
    }

    #[test]
    fn domain_node_and_yaw_lock() {
        let out = run(domain_info, domain_cook, &[Some(ref_json(2.16, 4.6))], &[]);
        let d = Domain::from_json(out[0].as_json().unwrap()).unwrap();
        assert_eq!(d.min_m[1], 0.0);
        assert!(out[1].as_report().unwrap().passes());
        assert_eq!(out[2].as_geometry().unwrap().point_count(), 8);
        let mut p = Params::from_specs(&domain_info().params);
        p.set("yaw_deg", ParamValue::Float(5.0));
        assert!(domain_cook(&ctx("yaw"), &[], &p).is_err());
        p.set("half_model", ParamValue::Bool(false));
        let full = domain_cook(&ctx("yaw"), &[], &p).unwrap();
        let d = Domain::from_json(full[0].as_json().unwrap()).unwrap();
        assert!(d.min_m[1] < 0.0 && d.patch_names().len() == 5);
    }

    #[test]
    fn g10_wheel_omega() {
        let w = wheel_omega(16.0, 0.3137, DVec3::Y).unwrap();
        assert!((w.abs() - 51.0).abs() < 0.1, "{w}");
        assert!(w < 0.0);
        assert!(wheel_omega(16.0, 0.3137, -DVec3::Y).unwrap() > 0.0);
        assert!(wheel_omega(16.0, 0.3137, DVec3::Z).is_none());
        // tyre bottom moves toward +x: v = omega (axis x r), r = -R z
        let v = DVec3::Y.cross(DVec3::new(0.0, 0.0, -0.3137)) * w;
        assert!((v.x - 16.0).abs() < 1e-9);
    }

    #[test]
    fn ground_wheels_flags_unrepresentative() {
        let out = run(gw_info, gw_cook, &[], &[("u_ms", ParamValue::Float(16.0)), ("r_dyn_mm", ParamValue::Float(313.7))]);
        let bc = BcSet::from_json(out[0].as_json().unwrap()).unwrap();
        assert_eq!(bc.wheels.len(), 4);
        assert!((out[1].as_report().unwrap().metrics["omega_rad_s"].value - 51.0).abs() < 0.1);
        assert!(out[1].as_report().unwrap().passes());
        let out = run(gw_info, gw_cook, &[], &[("ground_mode", ParamValue::Str("stationary".into()))]);
        assert!(!out[1].as_report().unwrap().passes());
    }

    #[test]
    fn g11_porous_coefficients() {
        let (d, f) = darcy_forchheimer(20.0, 15.0, 0.05, 1.8e-5, 1.1);
        assert!((d / 2.222e7 - 1.0).abs() < 1e-3, "{d}");
        assert!((f / 545.5 - 1.0).abs() < 1e-3, "{f}");
        let out = run(porous_info, porous_cook, &[], &[]);
        let z = out[0].as_json().unwrap()["zones"][0].clone();
        let z: PorousZone = serde_json::from_value(z).unwrap();
        assert!((z.d[1] / z.d[0] - 1000.0).abs() < 1e-9);
        assert!(z.bbox_m.is_none());
    }

    #[test]
    fn porous_curve_fit_and_accumulation() {
        let v: Vec<f64> = (1..8).map(|i| i as f64).collect();
        let dp: Vec<f64> = v.iter().map(|x| 20.0 * x + 15.0 * x * x).collect();
        let (a, b) = fit_dp_curve(&v, &dp).unwrap();
        assert!((a - 20.0).abs() < 1e-6 && (b - 15.0).abs() < 1e-6);
        assert!(fit_dp_curve(&[1.0], &[2.0]).is_none());
        let curve = Data::json(json!({"v": v, "dp": dp}));
        let first = run(porous_info, porous_cook, &[None, Some(curve.clone()), None, None], &[]);
        let second = run(porous_info, porous_cook, &[None, Some(curve), Some(first[0].clone()), None], &[("name", ParamValue::Str("intercooler".into()))]);
        assert_eq!(second[0].as_json().unwrap()["zones"].as_array().unwrap().len(), 2);
        let bad = Data::json(json!({"v": [1.0], "dp": [2.0]}));
        let mut p = Params::from_specs(&porous_info().params);
        p.set("a", ParamValue::Float(1.0));
        assert!(porous_cook(&ctx("porous"), &[None, Some(bad)], &p).is_err());
    }

    #[test]
    fn g9_wall_layer_estimates() {
        let (re, cf, ut) = flat_plate_u_tau(30.0, 4.6, 1.5e-5);
        assert!((re / 9.2e6 - 1.0).abs() < 1e-3);
        assert!((cf / 0.00263 - 1.0).abs() < 0.01, "{cf}");
        assert!((ut / 1.088 - 1.0).abs() < 0.01, "{ut}");
        let out = run(plan_info, plan_cook, &[Some(ref_json(2.16, 4.6))], &[]);
        let m = &out[1].as_report().unwrap().metrics;
        assert!((m["y1_mm_yplus1"].value / 0.0138 - 1.0).abs() < 0.01);
        assert!((m["y1_mm_yplus30"].value / 0.414 - 1.0).abs() < 0.01);
        assert!((m["y1_mm_yplus100"].value / 1.38 - 1.0).abs() < 0.01);
    }

    #[test]
    fn plan_boxes_levels_and_kit() {
        let kit = crate::util::testutil::box_between(DVec3::new(100.0, -300.0, 100.0), DVec3::new(900.0, 300.0, 300.0));
        let out = run(plan_info, plan_cook, &[Some(ref_json(2.16, 4.6)), None, Some(Data::geometry(kit)), None], &[]);
        let plan = out[0].as_json().unwrap();
        assert_eq!(plan["levels"]["kit"], 6);
        let boxes = plan["boxes"].as_array().unwrap();
        let names: Vec<&str> = boxes.iter().filter_map(|b| b["name"].as_str()).collect();
        for n in ["kitBox", "nearWake", "farWake", "underbodyBox"] {
            assert!(names.contains(&n), "{n}");
        }
        let kb = &boxes[0];
        assert!((kb["min"][0].as_f64().unwrap() - (0.1 - 0.15)).abs() < 1e-9);
        // the body cell at level 5 with dx0 0.32 is 10 mm
        assert!((out[1].as_report().unwrap().metrics["body_cell_mm"].value - 10.0).abs() < 1e-9);
        // body levels must be ordered
        let mut p = Params::from_specs(&plan_info().params);
        p.set("body_min", ParamValue::Int(6));
        p.set("body_max", ParamValue::Int(4));
        assert!(plan_cook(&ctx("plan"), &[], &p).is_err());
    }
}

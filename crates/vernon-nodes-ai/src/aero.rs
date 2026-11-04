//! `ai.aero_preview`: DoMINO DrivAerML surface and volume surrogates.
//!
//! The checkpoint takes a car surface in metres (flow along +x, z up) and a
//! fixed operating point (30 m/s, 1.205 kg/m3, no yaw). It returns, after
//! inverting its min-max scaling, dimensionless model-unit fields. Vernon does
//! not convert them to pascals or newtons and does not integrate a drag force:
//! the unit mapping is not verified. Every output carries [`crate::AERO_LABEL`].

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use glam::DVec3;
use serde_json::{json, Value};
use vernon_core::report::{Metric, Severity};
use vernon_core::{AttribArray, Geometry, Report};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::bridge::{self, check_resources, with_margin, BridgeError, Install};
use crate::prep::{self, Domain, Placement, PrepOptions, Prepared};
use crate::{colormap, fnv1a, quantile, read_f32, AERO_LABEL, CFD_VALIDATION};

pub const UNIT: &str = "model-units";
pub const SURFACE_CHANNELS: [&str; 4] = ["pressure", "shear_x", "shear_y", "shear_z"];
pub const VOLUME_CHANNELS: [&str; 5] = ["Ux", "Uy", "Uz", "pressure", "nut"];
/// Measured on an RTX 4090 Laptop (16 GB): 4,992 triangles peaked at 1.29 GB and 19,800 at 6.35 GB, so
/// peak GB {2248} 0.95 + 1.38e-8 * triangles^2 (two-point fit; 33,540 triangles then predicts 16.5 GB and did fail).
/// The volume path peaked at 3.32 GB in both runs. Process RAM peaked at 2.06 GB.
pub const VOLUME_PEAK_VRAM_GB: f64 = 3.32;
pub const PEAK_RSS_GB: f64 = 2.1;

/// Predicted peak GPU memory (GB) of the surface path for `triangles` faces.
pub fn surface_peak_vram_gb(triangles: usize) -> f64 {
    0.95 + 1.38e-8 * (triangles as f64).powi(2)
}

pub fn register(reg: &mut Registry) {
    reg.add(AeroPreviewNode);
}

/// Statistics of a surface prediction (model units).
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceStats {
    pub triangles: usize,
    pub area_m2: f64,
    pub p_mean: f64,
    pub p_min: f64,
    pub p_max: f64,
    pub tau_mean: f64,
    pub out_of_range_fraction: f64,
}

/// Statistics of a volume slice prediction (model units).
#[derive(Debug, Clone, PartialEq)]
pub struct SliceStats {
    pub points: usize,
    pub masked: usize,
    pub speed_mean: f64,
    pub speed_min: f64,
    pub speed_max: f64,
    pub p_mean: f64,
    pub out_of_range_fraction: f64,
}

/// Fraction of rows of `fields` (row-major, `cols` per row) with any channel outside [min, max].
pub fn out_of_range_fraction(fields: &[f32], cols: usize, min: &[f64], max: &[f64]) -> f64 {
    if cols == 0 || fields.is_empty() || min.len() < cols || max.len() < cols {
        return 0.0;
    }
    let rows = fields.len() / cols;
    let bad = fields
        .chunks_exact(cols)
        .filter(|r| r.iter().enumerate().any(|(c, v)| (*v as f64) < min[c] - 1e-6 || (*v as f64) > max[c] + 1e-6))
        .count();
    bad as f64 / rows.max(1) as f64
}

fn ranges(run: &Value) -> (Vec<f64>, Vec<f64>) {
    let get = |k: &str| {
        run.get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_f64).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    (get("training_min"), get("training_max"))
}

fn tri_area(t: &[DVec3; 3]) -> f64 {
    (t[1] - t[0]).cross(t[2] - t[0]).length() * 0.5
}

/// Paint the prediction on a copy of `geo`. `fields` is `triangles x 4` in the order of `prep.tris`.
/// `color_by` is `pressure` or `wall_shear`.
pub fn build_surface(
    geo: &Geometry,
    prep: &Prepared,
    fields: &[f32],
    color_by: &str,
    range: Option<(Vec<f64>, Vec<f64>)>,
) -> Result<(Geometry, SurfaceStats, (f64, f64)), String> {
    let nt = prep.tris.len();
    if fields.len() != nt * 4 {
        return Err(format!("surface prediction has {} values for {} triangles (expected {})", fields.len(), nt, nt * 4));
    }
    if fields.iter().any(|v| !v.is_finite()) {
        return Err("surface prediction contains non-finite values".into());
    }
    let np = geo.prims.len();
    let mut wsum = vec![0.0f64; np];
    let mut p = vec![0.0f64; np];
    let mut tau = vec![DVec3::ZERO; np];
    let (mut area_tot, mut p_acc, mut tau_acc) = (0.0, 0.0, 0.0);
    let (mut pmin, mut pmax) = (f64::INFINITY, f64::NEG_INFINITY);
    for (i, t) in prep.tris.iter().enumerate() {
        let a = tri_area(t).max(1e-18);
        let f = &fields[i * 4..i * 4 + 4];
        let tv = DVec3::new(f[1] as f64, f[2] as f64, f[3] as f64);
        let pr = prep.tri_prim[i];
        wsum[pr] += a;
        p[pr] += a * f[0] as f64;
        tau[pr] += a * tv;
        area_tot += a;
        p_acc += a * f[0] as f64;
        tau_acc += a * tv.length();
        pmin = pmin.min(f[0] as f64);
        pmax = pmax.max(f[0] as f64);
    }
    for i in 0..np {
        if wsum[i] > 0.0 {
            p[i] /= wsum[i];
            tau[i] /= wsum[i];
        }
    }
    let tau_mag: Vec<f64> = tau.iter().map(|v| v.length()).collect();
    let values: &Vec<f64> = if color_by == "wall_shear" { &tau_mag } else { &p };
    let used: Vec<f64> = (0..np).filter(|&i| wsum[i] > 0.0).map(|i| values[i]).collect();
    let (lo, hi) = (quantile(&used, 0.02), quantile(&used, 0.98));
    let span = if (hi - lo).abs() > 1e-12 { hi - lo } else { 1.0 };
    let prim_col: Vec<DVec3> = values.iter().map(|v| colormap((v - lo) / span)).collect();
    let mut out = geo.clone();
    let mut pc = vec![DVec3::ZERO; geo.points.len()];
    let mut pn = vec![0u32; geo.points.len()];
    for (pi, prim) in geo.prims.iter().enumerate() {
        if wsum[pi] <= 0.0 {
            continue;
        }
        for &v in &prim.vertices {
            pc[v as usize] += prim_col[pi];
            pn[v as usize] += 1;
        }
    }
    for i in 0..pc.len() {
        if pn[i] > 0 {
            pc[i] /= pn[i] as f64;
        } else {
            pc[i] = DVec3::splat(0.5);
        }
    }
    out.prim_attribs.set("ai_p", AttribArray::Float(p));
    out.prim_attribs.set("ai_tau", AttribArray::Vec3(tau));
    out.prim_attribs.set("ai_tau_mag", AttribArray::Float(tau_mag));
    out.point_attribs.set("Cd", AttribArray::Vec3(pc));
    out.detail_attribs.set("ai_label", AttribArray::Str(vec![AERO_LABEL.to_string()]));
    out.detail_attribs.set("ai_units", AttribArray::Str(vec![UNIT.to_string()]));
    let oor = range.map(|(mn, mx)| out_of_range_fraction(fields, 4, &mn, &mx)).unwrap_or(0.0);
    let stats = SurfaceStats {
        triangles: nt,
        area_m2: area_tot,
        p_mean: p_acc / area_tot.max(1e-18),
        p_min: pmin,
        p_max: pmax,
        tau_mean: tau_acc / area_tot.max(1e-18),
        out_of_range_fraction: oor,
    };
    Ok((out, stats, (lo, hi)))
}

/// Point cloud of the slice prediction in scene millimetres. `pts_m` are the model-frame query points.
pub fn build_slice(
    prep: &Prepared,
    pts_m: &[DVec3],
    fields: &[f32],
    masked: usize,
    range: Option<(Vec<f64>, Vec<f64>)>,
) -> Result<(Geometry, SliceStats, (f64, f64)), String> {
    let n = pts_m.len();
    if fields.len() != n * 5 {
        return Err(format!("volume prediction has {} values for {} points (expected {})", fields.len(), n, n * 5));
    }
    if fields.iter().any(|v| !v.is_finite()) {
        return Err("volume prediction contains non-finite values".into());
    }
    let mut g = Geometry::new();
    let (mut u, mut p, mut nut, mut speed) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (i, q) in pts_m.iter().enumerate() {
        g.add_point(prep.to_scene_mm(*q));
        let f = &fields[i * 5..i * 5 + 5];
        let uv = DVec3::new(f[0] as f64, f[1] as f64, f[2] as f64);
        u.push(uv);
        speed.push(uv.length());
        p.push(f[3] as f64);
        nut.push(f[4] as f64);
    }
    let (lo, hi) = (quantile(&speed, 0.02), quantile(&speed, 0.98));
    let span = if (hi - lo).abs() > 1e-12 { hi - lo } else { 1.0 };
    let cd: Vec<DVec3> = speed.iter().map(|s| colormap((s - lo) / span)).collect();
    let stats = SliceStats {
        points: n,
        masked,
        speed_mean: speed.iter().sum::<f64>() / n.max(1) as f64,
        speed_min: speed.iter().copied().fold(f64::INFINITY, f64::min),
        speed_max: speed.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        p_mean: p.iter().sum::<f64>() / n.max(1) as f64,
        out_of_range_fraction: range.map(|(mn, mx)| out_of_range_fraction(fields, 5, &mn, &mx)).unwrap_or(0.0),
    };
    g.point_attribs.set("ai_U", AttribArray::Vec3(u));
    g.point_attribs.set("ai_speed", AttribArray::Float(speed));
    g.point_attribs.set("ai_p", AttribArray::Float(p));
    g.point_attribs.set("ai_nut", AttribArray::Float(nut));
    g.point_attribs.set("Cd", AttribArray::Vec3(cd));
    g.detail_attribs.set("ai_label", AttribArray::Str(vec![AERO_LABEL.to_string()]));
    g.detail_attribs.set("ai_units", AttribArray::Str(vec![UNIT.to_string()]));
    Ok((g, stats, (lo, hi)))
}

fn p_str(p: &Params, k: &str) -> CookResult<String> {
    Ok(p.str(k)?.to_string())
}

fn legend(field: &str, lo: f64, hi: f64) -> Value {
    json!({"field": field, "unit": UNIT, "unit_note": "dimensionless model units from the checkpoint's min-max scaling inverse; not converted to Pa or m/s",
           "color_min": lo, "color_max": hi, "color_range": "2nd to 98th percentile", "ramp": "blue (low) to red (high)"})
}

pub struct AeroPreviewNode;

struct Settings {
    mode: String,
    color_by: String,
    y_m: f64,
    x_range: (f64, f64),
    z_range: (f64, f64),
    nx: usize,
    nz: usize,
}

fn settings(p: &Params) -> CookResult<Settings> {
    let (x0, x1) = (p.float("slice_x_min")?, p.float("slice_x_max")?);
    let (z0, z1) = (p.float("slice_z_min")?, p.float("slice_z_max")?);
    let (xmin, xmax) = (prep::VOLUME_BOX_MIN.x, prep::VOLUME_BOX_MAX.x);
    let (zmin, zmax) = (prep::VOLUME_BOX_MIN.z, prep::VOLUME_BOX_MAX.z);
    Ok(Settings {
        mode: p_str(p, "mode")?,
        color_by: p_str(p, "color_by")?,
        y_m: p.float("slice_y")?.clamp(prep::VOLUME_BOX_MIN.y, prep::VOLUME_BOX_MAX.y),
        x_range: (x0.min(x1).max(xmin), x0.max(x1).min(xmax)),
        z_range: (z0.min(z1).max(zmin), z0.max(z1).min(zmax)),
        nx: p.int("slice_nx")?.max(2) as usize,
        nz: p.int("slice_nz")?.max(2) as usize,
    })
}

impl NodeOp for AeroPreviewNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("cfd.ai_aero_preview", "AI Aero Preview", Stage::Cfd)
            .input(PortSpec::new("geometry", DataKind::Geometry))
            .output(PortSpec::new("surface", DataKind::Geometry))
            .output(PortSpec::new("slice", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("summary", DataKind::Json))
            .param(ParamSpec::path("ai_root", "AI-3D folder", &[], false).help("Folder holding the DoMINO install. Blank uses the VERNON_AI3D_ROOT environment variable."))
            .param(ParamSpec::string("wsl_distro", "WSL distribution", "AI-3D").help("Linux environment that holds the model's Python. Blank runs Python directly (Linux hosts)."))
            .param(ParamSpec::choice("mode", "Prediction", "surface", &["surface", "volume", "both"]).help("Surface pressure and wall shear, a volume slice of velocity, pressure and eddy viscosity, or both."))
            .param(ParamSpec::choice("placement", "Placement", "snap_to_training_frame", &["as_is", "snap_to_training_frame"]).help("Snap translates the mesh (never scales or rotates) so the nose, centreline and ground match the training frame. The model expects flow along +x, nose toward -x, z up."))
            .param(ParamSpec::bool("allow_outside", "Allow outside domain", false).help("Run even when the geometry is outside the checkpoint's bounding box. The result is labelled out of domain."))
            .param(ParamSpec::choice("color_by", "Colour surface by", "pressure", &["pressure", "wall_shear"]).help("Which predicted surface field drives the colour; the legend range is in the summary."))
            .param(ParamSpec::float("slice_y", "Slice plane y", 0.0, -2.25, 2.25).unit("m").help("Height of the volume slice plane in the model frame (0 = car centreline)."))
            .param(ParamSpec::float("slice_x_min", "Slice x min", -2.0, -3.5, 8.5).unit("m").help("Upstream end of the slice in the model frame (flow is +x)."))
            .param(ParamSpec::float("slice_x_max", "Slice x max", 6.0, -3.5, 8.5).unit("m").help("Downstream end of the slice in the model frame."))
            .param(ParamSpec::float("slice_z_min", "Slice z min", -0.3, -0.32, 3.0).unit("m").help("Bottom of the slice; the ground is at -0.32 m."))
            .param(ParamSpec::float("slice_z_max", "Slice z max", 2.0, -0.32, 3.0).unit("m").help("Top of the slice in the model frame."))
            .param(ParamSpec::int("slice_nx", "Slice points x", 64, 2, 256).help("Query points along x; more points cost more GPU memory."))
            .param(ParamSpec::int("slice_nz", "Slice points z", 24, 2, 128).help("Query points along z."))
            .param(ParamSpec::int("timeout_s", "Time limit", 1800, 60, 7200).unit("s").help("The model process is stopped after this long."))
            .param(ParamSpec::bool("run", "Run model", false).action().help("Starts the model. Without it the node only checks the geometry and loads an earlier result for identical inputs."))
            .help("Fast AI aerodynamic preview from the DoMINO DrivAerML checkpoints. Fixed operating point: 30 m/s, 1.205 kg/m3, no yaw. Outputs are dimensionless model units labelled 'AI prediction — CFD validation pending'. It is a screening aid, not a CFD result, and no force or drag is reported.")
            .emulates("AI surrogate for road-car external aerodynamics")
            .spec("docs/specs/ai-models.md")
            .implemented()
            .external("DoMINO (PhysicsNeMo)")
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(ins, 0)?;
        let s = settings(p)?;
        let placement = if p_str(p, "placement")? == "as_is" { Placement::AsIs } else { Placement::SnapToTrainingFrame };
        let opts = PrepOptions { placement, allow_outside: p.bool("allow_outside")?, ..PrepOptions::default() };
        let mut rep = Report::new("AI aero preview");
        rep.note(Severity::Warning, AERO_LABEL);
        rep.note(Severity::Info, format!("cfd_validation = {CFD_VALIDATION}: OpenFOAM acceptance stages are deferred; nothing here passes or replaces a CFD gate"));
        rep.note(Severity::Info, format!("operating point is fixed by the checkpoint: {} m/s inlet, {} kg/m3 air, zero yaw; it cannot be changed", prep::INLET_SPEED_MPS, prep::AIR_DENSITY));
        let prepared = match prep::prepare(&geo, &opts) {
            Ok(x) => x,
            Err(e) => {
                rep.note(Severity::Error, format!("geometry rejected before inference: {e}"));
                return Ok(empty_outputs(&geo, rep, json!({"label": AERO_LABEL, "cfd_validation": CFD_VALIDATION, "status": "rejected", "reason": e})));
            }
        };
        for c in &prepared.checks {
            rep.note(c.severity, format!("[{}] {}", c.id, c.message));
        }
        rep.metric("ai_open_edges", Metric::new(prepared.open_edges as f64, "count"));
        rep.metric("ai_triangles", Metric::new(prepared.tris.len() as f64, "count"));
        let ext = prepared.extents_m();
        rep.metric("ai_extent_x", Metric::new(ext.x, "m"));
        rep.metric("ai_extent_y", Metric::new(ext.y, "m"));
        rep.metric("ai_extent_z", Metric::new(ext.z, "m"));
        let want_surface = s.mode != "volume";
        let want_volume = s.mode != "surface";
        let (slice_pts, slice_idx, masked_ok) = if want_volume {
            prep::slice_points(&prepared, s.y_m, s.x_range, s.z_range, s.nx, s.nz)
        } else {
            (Vec::new(), Vec::new(), false)
        };
        let _ = slice_idx;
        if want_volume && !masked_ok {
            rep.note(Severity::Warning, "slice points inside the body were not removed because the mesh is not watertight; values there are meaningless");
        }
        let masked = if want_volume { s.nx * s.nz - slice_pts.len() } else { 0 };
        let stl = prep::stl_bytes(&prepared.tris);
        let key = {
            let mut b = stl.clone();
            b.extend_from_slice(format!("{}|{:?}|{:?}|{:?}|{}|{}|domino0.501", s.mode, s.y_m, s.x_range, s.z_range, s.nx, s.nz).as_bytes());
            fnv1a(&b)
        };
        let dir = ctx.work_dir.join(format!("aero_{key:016x}"));
        let summary_base = |status: &str| {
            json!({"label": AERO_LABEL, "cfd_validation": CFD_VALIDATION, "status": status, "domain": prepared.domain.label(),
                   "operating_point": {"inlet_speed_mps": prep::INLET_SPEED_MPS, "air_density": prep::AIR_DENSITY, "yaw_deg": 0.0},
                   "artifact_dir": dir.to_string_lossy(), "mode": s.mode,
                   "checks": prepared.checks.iter().map(|c| json!({"id": c.id, "severity": format!("{:?}", c.severity), "message": c.message})).collect::<Vec<_>>()})
        };
        if let Some(b) = prepared.blocking() {
            rep.note(Severity::Error, format!("inference not started: {}", b.message));
            return Ok(empty_outputs(&geo, rep, summary_base("blocked")));
        }
        let result_path = dir.join("result.json");
        let run = p.bool("run")?;
        if !result_path.is_file() {
            if !run {
                rep.note(Severity::Info, "not run: switch on 'Run model' to start the AI prediction (geometry checks above are complete)");
                return Ok(empty_outputs(&geo, rep, summary_base("not_run")));
            }
            run_model(ctx, p, &dir, &stl, &s, &slice_pts, prepared.tris.len())?;
        } else if run {
            rep.note(Severity::Info, "an earlier result for identical inputs was reused");
        }
        let res: Value = serde_json::from_slice(&std::fs::read(&result_path)?).map_err(|e| ctx.fail(format!("unreadable result.json: {e}")))?;
        let run_of = |mode: &str| res.get("runs").and_then(Value::as_array).and_then(|a| a.iter().find(|r| r.get("mode").and_then(Value::as_str) == Some(mode))).cloned();
        let mut surface_out = (*geo).clone();
        let mut slice_out = Geometry::new();
        let mut summary = summary_base("complete");
        summary["model"] = res.get("model").cloned().unwrap_or(Value::Null);
        summary["checkpoint"] = res.get("checkpoint").cloned().unwrap_or(Value::Null);
        summary["weights"] = res.get("manifest").cloned().unwrap_or(Value::Null);
        summary["checkpoint_sha256"] = res.get("checkpoint_sha256").cloned().unwrap_or(Value::Null);
        summary["runs"] = res.get("runs").cloned().unwrap_or(Value::Null);
        if want_surface {
            let f = read_f32(&dir.join("surface_fields.f32")).map_err(|e| ctx.fail(e))?;
            let rng = run_of("surface").map(|r| ranges(&r));
            let (g, st, (lo, hi)) = build_surface(&geo, &prepared, &f, &s.color_by, rng).map_err(|e| ctx.fail(e))?;
            surface_out = g;
            let m = |v: f64| Metric::new(v, UNIT);
            rep.metric("ai_surface_pressure_mean", m(st.p_mean));
            rep.metric("ai_surface_pressure_min", m(st.p_min));
            rep.metric("ai_surface_pressure_max", m(st.p_max));
            rep.metric("ai_surface_shear_mean", m(st.tau_mean));
            rep.metric("ai_surface_out_of_range_fraction", Metric::new(st.out_of_range_fraction, "1"));
            if st.out_of_range_fraction > 0.01 {
                rep.note(Severity::Warning, format!("{:.1}% of surface predictions lie outside the range seen in training", st.out_of_range_fraction * 100.0));
            }
            summary["surface"] = json!({"triangles": st.triangles, "area_m2": st.area_m2, "pressure_mean": st.p_mean, "pressure_min": st.p_min, "pressure_max": st.p_max,
                "shear_mean": st.tau_mean, "out_of_range_fraction": st.out_of_range_fraction, "unit": UNIT, "legend": legend(&s.color_by, lo, hi),
                "channels": SURFACE_CHANNELS, "area_weighted": true});
        }
        if want_volume {
            let f = read_f32(&dir.join("volume_fields.f32")).map_err(|e| ctx.fail(e))?;
            let rng = run_of("volume").map(|r| ranges(&r));
            let (g, st, (lo, hi)) = build_slice(&prepared, &slice_pts, &f, masked, rng).map_err(|e| ctx.fail(e))?;
            slice_out = g;
            let m = |v: f64| Metric::new(v, UNIT);
            rep.metric("ai_slice_speed_mean", m(st.speed_mean));
            rep.metric("ai_slice_speed_max", m(st.speed_max));
            rep.metric("ai_slice_pressure_mean", m(st.p_mean));
            rep.metric("ai_slice_out_of_range_fraction", Metric::new(st.out_of_range_fraction, "1"));
            if st.out_of_range_fraction > 0.05 {
                rep.note(Severity::Warning, format!("{:.1}% of slice predictions lie outside the range seen in training; treat the slice as qualitative at best", st.out_of_range_fraction * 100.0));
            }
            summary["slice"] = json!({"points": st.points, "masked_inside_body": st.masked, "speed_mean": st.speed_mean, "speed_min": st.speed_min, "speed_max": st.speed_max,
                "pressure_mean": st.p_mean, "out_of_range_fraction": st.out_of_range_fraction, "unit": UNIT, "legend": legend("speed", lo, hi), "channels": VOLUME_CHANNELS,
                "plane": {"y_m": s.y_m, "x_m": [s.x_range.0, s.x_range.1], "z_m": [s.z_range.0, s.z_range.1]}});
        }
        if prepared.domain != Domain::WithinTrainingSpan {
            rep.note(Severity::Warning, format!("domain: {}", prepared.domain.label()));
        }
        rep.note(Severity::Info, "no drag or lift force is reported: the checkpoint's dimensional scaling is unverified and Vernon has no verified force integration for it");
        let _ = std::fs::write(dir.join("summary.json"), serde_json::to_vec_pretty(&summary).unwrap_or_default());
        Ok(vec![
            Data::geometry(surface_out),
            Data::geometry(slice_out),
            Data::report(rep),
            Data::json(summary),
        ])
    }
}

fn empty_outputs(geo: &Arc<Geometry>, rep: Report, summary: Value) -> Vec<Data> {
    vec![Data::Geometry(geo.clone()), Data::geometry(Geometry::new()), Data::report(rep), Data::json(summary)]
}

fn run_model(ctx: &CookContext, p: &Params, dir: &Path, stl: &[u8], s: &Settings, slice_pts: &[DVec3], tris: usize) -> CookResult<()> {
    let install = Install::resolve(p.str("ai_root")?, p.str("wsl_distro")?).map_err(|e| ctx.fail(e.to_string()))?;
    install.require_model_dir("DoMINO").map_err(|e| ctx.fail(e.to_string()))?;
    let have = bridge::query_resources(&install).map_err(|e| ctx.fail(e.to_string()))?;
    let surf = surface_peak_vram_gb(tris);
    let vram = match s.mode.as_str() {
        "surface" => surf,
        "volume" => VOLUME_PEAK_VRAM_GB,
        _ => surf.max(VOLUME_PEAK_VRAM_GB),
    };
    check_resources(with_margin(vram, PEAK_RSS_GB), have, "DoMINO").map_err(|e| ctx.fail(e.to_string()))?;
    std::fs::create_dir_all(dir)?;
    let stl_path = dir.join("input.stl");
    std::fs::write(&stl_path, stl)?;
    let request = json!({
        "ai_root": install.root.to_string_lossy(),
        "stl": stl_path.to_string_lossy(),
        "out_dir": dir.to_string_lossy(),
        "mode": s.mode,
        "volume_points_m": slice_pts.iter().map(|q| [q.x, q.y, q.z]).collect::<Vec<_>>(),
    });
    let timeout = Duration::from_secs(p.int("timeout_s")?.max(60) as u64);
    bridge::run_job(&install, "DoMINO", "domino_bridge.py", bridge::DOMINO_BRIDGE, &request, dir, timeout).map_err(|e| match e {
        BridgeError::Cancelled => CookError::Other(vernon_graph::CANCELLED.into()),
        other => ctx.fail(other.to_string()),
    })?;
    Ok(())
}


//! Material-side nodes: printed material, layer axes, temperature derating,
//! temperature field map, creep plan and the material table.

use std::collections::HashMap;

use glam::{DMat3, DVec3};
use rayon::prelude::*;
use vernon_core::{AttribArray, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::material::{dma_anchor_table, dma_csv_table, hdt_anchor_table, preset, MaterialCard, Preset, PRESETS};
use crate::spatial::TriGrid;
use crate::study::{reading_times, suggested_creep_factor, CreepRow};
use crate::types::{finite, from_data, opt_from_data, to_data, OrientationCard};
use crate::util::{fea_info, solid_mesh};

// ---------------------------------------------------------------- material

/// Read a header + rows table; returns the row whose `name` matches `want`
/// (or the first data row) as lower-case key to number.
pub fn parse_material_table(text: &str, want: &str) -> HashMap<String, f64> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
    let Some(head) = lines.next() else { return HashMap::new() };
    let keys: Vec<String> = head.split([',', ';', '\t']).map(|k| k.trim().to_ascii_lowercase()).collect();
    let name_col = keys.iter().position(|k| k == "name");
    let rows: Vec<Vec<&str>> = lines.map(|l| l.split([',', ';', '\t']).map(str::trim).collect()).collect();
    let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let row = name_col
        .and_then(|c| rows.iter().find(|r| r.get(c).is_some_and(|n| norm(n) == norm(want))))
        .or(rows.first());
    let mut out = HashMap::new();
    if let Some(r) = row {
        for (k, v) in keys.iter().zip(r.iter()) {
            if let Ok(x) = v.parse::<f64>() {
                if x.is_finite() {
                    out.insert(k.clone(), x);
                }
            }
        }
    }
    out
}

/// Build the material card from the parameters, an optional table row and a preset.
pub fn build_card(params: &Params, table: &HashMap<String, f64>) -> Result<(MaterialCard, Preset), String> {
    let id = params.str("preset").map_err(|e| e.to_string())?;
    let custom = id == "custom";
    let p = if custom { PRESETS[0] } else { *preset(id).ok_or_else(|| format!("unknown preset '{id}'"))? };
    let pick = |name: &str, key: &str, dflt: Option<f64>| -> Result<Option<f64>, String> {
        let v = params.float(name).map_err(|e| e.to_string())?;
        if v > 0.0 {
            return Ok(Some(v));
        }
        Ok(table.get(key).copied().or(dflt))
    };
    let mut notes = Vec::new();
    let e_xy = pick("e_xy", "e_xy", Some(p.e_xy))?.unwrap_or(p.e_xy);
    let e_z = pick("e_z", "e_z", Some(p.e_z))?.unwrap_or(p.e_z);
    let nu_xy = pick("nu_xy", "nu_xy", Some(p.nu_xy))?.unwrap_or(p.nu_xy);
    let nu_xz = pick("nu_xz", "nu_xz", Some(p.nu_xz))?.unwrap_or(p.nu_xz);
    let g_xz = pick("g_xz", "g_xz", None)?.unwrap_or(e_z / (2.0 * (1.0 + nu_xz)));
    let xt = pick("xt", "xt", Some(p.xt))?.unwrap_or(p.xt);
    let z_ratio = pick("z_ratio", "z_ratio", Some(p.z_ratio))?.unwrap_or(p.z_ratio);
    let zt = xt * z_ratio;
    let xc = match pick("xc", "xc", None)? {
        Some(v) => v,
        None => {
            notes.push("compressive strength not given: assumed 1.2 x the tensile strength".to_string());
            1.2 * xt
        }
    };
    let zc = match pick("zc", "zc", None)? {
        Some(v) => v,
        None => 1.2 * zt,
    };
    let s12 = pick("s12", "s12", None)?.unwrap_or(0.6 * xt);
    let s13 = pick("s13", "s13", None)?.unwrap_or(0.5 * zt);
    let rho_g = pick("density_g_cm3", "density", Some(p.density_g_cm3))?.unwrap_or(p.density_g_cm3);
    let infill = params.float("infill_eff").map_err(|e| e.to_string())?;

    let mut c = MaterialCard::trans_iso(if custom { "custom" } else { p.name }, e_xy, e_z, nu_xy, nu_xz, g_xz);
    c.g12 = e_xy / (2.0 * (1.0 + nu_xy));
    c.density_t_mm3 = rho_g * 1e-9;
    c.xt = xt;
    c.xc = xc;
    c.zt = zt;
    c.zc = zc;
    c.s12 = s12;
    c.s13 = s13;
    c.s23 = s13;
    c.hdt_045_c = table.get("hdt_045").copied().or(Some(p.hdt_045_c));
    c.hdt_18_c = table.get("hdt_18").copied().or(p.hdt_18_c);
    c.dma = p.dma;
    c.water_abs_pct = table.get("water_abs_pct").copied().or(p.water_abs_pct);
    if params.str("mode").map_err(|e| e.to_string())? == "isotropic_z" {
        c.mode = "isotropic_z".into();
        c.e1 = e_z;
        c.e2 = e_z;
        c.e3 = e_z;
        for nu in [&mut c.nu12, &mut c.nu13, &mut c.nu23] {
            *nu = nu_xz;
        }
        let g = e_z / (2.0 * (1.0 + nu_xz));
        c.g12 = g;
        c.g13 = g;
        c.g23 = g;
        c.xt = zt;
        c.xc = zc;
        c.s12 = s13;
        notes.push("isotropic_z: the stacking-direction values are used in every direction (conservative bound)".into());
    }
    if infill < 1.0 {
        for v in [&mut c.e1, &mut c.e2, &mut c.e3, &mut c.g12, &mut c.g13, &mut c.g23, &mut c.density_t_mm3, &mut c.xt, &mut c.xc, &mut c.zt, &mut c.zc, &mut c.s12, &mut c.s13, &mut c.s23] {
            *v *= infill;
        }
        notes.push(format!("infill weighting {infill}: density, stiffness and strength scaled linearly (placeholder rule)"));
    }
    if !custom && !table.contains_key("e_xy") && params.float("e_xy").map_err(|e| e.to_string())? <= 0.0 {
        notes.push(p.note.to_string());
    }
    // the shared material table tags every value with its source; say which are placeholders
    notes.push(format!("strength ratio basis ({}): {}", p.shared.z_ratio_source.label(), p.z_ratio_basis));
    if !custom && !table.contains_key("water_abs_pct") && p.water_abs_pct.is_some() && p.shared.water_source == vernon_core::materials::Source::Assumed {
        notes.push(format!("water uptake {}% is assumed, not measured for {}", p.water_abs_pct.unwrap_or_default(), p.name));
    }
    c.notes = notes;
    c.check_stability().map_err(|e| format!("material constants rejected: {e}"))?;
    Ok((c, p))
}

pub struct MaterialFdm;

impl NodeOp for MaterialFdm {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.material_fdm", "Printed Material")
            .input(PortSpec::optional("table", DataKind::File))
            .output(PortSpec::new("material", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("preset", "Preset", "ppa_cf", &["pla", "petg", "asa", "pet_cf", "paht_cf", "ppa_cf", "custom"]).help("Printed-material table row; custom starts from the PLA row"))
            .param(ParamSpec::choice("mode", "Mode", "orthotropic", &["orthotropic", "isotropic_z"]).help("Direction-dependent constants, or isotropic with the stacking-direction values everywhere"))
            .param(ParamSpec::float("e_xy", "In-plane modulus", 0.0, 0.0, 100000.0).unit("MPa").help("0 takes the preset or table value"))
            .param(ParamSpec::float("e_z", "Stacking modulus", 0.0, 0.0, 100000.0).unit("MPa").help("0 takes the preset or table value"))
            .param(ParamSpec::float("nu_xy", "In-plane Poisson ratio", 0.0, 0.0, 0.49).help("0 takes the preset or table value"))
            .param(ParamSpec::float("nu_xz", "Stacking Poisson ratio", 0.0, 0.0, 0.49).help("0 takes the preset or table value"))
            .param(ParamSpec::float("g_xz", "Stacking shear modulus", 0.0, 0.0, 50000.0).unit("MPa").help("0 derives E_z / (2 (1 + nu))"))
            .param(ParamSpec::float("xt", "In-plane tensile strength", 0.0, 0.0, 2000.0).unit("MPa").help("0 takes the preset or table value"))
            .param(ParamSpec::float("xc", "In-plane compressive strength", 0.0, 0.0, 4000.0).unit("MPa").help("0 assumes 1.2 x the tensile strength"))
            .param(ParamSpec::float("z_ratio", "Stacking strength ratio", 0.0, 0.0, 1.5).help("Stacking-direction strength over in-plane strength; 0 takes the preset"))
            .param(ParamSpec::float("zc", "Stacking compressive strength", 0.0, 0.0, 4000.0).unit("MPa").help("0 assumes 1.2 x the stacking tensile strength"))
            .param(ParamSpec::float("s12", "In-plane shear strength", 0.0, 0.0, 2000.0).unit("MPa").help("0 assumes 0.6 x the tensile strength"))
            .param(ParamSpec::float("s13", "Interlayer shear strength", 0.0, 0.0, 2000.0).unit("MPa").help("0 assumes 0.5 x the stacking tensile strength"))
            .param(ParamSpec::float("density_g_cm3", "Density", 0.0, 0.0, 10.0).unit("g/cm3").help("0 takes the preset or table value"))
            .param(ParamSpec::float("infill_eff", "Infill weighting", 1.0, 0.05, 1.0).help("Scales density, stiffness and strength for partial infill"))
            .help("Printed-material constants for the structural solver: transversely isotropic stiffness, directional strengths, density and heat-deflection data. Absolute values in the preset table are placeholders until replaced by datasheet or coupon data.")
            .emulates("orthotropic elastic cards of finite-element solvers and the printed-material anisotropy table of the project report")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let want = params.str("preset")?.to_string();
        let table = match inputs.first().and_then(|d| d.as_ref()).and_then(|d| d.as_file()) {
            Some(path) => {
                let text = std::fs::read_to_string(path).map_err(|e| ctx.fail(format!("cannot read {}: {e}", path.display())))?;
                parse_material_table(&text, &want)
            }
            None => HashMap::new(),
        };
        let (card, p) = build_card(params, &table).map_err(|e| ctx.fail(e))?;
        let mut r = Report::new(format!("Printed material {}", card.name));
        let delta = card.check_stability().unwrap_or(0.0);
        r.metric("e_inplane_mpa", Metric::new(card.e1, "MPa"));
        r.metric("e_stack_mpa", Metric::new(card.e3, "MPa"));
        r.metric("e_stack_over_inplane", Metric::new(card.e3 / card.e1, ""));
        r.metric("xt_mpa", Metric::new(card.xt, "MPa"));
        r.metric("zt_mpa", Metric::new(card.zt, "MPa"));
        r.metric("strength_z_ratio", Metric::new(card.z_ratio(), ""));
        r.metric("stiffness_delta", Metric::new(delta, ""));
        r.metric("density_t_mm3", Metric::new(card.density_t_mm3, "t/mm3"));
        if let Some(h) = card.hdt_045_c {
            r.metric(vernon_core::keys::HDT_045_C, Metric::new(h, "degC"));
        }
        if let Some(w) = card.water_abs_pct {
            r.metric("water_absorption_pct", Metric::new(w, "%"));
        }
        for n in &card.notes {
            r.note(Severity::Warning, n.clone());
        }
        if p.hdt_045_annealed_c.is_some() {
            r.note(Severity::Info, "heat deflection temperature depends on annealing; the temperature derating node selects the row");
        }
        Ok(vec![to_data(&card), Data::report(r)])
    }
}

// ------------------------------------------------------------ layer axes

/// Material axes (rows) for a print rotation `R = Rz Ry Rx` (model to printer frame).
pub fn layer_axes(rot_deg: DVec3) -> [[f64; 3]; 3] {
    let r = DMat3::from_rotation_z(rot_deg.z.to_radians()) * DMat3::from_rotation_y(rot_deg.y.to_radians()) * DMat3::from_rotation_x(rot_deg.x.to_radians());
    let rt = r.transpose();
    [0, 1, 2].map(|k| {
        let c = rt.col(k);
        [c.x, c.y, c.z].map(|v| if v.abs() < 1e-15 { 0.0 } else { v })
    })
}

pub struct LayerAxes;

impl NodeOp for LayerAxes {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.layer_axes", "Layer Axes")
            .input(PortSpec::optional("part", DataKind::Geometry))
            .output(PortSpec::new("orientation", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::vec3("rotation", "Print rotation", DVec3::ZERO).unit("deg").help("Rotation of the part into the printer frame (about X, then Y, then Z); the build direction is the printer Z axis"))
            .param(ParamSpec::vec3("origin", "Origin", DVec3::ZERO).unit("mm").help("Reference point added to both axis points on the orientation card"))
            .param(ParamSpec::string("name", "Name", "OR1").help("Orientation name used in the deck"))
            .help("Material frame of one printed part: axis 1 and 2 in the print plane, axis 3 along the layer stacking. The build direction is global to the part and does not follow a curved panel's surface normal.")
            .emulates("local material orientation cards of finite-element solvers")
            .implemented()
    }

    fn cook(&self, _ctx: &CookContext, _inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let rot = params.vec3("rotation")?;
        let axes = layer_axes(rot);
        let name = crate::util::sanitize(params.str("name")?).to_uppercase();
        let card = OrientationCard { name, axes, origin: params.vec3("origin")?.into(), rot_deg: rot.into() };
        let mut r = Report::new("Layer axes");
        let b = DVec3::from(axes[2]);
        r.metric("build_dir_x", Metric::new(b.x, ""));
        r.metric("build_dir_y", Metric::new(b.y, ""));
        r.metric("build_dir_z", Metric::new(b.z, ""));
        r.metric("stack_angle_from_z_deg", Metric::new(b.dot(DVec3::Z).clamp(-1.0, 1.0).acos().to_degrees(), "deg"));
        r.note(Severity::Info, "build direction is global to the part; a curved panel's stacking axis does not follow its surface normal");
        Ok(vec![to_data(&card), Data::report(r)])
    }
}

// ------------------------------------------------------------- derating

pub struct MaterialDerate;

impl NodeOp for MaterialDerate {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.material_derate", "Temperature Derating")
            .input(PortSpec::new("material", DataKind::Json))
            .output(PortSpec::new("material", DataKind::Json))
            .output(PortSpec::new("retention", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("source", "Source", "hdt_anchor", &["dma_csv", "dma_anchor", "hdt_anchor"]).help("Measured curve, stored dynamic-mechanical anchors, or heat-deflection anchors"))
            .param(ParamSpec::path("dma_csv_path", "Curve file", &["csv", "txt"], false).help("Temperature and storage modulus columns, normalised at 23 degC"))
            .param(ParamSpec::bool("annealed", "Annealed", false).help("Selects the annealed heat-deflection row where the material has one"))
            .param(ParamSpec::float("hdt_045_c", "HDT at 0.45 MPa", 0.0, 0.0, 400.0).unit("degC").help("0 takes the material table value"))
            .param(ParamSpec::float("hdt_18_c", "HDT at 1.8 MPa", 0.0, 0.0, 400.0).unit("degC").help("0 takes the material table value or 15 K below the 0.45 MPa value"))
            .param(ParamSpec::float("creep_factor", "Creep factor", 1.0, 0.05, 1.0).help("Sustained-load knockdown on top of the short-time curve"))
            .param(ParamSpec::float("r_floor", "Retention floor", 0.05, 0.001, 0.5).help("Lowest retention ratio the table may reach"))
            .param(ParamSpec::int("points", "Table points", 6, 4, 8).help("Temperatures written to the solver deck"))
            .help("Retention of stiffness and strength with temperature, normalised at 23 degC, applied to the modulus, shear modulus and strength alike. Anchored curves are uncalibrated placeholders until a measured curve is supplied.")
            .emulates("temperature-dependent elastic constants and heat-deflection anchoring from the project report")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let mut card: MaterialCard = from_data(inputs, 0, "material card")?;
        let floor = params.float("r_floor")?;
        let n = params.int("points")?.clamp(4, 8) as usize;
        let annealed = params.bool("annealed")?;
        let pre = preset(&card.name);
        let pos = |v: f64| (v > 0.0).then_some(v);
        let hdt045 = pos(params.float("hdt_045_c")?).or_else(|| pre.map(|p| p.hdt_045(annealed))).or(card.hdt_045_c);
        let hdt18 = pos(params.float("hdt_18_c")?).or_else(|| pre.and_then(|p| p.hdt_18_c)).or(card.hdt_18_c);
        let table = match params.str("source")? {
            "dma_csv" => {
                let path = params.str("dma_csv_path")?;
                if path.is_empty() {
                    return Err(ctx.fail("no curve file chosen for the dma_csv source"));
                }
                let text = std::fs::read_to_string(path).map_err(|e| ctx.fail(format!("cannot read {path}: {e}")))?;
                dma_csv_table(&text, n, floor).map_err(|e| ctx.fail(e))?
            }
            "dma_anchor" => {
                let a = card.dma.or_else(|| pre.and_then(|p| p.dma)).ok_or_else(|| ctx.fail("this material has no stored dynamic-mechanical anchors; use hdt_anchor or dma_csv"))?;
                dma_anchor_table(&a, floor)
            }
            _ => {
                let h = hdt045.ok_or_else(|| ctx.fail("no heat-deflection temperature known for this material"))?;
                hdt_anchor_table(card.e1, h, hdt18, n, floor).map_err(|e| ctx.fail(e))?
            }
        };
        card.retention = Some(table.clone());
        card.creep_factor = params.float("creep_factor")?;
        card.annealed = annealed;
        card.hdt_045_c = hdt045;
        card.hdt_18_c = hdt18;
        let mut r = Report::new("Temperature derating");
        for t in [23.0, 40.0, 60.0, 85.0, 105.0] {
            r.metric(format!("retention_{t:.0}c"), Metric::new(card.retention_at(t), ""));
        }
        r.metric("creep_factor", Metric::new(card.creep_factor, ""));
        r.metric("table_points", Metric::new(table.points.len() as f64, ""));
        if let Some(h) = hdt045 {
            r.metric(vernon_core::keys::HDT_045_C, Metric::new(h, "degC"));
        }
        r.metric("uncalibrated", Metric::new(if table.uncalibrated { 1.0 } else { 0.0 }, "flag"));
        if table.uncalibrated {
            r.note(Severity::Warning, "retention curve is an uncalibrated placeholder: supply a measured storage-modulus curve for a trustworthy safety factor");
        }
        r.note(Severity::Info, "short-time curve: sustained loads creep, use the oven creep plan to set the creep factor");
        Ok(vec![to_data(&card), to_data(&table), Data::report(r)])
    }
}

// ------------------------------------------------------ temperature map

pub struct TemperatureMap;

impl NodeOp for TemperatureMap {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.temperature_map", "Temperature Field Map")
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .input(PortSpec::optional("surface", DataKind::Geometry))
            .input(PortSpec::optional("material", DataKind::Json))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("uniform_c", "Uniform temperature", 23.0, -40.0, 300.0).unit("degC").help("Used for every node when no surface is connected, and for nodes beyond the search distance"))
            .param(ParamSpec::choice("through_thickness", "Through thickness", "uniform", &["uniform", "linear"]).help("Keep the surface temperature through the wall, or fall linearly to the inside temperature"))
            .param(ParamSpec::float("t_inside_c", "Inside temperature", 30.0, -40.0, 300.0).unit("degC").help("Temperature reached at the wall depth"))
            .param(ParamSpec::float("wall_mm", "Wall depth", 3.0, 0.2, 100.0).unit("mm").help("Depth over which the linear fall happens"))
            .param(ParamSpec::float("max_dist_mm", "Search distance", 50.0, 0.1, 10000.0).unit("mm").help("Nodes farther than this from the surface keep the uniform temperature"))
            .help("Maps per-point surface temperatures onto the nodes of the solid mesh (nearest point on the surface, interpolated inside the triangle) and writes a point attribute temp_c.")
            .emulates("thermal-to-structural temperature field mapping")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let mesh = solid_mesh(ctx, inputs, 0)?;
        let uniform = params.float("uniform_c")?;
        let linear = params.str("through_thickness")? == "linear";
        let (t_in, wall, max_d) = (params.float("t_inside_c")?, params.float("wall_mm")?, params.float("max_dist_mm")?);
        let mut unmapped = 0usize;
        let temps: Vec<f64> = match inputs::opt_geometry(inputs, 1) {
            None => vec![uniform; mesh.points.len()],
            Some(surf) => {
                let pt = surf.point_attribs.floats("temp_c").ok();
                let pr = surf.prim_attribs.floats("temp_c").ok();
                if pt.is_none() && pr.is_none() {
                    return Err(ctx.fail("the surface has no temp_c attribute: run the surface heat balance first"));
                }
                let tris = surf.triangles();
                if tris.is_empty() {
                    return Err(ctx.fail("the surface has no triangles"));
                }
                let grid = TriGrid::new(tris.iter().map(|(_, t)| t.map(|i| surf.points[i as usize])).collect());
                let found: Vec<Option<f64>> = mesh
                    .points
                    .par_iter()
                    .map(|&p| {
                        let n = grid.nearest(p, max_d)?;
                        let (prim, t) = tris[n.tri];
                        let ts = match pt {
                            Some(v) => n.bary[0] * v[t[0] as usize] + n.bary[1] * v[t[1] as usize] + n.bary[2] * v[t[2] as usize],
                            None => pr.map_or(uniform, |v| v[prim]),
                        };
                        Some(if linear { ts + (t_in - ts) * (n.dist / wall).clamp(0.0, 1.0) } else { ts })
                    })
                    .collect();
                found
                    .into_iter()
                    .map(|v| {
                        v.unwrap_or_else(|| {
                            unmapped += 1;
                            uniform
                        })
                    })
                    .collect()
            }
        };
        let mut out = (*mesh).clone();
        out.point_attribs.set("temp_c", AttribArray::Float(temps.clone()));
        let (tmin, tmax) = temps.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, &t| (a.0.min(t), a.1.max(t)));
        let mut r = Report::new("Temperature field");
        r.metric("temp_min_c", Metric::new(finite(tmin), "degC"));
        r.metric("temp_max_c", Metric::new(finite(tmax), "degC"));
        r.metric("temp_mean_c", Metric::new(temps.iter().sum::<f64>() / temps.len().max(1) as f64, "degC"));
        r.metric("unmapped_nodes", Metric::new(unmapped as f64, ""));
        if unmapped > 0 {
            r.note(Severity::Warning, format!("{unmapped} nodes were farther than {max_d} mm from the surface and keep the uniform temperature"));
        }
        if let Some(card) = opt_from_data::<MaterialCard>(inputs, 2, "material card")? {
            if let Some(h) = card.hdt_045_c {
                r.metric("margin_to_hdt_c", Metric::new(h - tmax, "degC").with_limit(0.0, false));
            }
        }
        Ok(vec![Data::geometry(out), Data::report(r)])
    }
}

// ------------------------------------------------------------ creep plan

pub struct CreepPlan;

impl NodeOp for CreepPlan {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.creep_plan", "Oven Creep Test Plan")
            .input(PortSpec::optional("material", DataKind::Json))
            .output(PortSpec::new("plan", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("t1_c", "Oven temperature 1", 85.0, 40.0, 250.0).unit("degC").help("First oven temperature. Keep it below the heat deflection temperature of the material."))
            .param(ParamSpec::float("t2_c", "Oven temperature 2", 95.0, 40.0, 250.0).unit("degC").help("Second oven temperature."))
            .param(ParamSpec::float("t3_c", "Oven temperature 3", 105.0, 40.0, 250.0).unit("degC").help("Third oven temperature. The margin to the heat deflection temperature is reported for each."))
            .param(ParamSpec::float("hold_h", "Hold time", 24.0, 1.0, 1000.0).unit("h").help("Time each coupon is held under load in the oven, in hours."))
            .param(ParamSpec::float("stress_fraction", "Stress fraction", 0.2, 0.02, 0.8).help("Sustained stress as a fraction of the stacking-direction tensile strength"))
            .param(ParamSpec::float("coupon_w_mm", "Coupon width", 10.0, 2.0, 50.0).unit("mm").help("Width of the test coupon. With the thickness it sets the load for the chosen stress."))
            .param(ParamSpec::float("coupon_t_mm", "Coupon thickness", 4.0, 1.0, 20.0).unit("mm").help("Thickness of the test coupon."))
            .param(ParamSpec::float("allowed_strain_pct", "Allowed creep strain", 0.5, 0.01, 10.0).unit("%").help("Creep strain the part may reach in service. Measured strains above it give a creep factor below 1."))
            .param(ParamSpec::float("measured_1_pct", "Measured strain 1", 0.0, 0.0, 100.0).unit("%").help("Creep strain at the end of the hold at temperature 1; 0 means not measured"))
            .param(ParamSpec::float("measured_2_pct", "Measured strain 2", 0.0, 0.0, 100.0).unit("%").help("0 means not measured"))
            .param(ParamSpec::float("measured_3_pct", "Measured strain 3", 0.0, 0.0, 100.0).unit("%").help("0 means not measured"))
            .help("Plan for sustained-load coupon tests in an oven at three temperatures: load, hold time, reading times and the margin to the heat deflection temperature. Entering measured strains returns a suggested creep factor for the temperature derating node.")
            .emulates("oven creep tests recommended in the project report")
            .implemented()
    }

    fn cook(&self, _ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let card: MaterialCard = opt_from_data(inputs, 0, "material card")?.unwrap_or_else(|| {
            let p = &PRESETS[5];
            MaterialCard::trans_iso(p.name, p.e_xy, p.e_z, p.nu_xy, p.nu_xz, p.e_z / (2.0 * (1.0 + p.nu_xz)))
        });
        let hold = params.float("hold_h")?;
        let area = params.float("coupon_w_mm")? * params.float("coupon_t_mm")?;
        let stress = params.float("stress_fraction")? * card.zt;
        let allowed = params.float("allowed_strain_pct")?;
        let mut rows = Vec::new();
        let mut r = Report::new("Oven creep plan");
        let mut worst: f64 = 1.0;
        for (i, key) in ["t1_c", "t2_c", "t3_c"].iter().enumerate() {
            let t = params.float(key)?;
            let measured = params.float(&format!("measured_{}_pct", i + 1))?;
            let sugg = suggested_creep_factor(measured, allowed);
            let margin = card.hdt_045_c.map(|h| h - t);
            rows.push(CreepRow {
                temperature_c: t,
                hold_h: hold,
                stress_mpa: stress,
                load_n: stress * area,
                hdt_margin_c: margin,
                readings_h: reading_times(hold),
                suggested_creep_factor: sugg,
            });
            r.metric(format!("load_n_{t:.0}c"), Metric::new(stress * area, "N"));
            if let Some(m) = margin {
                r.metric(format!("hdt_margin_{t:.0}c"), Metric::new(m, "degC"));
                if m <= 0.0 {
                    r.note(Severity::Warning, format!("{t:.0} degC is above the heat deflection temperature of {}: expect large deformation under any load", card.name));
                }
            }
            if let Some(f) = sugg {
                r.metric(format!("suggested_creep_factor_{t:.0}c"), Metric::new(f, ""));
                worst = worst.min(f);
            }
        }
        r.metric("sustained_stress_mpa", Metric::new(stress, "MPa"));
        if rows.iter().any(|x| x.suggested_creep_factor.is_some()) {
            r.metric("suggested_creep_factor", Metric::new(worst, ""));
        } else {
            r.note(Severity::Info, "no measured strains entered: run the plan, then enter the end-of-hold strains to get a creep factor");
        }
        Ok(vec![to_data(&rows), Data::report(r)])
    }
}

// ------------------------------------------------------- material table

pub struct MaterialTable;

impl NodeOp for MaterialTable {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.material_table", "Printed Material Table")
            .output(PortSpec::new("table", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .help("The printed-material database: strength ratio of the stacking direction, heat deflection temperatures and water absorption per filament family. Absolute moduli and strengths are placeholders.")
            .emulates("printed-material comparison table of the project report")
            .implemented()
    }

    fn cook(&self, _ctx: &CookContext, _inputs: &[Option<Data>], _params: &Params) -> CookResult<Vec<Data>> {
        let mut r = Report::new("Printed material table");
        r.metric("materials", Metric::new(PRESETS.len() as f64, ""));
        for p in &PRESETS {
            r.metric(format!("{}_z_ratio", p.id), Metric::new(p.z_ratio, ""));
            r.metric(format!("{}_hdt_045_c", p.id), Metric::new(p.hdt_045_c, "degC"));
        }
        r.note(Severity::Warning, "absolute moduli and strengths are placeholders: use datasheet or coupon data for the actual spool");
        let v = serde_json::to_value(&PRESETS[..]).unwrap_or(serde_json::Value::Null);
        Ok(vec![Data::json(v), Data::report(r)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vernon_graph::NodeOp;

    fn params_of(op: &dyn NodeOp) -> Params {
        Params::from_specs(&op.info().params)
    }

    #[test]
    fn card_from_defaults_uses_the_ppa_cf_report_values() {
        let p = params_of(&MaterialFdm);
        let (c, pre) = build_card(&p, &HashMap::new()).unwrap();
        assert_eq!(pre.id, "ppa_cf");
        assert!((c.zt - 57.0).abs() < 1e-9 && (c.xt - 168.0).abs() < 1e-9);
        assert!((c.xc - 201.6).abs() < 1e-9 && (c.s12 - 100.8).abs() < 1e-9);
        assert!(c.check_stability().is_ok());
        assert!((c.density_t_mm3 - 1.28e-9).abs() < 1e-18);
    }

    #[test]
    fn overrides_table_and_isotropic_z() {
        let mut p = params_of(&MaterialFdm);
        p.set("preset", vernon_graph::ParamValue::Str("pla".into()));
        let table = parse_material_table("name,e_xy,xt\npetg,1,2\npla,3100,48\n", "pla");
        assert_eq!(table["e_xy"], 3100.0);
        let (c, _) = build_card(&p, &table).unwrap();
        assert_eq!((c.e1, c.xt), (3100.0, 48.0));
        p.set("e_xy", vernon_graph::ParamValue::Float(2800.0));
        assert_eq!(build_card(&p, &table).unwrap().0.e1, 2800.0);
        p.set("mode", vernon_graph::ParamValue::Str("isotropic_z".into()));
        let (c, _) = build_card(&p, &table).unwrap();
        assert!(c.e1 == c.e3 && c.xt == c.zt && (c.g12 - c.e3 / (2.0 * (1.0 + c.nu13))).abs() < 1e-9);
        p.set("infill_eff", vernon_graph::ParamValue::Float(0.5));
        let (c2, _) = build_card(&p, &table).unwrap();
        assert!((c2.e1 - 0.5 * c.e1).abs() < 1e-9);
    }

    #[test]
    fn layer_axes_for_known_rotations() {
        let a = layer_axes(DVec3::ZERO);
        assert_eq!(a, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        // part rolled 90 degrees about X: printer Z is model -Y or +Y
        let b = layer_axes(DVec3::new(90.0, 0.0, 0.0));
        assert!((DVec3::from(b[2]) - DVec3::new(0.0, 1.0, 0.0)).length() < 1e-12 || (DVec3::from(b[2]) + DVec3::new(0.0, 1.0, 0.0)).length() < 1e-12);
        for r in [DVec3::new(30.0, 40.0, 50.0), DVec3::new(-10.0, 80.0, 5.0)] {
            let m = layer_axes(r);
            for i in 0..3 {
                for j in 0..3 {
                    let d = DVec3::from(m[i]).dot(DVec3::from(m[j]));
                    assert!((d - if i == j { 1.0 } else { 0.0 }).abs() < 1e-12);
                }
            }
        }
    }

    #[test]
    fn g6_stress_scales_with_inverse_retention() {
        // linear model: displacement at 50 degC is 1 / r(50) of the 23 degC value
        let t = hdt_anchor_table(7500.0, 227.0, Some(196.0), 6, 0.05).unwrap();
        let mut c = MaterialCard::trans_iso("x", 7500.0, 4500.0, 0.35, 0.3, 1700.0);
        c.retention = Some(t.clone());
        let r50 = c.retention_at(50.0);
        assert!(r50 < 1.0 && r50 > 0.5, "{r50}");
        assert!((1.0 / r50 - 1.0 / t.at(50.0)).abs() < 1e-12);
    }
}

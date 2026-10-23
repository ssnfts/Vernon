//! Fit and orientation nodes: build-volume fit, orientation score, directional
//! strength, heat margin gate and stair-step estimate.

use glam::{DMat4, DVec3};
use serde_json::json;
use vernon_core::{keys, AttribArray, AttribType, Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::common::{info, material, v3_json, Material, MATERIAL_IDS};
use crate::fit::{fit_part, matrix_rows, FitOptions, Placement};
use crate::mesh;
use crate::nodes_split::{grid_params, read_bed};
use crate::orient::{self, HeatInput, OrientParams, Search, Tris, DEFAULT_WEIGHTS};

/// (id, geometry) of every part that owns closed faces.
fn parts_of(geo: &Geometry) -> Vec<(i64, Geometry)> {
    mesh::unique_parts(geo).into_iter().map(|id| (id, mesh::extract_part(geo, id))).collect()
}

fn material_param(info: NodeTypeInfo) -> NodeTypeInfo {
    info.param(ParamSpec::choice("material", "Material", "pla", &MATERIAL_IDS).help("Filament family; sets the Z strength ratio and heat deflection temperature."))
        .param(ParamSpec::float("k_override", "Z strength ratio override", 0.0, 0.0, 1.5).help("Z allowable divided by in-plane allowable; 0 uses the material table."))
}

/// Material and its Z strength ratio (honouring the override).
fn material_k(p: &Params) -> CookResult<(Material, f64)> {
    let m = material(p.str("material")?);
    let ko = p.float("k_override")?;
    Ok((m, if ko > 0.0 { ko } else { m.z_ratio }))
}

/// Flags a Z strength ratio that is a placeholder in the shared material table.
fn note_assumed_z(rep: &mut Report, m: &Material, overridden: bool) {
    if !overridden && m.shared.z_ratio_assumed() {
        rep.note(Severity::Warning, format!("{}; measure it on coupons or set the override", m.shared.z_ratio_note()));
    }
}

fn up_label(up: DVec3) -> String {
    for (v, s) in [(DVec3::Z, "+Z"), (DVec3::NEG_Z, "-Z"), (DVec3::X, "+X"), (DVec3::NEG_X, "-X"), (DVec3::Y, "+Y"), (DVec3::NEG_Y, "-Y")] {
        if v.dot(up) > 0.99998 {
            return s.into();
        }
    }
    format!("({:.2}, {:.2}, {:.2})", up.x, up.y, up.z)
}

// -------------------------------------------------------------- build fit

pub struct BuildFit;

impl NodeOp for BuildFit {
    fn info(&self) -> NodeTypeInfo {
        grid_params(
            info("print.build_fit", "Build Volume Fit", "Test every part against the printer volume in many orientations, keep the best one that fits and write its pose.")
                .emulates("bed-fit check and auto-orient of slicer plates")
                .implemented()
                .input(PortSpec::new("parts", DataKind::Geometry)),
        )
        .param(ParamSpec::choice("search", "Orientation search", "faces", &["axis", "faces", "extended"]).help("Axis directions only; plus the largest face groups; plus 64 evenly spread directions."))
        .param(ParamSpec::bool("z_spin", "Search z-spin", true).help("Also turn each part about the vertical axis to fit a long part along the bed diagonal."))
        .param(ParamSpec::bool("allow_scaling", "Allow scaling", false).help("Shrink a part that fits in no orientation. Scaling changes the part; the report gate fails when it is used."))
        .param(ParamSpec::float("min_scale", "Smallest scale", 0.5, 0.1, 1.0).help("Scaling below this counts as not fitting."))
        .param(ParamSpec::choice("placement", "Placement", "centre", &["centre", "corner"]).help("Centre each part on the bed, or put its minimum corner at the margin."))
        .param(ParamSpec::choice("layout", "Layout", "row", &["row", "bed"]).help("Row lines the placed parts up side by side for viewing and export; bed leaves each part at its own bed pose."))
        .param(ParamSpec::float("layer_height", "Layer height", 0.2, 0.05, 0.6).unit("mm").help("Used by the orientation score that ranks poses that fit."))
        .param(ParamSpec::float("overhang_angle", "Support threshold", 45.0, 20.0, 70.0).unit("deg").help("Faces pointing down within this angle of straight down count as overhang."))
        .param(ParamSpec::vec3("load_dir", "Load direction", DVec3::ZERO).help("Direction of the main service load; zero means none."))
        .param(ParamSpec::choice("material", "Material", "pla", &MATERIAL_IDS).help("Sets the Z strength ratio used when a load direction is given."))
        .output(PortSpec::new("parts", DataKind::Geometry))
        .output(PortSpec::new("placement", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let bed = read_bed(p)?;
        let margin = p.float("margin")?;
        if (bed - DVec3::splat(2.0 * margin)).min_element() <= 0.0 {
            return Err(ctx.fail("the margin leaves no usable bed volume"));
        }
        let parts = parts_of(&geo);
        if parts.is_empty() {
            return Err(ctx.fail("the input holds no closed faces to fit"));
        }
        let load = p.vec3("load_dir")?;
        let opts = FitOptions {
            bed,
            margin,
            search: Search::parse(p.str("search")?),
            z_spin: p.bool("z_spin")?,
            allow_scaling: p.bool("allow_scaling")?,
            min_scale: p.float("min_scale")?,
            placement: if p.str("placement")? == "corner" { Placement::Corner } else { Placement::Centre },
            orient: OrientParams {
                layer_h: p.float("layer_height")?,
                theta_o_deg: p.float("overhang_angle")?,
                loads: if load.length() > 0.0 { vec![load] } else { vec![] },
                k: material(p.str("material")?).z_ratio,
                weights: DEFAULT_WEIGHTS,
            },
        };
        let row = p.str("layout")? == "row";
        let mut rep = Report::new("Build volume fit");
        if load.length() > 0.0 {
            note_assumed_z(&mut rep, &material(p.str("material")?), false);
        }
        let mut placed = Vec::new();
        let mut entries = Vec::new();
        let (mut min_margin, mut min_scale, mut fails) = (f64::INFINITY, 1.0f64, 0usize);
        let mut cursor = 0.0;
        for (i, (id, pg)) in parts.iter().enumerate() {
            let tris = Tris::new(pg);
            let Some(r) = fit_part(&pg.points, &tris, &opts) else {
                return Err(ctx.fail(format!("part {id} has no faces to fit")));
            };
            let mut g = pg.clone();
            g.transform(r.matrix);
            let mut applied = r.matrix;
            if row {
                let b = g.bounds();
                let shift = DVec3::new(cursor - b.min.x, -b.min.y, 0.0);
                g.transform(DMat4::from_translation(shift));
                applied = DMat4::from_translation(shift) * r.matrix;
                cursor += b.size().x + 10.0;
            }
            min_margin = min_margin.min(r.margin);
            min_scale = min_scale.min(r.scale);
            if !r.fits {
                fails += 1;
                rep.note(Severity::Error, format!("part {id} does not fit: short by {:.1} mm in the best orientation", -r.margin));
            } else if r.scale < 1.0 {
                rep.note(Severity::Warning, format!("part {id} was scaled by {:.3} to fit", r.scale));
            }
            rep.metric(format!("part.{i}.fit_margin_mm"), Metric::new(r.margin, "mm"));
            rep.metric(format!("part.{i}.area_mm2"), Metric::new(pg.surface_area(), "mm2"));
            entries.push(json!({
                "part": id,
                "fits": r.fits,
                "margin_mm": r.margin,
                "scale": r.scale,
                "up": v3_json(r.up),
                "up_label": up_label(r.up),
                "spin_deg": r.spin.to_degrees(),
                "extent_mm": v3_json(r.extent),
                "bbox_min": v3_json(r.bbox_min),
                "bbox_max": v3_json(r.bbox_max),
                "score": r.score,
                "candidates": r.candidates,
                "bed_matrix": matrix_rows(&r.matrix),
                "matrix": matrix_rows(&applied),
            }));
            placed.push(g);
        }
        rep.metric("part_count", Metric::new(parts.len() as f64, "count"));
        rep.metric("fit_margin_mm", Metric::new(min_margin, "mm").with_limit(0.0, false));
        rep.metric("fail_count", Metric::new(fails as f64, "count").with_limit(0.0, true));
        rep.metric("scale_factor", Metric::new(min_scale, "ratio").with_limit(1.0, false));
        rep.note(Severity::Info, format!("usable volume {:.0} x {:.0} x {:.0} mm", opts.usable().x, opts.usable().y, opts.usable().z));
        let out = json!({"matrix_layout": "row-major 4x4, column vectors, mm", "usable_mm": v3_json(opts.usable()), "parts": entries});
        Ok(vec![Data::geometry(mesh::merge_all(&placed)), Data::json(out), Data::report(rep)])
    }
}

// ----------------------------------------------------------- orient score

pub struct OrientScoreNode;

impl NodeOp for OrientScoreNode {
    fn info(&self) -> NodeTypeInfo {
        material_param(
            info("print.orient_score", "Orientation Score", "Rank build directions by height, overhang, stair-step error and how well the main load sits in the layer plane.")
                .emulates("auto-orient helpers of slicers, with a strength term")
                .implemented()
                .input(PortSpec::new("part", DataKind::Geometry)),
        )
        .param(ParamSpec::int("part", "Part id", -1, -1, 100000).help("Score only this part id; -1 scores the whole input as one body."))
        .param(ParamSpec::float("layer_height", "Layer height", 0.2, 0.05, 0.6).unit("mm").help("Layer thickness used for layer count and stair-step."))
        .param(ParamSpec::float("overhang_angle", "Support threshold", 45.0, 20.0, 70.0).unit("deg").help("Faces pointing down within this angle of straight down count as overhang."))
        .param(ParamSpec::choice("search", "Orientation search", "extended", &["axis", "faces", "extended"]).help("Which build directions to try."))
        .param(ParamSpec::vec3("load_dir", "Load direction", DVec3::X).help("Direction of the main service load; zero means none."))
        .param(ParamSpec::float("force_n", "Load", 0.0, 0.0, 1e6).unit("N").help("Load magnitude; with a section area it gives the utilisation."))
        .param(ParamSpec::float("area_mm2", "Load section area", 0.0, 0.0, 1e7).unit("mm2").help("Cross-section carrying the load."))
        .param(ParamSpec::float("safety_factor", "Safety factor", 2.0, 1.0, 5.0).help("Applied to the in-plane allowable."))
        .param(ParamSpec::float("w_overhang", "Weight: overhang", DEFAULT_WEIGHTS[0], 0.0, 100.0).help("Weight of the overhang area fraction."))
        .param(ParamSpec::float("w_stair", "Weight: stair-step", DEFAULT_WEIGHTS[1], 0.0, 100.0).help("Weight of the mean stair-step fraction."))
        .param(ParamSpec::float("w_load", "Weight: load", DEFAULT_WEIGHTS[2], 0.0, 100.0).help("Weight of the inverse allowable ratio."))
        .param(ParamSpec::float("w_height", "Weight: height", DEFAULT_WEIGHTS[3], 0.0, 100.0).help("Weight of the height as a fraction of the body diagonal."))
        .output(PortSpec::new("ranked", DataKind::Json))
        .output(PortSpec::new("oriented", DataKind::Geometry))
        .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let part = p.int("part")?;
        let body = if part >= 0 { mesh::extract_part(&geo, part) } else { (*geo).clone() };
        let tris = Tris::new(&body);
        if tris.v.is_empty() {
            return Err(ctx.fail("there are no faces to score"));
        }
        let (mat, k) = material_k(p)?;
        let load = p.vec3("load_dir")?;
        let op = OrientParams {
            layer_h: p.float("layer_height")?,
            theta_o_deg: p.float("overhang_angle")?,
            loads: if load.length() > 0.0 { vec![load] } else { vec![] },
            k,
            weights: [p.float("w_overhang")?, p.float("w_stair")?, p.float("w_load")?, p.float("w_height")?],
        };
        let diag = tris.diagonal();
        let mut scores: Vec<orient::OrientScore> =
            orient::candidate_ups(&tris, Search::parse(p.str("search")?)).into_iter().map(|u| orient::score(&tris, diag, u, &op)).collect();
        scores.sort_by(|a, b| a.total.total_cmp(&b.total));
        let best = scores[0].clone();
        let ranked: Vec<_> = scores
            .iter()
            .take(50)
            .enumerate()
            .map(|(i, s)| {
                json!({"rank": i + 1, "up": v3_json(s.up), "label": up_label(s.up), "height_mm": s.height, "layers": s.layers,
                       "overhang_area_mm2": s.overhang_area, "stair_step_mean": s.cusp_mean, "load_ratio": s.r_min, "score": s.total})
            })
            .collect();
        let mut oriented = (*geo).clone();
        let rot = DMat4::from_mat3(orient::rotation_to_z(best.up));
        oriented.transform(rot);
        let lo = oriented.bounds().min;
        oriented.transform(DMat4::from_translation(DVec3::new(0.0, 0.0, -lo.z)));

        let mut rep = Report::new("Orientation score");
        note_assumed_z(&mut rep, &mat, p.float("k_override")? > 0.0);
        rep.metric("candidate_count", Metric::new(scores.len() as f64, "count"));
        rep.metric("height_mm", Metric::new(best.height, "mm"));
        rep.metric("layer_count", Metric::new(best.layers as f64, "count"));
        rep.metric("overhang_area_mm2", Metric::new(best.overhang_area, "mm2"));
        rep.metric("stair_step_mm", Metric::new(best.cusp_mean * op.layer_h, "mm"));
        rep.metric("load_ratio", Metric::new(best.r_min, "ratio"));
        rep.metric("orient_score", Metric::new(best.total, "score"));
        let (f, a) = (p.float("force_n")?, p.float("area_mm2")?);
        if f > 0.0 && a > 0.0 {
            let util = f * p.float("safety_factor")? / (a * best.r_min * mat.sigma_xy_mpa);
            rep.metric("utilisation", Metric::new(util, "ratio").with_limit(1.0, true));
        }
        rep.note(Severity::Info, format!("best build direction {} ({} layers)", up_label(best.up), best.layers));
        rep.note(Severity::Info, "strength ratios are a rough prior from one printer and stock profiles, not design allowables");
        Ok(vec![Data::json(json!({"best": {"up": v3_json(best.up), "label": up_label(best.up)}, "ranked": ranked})), Data::geometry(oriented), Data::report(rep)])
    }
}

// ------------------------------------------------------ directional strength

pub struct DirectionalStrength;

impl NodeOp for DirectionalStrength {
    fn info(&self) -> NodeTypeInfo {
        material_param(
            info("print.directional_strength", "Directional Strength Check", "Compare the section needed when a load runs along the layers with the section needed when it runs across them.")
                .emulates("anisotropy rule of thumb for printed mounts")
                .implemented(),
        )
        .param(ParamSpec::float("sigma_xy", "In-plane allowable override", 0.0, 0.0, 1000.0).unit("MPa").help("0 uses the material table."))
        .param(ParamSpec::float("force_n", "Load", 500.0, 0.0, 1e7).unit("N").help("Tensile load on the section."))
        .param(ParamSpec::float("safety_factor", "Safety factor", 2.0, 1.0, 5.0).help("Divides the allowable."))
        .param(ParamSpec::float("area_mm2", "Section area", 0.0, 0.0, 1e7).unit("mm2").help("Actual section; 0 skips the utilisation."))
        .param(ParamSpec::vec3("load_dir", "Load direction", DVec3::Z).help("Direction of the load."))
        .param(ParamSpec::vec3("build_dir", "Build direction", DVec3::Z).help("Direction the layers stack."))
        .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, ctx: &CookContext, _inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let (mat, k) = material_k(p)?;
        let so = p.float("sigma_xy")?;
        let sigma_xy = if so > 0.0 { so } else { mat.sigma_xy_mpa };
        let sf = p.float("safety_factor")?;
        let f = p.float("force_n")?;
        if sigma_xy <= 0.0 || sf <= 0.0 {
            return Err(ctx.fail("allowable and safety factor must be positive"));
        }
        let s = orient::strength(f, sf, sigma_xy, k);
        let r = orient::load_ratio(p.vec3("load_dir")?, p.vec3("build_dir")?, k);
        let mut rep = Report::new("Directional strength");
        note_assumed_z(&mut rep, &mat, p.float("k_override")? > 0.0);
        rep.metric("z_allowable_ratio", Metric::new(k, "ratio"));
        rep.metric("sigma_xy_mpa", Metric::new(sigma_xy, "MPa"));
        rep.metric("sigma_z_mpa", Metric::new(s.sigma_z, "MPa"));
        rep.metric("required_area_inplane_mm2", Metric::new(s.area_inplane, "mm2"));
        rep.metric("required_area_z_mm2", Metric::new(s.area_z, "mm2"));
        rep.metric("area_penalty", Metric::new(s.area_penalty, "ratio"));
        rep.metric("load_ratio", Metric::new(r, "ratio"));
        let a = p.float("area_mm2")?;
        if a > 0.0 {
            rep.metric("utilisation", Metric::new(f * sf / (a * r * sigma_xy), "ratio").with_limit(1.0, true));
        }
        rep.note(Severity::Info, "put service loads in the layer plane and size mounts against the Z allowable");
        if k < 1.0 && (p.vec3("load_dir")?.normalize_or_zero().dot(p.vec3("build_dir")?.normalize_or_zero())).abs() > 0.7 {
            rep.note(Severity::Warning, "the load runs mostly across the layers");
        }
        Ok(vec![Data::report(rep)])
    }
}

// ------------------------------------------------------------- heat margin

pub struct HeatMargin;


impl NodeOp for HeatMargin {
    fn info(&self) -> NodeTypeInfo {
        info("print.heat_margin", "Heat Margin Gate", "Compare the sunlit surface temperature with the heat deflection temperature of the material, minus a safety margin.")
            .emulates("filament datasheet heat deflection check")
            .implemented()
            .input(PortSpec::optional("thermal", DataKind::Report))
            .param(ParamSpec::choice("material", "Material", "asa", &MATERIAL_IDS).help("Filament family."))
            .param(ParamSpec::bool("annealed", "Annealed", false).help("Use the post-annealing deflection temperature where the material has one."))
            .param(ParamSpec::float("safety_k", "Safety margin", 10.0, 0.0, 30.0).unit("K").help("Required distance below the deflection temperature."))
            .param(ParamSpec::float("surface_temp_c", "Surface temperature", 0.0, 0.0, 300.0).unit("deg C").help("0 takes the value from the connected report, or computes it from the heat balance below."))
            .param(ParamSpec::float("ambient_c", "Ambient temperature", 45.0, -20.0, 60.0).unit("deg C").help("Air temperature."))
            .param(ParamSpec::float("wind_ms", "Wind speed", 0.0, 0.0, 40.0).unit("m/s").help("Air speed over the panel."))
            .param(ParamSpec::float("absorptivity", "Solar absorptivity", 0.95, 0.0, 1.0).help("Fraction of sunlight absorbed; dark paint is high."))
            .param(ParamSpec::float("irradiance", "Irradiance", 1000.0, 0.0, 1400.0).unit("W/m2").help("Sunlight power on the panel."))
            .param(ParamSpec::float("emissivity", "Emissivity", 0.9, 0.0, 1.0).help("Infrared emissivity of the surface."))
            .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, _ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let mat = material(p.str("material")?);
        let hdt = if p.bool("annealed")? { mat.hdt_annealed_c } else { mat.hdt_c };
        let mut rep = Report::new("Heat margin");
        let from_param = p.float("surface_temp_c")?;
        let thermal = inputs::opt_report(inp, 0);
        let from_input = thermal.as_ref().and_then(|r| r.first_metric(&keys::SURFACE_TEMP_ALIASES)).map(|(_, v)| v);
        if thermal.is_some() && from_input.is_none() && from_param <= 0.0 {
            rep.note(Severity::Warning, format!("the connected report has no surface temperature ({}); the heat balance below is used", keys::SURFACE_TEMP_ALIASES.join(", ")));
        }
        let (ts, source) = if from_param > 0.0 {
            (from_param, "parameter")
        } else if let Some(t) = from_input {
            (t, "connected report")
        } else {
            let h = HeatInput {
                ambient_c: p.float("ambient_c")?,
                wind_ms: p.float("wind_ms")?,
                absorptivity: p.float("absorptivity")?,
                irradiance: p.float("irradiance")?,
                emissivity: p.float("emissivity")?,
                ..Default::default()
            };
            (orient::surface_temp_c(&h), "heat balance")
        };
        let safety = p.float("safety_k")?;
        rep.metric(keys::SURFACE_TEMP_C, Metric::new(ts, "deg C"));
        rep.metric(keys::HDT_C, Metric::new(hdt, "deg C"));
        rep.metric("raw_margin_k", Metric::new(hdt - ts, "K"));
        rep.metric("heat_margin_k", Metric::new(hdt - ts - safety, "K").with_limit(0.0, false));
        rep.note(Severity::Info, format!("surface temperature from {source}"));
        if hdt - ts - safety < 0.0 {
            rep.note(Severity::Error, format!("{} is {:.1} K short of the required margin", mat.id, -(hdt - ts - safety)));
        }
        if mat.hdt_annealed_c > mat.hdt_c && !p.bool("annealed")? {
            rep.note(Severity::Info, "this material gains heat resistance when annealed");
        }
        Ok(vec![Data::report(rep)])
    }
}

// -------------------------------------------------------------- stair step

pub struct StairStep;

/// Per-prim stair-step fraction (|n.u|, or 0 for flat or non-faces) and flag.
pub fn stair_fractions(geo: &Geometry, up: DVec3, threshold: f64) -> (Vec<f64>, Vec<i64>, f64, f64) {
    let u = up.normalize_or_zero();
    let flat_cos = orient::FLAT_SLOPE_DEG.to_radians().cos();
    let (mut frac, mut flag) = (vec![0.0; geo.prims.len()], vec![0i64; geo.prims.len()]);
    let (mut flagged_area, mut total) = (0.0, 0.0);
    for (i, pr) in geo.prims.iter().enumerate() {
        if !pr.closed || pr.vertices.len() < 3 {
            continue;
        }
        let a = geo.prim_area(i);
        total += a;
        let c = geo.prim_normal(i).dot(u).abs();
        frac[i] = if c > flat_cos { 0.0 } else { c };
        if frac[i] > threshold {
            flag[i] = 1;
            flagged_area += a;
        }
    }
    (frac, flag, flagged_area, total)
}

impl NodeOp for StairStep {
    fn info(&self) -> NodeTypeInfo {
        info("print.stair_step", "Stair-Step Estimate", "Mark faces where layer stepping will show on shallow slopes for a given build direction.")
            .emulates("stair-step preview of slicers")
            .implemented()
            .input(PortSpec::new("part", DataKind::Geometry))
            .param(ParamSpec::vec3("up", "Build direction", DVec3::Z).help("Direction the layers stack."))
            .param(ParamSpec::float("layer_height", "Layer height", 0.2, 0.05, 0.6).unit("mm").help("Layer thickness."))
            .param(ParamSpec::float("threshold", "Threshold", 0.3, 0.05, 1.0).unit("x layer height").help("Faces whose step exceeds this share of the layer height are flagged."))
            .output(PortSpec::new("part", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let up = p.vec3("up")?;
        if up.length() < 1e-12 {
            return Err(ctx.fail("build direction must not be zero"));
        }
        let h = p.float("layer_height")?;
        let thr = p.float("threshold")?;
        let (frac, flag, flagged, total) = stair_fractions(&geo, up, thr);
        let mut out = (*geo).clone();
        let n = out.prims.len();
        let _ = out.prim_attribs.ensure("stair_step", AttribType::Float, n);
        out.prim_attribs.set("stair_step", AttribArray::Float(frac.clone()));
        out.prim_attribs.set("stair_flag", AttribArray::Int(flag));
        let mut rep = Report::new("Stair-step estimate");
        rep.metric("area_over_threshold_mm2", Metric::new(flagged, "mm2"));
        rep.metric("flagged_fraction", Metric::new(if total > 0.0 { flagged / total } else { 0.0 }, "ratio"));
        rep.metric("max_step_mm", Metric::new(frac.iter().copied().fold(0.0, f64::max) * h, "mm"));
        rep.metric("cutoff_slope_deg", Metric::new(thr.clamp(0.0, 1.0).acos().to_degrees(), "deg"));
        rep.note(Severity::Info, "the step of a face is the layer height times |n.u|; faces flatter than half a degree are exact layer planes");
        Ok(vec![Data::geometry(out), Data::report(rep)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testutil::cook;
    use crate::mesh::testutil::tri_box;
    use vernon_graph::ParamValue;

    fn geo_in(g: Geometry) -> Vec<Option<Data>> {
        vec![Some(Data::geometry(g))]
    }

    fn metric(d: &Data, name: &str) -> f64 {
        d.as_report().unwrap().metrics[name].value
    }

    #[test]
    fn build_fit_default_box_and_failure_gate() {
        let g = tri_box(DVec3::ZERO, DVec3::new(120.0, 30.0, 20.0));
        let set: &[(&str, ParamValue)] =
            &[("printer", ParamValue::Str("custom".into())), ("bed", ParamValue::Vec3(DVec3::new(100.0, 100.0, 100.0)))];
        let out = cook(&BuildFit, "fit_low", &geo_in(g.clone()), set).unwrap();
        assert_eq!(metric(&out[2], "fail_count"), 1.0);
        assert!(out[2].as_report().unwrap().metrics["fit_margin_mm"].passes() == Some(false));
        let set2: &[(&str, ParamValue)] =
            &[("printer", ParamValue::Str("custom".into())), ("bed", ParamValue::Vec3(DVec3::new(100.0, 100.0, 150.0)))];
        let out = cook(&BuildFit, "fit_tall", &geo_in(g), set2).unwrap();
        assert_eq!(metric(&out[2], "fail_count"), 0.0);
        assert!(metric(&out[2], "fit_margin_mm") >= 0.0);
        let j = out[1].as_json().unwrap();
        assert_eq!(j["parts"][0]["matrix"].as_array().unwrap().len(), 16);
    }

    #[test]
    fn build_fit_places_parts_in_a_row_without_overlap() {
        let mut g = tri_box(DVec3::ZERO, DVec3::new(50.0, 40.0, 30.0));
        let mut g2 = tri_box(DVec3::new(200.0, 0.0, 0.0), DVec3::new(60.0, 40.0, 20.0));
        let n1 = g.prims.len();
        mesh::set_int_prim_attr(&mut g, "part", vec![0; n1]);
        let n2 = g2.prims.len();
        mesh::set_int_prim_attr(&mut g2, "part", vec![1; n2]);
        g.merge(&g2);
        let out = cook(&BuildFit, "fit_row", &geo_in(g), &[]).unwrap();
        let placed = out[0].as_geometry().unwrap();
        let (a, b) = (mesh::extract_part(placed, 0).bounds(), mesh::extract_part(placed, 1).bounds());
        assert!(a.max.x < b.min.x || b.max.x < a.min.x);
        assert_eq!(metric(&out[2], "part_count"), 2.0);
    }

    #[test]
    fn orient_score_strip_prefers_flat_for_in_plane_load() {
        let g = tri_box(DVec3::ZERO, DVec3::new(100.0, 20.0, 3.0));
        let out = cook(&OrientScoreNode, "or", &geo_in(g), &[("material", ParamValue::Str("asa".into())), ("search", ParamValue::Str("axis".into()))]).unwrap();
        let best = &out[0].as_json().unwrap()["best"];
        assert_eq!(best["label"], "+Z");
        let ranked = out[0].as_json().unwrap()["ranked"].as_array().unwrap().len();
        assert_eq!(ranked, 6);
        let oriented = out[1].as_geometry().unwrap();
        assert!((oriented.bounds().min.z).abs() < 1e-9);
        assert_eq!(metric(&out[2], "layer_count"), 15.0);
    }

    #[test]
    fn strength_node_golden() {
        let out = cook(&DirectionalStrength, "st", &[], &[("material", ParamValue::Str("ppa_cf".into()))]).unwrap();
        assert!((metric(&out[0], "required_area_inplane_mm2") - 5.952).abs() < 1e-3);
        assert!((metric(&out[0], "required_area_z_mm2") - 17.544).abs() < 1e-3);
        assert!((metric(&out[0], "area_penalty") - 2.947).abs() < 1e-3);
    }

    #[test]
    fn strength_reads_the_shared_table_and_flags_assumed_z_ratios() {
        let notes = |mat: &str, ko: f64| {
            let out = cook(&DirectionalStrength, "st", &[], &[("material", ParamValue::Str(mat.into())), ("k_override", ParamValue::Float(ko))]).unwrap();
            let r = out[0].as_report().unwrap().clone();
            (r.metrics["z_allowable_ratio"].value, r.notes.iter().any(|(s, t)| *s == Severity::Warning && t.contains("assumed")))
        };
        // same Z ratio the structural stage uses (ASA 29 %), sourced, so no flag
        assert_eq!(notes("asa", 0.0), (vernon_core::materials::ASA.z_ratio, false));
        assert_eq!(material("asa").hdt_c, vernon_core::materials::ASA.hdt_045_c);
        // an unsourced ratio is flagged unless overridden
        assert_eq!(notes("pet_cf", 0.0), (0.40, true));
        assert_eq!(notes("pet_cf", 0.5), (0.5, false));
    }

    fn heat(material: &str, absorptivity: f64, safety: f64, annealed: bool) -> Report {
        let out = cook(
            &HeatMargin,
            "heat",
            &[],
            &[
                ("material", ParamValue::Str(material.into())),
                ("absorptivity", ParamValue::Float(absorptivity)),
                ("safety_k", ParamValue::Float(safety)),
                ("annealed", ParamValue::Bool(annealed)),
            ],
        )
        .unwrap();
        (**out[0].as_report().unwrap()).clone()
    }

    #[test]
    fn heat_gate_golden_verdicts() {
        let r = heat("asa", 0.95, 0.0, false);
        assert!((r.metrics["heat_margin_k"].value + 4.7).abs() < 0.5);
        assert!(!r.passes());
        let r = heat("pet_cf", 0.95, 0.0, false);
        assert!((r.metrics["heat_margin_k"].value - 100.3).abs() < 0.5);
        assert!(r.passes());
        assert!(heat("petg", 0.30, 0.0, false).passes());
        assert!(!heat("petg", 0.30, 10.0, false).passes());
        assert!(!heat("pla", 0.60, 0.0, false).passes());
        assert!(!heat("paht_cf", 0.95, 0.0, false).passes());
        assert!(heat("paht_cf", 0.95, 0.0, true).passes());
    }

    #[test]
    fn heat_takes_temperature_from_connected_report() {
        let mut up = Report::new("solar");
        up.metric("max_surface_temp_c", Metric::new(70.0, "deg C"));
        let out = cook(&HeatMargin, "heat_in", &[Some(Data::report(up))], &[("material", ParamValue::Str("pla".into()))]).unwrap();
        assert_eq!(metric(&out[0], "surface_temp_c"), 70.0);
        assert!(!out[0].as_report().unwrap().passes());
    }

    #[test]
    fn stair_step_cutoff_golden() {
        let u = DVec3::Z;
        // facet normals tilted from vertical by the slope angle
        let tri = |slope_deg: f64| {
            let s = slope_deg.to_radians();
            let n = DVec3::new(s.sin(), 0.0, s.cos()); // slope measured from horizontal
            let (a, b) = mesh::plane_frame(n);
            Geometry::from_polygons(vec![DVec3::ZERO, a, b], vec![vec![0, 1, 2]])
        };
        let f = |slope: f64| {
            let g = tri(slope);
            let (frac, flag, _, _) = stair_fractions(&g, u, 0.3);
            (frac[0], flag[0])
        };
        let (c10, f10) = f(10.0);
        assert!((c10 - 10f64.to_radians().cos()).abs() < 1e-9 && f10 == 1);
        let (c80, f80) = f(80.0);
        assert!((c80 - 0.1736).abs() < 1e-3 && f80 == 0);
        assert_eq!(f(0.0).1, 0); // exactly flat: no step
        assert_eq!(f(72.0).1, 1);
        assert_eq!(f(73.0).1, 0);
    }

    #[test]
    fn stair_step_node_reports_cutoff() {
        let g = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        let out = cook(&StairStep, "ss", &geo_in(g), &[]).unwrap();
        assert!((metric(&out[1], "cutoff_slope_deg") - 72.54).abs() < 0.01);
        assert_eq!(metric(&out[1], "area_over_threshold_mm2"), 0.0);
        assert!(out[0].as_geometry().unwrap().prim_attribs.contains("stair_step"));
    }
}

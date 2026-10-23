//! Incident radiation on a mesh from a sky matrix, relative sun exposure,
//! and the UV-dose proxy.

use std::f64::consts::PI;

use glam::DVec3;
use rayon::prelude::*;
use vernon_core::{AttribArray, Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{
    CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec,
    Stage,
};

use crate::bvh::Bvh;
use crate::sky::{expand_patches, tregenza_patches, Patch, SkyMatrix, SubPatch};
use crate::sun::{sun_vector, SunTable};
use crate::util::*;

#[derive(Clone, Debug)]
pub struct RadOptions {
    pub offset_mm: f64,
    pub grid_mm: f64,
    pub exact_sun: bool,
    pub ground: bool,
    pub subs: usize,
    pub flip: bool,
    pub occlusion: bool,
    pub by_vertex: bool,
    pub export_matrix: bool,
}

impl Default for RadOptions {
    fn default() -> Self {
        RadOptions {
            offset_mm: 1.0,
            grid_mm: 0.0,
            exact_sun: false,
            ground: true,
            subs: 1,
            flip: false,
            occlusion: true,
            by_vertex: false,
            export_matrix: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RadResult {
    /// Cumulative energy per primitive, Wh/m2.
    pub e_wh: Vec<f64>,
    pub e_sky: Vec<f64>,
    pub e_sun: Vec<f64>,
    pub e_gnd: Vec<f64>,
    pub normals: Vec<DVec3>,
    pub valid: Vec<bool>,
    pub areas_m2: Vec<f64>,
    /// Per-point energy when analysed per vertex.
    pub point_e: Option<Vec<f64>>,
    pub matrix: Vec<Vec<f32>>,
    pub samples: usize,
}

struct Evaluator {
    bvh: Option<Bvh>,
    subp: Vec<SubPatch>,
    centers: Vec<Patch>,
    lsky: Vec<f64>,
    sun_list: Vec<(DVec3, f64)>,
    lgnd: f64,
    offset: f64,
    ground: bool,
    want_matrix: bool,
}

#[derive(Default)]
struct PointEval {
    sky: f64,
    sun: f64,
    gnd: f64,
    row: Vec<f32>,
}

impl Evaluator {
    fn eval(&self, pos: DVec3, n: DVec3) -> PointEval {
        let o = pos + n * self.offset;
        let visible = |dir: DVec3| -> bool {
            match &self.bvh {
                Some(b) => !b.any_hit(o, dir, 1e-6, f64::INFINITY),
                None => true,
            }
        };
        let mut r = PointEval::default();
        if self.want_matrix {
            r.row = vec![0.0; self.centers.len()];
        }
        for sp in &self.subp {
            let c = n.dot(sp.dir);
            if c <= 1e-12 {
                continue;
            }
            let l = self.lsky[sp.parent];
            if l <= 0.0 && !self.want_matrix {
                continue;
            }
            if !visible(sp.dir) {
                continue;
            }
            r.sky += l * sp.omega * c;
            if self.want_matrix {
                r.row[sp.parent] += (c * sp.omega / self.centers[sp.parent].omega) as f32;
            }
        }
        for (v, b) in &self.sun_list {
            let c = n.dot(*v);
            if c > 1e-12 && visible(*v) {
                r.sun += b * c;
            }
        }
        if self.ground && self.lgnd > 0.0 && n.z < 1.0 - 1e-9 && visible(-DVec3::Z) {
            r.gnd = self.lgnd * PI * (1.0 - n.z) * 0.5;
        }
        r
    }
}

/// Sample positions of a primitive with area weights. A face is split into
/// `k * k` sub-triangles per fan triangle when its longest edge exceeds the
/// grid size (`k` capped at 4).
fn face_samples(g: &Geometry, prim: usize, grid: f64) -> Vec<(DVec3, f64)> {
    let v = &g.prims[prim].vertices;
    let mut out = Vec::new();
    let a0 = g.points[v[0] as usize];
    for i in 1..v.len() - 1 {
        let (b, c) = (g.points[v[i] as usize], g.points[v[i + 1] as usize]);
        let area = (b - a0).cross(c - a0).length() * 0.5;
        if area <= 1e-12 {
            continue;
        }
        let longest = (b - a0)
            .length()
            .max((c - a0).length())
            .max((c - b).length());
        let k = if grid > 0.0 {
            ((longest / grid).ceil() as usize).clamp(1, 4)
        } else {
            1
        };
        let kf = k as f64;
        let w = area / (kf * kf);
        for ii in 0..k {
            for jj in 0..(k - ii) {
                let (u, vv) = (ii as f64 + 1.0 / 3.0, jj as f64 + 1.0 / 3.0);
                out.push((a0 + (b - a0) * (u / kf) + (c - a0) * (vv / kf), w));
                if jj + 1 < k - ii {
                    let (u, vv) = (ii as f64 + 2.0 / 3.0, jj as f64 + 2.0 / 3.0);
                    out.push((a0 + (b - a0) * (u / kf) + (c - a0) * (vv / kf), w));
                }
            }
        }
    }
    out
}

fn collect_tris(g: &Geometry, out: &mut Vec<[DVec3; 3]>) {
    for (_, t) in g.triangles() {
        out.push([
            g.points[t[0] as usize],
            g.points[t[1] as usize],
            g.points[t[2] as usize],
        ]);
    }
}

fn build_bvh(study: &Geometry, context: Option<&Geometry>) -> Option<Bvh> {
    let mut tris = Vec::new();
    collect_tris(study, &mut tris);
    if let Some(c) = context {
        collect_tris(c, &mut tris);
    }
    if tris.is_empty() {
        None
    } else {
        Some(Bvh::build(tris))
    }
}

/// Incident radiation per primitive (and optionally per vertex).
pub fn incident(
    g: &Geometry,
    context: Option<&Geometry>,
    sky: &SkyMatrix,
    o: &RadOptions,
) -> RadResult {
    let centers = tregenza_patches(sky.mf);
    let sun_list: Vec<(DVec3, f64)> = if o.exact_sun {
        sky.sun_hours
            .iter()
            .map(|h| (DVec3::from_array(h.v), h.dni))
            .collect()
    } else {
        centers
            .iter()
            .zip(&sky.bsun)
            .filter(|(_, b)| **b > 0.0)
            .map(|(p, b)| (p.dir, *b))
            .collect()
    };
    let ev = Evaluator {
        bvh: if o.occlusion {
            build_bvh(g, context)
        } else {
            None
        },
        subp: expand_patches(&centers, o.subs),
        centers,
        lsky: sky.lsky.clone(),
        sun_list,
        lgnd: sky.lgnd,
        offset: o.offset_mm,
        ground: o.ground,
        want_matrix: o.export_matrix,
    };
    let np = g.prims.len();
    let sign = if o.flip { -1.0 } else { 1.0 };
    let normals: Vec<DVec3> = (0..np)
        .map(|i| {
            if g.prims[i].closed && g.prims[i].vertices.len() >= 3 {
                g.prim_normal(i) * sign
            } else {
                DVec3::ZERO
            }
        })
        .collect();
    let areas = prim_areas(g);
    let valid: Vec<bool> = (0..np)
        .map(|i| normals[i] != DVec3::ZERO && areas[i] > 1e-12)
        .collect();
    let mut res = RadResult {
        normals: normals.clone(),
        valid: valid.clone(),
        areas_m2: areas.iter().map(|a| a * 1e-6).collect(),
        e_wh: vec![0.0; np],
        e_sky: vec![0.0; np],
        e_sun: vec![0.0; np],
        e_gnd: vec![0.0; np],
        ..Default::default()
    };
    if o.by_vertex {
        let mut pn = vec![DVec3::ZERO; g.points.len()];
        for (i, ok) in valid.iter().enumerate() {
            if *ok {
                for &v in &g.prims[i].vertices {
                    pn[v as usize] += normals[i] * areas[i];
                }
            }
        }
        let pv: Vec<(f64, f64, f64)> = (0..g.points.len())
            .into_par_iter()
            .map(|i| {
                let n = pn[i].normalize_or_zero();
                if n == DVec3::ZERO {
                    return (0.0, 0.0, 0.0);
                }
                let e = ev.eval(g.points[i], n);
                (e.sky, e.sun, e.gnd)
            })
            .collect();
        let point_e: Vec<f64> = pv.iter().map(|e| e.0 + e.1 + e.2).collect();
        for (i, ok) in valid.iter().enumerate() {
            if !*ok {
                continue;
            }
            let vs = &g.prims[i].vertices;
            let m = vs.len() as f64;
            res.e_sky[i] = vs.iter().map(|&v| pv[v as usize].0).sum::<f64>() / m;
            res.e_sun[i] = vs.iter().map(|&v| pv[v as usize].1).sum::<f64>() / m;
            res.e_gnd[i] = vs.iter().map(|&v| pv[v as usize].2).sum::<f64>() / m;
            res.e_wh[i] = res.e_sky[i] + res.e_sun[i] + res.e_gnd[i];
        }
        res.samples = g.points.len();
        res.point_e = Some(point_e);
        return res;
    }
    let per_face: Vec<(f64, f64, f64, Vec<f32>, usize)> = (0..np)
        .into_par_iter()
        .map(|i| {
            if !valid[i] {
                return (0.0, 0.0, 0.0, Vec::new(), 0);
            }
            let samples = face_samples(g, i, o.grid_mm);
            let (mut s, mut u, mut gd, mut ws) = (0.0, 0.0, 0.0, 0.0);
            let mut row: Vec<f32> = Vec::new();
            for (pos, w) in &samples {
                let e = ev.eval(*pos, normals[i]);
                s += e.sky * w;
                u += e.sun * w;
                gd += e.gnd * w;
                ws += w;
                if ev.want_matrix {
                    if row.is_empty() {
                        row = vec![0.0; e.row.len()];
                    }
                    for (r, x) in row.iter_mut().zip(&e.row) {
                        *r += (*x as f64 * w) as f32;
                    }
                }
            }
            if ws > 0.0 {
                for r in row.iter_mut() {
                    *r = (*r as f64 / ws) as f32;
                }
                (s / ws, u / ws, gd / ws, row, samples.len())
            } else {
                (0.0, 0.0, 0.0, row, 0)
            }
        })
        .collect();
    for (i, (s, u, gd, row, n)) in per_face.into_iter().enumerate() {
        res.e_sky[i] = s;
        res.e_sun[i] = u;
        res.e_gnd[i] = gd;
        res.e_wh[i] = s + u + gd;
        res.samples += n;
        if o.export_matrix {
            res.matrix.push(row);
        }
    }
    res
}

fn tri_set_normals_ok(g: &Geometry) -> bool {
    face_count(g) > 0
}

// ---------------------------------------------------------------------------
// solar.incident_radiation
// ---------------------------------------------------------------------------

pub struct IncidentRadiationNode;

impl NodeOp for IncidentRadiationNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("solar.incident_radiation", "Incident Radiation", Stage::Solar)
            .input(PortSpec::new("sky_matrix", DataKind::Json))
            .input(PortSpec::new("study_mesh", DataKind::Geometry))
            .input(PortSpec::optional("context", DataKind::Geometry))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("matrix", DataKind::Json))
            .param(ParamSpec::float("grid_size_mm", "Grid size", 50.0, 0.0, 1.0e5).unit("mm").help("Faces whose longest edge exceeds this are sampled at several points and averaged. 0 analyses each face at its centroid."))
            .param(ParamSpec::float("offset_mm", "Offset", 1.0, 0.0, 1.0e4).unit("mm").help("Test points are lifted off the surface along the normal so the face does not block itself."))
            .param(ParamSpec::bool("irradiance", "Irradiance", false).help("Off: cumulative kWh/m2 drives the colours. On: average W/m2 drives them. Both are always written."))
            .param(ParamSpec::bool("by_vertex", "Per vertex", false).help("Analyse at the vertices instead of at the face centres."))
            .param(ParamSpec::choice("sun_mode", "Sun mode", "patch", &["patch", "exact"]).help("Sun beam through the sky patches, or one ray per daylight hour along the exact sun vector."))
            .param(ParamSpec::choice("patch_subsamples", "Patch subsamples", "1", &["1", "4"]).help("Rays per sky patch for the diffuse part."))
            .param(ParamSpec::choice("ground_mode", "Ground", "hemisphere", &["hemisphere", "none"]).help("Reflected light from a uniform lower hemisphere."))
            .param(ParamSpec::choice("average_over", "Average over", "all", &["all", "daylight"]).help("Hours used to turn energy into average W/m2."))
            .param(ParamSpec::bool("occlusion", "Occlusion", true).help("Cast rays against the study mesh and the context. Off: no shadows."))
            .param(ParamSpec::bool("flip_normals", "Flip normals", false))
            .param(ParamSpec::bool("export_matrix", "Export matrix", false).help("Write one visibility row per face and sky patch to the third output (limited to 20000 faces)."))
            .help("Cumulative or average solar radiation on every face from a sky matrix, with shadows from the mesh itself and optional context geometry. Writes irradiance, energy and a colour ramp.")
            .emulates("Incident radiation study from an EPW-driven sky matrix (ray-cast occlusion, no inter-reflection)")
            .spec(SOLAR_SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let jv = json_in(ins, 0)?;
        let sky = SkyMatrix::from_json(&jv).map_err(|e| ctx.fail(e))?;
        let g = inputs::geometry(ins, 1)?;
        let context = inputs::opt_geometry(ins, 2);
        if !tri_set_normals_ok(&g) {
            return Err(ctx.fail("the study mesh has no faces"));
        }
        let opts = RadOptions {
            offset_mm: p.float("offset_mm")?,
            grid_mm: p.float("grid_size_mm")?,
            exact_sun: p.str("sun_mode")? == "exact",
            ground: p.str("ground_mode")? == "hemisphere",
            subs: p.str("patch_subsamples")?.parse().unwrap_or(1),
            flip: p.bool("flip_normals")?,
            occlusion: p.bool("occlusion")?,
            by_vertex: p.bool("by_vertex")?,
            export_matrix: p.bool("export_matrix")? && g.prims.len() <= 20000,
        };
        let res = incident(&g, context.as_deref(), &sky, &opts);
        let hours = if p.str("average_over")? == "daylight" {
            sky.daylight_hours
        } else {
            sky.n_hours
        }
        .max(1.0);
        let np = g.prims.len();
        let kwh: Vec<f64> = res.e_wh.iter().map(|e| e / 1000.0).collect();
        let wm2: Vec<f64> = res.e_wh.iter().map(|e| e / hours).collect();
        let irradiance = p.bool("irradiance")?;
        let shown = if irradiance { &wm2 } else { &kwh };
        let vmax = shown.iter().cloned().fold(0.0_f64, f64::max).max(1e-12);
        let mut out = (*g).clone();
        out.prim_attribs
            .set("irr_kwh_m2", AttribArray::Float(kwh.clone()));
        out.prim_attribs
            .set("irr_w_m2", AttribArray::Float(wm2.clone()));
        out.prim_attribs
            .set("N", AttribArray::Vec3(res.normals.clone()));
        out.prim_attribs.set(
            "Cd",
            AttribArray::Vec3(shown.iter().map(|v| ramp_seq(v / vmax)).collect()),
        );
        let (pk, pw) = match &res.point_e {
            Some(pe) => (
                pe.iter().map(|e| e / 1000.0).collect::<Vec<_>>(),
                pe.iter().map(|e| e / hours).collect::<Vec<_>>(),
            ),
            None => (prim_to_point(&g, &kwh), prim_to_point(&g, &wm2)),
        };
        let pcd: Vec<DVec3> = (if irradiance { &pw } else { &pk })
            .iter()
            .map(|v| ramp_seq(v / vmax))
            .collect();
        out.point_attribs.set("irr_kwh_m2", AttribArray::Float(pk));
        out.point_attribs.set("irr_w_m2", AttribArray::Float(pw));
        out.point_attribs.set("Cd", AttribArray::Vec3(pcd));
        let mut r = Report::new("Incident radiation");
        let area: f64 = res.areas_m2.iter().sum();
        let total_kwh: f64 = (0..np).map(|i| kwh[i] * res.areas_m2[i]).sum();
        let mean_w = if area > 0.0 {
            (0..np).map(|i| wm2[i] * res.areas_m2[i]).sum::<f64>() / area
        } else {
            0.0
        };
        let mean_kwh = if area > 0.0 { total_kwh / area } else { 0.0 };
        r.metric(
            "peak_w_m2",
            Metric::new(wm2.iter().cloned().fold(0.0, f64::max), "W/m2"),
        );
        r.metric("mean_w_m2", Metric::new(mean_w, "W/m2"));
        r.metric(
            "peak_kwh_m2",
            Metric::new(kwh.iter().cloned().fold(0.0, f64::max), "kWh/m2"),
        );
        r.metric("mean_kwh_m2", Metric::new(mean_kwh, "kWh/m2"));
        r.metric("total_kwh", Metric::new(total_kwh, "kWh"));
        r.metric("period_hours", Metric::new(hours, "h"));
        r.metric(
            "faces",
            Metric::new(res.valid.iter().filter(|v| **v).count() as f64, "count"),
        );
        r.metric(
            "degenerate_faces",
            Metric::new(res.valid.iter().filter(|v| !**v).count() as f64, "count"),
        );
        r.metric("sample_points", Metric::new(res.samples as f64, "count"));
        if sky.sky_model != "isotropic" {
            r.note(Severity::Info, format!("sky model: {}", sky.sky_model));
        }
        if opts.exact_sun && sky.sun_hours.is_empty() {
            r.note(
                Severity::Warning,
                "exact sun mode found no daylight hours in the sky matrix",
            );
        }
        if p.bool("export_matrix")? && !opts.export_matrix {
            r.note(
                Severity::Warning,
                "matrix export skipped: more than 20000 faces",
            );
        }
        let matrix = if opts.export_matrix {
            let rows: Vec<Vec<f32>> = res.matrix.clone();
            Data::json(
                serde_json::json!({ "schema": "intersection_matrix.v1", "patches": 144 * sky.mf * sky.mf + 1, "rows": rows }),
            )
        } else {
            Data::json(
                serde_json::json!({ "schema": "intersection_matrix.v1", "patches": 144 * sky.mf * sky.mf + 1, "rows": [] }),
            )
        };
        Ok(vec![Data::geometry(out), Data::report(r), matrix])
    }
}

// ---------------------------------------------------------------------------
// solar.sun_exposure_map
// ---------------------------------------------------------------------------

pub struct SunExposureNode;

impl NodeOp for SunExposureNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("solar.sun_exposure_map", "Relative Sun Exposure", Stage::Solar)
            .input(PortSpec::new("study_mesh", DataKind::Geometry))
            .input(PortSpec::optional("context", DataKind::Geometry))
            .input(PortSpec::optional("sun", DataKind::Json))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("elevation_deg", "Sun elevation", 45.0, -90.0, 90.0).unit("deg").help("Used when no sun table is connected."))
            .param(ParamSpec::float("azimuth_deg", "Sun azimuth", 180.0, 0.0, 360.0).unit("deg").help("Clockwise from north. Used when no sun table is connected."))
            .param(ParamSpec::float("north_angle_deg", "North angle", 0.0, -360.0, 360.0).unit("deg"))
            .param(ParamSpec::float("dni_w_m2", "Beam irradiance", 900.0, 0.0, 1400.0).unit("W/m2").help("Used for the W/m2 map when the sun table carries no beam values."))
            .param(ParamSpec::choice("weights", "Weights", "uniform", &["uniform", "dni"]).help("Weight each sun position equally or by its beam irradiance."))
            .param(ParamSpec::bool("normalise", "Normalise", true).help("Scale the exposure map so its maximum is 1."))
            .param(ParamSpec::float("offset_mm", "Offset", 1.0, 0.0, 1.0e4).unit("mm"))
            .param(ParamSpec::bool("occlusion", "Shadows", true))
            .help("Relative sun exposure per face: cosine of the incidence angle and shadow, averaged over one or many sun positions. A relative map, not calibrated irradiance.")
            .emulates("Diffuse-direct bake of cosine and shadow with a sun lamp")
            .spec(SOLAR_SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        let context = inputs::opt_geometry(ins, 1);
        if !tri_set_normals_ok(&g) {
            return Err(ctx.fail("the study mesh has no faces"));
        }
        let north = p.float("north_angle_deg")?;
        let dni_default = p.float("dni_w_m2")?;
        let suns: Vec<(DVec3, f64)> = match opt_json_in(ins, 2)? {
            Some(v) => {
                let t = SunTable::from_json(&v).map_err(|e| ctx.fail(e))?;
                t.samples
                    .iter()
                    .filter(|s| s.vec[2] > 0.0)
                    .map(|s| {
                        (
                            DVec3::from_array(s.vec),
                            if s.dni > 0.0 { s.dni } else { dni_default },
                        )
                    })
                    .collect()
            }
            None => {
                let v = sun_vector(p.float("azimuth_deg")?, p.float("elevation_deg")?, north);
                if v.z > 0.0 {
                    vec![(v, dni_default)]
                } else {
                    vec![]
                }
            }
        };
        let uniform = p.str("weights")? == "uniform";
        let bvh = if p.bool("occlusion")? {
            build_bvh(&g, context.as_deref())
        } else {
            None
        };
        let offset = p.float("offset_mm")?;
        let np = g.prims.len();
        let areas = prim_areas(&g);
        let normals: Vec<DVec3> = (0..np)
            .map(|i| {
                if g.prims[i].closed && g.prims[i].vertices.len() >= 3 {
                    g.prim_normal(i)
                } else {
                    DVec3::ZERO
                }
            })
            .collect();
        let vals: Vec<(f64, f64)> = (0..np)
            .into_par_iter()
            .map(|i| {
                let n = normals[i];
                if n == DVec3::ZERO || areas[i] <= 1e-12 || suns.is_empty() {
                    return (0.0, 0.0);
                }
                let vs = &g.prims[i].vertices;
                let c = vs.iter().map(|&v| g.points[v as usize]).sum::<DVec3>() / vs.len() as f64;
                let o = c + n * offset;
                let (mut ex, mut wsum, mut irr) = (0.0, 0.0, 0.0);
                for (s, dni) in &suns {
                    let w = if uniform { 1.0 } else { *dni };
                    wsum += w;
                    let cos = n.dot(*s);
                    let blocked = bvh
                        .as_ref()
                        .is_some_and(|b| b.any_hit(o, *s, 1e-6, f64::INFINITY));
                    if cos > 1e-12 && !blocked {
                        ex += w * cos;
                        irr += dni * cos;
                    }
                }
                (
                    if wsum > 0.0 { ex / wsum } else { 0.0 },
                    irr / suns.len() as f64,
                )
            })
            .collect();
        let mut expo: Vec<f64> = vals.iter().map(|v| v.0).collect();
        let irr: Vec<f64> = vals.iter().map(|v| v.1).collect();
        let raw_max = expo.iter().cloned().fold(0.0, f64::max);
        if p.bool("normalise")? && raw_max > 1e-12 {
            for e in expo.iter_mut() {
                *e /= raw_max;
            }
        }
        let mut out = (*g).clone();
        out.prim_attribs
            .set("sun_exposure", AttribArray::Float(expo.clone()));
        out.prim_attribs
            .set("irr_w_m2", AttribArray::Float(irr.clone()));
        out.prim_attribs
            .set("N", AttribArray::Vec3(normals.clone()));
        out.prim_attribs.set(
            "Cd",
            AttribArray::Vec3(expo.iter().map(|e| ramp_seq(*e)).collect()),
        );
        let pe = prim_to_point(&g, &expo);
        out.point_attribs.set(
            "Cd",
            AttribArray::Vec3(pe.iter().map(|e| ramp_seq(*e)).collect()),
        );
        out.point_attribs
            .set("sun_exposure", AttribArray::Float(pe));
        let total: f64 = areas.iter().sum();
        let lit: f64 = (0..np).filter(|&i| expo[i] > 1e-6).map(|i| areas[i]).sum();
        let zero_normals = (0..np)
            .filter(|&i| g.prims[i].closed && normals[i] == DVec3::ZERO)
            .count();
        let mut r = Report::new("Sun exposure");
        r.metric(
            "peak_w_m2",
            Metric::new(irr.iter().cloned().fold(0.0, f64::max), "W/m2"),
        );
        r.metric(
            "lit_area_pct",
            Metric::new(
                if total > 0.0 {
                    100.0 * lit / total
                } else {
                    0.0
                },
                "%",
            ),
        );
        r.metric("sun_positions", Metric::new(suns.len() as f64, "count"));
        r.metric(
            "zero_normal_faces",
            Metric::new(zero_normals as f64, "count"),
        );
        if suns.is_empty() {
            r.note(
                Severity::Warning,
                "no sun position above the horizon: the map is all zero",
            );
        }
        Ok(vec![Data::geometry(out), Data::report(r)])
    }
}

// ---------------------------------------------------------------------------
// solar.uv_dose_proxy
// ---------------------------------------------------------------------------

pub struct UvDoseNode;

impl NodeOp for UvDoseNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("solar.uv_dose_proxy", "UV Dose Proxy", Stage::Solar)
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("f_uv", "UV fraction", 0.045, 0.0, 0.2).help("Share of global irradiance that is ultraviolet. A few percent."))
            .param(ParamSpec::bool("label_as_proxy", "Label as proxy", true).help("Add a note that the result is proportional to total radiation, not a spectral UV result."))
            .help("Scales a face-wise annual radiation map by a fixed ultraviolet fraction to give a proportional UV dose estimate.")
            .emulates("UV-dose proxy remark of the report")
            .spec(SOLAR_SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        let src = g.prim_attribs.floats("irr_kwh_m2").map_err(|_| {
            ctx.fail("the mesh has no irr_kwh_m2 attribute; run Incident Radiation first")
        })?;
        let f = p.float("f_uv")?;
        let uv: Vec<f64> = src.iter().map(|v| v * f).collect();
        let mut out = (*g).clone();
        out.prim_attribs
            .set("uv_kwh_m2", AttribArray::Float(uv.clone()));
        let pe = prim_to_point(&g, &uv);
        out.point_attribs.set("uv_kwh_m2", AttribArray::Float(pe));
        let areas = prim_areas(&g);
        let atot: f64 = areas.iter().sum();
        let mean = if atot > 0.0 {
            uv.iter().zip(&areas).map(|(u, a)| u * a).sum::<f64>() / atot
        } else {
            0.0
        };
        let mut r = Report::new("UV dose proxy");
        r.metric(
            "max_uv_kwh_m2",
            Metric::new(uv.iter().cloned().fold(0.0, f64::max), "kWh/m2"),
        );
        r.metric("mean_uv_kwh_m2", Metric::new(mean, "kWh/m2"));
        r.metric("f_uv", Metric::new(f, ""));
        if p.bool("label_as_proxy")? {
            r.note(
                Severity::Info,
                "proxy only: proportional to total radiation, not a spectral ultraviolet result",
            );
        }
        Ok(vec![Data::geometry(out), Data::report(r)])
    }
}

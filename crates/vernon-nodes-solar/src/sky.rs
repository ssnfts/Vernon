//! Tregenza/Reinhart sky patches and the cumulative sky matrix.

use std::f64::consts::PI;

use glam::DVec3;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use vernon_core::{AttribArray, Geometry, Metric, Report, Severity};
use vernon_graph::{
    CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params,
    PortSpec, Stage,
};

use crate::sun::{alg_of, sun_position_alg, sun_vector, Site, SunAlg, DEG};
use crate::util::*;
use crate::weather::Weather;

#[derive(Clone, Copy, Debug)]
pub struct Patch {
    pub alt_deg: f64,
    pub az_deg: f64,
    /// Solid angle, sr.
    pub omega: f64,
    pub dir: DVec3,
    /// Angular extent (altitude, azimuth) in degrees, used for sub-sampling.
    pub width_alt: f64,
    pub width_az: f64,
}

fn dir_of(az_deg: f64, alt_deg: f64) -> DVec3 {
    let (a, h) = (az_deg * DEG, alt_deg * DEG);
    DVec3::new(a.sin() * h.cos(), a.cos() * h.cos(), h.sin())
}

/// Tregenza patches for subdivision factor `mf` (1, 2, 4 give 145, 577, 2305).
pub fn tregenza_patches(mf: usize) -> Vec<Patch> {
    const T: [usize; 7] = [30, 30, 24, 24, 18, 12, 6];
    let mf = mf.max(1);
    let rows = 7 * mf;
    let alpha = 90.0 / (rows as f64 + 0.5);
    let mut out = Vec::with_capacity(144 * mf * mf + 1);
    for i in 0..rows {
        let n = mf * T[i / mf];
        let lo = (i as f64 * alpha) * DEG;
        let hi = ((i + 1) as f64 * alpha) * DEG;
        let omega = 2.0 * PI * (hi.sin() - lo.sin()) / n as f64;
        let alt = (i as f64 + 0.5) * alpha;
        for j in 0..n {
            let az = (j as f64 + 0.5) * 360.0 / n as f64;
            out.push(Patch {
                alt_deg: alt,
                az_deg: az,
                omega,
                dir: dir_of(az, alt),
                width_alt: alpha,
                width_az: 360.0 / n as f64,
            });
        }
    }
    let cap_lo = rows as f64 * alpha;
    out.push(Patch {
        alt_deg: 90.0,
        az_deg: 0.0,
        omega: 2.0 * PI * (1.0 - (cap_lo * DEG).sin()),
        dir: DVec3::Z,
        width_alt: 90.0 - cap_lo,
        width_az: 360.0,
    });
    out
}

/// A direction sample of a patch used by the diffuse integral.
#[derive(Clone, Copy, Debug)]
pub struct SubPatch {
    pub dir: DVec3,
    pub omega: f64,
    pub parent: usize,
}

/// Patch directions: the centre (`subs == 1`) or four quarter-point
/// directions per patch (`subs == 4`).
pub fn expand_patches(patches: &[Patch], subs: usize) -> Vec<SubPatch> {
    let mut out = Vec::new();
    for (k, p) in patches.iter().enumerate() {
        if subs <= 1 {
            out.push(SubPatch {
                dir: p.dir,
                omega: p.omega,
                parent: k,
            });
        } else if p.alt_deg >= 90.0 {
            let alt = 90.0 - p.width_alt * 0.5;
            for q in 0..4 {
                out.push(SubPatch {
                    dir: dir_of(45.0 + 90.0 * q as f64, alt),
                    omega: p.omega / 4.0,
                    parent: k,
                });
            }
        } else {
            for (sa, sz) in [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)] {
                out.push(SubPatch {
                    dir: dir_of(
                        p.az_deg + sz * p.width_az * 0.25,
                        p.alt_deg + sa * p.width_alt * 0.25,
                    ),
                    omega: p.omega / 4.0,
                    parent: k,
                });
            }
        }
    }
    out
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SunHour {
    pub v: [f64; 3],
    pub dni: f64,
}

/// Cumulative sky matrix: per-patch sky radiance sums, sun beam sums and the
/// ground radiance, over the selected hours.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkyMatrix {
    pub schema: String,
    pub mf: usize,
    pub ground_reflectance: f64,
    pub north_angle_deg: f64,
    /// Hours in the period.
    pub n_hours: f64,
    /// Hours with the sun above the horizon.
    pub daylight_hours: f64,
    pub sky_model: String,
    /// Sum over hours of patch radiance, Wh/m2/sr.
    pub lsky: Vec<f64>,
    /// Sum over hours of sun beam on each patch, Wh/m2 at normal incidence.
    pub bsun: Vec<f64>,
    /// Ground radiance sum, Wh/m2/sr.
    pub lgnd: f64,
    pub sun_hours: Vec<SunHour>,
    pub total_ghi_wh_m2: f64,
    pub total_dni_wh_m2: f64,
    pub total_dhi_wh_m2: f64,
}

impl SkyMatrix {
    pub fn from_json(v: &Value) -> Result<Self, String> {
        let m: SkyMatrix =
            serde_json::from_value(v.clone()).map_err(|e| format!("not a sky matrix: {e}"))?;
        if m.schema != "skymatrix.v1" {
            return Err(format!(
                "expected a skymatrix.v1 table, found '{}'",
                m.schema
            ));
        }
        if m.lsky.len() != 144 * m.mf * m.mf + 1 || m.bsun.len() != m.lsky.len() {
            return Err("sky matrix patch count does not match its density".into());
        }
        Ok(m)
    }

    pub fn to_data(&self) -> Data {
        Data::json(serde_json::to_value(self).unwrap_or(Value::Null))
    }

    /// Uniform overcast-style sky for tests: radiance `dhi / pi` on every
    /// patch for one hour, no sun, no ground.
    pub fn uniform(mf: usize, dhi: f64) -> SkyMatrix {
        let n = 144 * mf * mf + 1;
        SkyMatrix {
            schema: "skymatrix.v1".into(),
            mf,
            ground_reflectance: 0.2,
            north_angle_deg: 0.0,
            n_hours: 1.0,
            daylight_hours: 1.0,
            sky_model: "isotropic".into(),
            lsky: vec![dhi / PI; n],
            bsun: vec![0.0; n],
            lgnd: 0.0,
            sun_hours: vec![],
            total_ghi_wh_m2: dhi,
            total_dni_wh_m2: 0.0,
            total_dhi_wh_m2: dhi,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SkyOpts {
    pub mf: usize,
    pub rho: f64,
    pub north: f64,
    pub spread: usize,
    pub conserve: bool,
    pub offset_h: f64,
    pub alg: SunAlg,
    pub hoy_start: usize,
    pub hoy_end: usize,
    pub model: String,
}

impl Default for SkyOpts {
    fn default() -> Self {
        SkyOpts {
            mf: 1,
            rho: 0.2,
            north: 0.0,
            spread: 4,
            conserve: true,
            offset_h: -0.5,
            alg: SunAlg::default(),
            hoy_start: 0,
            hoy_end: usize::MAX,
            model: "isotropic".into(),
        }
    }
}

/// Spread a beam of `dni` over the `k` patches nearest to `sun`, returning
/// `(patch, B)` pairs. With `conserve` the horizontal component is exact.
pub fn sun_patch_weights(
    patches: &[Patch],
    sun: DVec3,
    dni: f64,
    k: usize,
    conserve: bool,
) -> Vec<(usize, f64)> {
    let k = k.clamp(1, patches.len().min(8));
    let mut d: Vec<(f64, usize)> = patches
        .iter()
        .enumerate()
        .map(|(i, p)| (sun.dot(p.dir), i))
        .collect();
    d.select_nth_unstable_by(k - 1, |a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    d.truncate(k);
    d.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let w: Vec<f64> = d.iter().map(|(c, _)| 1.0 / (1.002 - c).max(1e-6)).collect();
    let ws: f64 = w.iter().sum();
    let wn: Vec<f64> = w.iter().map(|x| x / ws).collect();
    let mut factor = 1.0;
    if conserve {
        let s: f64 = wn
            .iter()
            .zip(&d)
            .map(|(w, (_, i))| w * patches[*i].dir.z)
            .sum();
        if s > 1e-9 {
            factor = sun.z / s;
        }
    }
    d.iter()
        .zip(&wn)
        .map(|((_, i), w)| (*i, dni * w * factor))
        .collect()
}

pub fn build_sky_matrix(w: &Weather, o: &SkyOpts) -> SkyMatrix {
    let patches = tregenza_patches(o.mf);
    let n = patches.len();
    let site = Site::from(&w.location);
    let mut m = SkyMatrix {
        schema: "skymatrix.v1".into(),
        mf: o.mf,
        ground_reflectance: o.rho,
        north_angle_deg: o.north,
        n_hours: 0.0,
        daylight_hours: 0.0,
        sky_model: o.model.clone(),
        lsky: vec![0.0; n],
        bsun: vec![0.0; n],
        lgnd: 0.0,
        sun_hours: vec![],
        total_ghi_wh_m2: 0.0,
        total_dni_wh_m2: 0.0,
        total_dhi_wh_m2: 0.0,
    };
    let mut l_uniform = 0.0;
    for (i, r) in w.rows.iter().enumerate() {
        let hoy = w.hoy(i);
        if hoy < o.hoy_start || hoy > o.hoy_end {
            continue;
        }
        m.n_hours += 1.0;
        let sp = sun_position_alg(
            o.alg,
            site,
            r.year,
            r.month,
            r.day,
            w.clock_h(i, o.offset_h),
        );
        l_uniform += r.dhi.max(0.0) / PI;
        m.total_dhi_wh_m2 += r.dhi.max(0.0);
        m.total_ghi_wh_m2 += r.ghi.max(0.0);
        m.total_dni_wh_m2 += r.dni.max(0.0);
        m.lgnd += o.rho * r.ghi.max(0.0) / PI;
        if sp.altitude_deg > 0.0 {
            m.daylight_hours += 1.0;
            if r.dni > 0.0 {
                let v = sun_vector(sp.azimuth_deg, sp.altitude_deg, o.north);
                for (q, b) in sun_patch_weights(&patches, v, r.dni, o.spread, o.conserve) {
                    m.bsun[q] += b;
                }
                m.sun_hours.push(SunHour {
                    v: v.to_array(),
                    dni: r.dni,
                });
            }
        }
    }
    for l in m.lsky.iter_mut() {
        *l = l_uniform;
    }
    m
}

// ---------------------------------------------------------------------------
// solar.sky_matrix
// ---------------------------------------------------------------------------

pub struct SkyMatrixNode;

impl NodeOp for SkyMatrixNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("solar.sky_matrix", "Sky Patch Matrix", Stage::Solar)
            .input(PortSpec::new("weather", DataKind::Json))
            .output(PortSpec::new("matrix", DataKind::Json))
            .output(PortSpec::new("patches", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("density_mf", "Patch density", "1", &["1", "2", "4"]).help("Subdivision factor: 145, 577 or 2305 sky patches."))
            .param(ParamSpec::float("ground_reflectance", "Ground reflectance", 0.2, 0.0, 1.0).help("Reflectance of the ground, 0 to 1. It sets the radiation reflected onto the surface from below."))
            .param(
                ParamSpec::choice("sky_model", "Sky model", "isotropic", &["isotropic", "perez"])
                    .help("Radiance distribution over the sky dome."),
            )
            .param(ParamSpec::float("north_angle_deg", "North angle", 0.0, -360.0, 360.0).unit("deg").help("Counter-clockwise angle from north to +Y of the model."))
            .param(ParamSpec::int("sun_spread", "Sun spread", 4, 1, 8).help("Number of nearest patches the sun beam is spread over."))
            .param(ParamSpec::bool("conserve_horizontal", "Conserve horizontal", true).help("Rescale the spread beam so its horizontal component is exact."))
            .param(ParamSpec::choice("algorithm", "Sun algorithm", "noaa_meeus", &["noaa_low", "noaa_meeus"]).help("noaa_meeus is accurate to about 0.01 deg; noaa_low is a faster fit, off by up to about 0.5 deg near the equinoxes."))
            .param(ParamSpec::float("sun_time_offset_h", "Sun time offset", -0.5, -1.0, 1.0).unit("h").help("Offset from the end of each weather hour at which the sun is evaluated."))
            .param(ParamSpec::int("hoy_start", "First hour of year", 0, 0, 8783).help("Zero-based hour of year."))
            .param(ParamSpec::int("hoy_end", "Last hour of year", 8783, 0, 8783).help("Zero-based hour of year, inclusive."))
            .help("Accumulates a weather table into a cumulative sky matrix: sky radiance per patch of a hemispherical dome, the sun beam spread over nearby patches, and the ground radiance.")
            .emulates("Cumulative sky matrix (gendaymtx-style), Tregenza and Reinhart patches")
            .spec(SOLAR_SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let jv = json_in(ins, 0)?;
        let w = Weather::from_json(&jv).map_err(|e| ctx.fail(e))?;
        let mf: usize = p
            .str("density_mf")?
            .parse()
            .map_err(|_| CookError::BadParam("density_mf".into()))?;
        let mut report = Report::new("Sky matrix");
        let mut model = p.str("sky_model")?.to_string();
        if model == "perez" {
            placeholder(
                &mut report,
                "the all-weather sky luminance distribution (clearness and brightness bins) will replace the isotropic fallback used here",
            );
            model = "isotropic".into();
        }
        let opts = SkyOpts {
            mf,
            rho: p.float("ground_reflectance")?,
            north: p.float("north_angle_deg")?,
            spread: p.int("sun_spread")?.max(1) as usize,
            conserve: p.bool("conserve_horizontal")?,
            offset_h: p.float("sun_time_offset_h")?,
            alg: alg_of(p.str("algorithm")?),
            hoy_start: p.int("hoy_start")?.max(0) as usize,
            hoy_end: p.int("hoy_end")?.max(0) as usize,
            model,
        };
        let m = build_sky_matrix(&w, &opts);
        let patches = tregenza_patches(mf);
        let mut g = Geometry::new();
        let (mut alt, mut az, mut om) = (vec![], vec![], vec![]);
        for p in &patches {
            g.points.push(p.dir * 1000.0);
            alt.push(p.alt_deg);
            az.push(p.az_deg);
            om.push(p.omega);
        }
        g.point_attribs.set("alt", AttribArray::Float(alt));
        g.point_attribs.set("az", AttribArray::Float(az));
        g.point_attribs.set("omega", AttribArray::Float(om));
        g.point_attribs
            .set("lsky", AttribArray::Float(m.lsky.clone()));
        g.point_attribs
            .set("bsun", AttribArray::Float(m.bsun.clone()));
        report.metric("patches", Metric::new(patches.len() as f64, "count"));
        report.metric(
            "solid_angle_sum",
            Metric::new(patches.iter().map(|p| p.omega).sum::<f64>(), "sr"),
        );
        report.metric("hours", Metric::new(m.n_hours, "h"));
        report.metric("daylight_hours", Metric::new(m.daylight_hours, "h"));
        report.metric(
            "annual_ghi",
            Metric::new(m.total_ghi_wh_m2 / 1000.0, "kWh/m2"),
        );
        report.metric(
            "annual_dni",
            Metric::new(m.total_dni_wh_m2 / 1000.0, "kWh/m2"),
        );
        report.metric(
            "annual_dhi",
            Metric::new(m.total_dhi_wh_m2 / 1000.0, "kWh/m2"),
        );
        if m.n_hours == 0.0 {
            report.note(
                Severity::Error,
                "no weather rows fall inside the selected hour range",
            );
        }
        Ok(vec![m.to_data(), Data::geometry(g), Data::report(report)])
    }
}

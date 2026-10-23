//! Sun position (low-accuracy and Meeus-series equations), sunrise/sunset,
//! sun vectors and the sun-position / sun-path nodes.
//!
//! Frame: +X east, +Y north, +Z up. Azimuth is degrees clockwise from true
//! north; `north_angle_deg` rotates the model (counter-clockwise from north
//! to +Y, same convention as the sky matrix).

use std::f64::consts::PI;

use glam::DVec3;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use vernon_core::{Geometry, Metric, Prim, Report, Severity};
use vernon_graph::{
    CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params,
    PortSpec, Stage,
};

use crate::util::*;
use crate::weather::{Location, Weather};

pub const DEG: f64 = PI / 180.0;

pub fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Day of year, 1-based; month and day are clamped to valid values.
pub fn day_of_year(y: i32, m: u32, d: u32) -> u32 {
    let m = m.clamp(1, 12);
    let d = d.clamp(1, days_in_month(y, m));
    (1..m).map(|k| days_in_month(y, k)).sum::<u32>() + d
}

pub fn wrap180(x: f64) -> f64 {
    (x + 180.0).rem_euclid(360.0) - 180.0
}

#[derive(Clone, Copy, Debug)]
pub struct Site {
    pub lat: f64,
    pub lon: f64,
    pub tz: f64,
}

impl From<&Location> for Site {
    fn from(l: &Location) -> Self {
        Site {
            lat: l.lat,
            lon: l.lon,
            tz: l.tz,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SunPos {
    pub altitude_deg: f64,
    pub azimuth_deg: f64,
    pub zenith_deg: f64,
    pub eqtime_min: f64,
    pub decl_deg: f64,
    pub hour_angle_deg: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SunAlg {
    Low,
    Meeus,
}

fn finish(site: Site, clock_h: f64, eqtime: f64, decl: f64) -> SunPos {
    let tst = clock_h * 60.0 + eqtime + 4.0 * site.lon - 60.0 * site.tz;
    let ha_deg = wrap180(tst / 4.0 - 180.0);
    let lat = site.lat * DEG;
    let ha = ha_deg * DEG;
    let cos_zen = (lat.sin() * decl.sin() + lat.cos() * decl.cos() * ha.cos()).clamp(-1.0, 1.0);
    let zen = cos_zen.acos();
    let y = ha.sin();
    let x = ha.cos() * lat.sin() - decl.tan() * lat.cos();
    let az = (y.atan2(x) / DEG + 180.0).rem_euclid(360.0);
    SunPos {
        altitude_deg: 90.0 - zen / DEG,
        azimuth_deg: az,
        zenith_deg: zen / DEG,
        eqtime_min: eqtime,
        decl_deg: decl / DEG,
        hour_angle_deg: ha_deg,
    }
}

fn low_eqtime_decl(year: i32, doy: u32, clock_h: f64) -> (f64, f64) {
    let n_year = if is_leap(year) { 366.0 } else { 365.0 };
    let g = 2.0 * PI / n_year * (doy as f64 - 1.0 + (clock_h - 12.0) / 24.0);
    let eqtime = 229.18
        * (0.000075 + 0.001868 * g.cos()
            - 0.032077 * g.sin()
            - 0.014615 * (2.0 * g).cos()
            - 0.040849 * (2.0 * g).sin());
    let decl = 0.006918 - 0.399912 * g.cos() + 0.070257 * g.sin() - 0.006758 * (2.0 * g).cos()
        + 0.000907 * (2.0 * g).sin()
        - 0.002697 * (3.0 * g).cos()
        + 0.00148 * (3.0 * g).sin();
    (eqtime, decl)
}

fn julian_day_0h(y: i32, m: u32, d: u32) -> f64 {
    let a = (14 - m as i32) / 12;
    let yy = y + 4800 - a;
    let mm = m as i32 + 12 * a - 3;
    let jdn = d as i32 + (153 * mm + 2) / 5 + 365 * yy + yy / 4 - yy / 100 + yy / 400 - 32045;
    jdn as f64 - 0.5
}

fn meeus_eqtime_decl(jd: f64) -> (f64, f64) {
    let t = (jd - 2451545.0) / 36525.0;
    let l0 = (280.46646 + t * (36000.76983 + t * 0.0003032)).rem_euclid(360.0);
    let m = 357.52911 + t * (35999.05029 - 0.0001537 * t);
    let e = 0.016708634 - t * (0.000042037 + 0.0000001267 * t);
    let mr = m * DEG;
    let c = mr.sin() * (1.914602 - t * (0.004817 + 0.000014 * t))
        + (2.0 * mr).sin() * (0.019993 - 0.000101 * t)
        + (3.0 * mr).sin() * 0.000289;
    let omega = 125.04 - 1934.136 * t;
    let app = l0 + c - 0.00569 - 0.00478 * (omega * DEG).sin();
    let mean_obl =
        23.0 + (26.0 + (21.448 - t * (46.815 + t * (0.00059 - t * 0.001813))) / 60.0) / 60.0;
    let obl = mean_obl + 0.00256 * (omega * DEG).cos();
    let decl = ((obl * DEG).sin() * (app * DEG).sin()).asin();
    let y = (obl * DEG / 2.0).tan().powi(2);
    let l0r = l0 * DEG;
    let eq = y * (2.0 * l0r).sin() - 2.0 * e * mr.sin()
        + 4.0 * e * y * mr.sin() * (2.0 * l0r).cos()
        - 0.5 * y * y * (4.0 * l0r).sin()
        - 1.25 * e * e * (2.0 * mr).sin();
    (4.0 * eq / DEG, decl)
}

/// Sun position for a calendar date and local standard clock time (hours).
pub fn sun_position_alg(
    alg: SunAlg,
    site: Site,
    year: i32,
    month: u32,
    day: u32,
    clock_h: f64,
) -> SunPos {
    let (eq, decl) = match alg {
        SunAlg::Low => low_eqtime_decl(year, day_of_year(year, month, day), clock_h),
        SunAlg::Meeus => {
            let m = month.clamp(1, 12);
            let d = day.clamp(1, days_in_month(year, m));
            meeus_eqtime_decl(julian_day_0h(year, m, d) + (clock_h - site.tz) / 24.0)
        }
    };
    finish(site, clock_h, eq, decl)
}

/// Low-accuracy position from a day of year.
pub fn sun_position(site: Site, year: i32, doy: u32, clock_h: f64) -> SunPos {
    let (eq, decl) = low_eqtime_decl(year, doy, clock_h);
    finish(site, clock_h, eq, decl)
}

/// Atmospheric refraction correction in degrees for a geometric altitude.
pub fn refraction_deg(alt: f64) -> f64 {
    if alt > 85.0 {
        0.0
    } else if alt > 5.0 {
        let t = (alt * DEG).tan();
        (58.1 / t - 0.07 / t.powi(3) + 0.000086 / t.powi(5)) / 3600.0
    } else if alt > -0.575 {
        (1735.0 - 518.2 * alt + 103.4 * alt * alt - 12.79 * alt.powi(3) + 0.711 * alt.powi(4))
            / 3600.0
    } else {
        -20.774 / (alt * DEG).tan() / 3600.0
    }
}

/// Unit vector towards the sun in the model frame.
pub fn sun_vector(az_deg: f64, alt_deg: f64, north_deg: f64) -> DVec3 {
    let a = (az_deg - north_deg) * DEG;
    let h = alt_deg * DEG;
    DVec3::new(a.sin() * h.cos(), a.cos() * h.cos(), h.sin())
}

#[derive(Clone, Copy, Debug)]
pub struct SunTimes {
    pub sunrise_h: Option<f64>,
    pub noon_h: f64,
    pub sunset_h: Option<f64>,
    pub day_length_h: f64,
}

/// Sunrise, solar noon and sunset in local standard time (hours).
pub fn sun_times(alg: SunAlg, site: Site, year: i32, month: u32, day: u32) -> SunTimes {
    let sp = sun_position_alg(alg, site, year, month, day, 12.0);
    let lat = site.lat * DEG;
    let d = sp.decl_deg * DEG;
    let arg = (90.833 * DEG).cos() / (lat.cos() * d.cos()) - lat.tan() * d.tan();
    let noon_utc = 720.0 - 4.0 * site.lon - sp.eqtime_min;
    let local = |utc_min: f64| (utc_min + 60.0 * site.tz) / 60.0;
    let noon_h = local(noon_utc);
    if arg > 1.0 {
        return SunTimes {
            sunrise_h: None,
            noon_h,
            sunset_h: None,
            day_length_h: 0.0,
        };
    }
    if arg < -1.0 {
        return SunTimes {
            sunrise_h: None,
            noon_h,
            sunset_h: None,
            day_length_h: 24.0,
        };
    }
    let ha_s = arg.acos() / DEG;
    SunTimes {
        sunrise_h: Some(local(720.0 - 4.0 * (site.lon + ha_s) - sp.eqtime_min)),
        noon_h,
        sunset_h: Some(local(720.0 - 4.0 * (site.lon - ha_s) - sp.eqtime_min)),
        day_length_h: 8.0 * ha_s / 60.0,
    }
}

// ---------------------------------------------------------------------------
// Sun table exchanged between nodes
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SunSample {
    pub hoy: Option<usize>,
    pub altitude_deg: f64,
    pub azimuth_deg: f64,
    pub zenith_deg: f64,
    pub vec: [f64; 3],
    pub dni: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SunTable {
    pub schema: String,
    pub north_angle_deg: f64,
    pub samples: Vec<SunSample>,
}

impl SunTable {
    pub fn new(north: f64, samples: Vec<SunSample>) -> Self {
        SunTable {
            schema: "sun.v1".into(),
            north_angle_deg: north,
            samples,
        }
    }

    pub fn from_json(v: &Value) -> Result<Self, String> {
        let t: SunTable =
            serde_json::from_value(v.clone()).map_err(|e| format!("not a sun table: {e}"))?;
        if t.schema != "sun.v1" {
            return Err(format!("expected a sun.v1 table, found '{}'", t.schema));
        }
        Ok(t)
    }

    pub fn to_data(&self) -> Data {
        Data::json(serde_json::to_value(self).unwrap_or(Value::Null))
    }
}

pub fn make_sample(sp: &SunPos, north: f64, hoy: Option<usize>, dni: f64) -> SunSample {
    let v = sun_vector(sp.azimuth_deg, sp.altitude_deg, north);
    SunSample {
        hoy,
        altitude_deg: sp.altitude_deg,
        azimuth_deg: sp.azimuth_deg,
        zenith_deg: sp.zenith_deg,
        vec: v.to_array(),
        dni,
    }
}

fn alg_from(p: &Params, report: &mut Report) -> CookResult<SunAlg> {
    Ok(match p.str("algorithm")? {
        "noaa_low" => SunAlg::Low,
        "noaa_meeus" => SunAlg::Meeus,
        "spa" => {
            placeholder(
                report,
                "the table-driven high-accuracy series (heliocentric terms, nutation, parallax) will replace the Meeus-series fallback used here",
            );
            SunAlg::Meeus
        }
        other => return Err(CookError::BadParam(format!("algorithm = {other}"))),
    })
}

/// Shared by sky-matrix building: parse the `algorithm` choice without a report.
pub fn alg_of(s: &str) -> SunAlg {
    if s == "noaa_low" {
        SunAlg::Low
    } else {
        SunAlg::Meeus
    }
}

fn site_params(info: NodeTypeInfo) -> NodeTypeInfo {
    info.param(
        ParamSpec::float("lat", "Latitude", 25.2048, -90.0, 90.0)
            .unit("deg")
            .help("Degrees north."),
    )
    .param(
        ParamSpec::float("lon", "Longitude", 55.2708, -180.0, 180.0)
            .unit("deg")
            .help("Degrees east."),
    )
    .param(
        ParamSpec::float("tz_h", "Time zone", 4.0, -12.0, 14.0)
            .unit("h")
            .help("Hours ahead of UTC for local standard time."),
    )
}

fn site_of(p: &Params) -> CookResult<Site> {
    Ok(Site {
        lat: p.float("lat")?,
        lon: p.float("lon")?,
        tz: p.float("tz_h")?,
    })
}

// ---------------------------------------------------------------------------
// solar.sun_position
// ---------------------------------------------------------------------------

pub struct SunPositionNode;

impl NodeOp for SunPositionNode {
    fn info(&self) -> NodeTypeInfo {
        let info = NodeTypeInfo::new("solar.sun_position", "Sun Position", Stage::Solar)
            .input(PortSpec::optional("weather", DataKind::Json))
            .output(PortSpec::new("sun", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report));
        site_params(info)
            .param(ParamSpec::int("year", "Year", 2026, 1901, 2099))
            .param(ParamSpec::int("month", "Month", 6, 1, 12))
            .param(ParamSpec::int("day", "Day", 21, 1, 31))
            .param(
                ParamSpec::float("clock_time_h", "Clock time", 12.0, 0.0, 24.0)
                    .unit("h")
                    .help("Local standard time, decimal hours. Used when no weather table is connected."),
            )
            .param(
                ParamSpec::choice("algorithm", "Algorithm", "noaa_low", &["noaa_low", "noaa_meeus", "spa"])
                    .help("Series used for declination and equation of time."),
            )
            .param(ParamSpec::bool("refraction", "Refraction", false).help("Apply atmospheric refraction to the altitude."))
            .param(
                ParamSpec::float("north_angle_deg", "North angle", 0.0, -360.0, 360.0)
                    .unit("deg")
                    .help("Counter-clockwise angle from north to +Y of the model."),
            )
            .param(
                ParamSpec::float("sun_time_offset_h", "Sun time offset", -0.5, -1.0, 1.0)
                    .unit("h")
                    .help("Offset from the end of each weather hour at which the sun is evaluated (-0.5 is mid-hour)."),
            )
            .help("Computes sun altitude, azimuth and direction vectors for a place and time, or for every row of a weather table. Reports sunrise, solar noon and sunset.")
            .emulates("NOAA solar calculator equations, Blender Sun Position add-on, Helio")
            .spec(SOLAR_SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let mut report = Report::new("Sun position");
        let alg = alg_from(p, &mut report)?;
        let north = p.float("north_angle_deg")?;
        let refr = p.bool("refraction")?;
        let off = p.float("sun_time_offset_h")?;
        let adj = |mut sp: SunPos| {
            if refr {
                sp.altitude_deg += refraction_deg(sp.altitude_deg);
                sp.zenith_deg = 90.0 - sp.altitude_deg;
            }
            sp
        };
        let mut samples = Vec::new();
        let (site, year, month, day);
        if let Some(wv) = opt_json_in(ins, 0)? {
            let w = Weather::from_json(&wv).map_err(|e| ctx.fail(e))?;
            if w.rows.is_empty() {
                return Err(ctx.fail("weather table has no rows"));
            }
            site = Site::from(&w.location);
            for (i, r) in w.rows.iter().enumerate() {
                let sp = adj(sun_position_alg(
                    alg,
                    site,
                    r.year,
                    r.month,
                    r.day,
                    w.clock_h(i, off),
                ));
                samples.push(make_sample(&sp, north, Some(w.hoy(i)), r.dni));
            }
            let day_rows = samples.iter().filter(|s| s.altitude_deg > 0.0).count();
            let max_alt = samples
                .iter()
                .map(|s| s.altitude_deg)
                .fold(f64::MIN, f64::max);
            report.metric("rows", Metric::new(samples.len() as f64, "count"));
            report.metric("daylight_rows", Metric::new(day_rows as f64, "count"));
            report.metric("max_altitude", Metric::new(max_alt, "deg"));
            year = w.rows[0].year;
            month = w.rows[0].month;
            day = w.rows[0].day;
        } else {
            site = site_of(p)?;
            year = p.int("year")? as i32;
            month = p.int("month")? as u32;
            day = p.int("day")? as u32;
            let sp = adj(sun_position_alg(
                alg,
                site,
                year,
                month,
                day,
                p.float("clock_time_h")?,
            ));
            report.metric("altitude", Metric::new(sp.altitude_deg, "deg"));
            report.metric("azimuth", Metric::new(sp.azimuth_deg, "deg"));
            report.metric("zenith", Metric::new(sp.zenith_deg, "deg"));
            report.metric("equation_of_time", Metric::new(sp.eqtime_min, "min"));
            report.metric("declination", Metric::new(sp.decl_deg, "deg"));
            samples.push(make_sample(&sp, north, None, 0.0));
        }
        let st = sun_times(alg, site, year, month, day);
        report.metric("solar_noon", Metric::new(st.noon_h, "h"));
        report.metric("day_length", Metric::new(st.day_length_h, "h"));
        if let (Some(a), Some(b)) = (st.sunrise_h, st.sunset_h) {
            report.metric("sunrise", Metric::new(a, "h"));
            report.metric("sunset", Metric::new(b, "h"));
        } else if st.day_length_h == 0.0 {
            report.note(
                Severity::Info,
                "polar night: the sun does not rise on this date",
            );
        } else {
            report.note(
                Severity::Info,
                "midnight sun: the sun does not set on this date",
            );
        }
        Ok(vec![
            SunTable::new(north, samples).to_data(),
            Data::report(report),
        ])
    }
}

// ---------------------------------------------------------------------------
// solar.sun_path
// ---------------------------------------------------------------------------

pub struct SunPathNode;

impl NodeOp for SunPathNode {
    fn info(&self) -> NodeTypeInfo {
        let info = NodeTypeInfo::new("solar.sun_path", "Sun Path Curves", Stage::Solar)
            .output(PortSpec::new("curves", DataKind::Geometry))
            .output(PortSpec::new("sun", DataKind::Json));
        site_params(info)
            .param(ParamSpec::int("year", "Year", 2026, 1901, 2099))
            .param(ParamSpec::int("month", "Month", 6, 1, 12).help("Date of the diurnal path."))
            .param(ParamSpec::int("day", "Day", 21, 1, 31).help("Date of the diurnal path."))
            .param(ParamSpec::choice("mode", "Mode", "diurnal", &["analemma", "diurnal"]).help("Diurnal: one day, many times. Analemma: one clock time, every day of the year."))
            .param(ParamSpec::float("clock_time_h", "Clock time", 12.0, 0.0, 24.0).unit("h").help("Clock time of the analemma."))
            .param(ParamSpec::float("step_min", "Step", 10.0, 1.0, 240.0).unit("min").help("Time step of the diurnal path."))
            .param(ParamSpec::float("radius_mm", "Dome radius", 1000.0, 1.0, 1.0e7).unit("mm"))
            .param(ParamSpec::float("north_angle_deg", "North angle", 0.0, -360.0, 360.0).unit("deg"))
            .help("Builds the sun path as polylines on a dome: one day sampled through the day, or the figure-eight of one clock time over a year. Only positions above the horizon are kept.")
            .emulates("Sun position diurnal path and analemma collections")
            .spec(SOLAR_SPEC)
            .implemented()
    }

    fn cook(&self, _ctx: &CookContext, _ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let site = site_of(p)?;
        let year = p.int("year")? as i32;
        let north = p.float("north_angle_deg")?;
        let radius = p.float("radius_mm")?;
        let mut pts: Vec<(SunPos, Option<usize>)> = Vec::new();
        match p.str("mode")? {
            "analemma" => {
                let clock = p.float("clock_time_h")?;
                let n = if is_leap(year) { 366 } else { 365 };
                for doy in 1..=n {
                    pts.push((sun_position(site, year, doy, clock), Some(doy as usize)));
                }
            }
            "diurnal" => {
                let step = p.float("step_min")?.max(1.0);
                let doy = day_of_year(year, p.int("month")? as u32, p.int("day")? as u32);
                let mut t = 0.0;
                while t <= 24.0 * 60.0 + 1e-9 {
                    pts.push((sun_position(site, year, doy, t / 60.0), None));
                    t += step;
                }
            }
            other => return Err(CookError::BadParam(format!("mode = {other}"))),
        }
        let mut g = Geometry::new();
        let mut run: Vec<u32> = Vec::new();
        let mut samples = Vec::new();
        for (sp, id) in &pts {
            if sp.altitude_deg > 0.0 {
                let v = sun_vector(sp.azimuth_deg, sp.altitude_deg, north);
                let idx = g.add_point(v * radius);
                run.push(idx);
                samples.push(make_sample(sp, north, *id, 0.0));
            } else if run.len() >= 2 {
                g.add_prim(Prim::polyline(std::mem::take(&mut run)));
            } else {
                run.clear();
            }
        }
        if run.len() >= 2 {
            g.add_prim(Prim::polyline(run));
        }
        Ok(vec![
            Data::geometry(g),
            SunTable::new(north, samples).to_data(),
        ])
    }
}

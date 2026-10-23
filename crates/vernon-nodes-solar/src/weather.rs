//! Hourly weather table, EPW reader, design hour, peak-hour selection and
//! sky temperature.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use vernon_core::{Metric, Report, Severity};
use vernon_graph::{
    CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params,
    PortSpec, Stage,
};

use crate::sun::{day_of_year, sun_position, Site, DEG};
use crate::util::*;

pub const SIGMA: f64 = 5.670374419e-8;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Location {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    /// Hours ahead of UTC.
    pub tz: f64,
    pub elev: f64,
}

/// One hourly record. Irradiance in W/m2 (equal to Wh/m2 for hourly data).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Row {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// EPW hour 1..24 (hour N covers the 60 minutes ending at N:00).
    pub hour: f64,
    /// Exact clock time (local standard hours) at which to evaluate the sun;
    /// `None` means `hour + offset`.
    pub clock_h: Option<f64>,
    pub t_db: f64,
    pub t_dp: f64,
    pub rh: f64,
    pub pressure: f64,
    pub ghi: f64,
    pub dni: f64,
    pub dhi: f64,
    /// Horizontal infrared, W/m2, when present.
    pub ir: Option<f64>,
    pub wind_dir: f64,
    pub wind: f64,
    /// Total sky cover, tenths.
    pub sky_cover: f64,
    /// Opaque sky cover, tenths.
    pub opaque_cover: f64,
    pub albedo: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Weather {
    pub schema: String,
    pub location: Location,
    pub rows: Vec<Row>,
}

impl Weather {
    pub fn new(location: Location, rows: Vec<Row>) -> Self {
        Weather {
            schema: "weather.v1".into(),
            location,
            rows,
        }
    }

    pub fn from_json(v: &Value) -> Result<Self, String> {
        let w: Weather =
            serde_json::from_value(v.clone()).map_err(|e| format!("not a weather table: {e}"))?;
        if w.schema != "weather.v1" {
            return Err(format!("expected a weather.v1 table, found '{}'", w.schema));
        }
        Ok(w)
    }

    pub fn to_data(&self) -> Data {
        Data::json(serde_json::to_value(self).unwrap_or(Value::Null))
    }

    /// Hour of year, 0-based: `(day_of_year - 1) * 24 + (hour - 1)`.
    pub fn hoy(&self, i: usize) -> usize {
        let r = &self.rows[i];
        let doy = day_of_year(r.year, r.month, r.day) as usize;
        (doy - 1) * 24 + (r.hour.round().max(1.0) as usize - 1)
    }

    /// Local standard clock time at which the sun is evaluated for row `i`.
    pub fn clock_h(&self, i: usize, offset: f64) -> f64 {
        let r = &self.rows[i];
        r.clock_h.unwrap_or(r.hour + offset)
    }
}

// ---------------------------------------------------------------------------
// EPW reader
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct EpwStats {
    pub rows: usize,
    pub missing_dni: usize,
    pub missing_dhi: usize,
    pub missing_ghi: usize,
    pub carried: usize,
}

fn field(f: &[&str], i: usize) -> Option<f64> {
    f.get(i).and_then(|s| s.trim().parse::<f64>().ok())
}

fn present(v: Option<f64>, code: f64) -> Option<f64> {
    v.filter(|x| (x - code).abs() > 1e-6 && x.is_finite())
}

/// Parse an EPW file's text.
pub fn parse_epw(text: &str, strict: bool, carry: bool) -> Result<(Weather, EpwStats), String> {
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.lines();
    let mut loc: Option<Location> = None;
    for _ in 0..8 {
        let l = lines
            .next()
            .ok_or("the file ends inside the 8-line header")?;
        if l.trim_start().to_ascii_uppercase().starts_with("LOCATION") {
            let f: Vec<&str> = l.split(',').map(str::trim).collect();
            if f.len() < 10 {
                return Err("LOCATION header has fewer than 10 fields".into());
            }
            let num = |i: usize, what: &str| {
                f[i].parse::<f64>()
                    .map_err(|_| format!("LOCATION: bad {what} '{}'", f[i]))
            };
            loc = Some(Location {
                name: f[1].to_string(),
                lat: num(6, "latitude")?,
                lon: num(7, "longitude")?,
                tz: num(8, "time zone")?,
                elev: num(9, "elevation")?,
            });
        }
    }
    let location = loc.ok_or("no LOCATION header line")?;
    let mut stats = EpwStats::default();
    let mut rows: Vec<Row> = Vec::new();
    let mut prev_t = (20.0, 10.0, 0.0);
    for (ri, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let n = ri + 9;
        let f: Vec<&str> = line.split(',').collect();
        if strict && f.len() != 35 {
            return Err(format!("row {n}: expected 35 fields, found {}", f.len()));
        }
        if f.len() < 24 {
            return Err(format!("row {n}: only {} fields", f.len()));
        }
        let (Some(year), Some(month), Some(day), Some(hour)) =
            (field(&f, 0), field(&f, 1), field(&f, 2), field(&f, 3))
        else {
            return Err(format!("row {n}: bad date fields"));
        };
        if !(1.0..=24.0).contains(&hour)
            || !(1.0..=12.0).contains(&month)
            || !(1.0..=31.0).contains(&day)
        {
            return Err(format!("row {n}: date or hour out of range"));
        }
        let mut miss = |v: Option<f64>, slot: usize, d: f64| -> f64 {
            match v {
                Some(x) => {
                    match slot {
                        0 => prev_t.0 = x,
                        1 => prev_t.1 = x,
                        _ => prev_t.2 = x,
                    }
                    x
                }
                None => {
                    stats.carried += 1;
                    if carry {
                        match slot {
                            0 => prev_t.0,
                            1 => prev_t.1,
                            _ => prev_t.2,
                        }
                    } else {
                        d
                    }
                }
            }
        };
        let t_db = miss(present(field(&f, 6), 99.9), 0, 0.0);
        let t_dp = miss(present(field(&f, 7), 99.9), 1, 0.0);
        let wind = miss(present(field(&f, 21), 999.0), 2, 0.0);
        let dni = match present(field(&f, 14), 9999.0) {
            Some(x) if x >= 0.0 => x,
            _ => {
                stats.missing_dni += 1;
                0.0
            }
        };
        let dhi = match present(field(&f, 15), 9999.0) {
            Some(x) if x >= 0.0 => x,
            _ => {
                stats.missing_dhi += 1;
                0.0
            }
        };
        let ghi = match present(field(&f, 13), 9999.0) {
            Some(x) if x >= 0.0 => x,
            _ => {
                stats.missing_ghi += 1;
                dhi
            }
        };
        rows.push(Row {
            year: year as i32,
            month: month as u32,
            day: day as u32,
            hour,
            clock_h: None,
            t_db,
            t_dp,
            rh: present(field(&f, 8), 999.0).unwrap_or(50.0),
            pressure: present(field(&f, 9), 999999.0).unwrap_or(101325.0),
            ghi,
            dni,
            dhi,
            ir: present(field(&f, 12), 9999.0),
            wind_dir: present(field(&f, 20), 999.0).unwrap_or(0.0),
            wind,
            sky_cover: present(field(&f, 22), 99.0).unwrap_or(0.0),
            opaque_cover: present(field(&f, 23), 99.0).unwrap_or(0.0),
            albedo: present(field(&f, 32), 999.0).unwrap_or(0.2),
        });
    }
    if rows.is_empty() {
        return Err("the file has no data rows".into());
    }
    stats.rows = rows.len();
    Ok((Weather::new(location, rows), stats))
}

// ---------------------------------------------------------------------------
// Sky temperature
// ---------------------------------------------------------------------------

/// Cloud correction factor (opaque sky cover in tenths).
pub fn cloud_factor(n: f64) -> f64 {
    1.0 + 0.0224 * n - 0.0035 * n * n + 0.00028 * n * n * n
}

/// Clear-sky emissivity from the dew point (degC).
pub fn eps_clear_clark_allen(t_dp_c: f64) -> f64 {
    0.787 + 0.764 * ((t_dp_c + 273.15) / 273.0).ln()
}

/// Horizontal infrared (W/m2) from dry bulb, dew point and opaque cover.
pub fn ir_from_corr(t_db_c: f64, t_dp_c: f64, opaque_tenths: f64) -> f64 {
    let eps = eps_clear_clark_allen(t_dp_c) * cloud_factor(opaque_tenths);
    eps * SIGMA * (t_db_c + 273.15).powi(4)
}

/// Sky temperature (degC) from horizontal infrared.
pub fn tsky_from_ir(ir: f64) -> f64 {
    (ir.max(0.0) / SIGMA).powf(0.25) - 273.15
}

// ---------------------------------------------------------------------------
// solar.weather_import
// ---------------------------------------------------------------------------

pub struct WeatherImportNode;

impl NodeOp for WeatherImportNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("solar.weather_import", "Weather File Import", Stage::Solar)
            .output(PortSpec::new("weather", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::path("path", "File", &["epw"], false).help("Hourly weather file with the standard 8-line header and 35 fields per row."))
            .param(
                ParamSpec::choice("missing_policy", "Missing values", "carry", &["carry", "zero"])
                    .help("Temperature and wind gaps: repeat the previous valid hour, or use zero. Missing beam and diffuse values are always zero."),
            )
            .param(ParamSpec::bool("strict_field_count", "Strict field count", true).help("Reject rows that do not have exactly 35 fields."))
            .help("Reads an hourly typical-year weather file into a table (location, temperatures, radiation, wind, sky cover) and reports how many values were missing.")
            .emulates("EPW weather file reader (EnergyPlus format)")
            .spec(SOLAR_SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, _ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let path = p.str("path")?;
        if path.trim().is_empty() {
            return Err(ctx.fail("no weather file path set"));
        }
        let text = std::fs::read_to_string(path)
            .map_err(|e| ctx.fail(format!("cannot read '{path}': {e}")))?;
        let (w, st) = parse_epw(
            &text,
            p.bool("strict_field_count")?,
            p.str("missing_policy")? == "carry",
        )
        .map_err(|e| ctx.fail(e))?;
        let mut r = Report::new("Weather import");
        r.metric("rows", Metric::new(st.rows as f64, "count"));
        r.metric("missing_dni", Metric::new(st.missing_dni as f64, "count"));
        r.metric("missing_dhi", Metric::new(st.missing_dhi as f64, "count"));
        r.metric("missing_ghi", Metric::new(st.missing_ghi as f64, "count"));
        r.metric("carried_values", Metric::new(st.carried as f64, "count"));
        r.metric("latitude", Metric::new(w.location.lat, "deg"));
        r.metric("longitude", Metric::new(w.location.lon, "deg"));
        r.metric("time_zone", Metric::new(w.location.tz, "h"));
        r.metric("elevation", Metric::new(w.location.elev, "m"));
        let annual: f64 = w.rows.iter().map(|x| x.ghi).sum::<f64>() / 1000.0;
        r.metric("annual_ghi", Metric::new(annual, "kWh/m2"));
        if st.rows != 8760 && st.rows != 8784 {
            r.note(
                Severity::Warning,
                format!("{} rows is not a whole year (8760 or 8784)", st.rows),
            );
        }
        Ok(vec![w.to_data(), Data::report(r)])
    }
}

// ---------------------------------------------------------------------------
// solar.design_hour
// ---------------------------------------------------------------------------

pub struct DesignHourNode;

impl NodeOp for DesignHourNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("solar.design_hour", "Design Hour", Stage::Solar)
            .output(PortSpec::new("weather", DataKind::Json))
            .output(PortSpec::new("location", DataKind::Json))
            .param(ParamSpec::float("lat", "Latitude", 25.2048, -90.0, 90.0).unit("deg").help("Latitude of the site, in degrees north."))
            .param(ParamSpec::float("lon", "Longitude", 55.2708, -180.0, 180.0).unit("deg").help("Longitude of the site, in degrees east."))
            .param(ParamSpec::float("tz_h", "Time zone", 4.0, -12.0, 14.0).unit("h").help("Time zone as hours ahead of UTC."))
            .param(ParamSpec::int("year", "Year", 2026, 1901, 2099).help("Year of the design day."))
            .param(ParamSpec::int("month", "Month", 6, 1, 12).help("Month of the design day, 1 to 12."))
            .param(ParamSpec::int("day", "Day", 21, 1, 31).help("Day of the month of the design day."))
            .param(ParamSpec::float("clock_time_h", "Clock time", 12.0, 0.0, 24.0).unit("h").help("Local standard time of the design case."))
            .param(ParamSpec::float("dni_w_m2", "Direct normal", 900.0, 0.0, 1400.0).unit("W/m2").help("Direct normal irradiance of the design hour, in W/m2."))
            .param(ParamSpec::float("dhi_w_m2", "Diffuse horizontal", 100.0, 0.0, 1000.0).unit("W/m2").help("Diffuse horizontal irradiance of the design hour, in W/m2."))
            .param(ParamSpec::float("t_air_c", "Air temperature", 45.0, -60.0, 70.0).unit("degC").help("Air temperature of the design hour, in degrees Celsius."))
            .param(ParamSpec::float("wind_m_s", "Wind speed", 0.0, 0.0, 40.0).unit("m/s").help("Wind speed of the design hour. 0 means still air."))
            .param(ParamSpec::float("t_dew_c", "Dew point", 20.0, -60.0, 50.0).unit("degC").help("Dew point of the design hour. It sets the clear-sky long-wave radiation."))
            .param(ParamSpec::float("cloud_tenths", "Opaque sky cover", 0.0, 0.0, 10.0).unit("tenths").help("Opaque sky cover in tenths, 0 clear to 10 overcast."))
            .help("Builds a one-row weather table for a single design case (default: parked at noon on a summer day, clear sky, still air).")
            .emulates("Design-case weather input of the report's hand model")
            .spec(SOLAR_SPEC)
            .implemented()
    }

    fn cook(&self, _ctx: &CookContext, _ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let loc = Location {
            name: "design".into(),
            lat: p.float("lat")?,
            lon: p.float("lon")?,
            tz: p.float("tz_h")?,
            elev: 0.0,
        };
        let year = p.int("year")? as i32;
        let month = p.int("month")? as u32;
        let day = p.int("day")? as u32;
        let clock = p.float("clock_time_h")?;
        let dni = p.float("dni_w_m2")?;
        let dhi = p.float("dhi_w_m2")?;
        let doy = day_of_year(year, month, day);
        let sp = sun_position(Site::from(&loc), year, doy, clock);
        let ghi = dni * (sp.altitude_deg * DEG).sin().max(0.0) + dhi;
        let row = Row {
            year,
            month,
            day,
            hour: clock.ceil().clamp(1.0, 24.0),
            clock_h: Some(clock),
            t_db: p.float("t_air_c")?,
            t_dp: p.float("t_dew_c")?,
            rh: 50.0,
            pressure: 101325.0,
            ghi,
            dni,
            dhi,
            ir: None,
            wind_dir: 0.0,
            wind: p.float("wind_m_s")?,
            sky_cover: p.float("cloud_tenths")?,
            opaque_cover: p.float("cloud_tenths")?,
            albedo: 0.2,
        };
        let w = Weather::new(loc.clone(), vec![row]);
        Ok(vec![
            w.to_data(),
            Data::json(serde_json::to_value(&loc).unwrap_or(Value::Null)),
        ])
    }
}

// ---------------------------------------------------------------------------
// solar.peak_hour_select
// ---------------------------------------------------------------------------

pub struct PeakHourNode;

impl NodeOp for PeakHourNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("solar.peak_hour_select", "Peak Hour Select", Stage::Solar)
            .input(PortSpec::new("weather", DataKind::Json))
            .output(PortSpec::new("weather", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(
                ParamSpec::choice(
                    "criterion",
                    "Criterion",
                    "max_ghi_where_t_air_above",
                    &["max_ghi", "max_t_air", "max_ghi_where_t_air_above"],
                )
                .help("How the design hours are ranked."),
            )
            .param(ParamSpec::float("t_air_min_c", "Minimum air temperature", 35.0, -60.0, 70.0).unit("degC").help("Used by the temperature-gated criterion."))
            .param(ParamSpec::int("count", "Count", 1, 1, 8784).help("Number of hours to keep."))
            .help("Picks the hottest or sunniest hours of a weather table as design cases for the surface temperature check.")
            .emulates("Peak-hour choice in the report's hand model")
            .spec(SOLAR_SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let jv = json_in(ins, 0)?;
        let w = Weather::from_json(&jv).map_err(|e| ctx.fail(e))?;
        let tmin = p.float("t_air_min_c")?;
        let crit = p.str("criterion")?;
        let mut idx: Vec<(f64, usize)> = Vec::new();
        for (i, r) in w.rows.iter().enumerate() {
            let score = match crit {
                "max_ghi" => r.ghi,
                "max_t_air" => r.t_db,
                "max_ghi_where_t_air_above" => {
                    if r.t_db >= tmin {
                        r.ghi
                    } else {
                        continue;
                    }
                }
                other => return Err(CookError::BadParam(format!("criterion = {other}"))),
            };
            idx.push((score, i));
        }
        if idx.is_empty() {
            return Err(ctx.fail(format!(
                "no hour has an air temperature of at least {tmin} degC"
            )));
        }
        idx.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        idx.truncate(p.int("count")?.max(1) as usize);
        let rows: Vec<Row> = idx.iter().map(|&(_, i)| w.rows[i].clone()).collect();
        let best = idx[0].1;
        let mut r = Report::new("Peak hour");
        r.metric("hour_of_year", Metric::new(w.hoy(best) as f64, "h"));
        r.metric("ghi", Metric::new(w.rows[best].ghi, "W/m2"));
        r.metric("dni", Metric::new(w.rows[best].dni, "W/m2"));
        r.metric("dhi", Metric::new(w.rows[best].dhi, "W/m2"));
        r.metric("air_temperature", Metric::new(w.rows[best].t_db, "degC"));
        r.metric("wind_speed", Metric::new(w.rows[best].wind, "m/s"));
        r.metric("selected_hours", Metric::new(rows.len() as f64, "count"));
        Ok(vec![
            Weather::new(w.location.clone(), rows).to_data(),
            Data::report(r),
        ])
    }
}

// ---------------------------------------------------------------------------
// solar.sky_temperature
// ---------------------------------------------------------------------------

pub struct SkyTemperatureNode;

impl NodeOp for SkyTemperatureNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("solar.sky_temperature", "Sky Temperature", Stage::Solar)
            .input(PortSpec::new("weather", DataKind::Json))
            .output(PortSpec::new("sky", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(
                ParamSpec::choice("source", "Source", "clark_allen", &["epw_ir", "clark_allen", "fixed_depression"])
                    .help("Measured horizontal infrared, a clear-sky emissivity correlation, or a fixed temperature drop below air."),
            )
            .param(ParamSpec::float("depression_k", "Depression", 15.0, 0.0, 60.0).unit("K").help("Sky temperature drop below air for the fixed source."))
            .param(
                ParamSpec::choice(
                    "correlation",
                    "Correlation",
                    "clark_allen",
                    &["clark_allen", "martin_berdahl", "brunt", "idso"],
                )
                .help("Clear-sky emissivity correlation."),
            )
            .help("Effective sky temperature per hour, from measured infrared, an emissivity correlation with a cloud correction, or a fixed depression below air temperature.")
            .emulates("EnergyPlus climate calculations (sky temperature)")
            .spec(SOLAR_SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let jv = json_in(ins, 0)?;
        let w = Weather::from_json(&jv).map_err(|e| ctx.fail(e))?;
        let source = p.str("source")?;
        let corr = p.str("correlation")?;
        let dep = p.float("depression_k")?;
        let mut r = Report::new("Sky temperature");
        if corr != "clark_allen" {
            placeholder(&mut r, &format!("the {corr} emissivity correlation will replace the clark_allen fallback used here"));
        }
        let (mut t_sky, mut ir_v, mut dep_v) = (Vec::new(), Vec::new(), Vec::new());
        let mut fallback = 0usize;
        for row in &w.rows {
            let (ir, ts) = match source {
                "fixed_depression" => {
                    let ts = row.t_db - dep;
                    (SIGMA * (ts + 273.15).powi(4), ts)
                }
                "epw_ir" => match row.ir {
                    Some(ir) => (ir, tsky_from_ir(ir)),
                    None => {
                        fallback += 1;
                        let ir = ir_from_corr(row.t_db, row.t_dp, row.opaque_cover);
                        (ir, tsky_from_ir(ir))
                    }
                },
                "clark_allen" => {
                    let ir = ir_from_corr(row.t_db, row.t_dp, row.opaque_cover);
                    (ir, tsky_from_ir(ir))
                }
                other => return Err(CookError::BadParam(format!("source = {other}"))),
            };
            t_sky.push(ts);
            ir_v.push(ir);
            dep_v.push(row.t_db - ts);
        }
        let n = t_sky.len().max(1) as f64;
        r.metric(
            "mean_sky_temperature",
            Metric::new(t_sky.iter().sum::<f64>() / n, "degC"),
        );
        r.metric(
            "mean_depression",
            Metric::new(dep_v.iter().sum::<f64>() / n, "K"),
        );
        r.metric(
            "mean_infrared",
            Metric::new(ir_v.iter().sum::<f64>() / n, "W/m2"),
        );
        if fallback > 0 {
            r.note(
                Severity::Info,
                format!(
                    "{fallback} rows had no measured infrared; the correlation was used for them"
                ),
            );
        }
        let table = serde_json::json!({ "schema": "sky_temperature.v1", "t_sky_c": t_sky, "ir_w_m2": ir_v, "depression_k": dep_v });
        Ok(vec![Data::json(table), Data::report(r)])
    }
}

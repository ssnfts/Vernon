//! Crate tests: registration, smoke tests for every node, and golden
//! numbers from the clean-room specs.

use std::f64::consts::PI;
use std::sync::Arc;

use vernon_core::{AttribArray, DVec3, Geometry, Prim, Report};
use vernon_graph::{CookContext, CookResult, Data, ParamValue, Params, Registry, Stage};

use crate::radiation::{incident, RadOptions};
use crate::sky::{tregenza_patches, SkyMatrix};
use crate::sun::*;
use crate::thermal::{builtin_materials, solve_surface_temp, Balance, HcModel};
use crate::weather::{parse_epw, Weather};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn registry() -> Registry {
    let mut r = Registry::new();
    crate::register(&mut r);
    r
}

fn ctx(name: &str) -> CookContext {
    CookContext {
        node_name: name.to_string(),
        work_dir: std::env::temp_dir()
            .join("vernon-nodes-solar-tests")
            .join(name.replace('.', "_")),
        frame: 0.0,
    }
}

fn cook(id: &str, inputs: &[Option<Data>], set: &[(&str, ParamValue)]) -> CookResult<Vec<Data>> {
    let reg = registry();
    let info = reg.info(id).unwrap_or_else(|| panic!("unknown node {id}"));
    let mut params = Params::from_specs(&info.params);
    for (k, v) in set {
        assert!(params.get(k).is_some(), "{id} has no param {k}");
        params.set(k, v.clone());
    }
    let out = reg.get(id).unwrap().cook(&ctx(id), inputs, &params)?;
    assert_eq!(out.len(), info.outputs.len(), "{id}: output count");
    for (d, port) in out.iter().zip(&info.outputs) {
        assert_eq!(d.kind(), port.kind, "{id}: output kind of '{}'", port.name);
    }
    Ok(out)
}

fn f(v: f64) -> ParamValue {
    ParamValue::Float(v)
}
fn i(v: i64) -> ParamValue {
    ParamValue::Int(v)
}
fn s(v: &str) -> ParamValue {
    ParamValue::Str(v.into())
}
fn b(v: bool) -> ParamValue {
    ParamValue::Bool(v)
}

fn geo(d: &Data) -> Geometry {
    (**d.as_geometry().expect("geometry")).clone()
}
fn rep(d: &Data) -> Report {
    (**d.as_report().expect("report")).clone()
}
fn m(r: &Report, name: &str) -> f64 {
    r.metrics
        .get(name)
        .unwrap_or_else(|| {
            panic!(
                "no metric {name} in {:?}",
                r.metrics.keys().collect::<Vec<_>>()
            )
        })
        .value
}
fn gd(g: Geometry) -> Option<Data> {
    Some(Data::geometry(g))
}

/// A flat quad of `w x h` mm centred at `c` facing `normal` (outward).
fn quad(c: DVec3, normal: DVec3, w: f64, h: f64) -> Geometry {
    let n = normal.normalize();
    let up = if n.z.abs() > 0.99 { DVec3::Y } else { DVec3::Z };
    let u = up.cross(n).normalize();
    let v = n.cross(u);
    let pts = vec![
        c - u * (w / 2.0) - v * (h / 2.0),
        c + u * (w / 2.0) - v * (h / 2.0),
        c + u * (w / 2.0) + v * (h / 2.0),
        c - u * (w / 2.0) + v * (h / 2.0),
    ];
    Geometry::from_polygons(pts, vec![vec![0, 1, 2, 3]])
}

/// Design-hour weather table for Dubai.
fn design(dni: f64, dhi: f64, month: i64, day: i64, clock: f64, ta: f64) -> Data {
    cook(
        "solar.design_hour",
        &[],
        &[
            ("dni_w_m2", f(dni)),
            ("dhi_w_m2", f(dhi)),
            ("month", i(month)),
            ("day", i(day)),
            ("clock_time_h", f(clock)),
            ("t_air_c", f(ta)),
        ],
    )
    .unwrap()
    .remove(0)
}

fn sky_of(weather: &Data, mf: &str, rho: f64) -> Data {
    cook(
        "solar.sky_matrix",
        &[Some(weather.clone())],
        &[("density_mf", s(mf)), ("ground_reflectance", f(rho))],
    )
    .unwrap()
    .remove(0)
}

fn matrix(d: &Data) -> SkyMatrix {
    SkyMatrix::from_json(d.as_json().unwrap()).unwrap()
}

/// Triangulated sheet of `nu x nv` quads bent into a gentle arch (a disk).
fn panel(nu: usize, nv: usize) -> Geometry {
    let mut pts = Vec::new();
    for j in 0..=nv {
        for k in 0..=nu {
            let x = k as f64 / nu as f64 * 400.0;
            let y = j as f64 / nv as f64 * 300.0;
            pts.push(DVec3::new(x, y, 0.0004 * (x - 200.0).powi(2)));
        }
    }
    let id = |k: usize, j: usize| (j * (nu + 1) + k) as u32;
    let mut faces = Vec::new();
    for j in 0..nv {
        for k in 0..nu {
            faces.push(vec![id(k, j), id(k + 1, j), id(k + 1, j + 1)]);
            faces.push(vec![id(k, j), id(k + 1, j + 1), id(k, j + 1)]);
        }
    }
    Geometry::from_polygons(pts, faces)
}

/// Half cylinder strip (angle 0..pi), radius `r`, height `h`.
fn half_cylinder(r: f64, h: f64, nu: usize, nv: usize) -> Geometry {
    let mut pts = Vec::new();
    for k in 0..=nu {
        for j in 0..=nv {
            let a = k as f64 / nu as f64 * PI;
            pts.push(DVec3::new(
                r * a.cos(),
                r * a.sin(),
                j as f64 / nv as f64 * h,
            ));
        }
    }
    let id = |k: usize, j: usize| (k * (nv + 1) + j) as u32;
    let mut faces = Vec::new();
    for k in 0..nu {
        for j in 0..nv {
            faces.push(vec![id(k, j), id(k + 1, j), id(k + 1, j + 1)]);
            faces.push(vec![id(k, j), id(k + 1, j + 1), id(k, j + 1)]);
        }
    }
    Geometry::from_polygons(pts, faces)
}

/// Closed tube around Z with `n` segments and `nv` rows (open ends).
fn tube(r: f64, h: f64, n: usize, nv: usize) -> Geometry {
    let mut pts = Vec::new();
    for j in 0..=nv {
        for k in 0..n {
            let a = k as f64 / n as f64 * 2.0 * PI;
            pts.push(DVec3::new(
                r * a.cos(),
                r * a.sin(),
                j as f64 / nv as f64 * h,
            ));
        }
    }
    let id = |k: usize, j: usize| (j * n + k % n) as u32;
    let mut faces = Vec::new();
    for j in 0..nv {
        for k in 0..n {
            faces.push(vec![id(k, j), id(k + 1, j), id(k + 1, j + 1)]);
            faces.push(vec![id(k, j), id(k + 1, j + 1), id(k, j + 1)]);
        }
    }
    Geometry::from_polygons(pts, faces)
}

/// Spherical cap, radius `r`, half angle `half_deg`, `rings` x `sectors`.
fn cap(r: f64, half_deg: f64, rings: usize, sectors: usize) -> Geometry {
    let mut pts = vec![DVec3::new(0.0, 0.0, r)];
    for i in 1..=rings {
        let th = i as f64 / rings as f64 * half_deg.to_radians();
        for j in 0..sectors {
            let ph = j as f64 / sectors as f64 * 2.0 * PI;
            pts.push(DVec3::new(
                r * th.sin() * ph.cos(),
                r * th.sin() * ph.sin(),
                r * th.cos(),
            ));
        }
    }
    let id = |i: usize, j: usize| (1 + (i - 1) * sectors + j % sectors) as u32;
    let mut faces = Vec::new();
    for j in 0..sectors {
        faces.push(vec![0, id(1, j), id(1, j + 1)]);
    }
    for i in 1..rings {
        for j in 0..sectors {
            let (a, bb, c, d) = (id(i, j), id(i, j + 1), id(i + 1, j + 1), id(i + 1, j));
            faces.push(vec![a, d, c]);
            faces.push(vec![a, c, bb]);
        }
    }
    Geometry::from_polygons(pts, faces)
}

fn uv_sphere(r: f64, stacks: usize, slices: usize) -> Geometry {
    let mut pts = vec![DVec3::new(0.0, 0.0, r)];
    for i in 1..stacks {
        let th = i as f64 / stacks as f64 * PI;
        for j in 0..slices {
            let ph = j as f64 / slices as f64 * 2.0 * PI;
            pts.push(DVec3::new(
                r * th.sin() * ph.cos(),
                r * th.sin() * ph.sin(),
                r * th.cos(),
            ));
        }
    }
    pts.push(DVec3::new(0.0, 0.0, -r));
    let bottom = (pts.len() - 1) as u32;
    let id = |i: usize, j: usize| (1 + (i - 1) * slices + j % slices) as u32;
    let mut faces = Vec::new();
    for j in 0..slices {
        faces.push(vec![0, id(1, j), id(1, j + 1)]);
        faces.push(vec![bottom, id(stacks - 1, j + 1), id(stacks - 1, j)]);
    }
    for i in 1..stacks - 1 {
        for j in 0..slices {
            let (a, bb, c, d) = (id(i, j), id(i, j + 1), id(i + 1, j + 1), id(i + 1, j));
            faces.push(vec![a, d, c]);
            faces.push(vec![a, c, bb]);
        }
    }
    Geometry::from_polygons(pts, faces)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

#[test]
fn registration_is_complete_and_clean() {
    let reg = registry();
    assert!(reg.len() >= 20, "{} nodes", reg.len());
    let forbidden = [
        "rhino",
        "fusion",
        "autodesk",
        "3ds max",
        "quicksurface",
        "geomagic",
        "zeiss",
        "airshaper",
        "simscale",
        "ansys",
        "optimumlap",
        "plasticity",
        "polycam",
        "ladybug",
        "cloudcompare",
        "houdini",
        "blender",
        "squish",
        "helio",
        "energyplus",
    ];
    for info in reg.types() {
        let solar = info.id.starts_with("solar.");
        let flatten = info.id.starts_with("flatten.");
        assert!(solar || flatten, "{}", info.id);
        assert_eq!(
            info.stage,
            if solar { Stage::Solar } else { Stage::Flatten },
            "{}",
            info.id
        );
        assert!(!info.help.is_empty(), "{} has no help", info.id);
        assert!(!info.emulates.is_empty(), "{} has no emulates", info.id);
        let spec = info.spec.as_deref().unwrap_or_default();
        assert!(
            spec.starts_with("docs/specs/") && spec.ends_with(".md"),
            "{} spec {spec}",
            info.id
        );
        assert!(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(spec)
                .exists(),
            "{spec}"
        );
        let mut text = format!("{} {}", info.label, info.help);
        for p in &info.params {
            text.push_str(&format!(" {} {}", p.label, p.help));
            if let vernon_graph::ParamKind::Enum { options, .. } = &p.kind {
                text.push_str(&options.join(" "));
            }
        }
        for pt in info.inputs.iter().chain(&info.outputs) {
            text.push_str(&format!(" {}", pt.name));
        }
        let lower = text.to_lowercase();
        for w in forbidden {
            assert!(
                !lower.contains(w),
                "{} user-visible text contains '{w}'",
                info.id
            );
        }
        for p in &info.params {
            assert!(!p.label.is_empty(), "{}.{} has no label", info.id, p.name);
        }
    }
    // The stage-gating ids exist.
    for id in [
        "solar.weather_import",
        "solar.design_hour",
        "solar.peak_hour_select",
        "solar.sun_position",
        "solar.sun_path",
        "solar.sky_temperature",
        "solar.sky_matrix",
        "solar.incident_radiation",
        "solar.sun_exposure_map",
        "solar.surface_balance",
        "solar.balance_sweep",
        "solar.material_table",
        "solar.hdt_check",
        "solar.uv_dose_proxy",
        "flatten.seam_cut",
        "flatten.conformal_map",
        "flatten.stretch_aware",
        "flatten.strain_report",
        "flatten.map_back",
        "flatten.uv_stretch_overlay",
    ] {
        assert!(reg.info(id).is_some(), "missing {id}");
    }
    // Only the genuinely incomplete node is marked white-box.
    let wb: Vec<&str> = reg
        .types()
        .filter(|t| t.whitebox)
        .map(|t| t.id.as_str())
        .collect();
    assert_eq!(wb, vec!["flatten.stretch_aware"]);
}

// ---------------------------------------------------------------------------
// Sun position (spec G5)
// ---------------------------------------------------------------------------

fn dubai() -> Site {
    Site {
        lat: 25.2048,
        lon: 55.2708,
        tz: 4.0,
    }
}

#[test]
fn sun_position_matches_the_dubai_goldens() {
    let cases = [
        (2026, 6, 21, 12.0, 85.0, 0.15, None),
        (2026, 12, 21, 12.0, 41.21, 0.05, Some((174.89, 0.1))),
        (2026, 6, 21, 7.0, 18.23, 0.15, Some((72.06, 0.1))),
        (2026, 8, 1, 15.0, 53.54, 0.15, Some((266.77, 0.5))),
        (2026, 3, 20, 12.0, 63.51, 0.25, Some((164.67, 0.3))),
    ];
    for (y, mo, d, h, alt, talt, az) in cases {
        for alg in [SunAlg::Low, SunAlg::Meeus] {
            let sp = sun_position_alg(alg, dubai(), y, mo, d, h);
            assert!(
                (sp.altitude_deg - alt).abs()
                    <= talt + if alg == SunAlg::Meeus { 0.15 } else { 0.0 },
                "{alg:?} {mo}-{d} {h}: alt {}",
                sp.altitude_deg
            );
            if let Some((a, ta)) = az {
                assert!(
                    (sp.azimuth_deg - a).abs() <= ta + if alg == SunAlg::Meeus { 0.3 } else { 0.0 },
                    "{alg:?} {mo}-{d} {h}: az {}",
                    sp.azimuth_deg
                );
            }
        }
    }
    // Azimuth is finite at the equinox and near the zenith.
    let sp = sun_position_alg(
        SunAlg::Low,
        Site {
            lat: 0.0,
            lon: 0.0,
            tz: 0.0,
        },
        2026,
        3,
        20,
        12.0,
    );
    assert!(sp.azimuth_deg.is_finite() && sp.altitude_deg > 85.0);
}

#[test]
fn sunrise_noon_sunset_match_the_goldens() {
    let hm = |h: f64| (h * 60.0).round();
    let jun = sun_times(SunAlg::Low, dubai(), 2026, 6, 21);
    assert!((hm(jun.sunrise_h.unwrap()) - (5.0 * 60.0 + 29.0)).abs() <= 2.0);
    assert!((hm(jun.noon_h) - (12.0 * 60.0 + 20.0)).abs() <= 2.0);
    assert!((hm(jun.sunset_h.unwrap()) - (19.0 * 60.0 + 11.0)).abs() <= 2.0);
    assert!((jun.day_length_h - 13.71).abs() < 0.05);
    let dec = sun_times(SunAlg::Low, dubai(), 2026, 12, 21);
    assert!((hm(dec.sunrise_h.unwrap()) - 7.0 * 60.0).abs() <= 2.0);
    assert!((hm(dec.noon_h) - (12.0 * 60.0 + 17.0)).abs() <= 2.0);
    assert!((hm(dec.sunset_h.unwrap()) - (17.0 * 60.0 + 34.0)).abs() <= 2.0);
    let polar = sun_times(
        SunAlg::Low,
        Site {
            lat: 80.0,
            lon: 0.0,
            tz: 0.0,
        },
        2026,
        12,
        21,
    );
    assert!(polar.sunrise_h.is_none() && polar.day_length_h == 0.0);
}

#[test]
fn sun_vector_conventions() {
    let v = sun_vector(180.0, 45.0, 0.0);
    let r2 = std::f64::consts::FRAC_1_SQRT_2;
    assert!((v - DVec3::new(0.0, -r2, r2)).length() < 1e-12);
    assert!((sun_vector(90.0, 0.0, 0.0) - DVec3::X).length() < 1e-12);
    assert!((sun_vector(0.0, 0.0, 0.0) - DVec3::Y).length() < 1e-12);
    assert!((sun_vector(10.0, 90.0, 0.0) - DVec3::Z).length() < 1e-12);
    // A north angle of 90 puts model north towards -X: the north sun lies on -X.
    assert!((sun_vector(0.0, 0.0, 90.0) - DVec3::NEG_X).length() < 1e-12);
    assert!((sun_vector(33.0, 17.0, 5.0).length() - 1.0).abs() < 1e-12);
    // Refraction is zero high in the sky and about 0.5 degree at the horizon.
    assert_eq!(refraction_deg(86.0), 0.0);
    assert!((refraction_deg(0.0) - 0.48).abs() < 0.1);
}

#[test]
fn sun_position_node_reports_the_day() {
    let out = cook(
        "solar.sun_position",
        &[],
        &[("month", i(12)), ("day", i(21)), ("clock_time_h", f(12.0))],
    )
    .unwrap();
    let r = rep(&out[1]);
    assert!((m(&r, "altitude") - 41.21).abs() < 0.05);
    assert!((m(&r, "azimuth") - 174.89).abs() < 0.1);
    assert!((m(&r, "day_length") - 10.57).abs() < 0.05);
    let t = SunTable::from_json(out[0].as_json().unwrap()).unwrap();
    assert_eq!(t.samples.len(), 1);
    assert!((t.samples[0].vec[2] - (41.21f64).to_radians().sin()).abs() < 1e-3);
    // The spa option falls back with a placeholder warning.
    let out = cook("solar.sun_position", &[], &[("algorithm", s("spa"))]).unwrap();
    assert!(rep(&out[1])
        .notes
        .iter()
        .any(|(sev, t)| *sev == vernon_core::Severity::Warning
            && t.starts_with("white-box placeholder")));
}

#[test]
fn sun_path_builds_polylines_above_the_horizon() {
    let out = cook("solar.sun_path", &[], &[]).unwrap();
    let g = geo(&out[0]);
    assert_eq!(g.prims.len(), 1);
    assert!(g
        .points
        .iter()
        .all(|p| p.z > 0.0 && (p.length() - 1000.0).abs() < 1e-6));
    let out = cook("solar.sun_path", &[], &[("mode", s("analemma"))]).unwrap();
    assert!(geo(&out[0]).point_count() > 300);
}

// ---------------------------------------------------------------------------
// Weather (spec G10, G4)
// ---------------------------------------------------------------------------

fn fake_epw(year_len_days: u32, tz: f64, dni9999_row: Option<usize>) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "LOCATION,Testville,DU,ARE,Synthetic,412230,25.25,55.33,{tz:.1},5.0\n"
    ));
    s.push_str("DESIGN CONDITIONS,0\nTYPICAL/EXTREME PERIODS,0\nGROUND TEMPERATURES,0\nHOLIDAYS/DAYLIGHT SAVINGS,No,0,0,0\nCOMMENTS 1,test\nCOMMENTS 2,test\nDATA PERIODS,1,1,Data,Sunday, 1/ 1,12/31\n");
    let months = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = year_len_days == 366;
    let site = Site {
        lat: 25.25,
        lon: 55.33,
        tz,
    };
    let mut n = 0usize;
    for (mi, dim) in months.iter().enumerate() {
        let dim = if mi == 1 && leap { 29 } else { *dim };
        for d in 1..=dim {
            for h in 1..=24 {
                let doy = day_of_year(if leap { 2024 } else { 2026 }, mi as u32 + 1, d);
                let sp = sun_position(site, if leap { 2024 } else { 2026 }, doy, h as f64 - 0.5);
                let (dni, dhi) = if sp.altitude_deg > 0.0 {
                    (500.0, 100.0)
                } else {
                    (0.0, 0.0)
                };
                let ghi = dni * sp.altitude_deg.to_radians().sin().max(0.0) + dhi;
                let dni_field = if Some(n) == dni9999_row {
                    "9999".to_string()
                } else {
                    format!("{dni}")
                };
                let mut f = vec!["0".to_string(); 35];
                f[0] = format!("{}", if leap { 2024 } else { 2026 });
                f[1] = format!("{}", mi + 1);
                f[2] = format!("{d}");
                f[3] = format!("{h}");
                f[4] = "60".into();
                f[6] = "30.0".into();
                f[7] = "15.0".into();
                f[8] = "40".into();
                f[9] = "101325".into();
                f[12] = "400".into();
                f[13] = format!("{ghi}");
                f[14] = dni_field;
                f[15] = format!("{dhi}");
                f[20] = "180".into();
                f[21] = "3.0".into();
                f[22] = "2".into();
                f[23] = "2".into();
                f[32] = "0.2".into();
                s.push_str(&f.join(","));
                s.push('\n');
                n += 1;
            }
        }
    }
    s
}

#[test]
fn epw_round_trips_and_handles_missing_codes() {
    let (w, st) = parse_epw(&fake_epw(365, 4.0, Some(30)), true, true).unwrap();
    assert_eq!(w.rows.len(), 8760);
    assert_eq!(st.rows, 8760);
    assert_eq!(st.missing_dni, 1);
    assert_eq!(w.rows[30].dni, 0.0);
    assert_eq!(w.location.tz, 4.0);
    assert_eq!(w.location.elev, 5.0);
    assert!((w.location.lat - 25.25).abs() < 1e-12);
    // hour 24 of day 1 is hour-of-year index 23
    assert_eq!(w.rows[23].hour, 24.0);
    assert_eq!(w.hoy(23), 23);
    assert_eq!(w.hoy(8759), 8759);
    // negative time zone parses as f64
    let (w2, _) = parse_epw(&fake_epw(365, -6.0, None), true, true).unwrap();
    assert_eq!(w2.location.tz, -6.0);
    // leap year
    let (w3, _) = parse_epw(&fake_epw(366, 4.0, None), true, true).unwrap();
    assert_eq!(w3.rows.len(), 8784);
    // wrong field count
    let bad = fake_epw(365, 4.0, None).replacen(",0.2,0,0\n", ",0.2,0\n", 1);
    let err = parse_epw(&bad, true, true).unwrap_err();
    assert!(err.starts_with("row 9:"), "{err}");
    assert!(parse_epw(&bad, false, true).is_ok());
}

#[test]
fn weather_import_node_reads_a_file() {
    let dir = std::env::temp_dir()
        .join("vernon-nodes-solar-tests")
        .join("epw");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("synthetic.epw");
    std::fs::write(&path, fake_epw(365, 4.0, Some(5))).unwrap();
    let out = cook(
        "solar.weather_import",
        &[],
        &[("path", s(path.to_str().unwrap()))],
    )
    .unwrap();
    let r = rep(&out[1]);
    assert_eq!(m(&r, "rows"), 8760.0);
    assert_eq!(m(&r, "missing_dni"), 1.0);
    assert!(Weather::from_json(out[0].as_json().unwrap()).is_ok());
    assert!(cook("solar.weather_import", &[], &[]).is_err());
    // peak hour: the hottest hour is all the same (30 degC); the sunniest hour is selected
    let pk = cook(
        "solar.peak_hour_select",
        &[Some(out[0].clone())],
        &[("criterion", s("max_ghi")), ("count", i(3))],
    )
    .unwrap();
    let w = Weather::from_json(pk[0].as_json().unwrap()).unwrap();
    assert_eq!(w.rows.len(), 3);
    assert!(w.rows[0].ghi >= w.rows[2].ghi);
    // temperature gate that nothing passes is a clean error
    assert!(cook(
        "solar.peak_hour_select",
        &[Some(out[0].clone())],
        &[("t_air_min_c", f(60.0))]
    )
    .is_err());
}

#[test]
fn sky_temperature_matches_the_worked_examples() {
    let t = crate::weather::ir_from_corr(20.0, 10.0, 0.0);
    assert!((t - 341.2).abs() < 0.1, "{t}");
    assert!((crate::weather::tsky_from_ir(t) - 5.38).abs() < 0.05);
    assert!((crate::weather::eps_clear_clark_allen(10.0) - 0.8149).abs() < 1e-4);
    let ir = crate::weather::ir_from_corr(45.0, 20.0, 0.0);
    assert!((ir - 488.8).abs() < 0.3, "{ir}");
    assert!((crate::weather::tsky_from_ir(ir) - 31.56).abs() < 0.05);
    let eps5 = crate::weather::eps_clear_clark_allen(20.0) * crate::weather::cloud_factor(5.0);
    assert!((eps5 - 0.8915).abs() < 1e-3);
    assert!(
        (crate::weather::tsky_from_ir(crate::weather::ir_from_corr(45.0, 20.0, 5.0)) - 35.99).abs()
            < 0.1
    );
    // node: fixed depression gives exactly 15 K
    let w = design(900.0, 100.0, 6, 21, 12.0, 45.0);
    let out = cook(
        "solar.sky_temperature",
        &[Some(w.clone())],
        &[("source", s("fixed_depression"))],
    )
    .unwrap();
    assert!((m(&rep(&out[1]), "mean_depression") - 15.0).abs() < 1e-9);
    let out = cook(
        "solar.sky_temperature",
        &[Some(w)],
        &[("correlation", s("brunt"))],
    )
    .unwrap();
    assert!(rep(&out[1])
        .notes
        .iter()
        .any(|(_, t)| t.starts_with("white-box placeholder")));
}

// ---------------------------------------------------------------------------
// Sky patches (spec G6)
// ---------------------------------------------------------------------------

#[test]
fn tregenza_patch_geometry() {
    for (mf, n) in [(1usize, 145usize), (2, 577), (4, 2305)] {
        let p = tregenza_patches(mf);
        assert_eq!(p.len(), n);
        let total: f64 = p.iter().map(|x| x.omega).sum();
        assert!((total - 2.0 * PI).abs() < 1e-9, "mf {mf}: {total}");
    }
    let p = tregenza_patches(1);
    let rows = [
        0.043545, 0.041642, 0.047398, 0.040673, 0.042893, 0.044522, 0.045517,
    ];
    let mut idx = 0;
    for (row, count) in [30, 30, 24, 24, 18, 12, 6].iter().enumerate() {
        assert!(
            (p[idx].omega - rows[row]).abs() < 1e-6,
            "row {row}: {}",
            p[idx].omega
        );
        idx += count;
    }
    assert!((p[144].omega - 0.034420).abs() < 1e-6);
    // cosine-weighted horizontal sum / pi and a south-facing wall
    let horiz = [1.0055, 1.0015, 1.0004];
    let wall = [0.4979, 0.5002, 0.5000];
    for (k, mf) in [1usize, 2, 4].iter().enumerate() {
        let p = tregenza_patches(*mf);
        let h: f64 = p.iter().map(|x| x.omega * x.dir.z.max(0.0)).sum::<f64>() / PI;
        let w: f64 = p
            .iter()
            .map(|x| x.omega * x.dir.dot(DVec3::NEG_Y).max(0.0))
            .sum::<f64>()
            / PI;
        assert!((h - horiz[k]).abs() < 1e-4, "mf {mf}: horizontal {h}");
        assert!((w - wall[k]).abs() < 1e-4, "mf {mf}: wall {w}");
    }
}

#[test]
fn sky_matrix_node_outputs() {
    let w = design(900.0, 100.0, 6, 21, 12.0, 45.0);
    let out = cook(
        "solar.sky_matrix",
        &[Some(w.clone())],
        &[("density_mf", s("2"))],
    )
    .unwrap();
    let mx = matrix(&out[0]);
    assert_eq!(mx.lsky.len(), 577);
    assert_eq!(geo(&out[1]).point_count(), 577);
    let r = rep(&out[2]);
    assert!((m(&r, "solid_angle_sum") - 2.0 * PI).abs() < 1e-9);
    // beam conserved: horizontal component of the spread beam equals dni sin(alt)
    let p = tregenza_patches(2);
    let horiz: f64 = p.iter().zip(&mx.bsun).map(|(q, b)| b * q.dir.z).sum();
    let alt = sun_position(dubai(), 2026, day_of_year(2026, 6, 21), 12.0).altitude_deg;
    assert!(
        (horiz - 900.0 * alt.to_radians().sin()).abs() < 1e-6,
        "{horiz}"
    );
    // perez requested: placeholder warning, isotropic result
    let out = cook("solar.sky_matrix", &[Some(w)], &[("sky_model", s("perez"))]).unwrap();
    assert!(rep(&out[2])
        .notes
        .iter()
        .any(|(_, t)| t.starts_with("white-box placeholder")));
}

// ---------------------------------------------------------------------------
// Incident radiation (spec G7, G8, G9)
// ---------------------------------------------------------------------------

fn opts() -> RadOptions {
    RadOptions {
        offset_mm: 0.1,
        ..Default::default()
    }
}

#[test]
fn uniform_sky_on_horizontal_and_vertical_faces() {
    for (mf, tol) in [(1usize, 0.010), (2, 0.002), (4, 0.001)] {
        let sky = SkyMatrix::uniform(mf, 100.0);
        let r = incident(
            &quad(DVec3::ZERO, DVec3::Z, 10.0, 10.0),
            None,
            &sky,
            &opts(),
        );
        assert!(
            (r.e_wh[0] / 100.0 - 1.0).abs() < tol,
            "mf {mf}: {}",
            r.e_wh[0]
        );
        let r = incident(
            &quad(DVec3::ZERO, DVec3::NEG_Y, 10.0, 10.0),
            None,
            &sky,
            &opts(),
        );
        assert!(
            (r.e_wh[0] / 50.0 - 1.0).abs() < 0.005,
            "mf {mf}: wall {}",
            r.e_wh[0]
        );
    }
}

#[test]
fn occlusion_blocks_the_dome() {
    let sky = SkyMatrix::uniform(2, 100.0);
    let plate = quad(DVec3::new(0.0, 0.0, 1.0), DVec3::Z, 2.0e6, 2.0e6);
    let r = incident(
        &quad(DVec3::ZERO, DVec3::Z, 10.0, 10.0),
        Some(&plate),
        &sky,
        &opts(),
    );
    assert_eq!(r.e_wh[0], 0.0);
    // a tall wall beside the point blocks about half of the dome
    for (mf, gate) in [(1usize, 0.06), (2, 0.04), (4, 0.01)] {
        let sky = SkyMatrix::uniform(mf, 100.0);
        let wall = quad(DVec3::new(0.0, 0.0, 5.0e6), DVec3::X, 1.0e7, 1.0e7);
        let r = incident(
            &quad(DVec3::new(1.0, 0.0, 0.0), DVec3::Z, 0.5, 0.5),
            Some(&wall),
            &sky,
            &opts(),
        );
        assert!(
            (r.e_wh[0] / 50.0 - 1.0).abs() < gate,
            "mf {mf}: {}",
            r.e_wh[0]
        );
    }
    // occlusion off ignores the context
    let o = RadOptions {
        occlusion: false,
        ..opts()
    };
    let r = incident(
        &quad(DVec3::ZERO, DVec3::Z, 10.0, 10.0),
        Some(&plate),
        &SkyMatrix::uniform(1, 100.0),
        &o,
    );
    assert!(r.e_wh[0] > 90.0);
}

#[test]
fn ground_term_and_totals() {
    // ground radiance only: GHI 1000, rho 0.2
    let mut sky = SkyMatrix::uniform(1, 0.0);
    sky.lsky = vec![0.0; 145];
    sky.lgnd = 0.2 * 1000.0 / PI;
    let r = incident(
        &quad(DVec3::ZERO, DVec3::NEG_Y, 10.0, 10.0),
        None,
        &sky,
        &opts(),
    );
    assert!((r.e_wh[0] - 100.0).abs() < 1.0, "{}", r.e_wh[0]);
    let r = incident(
        &quad(DVec3::ZERO, DVec3::Z, 10.0, 10.0),
        None,
        &sky,
        &opts(),
    );
    assert_eq!(r.e_wh[0], 0.0);
    // total kWh = sum(E * A): a 1 m x 2 m quad is 2 m2
    let w = design(900.0, 100.0, 6, 21, 12.0, 45.0);
    let skyd = sky_of(&w, "1", 0.2);
    let out = cook(
        "solar.incident_radiation",
        &[Some(skyd), gd(quad(DVec3::ZERO, DVec3::Z, 1000.0, 2000.0))],
        &[("irradiance", b(true))],
    )
    .unwrap();
    let r = rep(&out[1]);
    let g = geo(&out[0]);
    g.validate().unwrap();
    let kwh = g.prim_attribs.floats("irr_kwh_m2").unwrap()[0];
    let wm2 = g.prim_attribs.floats("irr_w_m2").unwrap()[0];
    assert!(
        (kwh * 1000.0 - wm2).abs() < 1e-9,
        "one hour: Wh/m2 equals W/m2"
    );
    assert!((m(&r, "total_kwh") - kwh * 2.0).abs() < 1e-12);
    assert_eq!(m(&r, "faces"), 1.0);
    // sub-sampling large faces does not change a uniform result
    let out2 = cook(
        "solar.incident_radiation",
        &[
            Some(sky_of(&w, "1", 0.2)),
            gd(quad(DVec3::ZERO, DVec3::Z, 1000.0, 2000.0)),
        ],
        &[("grid_size_mm", f(100.0))],
    )
    .unwrap();
    let kwh2 = geo(&out2[0]).prim_attribs.floats("irr_kwh_m2").unwrap()[0];
    assert!((kwh2 - kwh).abs() / kwh < 1e-9);
}

#[test]
fn direct_beam_patch_and_exact_modes() {
    // Dubai 21 Dec 12:00, DNI 800, no diffuse, no ground.
    let w = design(800.0, 0.0, 12, 21, 12.0, 25.0);
    let south = quad(DVec3::ZERO, DVec3::NEG_Y, 10.0, 10.0);
    let flat = quad(DVec3::ZERO, DVec3::Z, 10.0, 10.0);
    let run = |mf: &str, mode: &str, g: &Geometry| -> f64 {
        let out = cook(
            "solar.incident_radiation",
            &[Some(sky_of(&w, mf, 0.0)), gd(g.clone())],
            &[
                ("sun_mode", s(mode)),
                ("ground_mode", s("none")),
                ("occlusion", b(false)),
                ("irradiance", b(true)),
            ],
        )
        .unwrap();
        geo(&out[0]).prim_attribs.floats("irr_w_m2").unwrap()[0]
    };
    assert!((run("1", "exact", &flat) - 527.1).abs() < 0.3);
    let exact_south = run("1", "exact", &south);
    assert!((exact_south - 599.4).abs() < 0.6, "{exact_south}");
    for (mf, gate) in [("1", 0.05), ("2", 0.05), ("4", 0.04)] {
        let v = run(mf, "patch", &south);
        assert!((v / exact_south - 1.0).abs() < gate, "mf {mf}: {v}");
        // the horizontal component of the beam is conserved exactly
        assert!((run(mf, "patch", &flat) / run(mf, "exact", &flat) - 1.0).abs() < 1e-4);
    }
}

#[test]
fn shadow_from_context_blocks_the_beam() {
    let w = design(800.0, 0.0, 12, 21, 12.0, 25.0);
    let flat = quad(DVec3::ZERO, DVec3::Z, 10.0, 10.0);
    let roof = quad(DVec3::new(0.0, 0.0, 50.0), DVec3::Z, 1.0e5, 1.0e5);
    let out = cook(
        "solar.incident_radiation",
        &[Some(sky_of(&w, "1", 0.0)), gd(flat), gd(roof)],
        &[
            ("sun_mode", s("exact")),
            ("ground_mode", s("none")),
            ("irradiance", b(true)),
        ],
    )
    .unwrap();
    assert_eq!(
        geo(&out[0]).prim_attribs.floats("irr_w_m2").unwrap()[0],
        0.0
    );
}

#[test]
fn incident_radiation_is_independent_of_the_thread_count() {
    let w = design(900.0, 100.0, 6, 21, 12.0, 45.0);
    let sky = matrix(&sky_of(&w, "1", 0.2));
    let g = panel(6, 4);
    let run = |threads: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| incident(&g, None, &sky, &RadOptions::default()).e_wh)
    };
    let a = run(1);
    let c = run(8);
    assert_eq!(a, c);
    assert!(a.iter().all(|v| *v > 0.0));
}

#[test]
fn synthetic_year_horizontal_matches_the_weather_totals() {
    let (wx, _) = parse_epw(&fake_epw(365, 4.0, None), true, true).unwrap();
    let annual_ghi: f64 = wx.rows.iter().map(|r| r.ghi).sum::<f64>() / 1000.0;
    let sky = matrix(
        &cook(
            "solar.sky_matrix",
            &[Some(wx.to_data())],
            &[("sun_time_offset_h", f(-0.5))],
        )
        .unwrap()
        .remove(0),
    );
    assert_eq!(sky.n_hours, 8760.0);
    let o = RadOptions {
        ground: false,
        occlusion: false,
        ..opts()
    };
    let r = incident(&quad(DVec3::ZERO, DVec3::Z, 10.0, 10.0), None, &sky, &o);
    let kwh = r.e_wh[0] / 1000.0;
    assert!(
        (kwh / annual_ghi - 1.0).abs() < 0.01,
        "{kwh} vs {annual_ghi}"
    );
}

#[test]
fn per_vertex_subsamples_and_daylight_average() {
    // A flat sheet under a uniform sky: face and vertex analysis agree.
    let mut sheet = panel(3, 2);
    for p in &mut sheet.points {
        p.z = 0.0;
    }
    let w = design(900.0, 100.0, 6, 21, 12.0, 45.0);
    let sky = sky_of(&w, "1", 0.2);
    let run = |set: &[(&str, ParamValue)]| {
        let out = cook(
            "solar.incident_radiation",
            &[Some(sky.clone()), gd(sheet.clone())],
            set,
        )
        .unwrap();
        geo(&out[0])
    };
    let by_face = run(&[("irradiance", b(true))]);
    let by_vertex = run(&[("irradiance", b(true)), ("by_vertex", b(true))]);
    let f = by_face.prim_attribs.floats("irr_w_m2").unwrap();
    let v = by_vertex.prim_attribs.floats("irr_w_m2").unwrap();
    for (a, c) in f.iter().zip(v) {
        assert!((a - c).abs() < 1e-9, "{a} vs {c}");
    }
    assert_eq!(
        by_vertex.point_attribs.floats("irr_w_m2").unwrap().len(),
        sheet.point_count()
    );
    // four rays per patch stay close to the analytic uniform-sky value
    let sky1 = SkyMatrix::uniform(1, 100.0);
    let o4 = RadOptions { subs: 4, ..opts() };
    let h = incident(&quad(DVec3::ZERO, DVec3::Z, 10.0, 10.0), None, &sky1, &o4).e_wh[0];
    let wl = incident(
        &quad(DVec3::ZERO, DVec3::NEG_Y, 10.0, 10.0),
        None,
        &sky1,
        &o4,
    )
    .e_wh[0];
    assert!((h / 100.0 - 1.0).abs() < 0.01, "{h}");
    assert!((wl / 50.0 - 1.0).abs() < 0.01, "{wl}");
    // wall-foot error with 4 rays at the coarse density is not worse than with 1
    let wall = quad(DVec3::new(0.0, 0.0, 5.0e6), DVec3::X, 1.0e7, 1.0e7);
    let foot = quad(DVec3::new(1.0, 0.0, 0.0), DVec3::Z, 0.5, 0.5);
    let e1 = incident(&foot, Some(&wall), &sky1, &opts()).e_wh[0];
    let e4 = incident(&foot, Some(&wall), &sky1, &o4).e_wh[0];
    assert!(
        (e4 / 50.0 - 1.0).abs() <= (e1 / 50.0 - 1.0).abs() + 1e-9,
        "{e1} {e4}"
    );
    // averaging over daylight hours equals the all-hours value for a one-hour period
    let d = run(&[("irradiance", b(true)), ("average_over", s("daylight"))]);
    assert_eq!(d.prim_attribs.floats("irr_w_m2").unwrap(), f);
}

#[test]
fn intersection_matrix_rows() {
    let w = design(900.0, 100.0, 6, 21, 12.0, 45.0);
    let out = cook(
        "solar.incident_radiation",
        &[
            Some(sky_of(&w, "1", 0.2)),
            gd(quad(DVec3::ZERO, DVec3::Z, 10.0, 10.0)),
        ],
        &[("export_matrix", b(true))],
    )
    .unwrap();
    let mx = out[2].as_json().unwrap();
    let rows = mx["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let row = rows[0].as_array().unwrap();
    assert_eq!(row.len(), 145);
    // horizontal face: every value is the cosine of the zenith angle
    let p = tregenza_patches(1);
    for (k, v) in row.iter().enumerate() {
        assert!((v.as_f64().unwrap() - p[k].dir.z.max(0.0)).abs() < 1e-5);
    }
}

#[test]
fn sun_exposure_map_cosine_and_shadow() {
    let sphere = uv_sphere(100.0, 12, 24);
    let mut chk = sphere.clone();
    chk.compute_normals();
    for (i, p) in chk.prims.iter().enumerate() {
        let c = p
            .vertices
            .iter()
            .map(|&v| chk.points[v as usize])
            .sum::<DVec3>()
            / p.vertices.len() as f64;
        assert!(
            chk.prim_attribs.vec3s("N").unwrap()[i].dot(c) > 0.0,
            "sphere face {i} points inward"
        );
    }
    let out = cook(
        "solar.sun_exposure_map",
        &[gd(sphere.clone())],
        &[("elevation_deg", f(90.0)), ("normalise", b(false))],
    )
    .unwrap();
    let g = geo(&out[0]);
    let e = g.prim_attribs.floats("sun_exposure").unwrap();
    let n = chk.prim_attribs.vec3s("N").unwrap();
    for i in 0..e.len() {
        let z = n[i].z.max(0.0);
        assert!((e[i] - z).abs() < 1e-9, "face {i}: {} vs {z}", e[i]);
        if n[i].z < 0.0 {
            assert_eq!(e[i], 0.0);
        }
    }
    assert!(e.iter().cloned().fold(0.0, f64::max) > 0.9);
    // flat quad, sun at 45 degrees elevation, un-normalised
    let out = cook(
        "solar.sun_exposure_map",
        &[gd(quad(DVec3::ZERO, DVec3::Z, 10.0, 10.0))],
        &[("elevation_deg", f(45.0)), ("normalise", b(false))],
    )
    .unwrap();
    let e = geo(&out[0]).prim_attribs.floats("sun_exposure").unwrap()[0];
    assert!((e - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
    let w = geo(&out[0]).prim_attribs.floats("irr_w_m2").unwrap()[0];
    assert!((w - 900.0 * std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
    // shadow
    let roof = quad(DVec3::new(0.0, 0.0, 30.0), DVec3::Z, 1.0e5, 1.0e5);
    let out = cook(
        "solar.sun_exposure_map",
        &[gd(quad(DVec3::ZERO, DVec3::Z, 10.0, 10.0)), gd(roof)],
        &[("normalise", b(false))],
    )
    .unwrap();
    assert_eq!(
        geo(&out[0]).prim_attribs.floats("sun_exposure").unwrap()[0],
        0.0
    );
    assert_eq!(m(&rep(&out[1]), "lit_area_pct"), 0.0);
}

// ---------------------------------------------------------------------------
// Surface balance and heat deflection (spec G1, G2, G3, G11)
// ---------------------------------------------------------------------------

fn balance(alpha: f64, g: f64, ta_c: f64, wind: f64) -> Balance {
    Balance {
        alpha,
        eps: 0.9,
        g,
        ta_k: ta_c + 273.15,
        tsky_k: ta_c - 15.0 + 273.15,
        wind,
        cos_tilt: 1.0,
        tg_k: ta_c + 273.15,
        hb: 0.0,
        tin_k: ta_c + 273.15,
        model: HcModel::McAdams,
    }
}

fn ts(b: &Balance) -> f64 {
    let (t, _, ok) = solve_surface_temp(b);
    assert!(ok);
    t - 273.15
}

#[test]
fn balance_reproduces_the_report_table() {
    let table = [
        (
            0.95,
            [(0.0, 104.7, 108.1), (1.0, 93.4, 97.3), (3.0, 79.6, 83.9)],
            [(105, 108), (93, 97), (80, 84)],
        ),
        (
            0.60,
            [(0.0, 82.6, 86.4), (1.0, 74.8, 79.0), (3.0, 65.9, 70.4)],
            [(83, 86), (75, 79), (66, 70)],
        ),
        (
            0.30,
            [(0.0, 61.3, 65.5), (1.0, 57.6, 62.1), (3.0, 53.7, 58.4)],
            [(61, 66), (58, 62), (54, 58)],
        ),
    ];
    for (alpha, rows, ints) in table {
        for (k, (v, t45, t50)) in rows.iter().enumerate() {
            let a = ts(&balance(alpha, 1000.0, 45.0, *v));
            let c = ts(&balance(alpha, 1000.0, 50.0, *v));
            assert!(
                (a - t45).abs() <= 0.2,
                "alpha {alpha} V {v} Ta45: {a} vs {t45}"
            );
            assert!(
                (c - t50).abs() <= 0.2,
                "alpha {alpha} V {v} Ta50: {c} vs {t50}"
            );
            assert_eq!(
                a.round() as i32,
                ints[k].0,
                "alpha {alpha} V {v} Ta45 rounds to {}",
                ints[k].0
            );
            if !(alpha == 0.30 && *v == 0.0) {
                assert_eq!(
                    c.round() as i32,
                    ints[k].1,
                    "alpha {alpha} V {v} Ta50 rounds to {}",
                    ints[k].1
                );
            }
        }
    }
    // the default constants matter: Tsky = Ta - 15 and hc = 5.7 + 3.8 V (not Ta - 6 / 10.45 - v + 10 sqrt v)
    assert!(ts(&balance(0.95, 1000.0, 45.0, 0.0)) > 100.0);
}

#[test]
fn balance_sensitivities() {
    let mut b0 = balance(0.95, 1000.0, 45.0, 0.0);
    b0.eps = 0.8;
    assert!((ts(&b0) - 109.05).abs() < 0.2);
    b0.eps = 0.95;
    assert!((ts(&b0) - 102.76).abs() < 0.2);
    for (dep, expect) in [(0.0, 110.15), (25.0, 101.46)] {
        let mut b1 = balance(0.95, 1000.0, 45.0, 0.0);
        b1.tsky_k = 45.0 - dep + 273.15;
        assert!((ts(&b1) - expect).abs() < 0.2, "depression {dep}");
    }
    for (tin, expect) in [(45.0, 90.58), (60.0, 94.21)] {
        let mut b1 = balance(0.95, 1000.0, 45.0, 0.0);
        b1.hb = 5.0;
        b1.tin_k = tin + 273.15;
        assert!((ts(&b1) - expect).abs() < 0.2, "Tin {tin}");
    }
    let mut v = balance(0.95, 600.0, 45.0, 0.0);
    v.cos_tilt = 0.0;
    assert!((ts(&v) - 83.62).abs() < 0.2);
    v.tg_k = 45.0 + 20.0 + 273.15;
    assert!((ts(&v) - 88.38).abs() < 0.2);
    let mut t = balance(0.6, 900.0, 50.0, 3.0);
    t.cos_tilt = 20.0f64.to_radians().cos();
    assert!((ts(&t) - 68.16).abs() < 0.2);
    // the wind-tunnel correlation convects less than 5.7 + 3.8 V at 3 m/s: hotter than the default, cooler than still air
    let mut mo = balance(0.95, 1000.0, 45.0, 3.0);
    let default_ts = ts(&mo);
    mo.model = HcModel::MowittWindward;
    assert!(ts(&mo) > default_ts && ts(&mo) < 104.7, "{}", ts(&mo));
}

#[test]
fn balance_sweep_node_matches_the_table() {
    let out = cook("solar.balance_sweep", &[], &[]).unwrap();
    let r = rep(&out[1]);
    assert!((m(&r, "ts_c.alpha0.95.v0.ta45") - 104.7).abs() < 0.2);
    assert!((m(&r, "ts_c.alpha0.3.v3.ta45") - 53.7).abs() < 0.2);
    assert_eq!(m(&r, "non_converged"), 0.0);
    assert_eq!(
        out[0].as_json().unwrap()["rows"].as_array().unwrap().len(),
        18
    );
    assert!(cook("solar.balance_sweep", &[], &[("alphas", s("0.9, x"))]).is_err());
}

fn with_temp(t: f64) -> Data {
    let mut g = quad(DVec3::ZERO, DVec3::Z, 100.0, 100.0);
    g.prim_attribs.set("temp_c", AttribArray::Float(vec![t]));
    Data::geometry(g)
}

fn verdict(temp: f64, material: &str, annealed: bool, warn: f64) -> (i64, Report) {
    let out = cook(
        "solar.hdt_check",
        &[Some(with_temp(temp))],
        &[
            ("material", s(material)),
            ("annealed", b(annealed)),
            ("margin_warn_k", f(warn)),
        ],
    )
    .unwrap();
    (
        geo(&out[0]).prim_attribs.ints("hdt_verdict").unwrap()[0],
        rep(&out[1]),
    )
}

#[test]
fn hdt_verdicts_match_the_goldens() {
    // black, still air, Ta 45: 104.7 degC
    for (mat, fail) in [
        ("PLA", true),
        ("PETG", true),
        ("ASA", true),
        ("PET-CF", false),
        ("PPA-CF", false),
    ] {
        let (v, r) = verdict(104.7, mat, false, 0.0);
        assert_eq!(v == 2, fail, "{mat}");
        assert_eq!(r.passes(), !fail, "{mat}");
    }
    assert_eq!(verdict(104.7, "PAHT-CF", true, 0.0).0, 0);
    assert_eq!(verdict(104.7, "PAHT-CF", false, 0.0).0, 2);
    // white, still air: 61.3
    assert_eq!(verdict(61.3, "PLA", false, 0.0).0, 2);
    assert_eq!(verdict(61.3, "PETG", false, 0.0).0, 0);
    assert_eq!(verdict(61.3, "ASA", false, 0.0).0, 0);
    // white, 3 m/s: 53.7 -- PLA ok at margin 0, warn at 10 K
    assert_eq!(verdict(53.7, "PLA", false, 0.0).0, 0);
    let (v, r) = verdict(53.7, "PLA", false, 10.0);
    assert_eq!(v, 1);
    assert!(r.passes(), "a warning alone does not fail the stage gate");
    let (_, r) = verdict(104.7, "ASA", false, 10.0);
    assert!((m(&r, "min_margin_k") - (100.0 - 104.7)).abs() < 1e-9);
    assert_eq!(m(&r, "fail_area_pct"), 100.0);
    // a material with no heat-deflection value is a clean error, a glass-transition cap applies
    assert!(cook(
        "solar.hdt_check",
        &[Some(with_temp(50.0))],
        &[("material", s("PA6-CF"))]
    )
    .is_err());
    assert_eq!(verdict(55.0, "CF-ABS", false, 0.0).0, 2);
    // the material report overrides the table
    let mut mr = Report::new("material");
    mr.metric("hdt_c", vernon_core::Metric::new(120.0, "degC"));
    let out = cook(
        "solar.hdt_check",
        &[Some(with_temp(104.7)), None, Some(Data::report(mr))],
        &[("material", s("PLA"))],
    )
    .unwrap();
    assert_eq!(geo(&out[0]).prim_attribs.ints("hdt_verdict").unwrap()[0], 0);
    assert!((m(&rep(&out[1]), "min_margin_k") - 15.3).abs() < 1e-9);
    assert_eq!(builtin_materials().len(), 8);
    // the printable rows are the shared table the print and structural stages read
    for s in &vernon_core::materials::PRINT_MATERIALS {
        let row = builtin_materials().into_iter().find(|r| r.name == s.name).unwrap();
        assert_eq!(row.hdt_eff(false, false), Some(s.hdt(false)), "{}", s.name);
        assert_eq!(row.hdt_eff(true, false), Some(s.hdt(true)), "{}", s.name);
    }
    // the structural-stage material report names it hdt_045_c: still used
    let mut fea = Report::new("material");
    fea.metric("hdt_045_c", vernon_core::Metric::new(101.5, "degC"));
    let out = cook(
        "solar.hdt_check",
        &[Some(with_temp(50.0)), None, Some(Data::report(fea))],
        &[("material", s("ASA"))],
    )
    .unwrap();
    let r = rep(&out[1]);
    assert_eq!(m(&r, "hdt_c"), 101.5);
    assert!(r.notes.iter().any(|(_, t)| t.contains("(hdt_045_c)")));
    // a connected report without any heat-deflection metric is flagged
    let out = cook(
        "solar.hdt_check",
        &[Some(with_temp(50.0)), None, Some(Data::report(Report::new("empty")))],
        &[("material", s("ASA"))],
    )
    .unwrap();
    let r = rep(&out[1]);
    assert_eq!(m(&r, "hdt_c"), 100.0);
    assert!(r.notes.iter().any(|(sev, t)| *sev == vernon_core::Severity::Warning && t.contains("no heat-deflection metric")));
}

#[test]
fn material_table_node_and_csv() {
    let out = cook("solar.material_table", &[], &[]).unwrap();
    assert_eq!(
        out[0].as_json().unwrap()["rows"].as_array().unwrap().len(),
        8
    );
    let dir = std::env::temp_dir()
        .join("vernon-nodes-solar-tests")
        .join("mat");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("m.csv");
    std::fs::write(
        &path,
        "name,hdt_c,hdt_annealed_c,tg_c,water_abs_pct,source\nXYZ,130,,,0.1,lab\n",
    )
    .unwrap();
    let out = cook(
        "solar.material_table",
        &[],
        &[("csv_path", s(path.to_str().unwrap()))],
    )
    .unwrap();
    let rows = out[0].as_json().unwrap()["rows"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "XYZ");
    // use the user table in the check
    let out2 = cook(
        "solar.hdt_check",
        &[Some(with_temp(100.0)), Some(out[0].clone())],
        &[("material", s("xyz"))],
    )
    .unwrap();
    assert!((m(&rep(&out2[1]), "min_margin_k") - 30.0).abs() < 1e-9);
}

#[test]
fn end_to_end_design_chain() {
    // Dubai summer noon, DNI 900, DHI 100, Ta 45, still air: black ASA panel fails, white ok.
    let w = design(900.0, 100.0, 6, 21, 12.0, 45.0);
    let alt = sun_position(dubai(), 2026, day_of_year(2026, 6, 21), 12.0).altitude_deg;
    let sky = sky_of(&w, "1", 0.2);
    let rad = cook(
        "solar.incident_radiation",
        &[Some(sky), gd(quad(DVec3::ZERO, DVec3::Z, 500.0, 500.0))],
        &[("irradiance", b(true))],
    )
    .unwrap();
    let g = geo(&rad[0]);
    let gw = g.prim_attribs.floats("irr_w_m2").unwrap()[0];
    let expect = 900.0 * alt.to_radians().sin() + 100.0;
    assert!(
        (gw / expect - 1.0).abs() < 0.01,
        "G = {gw}, expected about {expect}"
    );
    let run = |alpha: f64| {
        let bal = cook(
            "solar.surface_balance",
            &[Some(Data::geometry(g.clone())), Some(w.clone())],
            &[("alpha", f(alpha))],
        )
        .unwrap();
        let chk = cook(
            "solar.hdt_check",
            &[Some(bal[0].clone())],
            &[("material", s("ASA"))],
        )
        .unwrap();
        (rep(&bal[1]), rep(&chk[1]), geo(&bal[0]))
    };
    let (br, hr, bg) = run(0.95);
    assert!(
        (m(&br, "max_temp_c") - 104.6).abs() < 0.6,
        "{}",
        m(&br, "max_temp_c")
    );
    assert!(!hr.passes());
    assert_eq!(m(&br, "non_converged_faces"), 0.0);
    bg.validate().unwrap();
    assert!(bg.point_attribs.contains("temp_c"));
    let (br2, hr2, _) = run(0.30);
    assert!((m(&br2, "max_temp_c") - 61.0).abs() < 0.6);
    assert!(hr2.passes());
    // vertical wall at noon on 21 Dec (DNI 800, DHI 100, rho 0.2): about 712 W/m2
    let w2 = design(800.0, 100.0, 12, 21, 12.0, 25.0);
    let rad = cook(
        "solar.incident_radiation",
        &[
            Some(sky_of(&w2, "4", 0.2)),
            gd(quad(DVec3::ZERO, DVec3::NEG_Y, 100.0, 100.0)),
        ],
        &[("irradiance", b(true))],
    )
    .unwrap();
    let wall = geo(&rad[0]).prim_attribs.floats("irr_w_m2").unwrap()[0];
    assert!((wall / 712.0 - 1.0).abs() < 0.05, "{wall}");
}

#[test]
fn surface_balance_uses_per_face_alpha_and_weather_wind() {
    let mut g = panel(2, 1);
    let n = g.prims.len();
    g.prim_attribs
        .set("irr_w_m2", AttribArray::Float(vec![1000.0; n]));
    g.prim_attribs.set(
        "alpha",
        AttribArray::Float(
            (0..n)
                .map(|k| if k % 2 == 0 { 0.95 } else { 0.3 })
                .collect(),
        ),
    );
    let out = cook("solar.surface_balance", &[gd(g)], &[]).unwrap();
    let t = geo(&out[0]).prim_attribs.floats("temp_c").unwrap().to_vec();
    assert!(t[0] > t[1] + 20.0);
}

// ---------------------------------------------------------------------------
// Flatten
// ---------------------------------------------------------------------------

fn edge_lengths(g: &Geometry) -> Vec<f64> {
    let mut v = Vec::new();
    for (_, t) in g.triangles() {
        for k in 0..3 {
            let (a, c) = (t[k] as usize, t[(k + 1) % 3] as usize);
            v.push(g.points[a].distance(g.points[c]));
        }
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v
}

fn angles(p: [DVec3; 3]) -> [f64; 3] {
    let mut out = [0.0; 3];
    for k in 0..3 {
        let (a, c) = (p[(k + 1) % 3] - p[k], p[(k + 2) % 3] - p[k]);
        out[k] = a.cross(c).length().atan2(a.dot(c)).to_degrees();
    }
    out
}

fn tight() -> Vec<(&'static str, ParamValue)> {
    vec![
        ("solver_tolerance", f(1e-14)),
        ("max_iterations", i(500_000)),
    ]
}

#[test]
fn half_cylinder_flattens_without_strain() {
    let g = half_cylinder(100.0, 200.0, 12, 6);
    let out = cook("flatten.conformal_map", &[gd(g.clone())], &tight()).unwrap();
    let r = rep(&out[2]);
    assert!(
        m(&r, "max_length_change_pct") < 1e-6,
        "{}",
        m(&r, "max_length_change_pct")
    );
    assert!(m(&r, "area_change_pct").abs() < 1e-8);
    assert_eq!(m(&r, "flipped_tris"), 0.0);
    assert!(r.passes());
    assert_eq!(m(&r, "boundary_loops"), 1.0);
    // developable: the angle defect vanishes
    assert!(
        m(&r, "angle_defect_rad") < 1e-3,
        "defect {}",
        m(&r, "angle_defect_rad")
    );
    // pattern: a rectangle of 12 chords x the height; uv and strain attributes exist
    let pat = geo(&out[1]);
    pat.validate().unwrap();
    assert!(pat.points.iter().all(|p| p.z == 0.0));
    let size = pat.bounds().size();
    let chord_sum = 12.0 * 2.0 * 100.0 * (PI / 24.0).sin();
    let (long, short) = (size.x.max(size.y), size.x.min(size.y));
    assert!((long - chord_sum).abs() < 1e-5, "{long} vs {chord_sum}");
    assert!((short - 200.0).abs() < 1e-5, "{short}");
    let mesh = geo(&out[0]);
    assert!(mesh.point_attribs.vec3s("uv").is_ok());
    assert!(mesh.prim_attribs.floats("stretch_pct").is_ok());
    // every pattern edge keeps its 3-D length
    let (a, c) = (edge_lengths(&mesh), edge_lengths(&pat));
    for (x, y) in a.iter().zip(&c) {
        assert!((x - y).abs() < 1e-6 * x.max(1.0), "{x} vs {y}");
    }
    // the total pattern area equals the 3-D area
    assert!((pat.surface_area() / g.surface_area() - 1.0).abs() < 1e-9);
}

#[test]
fn conformal_flatten_is_invariant_to_rigid_motion() {
    let g = cap(100.0, 40.0, 6, 24);
    let mut moved = g.clone();
    let rot = vernon_core::DMat4::from_rotation_translation(
        glam::DQuat::from_axis_angle(DVec3::new(1.0, 2.0, 3.0).normalize(), 0.9),
        DVec3::new(50.0, -20.0, 7.0),
    );
    moved.transform(rot);
    let a = cook("flatten.conformal_map", &[gd(g)], &tight()).unwrap();
    let c = cook("flatten.conformal_map", &[gd(moved)], &tight()).unwrap();
    let (ea, ec) = (edge_lengths(&geo(&a[1])), edge_lengths(&geo(&c[1])));
    for (x, y) in ea.iter().zip(&ec) {
        assert!((x - y).abs() < 1e-6, "{x} vs {y}");
    }
    assert!(
        (m(&rep(&a[2]), "max_length_change_pct") - m(&rep(&c[2]), "max_length_change_pct")).abs()
            < 1e-6
    );
}

#[test]
fn spherical_cap_flatten_is_conformal() {
    let g = cap(100.0, 40.0, 6, 24);
    let mut chk = g.clone();
    chk.compute_normals();
    for (k, p) in chk.prims.iter().enumerate() {
        let c = p
            .vertices
            .iter()
            .map(|&v| chk.points[v as usize])
            .sum::<DVec3>();
        assert!(
            chk.prim_attribs.vec3s("N").unwrap()[k].dot(c) > 0.0,
            "cap face {k} points inward"
        );
    }
    let out = cook("flatten.conformal_map", &[gd(g.clone())], &[]).unwrap();
    let r = rep(&out[2]);
    assert_eq!(m(&r, "flipped_tris"), 0.0);
    assert!(m(&r, "area_change_pct").abs() < 1e-8);
    assert!(
        m(&r, "max_length_change_pct") > 0.1,
        "a sphere cannot flatten without strain"
    );
    // angles are preserved to within a few degrees
    let mesh = geo(&out[0]);
    let pat = geo(&out[1]);
    let mut worst: f64 = 0.0;
    for (_, t) in mesh.triangles() {
        let p3 = [0, 1, 2].map(|k| mesh.points[t[k] as usize]);
        let p2 = [0, 1, 2].map(|k| pat.points[t[k] as usize]);
        let (a3, a2) = (angles(p3), angles(p2));
        for k in 0..3 {
            worst = worst.max((a3[k] - a2[k]).abs());
        }
    }
    assert!(worst < 2.5, "worst angle error {worst}");
    // isoperimetric inequality for the pattern: L^2 >= 4 pi A, so the boundary must lengthen
    let topo = crate::topo::analyze(&mesh);
    let lp = &topo.loops[0];
    let (mut l3, mut l2) = (0.0, 0.0);
    for k in 0..lp.len() {
        let (a, c) = (lp[k] as usize, lp[(k + 1) % lp.len()] as usize);
        l3 += mesh.points[a].distance(mesh.points[c]);
        l2 += pat.points[a].distance(pat.points[c]);
    }
    assert!(l2 * l2 >= 4.0 * PI * pat.surface_area() * (1.0 - 1e-9));
    assert!(l2 / l3 - 1.0 > 0.05, "boundary change {}", l2 / l3 - 1.0);
    assert_eq!(m(&r, "seam_paths"), 0.0);
}

#[test]
fn closed_tube_is_cut_and_flattens_to_a_rectangle() {
    let g = tube(100.0, 200.0, 128, 4);
    let out = cook("flatten.conformal_map", &[gd(g)], &[]).unwrap();
    let r = rep(&out[2]);
    assert_eq!(m(&r, "seam_paths"), 1.0);
    assert!(m(&r, "area_change_pct").abs() < 0.05);
    assert!(
        m(&r, "max_length_change_pct") < 0.1,
        "{}",
        m(&r, "max_length_change_pct")
    );
    let size = geo(&out[1]).bounds().size();
    let circumference = 2.0 * 128.0 * 100.0 * (PI / 128.0).sin();
    assert!(
        (size.x.max(size.y) - circumference).abs() / circumference < 0.01,
        "{size:?}"
    );
    // the cut mesh remembers where its points came from
    let mesh = geo(&out[0]);
    assert!(mesh.point_attribs.ints("src_point").is_ok());
    // refusing to cut is an error
    assert!(cook(
        "flatten.conformal_map",
        &[gd(tube(100.0, 200.0, 16, 2))],
        &[("auto_seam", b(false))]
    )
    .is_err());
}

#[test]
fn seam_cut_node() {
    // an open tube has two boundary loops; one cut joins them
    let g = tube(50.0, 100.0, 24, 3);
    let out = cook("flatten.seam_cut", &[gd(g.clone())], &[]).unwrap();
    let r = rep(&out[2]);
    assert_eq!(m(&r, "boundary_loops_before"), 2.0);
    assert_eq!(m(&r, "boundary_loops"), 1.0);
    assert_eq!(m(&r, "euler_char"), 1.0);
    assert_eq!(m(&r, "is_disk"), 1.0);
    let cut = geo(&out[0]);
    cut.validate().unwrap();
    assert!(cut.point_count() > g.point_count());
    // new boundary = original boundaries + two copies of the seam
    let before: f64 = edge_len_sum(&g, &crate::topo::analyze(&g));
    let ta = crate::topo::analyze(&cut);
    let after = edge_len_sum(&cut, &ta);
    let seam_len: f64 = {
        let sg = geo(&out[1]);
        sg.prims
            .iter()
            .map(|p| {
                p.vertices
                    .windows(2)
                    .map(|w| sg.points[w[0] as usize].distance(sg.points[w[1] as usize]))
                    .sum::<f64>()
            })
            .sum()
    };
    assert!(
        (after - before - 2.0 * seam_len).abs() < 1e-6,
        "{after} {before} {seam_len}"
    );
    // a disk passes through unchanged
    let d = panel(3, 2);
    let out = cook("flatten.seam_cut", &[gd(d.clone())], &[]).unwrap();
    assert_eq!(geo(&out[0]).point_count(), d.point_count());
    // a user slit from the boundary into the interior cuts one boundary vertex in two
    let p0 = d.points[4];
    let p1 = d.points[5];
    let mut seam = Geometry::new();
    let (s0, s1) = (seam.add_point(p0), seam.add_point(p1));
    seam.add_prim(Prim::polyline(vec![s0, s1]));
    let out = cook(
        "flatten.seam_cut",
        &[gd(d.clone()), gd(seam)],
        &[("auto", b(false))],
    )
    .unwrap();
    assert_eq!(geo(&out[0]).point_count(), d.point_count() + 1);
    assert_eq!(m(&rep(&out[2]), "seam_paths"), 1.0);
    assert_eq!(m(&rep(&out[2]), "is_disk"), 1.0);
}

fn edge_len_sum(g: &Geometry, t: &crate::topo::TopoInfo) -> f64 {
    t.loops
        .iter()
        .map(|lp| {
            (0..lp.len())
                .map(|k| {
                    g.points[lp[k] as usize].distance(g.points[lp[(k + 1) % lp.len()] as usize])
                })
                .sum::<f64>()
        })
        .sum()
}

#[test]
fn plane_fit_projects_a_tilted_quad() {
    let g = quad(
        DVec3::new(5.0, 6.0, 7.0),
        DVec3::new(0.3, -0.4, 0.8),
        100.0,
        100.0,
    );
    let out = cook(
        "flatten.conformal_map",
        &[gd(g.clone())],
        &[("method", s("plane_fit"))],
    )
    .unwrap();
    let r = rep(&out[2]);
    assert_eq!(m(&r, "flipped_tris"), 0.0);
    assert!(m(&r, "max_length_change_pct") < 1e-9);
    assert!((geo(&out[1]).surface_area() - 10000.0).abs() < 1e-8);
    // collinear points fail cleanly
    let line = Geometry::from_polygons(
        vec![DVec3::ZERO, DVec3::X, DVec3::new(2.0, 0.0, 0.0)],
        vec![vec![0, 1, 2]],
    );
    assert!(cook(
        "flatten.conformal_map",
        &[gd(line)],
        &[("method", s("plane_fit"))]
    )
    .is_err());
    assert!(cook("flatten.conformal_map", &[gd(g)], &[("method", s("bogus"))]).is_err());
}

#[test]
fn strain_report_scales_and_flips() {
    let g = panel(3, 2);
    let uv: Vec<DVec3> = g
        .points
        .iter()
        .map(|p| DVec3::new(p.x * 1.1, p.y * 1.1, 0.0))
        .collect();
    // use a flat sheet so the scale is pure
    let flat = {
        let mut f = g.clone();
        for p in &mut f.points {
            p.z = 0.0;
        }
        f
    };
    let mut pat = flat.clone();
    pat.points = uv.clone();
    let out = cook("flatten.strain_report", &[gd(flat.clone()), gd(pat)], &[]).unwrap();
    let r = rep(&out[2]);
    assert!((m(&r, "stretch_max_pct") - 10.0).abs() < 1e-8);
    assert_eq!(m(&r, "compress_max_pct"), 0.0);
    assert!((m(&r, "area_change_pct") - 21.0).abs() < 1e-8);
    assert_eq!(m(&r, "stretch_share_pct").round(), 100.0);
    assert!(!r.passes(), "10 percent exceeds the 5 percent limit");
    let markers = geo(&out[1]);
    assert_eq!(markers.point_count(), 10);
    assert!(markers
        .point_attribs
        .floats("strain_pct")
        .unwrap()
        .iter()
        .all(|v| (*v - 10.0).abs() < 1e-8));
    // mirrored triangle
    let mut pat = flat.clone();
    pat.points = flat.points.clone();
    let k = 5;
    pat.points[k] = DVec3::new(pat.points[k].x, -pat.points[k].y - 500.0, 0.0);
    let out = cook("flatten.strain_report", &[gd(flat.clone()), gd(pat)], &[]).unwrap();
    let r = rep(&out[2]);
    assert!(m(&r, "flipped_tris") >= 1.0);
    assert!(!r.passes());
    // missing pattern and uv
    assert!(cook("flatten.strain_report", &[gd(flat)], &[]).is_err());
}

#[test]
fn uv_overlay_colours_by_stretch() {
    let mut flat = panel(3, 2);
    for p in &mut flat.points {
        p.z = 0.0;
    }
    let n = flat.points.len();
    flat.point_attribs.set(
        "uv",
        AttribArray::Vec3(
            flat.points
                .iter()
                .map(|p| DVec3::new(p.x * 1.02, p.y, 0.0))
                .collect(),
        ),
    );
    let out = cook("flatten.uv_stretch_overlay", &[gd(flat.clone())], &[]).unwrap();
    let g = geo(&out[0]);
    g.validate().unwrap();
    let cd = g.point_attribs.vec3s("Cd").unwrap();
    assert_eq!(cd.len(), n);
    // stretched: red channel stays high, blue falls
    assert!(cd.iter().all(|c| c.x > c.z));
    let pat = geo(&out[1]);
    assert!((pat.points[1].x - flat.points[1].x * 1.02).abs() < 1e-9);
    assert!((m(&rep(&out[2]), "stretch_max_pct") - 2.0).abs() < 1e-8);
    assert!(cook(
        "flatten.uv_stretch_overlay",
        &[gd(flat)],
        &[("mode", s("bogus"))]
    )
    .is_err());
}

#[test]
fn map_back_is_exact_on_vertices_and_follows_the_surface() {
    let g = half_cylinder(100.0, 200.0, 12, 6);
    let out = cook("flatten.conformal_map", &[gd(g)], &[]).unwrap();
    let mesh = geo(&out[0]);
    let pat = geo(&out[1]);
    // all pattern vertices map to the original vertices
    let cloud = Geometry::from_points(pat.points.clone());
    let back = cook(
        "flatten.map_back",
        &[gd(pat.clone()), gd(mesh.clone()), gd(cloud)],
        &[],
    )
    .unwrap();
    let mg = geo(&back[0]);
    assert_eq!(mg.point_count(), mesh.point_count());
    let diag = mesh.bounds().size().length();
    for (k, p) in mg.points.iter().enumerate() {
        assert!(p.distance(mesh.points[k]) < 1e-9 * diag, "vertex {k}");
    }
    // a straight line across the pattern: the mapped curve has the same length (isometry)
    let bb = pat.bounds();
    let a = bb.min + (bb.max - bb.min) * DVec3::new(0.1, 0.15, 0.0);
    let c = bb.min + (bb.max - bb.min) * DVec3::new(0.9, 0.8, 0.0);
    let mut line = Geometry::new();
    let (i0, i1) = (line.add_point(a), line.add_point(c));
    line.add_prim(Prim::polyline(vec![i0, i1]));
    let back = cook(
        "flatten.map_back",
        &[gd(pat.clone()), gd(mesh.clone()), gd(line)],
        &[],
    )
    .unwrap();
    let r = rep(&back[1]);
    assert!((m(&r, "mapped_length_mm") - a.distance(c)).abs() < 1e-6 * a.distance(c));
    assert_eq!(m(&r, "outside_length_mm"), 0.0);
    let mapped = geo(&back[0]);
    assert!(
        mapped.point_count() > 3,
        "the curve is split at triangle edges"
    );
    // mapped points stay on or inside the cylinder surface (facets are chords)
    for p in &mapped.points {
        let rad = (p.x * p.x + p.y * p.y).sqrt();
        assert!(
            rad <= 100.0 + 1e-6 && rad > 100.0 * (PI / 24.0).cos() - 1e-6,
            "radius {rad}"
        );
    }
    // a curve leaving the pattern is clipped and reported
    let mut out_line = Geometry::new();
    let (j0, j1) = (
        out_line.add_point(a),
        out_line.add_point(bb.max + DVec3::new(500.0, 0.0, 0.0)),
    );
    out_line.add_prim(Prim::polyline(vec![j0, j1]));
    let back = cook("flatten.map_back", &[gd(pat), gd(mesh), gd(out_line)], &[]).unwrap();
    assert!(m(&rep(&back[1]), "outside_length_mm") > 100.0);
}

#[test]
fn stretch_aware_flatten_runs_and_responds_to_its_parameters() {
    let g = cap(100.0, 30.0, 6, 24);
    let run = |set: &[(&str, ParamValue)]| {
        let out = cook("flatten.stretch_aware", &[gd(g.clone())], set).unwrap();
        (rep(&out[2]), geo(&out[0]), geo(&out[1]))
    };
    let (free, mesh, pat) = run(&[]);
    assert_eq!(m(&free, "flipped_tris"), 0.0);
    assert!(m(&free, "arap_iterations") >= 1.0);
    assert!(free
        .notes
        .iter()
        .any(|(_, t)| t.starts_with("white-box placeholder")));
    // the refinement does not make the worst length change worse than the conformal start
    assert!(m(&free, "max_length_change_pct") <= m(&free, "initial_max_length_change_pct") + 1e-9);
    mesh.validate().unwrap();
    pat.validate().unwrap();
    // preserving the boundary keeps boundary lengths closer
    let bnd_change = |mesh: &Geometry, pat: &Geometry| -> f64 {
        let t = crate::topo::analyze(mesh);
        let lp = &t.loops[0];
        let mut s = 0.0;
        for k in 0..lp.len() {
            let (a, c) = (lp[k] as usize, lp[(k + 1) % lp.len()] as usize);
            s += (pat.points[a].distance(pat.points[c]) / mesh.points[a].distance(mesh.points[c])
                - 1.0)
                .abs();
        }
        s / lp.len() as f64
    };
    let (_, m2, p2) = run(&[("preserve_boundary", b(true))]);
    assert!(
        bnd_change(&m2, &p2) < bnd_change(&mesh, &pat),
        "{} vs {}",
        bnd_change(&m2, &p2),
        bnd_change(&mesh, &pat)
    );
    // biasing changes the outcome: stretch-only compresses less than compress-only
    let (so, _, _) = run(&[("deformation", s("stretch_only"))]);
    let (co, _, _) = run(&[("deformation", s("compress_only"))]);
    assert!(m(&so, "compress_max_pct") <= m(&co, "compress_max_pct") + 1e-9);
    assert!(m(&co, "stretch_max_pct") <= m(&so, "stretch_max_pct") + 1e-9);
    assert!(cook(
        "flatten.stretch_aware",
        &[gd(g)],
        &[("deformation", s("bogus"))]
    )
    .is_err());
}

// ---------------------------------------------------------------------------
// Smoke: every node cooks on small synthetic inputs with default parameters
// ---------------------------------------------------------------------------

#[test]
fn every_node_cooks_with_defaults() {
    let reg = registry();
    let weather = design(900.0, 100.0, 6, 21, 12.0, 45.0);
    let sky = sky_of(&weather, "1", 0.2);
    let sun = cook("solar.sun_position", &[Some(weather.clone())], &[])
        .unwrap()
        .remove(0);
    let panel_g = panel(4, 3);
    let radiated = cook(
        "solar.incident_radiation",
        &[Some(sky.clone()), gd(panel_g.clone())],
        &[],
    )
    .unwrap()
    .remove(0);
    let balanced = cook("solar.surface_balance", &[Some(radiated.clone())], &[])
        .unwrap()
        .remove(0);
    let flat = cook("flatten.conformal_map", &[gd(panel_g.clone())], &[]).unwrap();
    let materials = cook("solar.material_table", &[], &[]).unwrap().remove(0);
    let dir = std::env::temp_dir()
        .join("vernon-nodes-solar-tests")
        .join("smoke");
    std::fs::create_dir_all(&dir).unwrap();
    let epw = dir.join("smoke.epw");
    std::fs::write(&epw, fake_epw(365, 4.0, None)).unwrap();
    for info in reg.types() {
        let some = |d: &Data| Some(d.clone());
        let inputs: Vec<Option<Data>> = match info.id.as_str() {
            "solar.peak_hour_select" | "solar.sky_temperature" | "solar.sky_matrix" => {
                vec![some(&weather)]
            }
            "solar.sun_position" => vec![some(&weather)],
            "solar.incident_radiation" => vec![some(&sky), gd(panel_g.clone())],
            "solar.sun_exposure_map" => vec![gd(panel_g.clone()), None, some(&sun)],
            "solar.uv_dose_proxy" => vec![some(&radiated)],
            "solar.surface_balance" => vec![some(&radiated), some(&weather)],
            "solar.hdt_check" => vec![some(&balanced), some(&materials)],
            "flatten.seam_cut" | "flatten.conformal_map" | "flatten.stretch_aware" => {
                vec![gd(panel_g.clone())]
            }
            "flatten.strain_report" | "flatten.uv_stretch_overlay" => {
                vec![some(&flat[0]), some(&flat[1])]
            }
            "flatten.map_back" => vec![
                some(&flat[1]),
                some(&flat[0]),
                gd(Geometry::from_points(vec![flat_point(&flat[1])])),
            ],
            _ => vec![],
        };
        let mut params = Params::from_specs(&info.params);
        if info.id == "solar.weather_import" {
            params.set("path", s(epw.to_str().unwrap()));
        }
        let out = reg
            .get(&info.id)
            .unwrap()
            .cook(&ctx(&info.id), &inputs, &params)
            .unwrap_or_else(|e| panic!("{} failed: {e}", info.id));
        assert_eq!(out.len(), info.outputs.len(), "{}", info.id);
        for (d, port) in out.iter().zip(&info.outputs) {
            assert_eq!(d.kind(), port.kind, "{} output {}", info.id, port.name);
            if let Data::Geometry(g) = d {
                g.validate().unwrap_or_else(|e| panic!("{}: {e}", info.id));
            }
        }
        // missing required inputs are clean errors, not panics
        let none: Vec<Option<Data>> = vec![None; info.inputs.len()];
        let required = info.inputs.iter().any(|p| p.required);
        let r = reg.get(&info.id).unwrap().cook(
            &ctx(&info.id),
            &none,
            &Params::from_specs(&info.params),
        );
        if required {
            assert!(r.is_err(), "{} should reject missing inputs", info.id);
        }
    }
}

fn flat_point(pattern: &Data) -> DVec3 {
    let g = pattern.as_geometry().unwrap();
    let c = g.bounds().center();
    DVec3::new(c.x, c.y, 0.0)
}

#[test]
fn shared_data_is_not_mutated() {
    // cooking must leave the input geometry untouched (Arc'd inputs)
    let g = Arc::new(panel(3, 2));
    let before = (*g).clone();
    let w = design(900.0, 100.0, 6, 21, 12.0, 45.0);
    let _ = cook(
        "solar.incident_radiation",
        &[Some(sky_of(&w, "1", 0.2)), Some(Data::Geometry(g.clone()))],
        &[],
    )
    .unwrap();
    let _ = cook(
        "flatten.conformal_map",
        &[Some(Data::Geometry(g.clone()))],
        &[],
    )
    .unwrap();
    assert_eq!(*g, before);
    let _ = Prim::polygon(vec![0, 1, 2]);
}

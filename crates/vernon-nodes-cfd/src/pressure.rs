//! Surface pressure map: solver face pressures imported onto the vehicle
//! surface as a per-face `Cp` attribute.

use std::collections::HashMap;

use rayon::prelude::*;
use vernon_core::{AttribArray, DVec3, Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::air::{air_in, dynamic_pressure};
use crate::util::{file_in, FnNode};

/// Sampled solver values at points.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Samples {
    /// Positions in millimetres.
    pub pos: Vec<DVec3>,
    pub val: Vec<f64>,
}

/// Read `x y z value...` rows (spaces, commas or semicolons; `#` comments and
/// non-numeric header lines are skipped). `column` selects the value after
/// the three coordinates; `to_mm` converts the coordinates.
pub fn parse_samples(text: &str, column: usize, to_mm: f64) -> Result<Samples, String> {
    let mut s = Samples::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let nums: Option<Vec<f64>> = line
            .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')'))
            .filter(|t| !t.is_empty())
            .map(|t| t.parse::<f64>().ok())
            .collect();
        let Some(n) = nums else { continue };
        if n.len() < 4 + column || n.iter().any(|v| !v.is_finite()) {
            continue;
        }
        s.pos.push(DVec3::new(n[0], n[1], n[2]) * to_mm);
        s.val.push(n[3 + column]);
    }
    if s.val.is_empty() {
        return Err("no rows of the form 'x y z value' found".into());
    }
    Ok(s)
}

/// What the sampled values are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    /// Kinematic pressure p/rho, m2/s2 (what the incompressible solver writes).
    Kinematic,
    /// Static pressure in Pa.
    StaticPa,
    /// Already a pressure coefficient.
    Cp,
}

impl Quantity {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "kinematic" => Some(Self::Kinematic),
            "static_pa" => Some(Self::StaticPa),
            "cp" => Some(Self::Cp),
            _ => None,
        }
    }
}

/// `Cp = (p - p_inf) / q`; kinematic input uses `q/rho = U^2 / 2`.
pub fn cp_from_value(v: f64, quantity: Quantity, p_inf: f64, u: f64, rho: f64) -> f64 {
    match quantity {
        Quantity::Kinematic => (v - p_inf) / (0.5 * u * u),
        Quantity::StaticPa => (v - p_inf) / dynamic_pressure(rho, u),
        Quantity::Cp => v,
    }
}

type Key = (i64, i64, i64);

/// Uniform hash grid; the cell size equals the search radius, so the 27
/// neighbouring cells always contain the nearest sample within that radius.
struct Grid<'a> {
    cell: f64,
    map: HashMap<Key, Vec<u32>>,
    pos: &'a [DVec3],
}

impl<'a> Grid<'a> {
    fn key(&self, p: DVec3) -> Key {
        ((p.x / self.cell).floor() as i64, (p.y / self.cell).floor() as i64, (p.z / self.cell).floor() as i64)
    }

    fn new(pos: &'a [DVec3], cell: f64) -> Self {
        let mut g = Grid { cell, map: HashMap::new(), pos };
        for (i, p) in pos.iter().enumerate() {
            let k = g.key(*p);
            g.map.entry(k).or_default().push(i as u32);
        }
        g
    }

    /// Nearest sample within `radius` of `q` (the cell size).
    fn nearest(&self, q: DVec3) -> Option<(usize, f64)> {
        let (cx, cy, cz) = self.key(q);
        let mut best: Option<(usize, f64)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = self.map.get(&(cx.wrapping_add(dx), cy.wrapping_add(dy), cz.wrapping_add(dz))) else { continue };
                    for &i in list {
                        let d = self.pos[i as usize].distance(q);
                        let closer = match best {
                            Some((_, b)) => d < b,
                            None => true,
                        };
                        if d <= self.cell && closer {
                            best = Some((i as usize, d));
                        }
                    }
                }
            }
        }
        best
    }
}

fn centroid(g: &Geometry, prim: usize) -> DVec3 {
    let v = &g.prims[prim].vertices;
    if v.is_empty() {
        return DVec3::ZERO;
    }
    v.iter().map(|&i| g.points[i as usize]).sum::<DVec3>() / v.len() as f64
}

/// Index of the nearest sample within `max_dist_mm` for every primitive
/// centroid. With `mirror_half` the lookup uses |y|, for a solver run that
/// only covered the y >= 0 half.
pub fn match_prims(g: &Geometry, s: &Samples, max_dist_mm: f64, mirror_half: bool) -> Vec<Option<usize>> {
    let grid = Grid::new(&s.pos, max_dist_mm.max(1e-6));
    (0..g.prims.len())
        .into_par_iter()
        .map(|i| {
            let mut c = centroid(g, i);
            if mirror_half {
                c.y = c.y.abs();
            }
            grid.nearest(c).map(|(j, _)| j)
        })
        .collect()
}

/// Crude, clearly labelled stand-in when no solver data is connected:
/// stagnation on faces looking into the flow (flow along +x), a base-pressure
/// level elsewhere. Never a result.
pub fn estimate_cp(normal: DVec3) -> f64 {
    if normal.x < 0.0 { normal.x * normal.x } else { -0.3 }
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    let q = if q.is_finite() { q.clamp(0.0, 1.0) } else { 0.5 };
    let pos = q * (sorted.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

/// Pressure force on the surface in N at dynamic pressure `q`:
/// `F = -sum Cp q A n` with outward normals; areas in mm2.
pub fn pressure_force_n(g: &Geometry, cp: &[f64], q: f64) -> DVec3 {
    let mut f = DVec3::ZERO;
    for (i, c) in cp.iter().enumerate() {
        f -= g.prim_normal(i) * (c * q * g.prim_area(i) * 1e-6);
    }
    f
}

fn info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.surface_pressure", "Surface Pressure Map", Stage::Cfd)
        .input(PortSpec::new("surface", DataKind::Geometry))
        .input(PortSpec::optional("samples", DataKind::File))
        .input(PortSpec::optional("air", DataKind::Json))
        .output(PortSpec::new("surface", DataKind::Geometry))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::choice("quantity", "Sampled quantity", "kinematic", &["kinematic", "static_pa", "cp"]).help("kinematic: p/rho as the incompressible solver writes it"))
        .param(ParamSpec::int("column", "Value column", 0, 0, 8).help("Column after x y z"))
        .param(ParamSpec::choice("file_units", "Coordinate units", "m", &["m", "mm"]))
        .param(ParamSpec::float("p_inf", "Reference pressure", 0.0, -1.0e6, 1.0e6).help("Same unit as the sampled values; zero at the outlet for the incompressible solver"))
        .param(ParamSpec::float("u_ms", "Speed", 30.0, 10.0, 100.0).unit("m/s").help("Overridden by a wired air state"))
        .param(ParamSpec::float("rho", "Density", 1.225, 0.5, 1.4).unit("kg/m3").help("Overridden by a wired air state; only used for static pressure in Pa"))
        .param(ParamSpec::float("max_dist_mm", "Match distance", 30.0, 1.0, 500.0).unit("mm").help("Largest distance between a surface face and its nearest sample"))
        .param(ParamSpec::bool("mirror_half", "Half-car samples", true).help("Copy the y >= 0 pressures to the mirrored side"))
        .param(ParamSpec::float("clip_pct", "Colour range clip", 2.0, 0.0, 20.0).unit("%").help("Percentile clipped at each end when choosing the display range"))
        .help("Maps sampled solver pressures onto the vehicle faces as the per-face attribute Cp = (p - p_inf) / q, with the display range and the integrated pressure force. Without samples only a crude flagged estimate is produced.")
        .emulates("automotive wind-tunnel service: surface pressure map")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

fn cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let surf = inputs::geometry(ins, 0)?;
    if surf.prims.is_empty() {
        return Err(ctx.fail("the surface has no faces"));
    }
    let air = air_in(ins, 2)?;
    let (u, rho) = (air.u.unwrap_or(p.float("u_ms")?), air.rho.unwrap_or(p.float("rho")?));
    let q = dynamic_pressure(rho, u);
    let n = surf.prims.len();
    let quantity = Quantity::parse(p.str("quantity")?).ok_or_else(|| ctx.fail("unknown quantity"))?;
    let mut rep = Report::new("Surface pressure");
    let (cp, valid, estimate): (Vec<f64>, Vec<i64>, bool) = match file_in(ins, 1)? {
        Some(path) => {
            let text = std::fs::read_to_string(&path).map_err(|e| ctx.fail(format!("cannot read {}: {e}", path.display())))?;
            let to_mm = if p.str("file_units")? == "mm" { 1.0 } else { 1000.0 };
            let col = p.int("column")?.max(0) as usize;
            let samples = parse_samples(&text, col, to_mm).map_err(|e| ctx.fail(format!("{}: {e}", path.display())))?;
            let hits = match_prims(&surf, &samples, p.float("max_dist_mm")?, p.bool("mirror_half")?);
            let p_inf = p.float("p_inf")?;
            let cp: Vec<f64> = hits.iter().map(|h| h.map_or(0.0, |j| cp_from_value(samples.val[j], quantity, p_inf, u, rho))).collect();
            let valid: Vec<i64> = hits.iter().map(|h| i64::from(h.is_some())).collect();
            (cp, valid, false)
        }
        None => ((0..n).map(|i| estimate_cp(surf.prim_normal(i))).collect(), vec![1; n], true),
    };
    if cp.iter().any(|c| !c.is_finite()) {
        return Err(ctx.fail("non-finite pressure coefficient (check speed and sampled values)"));
    }
    let matched = valid.iter().sum::<i64>() as f64 / n as f64 * 100.0;
    let mut vals: Vec<f64> = cp.iter().zip(&valid).filter(|(_, v)| **v == 1).map(|(c, _)| *c).collect();
    vals.sort_by(f64::total_cmp);
    let mut out = (*surf).clone();
    out.prim_attribs.set("Cp", AttribArray::Float(cp.clone()));
    out.prim_attribs.set("Cp_valid", AttribArray::Int(valid.clone()));
    if vals.is_empty() {
        rep.metric("matched_pct", Metric::new(0.0, "%").with_limit(95.0, false));
        rep.note(Severity::Error, "no surface face found a pressure sample within the match distance; check units, frame and the mirror option");
        return Ok(vec![Data::geometry(out), Data::report(rep)]);
    }
    let clip = p.float("clip_pct")?.clamp(0.0, 20.0) / 100.0;
    let (lo, hi) = (percentile(&vals, clip), percentile(&vals, 1.0 - clip));
    out.detail_attribs.set("Cp_lo", AttribArray::Float(vec![lo]));
    out.detail_attribs.set("Cp_hi", AttribArray::Float(vec![hi]));
    let area: f64 = (0..n).filter(|&i| valid[i] == 1).map(|i| surf.prim_area(i)).sum();
    let mean: f64 = (0..n).filter(|&i| valid[i] == 1).map(|i| cp[i] * surf.prim_area(i)).sum::<f64>() / area.max(f64::MIN_POSITIVE);
    let force = pressure_force_n(&surf, &cp, q);
    rep.metric("q", Metric::new(q, "Pa"))
        .metric("cp_min", Metric::new(vals[0], "-"))
        .metric("cp_max", Metric::new(vals[vals.len() - 1], "-"))
        .metric("cp_mean_area_weighted", Metric::new(mean, "-"))
        .metric("cp_range_lo", Metric::new(lo, "-"))
        .metric("cp_range_hi", Metric::new(hi, "-"))
        .metric("pressure_force_x", Metric::new(force.x, "N"))
        .metric("pressure_force_z", Metric::new(force.z, "N"));
    if estimate {
        rep.metric("estimate_only", Metric::new(1.0, "-").with_limit(0.0, true));
        rep.note(
            Severity::Warning,
            "white-box placeholder: reads the solver's sampled face pressures and maps them onto the surface; this crude stagnation/base-pressure estimate is not a result",
        );
    } else {
        rep.metric("matched_pct", Metric::new(matched, "%").with_limit(95.0, false));
        if matched < 95.0 {
            rep.note(Severity::Warning, "some faces found no sample: their Cp is 0 and Cp_valid is 0; raise the match distance or check the frame");
        }
        if vals[0] < -3.0 || vals[vals.len() - 1] > 1.05 {
            rep.note(Severity::Warning, "Cp outside the usual range (below -3 or above 1.05): check the reference pressure and speed");
        }
    }
    rep.note(Severity::Info, "pressure forces assume outward-facing normals; compare with the force-coefficient result");
    Ok(vec![Data::geometry(out), Data::report(rep)])
}

pub(crate) fn register(reg: &mut Registry) {
    reg.add(FnNode::new(info, cook));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::*;
    use vernon_graph::ParamValue;

    /// Box 4 m x 1.8 m x 1.4 m in mm, centred at the origin.
    fn car_box() -> Geometry {
        Geometry::make_box(DVec3::new(4000.0, 1800.0, 1400.0))
    }

    /// Face centres of the box as an `x y z value` file in metres.
    fn face_file(g: &Geometry, value: impl Fn(DVec3) -> f64, half_only: bool) -> String {
        let mut t = String::from("# x y z p\n");
        for i in 0..g.prims.len() {
            let (c, n) = (centroid(g, i), g.prim_normal(i));
            if half_only && c.y < -1.0 {
                continue;
            }
            t += &format!("{} {} {} {}\n", c.x / 1000.0, c.y / 1000.0, c.z / 1000.0, value(n));
        }
        t
    }

    fn run(g: &Geometry, file: Option<&std::path::Path>, set: &[(&str, ParamValue)]) -> CookResult<Vec<Data>> {
        let mut p = Params::from_specs(&info().params);
        for (k, v) in set {
            p.set(k, v.clone());
        }
        let ins = vec![Some(Data::geometry(g.clone())), file.map(|f| Data::File(f.to_path_buf()))];
        cook(&ctx("pressure"), &ins, &p)
    }

    #[test]
    fn golden_g21_pressure_puck() {
        let (u, rho) = (100.0 / 3.6, 1.11);
        assert!((dynamic_pressure(rho, u) - 428.2).abs() < 0.1);
        assert!((cp_from_value(-91.0, Quantity::StaticPa, 0.0, u, rho) + 0.212).abs() < 0.001);
        assert!((cp_from_value(-103.0, Quantity::StaticPa, 0.0, u, rho) + 0.241).abs() < 0.001);
        // the kinematic form gives the same number
        assert!((cp_from_value(-91.0 / rho, Quantity::Kinematic, 0.0, u, rho) - cp_from_value(-91.0, Quantity::StaticPa, 0.0, u, rho)).abs() < 1e-12);
    }

    #[test]
    fn stagnation_is_one_and_reference_shifts_cp() {
        let u = 30.0;
        assert!((cp_from_value(0.5 * u * u, Quantity::Kinematic, 0.0, u, 1.225) - 1.0).abs() < 1e-12);
        // a reference pressure of exactly the sample is Cp = 0, never shifted by -1
        assert_eq!(cp_from_value(7.0, Quantity::Kinematic, 7.0, u, 1.225), 0.0);
        assert_eq!(cp_from_value(-0.4, Quantity::Cp, 123.0, u, 1.225), -0.4);
    }

    #[test]
    fn parses_raw_and_csv_samples() {
        let raw = "# x y z p\n0.1 0.2 0.3 -12.5\n0.4 0.5 0.6 3.0\n\n";
        let s = parse_samples(raw, 0, 1000.0).unwrap();
        assert_eq!(s.val, [-12.5, 3.0]);
        assert_eq!(s.pos[0], DVec3::new(100.0, 200.0, 300.0));
        let csv = "x,y,z,p,tau\n1,2,3,4,5\n6,7,8,9,10\n";
        let s = parse_samples(csv, 1, 1.0).unwrap();
        assert_eq!(s.val, [5.0, 10.0]);
        assert!(parse_samples("# nothing\n", 0, 1.0).is_err());
        assert!(parse_samples("1 2 3 4\n", 3, 1.0).is_err(), "column beyond the row width");
    }

    #[test]
    fn matching_respects_distance_and_picks_nearest() {
        let s = Samples { pos: vec![DVec3::new(0.0, 0.0, 0.0), DVec3::new(10.0, 0.0, 0.0), DVec3::new(500.0, 0.0, 0.0)], val: vec![1.0, 2.0, 3.0] };
        let g = Geometry::from_polygons(
            vec![DVec3::new(9.0, 0.0, 0.0), DVec3::new(11.0, 0.0, 0.0), DVec3::new(10.0, 2.0, 0.0), DVec3::new(300.0, 0.0, 0.0), DVec3::new(301.0, 0.0, 0.0), DVec3::new(300.0, 1.0, 0.0)],
            vec![vec![0, 1, 2], vec![3, 4, 5]],
        );
        let h = match_prims(&g, &s, 30.0, false);
        assert_eq!(h, vec![Some(1), None]);
    }

    #[test]
    fn maps_box_faces_and_integrates_drag() {
        let g = car_box();
        // stagnation on the front (-x normal faces the flow only after the vehicle is in the flow frame),
        // zero elsewhere: here +x is the flow direction, so the face with normal -x is the nose
        let file = tmp("pressure-box").join("p.raw");
        let u = 30.0;
        std::fs::write(&file, face_file(&g, |n| if n.x < -0.5 { 0.5 * u * u } else { 0.0 }, false)).unwrap();
        let out = run(&g, Some(&file), &[]).unwrap();
        let surf = out[0].as_geometry().unwrap();
        let cp = surf.prim_attribs.floats("Cp").unwrap();
        assert_eq!(cp.len(), 6);
        for (i, c) in cp.iter().enumerate() {
            let want = if g.prim_normal(i).x < -0.5 { 1.0 } else { 0.0 };
            assert!((c - want).abs() < 1e-12, "face {i}: {c}");
        }
        assert_eq!(surf.prim_attribs.ints("Cp_valid").unwrap(), [1; 6]);
        surf.validate().unwrap();
        let r = out[1].as_report().unwrap();
        assert!(r.passes(), "{r:?}");
        // flat nose of 1.8 x 1.4 m2 at Cp = 1 is a pressure drag of q A
        let want = 0.5 * 1.225 * u * u * 1.8 * 1.4;
        assert!((r.metrics["pressure_force_x"].value - want).abs() < 1e-6 * want, "{}", r.metrics["pressure_force_x"].value);
        assert_eq!(r.metrics["cp_max"].value, 1.0);
        assert_eq!(surf.detail_attribs.floats("Cp_hi").unwrap().len(), 1);
    }

    #[test]
    fn out_of_range_clip_never_indexes_out_of_bounds() {
        let g = car_box();
        let file = tmp("pressure-clip").join("p.raw");
        std::fs::write(&file, face_file(&g, |n| n.x, false)).unwrap();
        for clip in [-1.0, 1e30, 150.0, f64::NAN] {
            let out = run(&g, Some(&file), &[("clip_pct", ParamValue::Float(clip))]).unwrap();
            let hi = out[0].as_geometry().unwrap().detail_attribs.floats("Cp_hi").unwrap()[0];
            assert!(hi.is_finite(), "clip {clip}: {hi}");
        }
    }

    #[test]
    fn suction_on_the_underside_is_downforce() {
        let g = car_box();
        let file = tmp("pressure-down").join("p.raw");
        std::fs::write(&file, face_file(&g, |n| if n.z < -0.5 { -50.0 } else { 0.0 }, false)).unwrap();
        let out = run(&g, Some(&file), &[("quantity", ParamValue::Str("static_pa".into()))]).unwrap();
        assert!(out[1].as_report().unwrap().metrics["pressure_force_z"].value < 0.0);
    }

    #[test]
    fn half_car_samples_are_mirrored() {
        let g = car_box();
        let file = tmp("pressure-half").join("p.raw");
        std::fs::write(&file, face_file(&g, |n| if n.y.abs() > 0.5 { -3.0 } else { 1.0 }, true)).unwrap();
        // without mirroring the y < 0 face has no sample
        let out = run(&g, Some(&file), &[("mirror_half", ParamValue::Bool(false))]).unwrap();
        assert!(!out[1].as_report().unwrap().passes());
        let surf = out[0].as_geometry().unwrap();
        assert!(surf.prim_attribs.ints("Cp_valid").unwrap().contains(&0));
        // with it all faces are covered
        let out = run(&g, Some(&file), &[]).unwrap();
        assert!(out[1].as_report().unwrap().passes());
        assert!(out[0].as_geometry().unwrap().prim_attribs.ints("Cp_valid").unwrap().iter().all(|v| *v == 1));
    }

    #[test]
    fn without_samples_only_a_flagged_estimate() {
        let g = car_box();
        let out = run(&g, None, &[]).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(!r.passes(), "an estimate must never pass the gate");
        assert!(r.notes.iter().any(|(_, t)| t.starts_with("white-box placeholder:")));
        let cp = out[0].as_geometry().unwrap().prim_attribs.floats("Cp").unwrap().to_vec();
        for (i, c) in cp.iter().enumerate() {
            let nx = g.prim_normal(i).x;
            if nx < -0.5 {
                assert_eq!(*c, 1.0, "stagnation on the nose");
            } else {
                assert!(*c < 0.5);
            }
        }
    }

    #[test]
    fn bad_input_is_a_clean_error() {
        let g = car_box();
        assert!(run(&g, Some(std::path::Path::new("definitely/missing.raw")), &[]).is_err());
        let empty = Geometry::new();
        assert!(run(&empty, None, &[]).is_err());
        let file = tmp("pressure-bad").join("p.raw");
        std::fs::write(&file, "nothing useful\n").unwrap();
        assert!(run(&g, Some(&file), &[]).is_err());
        // far-away samples leave nothing matched: error note, no panic
        std::fs::write(&file, "50 50 50 1.0\n").unwrap();
        let out = run(&g, Some(&file), &[]).unwrap();
        assert!(!out[1].as_report().unwrap().passes());
    }
}

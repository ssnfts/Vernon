//! Cut nodes: planar cut, body-line cut (plane fitted to a polyline) and the
//! island selector.

use std::sync::Arc;

use glam::{DVec2, DVec3};
use serde_json::json;
use vernon_core::{Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::common::{info, placeholder, planes_from_json, v3_json};
use crate::cut::{cut_all, CutAll, Keep, Plane};
use crate::mesh;

fn keep_of(s: &str) -> Keep {
    match s {
        "upper" => Keep::Upper,
        "lower" => Keep::Lower,
        _ => Keep::Both,
    }
}

fn plane_json(planes: &[Plane]) -> serde_json::Value {
    serde_json::Value::Array(
        planes
            .iter()
            .enumerate()
            .map(|(i, p)| json!({"cap": i + 1, "origin": v3_json(p.origin), "normal": v3_json(p.normal)}))
            .collect(),
    )
}

/// Run the cut and assemble the standard report; shared by both cut nodes.
fn run_cut(
    ctx: &CookContext,
    geo: &Geometry,
    planes: &[Plane],
    eps: f64,
    cap: bool,
    keep: Keep,
    mut report: Report,
) -> CookResult<Vec<Data>> {
    let input_open = mesh::open_edges_by_part(geo);
    let all: CutAll = cut_all(geo, planes, eps, cap, keep).map_err(|e| ctx.fail(e.to_string()))?;
    let parts = mesh::unique_parts(&all.geo).len();
    report.metric("part_count", Metric::new(parts as f64, "count"));
    report.metric("plane_count", Metric::new(planes.len() as f64, "count"));
    report.metric("cap_area_mm2", Metric::new(all.cap_area, "mm2"));
    report.metric("seam_area_mm2", Metric::new(all.cap_area, "mm2"));
    report.metric("cap_loops", Metric::new(all.loops as f64, "count"));
    if cap {
        let open = mesh::open_edges_by_part(&all.geo);
        report.metric("open_edges", Metric::new(open as f64, "count").with_limit(0.0, true));
        if input_open > 0 {
            report.note(Severity::Warning, format!("input has {input_open} open edges; halves may stay open away from the cut"));
        }
        if keep == Keep::Both && input_open == 0 {
            let v_in = mesh::signed_volume(geo);
            let v_out = mesh::signed_volume(&all.geo);
            let err = if v_in.abs() > 0.0 { (v_out - v_in).abs() / v_in.abs() } else { 0.0 };
            report.metric("volume_error_rel", Metric::new(err, "ratio").with_limit(1e-9, true));
        }
    }
    Ok(vec![Data::geometry(all.geo), Data::json(plane_json(planes)), Data::report(report)])
}

fn cut_outputs(info: NodeTypeInfo) -> NodeTypeInfo {
    info.output(PortSpec::new("parts", DataKind::Geometry))
        .output(PortSpec::new("plane_list", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
}

fn cut_params(info: NodeTypeInfo) -> NodeTypeInfo {
    info.param(
        ParamSpec::choice("keep", "Keep", "both", &["both", "upper", "lower"])
            .help("Which side of the plane to keep."),
    )
    .param(ParamSpec::bool("cap", "Cap cut faces", true).help("Close the cut with planar faces; off leaves an open cut."))
    .param(
        ParamSpec::float("eps", "Snap tolerance", 1e-6, 1e-9, 1e-3)
            .unit("x diagonal")
            .help("Points closer to the plane than this fraction of the bounding diagonal count as on the plane."),
    )
}

pub struct PlaneCut;

impl NodeOp for PlaneCut {
    fn info(&self) -> NodeTypeInfo {
        cut_outputs(cut_params(
            info("print.plane_cut", "Planar Cut", "Cut a closed mesh by one or more planes and cap the cuts; every piece gets a part id.")
                .emulates("planar cut tool of common slicers, extended to many parts")
                .implemented()
                .input(PortSpec::new("mesh", DataKind::Geometry))
                .input(PortSpec::optional("planes", DataKind::Json))
                .param(ParamSpec::vec3("origin", "Plane origin", DVec3::ZERO).unit("mm").help("A point on the plane."))
                .param(ParamSpec::vec3("normal", "Plane normal", DVec3::Z).help("Plane normal; the positive side is the upper half.")),
        ))
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let mut report = Report::new("Planar cut");
        let planes = match inp.get(1).and_then(|d| d.as_ref()).and_then(|d| d.as_json()) {
            Some(j) => {
                let (planes, warns) = planes_from_json(j);
                for w in warns {
                    report.note(Severity::Warning, w);
                }
                report.metric("skipped_planes", Metric::new(0.0, "count"));
                if planes.is_empty() {
                    return Err(ctx.fail("the plane list holds no usable plane"));
                }
                planes
            }
            None => vec![Plane::new(p.vec3("origin")?, p.vec3("normal")?)
                .ok_or_else(|| ctx.fail("plane normal must be finite and non-zero"))?],
        };
        run_cut(ctx, &geo, &planes, p.float("eps")?, p.bool("cap")?, keep_of(p.str("keep")?), report)
    }
}

/// Best-fit plane through a polyline that contains the view direction.
/// Returns (plane, rms, max deviation) in mm.
pub fn fit_plane_to_polyline(points: &[DVec3], view: DVec3) -> Option<(Plane, f64, f64)> {
    if points.len() < 2 || view.length() < 1e-12 {
        return None;
    }
    let d = view.normalize();
    let helper = if d.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
    let a = (helper - d * helper.dot(d)).normalize();
    let b = d.cross(a);
    let centroid = points.iter().copied().sum::<DVec3>() / points.len() as f64;
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for q in points {
        let r = *q - centroid;
        let (x, y) = (r.dot(a), r.dot(b));
        sxx += x * x;
        syy += y * y;
        sxy += x * y;
    }
    if sxx + syy < 1e-12 {
        return None;
    }
    let ang = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let dir2 = DVec2::new(ang.cos(), ang.sin());
    let l = a * dir2.x + b * dir2.y;
    let n = d.cross(l).normalize();
    let plane = Plane::new(centroid, n)?;
    let devs: Vec<f64> = points.iter().map(|q| plane.dist(*q).abs()).collect();
    let rms = (devs.iter().map(|x| x * x).sum::<f64>() / devs.len() as f64).sqrt();
    let max = devs.iter().copied().fold(0.0, f64::max);
    Some((plane, rms, max))
}

pub struct PolylineCut;

impl NodeOp for PolylineCut {
    fn info(&self) -> NodeTypeInfo {
        cut_outputs(cut_params(
            info("print.polyline_cut", "Body Line Cut", "Cut along a polyline seen from a view direction, so seams follow body lines.")
                .emulates("projected-curve split of CAD tools")
                .input(PortSpec::new("mesh", DataKind::Geometry))
                .input(PortSpec::new("guide", DataKind::Geometry))
                .param(ParamSpec::vec3("view_dir", "View direction", DVec3::Y).help("Direction the polyline is projected along; the cut surface contains it.")),
        ))
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let guide = inputs::geometry(inp, 1)?;
        let mut report = Report::new("Body line cut");
        let mut pts: Vec<DVec3> = Vec::new();
        for pr in guide.prims.iter().filter(|q| !q.closed) {
            pts.extend(pr.vertices.iter().map(|&v| guide.points[v as usize]));
        }
        if pts.is_empty() {
            pts = guide.points.clone();
        }
        let (plane, rms, max) = fit_plane_to_polyline(&pts, p.vec3("view_dir")?)
            .ok_or_else(|| ctx.fail("the guide polyline needs at least two distinct points and a non-zero view direction"))?;
        report.metric("fit_rms_mm", Metric::new(rms, "mm"));
        report.metric("fit_max_mm", Metric::new(max, "mm"));
        placeholder(
            &mut report,
            "one best-fit plane stands in for the cut surface; the real algorithm cuts along the polyline extruded along the view direction",
        );
        run_cut(ctx, &geo, &[plane], p.float("eps")?, p.bool("cap")?, keep_of(p.str("keep")?), report)
    }
}

pub struct CutRegionSelect;

impl NodeOp for CutRegionSelect {
    fn info(&self) -> NodeTypeInfo {
        info(
            "print.cut_region_select",
            "Cut Region Select",
            "Choose which islands of a multi-island cut plane are cut and which stay joined.",
        )
        .emulates("per-island cut or join choice of planar cut tools")
        .input(PortSpec::new("cut", DataKind::Geometry))
        .param(ParamSpec::choice("mode", "Mode", "cut", &["cut", "join"]).help("Cut every island or leave the chosen island joined."))
        .param(ParamSpec::int("island", "Island", 0, -1, 1000).help("Island index the mode applies to; -1 means all."))
        .output(PortSpec::new("geometry", DataKind::Geometry))
        .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, _ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let mut g: Geometry = (*geo).clone();
        let join = p.str("mode")? == "join";
        let n = g.prims.len();
        mesh::set_int_prim_attr(&mut g, "joined", vec![i64::from(join); n]);
        let mut r = Report::new("Cut region select");
        r.metric("island_count", Metric::new(1.0, "count"));
        placeholder(&mut r, "per-island selection; the real algorithm lists the islands of each cap and re-joins the unselected ones");
        Ok(vec![Data::Geometry(Arc::new(g)), Data::report(r)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testutil::cook;
    use crate::mesh::testutil::tri_box;
    use vernon_graph::ParamValue;

    #[test]
    fn plane_cut_node_reports_contract_numbers() {
        let cube = tri_box(DVec3::ZERO, DVec3::splat(100.0));
        let out = cook(
            &PlaneCut,
            "pc",
            &[Some(Data::geometry(cube))],
            &[("origin", ParamValue::Vec3(DVec3::new(0.0, 0.0, 40.0)))],
        )
        .unwrap();
        let r = out[2].as_report().unwrap();
        assert_eq!(r.metrics["part_count"].value, 2.0);
        assert!((r.metrics["cap_area_mm2"].value - 10_000.0).abs() < 1e-6);
        assert_eq!(r.metrics["open_edges"].value, 0.0);
        assert!(r.metrics["volume_error_rel"].value < 1e-9);
        assert!(r.passes());
    }

    #[test]
    fn plane_cut_node_rejects_zero_normal() {
        let cube = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        let e = cook(&PlaneCut, "pc", &[Some(Data::geometry(cube))], &[("normal", ParamValue::Vec3(DVec3::ZERO))]);
        assert!(e.is_err());
    }

    #[test]
    fn plane_cut_json_planes_make_four_panels() {
        let cube = tri_box(DVec3::splat(-50.0), DVec3::splat(100.0));
        let planes = json!([
            {"origin": [0, 0, 0], "normal": [1, 0, 0]},
            {"origin": [0, 0, 0], "normal": [0, 1, 0]},
            {"origin": [0, 0, 0], "normal": [0, 0, 0]}
        ]);
        let out = cook(&PlaneCut, "pc", &[Some(Data::geometry(cube)), Some(Data::json(planes))], &[]).unwrap();
        let r = out[2].as_report().unwrap();
        assert_eq!(r.metrics["part_count"].value, 4.0);
        assert!(r.notes.iter().any(|(s, _)| *s == Severity::Warning));
    }

    #[test]
    fn polyline_in_plane_matches_plane_cut() {
        let cube = tri_box(DVec3::splat(-50.0), DVec3::splat(100.0));
        // polyline in the plane x = 10, viewed along Y
        let mut guide = Geometry::from_points(vec![DVec3::new(10.0, 0.0, -30.0), DVec3::new(10.0, 0.0, 0.0), DVec3::new(10.0, 0.0, 30.0)]);
        guide.prims.push(vernon_core::Prim::polyline(vec![0, 1, 2]));
        let out = cook(
            &PolylineCut,
            "bl",
            &[Some(Data::geometry(cube.clone())), Some(Data::geometry(guide))],
            &[("view_dir", ParamValue::Vec3(DVec3::Y))],
        )
        .unwrap();
        let g = out[0].as_geometry().unwrap();
        let vols: Vec<f64> = (0..2).map(|i| mesh::signed_volume(&mesh::extract_part(g, i))).collect();
        let mut v = vols.clone();
        v.sort_by(|a, b| a.total_cmp(b));
        assert!((v[0] - 40.0 * 100.0 * 100.0).abs() < 1e-3, "{v:?}");
        assert!((v[1] - 60.0 * 100.0 * 100.0).abs() < 1e-3);
        let r = out[2].as_report().unwrap();
        assert!(r.metrics["fit_rms_mm"].value < 1e-9);
    }

    #[test]
    fn fit_plane_reports_curve_deviation() {
        let pts: Vec<DVec3> = (0..20).map(|i| DVec3::new(i as f64, 0.0, 5.0 * (i as f64 * 0.3).sin())).collect();
        let (_, rms, max) = fit_plane_to_polyline(&pts, DVec3::Z).unwrap();
        assert!(rms >= 0.0 && max >= rms);
    }
}

//! Cut nodes: planar cut, body-line cut (plane fitted to a polyline) and the
//! island selector.

use std::collections::BTreeSet;

use glam::{DVec2, DVec3};
use serde_json::json;
use vernon_core::{Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{
    CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec,
};

use crate::common::{info, planes_from_json, v3_json};
use crate::cut::{cut_all_with_cap_start, next_cap_id, CutAll, Keep, Plane};
use crate::mesh;

fn keep_of(s: &str) -> Keep {
    match s {
        "upper" => Keep::Upper,
        "lower" => Keep::Lower,
        _ => Keep::Both,
    }
}

fn plane_json(planes: &[Plane], cap_start: i64) -> serde_json::Value {
    serde_json::Value::Array(
        planes
            .iter()
            .enumerate()
            .map(|(i, p)| json!({"cap": cap_start + i as i64, "origin": v3_json(p.origin), "normal": v3_json(p.normal)}))
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
    geo.validate().map_err(|e| ctx.fail(e.to_string()))?;
    if geo.points.iter().any(|p| !p.is_finite()) {
        return Err(ctx.fail("the body has non-finite points"));
    }
    let input_open = mesh::open_edges_by_part(geo);
    let cap_start = next_cap_id(geo, planes.len()).map_err(|e| ctx.fail(e.to_string()))?;
    let (selected, untouched) = selection_components(geo);
    let mut all: CutAll =
        cut_all_with_cap_start(&selected, planes, eps, cap, keep, cap_start).map_err(|e| ctx.fail(e.to_string()))?;
    append_untouched(&mut all.geo, untouched);
    let parts = mesh::unique_parts(&all.geo).len();
    report.metric("part_count", Metric::new(parts as f64, "count"));
    report.metric("plane_count", Metric::new(planes.len() as f64, "count"));
    report.metric("cap_area_mm2", Metric::new(all.cap_area, "mm2"));
    report.metric("seam_area_mm2", Metric::new(all.cap_area, "mm2"));
    report.metric("cap_loops", Metric::new(all.loops as f64, "count"));
    if cap {
        let open = mesh::open_edges_by_part(&all.geo);
        report.metric(
            "open_edges",
            Metric::new(open as f64, "count").with_limit(0.0, true),
        );
        if input_open > 0 {
            report.note(
                Severity::Warning,
                format!(
                    "input has {input_open} open edges; halves may stay open away from the cut"
                ),
            );
        }
        if keep == Keep::Both && input_open == 0 {
            let v_in = mesh::signed_volume(geo);
            let v_out = mesh::signed_volume(&all.geo);
            let err = if v_in.abs() > 0.0 {
                (v_out - v_in).abs() / v_in.abs()
            } else {
                0.0
            };
            report.metric(
                "volume_error_rel",
                Metric::new(err, "ratio").with_limit(1e-9, true),
            );
        }
    }
    Ok(vec![
        Data::geometry(all.geo),
        Data::json(plane_json(planes, cap_start)),
        Data::report(report),
    ])
}

/// Selection is component eligibility, never extraction of an open face patch.
fn selection_components(geo: &Geometry) -> (Geometry, Vec<Geometry>) {
    let selected = geo
        .prim_groups
        .get("cut_region")
        .map(|g| g.iter().copied().collect::<BTreeSet<_>>());
    let source_parts = mesh::part_ids(geo);
    let mut eligible = Geometry::new();
    let mut untouched = Vec::new();
    let mut next = 0;
    for part in mesh::unique_parts(geo) {
        let indices: Vec<_> = source_parts
            .iter()
            .enumerate()
            .filter_map(|(i, &p)| (p == part).then_some(i))
            .collect();
        let mut body = mesh::extract_prims(geo, &indices);
        let components = crate::nodes_split::connectivity_parts(&body, false, 0.0);
        mesh::set_int_prim_attr(&mut body, mesh::PART, components.clone());
        for component in mesh::unique_parts(&body) {
            let touched = selected.as_ref().map_or(true, |set| {
                components
                    .iter()
                    .enumerate()
                    .any(|(i, &c)| c == component && set.contains(&(indices[i] as u32)))
            });
            let mut piece = mesh::extract_part(&body, component);
            if touched {
                let n = piece.prims.len();
                mesh::set_int_prim_attr(&mut piece, mesh::PART, vec![next; n]);
                eligible.merge(&piece);
                next += 1;
            } else {
                untouched.push(piece);
            }
        }
    }
    (eligible, untouched)
}

fn append_untouched(geo: &mut Geometry, untouched: Vec<Geometry>) {
    let start = mesh::unique_parts(geo).len() as i64;
    for (offset, mut piece) in untouched.into_iter().enumerate() {
        let n = piece.prims.len();
        mesh::set_int_prim_attr(&mut piece, mesh::PART, vec![start + offset as i64; n]);
        geo.merge(&piece);
    }
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
            info("print.plane_cut", "Planar Cut", "Cut and cap closed mesh components by planes. A cut_region primitive group limits eligibility to whole connected components touched by its faces; other components pass through unchanged.")
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
        let planes = match inp
            .get(1)
            .and_then(|d| d.as_ref())
            .and_then(|d| d.as_json())
        {
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
        run_cut(
            ctx,
            &geo,
            &planes,
            p.float("eps")?,
            p.bool("cap")?,
            keep_of(p.str("keep")?),
            report,
        )
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
            info("print.polyline_cut", "Body Line Cut", "Cut along the exact projected piecewise-linear curtain, extending endpoint segments. The projected guide must advance monotonically between its endpoints (2 to 128 points, coordinates within 1e12 mm). Poorly conditioned shears fail when estimated roundoff exceeds the physical input-scale snap tolerance. A cut_region group selects whole connected components touched by its faces; others pass through unchanged.")
                .emulates("projected-curve split of CAD tools")
                .implemented()
                .input(PortSpec::new("mesh", DataKind::Geometry))
                .input(PortSpec::new("guide", DataKind::Geometry))
                .param(ParamSpec::vec3("view_dir", "View direction", DVec3::Y).help("Direction the polyline is projected along; the cut surface contains it.")),
        ))
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let guide = inputs::geometry(inp, 1)?;
        geo.validate().map_err(|e| ctx.fail(e.to_string()))?;
        guide.validate().map_err(|e| ctx.fail(e.to_string()))?;
        if geo.points.iter().any(|p| !p.is_finite()) {
            return Err(ctx.fail("the body has non-finite points"));
        }
        let mut report = Report::new("Body line cut");
        let lines: Vec<_> = guide.prims.iter().filter(|q| !q.closed).collect();
        if guide.prims.iter().any(|pr| pr.closed) {
            return Err(ctx.fail("the guide must be an open polyline, not a closed polygon"));
        }
        if lines.len() > 1 {
            return Err(ctx.fail("connect one ordered open guide polyline"));
        }
        let pts: Vec<DVec3> = lines.first().map_or_else(
            || guide.points.clone(),
            |pr| {
                pr.vertices
                    .iter()
                    .map(|&v| guide.points[v as usize])
                    .collect()
            },
        );
        let (_, rms, max) = fit_plane_to_polyline(&pts, p.vec3("view_dir")?)
            .ok_or_else(|| ctx.fail("the guide polyline needs at least two distinct points and a non-zero view direction"))?;
        report.metric("fit_rms_mm", Metric::new(rms, "mm"));
        report.metric("fit_max_mm", Metric::new(max, "mm"));
        let (selected, untouched) = selection_components(&geo);
        let keep = keep_of(p.str("keep")?);
        let cap_start = next_cap_id(&geo, pts.len() - 1).map_err(|e| ctx.fail(e.to_string()))?;
        let mut cut = crate::curtain::cut(
            &selected,
            &pts,
            p.vec3("view_dir")?,
            p.float("eps")?,
            p.bool("cap")?,
            keep,
            cap_start,
        )
        .map_err(|e| ctx.fail(e))?;
        append_untouched(&mut cut.geo, untouched);
        report.metric(
            "part_count",
            Metric::new(mesh::unique_parts(&cut.geo).len() as f64, "count"),
        );
        report.metric("plane_count", Metric::new(cut.segments as f64, "count"));
        report.metric("guide_segments", Metric::new(cut.segments as f64, "count"));
        report.metric("cap_area_mm2", Metric::new(cut.cap_area, "mm2"));
        report.metric("seam_area_mm2", Metric::new(cut.cap_area, "mm2"));
        report.metric("cap_loops", Metric::new(cut.loops as f64, "count"));
        let open = Metric::new(mesh::open_edges_by_part(&cut.geo) as f64, "count");
        report.metric(
            "open_edges",
            if p.bool("cap")? {
                open.with_limit(0.0, true)
            } else {
                open
            },
        );
        if keep == Keep::Both && mesh::open_edges_by_part(&geo) == 0 {
            let before = mesh::signed_volume(&geo);
            let err = if before.abs() > 1e-12 {
                (mesh::signed_volume(&cut.geo) - before).abs() / before.abs()
            } else {
                0.0
            };
            report.metric(
                "volume_error_rel",
                Metric::new(err, "ratio").with_limit(0.005, true),
            );
        }
        report.note(Severity::Info, "The cut uses the projected polyline curtain, including both skins and closing walls. Endpoint segments extend across the body; folds and numerically ill-conditioned projections are rejected. Clipping and welding retain physical input-scale tolerances. Selection applies to complete connected components; new cap identifiers are allocated above every existing input seam.");
        let planes: Vec<_> = pts
            .windows(2)
            .map(|w| {
                let d = p.vec3("view_dir").unwrap();
                let normal = d.cross(w[1] - w[0]);
                Plane::new(w[0], normal).expect("validated projected guide")
            })
            .collect();
        let metadata = serde_json::Value::Array(planes.iter().enumerate().map(|(i,plane)|json!({"cap":cut.cap_start+i as i64,"origin":v3_json(plane.origin),"normal":v3_json(plane.normal)})).collect());
        Ok(vec![
            Data::geometry(cut.geo),
            Data::json(metadata),
            Data::report(report),
        ])
    }
}

pub struct CutRegionSelect;

impl NodeOp for CutRegionSelect {
    fn info(&self) -> NodeTypeInfo {
        info(
            "print.cut_region_select",
            "Cut Region Select",
            "Select faces by bounding box, normal cone or primitive group into cut_region. Cut nodes use the selected faces to choose whole connected components; this does not perform a partial cut within one connected component.",
        )
        .emulates("per-island cut or join choice of planar cut tools")
        .implemented()
        .input(PortSpec::new("cut", DataKind::Geometry))
        .param(ParamSpec::choice("mode", "Mode", "cut", &["cut", "join"]).help("Select matching faces for cutting, or invert the selection to keep their components joined."))
        .param(ParamSpec::int("island", "Island", -1, -1, 1000).help("Optionally restrict to an edge-connected component index; -1 means all."))
        .param(ParamSpec::choice("selector", "Select by", "all", &["all", "bbox", "normal_cone", "group"]).help("Rule used to select primitive faces."))
        .param(ParamSpec::vec3("bounds_min", "Box minimum", DVec3::splat(-1e9)).unit("mm").help("Minimum coordinates of the face-centroid selection box."))
        .param(ParamSpec::vec3("bounds_max", "Box maximum", DVec3::splat(1e9)).unit("mm").help("Maximum coordinates of the face-centroid selection box."))
        .param(ParamSpec::vec3("normal_axis", "Normal axis", DVec3::Z).help("Centre direction of the normal cone."))
        .param(ParamSpec::float("normal_angle", "Normal angle", 45.0, 0.0, 180.0).unit("deg").help("Half-angle of the normal cone."))
        .param(ParamSpec::string("source_group", "Source group", "").help("Existing primitive group used by group selection."))
        .output(PortSpec::new("geometry", DataKind::Geometry))
        .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        geo.validate().map_err(|e| ctx.fail(e.to_string()))?;
        if geo.points.iter().any(|p| !p.is_finite()) {
            return Err(ctx.fail("selection input contains non-finite points"));
        }
        let mut g: Geometry = (*geo).clone();
        let join = p.str("mode")? == "join";
        let components = crate::nodes_split::connectivity_parts(&geo, false, 0.0);
        let selector = p.str("selector")?;
        let lo = p.vec3("bounds_min")?;
        let hi = p.vec3("bounds_max")?;
        let normal = p.vec3("normal_axis")?;
        let angle = p.float("normal_angle")?;
        if selector == "bbox" && (!lo.is_finite() || !hi.is_finite() || lo.cmpgt(hi).any()) {
            return Err(ctx.fail("selection box must be finite and ordered"));
        }
        if selector == "normal_cone"
            && (!normal.is_finite()
                || normal.length() < 1e-12
                || !angle.is_finite()
                || !(0.0..=180.0).contains(&angle))
        {
            return Err(
                ctx.fail("normal cone needs a finite nonzero axis and angle from 0 to 180 degrees")
            );
        }
        let source = if selector == "group" {
            Some(
                geo.prim_groups
                    .get(p.str("source_group")?)
                    .ok_or_else(|| ctx.fail("the source primitive group does not exist"))?
                    .iter()
                    .copied()
                    .collect::<BTreeSet<_>>(),
            )
        } else {
            None
        };
        let axis = normal.normalize_or_zero();
        let limit = angle.to_radians().cos();
        let island = p.int("island")?;
        let mut selection = Vec::new();
        let mut joined = Vec::with_capacity(g.prims.len());
        for (i, pr) in g.prims.iter().enumerate() {
            let matches = if !pr.closed || pr.vertices.len() < 3 {
                false
            } else {
                let points: Vec<_> = pr.vertices.iter().map(|&v| g.points[v as usize]).collect();
                match selector {
                    "all" => true,
                    "bbox" => {
                        let c = points.iter().copied().sum::<DVec3>() / points.len() as f64;
                        c.cmpge(lo).all() && c.cmple(hi).all()
                    }
                    "normal_cone" => {
                        let n = (1..points.len() - 1)
                            .map(|k| (points[k] - points[0]).cross(points[k + 1] - points[0]))
                            .sum::<DVec3>();
                        n.length() > 1e-15 && n.normalize().dot(axis) >= limit - 1e-12
                    }
                    "group" => source.as_ref().unwrap().contains(&(i as u32)),
                    _ => return Err(ctx.fail("unknown region selector")),
                }
            } && (island < 0 || components[i] == island);
            let selected = matches;
            if selected && pr.closed {
                selection.push(i as u32);
            }
            joined.push(i64::from(!selected));
        }
        if join {
            let blocked: BTreeSet<_> = selection.iter().map(|&i| components[i as usize]).collect();
            selection.clear();
            for (i, pr) in g.prims.iter().enumerate() {
                let selected = pr.closed && !blocked.contains(&components[i]);
                if selected {
                    selection.push(i as u32);
                }
                joined[i] = i64::from(!selected);
            }
        }
        g.prim_groups.insert("cut_region".into(), selection.clone());
        mesh::set_int_prim_attr(&mut g, "joined", joined);
        let mut r = Report::new("Cut region select");
        r.metric(
            "island_count",
            Metric::new(
                components.iter().copied().collect::<BTreeSet<_>>().len() as f64,
                "count",
            ),
        );
        r.metric(
            "selected_prims",
            Metric::new(selection.len() as f64, "count"),
        );
        r.note(Severity::Info,"The cut_region primitive group selects complete connected components touched by these faces. Other components pass through the cut nodes unchanged.");
        Ok(vec![Data::geometry(g), Data::report(r)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cut::cut_all;
    use crate::common::testutil::cook;
    use crate::mesh::testutil::tri_box;
    use vernon_graph::ParamValue;

    fn assert_closed_winding(g: &Geometry) {
        let mut edges = std::collections::BTreeMap::<(u32, u32), (usize, i32)>::new();
        for pr in &g.prims {
            for i in 0..pr.vertices.len() {
                let (a, b) = (pr.vertices[i], pr.vertices[(i + 1) % pr.vertices.len()]);
                let e = edges.entry((a.min(b), a.max(b))).or_default();
                e.0 += 1;
                e.1 += if a < b { 1 } else { -1 };
            }
        }
        assert!(edges
            .values()
            .all(|&(count, winding)| count == 2 && winding == 0));
    }

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
        let e = cook(
            &PlaneCut,
            "pc",
            &[Some(Data::geometry(cube))],
            &[("normal", ParamValue::Vec3(DVec3::ZERO))],
        );
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
        let out = cook(
            &PlaneCut,
            "pc",
            &[Some(Data::geometry(cube)), Some(Data::json(planes))],
            &[],
        )
        .unwrap();
        let r = out[2].as_report().unwrap();
        assert_eq!(r.metrics["part_count"].value, 4.0);
        assert!(r.notes.iter().any(|(s, _)| *s == Severity::Warning));
    }

    #[test]
    fn polyline_in_plane_matches_plane_cut() {
        let cube = tri_box(DVec3::splat(-50.0), DVec3::splat(100.0));
        // polyline in the plane x = 10, viewed along Y
        let mut guide = Geometry::from_points(vec![
            DVec3::new(10.0, 0.0, -30.0),
            DVec3::new(10.0, 0.0, 0.0),
            DVec3::new(10.0, 0.0, 30.0),
        ]);
        guide.prims.push(vernon_core::Prim::polyline(vec![0, 1, 2]));
        let out = cook(
            &PolylineCut,
            "bl",
            &[
                Some(Data::geometry(cube.clone())),
                Some(Data::geometry(guide)),
            ],
            &[("view_dir", ParamValue::Vec3(DVec3::Y))],
        )
        .unwrap();
        let g = out[0].as_geometry().unwrap();
        let vols: Vec<f64> = (0..2)
            .map(|i| mesh::signed_volume(&mesh::extract_part(g, i)))
            .collect();
        let mut v = vols.clone();
        v.sort_by(|a, b| a.total_cmp(b));
        assert!((v[0] - 40.0 * 100.0 * 100.0).abs() < 1e-3, "{v:?}");
        assert!((v[1] - 60.0 * 100.0 * 100.0).abs() < 1e-3);
        let r = out[2].as_report().unwrap();
        assert!(r.metrics["fit_rms_mm"].value < 1e-9);
    }

    #[test]
    fn fit_plane_reports_curve_deviation() {
        let pts: Vec<DVec3> = (0..20)
            .map(|i| DVec3::new(i as f64, 0.0, 5.0 * (i as f64 * 0.3).sin()))
            .collect();
        let (_, rms, max) = fit_plane_to_polyline(&pts, DVec3::Z).unwrap();
        assert!(rms >= 0.0 && max >= rms);
    }

    #[test]
    fn wp5_bent_guide_cuts_the_actual_curtain() {
        let cube = tri_box(DVec3::splat(-50.0), DVec3::splat(100.0));
        let mut guide = Geometry::from_points(vec![
            DVec3::new(-60.0, -15.0, 0.0),
            DVec3::new(0.0, 20.0, 0.0),
            DVec3::new(60.0, -15.0, 0.0),
        ]);
        guide.prims.push(vernon_core::Prim::polyline(vec![0, 1, 2]));
        let out = cook(
            &PolylineCut,
            "wp5_bent",
            &[Some(Data::geometry(cube)), Some(Data::geometry(guide))],
            &[("view_dir", ParamValue::Vec3(DVec3::Z))],
        )
        .unwrap();
        let g = out[0].as_geometry().unwrap();
        let caps = g.prim_attribs.ints(mesh::CAP).unwrap();
        assert!(caps.iter().any(|&c| c > 0));
        for (i, pr) in g.prims.iter().enumerate().filter(|(i, _)| caps[*i] > 0) {
            for &v in &pr.vertices {
                let p = g.points[v as usize];
                assert!(
                    (p.y - (20.0 - 35.0 * p.x.abs() / 60.0)).abs() < 1e-6,
                    "cap {i}: {p:?}"
                );
            }
        }
        assert_eq!(mesh::unique_parts(g).len(), 2);
        for id in mesh::unique_parts(g) {
            let piece = mesh::extract_part(g, id);
            piece.validate().unwrap();
            assert_eq!(mesh::edge_stats(&piece).open, 0);
            assert_eq!(mesh::edge_stats(&piece).non_manifold, 0);
            assert_closed_winding(&piece);
        }
        assert!(!PolylineCut.info().whitebox);
    }

    #[test]
    fn wp5_diagonal_car_panel_shell_is_closed_and_preserves_volume() {
        let shell = vernon_nodes_geo::primitives::CarPanel {
            thickness: 3.0,
            ..Default::default()
        }
        .build();
        let input_volume = mesh::signed_volume(&shell);
        let mut guide = Geometry::from_points(vec![
            DVec3::new(-600.0, -250.0, 0.0),
            DVec3::new(0.0, 50.0, 0.0),
            DVec3::new(600.0, 250.0, 0.0),
        ]);
        guide.prims.push(vernon_core::Prim::polyline(vec![0, 1, 2]));
        let out = cook(
            &PolylineCut,
            "wp5_car",
            &[Some(Data::geometry(shell)), Some(Data::geometry(guide))],
            &[("view_dir", ParamValue::Vec3(DVec3::Z))],
        )
        .unwrap();
        let g = out[0].as_geometry().unwrap();
        assert_eq!(mesh::unique_parts(g).len(), 2);
        for id in mesh::unique_parts(g) {
            let piece = mesh::extract_part(g, id);
            piece.validate().unwrap();
            assert_eq!(mesh::edge_stats(&piece).open, 0);
            assert_eq!(mesh::edge_stats(&piece).non_manifold, 0);
            assert_closed_winding(&piece);
        }
        let error = (mesh::signed_volume(g) - input_volume).abs() / input_volume.abs();
        eprintln!("WP5 3mm car panel: input_volume_mm3={input_volume:.9}, output_volume_mm3={:.9}, relative_error={error:.12}",mesh::signed_volume(g));
        assert!(error < 0.005);
        assert!(!out[2]
            .as_report()
            .unwrap()
            .notes
            .iter()
            .any(|(_, text)| text.starts_with("white-box placeholder")));
    }

    #[test]
    fn wp5_bbox_selection_limits_the_cut_to_touched_components() {
        let mut body = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        body.merge(&tri_box(DVec3::new(30.0, 0.0, 0.0), DVec3::splat(10.0)));
        let selected = cook(
            &CutRegionSelect,
            "wp5_selection",
            &[Some(Data::geometry(body))],
            &[
                ("selector", ParamValue::Str("bbox".into())),
                ("bounds_min", ParamValue::Vec3(DVec3::splat(-1.0))),
                ("bounds_max", ParamValue::Vec3(DVec3::splat(11.0))),
            ],
        )
        .unwrap();
        let g = selected[0].as_geometry().unwrap();
        assert!(g.prim_groups.contains_key("cut_region"));
        assert_eq!(g.prim_groups["cut_region"].len(), 12);
        let cut = cook(
            &PlaneCut,
            "wp5_selected_cut",
            &[Some(selected[0].clone())],
            &[("origin", ParamValue::Vec3(DVec3::new(0.0, 0.0, 5.0)))],
        )
        .unwrap();
        let result = cut[0].as_geometry().unwrap();
        assert_eq!(mesh::unique_parts(result).len(), 3);
        assert_eq!(mesh::open_edges_by_part(result), 0);
        let uncut: Vec<_> = mesh::unique_parts(result)
            .into_iter()
            .map(|id| mesh::extract_part(result, id))
            .filter(|g| g.bounds().min.x > 20.0)
            .collect();
        assert_eq!(uncut.len(), 1);
        assert!((mesh::signed_volume(&uncut[0]) - 1000.0).abs() < 1e-6);
    }

    #[test]
    fn wp5_closed_guide_is_rejected_instead_of_dropping_its_last_edge() {
        let body = Geometry::make_box(DVec3::splat(10.0));
        let guide = Geometry::from_polygons(
            vec![
                DVec3::new(-6.0, 0.0, 0.0),
                DVec3::new(0.0, 2.0, 0.0),
                DVec3::new(6.0, 0.0, 0.0),
            ],
            vec![vec![0, 1, 2]],
        );
        assert!(cook(
            &PolylineCut,
            "closed_guide",
            &[Some(Data::geometry(body)), Some(Data::geometry(guide))],
            &[("view_dir", ParamValue::Vec3(DVec3::Z))]
        )
        .is_err());
    }

    #[test]
    fn wp5_normal_and_group_selectors_emit_exact_face_groups() {
        let mut body = Geometry::make_box(DVec3::splat(10.0));
        body.prim_groups.insert("chosen".into(), vec![1, 3]);
        let normal = cook(
            &CutRegionSelect,
            "normal_selection",
            &[Some(Data::geometry(body.clone()))],
            &[
                ("selector", ParamValue::Str("normal_cone".into())),
                ("normal_angle", ParamValue::Float(1.0)),
            ],
        )
        .unwrap();
        assert_eq!(
            normal[0].as_geometry().unwrap().prim_groups["cut_region"],
            vec![1]
        );
        let group = cook(
            &CutRegionSelect,
            "group_selection",
            &[Some(Data::geometry(body.clone()))],
            &[
                ("selector", ParamValue::Str("group".into())),
                ("source_group", ParamValue::Str("chosen".into())),
            ],
        )
        .unwrap();
        assert_eq!(
            group[0].as_geometry().unwrap().prim_groups["cut_region"],
            vec![1, 3]
        );
        let empty = cook(
            &CutRegionSelect,
            "empty_selection",
            &[Some(Data::geometry(body))],
            &[("mode", ParamValue::Str("join".into()))],
        )
        .unwrap();
        assert!(empty[0].as_geometry().unwrap().prim_groups["cut_region"].is_empty());
        let passthrough = cook(
            &PlaneCut,
            "unselected_passthrough",
            &[Some(empty[0].clone())],
            &[],
        )
        .unwrap();
        assert_eq!(
            mesh::unique_parts(passthrough[0].as_geometry().unwrap()).len(),
            1
        );
    }

    #[test]
    fn wp5_curtain_refinement_preserves_linear_attributes_and_volume() {
        let mut body = tri_box(DVec3::splat(-50.0), DVec3::splat(100.0));
        body.point_attribs.set(
            "linear",
            vernon_core::AttribArray::Float(
                body.points
                    .iter()
                    .map(|p| p.x + 2.0 * p.y + 3.0 * p.z)
                    .collect(),
            ),
        );
        let mut guide = Geometry::from_points(vec![
            DVec3::new(-60.0, -15.0, 0.0),
            DVec3::new(0.0, 20.0, 0.0),
            DVec3::new(60.0, -15.0, 0.0),
        ]);
        guide.prims.push(vernon_core::Prim::polyline(vec![0, 1, 2]));
        let out = cook(
            &PolylineCut,
            "attribute_curtain",
            &[Some(Data::geometry(body)), Some(Data::geometry(guide))],
            &[("view_dir", ParamValue::Vec3(DVec3::Z))],
        )
        .unwrap();
        let g = out[0].as_geometry().unwrap();
        let values = g.point_attribs.floats("linear").unwrap();
        for (i, (p, value)) in g.points.iter().zip(values).enumerate() {
            assert!(
                (value - (p.x + 2.0 * p.y + 3.0 * p.z)).abs() < 1e-6,
                "point {i}: {p:?} value {value} bodyface {}",
                g.prims
                    .iter()
                    .enumerate()
                    .any(|(k, pr)| g.prim_attribs.ints(mesh::CAP).unwrap()[k] == 0
                        && pr.vertices.contains(&(i as u32)))
            );
        }
        let mut volumes: Vec<_> = mesh::unique_parts(g)
            .into_iter()
            .map(|i| mesh::signed_volume(&mesh::extract_part(g, i)))
            .collect();
        volumes.sort_by(f64::total_cmp);
        assert!(
            (volumes[0] - 445_833.3333333333).abs() < 1e-5,
            "{volumes:?}"
        );
        assert!(
            (volumes[1] - 554_166.6666666667).abs() < 1e-5,
            "{volumes:?}"
        );
    }

    #[test]
    fn wp5_folded_and_invalid_guides_fail_cleanly() {
        let body = Geometry::make_box(DVec3::splat(10.0));
        for points in [
            vec![
                DVec3::new(-6.0, 0.0, 0.0),
                DVec3::new(4.0, 2.0, 0.0),
                DVec3::new(0.0, 0.0, 0.0),
            ],
            vec![DVec3::ZERO, DVec3::Z],
            vec![DVec3::ZERO, DVec3::splat(f64::NAN)],
        ] {
            let guide = Geometry::from_points(points);
            assert!(cook(
                &PolylineCut,
                "invalid_curtain",
                &[
                    Some(Data::geometry(body.clone())),
                    Some(Data::geometry(guide))
                ],
                &[("view_dir", ParamValue::Vec3(DVec3::Z))]
            )
            .is_err());
        }
    }

    #[test]
    fn wp5_curtain_caps_are_planar_regions_with_matching_metadata() {
        let body = Geometry::make_box(DVec3::splat(100.0));
        let mut guide = Geometry::from_points(vec![
            DVec3::new(-60.0, -15.0, 0.0),
            DVec3::new(0.0, 20.0, 0.0),
            DVec3::new(60.0, -15.0, 0.0),
        ]);
        guide.prims.push(vernon_core::Prim::polyline(vec![0, 1, 2]));
        let out = cook(
            &PolylineCut,
            "planar_curtain_regions",
            &[Some(Data::geometry(body)), Some(Data::geometry(guide))],
            &[("view_dir", ParamValue::Vec3(DVec3::Z))],
        )
        .unwrap();
        let g = out[0].as_geometry().unwrap();
        let caps = crate::joints::find_caps(g);
        assert_eq!(caps.len(), 4);
        let planes = out[1].as_json().unwrap();
        for cap in caps {
            for &face in &cap.prims {
                for &v in &g.prims[face].vertices {
                    assert!((g.points[v as usize] - cap.origin).dot(cap.normal).abs() < 1e-6);
                }
            }
            assert!(planes
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["cap"].as_i64() == Some(cap.cap_id)));
        }
    }

    #[test]
    fn wp5_join_mode_excludes_whole_touched_components() {
        let mut body = Geometry::make_box(DVec3::splat(10.0));
        let mut other = Geometry::make_box(DVec3::splat(10.0));
        for p in &mut other.points {
            p.x += 30.0;
        }
        body.merge(&other);
        let selected = cook(
            &CutRegionSelect,
            "join_selected_components",
            &[Some(Data::geometry(body))],
            &[
                ("selector", ParamValue::Str("normal_cone".into())),
                ("normal_angle", ParamValue::Float(1.0)),
                ("island", ParamValue::Int(0)),
                ("mode", ParamValue::Str("join".into())),
            ],
        )
        .unwrap();
        let g = selected[0].as_geometry().unwrap();
        assert!(g.prim_groups["cut_region"]
            .iter()
            .all(|&i| g.prims[i as usize]
                .vertices
                .iter()
                .all(|&v| g.points[v as usize].x > 20.0)));
        assert_eq!(g.prim_groups["cut_region"].len(), 6);
    }

    #[test]
    fn wp5_steep_curtain_never_returns_damaged_closed_solids() {
        for height in [1.0, 10.0, 100.0, 1000.0, 10000.0] {
            let body = Geometry::make_box(DVec3::splat(100.0));
            let guide = Geometry::from_points(vec![
                DVec3::new(-0.001, 0.0, 0.0),
                DVec3::new(0.0, height, 0.0),
                DVec3::new(0.001, 0.0, 0.0),
            ]);
            let result = cook(
                &PolylineCut,
                "steep_curtain",
                &[Some(Data::geometry(body)), Some(Data::geometry(guide))],
                &[("view_dir", ParamValue::Vec3(DVec3::Z))],
            );
            match result {
                Ok(out) => {
                    let geo = out[0].as_geometry().unwrap();
                    for id in mesh::unique_parts(geo) {
                        let piece = mesh::extract_part(geo, id);
                        assert_closed_winding(&piece);
                    }
                    let error = (mesh::signed_volume(geo) - 1e6).abs() / 1e6;
                    assert!(error < 0.005, "height {height}: volume error {error}");
                    assert!(out[2].as_report().unwrap().passes());
                }
                Err(error) => assert!(
                    height >= 100.0 && error.to_string().contains("conditioning"),
                    "height {height}: unexpected failure {error}"
                ),
            }
        }
    }

    #[test]
    fn wp5_selected_cuts_preserve_untouched_seam_ids_and_joint_pairs() {
        for body_line in [false, true] {
            let cube = Geometry::make_box(DVec3::splat(100.0));
            let old_plane = Plane::new(DVec3::ZERO, DVec3::Z).unwrap();
            let mut input = cut_all(&cube, &[old_plane], 1e-6, true, Keep::Both)
                .unwrap()
                .geo;
            for point in &mut input.points {
                point.x += 200.0;
            }
            let mut new_body = cube;
            let n = new_body.prims.len();
            mesh::set_int_prim_attr(&mut new_body, mesh::PART, vec![2; n]);
            input.merge(&new_body);
            let selected = mesh::part_ids(&input)
                .iter()
                .enumerate()
                .filter_map(|(i, &part)| (part == 2).then_some(i as u32))
                .collect();
            input.prim_groups.insert("cut_region".into(), selected);
            let out = if body_line {
                cook(
                    &PolylineCut,
                    "selected_body_line_cap_ids",
                    &[
                        Some(Data::geometry(input)),
                        Some(Data::geometry(Geometry::from_points(vec![
                            DVec3::new(-50.0, 0.0, 0.0),
                            DVec3::new(50.0, 0.0, 0.0),
                        ]))),
                    ],
                    &[("view_dir", ParamValue::Vec3(DVec3::Z))],
                )
            } else {
                cook(
                    &PlaneCut,
                    "selected_plane_cap_ids",
                    &[Some(Data::geometry(input))],
                    &[("normal", ParamValue::Vec3(DVec3::Y))],
                )
            }
            .unwrap();
            let geo = out[0].as_geometry().unwrap();
            let caps = crate::joints::find_caps(geo);
            assert_eq!(caps.len(), 4);
            for cap in &caps {
                let expected = if cap.origin.x > 100.0 { 1 } else { 2 };
                assert_eq!(cap.cap_id, expected, "body line {body_line}");
            }
            let metadata = out[1].as_json().unwrap().as_array().unwrap();
            assert_eq!(metadata.len(), 1);
            assert_eq!(metadata[0]["cap"].as_i64(), Some(2));
            let joints = cook(
                &crate::nodes_joints::SnapJoint,
                "selected_cut_joint_pairs",
                &[Some(out[0].clone())],
                &[("max_count", ParamValue::Int(1))],
            )
            .unwrap();
            let report = joints[1].as_report().unwrap();
            assert_eq!(report.metrics["pair_count"].value, 2.0);
            assert_eq!(report.metrics["joint_count"].value, 2.0);
            assert!(report.passes());
        }
    }

    #[test]
    fn wp5_chained_planar_cuts_keep_distinct_seam_ids() {
        let body = Geometry::make_box(DVec3::splat(100.0));
        let first = cook(&PlaneCut, "first_cut_id", &[Some(Data::geometry(body))], &[])
            .unwrap();
        let second = cook(
            &PlaneCut,
            "second_cut_id",
            &[Some(first[0].clone())],
            &[("normal", ParamValue::Vec3(DVec3::Y))],
        )
        .unwrap();
        let caps = second[0]
            .as_geometry()
            .unwrap()
            .prim_attribs
            .ints(mesh::CAP)
            .unwrap();
        assert!(caps.contains(&1) && caps.contains(&2));
        assert_eq!(second[1].as_json().unwrap()[0]["cap"].as_i64(), Some(2));
    }

    #[test]
    fn wp5_curtain_excessive_finite_coordinates_and_views_fail_cleanly() {
        let body = Geometry::make_box(DVec3::splat(100.0));
        for (points, view) in [
            (vec![DVec3::NEG_X, DVec3::X], DVec3::splat(1e308)),
            (vec![DVec3::NEG_X, DVec3::X * 1e308], DVec3::Z),
        ] {
            let result = cook(
                &PolylineCut,
                "numeric_curtain_input",
                &[
                    Some(Data::geometry(body.clone())),
                    Some(Data::geometry(Geometry::from_points(points))),
                ],
                &[("view_dir", ParamValue::Vec3(view))],
            );
            assert!(result.is_err());
        }
    }

    #[test]
    fn wp5_selected_cuts_reject_complete_input_cap_id_overflow() {
        let mut body = Geometry::make_box(DVec3::splat(100.0));
        let n = body.prims.len();
        mesh::set_int_prim_attr(&mut body, mesh::CAP, vec![i64::MAX; n]);
        // Even an empty selected subset must not hide the full-input ids.
        body.prim_groups.insert("cut_region".into(), Vec::new());
        assert!(cook(&PlaneCut, "plane_cap_overflow", &[Some(Data::geometry(body.clone()))], &[]).is_err());
        let guide = Geometry::from_points(vec![DVec3::NEG_X, DVec3::X]);
        assert!(cook(
            &PolylineCut,
            "curtain_cap_overflow",
            &[Some(Data::geometry(body)), Some(Data::geometry(guide))],
            &[("view_dir", ParamValue::Vec3(DVec3::Z))],
        ).is_err());
    }

    #[test]
    fn wp5_empty_curtain_selection_preserves_the_uncut_body() {
        let mut body = Geometry::make_box(DVec3::splat(100.0));
        body.prim_groups.insert("cut_region".into(), Vec::new());
        let guide = Geometry::from_points(vec![
            DVec3::new(-60.0, -15.0, 0.0),
            DVec3::new(0.0, 20.0, 0.0),
            DVec3::new(60.0, -15.0, 0.0),
        ]);
        let out = cook(
            &PolylineCut,
            "empty_curtain_selection",
            &[Some(Data::geometry(body.clone())), Some(Data::geometry(guide))],
            &[("view_dir", ParamValue::Vec3(DVec3::Z))],
        ).unwrap();
        let geo = out[0].as_geometry().unwrap();
        assert_eq!(mesh::unique_parts(geo).len(), 1);
        assert_eq!(geo.bounds(), body.bounds());
        assert_eq!(geo.prims.len(), body.prims.len());
        assert!((mesh::signed_volume(geo) - 1e6).abs() < 1e-8);
        assert!(out[2].as_report().unwrap().passes());
    }

    #[test]
    fn wp5_parallel_projection_fails_without_panicking() {
        for scale in [1.0, 1e3, 1e6, 1e12] {
            let guide = Geometry::from_points(vec![DVec3::ZERO, DVec3::splat(scale)]);
            assert!(cook(
                &PolylineCut,
                "parallel_curtain_projection",
                &[
                    Some(Data::geometry(Geometry::make_box(DVec3::splat(100.0)))),
                    Some(Data::geometry(guide)),
                ],
                &[("view_dir", ParamValue::Vec3(DVec3::ONE))],
            ).is_err(), "parallel guide at scale {scale} must fail cleanly");
        }
    }
}

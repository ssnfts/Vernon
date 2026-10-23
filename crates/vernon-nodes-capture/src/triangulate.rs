//! Marker triangulation from a sparse model: linear (DLT) start, Gauss-Newton
//! refinement on the reprojection error, and rejection of bad observations.

use std::path::{Path, PathBuf};

use glam::{DMat3, DVec3};
use serde_json::Value;
use vernon_core::{AttribArray, Geometry, Metric, Report, Severity};
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::colmap_model::{read_model_dir, Camera, Image};
use crate::linalg::{solve, sym_eigen};
use crate::util::{self, base, set_metric_flag};

/// One view of a marker: pose, intrinsics scale and the undistorted
/// normalised observation.
#[derive(Debug, Clone, Copy)]
pub struct ViewObs {
    pub rot: DMat3,
    pub t: DVec3,
    pub centre: DVec3,
    pub x: f64,
    pub y: f64,
    pub fx: f64,
    pub fy: f64,
}

/// A triangulated marker.
#[derive(Debug, Clone)]
pub struct Triangulated {
    pub point: DVec3,
    pub rms_px: f64,
    pub max_px: f64,
    pub used: Vec<bool>,
    /// Mean distance from the point to the viewing rays, model units.
    pub mean_ray_dist: f64,
}

fn reproj(v: &ViewObs, p: DVec3) -> Option<(f64, f64)> {
    let pc = v.rot * p + v.t;
    if pc.z <= 1e-12 {
        return None;
    }
    Some((v.fx * (pc.x / pc.z - v.x), v.fy * (pc.y / pc.z - v.y)))
}

fn dlt(views: &[(usize, &ViewObs)]) -> Option<DVec3> {
    let mut ata = [0.0; 16];
    for (_, v) in views {
        let r1 = v.rot.row(0);
        let r2 = v.rot.row(1);
        let r3 = v.rot.row(2);
        let rows = [
            [v.x * r3.x - r1.x, v.x * r3.y - r1.y, v.x * r3.z - r1.z, v.x * v.t.z - v.t.x],
            [v.y * r3.x - r2.x, v.y * r3.y - r2.y, v.y * r3.z - r2.z, v.y * v.t.z - v.t.y],
        ];
        for row in rows {
            for i in 0..4 {
                for j in 0..4 {
                    ata[i * 4 + j] += row[i] * row[j];
                }
            }
        }
    }
    let (vals, vecs) = sym_eigen(4, &ata);
    let k = (0..4).min_by(|&a, &b| vals[a].total_cmp(&vals[b]))?;
    let h = [vecs[k], vecs[4 + k], vecs[8 + k], vecs[12 + k]];
    if h[3].abs() < 1e-14 {
        return None;
    }
    Some(DVec3::new(h[0] / h[3], h[1] / h[3], h[2] / h[3]))
}

fn refine(views: &[(usize, &ViewObs)], mut p: DVec3) -> DVec3 {
    for _ in 0..10 {
        let mut a = [0.0; 9];
        let mut b = [0.0; 3];
        for (_, v) in views {
            let pc = v.rot * p + v.t;
            if pc.z <= 1e-12 {
                return p;
            }
            let r = [v.fx * (pc.x / pc.z - v.x), v.fy * (pc.y / pc.z - v.y)];
            // d(pc/z)/dpc rows, then through the rotation.
            let jx = v.rot.transpose() * DVec3::new(1.0 / pc.z, 0.0, -pc.x / (pc.z * pc.z)) * v.fx;
            let jy = v.rot.transpose() * DVec3::new(0.0, 1.0 / pc.z, -pc.y / (pc.z * pc.z)) * v.fy;
            for (jr, res) in [(jx, r[0]), (jy, r[1])] {
                let j = jr.to_array();
                for i in 0..3 {
                    for k in 0..3 {
                        a[i * 3 + k] += j[i] * j[k];
                    }
                    b[i] -= j[i] * res;
                }
            }
        }
        for i in 0..3 {
            a[i * 4] += 1e-12;
        }
        let Some(dx) = solve(3, &a, &b) else { break };
        let step = DVec3::new(dx[0], dx[1], dx[2]);
        if !step.is_finite() {
            break;
        }
        p += step;
        if step.length() < 1e-12 * (1.0 + p.length()) {
            break;
        }
    }
    p
}

/// Triangulate one marker from its views.
pub fn triangulate(views: &[ViewObs], min_views: usize, max_reproj_px: f64, do_refine: bool) -> Result<Triangulated, String> {
    let min_views = min_views.max(2);
    if views.len() < min_views {
        return Err(format!("seen in {} view(s), need {min_views}", views.len()));
    }
    let mut used = vec![true; views.len()];
    loop {
        let act: Vec<(usize, &ViewObs)> = views.iter().enumerate().filter(|(i, _)| used[*i]).collect();
        let mut p = dlt(&act).ok_or("the viewing rays are degenerate (parallel or coincident)")?;
        if do_refine {
            p = refine(&act, p);
        }
        let errs: Vec<(usize, f64)> = act
            .iter()
            .map(|(i, v)| (*i, reproj(v, p).map_or(f64::INFINITY, |(a, b)| a.hypot(b))))
            .collect();
        let (worst_i, worst) = errs.iter().copied().fold((0, 0.0), |acc, e| if e.1 > acc.1 { e } else { acc });
        if worst > max_reproj_px {
            if act.len() > min_views {
                used[worst_i] = false;
                continue;
            }
            return Err(format!("reprojection error {worst:.2} px exceeds the {max_reproj_px:.2} px limit"));
        }
        let rms = (errs.iter().map(|e| e.1 * e.1).sum::<f64>() / errs.len() as f64).sqrt();
        let ray = act
            .iter()
            .map(|(_, v)| {
                let d = (v.rot.transpose() * DVec3::new(v.x, v.y, 1.0)).normalize_or_zero();
                (p - v.centre).cross(d).length()
            })
            .sum::<f64>()
            / act.len() as f64;
        return Ok(Triangulated {
            point: p,
            rms_px: rms,
            max_px: worst,
            used,
            mean_ray_dist: ray,
        });
    }
}

/// One 2D marker observation.
#[derive(Debug, Clone, PartialEq)]
pub struct Obs {
    pub image: String,
    pub id: String,
    pub u: f64,
    pub v: f64,
}

/// Parse rows `image, marker_id, u, v` (comma, semicolon, tab or space).
pub fn parse_observations_text(text: &str) -> Result<Vec<Obs>, String> {
    let mut out = Vec::new();
    for (i, l) in text.lines().enumerate() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = if l.contains([',', ';', '\t']) {
            l.split([',', ';', '\t']).map(str::trim).collect()
        } else {
            l.split_whitespace().collect()
        };
        if f.len() < 4 {
            return Err(format!("line {}: expected 'image, marker_id, u, v'", i + 1));
        }
        let (Ok(u), Ok(v)) = (f[2].parse::<f64>(), f[3].parse::<f64>()) else {
            if out.is_empty() {
                continue; // header row
            }
            return Err(format!("line {}: u and v must be numbers", i + 1));
        };
        if !(u.is_finite() && v.is_finite()) {
            return Err(format!("line {}: u and v must be finite", i + 1));
        }
        out.push(Obs {
            image: f[0].to_string(),
            id: f[1].to_string(),
            u,
            v,
        });
    }
    Ok(out)
}

/// Parse an array of `{image, id, u, v}` objects.
pub fn observations_from_json(v: &Value) -> Result<Vec<Obs>, String> {
    let arr = v
        .as_array()
        .or_else(|| v.get("observations").and_then(Value::as_array))
        .ok_or("expected a JSON array of {image, id, u, v} objects")?;
    let text = |o: &Value, keys: &[&str]| {
        keys.iter().find_map(|k| o.get(*k)).and_then(|x| match x {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
    };
    arr.iter()
        .enumerate()
        .map(|(i, o)| {
            let image = text(o, &["image", "image_name", "name"]);
            let id = text(o, &["id", "marker", "marker_id"]);
            let u = o.get("u").and_then(Value::as_f64);
            let vv = o.get("v").and_then(Value::as_f64);
            match (image, id, u, vv) {
                (Some(image), Some(id), Some(u), Some(v)) => Ok(Obs { image, id, u, v }),
                _ => Err(format!("row {i}: needs 'image', 'id', 'u' and 'v'")),
            }
        })
        .collect()
}

fn model_dir_of(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    }
}

/// Triangulate markers from 2D observations and a sparse model.
pub struct MarkerTriangulate;

impl NodeOp for MarkerTriangulate {
    fn info(&self) -> NodeTypeInfo {
        base("capture.marker_triangulate", "Triangulate Markers")
            .input(PortSpec::optional("model", DataKind::File))
            .input(PortSpec::optional("observations", DataKind::Json))
            .output(PortSpec::new("points", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::string("model_dir", "Model folder", "").help("Folder with cameras.txt and images.txt; used when no model input is connected."))
            .param(ParamSpec::string("observations", "Observations", "").help("Rows 'image, marker_id, u, v' in pixels; ignored when an observations table is connected."))
            .param(ParamSpec::int("min_views", "Minimum views", 3, 2, 50).help("Fewest images in which a marker must be seen to be triangulated. More views give a more reliable position."))
            .param(ParamSpec::float("max_reproj", "Reprojection limit", 2.0, 0.1, 20.0).unit("px").help("Observations further from the solution are dropped."))
            .param(ParamSpec::bool("refine", "Refine", true).help("Polish the linear solution on the reprojection error."))
            .help("Finds the 3D position of marked targets from their pixel positions in several photos and the camera poses of a sparse model. Rejects observations that disagree and reports the pixel residual. The result has the model's arbitrary scale until a scale node runs.")
            .emulates("coded-target and marker triangulation from a photogrammetry solution")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let dir = match inputs.first().and_then(|d| d.as_ref()).and_then(|d| d.as_file()) {
            Some(p) => model_dir_of(p),
            None => {
                let s = params.str("model_dir")?;
                if s.is_empty() {
                    return Err(ctx.fail("no model: connect a model folder or fill 'model_dir'"));
                }
                PathBuf::from(s)
            }
        };
        let model = read_model_dir(&dir, true).map_err(|m| ctx.fail(m))?;
        let obs = match util::json_input(inputs, 1) {
            Some(v) => observations_from_json(&v).map_err(|m| ctx.fail(format!("observations input: {m}")))?,
            None => parse_observations_text(params.str("observations")?)
                .map_err(|m| ctx.fail(format!("observations parameter: {m}")))?,
        };
        if obs.is_empty() {
            return Err(ctx.fail("no observations given"));
        }
        let min_views = params.int("min_views")?.max(2) as usize;
        let max_px = params.float("max_reproj")?;
        util::require_positive(ctx, "max_reproj", max_px)?;
        let refine = params.bool("refine")?;
        let find_image = |key: &str| -> Option<&Image> {
            model
                .images
                .iter()
                .find(|i| i.name == key)
                .or_else(|| key.parse::<u32>().ok().and_then(|id| model.images.iter().find(|i| i.id == id)))
        };
        let camera = |id: u32| -> Option<&Camera> { model.cameras.iter().find(|c| c.id == id) };
        // Group observations per marker, keeping first-seen order.
        let mut order: Vec<String> = Vec::new();
        let mut per_marker: std::collections::HashMap<String, Vec<ViewObs>> = std::collections::HashMap::new();
        let mut unknown_images = 0usize;
        for o in &obs {
            let Some(img) = find_image(&o.image) else {
                unknown_images += 1;
                continue;
            };
            let cam = camera(img.camera_id)
                .ok_or_else(|| ctx.fail(format!("image '{}' refers to unknown camera {}", img.name, img.camera_id)))?;
            let k = cam.intrinsics().map_err(|m| ctx.fail(m))?;
            let (x, y) = k.unproject(o.u, o.v);
            let view = ViewObs {
                rot: img.rotation(),
                t: img.translation(),
                centre: img.centre(),
                x,
                y,
                fx: k.fx,
                fy: k.fy,
            };
            if !per_marker.contains_key(&o.id) {
                order.push(o.id.clone());
            }
            per_marker.entry(o.id.clone()).or_default().push(view);
        }
        let mut pts = Vec::new();
        let mut ids = Vec::new();
        let mut rms = Vec::new();
        let mut views = Vec::new();
        let mut rejected: Vec<(String, String)> = Vec::new();
        let mut dropped_obs = 0usize;
        let mut ray_sum = 0.0;
        let mut max_rms: f64 = 0.0;
        for id in &order {
            let vs = &per_marker[id];
            match triangulate(vs, min_views, max_px, refine) {
                Ok(t) => {
                    dropped_obs += t.used.iter().filter(|u| !**u).count();
                    pts.push(t.point);
                    ids.push(id.clone());
                    rms.push(t.rms_px);
                    views.push(t.used.iter().filter(|u| **u).count() as i64);
                    ray_sum += t.mean_ray_dist;
                    max_rms = max_rms.max(t.rms_px);
                }
                Err(m) => rejected.push((id.clone(), m)),
            }
        }
        let mut geo = Geometry::from_points(pts);
        geo.point_attribs.set("id", AttribArray::Str(ids));
        geo.point_attribs.set("rms_px", AttribArray::Float(rms.clone()));
        geo.point_attribs.set("views", AttribArray::Int(views));
        set_metric_flag(&mut geo, false);
        let mut rep = Report::new("Triangulate Markers");
        rep.metric("markers", Metric::new(order.len() as f64, "markers"));
        rep.metric("triangulated", Metric::new(geo.point_count() as f64, "markers"));
        rep.metric("markers_rejected", Metric::new(rejected.len() as f64, "markers").with_limit(0.0, true));
        rep.metric("observations_dropped", Metric::new(dropped_obs as f64, "observations"));
        if !rms.is_empty() {
            let mean = rms.iter().sum::<f64>() / rms.len() as f64;
            rep.metric("mean_rms_px", Metric::new(mean, "px").with_limit(max_px, true));
            rep.metric("max_rms_px", Metric::new(max_rms, "px"));
            rep.metric("mean_ray_distance", Metric::new(ray_sum / rms.len() as f64, "model units"));
        }
        for (id, m) in &rejected {
            rep.note(Severity::Warning, format!("marker '{id}' rejected: {m}"));
        }
        if unknown_images > 0 {
            rep.note(Severity::Warning, format!("{unknown_images} observation(s) name images that are not in the model"));
        }
        rep.note(
            Severity::Info,
            "positions are in the model's arbitrary scale and marked non-metric; run Solve Scale Bars or Scale From Distance next",
        );
        Ok(vec![Data::geometry(geo), Data::report(rep)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colmap_model::fixtures::write_model;
    use glam::DQuat;
    use serde_json::json;

    fn cameras() -> Vec<Camera> {
        vec![
            Camera {
                id: 1,
                model: "SIMPLE_RADIAL".into(),
                width: 4000,
                height: 3000,
                params: vec![3200.0, 2000.0, 1500.0, -0.03],
            },
            Camera {
                id: 2,
                model: "PINHOLE".into(),
                width: 4000,
                height: 3000,
                params: vec![2800.0, 2810.0, 2000.0, 1500.0],
            },
        ]
    }

    /// Four cameras on a ring looking at the origin from 3 m.
    fn images() -> Vec<Image> {
        (0..4)
            .map(|k| {
                let ang = k as f64 * 0.5 - 0.7;
                let rot = DQuat::from_rotation_y(ang);
                // World-to-camera: R = rot^-1 sends the camera axis to the origin.
                let r = rot.inverse();
                let centre = rot * DVec3::new(0.0, 0.0, -3000.0);
                let t = -(r * centre);
                Image {
                    id: k + 1,
                    q: [r.w, r.x, r.y, r.z],
                    t: t.to_array(),
                    camera_id: if k % 2 == 0 { 1 } else { 2 },
                    name: format!("img{k}.jpg"),
                }
            })
            .collect()
    }

    fn project(cams: &[Camera], im: &Image, p: DVec3) -> (f64, f64) {
        let cam = cams.iter().find(|c| c.id == im.camera_id).unwrap();
        cam.intrinsics().unwrap().project(im.rotation() * p + im.translation())
    }

    fn obs_text(cams: &[Camera], imgs: &[Image], markers: &[(&str, DVec3)], bad: Option<(&str, usize, f64)>) -> String {
        let mut s = String::from("image,marker,u,v\n");
        for (id, p) in markers {
            for (k, im) in imgs.iter().enumerate() {
                let (mut u, v) = project(cams, im, *p);
                if let Some((bid, bk, du)) = bad {
                    if bid == *id && bk == k {
                        u += du;
                    }
                }
                s += &format!("{},{},{u:.10},{v:.10}\n", im.name, id);
            }
        }
        s
    }

    #[test]
    fn recovers_marker_positions_through_distortion() {
        let cams = cameras();
        let imgs = images();
        let dir = write_model("tri", &cams, &imgs);
        let markers = [("M1", DVec3::new(120.0, -80.0, 40.0)), ("M2", DVec3::new(-300.0, 200.0, -90.0))];
        let op = MarkerTriangulate;
        let mut p = Params::from_specs(&op.info().params);
        p.set("model_dir", vernon_graph::ParamValue::Str(dir.to_string_lossy().into_owned()));
        p.set("observations", vernon_graph::ParamValue::Str(obs_text(&cams, &imgs, &markers, None)));
        let ctx = CookContext {
            node_name: "t".into(),
            work_dir: std::env::temp_dir(),
            frame: 0.0,
        };
        let out = op.cook(&ctx, &[], &p).unwrap();
        let g = out[0].as_geometry().unwrap();
        assert_eq!(g.point_count(), 2);
        for (i, (_, truth)) in markers.iter().enumerate() {
            assert!((g.points[i] - *truth).length() < 1e-4, "marker {i}: {:?} vs {truth:?}", g.points[i]);
        }
        let rep = out[1].as_report().unwrap();
        assert!(rep.metrics["max_rms_px"].value < 1e-4);
        assert_eq!(rep.metrics["markers_rejected"].value, 0.0);
        assert_eq!(util::metric_flag(g), Some(false));
        assert_eq!(g.point_attribs.strs("id").unwrap(), ["M1", "M2"]);
    }

    #[test]
    fn a_wrong_observation_is_dropped_and_the_point_still_recovered() {
        let cams = cameras();
        let imgs = images();
        let dir = write_model("tri-bad", &cams, &imgs);
        let truth = DVec3::new(50.0, 60.0, -20.0);
        let text = obs_text(&cams, &imgs, &[("A", truth)], Some(("A", 2, 40.0)));
        let obs = parse_observations_text(&text).unwrap();
        assert_eq!(obs.len(), 4);
        let op = MarkerTriangulate;
        let mut p = Params::from_specs(&op.info().params);
        p.set("model_dir", vernon_graph::ParamValue::Str(dir.to_string_lossy().into_owned()));
        p.set("min_views", vernon_graph::ParamValue::Int(3));
        let ctx = CookContext {
            node_name: "t".into(),
            work_dir: std::env::temp_dir(),
            frame: 0.0,
        };
        let json_obs = Data::json(json!(obs.iter().map(|o| json!({"image": o.image, "id": o.id, "u": o.u, "v": o.v})).collect::<Vec<_>>()));
        let out = op.cook(&ctx, &[None, Some(json_obs.clone())], &p).unwrap();
        let g = out[0].as_geometry().unwrap();
        assert_eq!(g.point_count(), 1);
        assert!((g.points[0] - truth).length() < 1e-3, "{:?}", g.points[0]);
        assert_eq!(out[1].as_report().unwrap().metrics["observations_dropped"].value, 1.0);
        // Requiring four good views rejects the marker instead of guessing.
        p.set("min_views", vernon_graph::ParamValue::Int(4));
        let out = op.cook(&ctx, &[None, Some(json_obs)], &p).unwrap();
        assert_eq!(out[0].as_geometry().unwrap().point_count(), 0);
        assert!(!out[1].as_report().unwrap().passes());
    }

    #[test]
    fn observation_parsing_and_errors() {
        assert_eq!(parse_observations_text("a.jpg 7 10.5 20\n").unwrap()[0].id, "7");
        assert!(parse_observations_text("a.jpg,7,x,20\nb.jpg,7,1,2\n").is_ok(), "header-like first row is skipped");
        assert!(parse_observations_text("a.jpg,7,1,2\nb.jpg,7,x,2\n").is_err());
        assert!(parse_observations_text("a.jpg,7\n").is_err());
        assert!(observations_from_json(&json!([{"image": "a", "id": 1}])).is_err());
        let op = MarkerTriangulate;
        let ctx = CookContext {
            node_name: "t".into(),
            work_dir: std::env::temp_dir(),
            frame: 0.0,
        };
        assert!(op.cook(&ctx, &[], &Params::from_specs(&op.info().params)).is_err());
    }

    #[test]
    fn parallel_rays_are_degenerate() {
        let v = |x: f64| ViewObs {
            rot: DMat3::IDENTITY,
            t: DVec3::new(x, 0.0, 0.0),
            centre: DVec3::new(-x, 0.0, 0.0),
            x: 0.0,
            y: 0.0,
            fx: 1000.0,
            fy: 1000.0,
        };
        // Two cameras translated sideways, both seeing the point at the image
        // centre: rays are parallel, so no finite intersection exists.
        let r = triangulate(&[v(0.0), v(100.0), v(200.0)], 2, 2.0, true);
        assert!(r.is_err() || r.unwrap().point.z.abs() > 1e6);
    }
}

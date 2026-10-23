//! Distance nodes: area-weighted surface sampling, cloud-to-cloud distance
//! (with local surface models) and signed cloud-to-mesh distance.
//!
//! Deviation convention: millimetres, positive on the outward-normal side of
//! the reference, stored in the point attribute `dist`; `dist_valid` is 1
//! where a true value was found and 0 where the search gave up.

use glam::{DMat3, DVec3};
use rayon::prelude::*;
use vernon_core::{AttribArray, Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::kdtree::KdTree;
use crate::linalg::{eigen3, solve_pinv, Mat, MAXN};
use crate::stats::{ramp, range_t, Scale};
use crate::trimesh::{SignMode, TriMesh};
use crate::util::{add, ensure_metric, pct, pf, pi, Rng, SPEC_ALIGN};

pub fn register(reg: &mut Registry) {
    add(reg, sample_info, sample_cook);
    add(reg, cloud_info, cloud_cook);
    add(reg, mesh_info, mesh_cook);
}

fn geo_out() -> PortSpec {
    PortSpec::new("geometry", DataKind::Geometry)
}

fn rep_out() -> PortSpec {
    PortSpec::new("report", DataKind::Report)
}

pub const GREY: DVec3 = DVec3::new(0.5, 0.5, 0.5);

// ----------------------------------------------------------- sample surface

fn sample_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.sample_surface", "Sample Surface", Stage::Clean)
        .input(PortSpec::new("geometry", DataKind::Geometry))
        .output(geo_out())
        .output(rep_out())
        .param(ParamSpec::choice("mode", "Mode", "density", &["density", "count"]).help("Choose the number of samples from a density or give it directly."))
        .param(
            ParamSpec::float("density", "Density", 1.0, 0.01, 100.0)
                .unit("points/mm²")
                .help("Samples per unit surface area in density mode."),
        )
        .param(ParamSpec::int("count", "Count", 100000, 1, 50_000_000).help("Number of samples in count mode."))
        .param(ParamSpec::int("seed", "Seed", 1, 0, 1_000_000_000).help("Random seed."))
        .help("Scatters points uniformly over a mesh surface (area-weighted, stratified) and writes normals, so a sparse mesh can be compared by its surface instead of only its vertices.")
        .emulates("sampling points on a mesh surface")
        .spec(SPEC_ALIGN)
        .implemented()
}

fn sample_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    ensure_metric(ctx, &g)?;
    let mesh = TriMesh::from_geometry(&g).ok_or_else(|| ctx.fail("input has no faces to sample"))?;
    let areas: Vec<f64> = (0..mesh.tris.len() as u32).map(|t| mesh.tri_area(t)).collect();
    let mut cum = Vec::with_capacity(areas.len());
    let mut total = 0.0;
    for a in &areas {
        total += a;
        cum.push(total);
    }
    if total <= 0.0 {
        return Err(ctx.fail("the mesh has zero area"));
    }
    let count = if p.str("mode")? == "count" {
        pi(p, "count", 1, 50_000_000)? as usize
    } else {
        let d = pf(p, "density", 0.01, 100.0)?;
        let c = (d * total).round();
        if c > 50_000_000.0 {
            return Err(ctx.fail(format!("density {d} would create {c:.0} points; lower it or use count mode")));
        }
        (c as usize).max(1)
    };
    let seed = pi(p, "seed", 0, 1_000_000_000)? as u64;
    let vn = g.point_attribs.vec3s("N").ok().filter(|n| n.len() == g.points.len());
    const CHUNK: usize = 65_536;
    let chunks: Vec<(Vec<DVec3>, Vec<DVec3>, Vec<i64>)> = (0..count.div_ceil(CHUNK))
        .into_par_iter()
        .map(|c| {
            let mut rng = Rng::new(seed.wrapping_mul(1_000_003).wrapping_add(c as u64));
            let lo = c * CHUNK;
            let hi = (lo + CHUNK).min(count);
            let mut pts = Vec::with_capacity(hi - lo);
            let mut nrm = Vec::with_capacity(hi - lo);
            let mut src = Vec::with_capacity(hi - lo);
            for j in lo..hi {
                // stratified position along the cumulative area
                let u = (j as f64 + rng.next_f64()) / count as f64 * total;
                let t = cum.partition_point(|&x| x < u).min(cum.len() - 1);
                let (r1, r2) = (rng.next_f64().sqrt(), rng.next_f64());
                let (w0, w1, w2) = (1.0 - r1, r1 * (1.0 - r2), r1 * r2);
                let [a, b, cc] = mesh.tri_points(t as u32);
                pts.push(a * w0 + b * w1 + cc * w2);
                let tv = mesh.tris[t];
                let n = match vn {
                    Some(vn) => {
                        let s = vn[tv[0] as usize] * w0 + vn[tv[1] as usize] * w1 + vn[tv[2] as usize] * w2;
                        let s = s.normalize_or_zero();
                        if s == DVec3::ZERO {
                            mesh.tri_n[t]
                        } else {
                            s
                        }
                    }
                    None => mesh.tri_n[t],
                };
                nrm.push(n);
                src.push(mesh.src_prim[t] as i64);
            }
            (pts, nrm, src)
        })
        .collect();
    let mut pts = Vec::with_capacity(count);
    let mut nrm = Vec::with_capacity(count);
    let mut src = Vec::with_capacity(count);
    for (a, b, c) in chunks {
        pts.extend(a);
        nrm.extend(b);
        src.extend(c);
    }
    let mut out = Geometry::from_points(pts);
    out.point_attribs.set("N", AttribArray::Vec3(nrm));
    out.point_attribs.set("src_prim", AttribArray::Int(src));
    out.detail_attribs = g.detail_attribs.clone();
    let mut rep = Report::new("Sample Surface");
    rep.metric("samples", Metric::new(count as f64, "points"));
    rep.metric("surface_area", Metric::new(total, "mm²"));
    rep.metric("density", Metric::new(count as f64 / total, "points/mm²"));
    if mesh.topo.degenerate_tris > 0 {
        rep.note(Severity::Info, format!("{} degenerate faces were skipped", mesh.topo.degenerate_tris));
    }
    Ok(vec![Data::geometry(out), Data::report(rep)])
}

// ---------------------------------------------------------- shared helpers

/// Summary report of a deviation array with validity flags.
fn deviation_report(title: &str, dist: &[f64], valid: &[i64]) -> Report {
    let vals: Vec<f64> = dist
        .iter()
        .zip(valid)
        .filter(|(d, v)| **v == 1 && d.is_finite())
        .map(|(d, _)| *d)
        .collect();
    let m = crate::stats::moments(&vals, None);
    let mut rep = Report::new(title);
    rep.metric("valid_points", Metric::new(vals.len() as f64, "points"));
    rep.metric("invalid_points", Metric::new((dist.len() - vals.len()) as f64, "points"));
    rep.metric("mean_dev", Metric::new(m.mean, "mm"));
    rep.metric("std_dev", Metric::new(m.std, "mm"));
    rep.metric("rms_dev", Metric::new(m.rms, "mm"));
    rep.metric("max_abs_dev", Metric::new(m.max_abs, "mm"));
    if vals.is_empty() {
        rep.note(Severity::Warning, "no point found a counterpart on the reference");
    } else if dist.len() > vals.len() {
        rep.note(
            Severity::Info,
            format!("{} point(s) had no counterpart within the maximum distance and are flagged invalid", dist.len() - vals.len()),
        );
    }
    rep
}

fn write_dev(out: &mut Geometry, dist: Vec<f64>, valid: Vec<i64>) {
    out.point_attribs.set("dist", AttribArray::Float(dist));
    out.point_attribs.set("dist_valid", AttribArray::Int(valid));
}

fn cloud_points(g: &Geometry) -> &[DVec3] {
    &g.points
}

// ---------------------------------------------------------- cloud distance

fn cloud_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.cloud_distance", "Point Distance", Stage::Clean)
        .input(PortSpec::new("compared", DataKind::Geometry))
        .input(PortSpec::new("reference", DataKind::Geometry))
        .output(geo_out())
        .output(rep_out())
        .param(ParamSpec::float("max_dist", "Max distance", 0.0, 0.0, 100000.0).unit("mm").help("0 = off. Points farther than this get the value 'max distance' and are flagged invalid."))
        .param(ParamSpec::choice("local_model", "Local model", "none", &["none", "plane", "quadric", "triangulation"]).help("none: nearest-neighbour distance (over-reports on sparse references). plane/quadric: distance to a surface fitted to the neighbours of the nearest reference point; the smaller value wins. triangulation currently falls back to plane."))
        .param(ParamSpec::choice("neighbourhood", "Neighbourhood", "radius", &["radius", "count"]).help("How neighbours of the nearest reference point are gathered."))
        .param(ParamSpec::float("radius", "Radius", 2.0, 0.1, 100.0).unit("mm").help("Neighbourhood radius."))
        .param(ParamSpec::int("k", "Neighbour count", 6, 3, 64).help("Number of neighbours in count mode."))
        .param(ParamSpec::bool("split_xyz", "Split X/Y/Z", false).help("Also write the absolute per-axis offsets to the nearest reference point as 'dx', 'dy', 'dz'."))
        .help("Unsigned distance from each compared point to a reference cloud. Writes 'dist' and 'dist_valid'. For a sparse reference use a local plane or quadric model.")
        .emulates("cloud-to-cloud distance with local surface models")
        .spec(SPEC_ALIGN)
        .implemented()
}

/// Centroid and principal axes (strongest first) of a neighbourhood.
fn frame_fit(pts: &[DVec3], ids: &[u32]) -> (DVec3, [DVec3; 3]) {
    let c = ids.iter().map(|&i| pts[i as usize]).fold(DVec3::ZERO, |a, b| a + b) / ids.len() as f64;
    let mut cov = DMat3::ZERO;
    for &i in ids {
        let d = pts[i as usize] - c;
        cov += DMat3::from_cols(d * d.x, d * d.y, d * d.z);
    }
    (c, eigen3(&cov).1)
}

/// Plane normal and centroid of a neighbourhood.
fn plane_fit(pts: &[DVec3], ids: &[u32]) -> (DVec3, DVec3) {
    let (c, axes) = frame_fit(pts, ids);
    (c, axes[2])
}

/// Vertical distance of `q` to a quadric height field fitted through the
/// neighbourhood, in the local plane frame. `None` when ill-conditioned.
fn quadric_distance(pts: &[DVec3], ids: &[u32], origin: DVec3, q: DVec3) -> Option<f64> {
    let (_, axes) = frame_fit(pts, ids);
    let (u, v, w) = (axes[0], axes[1], axes[2]);
    let to_local = |x: DVec3| {
        let d = x - origin;
        (d.dot(u), d.dot(v), d.dot(w))
    };
    let mut ata: Mat = [[0.0; MAXN]; MAXN];
    let mut atb = [0.0; MAXN];
    for &i in ids {
        let (a, b, h) = to_local(pts[i as usize]);
        let row = [a * a, a * b, b * b, a, b, 1.0];
        for r in 0..6 {
            for cc in 0..6 {
                ata[r][cc] += row[r] * row[cc];
            }
            atb[r] += row[r] * h;
        }
    }
    let sol = solve_pinv(6, &ata, &atb, 1e10);
    if sol.free.len() > 1 {
        return None;
    }
    let (a, b, hq) = to_local(q);
    let x = sol.x;
    let fit = x[0] * a * a + x[1] * a * b + x[2] * b * b + x[3] * a + x[4] * b + x[5];
    Some((hq - fit).abs())
}

fn cloud_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let cmp = inputs::geometry(inp, 0)?;
    let rf = inputs::geometry(inp, 1)?;
    ensure_metric(ctx, &cmp)?;
    ensure_metric(ctx, &rf)?;
    if rf.points.is_empty() {
        return Err(ctx.fail("the reference has no points"));
    }
    let max_dist = pf(p, "max_dist", 0.0, 100000.0)?;
    let model = p.str("local_model")?.to_string();
    let by_radius = p.str("neighbourhood")? == "radius";
    let radius = pf(p, "radius", 0.1, 100.0)?;
    let k = pi(p, "k", 3, 64)? as usize;
    let split = p.bool("split_xyz")?;
    let refp = cloud_points(&rf);
    let tree = KdTree::new(refp);
    let cap2 = if max_dist > 0.0 { max_dist * max_dist } else { f64::INFINITY };
    let min_nb = if model == "quadric" { 6 } else { 3 };
    #[derive(Clone, Copy)]
    struct Row {
        d: f64,
        valid: i64,
        off: DVec3,
        modelled: bool,
    }
    let rows: Vec<Row> = cmp
        .points
        .par_iter()
        .map_init(
            || (Vec::<u32>::new(), Vec::<(f64, u32)>::new()),
            |(ids, knn), q| {
                let Some((i, d2)) = tree.nearest_within(*q, cap2) else {
                    return Row {
                        d: max_dist,
                        valid: 0,
                        off: DVec3::ZERO,
                        modelled: false,
                    };
                };
                let nn = refp[i as usize];
                let mut d = d2.sqrt();
                let mut modelled = false;
                if model != "none" {
                    if by_radius {
                        tree.radius(nn, radius, ids);
                    } else {
                        tree.knn(nn, k, knn);
                        ids.clear();
                        ids.extend(knn.iter().map(|e| e.1));
                    }
                    if ids.len() >= min_nb {
                        let dm = if model == "quadric" {
                            quadric_distance(refp, ids, nn, *q)
                        } else {
                            let (c, n) = plane_fit(refp, ids);
                            Some(n.dot(*q - c).abs())
                        };
                        if let Some(dm) = dm {
                            if dm.is_finite() && dm < d {
                                d = dm;
                                modelled = true;
                            }
                        }
                    }
                }
                Row {
                    d,
                    valid: 1,
                    off: (*q - nn).abs(),
                    modelled,
                }
            },
        )
        .collect();
    let dist: Vec<f64> = rows.iter().map(|r| r.d).collect();
    let valid: Vec<i64> = rows.iter().map(|r| r.valid).collect();
    let mut rep = deviation_report("Point Distance", &dist, &valid);
    let modelled = rows.iter().filter(|r| r.modelled).count();
    if model != "none" {
        rep.metric("model_used", Metric::new(pct(modelled, rows.len()), "%"));
    }
    if model == "triangulation" {
        rep.note(Severity::Warning, "white-box placeholder: the triangulation model is not implemented yet and the plane model was used instead");
    }
    if model == "quadric" {
        rep.note(Severity::Info, "quadric distances are vertical distances to the fitted height field and approximate the true distance");
    }
    if model == "none" {
        rep.note(Severity::Info, "no local model: distances are nearest-neighbour values and over-report on sparse references");
    }
    let mut out = (*cmp).clone();
    write_dev(&mut out, dist, valid);
    if split {
        out.point_attribs.set("dx", AttribArray::Float(rows.iter().map(|r| r.off.x).collect()));
        out.point_attribs.set("dy", AttribArray::Float(rows.iter().map(|r| r.off.y).collect()));
        out.point_attribs.set("dz", AttribArray::Float(rows.iter().map(|r| r.off.z).collect()));
    }
    Ok(vec![Data::geometry(out), Data::report(rep)])
}

// ----------------------------------------------------------- mesh distance

pub(crate) fn mesh_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.mesh_distance", "Surface Distance", Stage::Clean)
        .input(PortSpec::new("compared", DataKind::Geometry))
        .input(PortSpec::new("reference", DataKind::Geometry))
        .output(geo_out())
        .output(rep_out())
        .param(ParamSpec::bool("signed", "Signed", true).help("Positive on the outward-normal side of the reference mesh, negative below it."))
        .param(ParamSpec::bool("flip", "Flip sign", false).help("Negate the sign, for reference meshes with reversed winding."))
        .param(ParamSpec::float("max_dist", "Max distance", 0.0, 0.0, 100000.0).unit("mm").help("0 = off. Points farther than this get the value 'max distance' and are flagged invalid."))
        .param(ParamSpec::bool("pseudonormals", "Edge-safe sign", true).help("Use angle-weighted pseudo-normals so points nearest to an edge or vertex get the right sign. Off: sign from the nearest face only."))
        .param(ParamSpec::bool("colorize", "Write colours", true).help("Also write a symmetric colour map to 'Cd'."))
        .param(ParamSpec::float("color_limit", "Colour range", 0.5, 0.001, 1000.0).unit("mm").help("Colours span minus to plus this value, with zero on green."))
        .help("Signed distance from each compared point (or mesh vertex) to the closest point on the reference mesh. Writes 'dist' (mm, signed), 'dist_valid' and the colour 'Cd'. Reports how many reference edges are open, because the sign is only reliable near closed or consistently oriented surfaces.")
        .emulates("cloud-to-mesh signed distance with colour deviation map")
        .spec(SPEC_ALIGN)
        .implemented()
}

pub(crate) fn mesh_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let cmp = inputs::geometry(inp, 0)?;
    let rf = inputs::geometry(inp, 1)?;
    ensure_metric(ctx, &cmp)?;
    ensure_metric(ctx, &rf)?;
    let mesh = TriMesh::from_geometry(&rf).ok_or_else(|| ctx.fail("the reference needs faces; for a point reference use the point distance node"))?;
    if cmp.points.is_empty() {
        return Err(ctx.fail("nothing to compare: the compared geometry has no points"));
    }
    let signed = p.bool("signed")?;
    let flip = p.bool("flip")?;
    let max_dist = pf(p, "max_dist", 0.0, 100000.0)?;
    let mode = if !signed {
        SignMode::None
    } else if p.bool("pseudonormals")? {
        SignMode::Pseudo
    } else {
        SignMode::Face
    };
    let colorize = p.bool("colorize")?;
    let lim = pf(p, "color_limit", 0.001, 1000.0)?;
    let cap2 = if max_dist > 0.0 { max_dist * max_dist } else { f64::INFINITY };
    let res: Vec<(f64, i64)> = cmp
        .points
        .par_iter()
        .map(|q| match mesh.closest(*q, cap2, mode) {
            Some(h) => {
                let v = if flip && signed { -h.signed } else { h.signed };
                (v, 1)
            }
            None => (max_dist, 0),
        })
        .collect();
    let dist: Vec<f64> = res.iter().map(|r| r.0).collect();
    let valid: Vec<i64> = res.iter().map(|r| r.1).collect();
    let mut rep = deviation_report("Surface Distance", &dist, &valid);
    rep.metric("reference_open_edges", Metric::new(mesh.topo.open_edges as f64, "edges"));
    rep.metric("reference_faces", Metric::new(mesh.tris.len() as f64, ""));
    if signed && mesh.topo.open_edges > 0 {
        rep.note(
            Severity::Warning,
            format!(
                "the reference has {} open edge(s): the sign is only meaningful near a consistently oriented sheet",
                mesh.topo.open_edges
            ),
        );
    }
    if signed && (mesh.topo.inconsistent_edges > 0 || mesh.topo.nonmanifold_edges > 0) {
        rep.note(
            Severity::Warning,
            format!(
                "the reference has {} inconsistently oriented and {} non-manifold edge(s); signs may flip between neighbouring points",
                mesh.topo.inconsistent_edges, mesh.topo.nonmanifold_edges
            ),
        );
    }
    if mesh.topo.is_closed() && mesh.topo.signed_volume < 0.0 && signed && !flip {
        rep.note(Severity::Info, "the reference is closed but inward-facing (negative volume); consider 'flip sign'");
    }
    let mut out = (*cmp).clone();
    if colorize {
        let cd: Vec<DVec3> = res
            .iter()
            .map(|(d, v)| {
                if *v == 1 {
                    ramp(Scale::Diverging, range_t(*d, -lim, lim, 0))
                } else {
                    GREY
                }
            })
            .collect();
        out.point_attribs.set("Cd", AttribArray::Vec3(cd));
    }
    write_dev(&mut out, dist, valid);
    Ok(vec![Data::geometry(out), Data::report(rep)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;

    #[test]
    fn sampling_covers_a_cube_by_area() {
        let g = Geometry::make_box(DVec3::splat(100.0));
        let out = run(sample_info, sample_cook, vec![geo(g.clone())], &[("density", f(0.1))]).unwrap();
        let o = out_geo(&out[0]);
        assert_eq!(o.points.len(), 6000);
        let mesh = TriMesh::from_geometry(&g).unwrap();
        // every sample lies on the surface and its normal is a unit axis vector
        for (q, n) in o.points.iter().zip(o.point_attribs.vec3s("N").unwrap()) {
            assert!(mesh.distance(*q) < 1e-9);
            assert!((n.length() - 1.0).abs() < 1e-9);
        }
        // roughly 1000 samples on each face (±8 %)
        let mut per_face = [0usize; 6];
        for q in &o.points {
            let f = if q.x.abs() > 49.999 {
                if q.x > 0.0 {
                    0
                } else {
                    1
                }
            } else if q.y.abs() > 49.999 {
                if q.y > 0.0 {
                    2
                } else {
                    3
                }
            } else if q.z > 0.0 {
                4
            } else {
                5
            };
            per_face[f] += 1;
        }
        for c in per_face {
            assert!((c as f64 - 1000.0).abs() < 80.0, "{per_face:?}");
        }
        let r = out_report(&out[1]);
        assert!((r.metrics["surface_area"].value - 60000.0).abs() < 1e-6);
        // deterministic
        let again = run(sample_info, sample_cook, vec![geo(g)], &[("density", f(0.1))]).unwrap();
        assert_eq!(out_geo(&again[0]).points, o.points);
    }

    #[test]
    fn sampling_count_mode_and_errors() {
        let g = sphere_geo(50.0, 24, 12);
        let out = run(sample_info, sample_cook, vec![geo(g)], &[("mode", s("count")), ("count", i(1234))]).unwrap();
        assert_eq!(out_geo(&out[0]).points.len(), 1234);
        assert!(run(sample_info, sample_cook, vec![geo(Geometry::from_points(vec![DVec3::ZERO; 4]))], &[]).is_err());
    }

    #[test]
    fn golden_8_unsigned_vs_modelled() {
        let reference = grid_plane(40, 1.0);
        let cmp = Geometry::from_points(vec![DVec3::new(20.5, 20.5, 0.3)]);
        let none = run(cloud_info, cloud_cook, vec![geo(cmp.clone()), geo(reference.clone())], &[]).unwrap();
        let d = out_geo(&none[0]).point_attribs.floats("dist").unwrap()[0];
        assert!((d - 0.768).abs() < 1e-3, "{d}");
        let plane = run(cloud_info, cloud_cook, vec![geo(cmp.clone()), geo(reference.clone())], &[("local_model", s("plane"))]).unwrap();
        let d = out_geo(&plane[0]).point_attribs.floats("dist").unwrap()[0];
        assert!((d - 0.3).abs() < 1e-9, "{d}");
        let quad = run(
            cloud_info,
            cloud_cook,
            vec![geo(cmp.clone()), geo(reference.clone())],
            &[("local_model", s("quadric")), ("neighbourhood", s("count")), ("k", i(12))],
        )
        .unwrap();
        let d = out_geo(&quad[0]).point_attribs.floats("dist").unwrap()[0];
        assert!((d - 0.3).abs() < 1e-6, "{d}");
        let tri = run(cloud_info, cloud_cook, vec![geo(cmp), geo(reference)], &[("local_model", s("triangulation"))]).unwrap();
        assert!(out_report(&tri[1]).notes.iter().any(|n| n.1.contains("white-box placeholder")));
    }

    #[test]
    fn cloud_distance_max_dist_and_split() {
        let reference = grid_plane(10, 1.0);
        let cmp = Geometry::from_points(vec![DVec3::new(5.0, 5.0, 0.4), DVec3::new(5.0, 5.0, 50.0)]);
        let out = run(cloud_info, cloud_cook, vec![geo(cmp), geo(reference)], &[("max_dist", f(2.0)), ("split_xyz", b(true))]).unwrap();
        let o = out_geo(&out[0]);
        assert_eq!(o.point_attribs.ints("dist_valid").unwrap(), &[1, 0]);
        let d = o.point_attribs.floats("dist").unwrap();
        assert!((d[0] - 0.4).abs() < 1e-12);
        assert_eq!(d[1], 2.0);
        assert!((o.point_attribs.floats("dz").unwrap()[0] - 0.4).abs() < 1e-12);
        let r = out_report(&out[1]);
        assert_eq!(r.metrics["invalid_points"].value, 1.0);
    }

    fn plate() -> Geometry {
        Geometry::from_polygons(
            vec![
                DVec3::new(-100.0, -100.0, 0.0),
                DVec3::new(100.0, -100.0, 0.0),
                DVec3::new(100.0, 100.0, 0.0),
                DVec3::new(-100.0, 100.0, 0.0),
            ],
            vec![vec![0, 1, 2], vec![0, 2, 3]],
        )
    }

    #[test]
    fn golden_9_signed_plate_node() {
        let cmp = Geometry::from_points(vec![DVec3::new(10.0, 10.0, 0.3), DVec3::new(-20.0, 5.0, -0.4), DVec3::new(50.0, 50.0, 0.0)]);
        let out = run(mesh_info, mesh_cook, vec![geo(cmp.clone()), geo(plate())], &[]).unwrap();
        let d = out_geo(&out[0]).point_attribs.floats("dist").unwrap().to_vec();
        assert!((d[0] - 0.3).abs() < 1e-12 && (d[1] + 0.4).abs() < 1e-12 && d[2].abs() < 1e-12);
        let r = out_report(&out[1]);
        assert_eq!(r.metrics["reference_open_edges"].value, 4.0);
        assert!(r.notes.iter().any(|n| n.0 == Severity::Warning));
        let flipped = run(mesh_info, mesh_cook, vec![geo(cmp), geo(plate())], &[("flip", b(true))]).unwrap();
        let df = out_geo(&flipped[0]).point_attribs.floats("dist").unwrap();
        assert!((df[0] + 0.3).abs() < 1e-12 && (df[1] - 0.4).abs() < 1e-12);
        // colour: +0.3 is above green, -0.4 below it
        let cd = out_geo(&out[0]).point_attribs.vec3s("Cd").unwrap();
        assert!(cd[0].x > cd[1].x);
        assert!(cd[1].z > cd[0].z);
    }

    #[test]
    fn golden_10_cube_edge_sign_node() {
        let cube = Geometry::make_box(DVec3::splat(100.0));
        let cmp = Geometry::from_points(vec![DVec3::new(60.0, 60.0, 0.0), DVec3::ZERO]);
        let out = run(mesh_info, mesh_cook, vec![geo(cmp), geo(cube)], &[]).unwrap();
        let d = out_geo(&out[0]).point_attribs.floats("dist").unwrap();
        assert!((d[0] - 14.142136).abs() < 1e-5);
        assert!((d[1] + 50.0).abs() < 1e-9);
        let r = out_report(&out[1]);
        assert_eq!(r.metrics["reference_open_edges"].value, 0.0);
    }

    #[test]
    fn swapping_roles_flips_the_sign_of_a_displaced_sheet() {
        // mesh A below mesh B; distance of A's vertices to B is negative... of B's to A is positive
        let mut a = plate();
        let mut bgeo = plate();
        for p in a.points.iter_mut() {
            p.z -= 0.2;
        }
        for p in bgeo.points.iter_mut() {
            p.z += 0.2;
        }
        let ab = run(mesh_info, mesh_cook, vec![geo(a.clone()), geo(bgeo.clone())], &[]).unwrap();
        let ba = run(mesh_info, mesh_cook, vec![geo(bgeo), geo(a)], &[]).unwrap();
        let d1 = out_geo(&ab[0]).point_attribs.floats("dist").unwrap()[0];
        let d2 = out_geo(&ba[0]).point_attribs.floats("dist").unwrap()[0];
        assert!((d1 + 0.4).abs() < 1e-9 && (d2 - 0.4).abs() < 1e-9);
    }

    #[test]
    fn max_dist_flags_invalid_points() {
        let cmp = Geometry::from_points(vec![DVec3::new(0.0, 0.0, 1.0), DVec3::new(0.0, 0.0, 80.0)]);
        let out = run(mesh_info, mesh_cook, vec![geo(cmp), geo(plate())], &[("max_dist", f(5.0))]).unwrap();
        let o = out_geo(&out[0]);
        assert_eq!(o.point_attribs.ints("dist_valid").unwrap(), &[1, 0]);
        let cd = o.point_attribs.vec3s("Cd").unwrap();
        assert_eq!(cd[1], GREY);
    }

    #[test]
    fn unsigned_mode_and_errors() {
        let cmp = Geometry::from_points(vec![DVec3::new(0.0, 0.0, -2.0)]);
        let out = run(mesh_info, mesh_cook, vec![geo(cmp.clone()), geo(plate())], &[("signed", b(false))]).unwrap();
        assert!((out_geo(&out[0]).point_attribs.floats("dist").unwrap()[0] - 2.0).abs() < 1e-12);
        // reference without faces is a clean error
        assert!(run(mesh_info, mesh_cook, vec![geo(cmp), geo(Geometry::from_points(vec![DVec3::ZERO]))], &[]).is_err());
    }

    #[test]
    fn scales_to_a_few_hundred_thousand_points() {
        let mesh = sphere_geo(50.0, 96, 48);
        let mut rng = Rng::new(3);
        let pts: Vec<DVec3> = (0..200_000)
            .map(|_| {
                let d = DVec3::new(rng.next_f64() - 0.5, rng.next_f64() - 0.5, rng.next_f64() - 0.5).normalize_or_zero();
                d * (50.0 + (rng.next_f64() - 0.5) * 2.0)
            })
            .collect();
        let out = run(mesh_info, mesh_cook, vec![geo(Geometry::from_points(pts.clone())), geo(mesh)], &[]).unwrap();
        let d = out_geo(&out[0]).point_attribs.floats("dist").unwrap();
        // sign agrees with radius (the tessellation sags below 50 mm by <= 0.5 mm)
        let wrong = pts
            .iter()
            .zip(d)
            .filter(|(q, d)| (q.length() - 50.0).abs() > 0.6 && ((q.length() > 50.0) != (**d > 0.0)))
            .count();
        assert_eq!(wrong, 0);
    }

    #[test]
    #[ignore = "performance check: run with --release -- --ignored"]
    fn one_million_points_signed_distance() {
        let mesh = sphere_geo(50.0, 128, 64);
        let mut rng = Rng::new(4);
        let pts: Vec<DVec3> = (0..1_000_000)
            .map(|_| {
                let d = DVec3::new(rng.next_f64() - 0.5, rng.next_f64() - 0.5, rng.next_f64() - 0.5).normalize_or_zero();
                d * (50.0 + (rng.next_f64() - 0.5) * 4.0)
            })
            .collect();
        let t = std::time::Instant::now();
        let out = run(mesh_info, mesh_cook, vec![geo(Geometry::from_points(pts)), geo(mesh)], &[]).unwrap();
        eprintln!("1M signed distances in {:?}", t.elapsed());
        assert_eq!(out_geo(&out[0]).points.len(), 1_000_000);
    }
}

//! Mirror-plane detection and mirror completion for scans that cover one
//! side of a symmetric object better than the other.

use std::collections::HashMap;

use glam::{DMat3, DVec3};
use rayon::prelude::*;
use serde_json::{json, Value};
use vernon_core::{AttribArray, Geometry, Metric, Prim, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::kdtree::KdTree;
use crate::linalg::eigen3;
use crate::util::{add, ensure_metric, json_vec3, opt_json, partial_shuffle, pct, pf, pi, vec3_json, Rng, SPEC_ALIGN};

pub fn register(reg: &mut Registry) {
    add(reg, detect_info, detect_cook);
    add(reg, mirror_info, mirror_cook);
}

pub const PLANE_SCHEMA: &str = "plane.v1";

/// A plane `n . x = d` with unit normal.
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub n: DVec3,
    pub d: f64,
}

impl Plane {
    pub fn from_point_normal(o: DVec3, n: DVec3) -> Option<Plane> {
        let n = n.normalize_or_zero();
        (n != DVec3::ZERO).then(|| Plane { n, d: n.dot(o) })
    }
    #[inline]
    pub fn signed(&self, p: DVec3) -> f64 {
        self.n.dot(p) - self.d
    }
    #[inline]
    pub fn reflect(&self, p: DVec3) -> DVec3 {
        p - 2.0 * self.signed(p) * self.n
    }
    pub fn origin(&self) -> DVec3 {
        self.n * self.d
    }
    pub fn to_json(self) -> Value {
        json!({"schema": PLANE_SCHEMA, "origin": vec3_json(self.origin()), "normal": vec3_json(self.n)})
    }
    pub fn from_json(v: &Value) -> Option<Plane> {
        Plane::from_point_normal(json_vec3(v.get("origin")?)?, json_vec3(v.get("normal")?)?)
    }
}

fn plane_out() -> PortSpec {
    PortSpec::new("plane", DataKind::Json)
}
fn geo_out() -> PortSpec {
    PortSpec::new("geometry", DataKind::Geometry)
}
fn rep_out() -> PortSpec {
    PortSpec::new("report", DataKind::Report)
}

// --------------------------------------------------------------- detection

/// Distances from the mirror image of each sample point to the nearest
/// sample point.
fn mirror_distances(plane: &Plane, probe: &[DVec3], tree: &KdTree) -> Vec<(f64, u32)> {
    probe
        .par_iter()
        .map(|p| tree.nearest(plane.reflect(*p)).map_or((f64::INFINITY, u32::MAX), |(i, d2)| (d2.sqrt(), i)))
        .collect()
}

fn trimmed_rms(d: &[(f64, u32)], keep: f64) -> f64 {
    let mut v: Vec<f64> = d.iter().map(|x| x.0).filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return f64::INFINITY;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = ((v.len() as f64 * keep).ceil() as usize).clamp(1, v.len());
    (v[..n].iter().map(|x| x * x).sum::<f64>() / n as f64).sqrt()
}

/// One refinement step: the pairs (p, nearest image) define the plane as
/// their perpendicular bisector. Pairs are weighted by how well they match
/// (Welsch weights on the mismatch distance) so that the orphans of a
/// missing region do not tilt the plane.
fn refine_plane(plane: &Plane, probe: &[DVec3], tree: &KdTree, sample: &[DVec3], keep: f64) -> Option<Plane> {
    let d = mirror_distances(plane, probe, tree);
    let mut order: Vec<usize> = (0..probe.len()).filter(|&i| d[i].0.is_finite()).collect();
    order.sort_by(|&a, &b| d[a].0.total_cmp(&d[b].0));
    if order.len() < 3 {
        return None;
    }
    let n = ((order.len() as f64 * keep).ceil() as usize).clamp(3, order.len());
    let median = d[order[order.len() / 2]].0;
    let sigma = (1.5 * median).max(1e-9);
    let mut a = [[0.0f64; 3]; 3];
    let mut pairs: Vec<(DVec3, DVec3, f64)> = Vec::with_capacity(n);
    for &i in order.iter().take(n) {
        let (p, q) = (probe[i], sample[d[i].1 as usize]);
        // the partner of p is q, so the plane normal is parallel to p - q
        let v = p - q;
        if v.length() < 1e-9 {
            continue;
        }
        let w = (-(d[i].0 / sigma).powi(2)).exp();
        for r in 0..3 {
            for c in 0..3 {
                a[r][c] += w * v[r] * v[c];
            }
        }
        pairs.push((p, q, w));
    }
    if pairs.len() < 3 {
        return None;
    }
    let m = DMat3::from_cols(DVec3::new(a[0][0], a[1][0], a[2][0]), DVec3::new(a[0][1], a[1][1], a[2][1]), DVec3::new(a[0][2], a[1][2], a[2][2]));
    let (vals, vecs) = eigen3(&m);
    if vals[0] <= 0.0 {
        return None;
    }
    let mut nn = vecs[0];
    if nn.dot(plane.n) < 0.0 {
        nn = -nn;
    }
    // weight the offset by the pair separation so nearly on-plane pairs do not dominate
    let (mut num, mut den) = (0.0, 0.0);
    for (p, q, wt) in &pairs {
        let w = wt * (p - q).length_squared();
        num += w * nn.dot((*p + *q) * 0.5);
        den += w;
    }
    Some(Plane { n: nn, d: num / den })
}

/// Best offset of a plane with fixed normal: coarse scan then fine scan of
/// the trimmed mirror error.
fn best_offset(n: DVec3, c: DVec3, extent: f64, probe: &[DVec3], tree: &KdTree, keep: f64) -> Plane {
    let d0 = n.dot(c);
    let score = |d: f64| trimmed_rms(&mirror_distances(&Plane { n, d }, probe, tree), keep);
    let (mut best_d, mut best_s) = (d0, score(d0));
    let coarse = extent * 0.15;
    for k in -20..=20 {
        let d = d0 + coarse * k as f64 / 20.0;
        let s = score(d);
        if s < best_s {
            best_s = s;
            best_d = d;
        }
    }
    let step = coarse / 20.0;
    let mid = best_d;
    for k in -10..=10 {
        let d = mid + step * k as f64 / 10.0;
        let s = score(d);
        if s < best_s {
            best_s = s;
            best_d = d;
        }
    }
    Plane { n, d: best_d }
}

pub struct Detection {
    pub plane: Plane,
    pub rms: f64,
    pub second_best_rms: f64,
    pub distances: Vec<f64>,
}

/// Find the dominant mirror plane of a point set.
pub fn detect_plane(points: &[DVec3], samples: usize, axis_hint: &str, iterations: usize, keep: f64, seed: u64) -> Option<Detection> {
    if points.len() < 8 {
        return None;
    }
    let mut rng = Rng::new(seed);
    let mut idx: Vec<u32> = (0..points.len() as u32).collect();
    let m = samples.min(points.len());
    partial_shuffle(&mut idx, m, &mut rng);
    let sample: Vec<DVec3> = idx[..m].iter().map(|&i| points[i as usize]).collect();
    let probe: Vec<DVec3> = sample.iter().take(5000).cloned().collect();
    let tree = KdTree::new(&sample);
    let c = sample.iter().sum::<DVec3>() / sample.len() as f64;
    let extent = {
        let mut bb = vernon_core::Aabb::empty();
        for q in &sample {
            bb.grow(*q);
        }
        bb.size().length()
    };
    let mut cov = [[0.0f64; 3]; 3];
    for p in &sample {
        let d = *p - c;
        for r in 0..3 {
            for k in 0..3 {
                cov[r][k] += d[r] * d[k];
            }
        }
    }
    let cm = DMat3::from_cols(DVec3::new(cov[0][0], cov[1][0], cov[2][0]), DVec3::new(cov[0][1], cov[1][1], cov[2][1]), DVec3::new(cov[0][2], cov[1][2], cov[2][2]));
    let (_, axes) = eigen3(&cm);
    let mut cands: Vec<DVec3> = match axis_hint {
        "x" => vec![DVec3::X],
        "y" => vec![DVec3::Y],
        "z" => vec![DVec3::Z],
        _ => {
            let mut v = vec![axes[0], axes[1], axes[2], DVec3::X, DVec3::Y, DVec3::Z];
            v.dedup_by(|a, b| a.dot(*b).abs() > 0.9999);
            v
        }
    };
    // drop duplicates that dedup_by could not see (non-adjacent)
    let mut uniq: Vec<DVec3> = Vec::new();
    for n in cands.drain(..) {
        if !uniq.iter().any(|u| u.dot(n).abs() > 0.9999) {
            uniq.push(n);
        }
    }
    let mut scored: Vec<(f64, Plane)> = uniq
        .into_iter()
        .filter(|n| n.length_squared() > 0.0)
        .map(|n| {
            let pl = best_offset(n.normalize(), c, extent, &probe, &tree, keep);
            (trimmed_rms(&mirror_distances(&pl, &probe, &tree), keep), pl)
        })
        .collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut best = scored.first()?.1;
    for _ in 0..iterations {
        let Some(next) = refine_plane(&best, &probe, &tree, &sample, keep) else { break };
        let moved = next.n.dot(best.n).clamp(-1.0, 1.0).acos() + (next.d - best.d).abs();
        best = next;
        if moved < 1e-9 {
            break;
        }
    }
    let d = mirror_distances(&best, &sample, &tree);
    let rms = trimmed_rms(&d, keep);
    // the refined plane competes with the best raw candidate
    let second = scored.get(1).map_or(f64::INFINITY, |s| s.0);
    Some(Detection { plane: best, rms, second_best_rms: second, distances: d.iter().map(|x| x.0).collect() })
}

fn detect_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.symmetry_detect", "Find Mirror Plane", Stage::Clean)
        .input(PortSpec::new("geometry", DataKind::Geometry))
        .output(geo_out())
        .output(plane_out())
        .output(rep_out())
        .param(ParamSpec::int("samples", "Sample points", 20000, 100, 1_000_000).help("Number of points used to search for the plane."))
        .param(ParamSpec::choice("axis_hint", "Search", "auto", &["auto", "x", "y", "z"]).help("auto: try the principal axes and the coordinate axes. x, y or z: only planes near that axis."))
        .param(ParamSpec::int("refine_iterations", "Refinement steps", 20, 0, 200).help("Rounds that tilt and shift the plane to fit the matched point pairs."))
        .param(
            ParamSpec::float("tolerance", "Match distance", 1.0, 0.001, 1000.0)
                .unit("mm")
                .help("A point counts as mirrored when its image lies this close to another point."),
        )
        .param(ParamSpec::float("trim", "Trimmed fraction", 0.9, 0.5, 1.0).help("Share of the best matches used for fitting and the error figure; the rest is treated as missing or one-sided data."))
        .param(ParamSpec::float("min_fraction", "Required symmetry", 0.8, 0.0, 1.0).help("Share of points that must have a mirror partner; below it the report warns."))
        .param(ParamSpec::int("seed", "Seed", 1, 0, 1_000_000).help("Seed of the random subsample."))
        .help("Finds the plane that best maps the point set onto itself and reports how symmetric the data is. Writes the point attribute 'mirror_dist'.")
        .emulates("mirror-plane search by principal axes plus iterative closest-point refinement of the reflection")
        .spec(SPEC_ALIGN)
        .implemented()
}

fn detect_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    ensure_metric(ctx, &g)?;
    let samples = pi(p, "samples", 100, 1_000_000)? as usize;
    let hint = p.str("axis_hint")?.to_string();
    let iters = pi(p, "refine_iterations", 0, 200)? as usize;
    let tol = pf(p, "tolerance", 0.001, 1000.0)?;
    let keep = pf(p, "trim", 0.5, 1.0)?;
    let min_frac = pf(p, "min_fraction", 0.0, 1.0)?;
    let seed = pi(p, "seed", 0, 1_000_000)? as u64;
    let det = detect_plane(&g.points, samples, &hint, iters, keep, seed).ok_or_else(|| ctx.fail("need at least 8 points to look for symmetry"))?;
    let pl = det.plane;

    // per-point attribute on the full input
    let tree = KdTree::new(&g.points);
    let md: Vec<f64> = g.points.par_iter().map(|q| tree.nearest(pl.reflect(*q)).map_or(f64::INFINITY, |(_, d2)| d2.sqrt())).collect();
    let mut out = (*g).clone();
    out.point_attribs.set("mirror_dist", AttribArray::Float(md.clone()));

    let within = md.iter().filter(|&&d| d <= tol).count();
    let frac = within as f64 / md.len().max(1) as f64;
    let mut sorted: Vec<f64> = md.iter().cloned().filter(|d| d.is_finite()).collect();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let q = |f: f64| if sorted.is_empty() { 0.0 } else { sorted[((sorted.len() as f64 * f).ceil() as usize).clamp(1, sorted.len()) - 1] };
    let mut rep = Report::new("Find Mirror Plane");
    rep.metric("mirror_rms", Metric::new(det.rms, "mm"));
    rep.metric("mirror_p50", Metric::new(q(0.5), "mm"));
    rep.metric("mirror_p95", Metric::new(q(0.95), "mm"));
    rep.metric("symmetric_fraction", Metric::new(frac, "ratio").with_limit(min_frac, false));
    rep.metric("tolerance", Metric::new(tol, "mm"));
    rep.metric("normal_x", Metric::new(pl.n.x, "ratio"));
    rep.metric("normal_y", Metric::new(pl.n.y, "ratio"));
    rep.metric("normal_z", Metric::new(pl.n.z, "ratio"));
    let o = pl.origin();
    rep.metric("origin_x", Metric::new(o.x, "mm"));
    rep.metric("origin_y", Metric::new(o.y, "mm"));
    rep.metric("origin_z", Metric::new(o.z, "mm"));
    rep.metric("points", Metric::new(g.points.len() as f64, "points"));
    if frac < min_frac {
        rep.note(Severity::Warning, format!("only {:.1} % of the points have a mirror partner within {tol} mm; the object may not be symmetric or the data is one-sided", 100.0 * frac));
    }
    if det.second_best_rms.is_finite() && det.second_best_rms < det.rms * 1.2 && det.rms > tol {
        rep.note(Severity::Warning, "another candidate plane fits almost as well; the result may be ambiguous");
    }
    Ok(vec![Data::geometry(out), Data::json(pl.to_json()), Data::report(rep)])
}

// --------------------------------------------------------- mirror complete

fn mirror_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.mirror_complete", "Mirror Complete", Stage::Clean)
        .input(PortSpec::new("geometry", DataKind::Geometry))
        .input(PortSpec::optional("plane", DataKind::Json))
        .output(geo_out())
        .output(rep_out())
        .param(ParamSpec::choice("plane_source", "Plane", "axis", &["axis", "custom"]).help("Used when no plane is connected. axis: the middle of the bounding box across an axis. custom: the origin and normal below."))
        .param(ParamSpec::choice("axis", "Axis", "x", &["x", "y", "z"]).help("Axis across which the bounding-box mid-plane mirrors."))
        .param(ParamSpec::vec3("origin", "Plane origin", glam::DVec3::ZERO).unit("mm").help("A point on the plane in custom mode."))
        .param(ParamSpec::vec3("normal", "Plane normal", glam::DVec3::X).help("Plane normal in custom mode."))
        .param(
            ParamSpec::choice("mode", "Mode", "fill_gaps", &["fill_gaps", "replace_side"])
                .help("fill_gaps: keep everything and add mirrored data only where the other side is empty. replace_side: keep one side and replace the other by its mirror image."),
        )
        .param(ParamSpec::choice("source_side", "Source side", "auto", &["auto", "positive", "negative"]).help("The side that is copied. auto: the side with more points."))
        .param(
            ParamSpec::float("gap_radius", "Gap radius", 0.0, 0.0, 10000.0)
                .unit("mm")
                .help("fill_gaps adds a mirrored point or face only when no existing point lies within this distance. 0 = twice the typical point spacing."),
        )
        .param(
            ParamSpec::float("weld_distance", "Plane weld distance", 0.01, 0.0, 100.0)
                .unit("mm")
                .help("Points this close to the plane count as lying on it and are not duplicated."),
        )
        .help("Completes a one-sided scan or half model by reflecting it across a mirror plane. Faces are copied with reversed winding.")
        .emulates("symmetry-based completion: mirror across a plane and merge on the plane")
        .spec(SPEC_ALIGN)
        .implemented()
}

fn nn_spacing(points: &[DVec3]) -> f64 {
    if points.len() < 2 {
        return 0.0;
    }
    let tree = KdTree::new(points);
    let step = (points.len() / 5000).max(1);
    let mut d: Vec<f64> = (0..points.len())
        .step_by(step)
        .collect::<Vec<_>>()
        .par_iter()
        .map_init(Vec::new, |buf, &i| {
            tree.knn(points[i], 2, buf);
            buf.iter().find(|(_, j)| *j as usize != i).map_or(0.0, |(d2, _)| d2.sqrt())
        })
        .collect();
    d.sort_by(|a, b| a.total_cmp(b));
    d[d.len() / 2]
}

fn reflect_vec3_rows(t: &mut vernon_core::AttribTable, from: usize, pl: &Plane) {
    if let Some(AttribArray::Vec3(ns)) = t.get_mut("N") {
        for n in ns.iter_mut().skip(from) {
            *n -= 2.0 * pl.n.dot(*n) * pl.n;
        }
    }
}

fn mirror_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    ensure_metric(ctx, &g)?;
    if g.points.is_empty() {
        return Err(ctx.fail("input is empty"));
    }
    let bounds = g.bounds();
    let plane = if let Some(j) = opt_json(inp, 1)? {
        Plane::from_json(&j).ok_or_else(|| ctx.fail("the plane input is not a plane.v1 record with origin and normal"))?
    } else if p.str("plane_source")? == "custom" {
        Plane::from_point_normal(p.vec3("origin")?, p.vec3("normal")?).ok_or_else(|| ctx.fail("the plane normal must not be zero"))?
    } else {
        let n = match p.str("axis")? {
            "y" => DVec3::Y,
            "z" => DVec3::Z,
            _ => DVec3::X,
        };
        Plane::from_point_normal(bounds.center(), n).ok_or_else(|| ctx.fail("internal error: zero axis"))?
    };
    let fill = p.str("mode")? == "fill_gaps";
    let weld = pf(p, "weld_distance", 0.0, 100.0)?;
    let mut gap = pf(p, "gap_radius", 0.0, 10000.0)?;
    let side_param = p.str("source_side")?.to_string();

    let face_ok = |i: usize| g.prims[i].closed && g.prims[i].vertices.len() >= 3;
    let has_faces = (0..g.prims.len()).any(face_ok);
    // elements: faces when present, points otherwise
    let elem_pts: Vec<DVec3> = if has_faces {
        g.prims
            .iter()
            .map(|q| if q.vertices.is_empty() { DVec3::ZERO } else { q.vertices.iter().map(|&v| g.points[v as usize]).sum::<DVec3>() / q.vertices.len() as f64 })
            .collect()
    } else {
        g.points.clone()
    };
    let (mut pos_n, mut neg_n) = (0usize, 0usize);
    for (i, e) in elem_pts.iter().enumerate() {
        if has_faces && !face_ok(i) {
            continue;
        }
        let s = plane.signed(*e);
        if s > weld {
            pos_n += 1;
        } else if s < -weld {
            neg_n += 1;
        }
    }
    let positive = match side_param.as_str() {
        "positive" => true,
        "negative" => false,
        _ => pos_n >= neg_n,
    };
    let sgn = if positive { 1.0 } else { -1.0 };
    let on_source = |e: DVec3| sgn * plane.signed(e) > weld;
    if gap <= 0.0 {
        gap = 2.0 * nn_spacing(&g.points);
    }
    let tree = KdTree::new(&g.points);
    let gap2 = gap * gap;
    let empty_near = |q: DVec3| tree.nearest_within(q, gap2).is_none();
    let unit = if has_faces { "faces" } else { "points" };

    let mut rep = Report::new("Mirror Complete");
    rep.metric("elements_positive_side", Metric::new(pos_n as f64, unit));
    rep.metric("elements_negative_side", Metric::new(neg_n as f64, unit));
    let src_count = if positive { pos_n } else { neg_n };
    if src_count == 0 {
        return Err(ctx.fail("the chosen source side has no data; check the plane and side"));
    }
    if side_param == "auto" && !fill && pos_n.min(neg_n) as f64 > 0.5 * pos_n.max(neg_n) as f64 {
        rep.note(Severity::Warning, "both sides hold a similar amount of data; replace_side discards the smaller one");
    }

    let mut out = if !has_faces {
        // ---- point cloud
        let src: Vec<usize> = (0..g.points.len()).filter(|&i| on_source(g.points[i])).collect();
        let base: Vec<usize> = if fill {
            (0..g.points.len()).collect()
        } else {
            (0..g.points.len()).filter(|&i| on_source(g.points[i]) || plane.signed(g.points[i]).abs() <= weld).collect()
        };
        let add_idx: Vec<usize> = src.iter().copied().filter(|&i| !fill || empty_near(plane.reflect(g.points[i]))).collect();
        let mut o = crate::util::gather_points(&g, &base);
        let mut m = crate::util::gather_points(&g, &add_idx);
        for q in &mut m.points {
            *q = plane.reflect(*q);
        }
        reflect_vec3_rows(&mut m.point_attribs, 0, &plane);
        o.merge(&m);
        rep.metric("points_added", Metric::new(add_idx.len() as f64, "points"));
        o
    } else {
        // ---- mesh
        let face_src: Vec<usize> = (0..g.prims.len()).filter(|&i| face_ok(i) && on_source(elem_pts[i])).collect();
        let crossing = (0..g.prims.len())
            .filter(|&i| face_ok(i))
            .filter(|&i| {
                let s: Vec<f64> = g.prims[i].vertices.iter().map(|&v| plane.signed(g.points[v as usize])).collect();
                s.iter().any(|&x| x > weld) && s.iter().any(|&x| x < -weld)
            })
            .count();
        if crossing > 0 {
            rep.note(Severity::Warning, format!("{crossing} face(s) cross the plane and are kept uncut"));
        }
        let mut o = Geometry::new();
        let map: Vec<u32>;
        let mut src_pts: Vec<usize>;
        let mut src_prims: Vec<usize>;
        if fill {
            o.points = g.points.clone();
            o.prims = g.prims.clone();
            map = (0..g.points.len() as u32).collect();
            src_pts = (0..g.points.len()).collect();
            src_prims = (0..g.prims.len()).collect();
        } else {
            let mut m = vec![u32::MAX; g.points.len()];
            src_pts = Vec::new();
            src_prims = Vec::new();
            for &fi in &face_src {
                let mut nv = Vec::with_capacity(g.prims[fi].vertices.len());
                for &v in &g.prims[fi].vertices {
                    if m[v as usize] == u32::MAX {
                        m[v as usize] = o.points.len() as u32;
                        let q = g.points[v as usize];
                        // points on the plane are snapped exactly onto it
                        o.points.push(if plane.signed(q).abs() <= weld { q - plane.signed(q) * plane.n } else { q });
                        src_pts.push(v as usize);
                    }
                    nv.push(m[v as usize]);
                }
                o.prims.push(Prim { vertices: nv, closed: true });
                src_prims.push(fi);
            }
            map = m;
        }
        let (base_pts, base_prims) = (o.points.len(), o.prims.len());
        let mirrored: Vec<usize> = face_src.iter().copied().filter(|&fi| !fill || empty_near(plane.reflect(elem_pts[fi]))).collect();
        let mut mmap: HashMap<u32, u32> = HashMap::new();
        for &fi in &mirrored {
            let mut nv: Vec<u32> = Vec::with_capacity(g.prims[fi].vertices.len());
            for &v in &g.prims[fi].vertices {
                let id = match mmap.get(&v) {
                    Some(&x) => x,
                    None => {
                        let q = g.points[v as usize];
                        let id = if plane.signed(q).abs() <= weld && map[v as usize] != u32::MAX {
                            map[v as usize]
                        } else {
                            o.points.push(plane.reflect(q));
                            src_pts.push(v as usize);
                            (o.points.len() - 1) as u32
                        };
                        mmap.insert(v, id);
                        id
                    }
                };
                nv.push(id);
            }
            nv.reverse();
            let mut s = nv.clone();
            s.sort_unstable();
            s.dedup();
            if s.len() >= 3 {
                o.prims.push(Prim { vertices: nv, closed: true });
                src_prims.push(fi);
            }
        }
        o.point_attribs = g.point_attribs.gather_all(&src_pts);
        o.prim_attribs = g.prim_attribs.gather_all(&src_prims);
        reflect_vec3_rows(&mut o.point_attribs, base_pts, &plane);
        reflect_vec3_rows(&mut o.prim_attribs, base_prims, &plane);
        rep.metric("faces_added", Metric::new((o.prims.len() - base_prims) as f64, "faces"));
        o
    };
    out.detail_attribs = g.detail_attribs.clone();
    rep.metric("points_in", Metric::new(g.points.len() as f64, "points"));
    rep.metric("points_out", Metric::new(out.points.len() as f64, "points"));
    rep.metric("gap_radius", Metric::new(gap, "mm"));
    rep.metric("source_fraction", Metric::new(pct(src_count, pos_n + neg_n), "%"));
    if let Err(e) = out.validate() {
        return Err(ctx.fail(format!("internal error: {e}")));
    }
    Ok(vec![Data::geometry(out), Data::report(rep)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meshedit::edge_stats;
    use crate::testkit::*;
    use vernon_graph::ParamValue;

    /// Body with exactly one mirror plane (y = 0 before the pose is applied):
    /// shear in x/z breaks the other two symmetries.
    fn body(n_pairs: usize, seed: u64) -> Vec<DVec3> {
        let mut rng = Rng::new(seed);
        let mut pts = Vec::new();
        while pts.len() < 2 * n_pairs {
            let d = DVec3::new(rng.next_f64() * 2.0 - 1.0, rng.next_f64() * 2.0 - 1.0, rng.next_f64() * 2.0 - 1.0);
            if d.length() < 0.1 || d.length() > 1.0 {
                continue;
            }
            let u = d.normalize();
            let make = |u: DVec3| DVec3::new(30.0 * u.x + 5.0 * u.z, 18.0 * u.y * (1.0 + 0.3 * u.x), 12.0 * u.z + 4.0 * u.x * u.x);
            pts.push(make(u));
            pts.push(make(DVec3::new(u.x, -u.y, u.z)));
        }
        pts
    }

    #[test]
    fn finds_the_single_plane_of_a_posed_body() {
        let rot = glam::DMat3::from_axis_angle(DVec3::new(1.0, 2.0, 3.0).normalize(), 0.7);
        let t = DVec3::new(100.0, -50.0, 20.0);
        let pts: Vec<DVec3> = body(5000, 7).into_iter().map(|p| rot * p + t).collect();
        let out = run(detect_info, detect_cook, vec![geo(Geometry::from_points(pts))], &[]).unwrap();
        let pl = Plane::from_json(out_json(&out[1])).unwrap();
        let expect = rot * DVec3::Y;
        assert!(pl.n.dot(expect).abs() > (0.2f64).to_radians().cos(), "normal {:?} vs {:?}", pl.n, expect);
        assert!(pl.signed(t).abs() < 0.1, "offset {}", pl.signed(t));
        let r = out_report(&out[2]);
        assert!(r.metrics["symmetric_fraction"].value > 0.95);
        assert!(r.metrics["mirror_rms"].value < 0.1);
        assert!(r.passes());
        assert!(out_geo(&out[0]).point_attribs.floats("mirror_dist").is_ok());
    }

    #[test]
    fn tolerates_a_missing_patch_and_recovers_the_plane() {
        let mut pts = body(5000, 11);
        // knock out one lobe on the -y side only
        pts.retain(|p| !(p.y < -5.0 && p.x > 10.0));
        let out = run(detect_info, detect_cook, vec![geo(Geometry::from_points(pts))], &[("axis_hint", s("y"))]).unwrap();
        let pl = Plane::from_json(out_json(&out[1])).unwrap();
        assert!(pl.n.dot(DVec3::Y).abs() > (1.0f64).to_radians().cos(), "normal {:?}", pl.n);
        assert!(pl.signed(DVec3::ZERO).abs() < 0.5);
    }

    #[test]
    fn asymmetric_data_is_flagged() {
        let mut rng = Rng::new(2);
        let pts: Vec<DVec3> = (0..3000).map(|_| DVec3::new(rng.next_f64() * 80.0, rng.next_f64() * 40.0, rng.next_f64() * 10.0 + rng.next_f64() * rng.next_f64() * 30.0)).collect();
        let out = run(detect_info, detect_cook, vec![geo(Geometry::from_points(pts))], &[("tolerance", f(0.05)), ("min_fraction", f(0.9))]).unwrap();
        let r = out_report(&out[2]);
        assert!(!r.passes());
        assert!(r.notes.iter().any(|n| n.0 == Severity::Warning));
        assert!(run(detect_info, detect_cook, vec![geo(Geometry::from_points(vec![DVec3::ZERO; 3]))], &[]).is_err());
    }

    #[test]
    fn fill_gaps_adds_only_the_missing_patch() {
        let full = body(6000, 5); // symmetric about y = 0
        let removed: Vec<DVec3> = full.iter().cloned().filter(|p| p.y < -3.0 && p.x > 8.0).collect();
        let kept: Vec<DVec3> = full.iter().cloned().filter(|p| !(p.y < -3.0 && p.x > 8.0)).collect();
        assert!(removed.len() > 200);
        let out = run(
            mirror_info,
            mirror_cook,
            vec![geo(Geometry::from_points(kept.clone())), None],
            &[("plane_source", s("custom")), ("origin", ParamValue::Vec3(DVec3::ZERO)), ("normal", ParamValue::Vec3(DVec3::Y)), ("source_side", s("positive")), ("gap_radius", f(1.5)), ("weld_distance", f(0.001))],
        )
        .unwrap();
        let o = out_geo(&out[0]);
        let r = out_report(&out[1]);
        let added = r.metrics["points_added"].value as usize;
        assert_eq!(o.points.len(), kept.len() + added);
        // added points land in the removed patch (within the gap radius of its edge)
        for q in &o.points[kept.len()..] {
            assert!(q.y < 0.0);
            assert!(q.x > 8.0 - 2.0 && q.y < -3.0 + 2.0, "{q:?}");
        }
        assert!((added as f64 - removed.len() as f64).abs() / (removed.len() as f64) < 0.35, "added {added} vs removed {}", removed.len());
    }

    #[test]
    fn replace_side_completes_a_half_sphere_mesh() {
        let mut half = sphere_geo(40.0, 40, 40);
        let cen: Vec<f64> = half.prims.iter().map(|p| p.vertices.iter().map(|&v| half.points[v as usize].z).sum::<f64>() / 3.0).collect();
        let keep: Vec<_> = half.prims.iter().enumerate().filter(|(i, _)| cen[*i] > 1e-6).map(|(_, p)| p.clone()).collect();
        half.prims = keep;
        let before = half.prims.len();
        let out = run(
            mirror_info,
            mirror_cook,
            vec![geo(half), None],
            &[("mode", s("replace_side")), ("plane_source", s("custom")), ("origin", ParamValue::Vec3(DVec3::ZERO)), ("normal", ParamValue::Vec3(DVec3::Z))],
        )
        .unwrap();
        let o = out_geo(&out[0]);
        assert_eq!(o.prims.len(), 2 * before);
        let tris: Vec<[u32; 3]> = o.triangles().into_iter().map(|t| t.1).collect();
        let st = edge_stats(&tris);
        assert_eq!((st.open_edges, st.nonmanifold_edges, st.inconsistent_edges), (0, 0, 0));
        let vol: f64 = crate::meshedit::signed_volume(&o.points, &tris);
        let sphere = 4.0 / 3.0 * std::f64::consts::PI * 40.0f64.powi(3);
        assert!((vol - sphere).abs() / sphere < 0.01, "{vol} vs {sphere}");
        assert!(o.validate().is_ok());
    }

    #[test]
    fn plane_input_overrides_and_bad_plane_fails() {
        let g = Geometry::from_points((0..50).map(|i| DVec3::new(i as f64, 1.0, 0.0)).chain((0..10).map(|i| DVec3::new(i as f64, -1.0, 0.0))).collect());
        let pl = Plane::from_point_normal(DVec3::ZERO, DVec3::Y).unwrap().to_json();
        let out = run(mirror_info, mirror_cook, vec![geo(g.clone()), json(pl)], &[("mode", s("replace_side")), ("weld_distance", f(0.1))]).unwrap();
        // 50 points on +y plus their 50 images; the 10 on -y are replaced
        assert_eq!(out_geo(&out[0]).points.len(), 100);
        assert!(run(mirror_info, mirror_cook, vec![geo(g), json(serde_json::json!({"origin": [0, 0, 0]}))], &[]).is_err());
    }
}

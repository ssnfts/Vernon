//! Point-cloud clean-up nodes: statistical and radius outlier removal, voxel
//! and random subsampling, light denoising and normal estimation.

use glam::DVec3;
use rayon::prelude::*;
use vernon_core::{AttribArray, AttribTable, Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::kdtree::KdTree;
use crate::linalg::eigen3;
use crate::util::{add, ensure_metric, gather_points, partial_shuffle, pct, pf, pi, Rng, SPEC_CAPTURE};

pub fn register(reg: &mut Registry) {
    add(reg, outlier_info, outlier_cook);
    add(reg, radius_info, radius_cook);
    add(reg, voxel_info, voxel_cook);
    add(reg, random_info, random_cook);
    add(reg, denoise_info, denoise_cook);
    add(reg, normals_info, normals_cook);
}

fn points_in() -> PortSpec {
    PortSpec::new("points", DataKind::Geometry)
}

fn geo_out() -> PortSpec {
    PortSpec::new("geometry", DataKind::Geometry)
}

fn rep_out() -> PortSpec {
    PortSpec::new("report", DataKind::Report)
}

fn count_report(title: &str, before: usize, after: usize) -> Report {
    let mut r = Report::new(title);
    r.metric("input_points", Metric::new(before as f64, "points"));
    r.metric("kept_points", Metric::new(after as f64, "points"));
    r.metric("removed_points", Metric::new((before - after) as f64, "points"));
    r.metric("removed_fraction", Metric::new(pct(before - after, before), "%"));
    if before == after {
        r.note(Severity::Info, "no points were removed");
    }
    r
}

// ---------------------------------------------------------------- outliers

fn outlier_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.outlier_remove", "Remove Outliers", Stage::Clean)
        .input(points_in())
        .output(geo_out())
        .output(rep_out())
        .param(
            ParamSpec::int("k", "Neighbours", 16, 3, 200).help("Number of nearest neighbours averaged per point."),
        )
        .param(
            ParamSpec::float("sigma", "Std-dev multiplier", 2.0, 0.5, 10.0)
                .help("Points whose mean neighbour distance exceeds mean + sigma * std are outliers."),
        )
        .param(
            ParamSpec::choice("mode", "Mode", "remove", &["remove", "flag"])
                .help("remove: delete outliers. flag: keep everything and write the point attribute 'outlier'."),
        )
        .help("Statistical outlier removal: compares each point's mean distance to its nearest neighbours with the global distribution.")
        .emulates("statistical outlier removal on k-NN mean distance (Rusu et al. 2008), as in point-cloud cleaning tools")
        .spec(SPEC_CAPTURE)
        .implemented()
}

fn outlier_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    ensure_metric(ctx, &g)?;
    let k = pi(p, "k", 3, 200)? as usize;
    let sigma = pf(p, "sigma", 0.5, 10.0)?;
    let flag = p.str("mode")? == "flag";
    let n = g.points.len();
    if n <= k {
        return Err(ctx.fail(format!("need more than k = {k} points, got {n}")));
    }
    let tree = KdTree::new(&g.points);
    let md: Vec<f64> = g
        .points
        .par_iter()
        .enumerate()
        .map_init(Vec::new, |buf, (i, q)| {
            tree.knn(*q, k + 1, buf);
            let (mut sum, mut cnt) = (0.0, 0usize);
            for &(d2, j) in buf.iter() {
                if j as usize == i {
                    continue;
                }
                if cnt == k {
                    break;
                }
                sum += d2.sqrt();
                cnt += 1;
            }
            if cnt == 0 {
                0.0
            } else {
                sum / cnt as f64
            }
        })
        .collect();
    let mu = md.iter().sum::<f64>() / n as f64;
    let var = md.iter().map(|d| (d - mu) * (d - mu)).sum::<f64>() / n as f64;
    let thr = mu + sigma * var.sqrt();
    let keep: Vec<usize> = (0..n).filter(|&i| md[i] <= thr).collect();
    let mut rep = count_report("Remove Outliers", n, keep.len());
    rep.metric("threshold", Metric::new(thr, "mm"));
    rep.metric("mean_neighbour_distance", Metric::new(mu, "mm"));
    if keep.len() * 2 < n {
        rep.note(Severity::Warning, "more than half of the points were classified as outliers; check k and sigma");
    }
    let out = if flag {
        let mut o = (*g).clone();
        let mut f = vec![1i64; n];
        for &i in &keep {
            f[i] = 0;
        }
        o.point_attribs.set("outlier", AttribArray::Int(f));
        o
    } else {
        gather_points(&g, &keep)
    };
    Ok(vec![Data::geometry(out), Data::report(rep)])
}

fn radius_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.radius_filter", "Remove Isolated Points", Stage::Clean)
        .input(points_in())
        .output(geo_out())
        .output(rep_out())
        .param(
            ParamSpec::float("radius", "Radius", 2.0, 0.001, 10000.0)
                .unit("mm")
                .help("Search radius around each point."),
        )
        .param(
            ParamSpec::int("min_neighbors", "Minimum neighbours", 4, 1, 1000)
                .help("Points with fewer other points inside the radius are removed."),
        )
        .help("Removes points that have too few neighbours within a fixed radius (floating specks and stray returns).")
        .emulates("radius outlier removal")
        .spec(SPEC_CAPTURE)
        .implemented()
}

fn radius_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    ensure_metric(ctx, &g)?;
    let r = pf(p, "radius", 0.001, 10000.0)?;
    let m = pi(p, "min_neighbors", 1, 1000)? as usize;
    if g.points.is_empty() {
        return Err(ctx.fail("input has no points"));
    }
    let tree = KdTree::new(&g.points);
    let keep: Vec<usize> = (0..g.points.len())
        .into_par_iter()
        .filter(|&i| tree.radius_count(g.points[i], r, m + 1) > m)
        .collect();
    let mut rep = count_report("Remove Isolated Points", g.points.len(), keep.len());
    rep.metric("radius_used", Metric::new(r, "mm"));
    Ok(vec![Data::geometry(gather_points(&g, &keep)), Data::report(rep)])
}

// --------------------------------------------------------------- subsample

fn voxel_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.voxel_subsample", "Voxel Subsample", Stage::Clean)
        .input(points_in())
        .output(geo_out())
        .output(rep_out())
        .param(
            ParamSpec::float("voxel_size", "Voxel size", 1.0, 0.001, 10000.0)
                .unit("mm")
                .help("Edge length of the grid cells; one point survives per occupied cell."),
        )
        .param(
            ParamSpec::choice(
                "mode",
                "Representative",
                "nearest_to_centroid",
                &["centroid", "nearest_to_centroid", "first"],
            )
            .help("centroid averages position and float/vector attributes; the other modes keep an original point."),
        )
        .help("Uniformly thins a cloud by keeping one point per grid cell.")
        .emulates("voxel-grid subsampling")
        .spec(SPEC_CAPTURE)
        .implemented()
}

fn average_attribs(g: &Geometry, order: &[u32], groups: &[(usize, usize)], reps: &[usize]) -> AttribTable {
    let mut out = AttribTable::default();
    for (name, col) in g.point_attribs.iter() {
        let arr = match col {
            AttribArray::Float(v) => AttribArray::Float(
                groups
                    .iter()
                    .map(|&(a, b)| order[a..b].iter().map(|&i| v[i as usize]).sum::<f64>() / (b - a) as f64)
                    .collect(),
            ),
            AttribArray::Vec3(v) => AttribArray::Vec3(
                groups
                    .iter()
                    .map(|&(a, b)| {
                        let s = order[a..b].iter().map(|&i| v[i as usize]).fold(DVec3::ZERO, |x, y| x + y);
                        let m = s / (b - a) as f64;
                        if name == "N" {
                            m.normalize_or_zero()
                        } else {
                            m
                        }
                    })
                    .collect(),
            ),
            other => other.gather(reps),
        };
        out.set(name, arr);
    }
    out
}

fn voxel_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    ensure_metric(ctx, &g)?;
    let v = pf(p, "voxel_size", 0.001, 10000.0)?;
    let mode = p.str("mode")?.to_string();
    let n = g.points.len();
    if n == 0 {
        return Err(ctx.fail("input has no points"));
    }
    let b = g.bounds();
    let ext = b.size();
    if ext.max_element() / v >= (1u64 << 21) as f64 - 2.0 {
        return Err(ctx.fail(format!(
            "voxel size {v} mm is too small for an extent of {:.0} mm (more than 2 million cells along one axis)",
            ext.max_element()
        )));
    }
    let keys: Vec<u64> = g
        .points
        .par_iter()
        .map(|q| {
            let c = ((*q - b.min) / v).floor();
            ((c.x as u64) << 42) | ((c.y as u64) << 21) | (c.z as u64)
        })
        .collect();
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.par_sort_unstable_by_key(|&i| (keys[i as usize], i));
    let mut groups: Vec<(usize, usize)> = Vec::new();
    let mut s = 0;
    for i in 1..=n {
        if i == n || keys[order[i] as usize] != keys[order[s] as usize] {
            groups.push((s, i));
            s = i;
        }
    }
    let rep_of = |&(a, e): &(usize, usize)| -> usize {
        match mode.as_str() {
            "first" => order[a] as usize,
            _ => {
                let c = order[a..e].iter().map(|&i| g.points[i as usize]).fold(DVec3::ZERO, |x, y| x + y) / (e - a) as f64;
                order[a..e]
                    .iter()
                    .map(|&i| i as usize)
                    .min_by(|&x, &y| {
                        (g.points[x] - c).length_squared().total_cmp(&(g.points[y] - c).length_squared())
                    })
                    .unwrap_or(order[a] as usize)
            }
        }
    };
    let mut sel: Vec<((usize, usize), usize)> = groups.par_iter().map(|gr| (*gr, rep_of(gr))).collect();
    sel.sort_by_key(|x| x.1);
    let groups: Vec<(usize, usize)> = sel.iter().map(|x| x.0).collect();
    let reps: Vec<usize> = sel.iter().map(|x| x.1).collect();
    let mut out = Geometry::from_points(Vec::new());
    if mode == "centroid" {
        out.points = groups
            .iter()
            .map(|&(a, e)| order[a..e].iter().map(|&i| g.points[i as usize]).fold(DVec3::ZERO, |x, y| x + y) / (e - a) as f64)
            .collect();
        out.point_attribs = average_attribs(&g, &order, &groups, &reps);
    } else {
        out.points = reps.iter().map(|&i| g.points[i]).collect();
        out.point_attribs = g.point_attribs.gather_all(&reps);
    }
    out.detail_attribs = g.detail_attribs.clone();
    let mut rep = count_report("Voxel Subsample", n, out.points.len());
    rep.metric("voxel_size_used", Metric::new(v, "mm"));
    if v > ext.length() * 0.5 {
        rep.note(Severity::Warning, "voxel size exceeds half of the bounding-box diagonal; almost everything collapses");
    }
    if !g.prims.is_empty() {
        rep.note(Severity::Info, "primitives were dropped: the result is a point cloud");
    }
    Ok(vec![Data::geometry(out), Data::report(rep)])
}

fn random_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.random_subsample", "Random Subsample", Stage::Clean)
        .input(points_in())
        .output(geo_out())
        .output(rep_out())
        .param(ParamSpec::choice("mode", "Mode", "count", &["count", "fraction"]).help("How the number of points to keep is given."))
        .param(ParamSpec::int("count", "Count", 100000, 1, 1_000_000_000).help("Points to keep in count mode."))
        .param(ParamSpec::float("fraction", "Fraction", 0.1, 0.0, 1.0).help("Share of points to keep in fraction mode."))
        .param(ParamSpec::int("seed", "Seed", 1, 0, 1_000_000_000).help("Random seed; the same seed gives the same subset."))
        .help("Keeps a uniformly random subset of the points, in their original order.")
        .emulates("random point subsampling")
        .spec(SPEC_CAPTURE)
        .implemented()
}

fn random_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    let n = g.points.len();
    if n == 0 {
        return Err(ctx.fail("input has no points"));
    }
    let m = if p.str("mode")? == "fraction" {
        let f = pf(p, "fraction", 0.0, 1.0)?;
        ((f * n as f64).round() as usize).clamp(1, n)
    } else {
        (pi(p, "count", 1, 1_000_000_000)? as usize).min(n)
    };
    let mut idx: Vec<u32> = (0..n as u32).collect();
    let mut rng = Rng::new(pi(p, "seed", 0, 1_000_000_000)? as u64);
    partial_shuffle(&mut idx, m, &mut rng);
    let mut keep: Vec<usize> = idx[..m].iter().map(|&i| i as usize).collect();
    keep.sort_unstable();
    let rep = count_report("Random Subsample", n, m);
    Ok(vec![Data::geometry(gather_points(&g, &keep)), Data::report(rep)])
}

// ----------------------------------------------------------------- denoise

fn denoise_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.denoise_points", "Denoise Points", Stage::Clean)
        .input(points_in())
        .output(geo_out())
        .output(rep_out())
        .param(ParamSpec::int("k", "Neighbours", 16, 3, 100).help("Neighbourhood size used to fit the local surface."))
        .param(
            ParamSpec::int("iterations", "Iterations", 1, 1, 20)
                .help("Number of smoothing passes; the neighbourhood is rebuilt each pass."),
        )
        .param(
            ParamSpec::float("strength", "Strength", 0.5, 0.0, 1.0)
                .help("Fraction of the distance to the local plane that each point moves per pass."),
        )
        .help("Pulls points toward a Gaussian-weighted local plane to reduce scan noise. Sharp corners round off slightly; off-surface speckle is better handled by outlier removal first.")
        .emulates("moving-least-squares style point smoothing")
        .spec(SPEC_CAPTURE)
        .implemented()
}

fn denoise_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    ensure_metric(ctx, &g)?;
    let k = pi(p, "k", 3, 100)? as usize;
    let iters = pi(p, "iterations", 1, 20)?;
    let strength = pf(p, "strength", 0.0, 1.0)?;
    let n = g.points.len();
    if n <= k {
        return Err(ctx.fail(format!("need more than k = {k} points, got {n}")));
    }
    let mut pts = g.points.clone();
    let mut total_shift = vec![0.0f64; n];
    for _ in 0..iters {
        let tree = KdTree::new(&pts);
        let new: Vec<DVec3> = pts
            .par_iter()
            .map_init(Vec::new, |buf, q| {
                tree.knn(*q, k + 1, buf);
                let h2 = buf.last().map_or(1.0, |e| e.0).max(1e-18) * 0.5;
                let (mut wsum, mut c) = (0.0, DVec3::ZERO);
                for &(d2, j) in buf.iter() {
                    let w = (-d2 / h2).exp();
                    wsum += w;
                    c += pts[j as usize] * w;
                }
                if wsum <= 0.0 {
                    return *q;
                }
                c /= wsum;
                let mut cov = glam::DMat3::ZERO;
                for &(d2, j) in buf.iter() {
                    let w = (-d2 / h2).exp();
                    let d = pts[j as usize] - c;
                    cov += glam::DMat3::from_cols(d * d.x, d * d.y, d * d.z) * w;
                }
                let (_, vecs) = eigen3(&cov);
                let nrm = vecs[2];
                *q - nrm * (nrm.dot(*q - c) * strength)
            })
            .collect();
        for i in 0..n {
            total_shift[i] += (new[i] - pts[i]).length();
        }
        pts = new;
    }
    let mut out = (*g).clone();
    let moved: Vec<f64> = (0..n).map(|i| (pts[i] - g.points[i]).length()).collect();
    out.points = pts;
    let mut rep = Report::new("Denoise Points");
    rep.metric("mean_shift", Metric::new(moved.iter().sum::<f64>() / n as f64, "mm"));
    rep.metric("max_shift", Metric::new(moved.iter().cloned().fold(0.0, f64::max), "mm"));
    if g.point_attribs.contains("N") {
        rep.note(Severity::Info, "existing normals were kept and may no longer match the moved points");
    }
    Ok(vec![Data::geometry(out), Data::report(rep)])
}

// ----------------------------------------------------------------- normals

fn normals_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.estimate_normals", "Estimate Normals", Stage::Clean)
        .input(points_in())
        .output(geo_out())
        .output(rep_out())
        .param(ParamSpec::int("k", "Neighbours", 16, 3, 200).help("Neighbourhood size for the local plane fit."))
        .param(
            ParamSpec::choice("orient", "Orientation", "up", &["none", "up", "viewpoint", "centroid_out"])
                .help("How the sign of each normal is chosen: none leaves it arbitrary; up makes Z non-negative; viewpoint faces a point; centroid_out points away from the centroid."),
        )
        .param(ParamSpec::vec3("viewpoint", "Viewpoint", DVec3::new(0.0, 0.0, 10000.0)).help("Used when orientation is 'viewpoint'."))
        .help("Estimates per-point normals by plane fitting (clouds) or area weighting (meshes). Also writes 'curv', the surface variation (0 flat, 1/3 isotropic noise).")
        .emulates("point-cloud normal estimation")
        .spec(SPEC_CAPTURE)
        .implemented()
}

fn normals_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    ensure_metric(ctx, &g)?;
    let mut out = (*g).clone();
    let mut rep = Report::new("Estimate Normals");
    if !g.prims.is_empty() {
        out.compute_normals();
        rep.note(Severity::Info, "input has faces: normals are area-weighted from the faces");
        return Ok(vec![Data::geometry(out), Data::report(rep)]);
    }
    let k = pi(p, "k", 3, 200)? as usize;
    let orient = p.str("orient")?.to_string();
    let vp = p.vec3("viewpoint")?;
    let n = g.points.len();
    if n <= k {
        return Err(ctx.fail(format!("need more than k = {k} points, got {n}")));
    }
    let centroid = g.points.iter().fold(DVec3::ZERO, |a, b| a + *b) / n as f64;
    let tree = KdTree::new(&g.points);
    let res: Vec<(DVec3, f64)> = g
        .points
        .par_iter()
        .map_init(Vec::new, |buf, q| {
            tree.knn(*q, k + 1, buf);
            let c = buf.iter().map(|e| g.points[e.1 as usize]).fold(DVec3::ZERO, |a, b| a + b) / buf.len() as f64;
            let mut cov = glam::DMat3::ZERO;
            for e in buf.iter() {
                let d = g.points[e.1 as usize] - c;
                cov += glam::DMat3::from_cols(d * d.x, d * d.y, d * d.z);
            }
            let (vals, vecs) = eigen3(&cov);
            let mut nrm = vecs[2];
            let tot = vals[0] + vals[1] + vals[2];
            let curv = if tot > 0.0 { vals[2].max(0.0) / tot } else { 0.0 };
            let flip = match orient.as_str() {
                "up" => nrm.z < 0.0,
                "viewpoint" => nrm.dot(vp - *q) < 0.0,
                "centroid_out" => nrm.dot(*q - centroid) < 0.0,
                _ => false,
            };
            if flip {
                nrm = -nrm;
            }
            (nrm, curv)
        })
        .collect();
    out.point_attribs.set("N", AttribArray::Vec3(res.iter().map(|r| r.0).collect()));
    out.point_attribs.set("curv", AttribArray::Float(res.iter().map(|r| r.1).collect()));
    rep.metric("points", Metric::new(n as f64, "points"));
    rep.metric(
        "mean_surface_variation",
        Metric::new(res.iter().map(|r| r.1).sum::<f64>() / n as f64, ""),
    );
    Ok(vec![Data::geometry(out), Data::report(rep)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;
    use vernon_core::AttribArray;

    /// Plane z = noise with injected far outliers at known indices.
    fn noisy_plane(n: usize, outliers: usize) -> Geometry {
        let mut rng = Rng::new(9);
        let side = (n as f64).sqrt() as usize;
        let mut pts = Vec::new();
        for j in 0..side {
            for i in 0..side {
                pts.push(DVec3::new(i as f64 * 0.5, j as f64 * 0.5, (rng.next_f64() - 0.5) * 0.07));
            }
        }
        for o in 0..outliers {
            let base = pts[o * 7 % pts.len()];
            pts.push(base + DVec3::new(0.0, 0.0, 25.0 + o as f64));
        }
        Geometry::from_points(pts)
    }

    #[test]
    fn sor_removes_injected_outliers() {
        let g = noisy_plane(10_000, 100);
        let total = g.points.len();
        let out = run(outlier_info, outlier_cook, vec![geo(g)], &[]).unwrap();
        let o = out_geo(&out[0]);
        // none of the outliers (z >= 25) survive, nearly all inliers do
        assert!(o.points.iter().all(|p| p.z < 1.0));
        assert!(o.points.len() > 9_500, "kept {}", o.points.len());
        let r = out_report(&out[1]);
        assert_eq!(r.metrics["input_points"].value as usize, total);
        assert!(r.metrics["removed_points"].value >= 100.0);
        o.validate().unwrap();
    }

    #[test]
    fn sor_flag_mode_keeps_everything() {
        let g = noisy_plane(2_500, 20);
        let n = g.points.len();
        let out = run(outlier_info, outlier_cook, vec![geo(g)], &[("mode", s("flag"))]).unwrap();
        let o = out_geo(&out[0]);
        assert_eq!(o.points.len(), n);
        let f = o.point_attribs.ints("outlier").unwrap();
        assert!(f[2_500..].iter().all(|&x| x == 1));
    }

    #[test]
    fn sor_rejects_too_few_points() {
        let g = Geometry::from_points(vec![DVec3::ZERO; 5]);
        assert!(run(outlier_info, outlier_cook, vec![geo(g)], &[]).is_err());
    }

    #[test]
    fn non_metric_clouds_are_refused() {
        let mut g = noisy_plane(400, 0);
        g.detail_attribs.set("metric", AttribArray::Int(vec![0]));
        assert!(run(outlier_info, outlier_cook, vec![geo(g)], &[]).is_err());
    }

    #[test]
    fn radius_filter_removes_specks() {
        let g = noisy_plane(2_500, 10);
        let out = run(radius_info, radius_cook, vec![geo(g)], &[("radius", f(1.0)), ("min_neighbors", i(3))]).unwrap();
        let o = out_geo(&out[0]);
        assert!(o.points.iter().all(|p| p.z < 1.0));
        assert!(o.points.len() >= 2_450);
    }

    #[test]
    fn voxel_subsample_counts_and_modes() {
        // 10x10x10 lattice with 1 mm spacing, voxel 2 mm -> 5x5x5 cells
        let mut pts = Vec::new();
        for z in 0..10 {
            for y in 0..10 {
                for x in 0..10 {
                    pts.push(DVec3::new(x as f64, y as f64, z as f64));
                }
            }
        }
        let g = Geometry::from_points(pts);
        for mode in ["centroid", "nearest_to_centroid", "first"] {
            let out = run(voxel_info, voxel_cook, vec![geo(g.clone())], &[("voxel_size", f(2.0)), ("mode", s(mode))]).unwrap();
            let o = out_geo(&out[0]);
            assert_eq!(o.points.len(), 125, "mode {mode}");
            o.validate().unwrap();
        }
        let out = run(voxel_info, voxel_cook, vec![geo(g)], &[("voxel_size", f(2.0)), ("mode", s("centroid"))]).unwrap();
        let o = out_geo(&out[0]);
        // centroids sit at half-integer positions
        assert!(o.points.iter().all(|p| ((p.x * 2.0).fract()).abs() < 1e-9));
        let r = out_report(&out[1]);
        assert_eq!(r.metrics["kept_points"].value, 125.0);
        assert!((r.metrics["removed_fraction"].value - 87.5).abs() < 1e-9);
    }

    #[test]
    fn voxel_centroid_averages_and_renormalises_attributes() {
        let mut g = Geometry::from_points(vec![DVec3::new(0.1, 0.1, 0.1), DVec3::new(0.3, 0.1, 0.1)]);
        g.point_attribs.set("N", AttribArray::Vec3(vec![DVec3::X, DVec3::Y]));
        g.point_attribs.set("w", AttribArray::Float(vec![1.0, 3.0]));
        let out = run(voxel_info, voxel_cook, vec![geo(g)], &[("voxel_size", f(1.0)), ("mode", s("centroid"))]).unwrap();
        let o = out_geo(&out[0]);
        assert_eq!(o.points.len(), 1);
        assert!((o.points[0] - DVec3::new(0.2, 0.1, 0.1)).length() < 1e-12);
        assert!((o.point_attribs.floats("w").unwrap()[0] - 2.0).abs() < 1e-12);
        let n = o.point_attribs.vec3s("N").unwrap()[0];
        assert!((n.length() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn voxel_too_small_voxel_is_a_clean_error() {
        let g = Geometry::from_points(vec![DVec3::ZERO, DVec3::new(5000.0, 0.0, 0.0)]);
        assert!(run(voxel_info, voxel_cook, vec![geo(g)], &[("voxel_size", f(0.001))]).is_err());
    }

    #[test]
    fn random_subsample_is_seeded_and_ordered() {
        let g = noisy_plane(2_500, 0);
        let a = run(random_info, random_cook, vec![geo(g.clone())], &[("count", i(500)), ("seed", i(3))]).unwrap();
        let b = run(random_info, random_cook, vec![geo(g.clone())], &[("count", i(500)), ("seed", i(3))]).unwrap();
        let c = run(random_info, random_cook, vec![geo(g.clone())], &[("count", i(500)), ("seed", i(4))]).unwrap();
        assert_eq!(out_geo(&a[0]).points, out_geo(&b[0]).points);
        assert_ne!(out_geo(&a[0]).points, out_geo(&c[0]).points);
        assert_eq!(out_geo(&a[0]).points.len(), 500);
        let fr = run(random_info, random_cook, vec![geo(g)], &[("mode", s("fraction")), ("fraction", f(0.2))]).unwrap();
        assert_eq!(out_geo(&fr[0]).points.len(), 500);
    }

    #[test]
    fn denoise_reduces_roughness_on_a_plane() {
        let g = noisy_plane(2_500, 0);
        let before = g.points.iter().map(|p| p.z * p.z).sum::<f64>() / g.points.len() as f64;
        let out = run(denoise_info, denoise_cook, vec![geo(g)], &[("strength", f(1.0)), ("iterations", i(2))]).unwrap();
        let o = out_geo(&out[0]);
        let after = o.points.iter().map(|p| p.z * p.z).sum::<f64>() / o.points.len() as f64;
        assert!(after < before * 0.5, "{before} -> {after}");
    }

    #[test]
    fn normals_on_an_ellipsoid_point_outward() {
        let mut g = ellipsoid_cloud(120.0, 60.0, 40.0, 6_000, 3);
        let truth = g.point_attribs.vec3s("N").unwrap().to_vec();
        g.point_attribs.remove("N");
        let out = run(normals_info, normals_cook, vec![geo(g)], &[("orient", s("centroid_out")), ("k", i(12))]).unwrap();
        let o = out_geo(&out[0]);
        let n = o.point_attribs.vec3s("N").unwrap();
        let good = n.iter().zip(&truth).filter(|(a, b)| a.dot(**b) > 0.95).count();
        assert!(good as f64 > 0.97 * n.len() as f64, "{good}/{}", n.len());
        assert!(o.point_attribs.floats("curv").is_ok());
    }

    #[test]
    fn normals_on_a_mesh_use_faces() {
        let out = run(normals_info, normals_cook, vec![geo(sphere_geo(50.0, 16, 8))], &[]).unwrap();
        let o = out_geo(&out[0]);
        assert!(o.prim_attribs.contains("N"));
    }
}

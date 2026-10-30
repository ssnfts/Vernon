//! Connected four-sided layouts, fitting and final network measurements.

use crate::bvh::TriBvh;
use crate::deviation::{area_samples, dev_stats, signed_distances};
use crate::fit::{fit_chart_surface, fit_local_surface, is_height_field, sample_mesh, TriMesh};
use crate::nurbs::{NurbsSurface, PatchSet};
use crate::patch_network::{reconcile, split_surface};
use crate::patchnodes::{fit_settings, parameterise, split_closed};
use crate::segment::{
    classify_labels, labels_from_geo, limit_regions, seg_mesh, segment, SegParams,
};
use crate::topology::{boundary_loops, tri_adjacency};
use crate::util::{check_finite, positive};
use glam::DVec3;
use vernon_core::{AttribArray, Metric, Report, Severity};
use vernon_graph::{CookContext, CookResult, Data, Params};

fn smooth_disk(m: &TriMesh) -> bool {
    let faces: Vec<_> = m.tris.iter().map(|t| t.to_vec()).collect();
    if boundary_loops(&faces).len() != 1 || !is_height_field(m) {
        return false;
    }
    let adjacency = tri_adjacency(&m.tris);
    let normal = |t: [u32; 3]| {
        (m.points[t[1] as usize] - m.points[t[0] as usize])
            .cross(m.points[t[2] as usize] - m.points[t[0] as usize])
            .normalize_or_zero()
    };
    adjacency.iter().enumerate().all(|(i, ns)| {
        ns.iter().flatten().all(|&j| {
            normal(m.tris[i]).dot(normal(m.tris[j as usize])) > 30.0_f64.to_radians().cos()
        })
    })
}

fn layout(surface: NurbsSurface, count: usize) -> Result<Vec<NurbsSurface>, String> {
    let (u0, u1) = surface.domain_u();
    let (v0, v1) = surface.domain_v();
    let du = surface
        .point(u0, (v0 + v1) * 0.5)
        .distance(surface.point(u1, (v0 + v1) * 0.5));
    let dv = surface
        .point((u0 + u1) * 0.5, v0)
        .distance(surface.point((u0 + u1) * 0.5, v1));
    // A full rectangular grid avoids T-junctions even for prime patch counts.
    let nu = (1..=count)
        .filter(|n| count.checked_rem(*n) == Some(0))
        .min_by(|&a, &b| {
            let score = |n: usize| {
                ((du / dv.max(1e-12)) * ((count / n) as f64 / n as f64))
                    .ln()
                    .abs()
            };
            score(a).total_cmp(&score(b))
        })
        .unwrap();
    let nv = count / nu;
    let mut result = Vec::new();
    for i in 0..nu {
        for j in 0..nv {
            result.push(restrict(
                surface.clone(),
                [
                    [
                        u0 + (u1 - u0) * i as f64 / nu as f64,
                        v0 + (v1 - v0) * j as f64 / nv as f64,
                    ],
                    [
                        u0 + (u1 - u0) * (i + 1) as f64 / nu as f64,
                        v0 + (v1 - v0) * (j + 1) as f64 / nv as f64,
                    ],
                ],
            )?);
        }
    }
    Ok(result)
}
fn connected_parts(m: &TriMesh) -> Vec<TriMesh> {
    let adj = tri_adjacency(&m.tris);
    let mut seen = vec![false; m.tris.len()];
    let mut out = Vec::new();
    for seed in 0..m.tris.len() {
        if seen[seed] {
            continue;
        }
        let mut ids = vec![seed];
        seen[seed] = true;
        let mut at = 0;
        while at < ids.len() {
            for &n in adj[ids[at]].iter().flatten() {
                if !seen[n as usize] {
                    seen[n as usize] = true;
                    ids.push(n as usize);
                }
            }
            at += 1;
        }
        out.push(m.sub(&ids));
    }
    out
}

// A labelled partition whose chart triangles exactly tile rectangles can share
// one fit. Restricting it by exact knot insertion preserves all derivatives.
fn rectangles(
    m: &TriMesh,
    labels: &[i64],
    uv: &[[f64; 2]],
    ids: &[i64],
) -> Option<Vec<[[f64; 2]; 2]>> {
    let mut result = Vec::new();
    for &id in ids {
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        let mut area = 0.0;
        for (t, &label) in m.tris.iter().zip(labels) {
            if label != id {
                continue;
            }
            for &v in t {
                for k in 0..2 {
                    lo[k] = lo[k].min(uv[v as usize][k]);
                    hi[k] = hi[k].max(uv[v as usize][k]);
                }
            }
            let [a, b, c] = t.map(|v| uv[v as usize]);
            area += ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() * 0.5;
        }
        let rectangle = (hi[0] - lo[0]) * (hi[1] - lo[1]);
        if rectangle <= 1e-12 || (area - rectangle).abs() > rectangle * 1e-5 {
            return None;
        }
        result.push([lo, hi]);
    }
    Some(result)
}

fn restrict(mut s: NurbsSurface, bounds: [[f64; 2]; 2]) -> Result<NurbsSurface, String> {
    for axis in 0..2 {
        let (a, b) = if axis == 0 {
            s.domain_u()
        } else {
            s.domain_v()
        };
        let (lo, hi) = (bounds[0][axis], bounds[1][axis]);
        if hi < b - 1e-10 {
            s = split_surface(&s, axis, hi)?.0;
        }
        if lo > a + 1e-10 {
            s = split_surface(&s, axis, lo)?.1;
        }
    }
    Ok(s)
}
// Verify source region interfaces are covered by measured whole-edge joins.
// Exterior edges do not enter this check. Failed/partial matches cannot vanish
// into the count of exterior unmatched edges and thereby pass continuity gates.
fn unjoined_boundaries(
    m: &TriMesh,
    labels: &[i64],
    ps: &PatchSet,
    patch_charts: &[i64],
    tol: f64,
) -> usize {
    use std::collections::{BTreeMap, BTreeSet};
    let mut boundaries: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    for (i, p) in ps.patches.iter().enumerate() {
        for (e, &b) in p.boundary_ids.iter().enumerate() {
            boundaries.entry(b).or_default().push((i, e));
        }
    }
    let mut joined: BTreeMap<(i64, i64), Vec<(DVec3, DVec3)>> = BTreeMap::new();
    let key = |a: i64, b: i64| (a.min(b), a.max(b));
    for pair in boundaries.values().filter(|p| p.len() == 2) {
        let ((a, e), (b, _)) = (pair[0], pair[1]);
        let (ra, rb) = (patch_charts[a], patch_charts[b]);
        if ra == rb {
            continue;
        }
        let s = &ps.patches[a].surface;
        let (u0, u1) = s.domain_u();
        let (v0, v1) = s.domain_v();
        let at = |t: f64| match e {
            0 => s.point(u0, v0 + (v1 - v0) * t),
            1 => s.point(u1, v0 + (v1 - v0) * t),
            2 => s.point(u0 + (u1 - u0) * t, v0),
            _ => s.point(u0 + (u1 - u0) * t, v1),
        };
        let list = joined.entry(key(ra, rb)).or_default();
        for k in 0..128 {
            list.push((at(k as f64 / 128.0), at((k + 1) as f64 / 128.0)));
        }
    }
    let mut missing = BTreeSet::new();
    for (i, ns) in tri_adjacency(&m.tris).iter().enumerate() {
        for (e, &n) in ns.iter().enumerate() {
            let Some(j) = n.map(|x| x as usize) else {
                continue;
            };
            if j <= i || labels[i] == labels[j] {
                continue;
            }
            let pair = key(labels[i], labels[j]);
            if missing.contains(&pair) {
                continue;
            }
            let Some(edges) = joined.get(&pair) else {
                missing.insert(pair);
                continue;
            };
            let (a, b) = (
                m.points[m.tris[i][e] as usize],
                m.points[m.tris[i][(e + 1) % 3] as usize],
            );
            for p in [a, (a + b) * 0.5, b] {
                let near = edges.iter().any(|&(q, r)| {
                    let d = r - q;
                    let t = ((p - q).dot(d) / d.length_squared().max(1e-30)).clamp(0.0, 1.0);
                    p.distance(q + d * t) <= tol
                });
                if !near {
                    missing.insert(pair);
                    break;
                }
            }
        }
    }
    missing.len()
}
fn triangle_key(m: &TriMesh, t: [u32; 3]) -> [[u64; 3]; 3] {
    let mut key = t.map(|v| m.points[v as usize].to_array().map(f64::to_bits));
    key.sort_unstable();
    key
}
pub fn cook(ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
    let geo = vernon_graph::node::inputs::geometry(inputs, 0)?;
    check_finite(ctx, &geo)?;
    geo.validate()
        .map_err(|e| ctx.fail(format!("invalid mesh: {e}")))?;
    let fs = fit_settings(ctx, params)?;
    let sm = seg_mesh(&geo);
    if sm.mesh.tris.is_empty() {
        return Err(ctx.fail("the input has no non-degenerate faces"));
    }
    let min_area = match params.str("detail")? {
        "coarse" => 0.05,
        "fine" => 0.002,
        _ => 0.01,
    };
    let sp = SegParams {
        sensitivity: params.float("sensitivity")?.clamp(0.0, 100.0),
        noise_sigma: positive(ctx, "noise_sigma_mm", params.float("noise_sigma_mm")?)?,
        min_area_frac: min_area,
        cad_tessellation: false,
    };
    let target = params.int("target_patches")?.max(0) as usize;
    if target > 256 {
        return Err(ctx
            .fail("automatic surfacing is limited to 256 patches per cook; lower Target patches"));
    }
    let labels = labels_from_geo(&geo, &sm);
    if labels.as_ref().is_some_and(|l| l.iter().any(|&x| x < 0)) {
        return Err(ctx.fail(
            "some faces have no region assignment; refine or complete the segmentation first",
        ));
    }
    let mut seg = match &labels {
        Some(l) => classify_labels(&sm.mesh, l, &sp),
        None => segment(&sm.mesh, &sp),
    };
    if target > 0 && labels.is_none() {
        seg = limit_regions(&sm.mesh, &seg, target, &sp);
    }
    let region_ids: Vec<i64> = match &labels {
        Some(l) => {
            let mut v = l.clone();
            v.sort_unstable();
            v.dedup();
            v
        }
        None => (0..seg.regions.len() as i64).collect(),
    };
    let mut report = Report::new("Auto surface");
    // A smooth disk is a single chart. Avoid artificial segmentation seams when
    // the whole chart meets tolerance, then lay out exact four-sided subpatches.
    let whole_chart = labels.is_none() && smooth_disk(&sm.mesh);
    let mut regions: Vec<(i64, TriMesh)> = if whole_chart {
        vec![(0, sm.mesh.clone())]
    } else {
        seg.regions
            .iter()
            .enumerate()
            .map(|(i, r)| {
                (
                    region_ids[i],
                    sm.mesh
                        .sub(&r.tris.iter().map(|&t| t as usize).collect::<Vec<_>>()),
                )
            })
            .collect()
    };
    let region_count = regions.len();
    if region_count > 256 {
        return Err(
            ctx.fail("more than 256 regions; merge regions or reduce segmentation sensitivity")
        );
    }
    let mut source_faces = std::collections::BTreeMap::<_, Vec<usize>>::new();
    for (i, &t) in sm.mesh.tris.iter().enumerate() {
        source_faces
            .entry(triangle_key(&sm.mesh, t))
            .or_default()
            .push(i);
    }
    let mut source_charts = vec![-1; sm.mesh.tris.len()];
    let mut patch_charts = Vec::new();
    let mut next_chart = 0;
    let mut surfaces = Vec::new();
    let mut ids = Vec::new();
    let mut rounds = 0;
    let common = if let Some(source_labels) = labels.as_ref().filter(|_| smooth_disk(&sm.mesh)) {
        let (uv, aspect, _) = parameterise(&sm.mesh, "boundary").map_err(|e| ctx.fail(e))?;
        rectangles(&sm.mesh, source_labels, &uv, &region_ids).map(|bounds| (uv, aspect, bounds))
    } else {
        None
    };
    if let Some((uv, aspect, bounds)) = common {
        let mut samples = sample_mesh(&sm.mesh, &uv, fs.extra.max(3), 60000);
        let fit = fit_chart_surface(
            &mut samples,
            fs.degree,
            fs.init_cp,
            fs.max_cp,
            aspect,
            fs.tol * 0.5,
            fs.smooth,
            3,
        )
        .ok_or_else(|| ctx.fail("the shared chart fit failed"))?;
        rounds = fit.iterations;
        for (&id, bounds) in region_ids.iter().zip(bounds) {
            for (i, &label) in labels.as_ref().unwrap().iter().enumerate() {
                if label == id {
                    source_charts[i] = next_chart;
                }
            }
            patch_charts.push(next_chart);
            next_chart += 1;
            surfaces.push(restrict(fit.surface.clone(), bounds).map_err(|e| ctx.fail(e))?);
            ids.push(id);
        }
        regions.clear();
    }
    let regions: Vec<_> = regions
        .drain(..)
        .flat_map(|(id, m)| connected_parts(&m).into_iter().map(move |m| (id, m)))
        .collect();
    for (id, whole) in regions {
        let faces: Vec<_> = whole.tris.iter().map(|t| t.to_vec()).collect();
        let pieces = if boundary_loops(&faces).is_empty() {
            split_closed(&whole)
        } else {
            vec![whole]
        };
        for m in pieces {
            let chart = next_chart;
            next_chart += 1;
            for &t in &m.tris {
                for &i in source_faces
                    .get(&triangle_key(&m, t))
                    .ok_or_else(|| ctx.fail("surface chart lost its source triangle mapping"))?
                {
                    source_charts[i] = chart;
                }
            }
            let mode = if whole_chart { "boundary" } else { "auto" };
            let (uv, aspect, _) =
                parameterise(&m, mode).map_err(|e| ctx.fail(format!("region {id}: {e}")))?;
            let extra = if m.points.len() < 3 * fs.init_cp * fs.init_cp {
                fs.extra.max(8)
            } else {
                fs.extra
            };
            let mut samples = sample_mesh(
                &m,
                &uv,
                if whole_chart { extra.max(3) } else { extra },
                60000,
            );
            let fitter = if whole_chart {
                fit_chart_surface
            } else {
                fit_local_surface
            };
            let fit = fitter(
                &mut samples,
                fs.degree,
                fs.init_cp,
                fs.max_cp,
                aspect,
                fs.tol * 0.5,
                fs.smooth,
                3,
            )
            .ok_or_else(|| ctx.fail(format!("region {id}: insufficient data for a surface fit")))?;
            rounds += fit.iterations;
            let count = if whole_chart {
                target.max(match params.str("detail")? {
                    "coarse" => 1,
                    "fine" => 8,
                    _ => 4,
                })
            } else {
                1
            };
            // Target is a cap when explicitly supplied, including one chart.
            let count = if whole_chart && target > 0 {
                target
            } else {
                count
            };
            for s in layout(fit.surface, count).map_err(|e| ctx.fail(e))? {
                surfaces.push(s);
                ids.push(id);
                patch_charts.push(chart);
            }
            if surfaces.len() > 256 {
                return Err(ctx.fail(
                    "the patch layout exceeded 256 patches; reduce detail or merge regions",
                ));
            }
        }
    }
    if surfaces.is_empty() {
        return Err(ctx.fail("no surface regions were fitted"));
    }
    let mut ps = PatchSet::new(surfaces);
    for (p, id) in ps.patches.iter_mut().zip(ids) {
        p.region_id = Some(id);
    }
    let g1 = params.str("continuity")? == "g1";
    let seams = reconcile(&mut ps, g1, (fs.tol * 2.0).min(1.0));
    if !seams.max_gap_mm.is_finite() || !seams.max_normal_deg.is_finite() {
        return Err(ctx.fail("seam reconciliation exceeded a resource bound or produced a degenerate tangent; simplify or refine the patch layout"));
    }
    for p in &ps.patches {
        p.surface.validate().map_err(|e| ctx.fail(e))?;
    }
    let mut preview = ps.tessellate((fs.tol * 0.1).max(0.001));
    let parts = preview
        .prim_attribs
        .ints("patch_id")
        .map_err(|e| ctx.fail(e.to_string()))?
        .to_vec();
    preview.prim_attribs.set("part", AttribArray::Int(parts));
    preview.compute_normals();
    let bvh = TriBvh::from_geometry(&preview, None);
    if bvh.is_empty() {
        return Err(ctx.fail("the fitted surfaces are degenerate"));
    }
    let mut points = geo.points.clone();
    points.extend(area_samples(&geo, 8000, 1));
    let distances = signed_distances(&bvh, &points);
    let dev = dev_stats(&distances, fs.tol);
    let reverse = dev_stats(
        &signed_distances(
            &TriBvh::from_geometry(&geo, None),
            &area_samples(&preview, 4000, 8),
        ),
        fs.tol,
    );
    if source_charts.iter().any(|&id| id < 0) {
        return Err(ctx.fail("some source triangles were not assigned to a surface chart"));
    }
    let unjoined = unjoined_boundaries(
        &sm.mesh,
        &source_charts,
        &ps,
        &patch_charts,
        (fs.tol * 2.0).max(dev.max_abs * 2.0),
    );
    report.metric(
        "unjoined_region_boundaries",
        Metric::new(unjoined as f64, "count").with_limit(0.0, true),
    );
    if unjoined > 0 {
        report.note(Severity::Warning,format!("{unjoined} source chart interfaces are not fully covered by measured whole-edge joins; partial-edge joins or a new layout are required"));
    }
    report.metric("region_count", Metric::new(region_count as f64, "count"));
    report.metric("patch_count", Metric::new(ps.patches.len() as f64, "count"));
    report.metric("skipped_regions", Metric::new(0.0, "count"));
    report.metric(
        "control_points",
        Metric::new(ps.control_point_count() as f64, "count"),
    );
    report.metric("refinement_rounds", Metric::new(rounds as f64, "count"));
    report.metric(
        "dev_max_mm",
        Metric::new(dev.max_abs, "mm").with_limit(fs.tol, true),
    );
    report.metric("dev_rms_mm", Metric::new(dev.rms, "mm"));
    report.metric(
        vernon_core::keys::DEV_P95_MM,
        Metric::new(dev.p95, "mm").with_limit(fs.tol, true),
    );
    report.metric(
        "dev_reverse_p95_mm",
        Metric::new(reverse.p95, "mm").with_limit(fs.tol, true),
    );
    report.metric("dev_reverse_max_mm", Metric::new(reverse.max_abs, "mm"));
    report.metric("sample_count", Metric::new(dev.count as f64, "count"));
    report.metric("seam_count", Metric::new(seams.seams as f64, "count"));
    report.metric(
        "unmatched_edges",
        Metric::new(seams.unmatched_edges as f64, "count"),
    );
    report.metric(
        "max_seam_gap_mm",
        Metric::new(seams.max_gap_mm, "mm").with_limit(0.01, true),
    );
    let angle = Metric::new(seams.max_normal_deg, "deg");
    report.metric(
        "max_normal_jump_deg",
        if g1 {
            angle.with_limit(1.0, true)
        } else {
            angle
        },
    );
    if dev.p95 > fs.tol || reverse.p95 > fs.tol {
        report.note(Severity::Warning,"the final network exceeds the requested deviation tolerance; increase the control-point limit, reduce smoothing or refine the regions");
    }
    if g1 && seams.max_normal_deg > 1.0 {
        report.note(
            Severity::Warning,
            "the measured tangent continuity is above one degree; refine the adjoining regions",
        );
    }
    if seams.seams == 0 {
        report.note(Severity::Info,"no compatible shared whole-edge seam was found; no inter-patch continuity is certified");
    }
    report.note(Severity::Info,"Boundary ids refer to u-min, u-max, v-min and v-max. Shared whole edges are reconciled; partial-edge joins and trimming are not supported. Deviation is measured on the final tessellated network using vertices and deterministic area samples.");
    Ok(vec![
        Data::json(ps.to_json()),
        Data::geometry(preview),
        Data::report(report),
    ])
}

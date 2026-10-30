//! Surface-vs-mesh deviation statistics and the `surface.deviation_check` node.
//! This is the numeric gate the print stage reads.

use glam::DVec3;
use vernon_core::{AttribArray, Geometry, Metric, Report, Severity};
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::bvh::TriBvh;
use crate::nurbs::PatchSet;
use crate::util::{check_finite, json_err, json_input, node_info, percentile, positive, triangulate_ring, Rng, SPEC_RE};

/// Summary of signed distances (positive = measured mesh outside the surface).
#[derive(Debug, Clone, Default)]
pub struct DevStats {
    pub count: usize,
    pub mean: f64,
    pub rms: f64,
    pub std: f64,
    pub max_abs: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub pct_within: f64,
}

/// Statistics of signed distances against a tolerance band.
pub fn dev_stats(signed: &[f64], tol: f64) -> DevStats {
    if signed.is_empty() {
        return DevStats::default();
    }
    let n = signed.len() as f64;
    let mean = signed.iter().sum::<f64>() / n;
    let rms = (signed.iter().map(|d| d * d).sum::<f64>() / n).sqrt();
    let mut abs: Vec<f64> = signed.iter().map(|d| d.abs()).collect();
    abs.sort_by(|a, b| a.total_cmp(b));
    let within = abs.partition_point(|&d| d <= tol + 1e-12);
    DevStats {
        count: signed.len(),
        mean,
        rms,
        // sample (N - 1) standard deviation, the shared convention
        std: vernon_core::stats::sample_std(signed),
        max_abs: *abs.last().expect("non-empty"),
        p50: percentile(&abs, 50.0),
        p95: percentile(&abs, 95.0),
        p99: percentile(&abs, 99.0),
        pct_within: 100.0 * within as f64 / n,
    }
}

/// Green (0) - yellow (half tolerance) - red (tolerance and beyond).
pub fn abs_ramp(d: f64, tol: f64) -> DVec3 {
    let t = (d.abs() / tol).clamp(0.0, 1.0);
    if t <= 0.5 {
        DVec3::new(2.0 * t, 1.0, 0.0)
    } else {
        DVec3::new(1.0, 2.0 * (1.0 - t), 0.0)
    }
}

/// Blue (-tol) - green (0) - red (+tol).
pub fn signed_ramp(d: f64, tol: f64) -> DVec3 {
    let s = (d / tol).clamp(-1.0, 1.0);
    if s < 0.0 {
        DVec3::new(0.0, 1.0 + s, -s)
    } else {
        DVec3::new(s, 1.0 - s, 0.0)
    }
}

/// Deterministic area-weighted samples on the closed faces of `geo`.
pub fn area_samples(geo: &Geometry, n: usize, seed: u64) -> Vec<DVec3> {
    let mut tris: Vec<[DVec3; 3]> = Vec::new();
    for p in geo.prims.iter().filter(|p| p.closed && p.vertices.len() >= 3) {
        for t in triangulate_ring(&geo.points, &p.vertices) {
            tris.push([geo.points[t[0] as usize], geo.points[t[1] as usize], geo.points[t[2] as usize]]);
        }
    }
    if tris.is_empty() || n == 0 {
        return vec![];
    }
    let mut cum = Vec::with_capacity(tris.len());
    let mut acc = 0.0;
    for t in &tris {
        acc += 0.5 * (t[1] - t[0]).cross(t[2] - t[0]).length();
        cum.push(acc);
    }
    if acc <= 0.0 {
        return vec![];
    }
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|_| {
            let r = rng.next_f64() * acc;
            let k = cum.partition_point(|&c| c < r).min(tris.len() - 1);
            let (mut a, mut b) = (rng.next_f64(), rng.next_f64());
            if a + b > 1.0 {
                a = 1.0 - a;
                b = 1.0 - b;
            }
            let t = &tris[k];
            t[0] + (t[1] - t[0]) * a + (t[2] - t[0]) * b
        })
        .collect()
}

/// Signed distances of `points` to `bvh`. Points with no hit are skipped.
pub fn signed_distances(bvh: &TriBvh, points: &[DVec3]) -> Vec<f64> {
    bvh.closest_many(points).into_iter().flatten().map(|h| h.signed).collect()
}

/// Tessellate the surface inputs of a deviation node into one geometry.
pub fn surface_geometry(ctx: &CookContext, surface: Option<&Geometry>, patches: Option<&serde_json::Value>, chord: f64) -> CookResult<Geometry> {
    let mut out = Geometry::new();
    if let Some(s) = surface {
        out.merge(s);
    }
    if let Some(j) = patches {
        let ps = PatchSet::from_json(j).map_err(|e| json_err(ctx, "patch set input", e))?;
        out.merge(&ps.tessellate(chord));
    }
    Ok(out)
}

/// `surface.deviation_check`
pub struct DeviationCheckNode;

impl NodeOp for DeviationCheckNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.deviation_check", "Surface Deviation", SPEC_RE)
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .input(PortSpec::optional("surface", DataKind::Geometry))
            .input(PortSpec::optional("patches", DataKind::Json))
            .output(PortSpec::new("deviation", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("tol_mm", "Tolerance", 0.5, 0.001, 50.0).unit("mm").help("Half-width of the accepted band; also the colour scale end point."))
            .param(ParamSpec::int("samples", "Samples", 200_000, 1000, 50_000_000).help("Area-weighted samples added to the mesh vertices."))
            .param(ParamSpec::float("p_gate", "Gate percentile", 95.0, 50.0, 100.0).unit("%").help("Share of samples that must lie inside the band."))
            .param(ParamSpec::choice("colour_map", "Colour map", "abs", &["abs", "signed"]).help("Absolute distance ramp, or a signed blue-green-red ramp."))
            .param(ParamSpec::int("seed", "Seed", 1, 0, 1_000_000).help("Seed of the deterministic sampler."))
            .help("Measures how far a surface lies from a reference mesh: signed distance at every mesh vertex plus area-weighted samples. Writes the signed distance and a colour per vertex and reports mean, RMS, maximum, percentiles and the share inside the tolerance band. The percentile gate is what the print stage checks.")
            .emulates("QUICKSURFACE live deviation")
            .emulates("Design X accuracy analyzer")
            .emulates("Metro-style sampled distance")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let mesh = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &mesh)?;
        let tol = positive(ctx, "tol_mm", params.float("tol_mm")?)?;
        let p_gate = params.float("p_gate")?.clamp(50.0, 100.0);
        let nsamp = params.int("samples")?.clamp(1000, 50_000_000) as usize;
        let seed = params.int("seed")? as u64;
        let signed_map = params.str("colour_map")? == "signed";
        let surface_in = vernon_graph::node::inputs::opt_geometry(inputs, 1);
        let patches_in = json_input(inputs, 2);
        if surface_in.is_none() && patches_in.is_none() {
            return Err(ctx.fail("connect a surface mesh or a patch set to measure against"));
        }
        if mesh.points.is_empty() {
            return Err(ctx.fail("the reference mesh has no points"));
        }
        let surf = surface_geometry(ctx, surface_in.as_deref(), patches_in.as_deref(), tol / 10.0)?;
        check_finite(ctx, &surf)?;
        let bvh = TriBvh::from_geometry(&surf, Some(1e-6));
        if bvh.is_empty() {
            return Err(ctx.fail("the surface has no triangles"));
        }

        // per-vertex distances (written back) and extra area samples (statistics only)
        let vertex_hits = bvh.closest_many(&mesh.points);
        let vertex_d: Vec<f64> = vertex_hits.iter().map(|h| h.map_or(0.0, |h| h.signed)).collect();
        let extra = area_samples(&mesh, nsamp, seed);
        let mut all_d = vertex_d.clone();
        all_d.extend(signed_distances(&bvh, &extra));
        let st = dev_stats(&all_d, tol);

        // reverse direction: surface samples to the mesh (Hausdorff-style check)
        let mesh_bvh = TriBvh::from_geometry(&mesh, Some(1e-6));
        let rev = if mesh_bvh.is_empty() {
            Vec::new()
        } else {
            let sp = area_samples(&surf, (nsamp / 4).max(1000), seed.wrapping_add(7));
            signed_distances(&mesh_bvh, &sp)
        };
        let rev_st = dev_stats(&rev, tol);

        let mut out = (*mesh).clone();
        out.point_attribs.set("dist", AttribArray::Float(vertex_d.clone()));
        let cd: Vec<DVec3> = vertex_d.iter().map(|&d| if signed_map { signed_ramp(d, tol) } else { abs_ramp(d, tol) }).collect();
        out.point_attribs.set("Cd", AttribArray::Vec3(cd));

        let gate_val = {
            let mut a: Vec<f64> = all_d.iter().map(|d| d.abs()).collect();
            a.sort_by(|x, y| x.total_cmp(y));
            percentile(&a, p_gate)
        };
        let mut report = Report::new("Surface deviation");
        report.note(vernon_core::Severity::Info, vernon_core::stats::CONVENTION);
        report.metric("dev_mean_mm", Metric::new(st.mean, "mm"));
        report.metric("dev_rms_mm", Metric::new(st.rms, "mm"));
        report.metric("dev_std_mm", Metric::new(st.std, "mm"));
        report.metric("dev_max_mm", Metric::new(st.max_abs, "mm"));
        report.metric("dev_p50_mm", Metric::new(st.p50, "mm"));
        report.metric(vernon_core::keys::DEV_P95_MM, Metric::new(st.p95, "mm").with_limit(tol, true));
        report.metric("dev_p99_mm", Metric::new(st.p99, "mm"));
        report.metric("dev_gate_mm", Metric::new(gate_val, "mm").with_limit(tol, true));
        report.metric("pct_within_tol", Metric::new(st.pct_within, "%"));
        report.metric("sample_count", Metric::new(st.count as f64, "count"));
        if !rev.is_empty() {
            report.metric("dev_reverse_p95_mm", Metric::new(rev_st.p95, "mm"));
            report.metric("dev_reverse_max_mm", Metric::new(rev_st.max_abs, "mm"));
        }
        if gate_val > tol {
            report.note(Severity::Error, format!("{p_gate:.1}% of samples lie within {gate_val:.4} mm, above the {tol} mm tolerance"));
        }
        if surf.surface_area() < 0.2 * mesh.surface_area() {
            report.note(Severity::Warning, "the surface covers much less area than the mesh; distances to its border may dominate");
        }
        Ok(vec![Data::geometry(out), Data::report(report)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{cook, f, geo_of, grid, report_of, some};

    fn plane() -> Geometry {
        grid(10, 10, 100.0, 100.0, |_, _| 0.0)
    }

    fn shifted(dz: f64) -> Geometry {
        let mut g = grid(20, 20, 80.0, 80.0, |_, _| dz);
        for p in &mut g.points {
            p.x += 10.0;
            p.y += 10.0;
        }
        g
    }

    fn run(mesh: &Geometry, tol: f64) -> (Geometry, Report) {
        let out = cook("surface.deviation_check", &[some(mesh), some(&plane()), None], &[("tol_mm", f(tol)), ("samples", crate::testutil::i(5000))]).unwrap();
        (geo_of(&out[0]), report_of(&out[1]))
    }

    #[test]
    fn plane_vs_offset_mesh_golden() {
        let (g, r) = run(&shifted(0.3), 0.5);
        let m = |k: &str| r.metrics[k].value;
        assert!((m("dev_mean_mm") - 0.3).abs() < 1e-6);
        assert!((m("dev_p95_mm") - 0.3).abs() < 1e-6);
        assert!((m("pct_within_tol") - 100.0).abs() < 1e-9);
        assert!(r.passes());
        assert!(g.point_attribs.floats("dist").unwrap().iter().all(|d| (d - 0.3).abs() < 1e-9));
        let (_, r2) = run(&shifted(0.3), 0.25);
        assert_eq!(r2.metrics["pct_within_tol"].value, 0.0);
        assert!(!r2.passes());
    }

    #[test]
    fn sign_flips_when_the_mesh_moves_inside() {
        let (_, r) = run(&shifted(-0.3), 0.5);
        assert!((r.metrics["dev_mean_mm"].value + 0.3).abs() < 1e-6);
    }

    #[test]
    fn colours_hit_the_ramp_end_points() {
        let (g, _) = run(&shifted(0.0), 0.5);
        let cd = g.point_attribs.vec3s("Cd").unwrap();
        assert!(cd.iter().all(|c| *c == DVec3::new(0.0, 1.0, 0.0)));
        let (g, _) = run(&shifted(0.5), 0.5);
        assert!(g.point_attribs.vec3s("Cd").unwrap().iter().all(|c| *c == DVec3::new(1.0, 0.0, 0.0)));
        assert_eq!(abs_ramp(0.25, 0.5), DVec3::new(1.0, 1.0, 0.0));
        assert_eq!(signed_ramp(-0.5, 0.5), DVec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn drift_budget_golden() {
        // 0.05 mm/m drift over a 4.5 m body: far end 0.225 mm
        let surface = grid(45, 2, 4500.0, 200.0, |_, _| 0.0);
        let mesh = grid(90, 2, 4500.0, 200.0, |x, _| 0.05 * x / 1000.0);
        let go = |tol: f64| {
            let out = cook("surface.deviation_check", &[some(&mesh), some(&surface), None], &[("tol_mm", f(tol)), ("samples", crate::testutil::i(20000))]).unwrap();
            report_of(&out[1])
        };
        let r = go(0.5);
        assert!((r.metrics["dev_max_mm"].value - 0.225).abs() < 0.002);
        assert!(r.passes());
        // sqrt(0.1^2 + 0.225^2 + 0.3^2) = 0.39 mm sits inside the 0.5 mm gate
        let budget = (0.1f64.powi(2) + 0.225f64.powi(2) + 0.3f64.powi(2)).sqrt();
        assert!((budget - 0.39).abs() < 0.005);
        let r = go(0.2);
        assert!(!r.passes());
        assert!(r.notes.iter().any(|(s, _)| *s == Severity::Error));
    }

    #[test]
    fn missing_surface_and_bad_tolerance_fail_cleanly() {
        assert!(cook("surface.deviation_check", &[some(&plane()), None, None], &[]).is_err());
        assert!(cook("surface.deviation_check", &[some(&plane()), some(&plane()), None], &[("tol_mm", f(0.0))]).is_err());
    }

    #[test]
    fn patch_set_input_is_tessellated() {
        use crate::nurbs::NurbsSurface;
        let pts: Vec<DVec3> = (0..4).flat_map(|i| (0..4).map(move |j| DVec3::new(100.0 * i as f64 / 3.0, 100.0 * j as f64 / 3.0, 0.0))).collect();
        let ps = PatchSet::new(vec![NurbsSurface::clamped_uniform(3, 3, 4, 4, pts)]);
        let out = cook("surface.deviation_check", &[some(&shifted(0.2)), None, Some(Data::json(ps.to_json()))], &[("samples", crate::testutil::i(2000))]).unwrap();
        let r = report_of(&out[1]);
        assert!((r.metrics["dev_mean_mm"].value - 0.2).abs() < 1e-6);
    }

    #[test]
    fn stats_percentiles_are_sorted_abs() {
        let s = dev_stats(&[-1.0, 2.0, 0.0, 1.0], 1.0);
        assert_eq!(s.max_abs, 2.0);
        assert!((s.pct_within - 75.0).abs() < 1e-9);
    }
}

#[cfg(test)]
mod shared_convention_tests {
    #[test]
    fn dev_stats_use_nearest_rank_and_sample_std() {
        let v: Vec<f64> = (1..=10).map(f64::from).collect();
        let s = super::dev_stats(&v, 9.8);
        // nearest rank: p95 of 1..10 is 10, so a 9.8 gate fails here as in the clean stage
        assert_eq!(s.p95, 10.0);
        assert_eq!(s.p50, 5.0);
        assert!((s.std - vernon_core::stats::sample_std(&v)).abs() < 1e-12);
    }
}

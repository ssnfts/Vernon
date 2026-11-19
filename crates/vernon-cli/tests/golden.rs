//! Golden tests: Vernon's output compared with output recorded from reference
//! tools on inputs Vernon wrote itself. See `tests/golden/README.md` for how
//! each reference was recorded; the tools are never inspected, only run.

use std::path::{Path, PathBuf};

use vernon_core::{DVec3, Geometry};
use vernon_graph::{CookContext, Data, ParamValue, Params};

fn golden(case: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden").join(case)
}

fn context(name: &str) -> CookContext {
    let work_dir = std::env::temp_dir().join(format!("vernon-golden-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&work_dir).unwrap();
    CookContext { node_name: name.into(), work_dir, frame: 0.0 }
}

fn read_input(path: &Path) -> Geometry {
    vernon_io::read(path, &Default::default()).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// One "x y z" line per vertex.
fn read_points(path: &Path) -> Vec<DVec3> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: Vec<f64> = l.split_whitespace().map(|t| t.parse().unwrap()).collect();
            DVec3::new(v[0], v[1], v[2])
        })
        .collect()
}

fn read_count(path: &Path) -> usize {
    std::fs::read_to_string(path).unwrap().trim().parse().unwrap()
}

/// Vertex orders differ between tools, so match every reference vertex to its
/// nearest Vernon vertex. The match must be one-to-one and within `tol`.
fn assert_same_points(ours: &[DVec3], theirs: &[DVec3], tol: f64, what: &str) -> f64 {
    assert_eq!(ours.len(), theirs.len(), "{what}: vertex count");
    let mut used = vec![false; ours.len()];
    let mut worst = 0.0f64;
    for (k, t) in theirs.iter().enumerate() {
        let (i, d) = ours
            .iter()
            .enumerate()
            .map(|(i, o)| (i, o.distance(*t)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        assert!(d <= tol, "{what}: reference vertex {k} at {t} is {d} mm from the nearest Vernon vertex");
        assert!(!used[i], "{what}: two reference vertices matched Vernon vertex {i}");
        used[i] = true;
        worst = worst.max(d);
    }
    worst
}

fn subd(cage: &Geometry, levels: i64, output: &str) -> Geometry {
    let reg = vernon_cli::registry();
    let op = reg.get("surface.subd_refine").expect("node registered");
    let mut p = Params::from_specs(&op.info().params);
    p.set("levels", ParamValue::Int(levels));
    p.set("output", ParamValue::Str(output.into()));
    let out = op.cook(&context("subd"), &[Some(Data::geometry(cage.clone()))], &p).unwrap();
    out[0].as_geometry().unwrap().as_ref().clone()
}

// Blender stores positions in 32-bit floats: about 1e-5 mm at this size.
const BLENDER_TOL_MM: f64 = 1e-4;

/// Rows of a CloudCompare ASCII export: position and the scalar in column 4.
fn read_scalar_cloud(path: &Path) -> Vec<(DVec3, f64)> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .filter(|l| !l.starts_with("//") && !l.trim().is_empty())
        .map(|l| {
            let v: Vec<f64> = l.split_whitespace().map(|t| t.parse().unwrap()).collect();
            (DVec3::new(v[0], v[1], v[2]), v[3])
        })
        .collect()
}

fn cook_distance(id: &str, compared: Geometry, reference: Geometry, set: &[(&str, ParamValue)]) -> Vec<f64> {
    let reg = vernon_cli::registry();
    let op = reg.get(id).expect("node registered");
    let mut p = Params::from_specs(&op.info().params);
    for (k, v) in set {
        p.set(k, v.clone());
    }
    let out = op.cook(&context("distance"), &[Some(Data::geometry(compared)), Some(Data::geometry(reference))], &p).unwrap();
    out[0].as_geometry().unwrap().point_attribs.floats("dist").unwrap().to_vec()
}

/// CloudCompare keeps point order but stores positions as 32-bit floats.
fn assert_same_scalars(ours: &[f64], compared: &Geometry, theirs: &[(DVec3, f64)], tol: f64, what: &str) -> f64 {
    assert_eq!(ours.len(), theirs.len(), "{what}: point count");
    let mut worst = 0.0f64;
    for (i, ((p, t), o)) in theirs.iter().zip(ours).enumerate() {
        assert!(p.distance(compared.points[i]) < 1e-3, "{what}: point {i} is out of order");
        let d = (o - t).abs();
        assert!(d <= tol, "{what}: point {i}: Vernon {o} mm, reference {t} mm");
        worst = worst.max(d);
    }
    worst
}

// The distances themselves are written with 32-bit precision too.
const CLOUD_TOL_MM: f64 = 1e-4;

// alignment-deviation.md section 5, case 15. Separate from the per-point bound.
const CLOUD_STATS_TOL_MM: f64 = 1e-3;

/// Independent test-side reduction of signed scalars: mean, population std (N),
/// sample std (N-1; zero for a singleton, matching the node's report contract).
/// Never call the production moments helper to construct reference expectations.
fn signed_statistics(values: &[f64]) -> Result<(f64, f64, f64), &'static str> {
    if values.is_empty() || values.iter().any(|v| !v.is_finite()) {
        return Err("statistics require nonempty finite signed values");
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let sum_squared = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>();
    let population_std = (sum_squared / n).sqrt();
    let sample_std = if values.len() > 1 { (sum_squared / (n - 1.0)).sqrt() } else { 0.0 };
    if !mean.is_finite() || !population_std.is_finite() || !sample_std.is_finite() {
        return Err("statistics overflowed");
    }
    Ok((mean, population_std, sample_std))
}

#[test]
fn signed_cloud_to_mesh_statistics_match_cloudcompare() {
    let dir = golden("cloudcompare/c2m_c2c");
    let compared = read_input(&dir.join("compared.xyz"));
    let reference = read_input(&dir.join("reference.obj"));
    let theirs = read_scalar_cloud(&dir.join("c2m.asc"));
    let reference_distances: Vec<f64> = theirs.iter().map(|(_, d)| *d).collect();
    let (reference_mean, reference_population_std, reference_sample_std) =
        signed_statistics(&reference_distances).expect("finite nonempty reference distances");

    let reg = vernon_cli::registry();
    let op = reg.get("clean.mesh_distance").expect("node registered");
    let mut params = Params::from_specs(&op.info().params);
    params.set("signed", ParamValue::Bool(true));
    params.set("max_dist", ParamValue::Float(0.0));
    let out = op
        .cook(&context("distance_stats"), &[Some(Data::geometry(compared.clone())), Some(Data::geometry(reference))], &params)
        .unwrap();
    let geometry = out[0].as_geometry().expect("distance geometry");
    let ours = geometry.point_attribs.floats("dist").expect("dist attribute");
    assert_eq!(ours.len(), compared.points.len(), "one distance per input point");
    assert_same_scalars(ours, &compared, &theirs, CLOUD_TOL_MM, "C2M statistics input");
    let (array_mean, array_population_std, array_sample_std) = signed_statistics(ours).expect("finite nonempty Vernon distances");

    // Array coverage: signed mean and sample std satisfy the estimator convention
    // in alignment-deviation.md section 3.5. Population std is supplementary only.
    for (name, actual, expected) in [
        ("array signed mean", array_mean, reference_mean),
        ("array sample std (N-1)", array_sample_std, reference_sample_std),
        ("array population std (N)", array_population_std, reference_population_std),
    ] {
        assert!((actual - expected).abs() <= CLOUD_STATS_TOL_MM, "{name}: Vernon {actual} mm, reference {expected} mm");
    }

    // Report coverage: real output port 1, not a report reconstructed from ours.
    // distance.rs::deviation_report calls stats::moments(None): sample std (N-1).
    let report = out[1].as_report().expect("distance report");
    for (key, expected) in [("valid_points", ours.len() as f64), ("invalid_points", 0.0)] {
        let metric = report.metrics.get(key).unwrap_or_else(|| panic!("missing report metric {key}"));
        assert_eq!(metric.unit, "points", "{key} unit");
        assert_eq!(metric.value, expected, "{key}: all fixture points must be valid");
    }
    for (key, expected) in [("mean_dev", reference_mean), ("std_dev", reference_sample_std)] {
        let metric = report.metrics.get(key).unwrap_or_else(|| panic!("missing report metric {key}"));
        assert_eq!(metric.unit, "mm", "{key} unit");
        assert!(metric.value.is_finite(), "nonfinite report metric {key}");
        assert!(
            (metric.value - expected).abs() <= CLOUD_STATS_TOL_MM,
            "report {key}: Vernon {} mm, reference reduction {expected} mm",
            metric.value
        );
    }
    // No claim about CloudCompare's UI statistics convention: the recorded
    // evidence is its per-point scalar export, reduced explicitly above.
}

#[test]
fn signed_cloud_to_mesh_distance_matches_cloudcompare() {
    let dir = golden("cloudcompare/c2m_c2c");
    let compared = read_input(&dir.join("compared.xyz"));
    let reference = read_input(&dir.join("reference.obj"));
    let ours = cook_distance("clean.mesh_distance", compared.clone(), reference, &[("signed", ParamValue::Bool(true))]);
    let worst = assert_same_scalars(&ours, &compared, &read_scalar_cloud(&dir.join("c2m.asc")), CLOUD_TOL_MM, "C2M");
    println!("C2M: worst difference {worst:.2e} mm");
}

#[test]
fn nearest_neighbour_cloud_distance_matches_cloudcompare() {
    let dir = golden("cloudcompare/c2m_c2c");
    let compared = read_input(&dir.join("compared.xyz"));
    let reference = read_input(&dir.join("reference.xyz"));
    let ours = cook_distance("clean.cloud_distance", compared.clone(), reference, &[("local_model", ParamValue::Str("none".into()))]);
    let worst = assert_same_scalars(&ours, &compared, &read_scalar_cloud(&dir.join("c2c.asc")), CLOUD_TOL_MM, "C2C");
    println!("C2C: worst difference {worst:.2e} mm");
}

#[test]
fn catmull_clark_level_2_matches_blender() {
    let dir = golden("blender/subd_catmull_clark");
    let cage = read_input(&dir.join("input.obj"));
    let ours = subd(&cage, 2, "level");
    assert_eq!(ours.prims.len(), read_count(&dir.join("refined_faces.txt")), "face count");
    let worst = assert_same_points(&ours.points, &read_points(&dir.join("refined.txt")), BLENDER_TOL_MM, "refined");
    println!("refined: worst vertex difference {worst:.2e} mm");
}

#[test]
fn catmull_clark_limit_surface_matches_blender() {
    let dir = golden("blender/subd_catmull_clark");
    let cage = read_input(&dir.join("input.obj"));
    let ours = subd(&cage, 2, "limit");
    assert_eq!(ours.prims.len(), read_count(&dir.join("limit_faces.txt")), "face count");
    let worst = assert_same_points(&ours.points, &read_points(&dir.join("limit.txt")), BLENDER_TOL_MM, "limit");
    println!("limit: worst vertex difference {worst:.2e} mm");
}

#[test]
fn signed_statistics_preserve_sign_and_distinguish_divisors() {
    // Mean -1; squared deviations sum to 8. Absolute-value reduction is wrong.
    let (mean, population_std, sample_std) = signed_statistics(&[-3.0, -1.0, 1.0]).unwrap();
    assert!((mean + 1.0).abs() < 1e-12);
    assert!((population_std - (8.0f64 / 3.0).sqrt()).abs() < 1e-12);
    assert!((sample_std - 2.0).abs() < 1e-12);
}

#[test]
fn signed_statistics_constant_and_singleton_have_zero_spread() {
    assert_eq!(signed_statistics(&[-4.0, -4.0, -4.0]).unwrap(), (-4.0, 0.0, 0.0));
    assert_eq!(signed_statistics(&[-4.0]).unwrap(), (-4.0, 0.0, 0.0));
}

#[test]
fn signed_statistics_reject_empty_nonfinite_and_overflowing_data() {
    assert!(signed_statistics(&[]).is_err());
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(signed_statistics(&[1.0, invalid]).is_err());
    }
    assert!(signed_statistics(&[-f64::MAX, f64::MAX]).is_err());
}

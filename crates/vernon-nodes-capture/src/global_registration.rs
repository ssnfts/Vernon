//! Bounded native FPFH/RANSAC registration (Rusu et al. 2009).

use std::collections::VecDeque;

use glam::{DMat4, DVec3};
use vernon_core::Geometry;

use crate::linalg::fit_similarity;
use crate::spatial::{plane_fit, voxel_centroids, Grid};
use crate::util::Rng;

#[derive(Clone, Debug)]
pub(crate) struct GlobalSettings {
    pub voxel: f64,
    pub normal_radius_factor: f64,
    pub feature_radius_factor: f64,
    pub dist_factor: f64,
    pub sample_size: usize,
    pub iterations: usize,
    pub confidence: f64,
    pub seed: u64,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            voxel: 5.0,
            normal_radius_factor: 2.0,
            feature_radius_factor: 5.0,
            dist_factor: 1.5,
            sample_size: 3,
            iterations: 100_000,
            confidence: 0.999,
            seed: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GlobalOutcome {
    pub transform: DMat4,
    pub fitness: f64,
    pub rmse: f64,
    pub correspondences: usize,
    pub feature_matches: usize,
    pub iterations: usize,
    pub source_features: usize,
    pub target_features: usize,
}

const MAX_INPUT_POINTS: usize = 2_000_000;
const MAX_SUPPORT_POINTS: usize = 20_000;
const MAX_KEYPOINTS: usize = 2_048;
const MAX_ITERATIONS: usize = 1_000_000;
const MAX_GEOMETRIC_HYPOTHESES: usize = 4_096;

type Histogram = [f32; 33];

struct Features {
    support: Vec<DVec3>,
    points: Vec<DVec3>,
    descriptors: Vec<Histogram>,
}

fn center_and_thin(g: &Geometry, voxel: f64) -> Result<(Vec<DVec3>, DVec3), String> {
    if g.points.len() < 12 || g.points.len() > MAX_INPUT_POINTS {
        return Err(format!(
            "feature registration needs 12..={MAX_INPUT_POINTS} input points per scan"
        ));
    }
    if g.points.iter().any(|p| !p.is_finite()) {
        return Err("feature registration needs finite source points".into());
    }
    let b = g.bounds();
    let span = b.max - b.min;
    if !span.is_finite()
        || !span.length_squared().is_finite()
        || b.min.abs().max(b.max.abs()).max_element() / voxel > 1e12
    {
        return Err("source coordinates exceed supported feature arithmetic".into());
    }
    let center = b.min + span * 0.5;
    let local: Vec<_> = g.points.iter().map(|p| *p - center).collect();
    let points = voxel_centroids(&local, voxel);
    if points.len() < 12 || points.len() > MAX_SUPPORT_POINTS {
        return Err(format!("requested voxel produces {} support points; supported range is 12..={MAX_SUPPORT_POINTS}; choose an appropriate voxel", points.len()));
    }
    Ok((points, center))
}

fn neighbors(grid: &Grid, points: &[DVec3], index: usize, radius: f64, cap: usize) -> Vec<u32> {
    let mut ids = Vec::new();
    grid.radius_query(points, points[index], radius, &mut ids);
    ids.retain(|&j| {
        j as usize != index && (points[j as usize] - points[index]).length_squared() > 1e-24
    });
    ids.sort_unstable_by(|&a, &b| {
        (points[a as usize] - points[index])
            .length_squared()
            .total_cmp(&(points[b as usize] - points[index]).length_squared())
            .then_with(|| a.cmp(&b))
    });
    ids.truncate(cap);
    ids
}

/// PCA signs are arbitrary. Propagate relative signs through each normal
/// neighbourhood component; descriptor matching handles an overall sign flip.
fn oriented_normals(points: &[DVec3], radius: f64) -> Vec<DVec3> {
    let grid = Grid::build(points, radius);
    let neighborhoods: Vec<_> = (0..points.len())
        .map(|i| neighbors(&grid, points, i, radius, 30))
        .collect();
    let mut normals: Vec<_> = neighborhoods
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mut ids = n.clone();
            ids.push(i as u32);
            plane_fit(points, &ids)
                .filter(|(_, lambda)| ids.len() >= 6 && lambda[1] > 1e-8 * lambda[2].max(1e-30))
                .map_or(DVec3::ZERO, |(normal, _)| normal)
        })
        .collect();
    let mut seen = vec![false; points.len()];
    let mut queue = VecDeque::new();
    for root in 0..points.len() {
        if seen[root] || normals[root] == DVec3::ZERO {
            continue;
        }
        seen[root] = true;
        queue.push_back(root);
        while let Some(i) = queue.pop_front() {
            for &j in &neighborhoods[i] {
                let j = j as usize;
                if seen[j] || normals[j] == DVec3::ZERO {
                    continue;
                }
                if normals[i].dot(normals[j]) < 0.0 {
                    normals[j] = -normals[j];
                }
                seen[j] = true;
                queue.push_back(j);
            }
        }
    }
    normals
}

fn pair_feature(p: DVec3, q: DVec3, mut a: DVec3, mut b: DVec3) -> Option<[f64; 3]> {
    let mut direction = (q - p).normalize_or_zero();
    if direction == DVec3::ZERO || a == DVec3::ZERO || b == DVec3::ZERO {
        return None;
    }
    // Canonical pair ordering is rotation invariant and unchanged by reversing
    // every normal. It avoids giving a pair two unrelated local frames.
    if a.dot(direction).abs() > b.dot(direction).abs() {
        std::mem::swap(&mut a, &mut b);
        direction = -direction;
    }
    let v = a.cross(direction).normalize_or_zero();
    if v.length_squared() < 0.5 {
        return None;
    }
    let w = a.cross(v);
    Some([
        v.dot(b).clamp(-1.0, 1.0),
        a.dot(direction).clamp(-1.0, 1.0),
        w.dot(b).atan2(a.dot(b)),
    ])
}

fn bin(value: f64, low: f64, high: f64) -> usize {
    (((value - low) / (high - low) * 11.0).floor() as usize).min(10)
}

fn normalize(hist: &mut Histogram) {
    for base in [0, 11, 22] {
        let block=&mut hist[base..base+11];
        let sum: f32 = block.iter().sum();
        if sum > 0.0 {
            for x in block {
                *x /= sum;
            }
        }
    }
}

fn spfh(points: &[DVec3], normals: &[DVec3], neighborhoods: &[Vec<u32>], i: usize) -> Histogram {
    let mut h = [0.0; 33];
    if normals[i] == DVec3::ZERO {
        return h;
    }
    for &j in &neighborhoods[i] {
        let j = j as usize;
        if let Some([a, p, t]) = pair_feature(points[i], points[j], normals[i], normals[j]) {
            h[bin(a, -1.0, 1.0)] += 1.0;
            h[11 + bin(p, -1.0, 1.0)] += 1.0;
            h[22 + bin(t, -std::f64::consts::PI, std::f64::consts::PI)] += 1.0;
        }
    }
    normalize(&mut h);
    h
}

fn descriptors(points: &[DVec3], normals: &[DVec3], radius: f64) -> (Vec<DVec3>, Vec<Histogram>) {
    let grid = Grid::build(points, radius);
    let neighborhoods: Vec<_> = (0..points.len())
        .map(|i| neighbors(&grid, points, i, radius, 100))
        .collect();
    let local: Vec<_> = (0..points.len())
        .map(|i| spfh(points, normals, &neighborhoods, i))
        .collect();
    let stride = points.len().div_ceil(MAX_KEYPOINTS);
    let mut selected = Vec::new();
    let mut output = Vec::new();
    for i in (0..points.len()).step_by(stride) {
        if neighborhoods[i].len() < 5 || local[i].iter().all(|x| *x == 0.0) {
            continue;
        }
        let mut h = local[i];
        let k = neighborhoods[i].len() as f64;
        // Published FPFH: own SPFH plus (1/k) times inverse-distance weighted
        // neighbouring SPFHs. Per-block L1 normalisation is our convention.
        for &j in &neighborhoods[i] {
            let j = j as usize;
            let weight = (1.0 / (k * (points[i] - points[j]).length())) as f32;
            for (x, &y) in h.iter_mut().zip(&local[j]) {
                *x += weight * y;
            }
        }
        normalize(&mut h);
        selected.push(points[i]);
        output.push(h);
    }
    (selected, output)
}

fn features(support: Vec<DVec3>, s: &GlobalSettings) -> Result<Features, String> {
    let normals = oriented_normals(&support, s.voxel * s.normal_radius_factor);
    let (points, descriptors) = descriptors(&support, &normals, s.voxel * s.feature_radius_factor);
    if descriptors.len() < 12 {
        return Err(
            "too few valid local shape features: increase neighbourhood radii or scan density"
                .into(),
        );
    }
    let mean: Vec<f64> = (0..33)
        .map(|k| descriptors.iter().map(|h| h[k] as f64).sum::<f64>() / descriptors.len() as f64)
        .collect();
    let variance: f64 = descriptors
        .iter()
        .map(|h| {
            h.iter()
                .zip(&mean)
                .map(|(x, m)| (*x as f64 - m).powi(2))
                .sum::<f64>()
        })
        .sum::<f64>()
        / descriptors.len() as f64;
    if variance < 1e-7 {
        return Err("scan is featureless at the requested radius; local histograms do not distinguish a rigid pose".into());
    }
    Ok(Features {
        support,
        points,
        descriptors,
    })
}

fn descriptor_distance(a: &Histogram, b: &Histogram) -> f32 {
    let mut same = 0.0;
    let mut flipped = 0.0;
    for k in 0..33 {
        same += (a[k] - b[k]).powi(2);
        // A simultaneous reversal of both pair normals keeps alpha and
        // reverses phi/theta for the documented u x direction frame.
        let j = if k < 11 {
            k
        } else {
            (k / 11) * 11 + 10 - k % 11
        };
        flipped += (a[k] - b[j]).powi(2);
    }
    same.min(flipped)
}

fn matches(source: &Features, target: &Features) -> Vec<(usize, usize)> {
    let mut forward = vec![(usize::MAX, f32::INFINITY); source.points.len()];
    let mut backward = vec![(usize::MAX, f32::INFINITY); target.points.len()];
    for (i, a) in source.descriptors.iter().enumerate() {
        for (j, b) in target.descriptors.iter().enumerate() {
            let d = descriptor_distance(a, b);
            if d < forward[i].1 {
                forward[i] = (j, d);
            }
            if d < backward[j].1 {
                backward[j] = (i, d);
            }
        }
    }
    forward
        .iter()
        .enumerate()
        .filter_map(|(i, &(j, _))| (j < backward.len() && backward[j].0 == i).then_some((i, j)))
        .collect()
}

fn edge_consistent(
    source: &Features,
    target: &Features,
    pairs: &[(usize, usize)],
    sample: &[usize],
) -> bool {
    for (k, &a) in sample.iter().enumerate() {
        for &b in &sample[..k] {
            let (sa, ta) = pairs[a];
            let (sb, tb) = pairs[b];
            let x = source.points[sa].distance(source.points[sb]);
            let y = target.points[ta].distance(target.points[tb]);
            if x < 1e-9 || y < 1e-9 || x.min(y) < 0.9 * x.max(y) {
                return false;
            }
        }
    }
    true
}

fn inliers(
    source: &Features,
    target: &Features,
    pairs: &[(usize, usize)],
    m: DMat4,
    gate2: f64,
) -> (Vec<usize>, f64) {
    let mut ids = Vec::new();
    let mut sum = 0.0;
    for (k, &(i, j)) in pairs.iter().enumerate() {
        let d = m
            .transform_point3(source.points[i])
            .distance_squared(target.points[j]);
        if d <= gate2 {
            ids.push(k);
            sum += d;
        }
    }
    let rms = (sum / ids.len().max(1) as f64).sqrt();
    (ids, rms)
}

fn fit(
    source: &Features,
    target: &Features,
    pairs: &[(usize, usize)],
    sample: &[usize],
) -> Option<DMat4> {
    let a: Vec<_> = sample.iter().map(|&k| source.points[pairs[k].0]).collect();
    let b: Vec<_> = sample.iter().map(|&k| target.points[pairs[k].1]).collect();
    fit_similarity(&a, &b, false)
        .ok()
        .map(|m| m.to_mat4())
        .filter(|m| m.is_finite())
}

fn geometric_quality(
    source: &Features,
    target: &Features,
    grid: &Grid,
    m: DMat4,
    gate: f64,
) -> (usize, f64) {
    let mut count = 0;
    let mut sq = 0.0;
    for p in &source.points {
        if let Some((_, d)) = grid.nearest(&target.support, m.transform_point3(*p), gate) {
            count += 1;
            sq += d;
        }
    }
    (
        count,
        if count > 0 {
            (sq / count as f64).sqrt()
        } else {
            f64::INFINITY
        },
    )
}

/// `(geometric inliers, geometric RMS, feature-pair inliers)`.
fn better_pose(candidate: (usize, f64, usize), incumbent: (usize, f64, usize)) -> bool {
    candidate.0 > incumbent.0 || (candidate.0 == incumbent.0 && candidate.1 < incumbent.1)
}

pub(crate) fn global_register(
    src: &Geometry,
    tgt: &Geometry,
    s: &GlobalSettings,
) -> Result<GlobalOutcome, String> {
    if [
        s.voxel,
        s.normal_radius_factor,
        s.feature_radius_factor,
        s.dist_factor,
    ]
    .iter()
    .any(|x| !x.is_finite() || *x <= 0.0)
        || !s.confidence.is_finite()
        || s.confidence <= 0.0
        || s.confidence >= 1.0
        || !(3..=10).contains(&s.sample_size)
        || s.iterations == 0
        || s.iterations > MAX_ITERATIONS
        || s.normal_radius_factor > 50.0
        || s.feature_radius_factor > 50.0
        || s.dist_factor > 100.0
    {
        return Err(format!(
            "invalid feature-registration settings; iteration limit is {MAX_ITERATIONS}"
        ));
    }
    let (sp, sc) = center_and_thin(src, s.voxel)?;
    let (tp, tc) = center_and_thin(tgt, s.voxel)?;
    let source = features(sp, s)?;
    let target = features(tp, s)?;
    let pairs = matches(&source, &target);
    if pairs.len() < s.sample_size.max(6) {
        return Err("too few mutual FPFH matches for a verified pose".into());
    }
    let gate = s.voxel * s.dist_factor;
    if !gate.is_finite() || !gate.powi(2).is_finite() {
        return Err("match gate exceeds finite arithmetic".into());
    }
    let gate2 = gate * gate;
    let mut rng = Rng::new(s.seed);
    let mut best: Option<(DMat4, usize, f64)> = None;
    let target_grid = Grid::build(&target.support, gate);
    let mut geometric_hypotheses = 0;
    let mut required = s.iterations;
    let mut executed = 0;
    while executed < required {
        executed += 1;
        let mut sample = Vec::with_capacity(s.sample_size);
        while sample.len() < s.sample_size {
            let k = rng.below(pairs.len());
            if !sample.contains(&k) {
                sample.push(k);
            }
        }
        if !edge_consistent(&source, &target, &pairs, &sample) {
            continue;
        }
        let Some(mut m) = fit(&source, &target, &pairs, &sample) else {
            continue;
        };
        let (mut ids, mut rms) = inliers(&source, &target, &pairs, m, gate2);
        if ids.len() < s.sample_size.max(6) {
            continue;
        }
        for _ in 0..3 {
            let Some(candidate) = fit(&source, &target, &pairs, &ids) else {
                break;
            };
            let (next, next_rms) = inliers(&source, &target, &pairs, candidate, gate2);
            if next.len() < ids.len() || (next.len() == ids.len() && next_rms >= rms) {
                break;
            }
            m = candidate;
            ids = next;
            rms = next_rms;
        }
        geometric_hypotheses += 1;
        if geometric_hypotheses > MAX_GEOMETRIC_HYPOTHESES {
            return Err(format!("ambiguous features exceeded {MAX_GEOMETRIC_HYPOTHESES} geometric hypotheses; use a more distinctive overlap"));
        }
        let (count, geometric_rmse) = geometric_quality(&source, &target, &target_grid, m, gate);
        if count < 12 || (count as f64 / source.points.len() as f64) < 0.15 {
            continue;
        }
        if best
            .as_ref()
            .is_some_and(|(_, b, r)| !better_pose((count, geometric_rmse, ids.len()), (*b, *r, 0)))
        {
            continue;
        }
        let probability = (ids.len() as f64 / pairs.len() as f64).powi(s.sample_size as i32);
        if probability > 0.0 {
            let bound = if probability >= 1.0 {
                1.0
            } else {
                (1.0 - s.confidence).ln() / (-probability).ln_1p()
            };
            if bound.is_finite() {
                required = required.min((bound.ceil() as usize).max(executed));
            }
        }
        best = Some((m, count, geometric_rmse));
    }
    let (m, count, rmse) = best.ok_or(
        "no nondegenerate feature pose survived the bounded RANSAC search and overlap check",
    )?;
    let fitness = count as f64 / source.points.len() as f64;
    let transform = DMat4::from_translation(tc) * m * DMat4::from_translation(-sc);
    if !transform.is_finite() {
        return Err("feature pose exceeds finite transform arithmetic".into());
    }
    Ok(GlobalOutcome {
        transform,
        fitness,
        rmse,
        correspondences: count,
        feature_matches: pairs.len(),
        iterations: executed,
        source_features: source.points.len(),
        target_features: target.points.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{DQuat, DVec3};

    fn asymmetric_patch() -> Geometry {
        let mut p = Vec::new();
        for ix in 0..46 {
            for iy in 0..33 {
                let x = ix as f64 * 4.0;
                let y = iy as f64 * 4.0;
                let z = 12.0 * (x / 42.0).sin() * (y / 57.0).cos()
                    + 0.0007 * x * y
                    + 8.0 * (-((x - 120.0).powi(2) + (y - 42.0).powi(2)) / 1500.0).exp();
                p.push(DVec3::new(x, y, z));
            }
        }
        Geometry::from_points(p)
    }

    #[test]
    fn wp7_features_find_rotated_partial_asymmetric_cloud() {
        let target = asymmetric_patch();
        let pose = DMat4::from_rotation_translation(
            DQuat::from_euler(glam::EulerRot::XYZ, 0.22, -0.17, 0.35),
            DVec3::new(170.0, -130.0, 80.0),
        );
        let mut source = Geometry::from_points(
            target
                .points
                .iter()
                .copied()
                .filter(|p| p.x > 20.0 && p.y < 116.0)
                .collect(),
        );
        source.transform(pose);
        let s = GlobalSettings {
            voxel: 4.5,
            normal_radius_factor: 3.0,
            feature_radius_factor: 7.0,
            ..GlobalSettings::default()
        };
        let out = global_register(&source, &target, &s).unwrap();
        let expected = pose.inverse();
        let probe = DVec3::new(80.0, 60.0, 10.0);
        let dt = (out.transform.transform_point3(pose.transform_point3(probe)) - probe).length();
        let (_, a, _) = out.transform.to_scale_rotation_translation();
        let (_, b, _) = expected.to_scale_rotation_translation();
        let angle = (a * b.inverse()).to_axis_angle().1.to_degrees().abs();
        eprintln!(
            "WP7 coarse partial dt={dt} angle={angle} fitness={} rmse={} matches={} iters={}",
            out.fitness, out.rmse, out.feature_matches, out.iterations
        );
        assert!(dt < 6.0 && angle < 4.0, "coarse pose dt={dt} angle={angle}");
        assert!(out.fitness > 0.7 && out.source_features > 30 && out.target_features > 30);
        let again = global_register(&source, &target, &s).unwrap();
        assert_eq!(out.transform, again.transform);
        assert_eq!(out.iterations, again.iterations);
    }

    #[test]
    fn wp7_global_rejects_bad_and_featureless_requests() {
        let g = asymmetric_patch();
        let mut s = GlobalSettings {voxel:f64::NAN,..GlobalSettings::default()};
        assert!(global_register(&g, &g, &s).is_err());
        s = GlobalSettings::default();
        s.confidence = 1.0;
        assert!(global_register(&g, &g, &s).is_err());
        let flat = Geometry::from_points(
            (0..30)
                .flat_map(|x| (0..30).map(move |y| DVec3::new(x as f64 * 4.0, y as f64 * 4.0, 0.0)))
                .collect(),
        );
        assert!(global_register(&flat, &flat, &GlobalSettings::default()).is_err());
        let mut nonfinite = g.clone();
        nonfinite.points[0].x = f64::INFINITY;
        assert!(global_register(&nonfinite, &g, &GlobalSettings::default()).is_err());
    }

    #[test]
    fn wp7_fpfh_is_rotation_invariant_and_normal_sign_aware() {
        let cloud = asymmetric_patch();
        let normals = oriented_normals(&cloud.points, 14.0);
        let (points, h) = descriptors(&cloud.points, &normals, 28.0);
        let r = DQuat::from_euler(glam::EulerRot::XYZ, 0.37, -0.24, 0.61);
        let moved: Vec<_> = cloud
            .points
            .iter()
            .map(|p| r * (*p) + DVec3::new(120.0, -47.0, 83.0))
            .collect();
        let turned: Vec<_> = normals.iter().map(|n| r * (*n)).collect();
        let (_, hr) = descriptors(&moved, &turned, 28.0);
        let reversed: Vec<_> = normals.iter().map(|n| -*n).collect();
        let (_, hf) = descriptors(&cloud.points, &reversed, 28.0);
        assert_eq!(points.len(), h.len());
        assert_eq!(h.len(), hr.len());
        assert_eq!(h.len(), hf.len());
        let max_rotation = h
            .iter()
            .zip(&hr)
            .map(|(a, b)| descriptor_distance(a, b))
            .fold(0.0_f32, f32::max);
        let max_flip = h
            .iter()
            .zip(&hf)
            .map(|(a, b)| descriptor_distance(a, b))
            .fold(0.0_f32, f32::max);
        eprintln!("WP7 descriptor max squared change rotation={max_rotation} flip={max_flip}");
        // Distances tied on a regular lattice can exchange the capped neighbour
        // order after rotation. Histograms remain close, rather than bit-equal.
        assert!(max_rotation < 0.015, "descriptor change {max_rotation}");
        assert!(max_flip < 1e-10, "normal flip change {max_flip}");
    }

    #[test]
    fn wp7_geometric_overlap_outvotes_more_false_feature_pairs() {
        let cloud = asymmetric_patch();
        let source = Features {
            support: cloud.points.clone(),
            points: cloud.points.clone(),
            descriptors: Vec::new(),
        };
        let target = Features {
            support: cloud.points.clone(),
            points: cloud.points,
            descriptors: Vec::new(),
        };
        let grid = Grid::build(&target.support, 6.0);
        let correct = geometric_quality(&source, &target, &grid, DMat4::IDENTITY, 6.0);
        let false_pose = geometric_quality(
            &source,
            &target,
            &grid,
            DMat4::from_translation(DVec3::new(190.0, 0.0, 0.0)),
            6.0,
        );
        assert_eq!(correct.0, source.points.len());
        assert_eq!(false_pose.0, 0);
        // Repeated descriptors can give the wrong hypothesis more feature
        // matches; original geometry is the independent pose-validation vote.
        assert!(better_pose(
            (correct.0, correct.1, 15),
            (false_pose.0, false_pose.1, 20)
        ));
    }
}

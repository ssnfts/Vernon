//! Information-weighted SE(3) pose graph with bounded robust LM.
//!
//! The target-frame residual and line-process objective follow the public
//! registration formulation described in the WP7 measured contract.

use glam::{DMat3, DMat4, DQuat, DVec3};
use nalgebra::{DMatrix, DVector, SMatrix, SVector};

type Matrix6 = SMatrix<f64, 6, 6>;
type Vector6 = SVector<f64, 6>;
type NormalizedSystem = (DMatrix<f64>, DVector<f64>, DVector<f64>);

#[derive(Clone, Debug)]
pub(crate) struct PoseEdge {
    pub source: usize,
    pub target: usize,
    /// Maps source-local points into target-local coordinates.
    pub transform: DMat4,
    /// Information for a left increment in the target coordinate frame.
    pub information: [f64; 36],
    pub uncertain: bool,
}

#[derive(Debug)]
pub(crate) struct PoseGraphOutcome {
    pub poses: Vec<DMat4>,
    pub weights: Vec<f64>,
    pub initial_cost: f64,
    pub final_cost: f64,
    pub iterations: usize,
    pub pruned_edges: usize,
}

pub(crate) fn optimize_pose_graph(
    initial: &[DMat4],
    edges: &[PoseEdge],
    reference: usize,
    prune: f64,
    gate_mm: f64,
) -> Result<PoseGraphOutcome, String> {
    let n = initial.len();
    if !(2..=6).contains(&n) || reference >= n || edges.is_empty() || edges.len() > 64 {
        return Err("pose graph needs 2 to 6 poses, 1 to 64 edges and a valid reference".into());
    }
    if !(prune.is_finite() && (0.0..=1.0).contains(&prune) && gate_mm.is_finite() && gate_mm > 0.0)
    {
        return Err(
            "prune must be in [0,1] and the pose-graph gate must be positive and finite".into(),
        );
    }
    for m in initial {
        validate_rigid(*m)?;
    }
    let mut checked = Vec::with_capacity(edges.len());
    for e in edges {
        if e.source >= n || e.target >= n || e.source == e.target {
            return Err("pose-graph edge has invalid or equal scan indices".into());
        }
        validate_rigid(e.transform)?;
        let info = validate_information(&e.information)?;
        // A squared translational gate times mean translational information has
        // the same units as r^T Omega r. This per-edge scale is fixed throughout
        // both optimization passes, not adapted to the residual being judged.
        let mu = gate_mm.powi(2) * (info[(3, 3)] / 3.0 + info[(4, 4)] / 3.0 + info[(5, 5)] / 3.0);
        if !mu.is_finite() || mu <= 0.0 {
            return Err(
                "edge information and gate do not define a finite translation scale".into(),
            );
        }
        checked.push(CheckedEdge {
            source: e.source,
            target: e.target,
            measurement: e.transform,
            info,
            uncertain: e.uncertain,
            mu,
        });
    }
    let mut keep = vec![true; edges.len()];
    check_connected(n, reference, &checked, &keep)?;
    let initial_cost = objective(initial, &checked, &keep)?;
    let mut poses = initial.to_vec();
    let mut iterations = optimize(&mut poses, &checked, &keep, reference)?;
    for (k, e) in checked.iter().enumerate() {
        if e.uncertain && edge_weight(e, edge_energy(e, &poses)?) < prune {
            keep[k] = false;
        }
    }
    let pruned_edges = keep.iter().filter(|&&k| !k).count();
    if pruned_edges > 0 {
        check_connected(n, reference, &checked, &keep)?;
        iterations += optimize(&mut poses, &checked, &keep, reference)?;
    }
    // Report the original robust objective before and after, including rejected
    // loops, so these two costs have exactly the same terms and units.
    let final_cost = objective(&poses, &checked, &vec![true; edges.len()])?;
    let weights = checked
        .iter()
        .enumerate()
        .map(|(k, e)| {
            if keep[k] {
                edge_energy(e, &poses).map(|energy| edge_weight(e, energy))
            } else {
                Ok(0.0)
            }
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(PoseGraphOutcome {
        poses,
        weights,
        initial_cost,
        final_cost,
        iterations,
        pruned_edges,
    })
}

struct CheckedEdge {
    source: usize,
    target: usize,
    measurement: DMat4,
    info: Matrix6,
    uncertain: bool,
    mu: f64,
}

fn validate_rigid(m: DMat4) -> Result<(), String> {
    if !m.is_finite()
        || m.x_axis.w.abs() > 1e-9
        || m.y_axis.w.abs() > 1e-9
        || m.z_axis.w.abs() > 1e-9
        || (m.w_axis.w - 1.0).abs() > 1e-9
    {
        return Err("pose graph needs finite affine rigid transforms".into());
    }
    let r = DMat3::from_mat4(m);
    if !(r.transpose() * r).abs_diff_eq(DMat3::IDENTITY, 1e-8)
        || (r.determinant() - 1.0).abs() > 1e-8
    {
        return Err("pose graph rejects scaling, shear and reflection".into());
    }
    Ok(())
}

fn validate_information(values: &[f64; 36]) -> Result<Matrix6, String> {
    if values.iter().any(|v| !v.is_finite()) {
        return Err("edge information must be finite".into());
    }
    let a = Matrix6::from_row_slice(values);
    let scale = a.iter().fold(0.0_f64, |s, v| s.max(v.abs()));
    if scale == 0.0 || (a - a.transpose()).iter().any(|v| v.abs() > scale * 1e-9) {
        return Err("edge information must be nonzero and symmetric".into());
    }
    // Half before adding: even a finite positive diagonal close to MAX must
    // not overflow before eigendecomposition. All eigensolves below operate
    // on a bounded, coordinate-normalized matrix and have an iteration limit.
    let a = a * 0.5 + a.transpose() * 0.5;
    if (0..6).any(|i| a[(i, i)] < 0.0) {
        return Err("edge information has a negative diagonal".into());
    }
    // Normalize by each observed coordinate's diagonal before deciding rank:
    // rotational and translational columns have different physical units.
    let mut normalized = a;
    for i in 0..6 {
        for j in 0..6 {
            if a[(i, i)] == 0.0 || a[(j, j)] == 0.0 {
                if a[(i, j)] != 0.0 {
                    return Err("zero information diagonal has a nonzero coupled row".into());
                }
                normalized[(i, j)] = 0.0;
            } else {
                normalized[(i, j)] = (a[(i, j)] / a[(i, i)].sqrt()) / a[(j, j)].sqrt();
            }
        }
    }
    if normalized.iter().any(|v| !v.is_finite()) {
        return Err("information normalization exceeds finite arithmetic".into());
    }
    let eigen = nalgebra::SymmetricEigen::try_new(normalized, 1e-12, 4096)
        .ok_or("information eigensolve exceeded its iteration budget")?
        .eigenvalues;
    // PSD is checked after coordinate normalization: a huge rotation block
    // must not hide a negative translation or a zero-diagonal coupling.
    if eigen.iter().any(|&v| v < -1e-10) {
        return Err("edge information must be positive semidefinite".into());
    }
    if eigen.iter().filter(|&&v| v > 1e-9).count() < 3 {
        return Err("edge information constrains fewer than three independent motions".into());
    }
    Ok(a)
}

fn cross_matrix(v: DVec3) -> DMat3 {
    DMat3::from_cols(
        DVec3::new(0.0, v.z, -v.y),
        DVec3::new(-v.z, 0.0, v.x),
        DVec3::new(v.y, -v.x, 0.0),
    )
}

fn se3_exp(x: &Vector6) -> DMat4 {
    let w = DVec3::new(x[0], x[1], x[2]);
    let v = DVec3::new(x[3], x[4], x[5]);
    let theta2 = w.length_squared();
    let (a, b) = if theta2 < 1e-8 {
        (
            0.5 - theta2 / 24.0 + theta2 * theta2 / 720.0,
            1.0 / 6.0 - theta2 / 120.0 + theta2 * theta2 / 5040.0,
        )
    } else {
        let t = theta2.sqrt();
        ((1.0 - t.cos()) / theta2, (t - t.sin()) / (theta2 * t))
    };
    let skew = cross_matrix(w);
    let trans = (DMat3::IDENTITY + skew * a + skew * skew * b) * v;
    DMat4::from_rotation_translation(DQuat::from_scaled_axis(w), trans)
}

fn se3_log(m: DMat4) -> Vector6 {
    let mut q = DQuat::from_mat3(&DMat3::from_mat4(m)).normalize();
    if q.w < 0.0 {
        q = -q;
    }
    let xyz = DVec3::new(q.x, q.y, q.z);
    let sin_half = xyz.length();
    let w = if sin_half < 1e-10 {
        xyz * 2.0
    } else {
        xyz * (2.0 * sin_half.atan2(q.w) / sin_half)
    };
    let theta2 = w.length_squared();
    let c = if theta2 < 1e-8 {
        1.0 / 12.0 + theta2 / 720.0 + theta2 * theta2 / 30240.0
    } else {
        let t = theta2.sqrt();
        let half = t * 0.5;
        (1.0 - half * half.cos() / half.sin()) / theta2
    };
    let skew = cross_matrix(w);
    let trans = (DMat3::IDENTITY - skew * 0.5 + skew * skew * c) * m.w_axis.truncate();
    Vector6::from_row_slice(&[w.x, w.y, w.z, trans.x, trans.y, trans.z])
}

fn residual(e: &CheckedEdge, poses: &[DMat4]) -> Vector6 {
    // Target-j tangent, consistent with information built from points in j.
    se3_log(poses[e.target].inverse() * poses[e.source] * e.measurement.inverse())
}

fn energy(e: &CheckedEdge, r: &Vector6) -> Result<f64, String> {
    let value = r.dot(&(e.info * r));
    if !value.is_finite() {
        return Err("pose-graph residual exceeds finite arithmetic".into());
    }
    Ok(value.max(0.0))
}

fn edge_energy(e: &CheckedEdge, poses: &[DMat4]) -> Result<f64, String> {
    energy(e, &residual(e, poses))
}
fn edge_weight(e: &CheckedEdge, energy: f64) -> f64 {
    if !e.uncertain {
        return 1.0;
    }
    let switch = if energy <= e.mu {
        1.0 / (1.0 + energy / e.mu)
    } else {
        let ratio = e.mu / energy;
        ratio / (1.0 + ratio)
    };
    switch.powi(2)
}

fn objective(poses: &[DMat4], edges: &[CheckedEdge], keep: &[bool]) -> Result<f64, String> {
    let mut sum = 0.0;
    for (e, &active) in edges.iter().zip(keep) {
        if active {
            let value = edge_energy(e, poses)?;
            sum += if e.uncertain {
                if value <= e.mu {
                    value / (1.0 + value / e.mu)
                } else {
                    e.mu / (1.0 + e.mu / value)
                }
            } else {
                value
            };
        }
    }
    if !sum.is_finite() {
        return Err("pose-graph objective exceeds finite arithmetic".into());
    }
    Ok(sum)
}

fn check_connected(
    n: usize,
    reference: usize,
    edges: &[CheckedEdge],
    keep: &[bool],
) -> Result<(), String> {
    let mut seen = vec![false; n];
    seen[reference] = true;
    for _ in 0..n {
        for (e, &active) in edges.iter().zip(keep) {
            if active && (seen[e.source] || seen[e.target]) {
                seen[e.source] = true;
                seen[e.target] = true;
            }
        }
    }
    if seen.iter().any(|&v| !v) {
        Err("pose graph is disconnected from its fixed reference".into())
    } else {
        Ok(())
    }
}

fn normal_equations(
    poses: &[DMat4],
    edges: &[CheckedEdge],
    keep: &[bool],
    reference: usize,
) -> Result<(DMatrix<f64>, DVector<f64>), String> {
    let size = (poses.len() - 1) * 6;
    let mut h = DMatrix::zeros(size, size);
    let mut g = DVector::zeros(size);
    for (e, &active) in edges.iter().zip(keep) {
        if active {
            let r = residual(e, poses);
            let weight = edge_weight(e, energy(e, &r)?);
            let mut jac = DMatrix::zeros(6, size);
            for node in [e.source, e.target] {
                if node != reference {
                    let base = if node < reference {
                        node * 6
                    } else {
                        (node - 1) * 6
                    };
                    for axis in 0..6 {
                        let epsilon = if axis < 3 { 1e-6 } else { 1e-4 };
                        let mut step = Vector6::zeros();
                        step[axis] = epsilon;
                        let mut plus = poses.to_vec();
                        plus[node] = se3_exp(&step) * poses[node];
                        let mut minus = poses.to_vec();
                        minus[node] = se3_exp(&(-step)) * poses[node];
                        let derivative =
                            (residual(e, &plus) - residual(e, &minus)) / (2.0 * epsilon);
                        for row in 0..6 {
                            jac[(row, base + axis)] = derivative[row];
                        }
                    }
                }
            }
            let weighted = e.info * weight;
            h += jac.transpose() * weighted * &jac;
            g += jac.transpose() * weighted * r;
        }
    }
    if h.iter().chain(g.iter()).any(|v| !v.is_finite()) {
        return Err("pose-graph linearization exceeds finite arithmetic".into());
    }
    Ok((h, g))
}

fn normalized_system(
    h: &DMatrix<f64>,
    g: &DVector<f64>,
) -> Result<NormalizedSystem, String> {
    let n = g.len();
    let mut scale = DVector::zeros(n);
    for i in 0..n {
        if h[(i, i)] <= 0.0 {
            return Err("pose graph has an unobservable motion".into());
        }
        scale[i] = 1.0 / h[(i, i)].sqrt();
    }
    let mut normalized = h.clone();
    let mut gradient = g.clone();
    for i in 0..n {
        gradient[i] *= scale[i];
        for j in 0..n {
            normalized[(i, j)] = (h[(i, j)] * scale[i]) * scale[j];
        }
    }
    if normalized
        .iter()
        .chain(gradient.iter())
        .any(|v| !v.is_finite())
    {
        return Err("normalized pose graph exceeds finite arithmetic".into());
    }
    let eigen = nalgebra::SymmetricEigen::try_new(normalized.clone(), 1e-12, 4096)
        .ok_or("pose-graph observability eigensolve exceeded its iteration budget")?
        .eigenvalues;
    let largest = eigen.iter().fold(0.0_f64, |a, &b| a.max(b));
    if eigen.iter().any(|&v| v <= largest * 1e-10) {
        return Err("pose graph has an unobservable or ill-conditioned motion".into());
    }
    Ok((normalized, gradient, scale))
}

fn optimize(
    poses: &mut Vec<DMat4>,
    edges: &[CheckedEdge],
    keep: &[bool],
    reference: usize,
) -> Result<usize, String> {
    let mut lambda = 1e-3;
    let mut cost = objective(poses, edges, keep)?;
    for iteration in 0..60 {
        let (h, g) = normal_equations(poses, edges, keep, reference)?;
        let (normalized, gradient, scale) = normalized_system(&h, &g)?;
        if gradient.amax() < 1e-9 {
            return Ok(iteration);
        }
        let mut accepted = None;
        for _ in 0..10 {
            let mut damped = normalized.clone();
            for i in 0..gradient.len() {
                damped[(i, i)] += lambda;
            }
            if let Some(step) = damped.cholesky().map(|factor| factor.solve(&(-&gradient))) {
                let mut delta = step.component_mul(&scale);
                if delta.iter().all(|v| v.is_finite()) {
                    for node in 0..poses.len() {
                        if node != reference {
                            let base = if node < reference {
                                node * 6
                            } else {
                                (node - 1) * 6
                            };
                            let angle =
                                DVec3::new(delta[base], delta[base + 1], delta[base + 2]).length();
                            let shift =
                                DVec3::new(delta[base + 3], delta[base + 4], delta[base + 5])
                                    .length();
                            let factor = (0.25 / angle.max(0.25)).min(100.0 / shift.max(100.0));
                            for axis in 0..6 {
                                delta[base + axis] *= factor;
                            }
                        }
                    }
                    for backtrack in 0..8 {
                        let step_factor = 0.5_f64.powi(backtrack);
                        let mut candidate = poses.clone();
                        for node in 0..poses.len() {
                            if node != reference {
                                let base = if node < reference {
                                    node * 6
                                } else {
                                    (node - 1) * 6
                                };
                                let d = Vector6::from_fn(|i, _| delta[base + i] * step_factor);
                                candidate[node] = se3_exp(&d) * poses[node];
                            }
                        }
                        if let Ok(next) = objective(&candidate, edges, keep) {
                            if next < cost {
                                accepted = Some((candidate, next));
                                break;
                            }
                        }
                    }
                }
            }
            if accepted.is_some() {
                break;
            }
            lambda = (lambda * 10.0).min(1e12);
        }
        let Some((candidate, next)) = accepted else {
            return Ok(iteration);
        };
        let improvement = cost - next;
        *poses = candidate;
        cost = next;
        lambda = (lambda * 0.3).max(1e-12);
        if improvement < 1e-12 * (1.0 + cost) {
            return Ok(iteration + 1);
        }
    }
    Ok(60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> [f64; 36] {
        let mut a = [0.0; 36];
        for i in 0..6 {
            a[i * 7] = if i < 3 { 2500.0 } else { 1.0 };
        }
        a
    }

    fn edge(source: usize, target: usize, truth: &[DMat4], uncertain: bool) -> PoseEdge {
        PoseEdge {
            source,
            target,
            transform: truth[target].inverse() * truth[source],
            information: info(),
            uncertain,
        }
    }

    fn truths() -> Vec<DMat4> {
        vec![
            DMat4::IDENTITY,
            DMat4::from_rotation_translation(
                DQuat::from_euler(glam::EulerRot::XYZ, 0.2, -0.1, 0.3),
                DVec3::new(50.0, 10.0, -5.0),
            ),
            DMat4::from_rotation_translation(
                DQuat::from_euler(glam::EulerRot::XYZ, -0.15, 0.24, -0.18),
                DVec3::new(100.0, -25.0, 16.0),
            ),
        ]
    }

    #[test]
    fn wp7_pose_noncommuting_drift_converges_with_exact_fixed_reference() {
        let truth = truths();
        let drift = DMat4::from_rotation_translation(
            DQuat::from_scaled_axis(DVec3::new(0.01, -0.02, 0.03)),
            DVec3::new(1.4, -2.0, 0.8),
        );
        let initial = vec![truth[0], drift * truth[1], drift * drift * truth[2]];
        let edges = vec![
            edge(1, 0, &truth, false),
            edge(2, 1, &truth, false),
            edge(2, 0, &truth, true),
        ];
        let out = optimize_pose_graph(&initial, &edges, 0, 0.25, 10.0).unwrap();
        assert_eq!(out.poses[0], initial[0]);
        for (got, want) in out.poses.iter().zip(&truth) {
            assert!(got.abs_diff_eq(*want, 2e-6), "pose {got:?} != {want:?}");
        }
        assert!(out.final_cost < out.initial_cost * 1e-8);
        assert!(out.iterations > 0 && out.iterations <= 120);
        assert_eq!(out.pruned_edges, 0);
        let again = optimize_pose_graph(&initial, &edges, 0, 0.25, 10.0).unwrap();
        assert_eq!(again.poses, out.poses);
    }

    #[test]
    fn wp7_pose_rejects_bad_closure_and_uses_retained_information() {
        let truth = truths();
        let mut edges = vec![
            edge(1, 0, &truth, false),
            edge(2, 1, &truth, false),
            edge(2, 0, &truth, true),
        ];
        edges[2].transform =
            DMat4::from_translation(DVec3::new(2000.0, -1000.0, 600.0)) * edges[2].transform;
        let out = optimize_pose_graph(&truth, &edges, 0, 0.25, 2.0).unwrap();
        assert_eq!(out.pruned_edges, 1);
        assert_eq!(out.weights[2], 0.0);
        assert_eq!(out.weights[0], 1.0);
        for (got, want) in out.poses.iter().zip(&truth) {
            assert!(got.abs_diff_eq(*want, 1e-6));
        }
    }

    #[test]
    fn wp7_pose_validates_rigidity_information_and_connectivity() {
        let truth = truths();
        let good = vec![edge(1, 0, &truth, false), edge(2, 1, &truth, false)];
        assert!(optimize_pose_graph(&truth, &good[..1], 0, 0.25, 2.0).is_err());
        assert!(optimize_pose_graph(&truth, &good, 3, 0.25, 2.0).is_err());
        assert!(optimize_pose_graph(&truth, &good, 0, f64::NAN, 2.0).is_err());
        assert!(optimize_pose_graph(&truth, &good, 0, 0.25, 0.0).is_err());
        let mut bad = good.clone();
        bad[0].information[0] = -1.0;
        assert!(optimize_pose_graph(&truth, &bad, 0, 0.25, 2.0).is_err());
        bad[0].information = [0.0; 36];
        assert!(optimize_pose_graph(&truth, &bad, 0, 0.25, 2.0).is_err());
        bad[0].information = info();
        bad[0].information[1] = 1.0;
        assert!(optimize_pose_graph(&truth, &bad, 0, 0.25, 2.0).is_err());
        bad[0].information = info();
        bad[0].transform = DMat4::from_scale(DVec3::splat(1.2));
        assert!(optimize_pose_graph(&truth, &bad, 0, 0.25, 2.0).is_err());
        let mut bad_pose = truth.clone();
        bad_pose[1].w_axis.x = f64::INFINITY;
        assert!(optimize_pose_graph(&bad_pose, &good, 0, 0.25, 2.0).is_err());
        // A graph connected by rank-three rotation-only information has no
        // constrained translations, even though every individual edge is PSD.
        let mut weak = good;
        for e in &mut weak {
            for i in 3..6 {
                e.information[i * 7] = 0.0;
            }
        }
        assert!(optimize_pose_graph(&truth, &weak, 0, 0.25, 2.0).is_err());
    }

    #[test]
    fn wp7_pose_reference_can_be_nonzero_and_nonidentity() {
        let truth = truths();
        let edges = vec![
            edge(1, 0, &truth, false),
            edge(2, 1, &truth, false),
            edge(2, 0, &truth, true),
        ];
        let mut initial = truth.clone();
        initial[0] = DMat4::from_translation(DVec3::new(0.8, 1.2, -0.3)) * truth[0];
        initial[2] = DMat4::from_rotation_x(0.01) * truth[2];
        let out = optimize_pose_graph(&initial, &edges, 1, 0.25, 5.0).unwrap();
        assert_eq!(out.poses[1], initial[1]);
        for (got, want) in out.poses.iter().zip(&truth) {
            assert!(got.abs_diff_eq(*want, 2e-6));
        }
    }

    #[test]
    fn wp7_pose_lie_roundtrip_handles_small_and_near_pi_rotation() {
        for angle in [0.0, 1e-10, 0.2, std::f64::consts::PI - 1e-8] {
            let axis = DVec3::new(0.3, -0.5, 0.7).normalize() * angle;
            let x = Vector6::from_row_slice(&[axis.x, axis.y, axis.z, 13.0, -20.0, 3.0]);
            let transform = se3_exp(&x);
            assert!(se3_exp(&se3_log(transform)).abs_diff_eq(transform, 1e-8));
        }
    }

    #[test]
    fn wp7_pose_robust_cost_keeps_its_finite_saturation_at_large_scale() {
        let edge = CheckedEdge {
            source: 1,
            target: 0,
            measurement: DMat4::from_translation(DVec3::new(-1.26e154, 0.0, 0.0)),
            info: Matrix6::identity(),
            uncertain: true,
            mu: 2.5e307,
        };
        let poses = vec![DMat4::IDENTITY; 2];
        let e = edge_energy(&edge, &poses).unwrap();
        let expected = edge.mu / (1.0 + edge.mu / e);
        let expected_weight = (1.0 / (1.0 + e / edge.mu)).powi(2);
        assert!((edge_weight(&edge, e) / expected_weight - 1.0).abs() < 1e-12);
        assert!((objective(&poses, &[edge], &[true]).unwrap() / expected - 1.0).abs() < 1e-12);
        let scaled_info = info().map(|v| v * 1e200);
        assert!(validate_information(&scaled_info).is_ok());
        assert!(expected_weight > 0.01);
    }

    #[test]
    fn wp7_pose_indefinite_information_cannot_hide_behind_units_or_other_edges() {
        let truth = truths();
        let initial = &truth[..2];
        let mut valid = edge(1, 0, initial, false);
        for i in 3..6 {
            valid.information[i * 7] = 10.0;
        }
        let mut invalid = valid.clone();
        for i in 0..3 {
            invalid.information[i * 7] = 1e12;
        }
        invalid.information[3 * 7] = -0.01;
        assert!(optimize_pose_graph(initial, &[valid.clone(), invalid], 0, 0.25, 2.0).is_err());
        let mut zero_coupled = valid.clone();
        for i in 0..3 {
            zero_coupled.information[i * 7] = 1e12;
        }
        zero_coupled.information[3 * 7] = 0.0;
        zero_coupled.information[3 * 6 + 4] = 0.01;
        zero_coupled.information[4 * 6 + 3] = 0.01;
        assert!(
            optimize_pose_graph(initial, &[valid.clone(), zero_coupled], 0, 0.25, 2.0).is_err()
        );
        let mut coupled = valid.clone();
        for i in 0..3 {
            coupled.information[i * 7] = 1e12;
        }
        for i in 3..6 {
            coupled.information[i * 7] = 1.0;
        }
        coupled.information[3 * 6 + 4] = 2.0;
        coupled.information[4 * 6 + 3] = 2.0;
        assert!(optimize_pose_graph(initial, &[valid, coupled], 0, 0.25, 2.0).is_err());
        let mut maximum = [0.0; 36];
        for i in 0..6 {
            maximum[i * 7] = f64::MAX;
        }
        assert!(validate_information(&maximum).is_ok());
    }
}

# Native scan registration: WP7 contract

Implementation contract, 2026-10-29, for `capture.global_align` and
`capture.multiway_align`. This supersedes the draft registration status and
method arrangements in [capture-scanning.md](capture-scanning.md) for these
two nodes. Focused acceptance and the full workspace checks passed, and GPU
launch was inspected; the local package commit records this checkpoint.

All lengths are millimetres; matrices act on column vectors and fitted
transforms are proper rigid motions. The implementation uses native Rust and
`nalgebra` under the repository's permissive dependency rules. No external
program, proprietary installation or copyleft algorithm source is used.

## Coordinate frames and compatibility

`T_ij` maps scan i into scan j. `X_i` maps scan i into the reference frame.
A consistent edge satisfies `X_i = X_j * T_ij`; predicted relative motion is
`inverse(X_j) * X_i`. Global Align returns source-to-target motion without
fitting scale. Multiway Align fixes the selected input scan at exact identity.

Global Align retains inputs `source`, `target`; outputs, in order, are placed
source geometry, transform JSON and Report. JSON contains the existing
`transform` matrix representation and `information`. With ICP refinement,
information is the actual final-correspondence 6x6 point-to-point information
`sum G^T G`, `G = [-[q]_x | I]`, in the target frame; the fitted ICP objective
and reported refined residual are point-to-plane. It is not a calibrated
measurement covariance or the point-to-plane Hessian. Without refinement,
information is null rather than a fabricated matrix.

Multiway retains required `scan_1`, `scan_2` and optional `scan_3` through
`scan_6`. Outputs remain merged geometry, transforms JSON and Report. JSON
adds `scan_slots` (zero-based original input slots) and `edges` beside the
ordered `transforms` array. `reference` is a zero-based original input slot;
it must be connected. Missing optional slots are compacted internally but
preserved in `scan_slots`. No explicit user-edge input is implemented.

Placed geometry uses the existing transform behavior. Merged scans retain
normal/colour attributes through the existing merge route, carry `scan`
point tags and optionally voxel-resample point clouds. Source meshes are
concatenated rather than reconstructed into one new surface. Registration
matches point positions and estimated shape; source `id` or normal attributes
do not provide hidden correspondence or pose truth.

## Global Align parameters

| Parameter | Default | Declared range / meaning |
|---|---:|---|
| `method` | `fpfh_ransac` | Existing choices `fpfh_ransac`, `fgr`; `fgr` explicitly errors |
| `voxel` | 5 mm | 0.05–50 mm; finest ICP scale; feature candidates use v, 2v, 4v |
| `normal_r` | 2 | 0.5–20 times each feature voxel; also finest-ICP normal radius factor |
| `fpfh_r` | 5 | 1–50 times each feature voxel |
| `dist` | 1.5 | 0.5–20 times each feature voxel; coarse geometric inlier gate |
| `ransac_n` | 3 | 3–10 distinct correspondence pairs per hypothesis |
| `iters` | 100,000 | UI range 100–10,000,000; kernel rejects requests above 1,000,000 |
| `conf` | 0.999 | 0.5–0.999999; stochastic confidence stopping |
| `refine` | true | Refine every viable coarse candidate with point-to-plane ICP |
| `seed` | 0 | Nonnegative integer; identical input ordering/settings reproduce output |

`fpfh_ransac` is implemented. The reserved `fgr` choice returns an explicit
unsupported-method error; it does not alias RANSAC or principal-axis alignment.
Unknown methods fail. Declared UI ranges and finite kernel budgets differ
where stated; exceeding a kernel budget is an error, not silent clamping.

## Feature kernel and explicit pyramid

This is Vernon's bounded arrangement of published registration methods, not
an output-equivalence claim for any external library.

1. Centre each cloud about its bounds and voxel-downsample at the requested
   feature level. Use local PCA normals with up to 30 noncoincident neighbours.
   A normal requires at least six fitting points and a nondegenerate local
   plane. Propagate relative normal signs through each normal-neighbour graph.
2. Compute actual 33-component FPFH. Canonically order each point/normal pair
   by the absolute normal/direction dot products. With normalized pair
   direction d, use u=n_source, normalized v=u cross d, w=u cross v. Reject
   parallel/degenerate frames. Bin alpha=v dot n_target and phi=u dot d into
   separate 11-bin intervals on [-1,1], and theta=atan2(w dot n_target,
   u dot n_target) into 11 bins on [-pi,pi]. Each SPFH block sums to one.
3. Add `(1/k) * sum(SPFH(neighbour)/distance)` to the query SPFH using at most
   100 feature neighbours, then normalize each 11-bin block to sum one.
   Distances are in mm. This convention is not scale invariant. Simultaneous
   pair-normal reversal leaves alpha unchanged and reverses phi/theta; matching
   uses the smaller squared descriptor distance with or without those two
   blocks reversed. This handles an overall sign ambiguity, not every
   disconnected/nonorientable normal configuration.
4. Select at most 2,048 evenly spaced keypoint candidates from support order.
   Too few valid descriptors or descriptor variance below 1e-7 fails. Mutual
   nearest-descriptor matches use stable first-index ties. Repeated, almost
   flat or symmetric geometry can still be ambiguous.
5. Seeded RANSAC draws distinct mutual pairs, rejects repeated target samples,
   rank-deficient fits and corresponding edges outside a 0.9 length ratio,
   then fits a proper rigid least-squares motion. At most three local refits
   use geometric feature-pair inliers. Every viable hypothesis is separately
   checked against nearest target-support geometry; independent geometric
   inlier count, then RMS, ranks candidates. At least 12 geometric inliers and
   fitness 0.15 are required. Feature-pair inlier share supplies the confidence
   stopping rule only after geometric validation; it is not a proof of global
   optimality.
6. The node adapter explicitly tries all three feature voxels `[v, 2v, 4v]`.
   When refinement is enabled, every viable candidate is refined at the same
   finest user voxel v using existing point-to-plane ICP, coarse factor 2,
   up to 60 iterations per stage, overlap 0.5, no normal-angle rejection and
   normal radius `normal_r * v`. A viable refined result needs fitness >=0.2
   and finite RMSE. Choose the smallest `RMSE / max(fitness,0.05)` across the
   levels, rather than stopping at the first successful larger-scale fit.
   With `refine=false`, the same score uses coarse geometric measurements.
   If all levels fail, return an explicit error; failed ICP does not silently
   promote the coarse pose as a refined success.

The pyramid was necessary in measured smooth-panel cases: the strict kernel
can find indistinguishable 11-bin features at fine v even with adequate point
count, while coarser neighbourhoods reveal shape. It changes feature scale
explicitly and reports `feature_voxel_mm`, retaining the user's fine ICP
scale. Some coarse hypotheses can refine to a wrong pose; comparing final
quality reduces this risk without certifying uniqueness.

## Global report and budgets

Coarse `fitness` and `coarse_rmse_mm` use selected valid source keypoints
against all target support points, not all raw source points. `refine=false`
returns this Euclidean nearest-geometry RMSE as `rmse_mm`; with refinement,
`rmse_mm` is the final existing ICP point-to-plane inlier RMSE on voxel clouds.
Legacy `inlier_rmse`, ICP iteration/condition metrics and warnings remain when
refinement runs. `start_mean_distance` separately samples raw source points
against raw target positions at the chosen coarse pose.

Other metrics are `feature_matches`, `source_features`, `target_features`,
`hypotheses`, `coarse_correspondences`, `coarse_fitness`, `feature_voxel_mm`
and `feature_scale_attempts` (three). These feature counters describe the
selected level, not the sum of discarded level searches. No zero-inlier
success or placeholder warning is emitted.

Per feature level, each cloud requires 12–2,000,000 raw points and 12–20,000
support points after that exact voxel. There are at most 2,048 keypoint
candidates, 30 normal neighbours and 100 feature neighbours. At most
1,000,000 RANSAC attempts and 4,096 geometrically evaluated hypotheses are
allowed; excessive ambiguity/budget requests fail. Absolute coordinate/voxel
ratio above 1e12, nonfinite/extreme arithmetic, inadequate features, matches
or overlap also fail. Centering and downsampling do not promise exact
rotation/order-invariant voxel boundaries or matching outcomes.

## Multiway Align parameters and pairwise graph

| Parameter | Default | Declared range / meaning |
|---|---:|---|
| `voxel` | 5 mm | 0.05–50 mm; finest pairwise sampling scale |
| `coarse` | 15 | 1–100 times voxel; coarse correspondence gate |
| `fine` | 1.5 | 0.5–20 times voxel; final correspondence gate |
| `prune` | 0.25 | 0–1; uncertain-edge line-process weight threshold |
| `solver` | `lm` | Levenberg–Marquardt only |
| `reference` | 0 | Original input slot 0–5; must be present |
| `auto_loops` | true | Attempt all pairs; false attempts consecutive present scans |
| `dedupe_voxel` | 1 mm | 0–50 mm; zero retains merged points |

Coarse/fine gates must be positive finite and coarse >=fine. Resampling must
be finite/nonnegative. With loops enabled, at most all 15 unique pairs of six
scans are attempted. With loops disabled, only consecutive scans in the
compacted present-scan order are tried; holes in original slots do not create
an artificial disconnected chain. Pair initialization uses the explicit
three-level FPFH pyramid, with a deterministic pair seed; then existing
point-to-plane ICP uses the requested fine voxel, fine gate, coarse/fine
factor, at most 60 iterations per stage, overlap 0.5 and no normal-angle gate.
Accepted pairs require finite RMSE and fitness >=0.2 and carry the actual
final-correspondence information described above.

A deterministic maximum-fitness spanning tree initializes the reference
poses. Its edges become certain weight-one constraints; other accepted pairs
are uncertain robust edges. This tree may differ from consecutive-scan
odometry. Disconnected accepted graphs fail instead of returning unregistered
parts. A wrong high-fitness tree edge can corrupt the solution; robust loop
pruning is not a guarantee that the backbone itself is correct.

## Information-weighted SE(3) solve

The edge residual is
`r_ij = log((inverse(X_j) * X_i) * inverse(T_ij))`
in the target-j left-increment tangent, with rotation first (radians) and
translation second (mm). Its information has the matching frame. True SE(3)
exp/log include translation Jacobians; reference pose remains exact.

For `e = r^T Omega r`, each uncertain edge uses the fixed scale
`mu = (fine * voxel)^2 * trace(Omega_translation)/3`, eliminated robust cost
`mu*e/(mu+e)` and line weight `l=(mu/(mu+e))^2`. Stable ratio branches avoid
overflow. This is a Vernon adaptation of the published line-process prior
`mu*(sqrt(l)-1)^2`; mu is not fitted to whichever residual is being judged.
Certain tree edges retain ordinary quadratic cost and weight one.

LM jointly updates all free poses. Central numerical Jacobians use 1e-6 rad
and 1e-4 mm left perturbations. Before damping, the gauge-fixed normalized
Hessian must have minimum eigenvalue greater than maximum times 1e-10.
Per-edge information must be finite symmetric (relative tolerance 1e-9),
nonnegative on its diagonal, zero off-diagonal wherever a diagonal is zero,
and PSD after coordinate normalization (negative eigenvalue tolerance 1e-10)
with at least three eigenvalues above 1e-9. This catches indefinite data that
a large rotational unit scale might otherwise hide. Eigensolves allow at most
4,096 iterations; unsupported arithmetic rejects.

Each solve allows 60 LM iterations, ten damping trials per iteration and
at most eight half-step backtracks; increments are bounded to 0.25 rad and
100 mm per iteration. Accepted steps lower the actual robust objective with
recomputed switches. After the first solve, uncertain edges below `prune`
are removed, connectivity/observability rechecked, and the retained graph is
solved for at most 60 more iterations. `prune=0` retains uncertain edges but
still downweights them. The private kernel supports 2–6 poses / 1–64 edges;
the public node generates no more than 15 unique edges and 30 free variables.

Reported initial/final graph costs include the same original robust terms,
including pruned loops, to preserve comparable units and membership. Removed
edge confidence is zero. A finite best local state is returned when no further
decrease is found; this does not claim global convergence. The optimization
is nonconvex and does not prove physical registration or feature uniqueness.

## Multiway output measurements

`edges`, `retained_edges`, `pruned_edges`, `rejected_pairs`,
`graph_initial_cost`, `graph_final_cost` and `graph_iterations` describe the
actual graph and solve. Costs have information-weighted mixed units and are
not labelled mm. Each JSON edge records original `source_slot`, `target_slot`,
measured relative `transform`, `information`, `uncertain` and final `confidence`.
Per-edge fitness/RMSE report keys use one-based slot labels for readability.

After optimization, retained relative poses are remeasured on the requested
final voxel clouds without another pose update. `fitness` is the unweighted
mean of these retained-edge source-inlier shares; `rmse_mm` is the
correspondence-count-weighted aggregate of their point-to-plane squared
residuals. `worst_edge_rmse` is the maximum such edge RMSE. Lost pairwise
overlap (fewer than six final correspondences) fails. Merged geometry uses
these same optimized poses. `points_out` counts the actual merged result.
The node does not report a separate loop-gap distance; no synthetic zero
closure metric is supplied when loops are absent.

## Recorded focused acceptance — 2026-10-29

Actual saved `wp7-integration-random.log` ran both CLI integration tests:
two passed, zero failed/ignored, 26.77 s. The required fixture uses the real
`geo.car_panel` built with 120 columns / 80 rows, five different overlapping
rectangular crops, independent Gaussian coordinate noise sigma 0.02 mm and
independently shuffled point order. Pose seed 21841 generates arbitrary-axis
rigid motions, angles from 10 to 30 degrees and translation magnitudes from
40 to 200 mm; the last scan reaches exactly 30 degrees / 200 mm. The first
scan fixes the identity reference. Node voxel is 8 mm and merge resampling is
disabled. Geometry contains no ids or hidden pose attributes.

| Scan (zero-based) | Translation error (mm) | Rotation error (degrees) |
|---|---:|---:|
| 0, fixed reference | 0 | 0 |
| 1 | 0.003853578 | 0.003947855 |
| 2 | 0.005211290 | 0.003085431 |
| 3 | 0.035682964 | 0.005842080 |
| 4 | 0.023396106 | 0.001136010 |

Every recovered pose satisfies the HANDOFF limit <=0.5 mm and <=0.5 degree.
The report measured `rmse_mm=0.030022946528821624` and
`fitness=0.8625538794822465`. Pose truth is used only by the test's assertions,
not by feature matching or graph construction. Crops share samples from the
same procedural panel grid before independent noise/shuffling; this is not
a test of every possible scanner sampling pattern or arbitrary overlap.

`wp7-capture-green.log` records 80 capture tests passed, zero failed/ignored.
The focused kernel suite contains four global and seven pose tests, including
independent geometry-based hypothesis ranking, Lie roundtrip near zero/pi,
fixed/nondefault references, contradictory loop rejection, robust finite-scale
costs and information PSD regressions. A node case with slots [0,1,4], reference
slot 4 and loops disabled verifies slot mapping and chaining behavior. Coarse
feature-only tests measure a weaker pose accuracy than the final handoff;
refinement is necessary for the recorded fine acceptance. No universally
identifiable pose, exact external-library equivalence or arbitrary-noise
success is claimed.

Full detached run `wp7-final-20261029-a` completed all five checks at
2026-10-29 10:12:45 UTC: workspace build, 1,131 tests passed / zero failed /
eight ignored, all-target Clippy with warnings denied, 36-node demo with
zero failures / two expected gates, and matching generated catalogue.
Before/after source fingerprint:
`e50c8d7e9de3f508e6d3c2155e7cdc61f8f692fd902b283121963d9bec7adaed`.
The GPU app opened using Vulkan / 4x MSAA on the RTX 4090 Laptop GPU;
viewports and the Capture ribbon were inspected, then it closed normally.
Final independent kernel and adapter reviews have no remaining blocker.

## Public sources consulted

These sources describe algorithms and observable interfaces. No implementation
source was copied or used as a template. All prose here is independently
written; defaults and validation arrangements are Vernon's choices unless
specifically attributed.

- Rusu, Blodow, Beetz, *Fast Point Feature Histograms (FPFH) for 3D
  Registration*, ICRA 2009, [DOI](https://doi.org/10.1109/ROBOT.2009.5152473),
  [public paper](https://www.cvl.iis.u-tokyo.ac.jp/class2016/2016w/papers/6.3DdataProcessing/Rusu_FPFH_ICRA2009.pdf).
  Initial paper retrieval succeeded; later slice requests timed out. The
  official [FPFH theory](https://pointclouds.org/documentation/tutorials/fpfh_estimation.html)
  and [PFH theory](https://pointclouds.org/documentation/tutorials/pfh_estimation.html)
  supplied accessible angular/histogram definitions. No library source was read.
- Choi, Zhou, Koltun, *Robust Reconstruction of Indoor Scenes*, CVPR 2015,
  [paper record](https://www.cv-foundation.org/openaccess/content_cvpr_2015/html/Choi_Robust_Reconstruction_of_2015_CVPR_paper.html)
  and [author supplementary material](https://qianyi.info/docs/papers/cvpr15_loopclosure_supp.pdf),
  Appendix B. The full paper exceeded the retrieval service's file-size limit;
  its abstract, indexed technical text and public supplement were consulted.
  The pose-level weighted SE(3) arrangement above is a Vernon adaptation.
- Zhou, Park, Koltun, *Fast Global Registration*, ECCV 2016,
  [author paper](https://qianyi.info/docs/papers/eccv16_registration.pdf),
  sections 3.1–3.3 and 4: robust penalty/line-process dual, mutual feature
  matches, tuple checking and joint rigid updates. This is also the reference
  for `fgr` should that optional method be implemented.
- [Official global-registration tutorial](https://www.open3d.org/docs/latest/tutorial/pipelines/global_registration.html)
  supplies the observable voxel/radius/neighbour conventions already present
  in Vernon's earlier capture spec. No external library is linked or run.

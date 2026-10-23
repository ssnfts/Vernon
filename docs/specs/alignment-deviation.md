# Spec: alignment and deviation (stage 2, "clean / align / measure")

Clean-room spec. Sources are public docs, forum answers and papers listed in section 6; wording is ours.
Reference tools are named here for documentation only; they never appear in UI strings (see `CLEAN_ROOM.md`).
No CloudCompare source code (GPL) was read; behaviour comes from its wiki and from the cited papers.

## 1. Scope and reference tools

Goal: take a scan (mesh or point cloud) and a reference (cleaned mesh or CAD tessellation), bring them into
one frame, measure signed deviation, and emit numbers that a later stage can gate on, for example
"95% of scan points within +/-0.5 mm" (the toolchain note's recommended report form, with the scan as the
compared cloud and the reference as the baseline).

| Tool | Version basis | Used here for |
|---|---|---|
| CloudCompare (GPL-2.0+, separate process only) | wiki pages describing the 2.6.x dialogs, with notes up to 2.13 | point-pair alignment, ICP, cloud/cloud and cloud/mesh distance, colour scales, histogram |
| ZEISS INSPECT (formerly GOM Inspect), commercial | public forum answers and third-party tutorials; the vendor Tech Guide was not reachable | best-fit alignment with a max distance, colour deviation map, legend, tolerance-band statistics |

Out of scope: global multi-scan registration, feature-based automatic registration (FPFH/RANSAC), non-rigid
warping, GD&T datum-reference logic, CAD-to-CAD comparison.

Conventions for Vernon: millimetres, right-handed, f64 internally, coordinates centred on the reference
bounding-box centre before accumulation (an f32 value near 4500 mm has a spacing of about 0.0005 mm, which
is the size of the effect being measured). Deviation sign: positive when the compared point lies on the
outward-normal side of the reference surface (scan proud of nominal), negative when below it.

## 2. Feature catalogue (observable behaviour)

### 2.1 Point-pair coarse alignment (CloudCompare)
- Two entities: a fixed reference and a moving "aligned" entity. The user picks matching points in the
  same order on both; points may be typed in, picked on geometry, or taken as fitted sphere centres.
- Minimum of 3 pairs. The tool previews the result, lets pairs be added or removed, and can reset the pose.
- Output: RMS over pairs, a per-pair error column (used to spot bad picks), a 4x4 matrix, and a report that
  is also written to the console.
- Scale is fixed to 1 by default. Unlocking it estimates a uniform scale and the matrix is no longer rigid.
- Sphere-centre option: rough search radius plus a maximum fit RMS expressed as a percentage of the radius.

### 2.2 ICP refinement (CloudCompare)
| Parameter | Behaviour | Default / range as documented |
|---|---|---|
| Number of iterations | upper bound on iterations | page gives no number |
| RMS difference | stop when the change of registration error between iterations falls under this | page gives no number; below about 1e-8 it reaches 32-bit float precision |
| Final overlap | fraction of the moving cloud expected to overlap the reference; permits partial overlap, "10% or less" | since 2.6.1, percent |
| Adjust scale | estimate a scale factor too (photogrammetry clouds of unknown scale) | off |
| Random sampling limit | cap on points drawn at random from the moving cloud per iteration; above the cloud size means no subsampling | 50 000 points |
| Rotation / translation constraints | lock rotation to a chosen axis, or translation to none/one/several axes | none |
| Farthest point removal | drop the moving points that are too far from the reference at each iteration | off |
| Scalar field as weights | weight either the moving or the reference points by a displayed scalar (not both) | off |
- Roles: reference ("model") never moves; moving ("data") does; a swap button exchanges them.
- The wiki states no defaults for iterations or RMS threshold and no output text; Vernon picks its own
  defaults (section 4) and must verify against a licensed copy in the golden tests.

### 2.3 Cloud-to-cloud distance (CloudCompare)
- Exactly two clouds. The compared cloud receives a scalar field; the reference is the baseline and should
  be the denser and larger one.
- Without a local model the value is the nearest-neighbour distance, which over-reports on sparse or holey
  references. Local models fit a least-squares plane, a Delaunay triangulation of locally projected
  neighbours, or a six-parameter quadric (height function) through the nearest point and its neighbours.
  Neighbourhood = fixed neighbour count (valid for uniform density only) or a sphere radius (the default).
  Since 2.5.2 the result is the smaller of nearest-neighbour and model distance.
- Max distance caps the search; points beyond it get the threshold value instead of a true distance.
- Octree level is automatic and only affects speed. A preliminary "approximate distance" pass exists only
  to pick that level and its statistics are explicitly not measurements.
- Option to split X/Y/Z: three extra fields of absolute per-axis offset to the nearest reference point.
- The result is unsigned; signed and flip-normals options are not available for this tool.

### 2.4 Cloud-to-mesh distance (CloudCompare)
- Distances from compared points (or compared-mesh vertices) to the reference mesh surface.
- Signed option: sign follows the reference triangle normals; flip-normals inverts it for reversed winding.
  The wiki does not describe edges, vertices or ties, so Vernon must choose a rule (section 3.4).
- Max distance, multi-thread and automatic octree level as in 2.3. Split XYZ is not supported.
- A sparse compared mesh should first be resampled (a "sample points on mesh" tool) so vertex spacing does
  not limit the sampling of the surface.

### 2.5 Colour scale and histogram (CloudCompare)
- Seven built-in scales (blue-green-yellow-red, grey, blue-white-red, red-yellow, red-white, intensity 0-1,
  HSV angle) that cannot be edited; custom scales are made by copying.
- A scale is relative (stretches over the field's min..max, positions in percent) or absolute (steps at
  fixed values). Absolute is the documented way to get identical colours across several clouds.
- Display-range controls (forum and tutorial evidence): draggable limit markers; values beyond them show
  as saturated end colours; points outside the displayed range can be greyed; options to force zero into
  the range, to make the range symmetric, and a logarithmic mapping.
- Histogram: bin count changes with the mouse wheel (no default stated); clicking places a marker that reads
  the percentage of points below that bin and its value; bars take the colours of the active scale; export
  to CSV or image (since 2.6.2).
- Statistics: mean and standard deviation; a Gauss or Weibull fit with a chi-squared goodness figure. The
  wiki notes that unsigned distances cannot be fitted with a plain normal law (folded distribution).

### 2.6 Best-fit alignment (ZEISS INSPECT)
- Alignment families: pre-alignment, reference-point (RPS), hierarchical (geometry-element based), local
  best fit, global best fit. Pre-alignment, local and global fit give different deviations on the same data.
- Local best fit minimises the squared distance from scan points to the reference surface. A maximum
  distance decides which points take part: points farther away are ignored for both the transformation and
  the reported alignment deviation. The vendor's forum answer says the value must exceed the expected
  deviations and that values under 1 mm rarely make sense.
- Observed trap: with a 0.01 mm max distance the mesh barely moved and the reported deviation was 0.0038 mm,
  while a 100 times larger figure came out once the fit could converge. A tight cap therefore hides misfit.

### 2.7 Deviation map, legend, statistics (ZEISS INSPECT)
- "Surface comparison" colours the reference (or the scan) by distance. Max distance bounds the search;
  beyond it the surface stays grey. A direction-reverse toggle flips the sign.
- Typical legend: symmetric tolerance such as +/-0.10 widened to +/-0.50 mm; green near nominal, red where
  material is proud of the reference, blue where it is below. Points outside the legend limits were reported
  grey in some threads, so grey means "outside range or no data" there.
- Statistics: a "Statistic" label template lists tolerance bands (lower limit, upper limit in mm, point count,
  percent of points) plus an out-of-range row; values can be exported through a user-defined check.
- Mean distance and standard deviation are not area-weighted; the integrated distance is. Dense triangles
  therefore weigh more in the per-point mean (confirmed only in direction by the vendor reply).
- No percentile report exists in the sources seen; "x% within +/-t" is a Vernon addition built on the bands.

## 3. Algorithms (published methods, implementer detail)

### 3.1 Closed-form rigid fit from paired points (Horn 1987; Arun-Huang-Blostein 1987; Umeyama 1991; Kabsch 1976)
Given pairs `p_i -> q_i` with weights `w_i`:
1. `pc = sum(w p)/sum(w)`, `qc = sum(w q)/sum(w)`; centre both sets.
2. `H = sum(w (p_i-pc)(q_i-qc)^T)`; SVD `H = U S V^T`.
3. `R = V diag(1, 1, det(V U^T)) U^T` (the determinant term prevents a reflection); `t = qc - R pc`.
4. With scale (Umeyama): `s = trace(S D) / sum(w |p_i-pc|^2)` where `D = diag(1,1,det(V U^T))`; `t = qc - s R pc`.
5. RMS `= sqrt(sum(w |s R p_i + t - q_i|^2) / sum(w))`; per-pair error `|s R p_i + t - q_i|`.
Degeneracy: fewer than 3 pairs, or centred `P` with rank under 2 (collinear points) -> error, because rotation
about the line is free. Horn's quaternion form (largest eigenvector of a 4x4 symmetric matrix) is an equivalent
alternative; both must agree to 1e-9 on noiseless data.

### 3.2 ICP loop (Besl-McKay 1992; Chen-Medioni 1991/92; variants per Rusinkiewicz-Levoy 2001)
The six stages of the survey, with the Vernon choice for each:
1. Selection: random subsample of the moving points, redrawn each iteration, capped at `sample_limit`
   (50 000). Optional normal-space sampling (bucket normals by direction, draw evenly) for nearly flat parts
   with small features; the survey shows it is the only variant that converged on its incised-plane scene.
2. Matching: closest point on the reference (k-d tree over points, or closest point on triangle through a
   bounding-volume tree). Compatibility filter: normals within 45 degrees (survey's setting).
3. Weighting: constant by default; optional linear-with-distance, normal-compatibility (`n_p . n_q`), or a
   user scalar. The survey found small, data-dependent effects.
4. Rejection, any of: worst 10% of pair distances; distances over 2.5 sigma of the pair distances;
   `max_pair_dist` hard cap (the best-fit "max distance"); and the trimmed form below.
5. Error metric: point-to-point (closed form, 3.1) or point-to-plane `sum(((R p + t - q) . n_q)^2)`.
6. Minimisation: repeat 2-5 from the current pose until the stopping rule fires. No extrapolation (overshoot).

Point-to-plane, linearised for small angles (no closed form exists): unknown `x = (rx, ry, rz, tx, ty, tz)`;
each pair gives a row `a_i = ((p_i x n_i)^T, n_i^T)` and `b_i = n_i . (q_i - p_i)`. Solve the 6x6 normal
equations `A^T W A x = A^T W b`, rebuild the exact rotation from `(rx, ry, rz)` by Rodrigues' formula, compose
with the running pose, repeat. A condition number over 1e8 means a sliding degeneracy (plane, cylinder,
extrusion): warn and report which singular direction is free.
Constraints (axis lock): delete the matching unknown from the linear system. Scale (optional): append a seventh
unknown or re-estimate `s` with Umeyama on the matched set each iteration.

Trimmed ICP for partial overlap (Chetverikov et al.): with overlap fraction `f`, sort squared pair residuals and
keep the smallest `ceil(f N)` pairs for the minimisation. The CloudCompare "final overlap" is documented only
as an expected overlap percentage; mapping it to trimming is our interpretation and must be tested.
Stopping: iteration cap, or `|rms_k - rms_{k-1}| < rms_delta`; also stop if the update step is below 1e-9 mm and
1e-9 rad. Report the RMS of the pairs used and, separately, the RMS over all moving points to the reference.
Convergence is local: the basin is roughly the feature size of the surface, so an initial pose is mandatory.
Initial pose sources: point pairs (3.1), or principal axes (centroid plus PCA frames, 4 proper-rotation sign
choices, keep the lowest trimmed RMS); principal-axes seeding is mentioned in the survey's introduction as one way to get a start pose.

### 3.3 Distance without sign (cloud-to-cloud)
- Exact nearest neighbour with a k-d tree (Vernon does not need an octree level; results must be equal to or
  slightly smaller than an octree-limited search).
- Local model: gather neighbours of the nearest reference point (radius `r`, or `k`); plane = eigenvector of the
  smallest covariance eigenvalue through the centroid; distance `|n . (x - c)|`; result `min(nn, model)`.
  Quadric: fit `h = a u^2 + b uv + c v^2 + d u + e v + f` in the plane frame by least squares; use `|h(x)|`
  as an approximate distance (flag it as approximate). Triangulation: Delaunay of the projected neighbours,
  then point-to-triangle distance.
- `max_dist`: values beyond it are stored as `max_dist` and the point is flagged invalid (no true value).

### 3.4 Signed distance to a triangle mesh
1. Closest point on triangle by region classification (vertex / edge / face regions, as in Ericson's closest-point
   routines); accelerate with an AABB tree traversed nearest-first and pruned by the best distance so far or
   `max_dist`.
2. Sign = sign of `n . (x - c)`. For a face-interior hit `n` is the face normal. For an edge or vertex hit the
   face normal is ambiguous, so use the angle-weighted pseudo-normal (Baerentzen-Aanaes 2005): edge = sum of
   the two adjacent face normals; vertex = face normals weighted by the interior angle at that vertex. This is
   a Vernon decision because the CloudCompare wiki is silent on edges.
3. Open meshes: sign is only meaningful near a consistently oriented sheet; report the open-edge count.
4. `flip` negates the sign. Winding is taken from the mesh; run a normals check first.

### 3.5 Statistics, tolerance reporting, colouring
- `N` valid samples `d_i` (invalid ones excluded and counted). mean, sample std (N-1), `rms = sqrt(mean d^2)`,
  min, max. Weighting is selectable: per point, or by area share (point weight = mesh area per point).
- Percentile of `|d|` by nearest rank: sort ascending, value at index `ceil(q N)` (1-based). Fraction within
  tolerance: `count(|d| <= t) / N` (inclusive). "95% within +/-t" is true iff `fraction_within(t) >= 0.95`,
  equivalently `P95(|d|) <= t`. Also report the mean beside it because a tail can hide under a good mean.
- Tolerance bands: edges `e_0 < e_1 < ...` -> rows `[e_j, e_{j+1})`, last closed on the right, plus below/above
  rows; each with count and percent.
- Histogram: `B` equal bins over `[lo, hi]` with `idx = floor((d - lo)/(hi - lo) B)`, last bin right-closed;
  use integer micrometre arithmetic in tests to avoid float edge flips; also emit the cumulative percentage
  (what the CloudCompare marker reads off).
- Colour: `t = clamp((d + L)/(2L), 0, 1)` for a symmetric range `[-L, +L]`; piecewise-linear stops
  (Vernon's own palette, blue-cyan-green-yellow-red at 0, .25, .5, .75, 1) so zero sits on green; invalid points
  grey. Symmetric by default so a systematic offset is visible, with options for manual min/max and stepped
  bands (`steps` > 0 quantises `t`).

## 4. Proposed Vernon nodes (stage `clean`, 10 nodes)

Labels are generic. `emulates` strings may name the reference tool; UI strings may not.
Status: **now** = implement with a real algorithm; **wb** = white-box stub first (ports and params correct).

| Id | Label | Inputs | Outputs | Key params (default; range; unit) | Status |
|---|---|---|---|---|---|
| `clean.pair_align` | Pick-Pair Align | `moving` Geometry, `reference` Geometry, `pairs` Json (list of `[p, q]`) | `geometry`, `transform` Json (4x4 + scale), `report` | `fix_scale` (true), `min_pairs` (3; 3-64) | now |
| `clean.auto_prealign` | Auto Pre-Align | `moving`, `reference` | `geometry`, `transform`, `report` | `method` (principal_axes), `sample_limit` (20000; 1000-1e6) | wb |
| `clean.icp_align` | ICP Align | `moving`, `reference` | `geometry`, `transform`, `report` (iterations, rms per iteration, rms all, condition number) | `metric` (point_to_plane; point_to_point/point_to_plane), `max_iterations` (50; 1-1000), `rms_delta` (1e-6; 1e-10-1; mm), `overlap` (1.0; 0.1-1.0), `sample_limit` (50000; 1000-5e6), `max_pair_dist` (0 = off; 0-1e4; mm), `reject` (none; none/worst_pct/sigma), `reject_value` (10 % or 2.5 sigma), `normal_angle` (45; 0-90; deg), `adjust_scale` (false), `lock_rotation`/`lock_translation` (none; any of x,y,z), `fit_mode` (global/local) | now |
| `clean.sample_surface` | Sample Surface | `geometry` (mesh) | `geometry` (points with normals) | `mode` (density/count), `density` (1.0; 0.01-100; points/mm2), `count` (100000), `seed` (1) | now |
| `clean.cloud_distance` | Point Distance | `compared`, `reference` | `geometry` (attrib `dev`, `dev_valid`, optional `dev_x/y/z`), `report` | `max_dist` (0 = off; mm), `local_model` (none; none/plane/quadric/triangulation), `neighbourhood` (radius/count), `radius` (2.0; 0.1-100; mm), `k` (6; 3-64), `split_xyz` (false) | now for none+plane; wb for quadric+triangulation |
| `clean.mesh_distance` | Surface Distance | `compared` (points or mesh), `reference` (mesh) | `geometry` (attrib `dev` signed, `dev_valid`), `report` | `signed` (true), `flip` (false), `max_dist` (0 = off; mm), `pseudonormals` (true) | now |
| `clean.deviation_stats` | Deviation Statistics | `geometry` with `dev` | `report`, `stats` Json (`deviation_report.v1`) | `attrib` (dev), `tolerance` (0.5; 0.001-100; mm), `target_fraction` (0.95; 0.5-1), `weighting` (point/area), `percentiles` (50,90,95,99), `band_edges` (list; mm) | now |
| `clean.deviation_histogram` | Deviation Histogram | `geometry` with `dev` | `histogram` Json (edges, counts, cumulative %), CSV `file` optional | `bins` (100; 4-2000), `range_mode` (symmetric/auto/manual), `limit` (0.5; mm), `export_csv` (false) | now |
| `clean.deviation_colors` | Deviation Colors | `geometry` with `dev` | `geometry` (point colour `Cd`) | `range_mode` (symmetric; symmetric/auto/manual), `limit` (0.5; mm), `scale` (diverging/spectrum/grey), `steps` (0 = smooth; 0-32), `invalid_color` (grey) | now |
| `clean.tolerance_gate` | Tolerance Gate | `stats` Json | `report` (pass/fail, severity) | `tolerance`, `target_fraction`, `fail_on_miss` (false -> Warning, true -> Error) | now |

Report contract `deviation_report.v1` (Json): `{n_valid, n_invalid, mean, std, rms, min, max, abs_percentiles
{p50,p90,p95,p99}, within:[{tol, fraction}], bands:[{lo,hi,count,percent}], below, above, alignment:{rms_before,
rms_after, iterations, transform}, verdict:{tolerance, target_fraction, pass}}`. The surface stage reuses the same
contract for mesh-to-surface checks, so each stage emits numbers the next one can verify.

Graph shape: `scan -> pair_align / auto_prealign -> icp_align -> mesh_distance -> (deviation_stats, deviation_colors,
deviation_histogram) -> tolerance_gate`, with the scan on the compared (`moving`/`compared`) side and the cleaned
reference on the reference side.

## 5. Golden test ideas (numbers are targets to confirm, except the arithmetic ones, which were computed)

1. Closed-form fit, noiseless. Points (0,0,0), (100,0,0), (0,50,0), (0,0,25) mm moved by 10 deg about Z and
   t = (5, -3, 2). Expect R, t within 1e-9 and RMS < 1e-9. The Horn and SVD routes agree to 1e-9.
2. Degenerate input. Two pairs, or three collinear pairs (0,0,0), (1,0,0), (2,0,0): return a typed error, not a pose.
3. Scale. Same points scaled by 1.02 with `fix_scale = false`: recovered scale 1.02 +/- 1e-6; with `fix_scale = true`
   the RMS is strictly larger.
4. ICP, distinct-axis shape. Ellipsoid semi-axes 120 x 60 x 40 mm (a sphere would leave rotation free), 20 000 points,
   moved by 3 deg about (1,2,3)/sqrt(14) and t = (2, -1, 0.5) mm. Point-to-plane: recovered translation within
   0.01 mm, rotation within 0.01 deg, final RMS < 0.005 mm, under 30 iterations.
5. ICP degeneracy. Flat plate 200 x 100 mm with a slide of 5 mm along X: the condition number exceeds 1e8, the report names a free
   direction, and the tool does not claim convergence.
6. Partial overlap. Reference plus a 20% patch displaced 30 mm. `overlap = 0.8` ends with RMS < 0.01 mm; `overlap = 1.0`
   ends above 0.5 mm. This pins down the trimming interpretation.
7. Max-distance trap (the best-fit lesson). Same pair with `max_pair_dist = 0.01` leaves the pose within 0.01 mm of the
   start and reports a tiny RMS; the report must also show the share of points excluded, and flag it above 50%.
8. Unsigned vs modelled distance. Reference plane sampled on a 1 mm grid; compared point 0.3 mm above a cell centre.
   `none` gives 0.768 mm (`sqrt(0.3^2 + 0.5^2 + 0.5^2)`); `plane` model gives 0.300 mm.
9. Signed mesh distance. Two-triangle plate 200 x 200 mm, normal +Z; compared points (10,10,0.3), (-20,5,-0.4), (50,50,0)
   give +0.3, -0.4, 0.0; `flip` negates them.
10. Edge sign. 100 mm cube centred at the origin: point (60,60,0) is 14.142136 mm from the vertical edge, sign +; centre
    (0,0,0) gives -50. Pseudonormals must yield these, a naive face-normal sign is allowed to fail the first.
11. Statistics, arithmetic checked. `|d|` = 0.001, 0.002, ..., 1.000 mm (N = 1000): P50 = 0.500, P95 = 0.950, mean = 0.5005,
    rms = 0.577783, fraction within 0.5 mm = 0.500. The gate at 0.95 within 0.5 mm fails.
12. The report form from the brief. 10 000 samples: 9 500 at 0.2 mm and 500 at 0.8 mm. fraction within 0.5 mm = 0.95 passes
    (inclusive boundary), mean `|d|` = 0.23, rms = 0.26458, P95 = 0.2, P99 = 0.8. With 9 499 and 501 it fails.
13. Histogram edges. The 1000 values of test 11 in 10 bins over [0, 1.0] (integer micrometres): counts
    99, 100, 100, 100, 100, 100, 100, 100, 100, 101 (the first bin lacks 0.000, the last includes 1.000).
14. Colour map. With `limit = 0.5`: d = -0.5 -> t = 0 (blue), 0 -> 0.5 (green), +0.25 -> 0.75 (yellow), +2.0 -> t = 1 (red,
    clamped); invalid -> grey. Swapping `compared` and `reference` flips the sign.
15. Reference-tool cross-check (user-made data only, licensed copy of the reference tool run as a separate
    process): identical inputs, compare mean/std of signed mesh distance within 1e-3 mm and ICP final RMS within 5%.
    Differences from octree approximation or random sampling are documented, not hidden.

## 5a. Node map: stored transforms and mirror symmetry

These three `clean` nodes were added after the first draft of section 4 and were not specified there. Behaviour is
described here from published methods only (Sources, last items).

**`clean.apply_transform`** re-applies a registration that an align node recorded. The second input is the
`transform.v1` JSON record (rotation, translation, uniform scale). A point maps to `x' = s R x + t`; with `invert`
on, the inverse `x = R^T (x' - t) / s` is applied. Stored point and face normals are transformed with the inverse
transpose and renormalised, every other attribute and the topology are untouched. The scale must be finite and
positive, otherwise the cook fails and names the value. Checks: applying a transform and then its inverse returns the
input to 1e-9 mm; applying the record an ICP node wrote to that node's own moving cloud reproduces its output.

**`clean.symmetry_detect`** looks for the plane `n . x = d` that maps the point set onto itself, in the spirit of the
planar-reflective symmetry transform of Podolak et al. and the partial-symmetry formulation of Mitra et al.
Method:

1. Draw up to `samples` points with a seeded shuffle; the first 5000 of them are the probes; a k-d tree holds the
   whole sample.
2. Candidate normals are the three principal axes of the sample and the three coordinate axes (duplicates within
   0.9999 of parallel are dropped), or the single axis named by `axis_hint`. For each normal the offset is searched:
   a coarse scan of 41 offsets over plus and minus 15 % of the bounding-box diagonal around the centroid plane, then
   a finer scan of 21 offsets around the best one.
3. The score of a plane is the trimmed RMS (best `trim` fraction) of the distance from the mirror image of each probe
   to its nearest sample point, so a missing half costs little.
4. The best candidate is refined for up to `refine_iterations` rounds: each probe and its nearest image partner `q`
   form a pair; the new normal is the dominant eigenvector of the scatter of `p - q`, pairs weighted by
   `exp(-(d / (1.5 * median d))^2)` so that the orphans of a missing region barely count; the offset is the weighted
   mean of the pair midpoints on that normal (weights also scale with the squared pair separation so near-plane pairs
   do not dominate). Iteration stops when the plane moves less than 1e-9.

Outputs: the input with the point attribute `mirror_dist` (mm, distance from the mirror image of every point to the
nearest point), the plane as `plane.v1` JSON, and a report with `mirror_rms`, `mirror_p50`, `mirror_p95`,
`symmetric_fraction` (share of points whose image lies within `tolerance`, gated by `min_fraction`), the plane
normal and origin, and the point count. Warnings: the fraction is below the gate (the object may not be symmetric or the
data is one-sided); a second candidate is within 20 % of the best RMS (ambiguous). Fewer than 8 points is an error.
Checks (analytic): a cloud sampled from an ellipsoid with three different semi-axes recovers a coordinate plane to
0.1 degree and 0.05 mm; deleting one octant leaves the normal within 1 degree; a cloud with no symmetry reports a
fraction below the gate.

**`clean.mirror_complete`** reflects data across a plane to fill what a scan missed. The plane comes from the optional
`plane` input, else from `origin` and `normal` (`plane_source = custom`), else it is the bounding-box mid-plane across
`axis`. For a cloud the elements are points, for a mesh they are faces classified by their centroids. The source side
is `positive`, `negative`, or for `auto` the side with more elements (an empty source side is an error). In
`fill_gaps` everything is kept and the mirror image of a source element is added only when no existing point lies
within `gap_radius` of it (0 means twice the median nearest-neighbour spacing). In `replace_side` the source side is
kept and the other side is replaced by its mirror image; a warning is given when both sides hold similar amounts
(the smaller one is discarded). Elements within `weld_distance` of the plane lie on it and are not duplicated. Mirrored
faces get reversed vertex order and mirrored normals, so a closed outward-wound half yields a closed outward-wound
whole. The report counts elements on each side and what was added. Checks: a half box mirrored with `replace_side`
has twice the volume and no open edges; a cloud missing a patch gains points only inside the patch.

Sources for this section:

- Podolak, Shilane, Golovinskiy, Rusinkiewicz, Funkhouser, "A planar-reflective symmetry transform for 3D shapes",
  ACM TOG 25(3), 2006. https://doi.org/10.1145/1141911.1141923
- Mitra, Guibas, Pauly, "Partial and approximate symmetry detection for 3D geometry", ACM TOG 25(3), 2006.
  https://doi.org/10.1145/1141911.1141924

## 6. Sources

CloudCompare wiki (docs only):
- https://www.cloudcompare.org/doc/wiki/index.php/ICP
- https://www.cloudcompare.org/doc/wiki/index.php/Align
- https://www.cloudcompare.org/doc/wiki/index.php/Cloud-to-Cloud_Distance
- https://www.cloudcompare.org/doc/wiki/index.php/Cloud-to-Mesh_Distance
- https://cloudcompare.org/doc/wiki/index.php/Scalar_fields%5CColor_Scales_Manager
- https://cloudcompare.org/doc/wiki/index.php/Scalar_fields%5CShow_histogram
- https://www.cloudcompare.org/doc/wiki/index.php/Scalar_fields%5CCompute_statistical_parameters (search snippet only)
- https://mail.cloudcompare.org/forum/viewtopic.php?p=23935 (colour-scale scaling thread, search snippet only)
- https://dozuki.umd.edu/Guide/Parts+Comparison/153 (comparison workflow; display-range options)

ZEISS INSPECT / GOM Inspect (public forum and third-party material; vendor Tech Guide not reached):
- https://qualityforum.zeiss.com/topic/3487-calculation-of-deviation-in-best-fit-alignment/
- https://qualityforum.zeiss.com/topic/27174-how-are-%E2%80%9Cmean-distance%E2%80%9D-and-%E2%80%9Cdistance-standard-deviation%E2%80%9D-weighted-in-zeiss-inspect-surface-comparison/
- https://qualityforum.zeiss.com/topic/847-outputting-deviation-data-in-a-table-gom-inspect/
- https://qualityforum.zeiss.com/topic/1379-color-legend-problem/ (search snippet only)
- https://www.capture3d.com/knowledge-center/blog/comparing-actual-and-nominal-cad-data-in-gom-software (search snippet only)
- https://mfg.trimech.com/automating-qa-reports-with-zeiss-3d-scanners-and-software/ (search snippet only)

Papers and books:
- Besl, McKay, "A Method for Registration of 3-D Shapes", IEEE TPAMI 14(2):239-256, 1992. https://doi.org/10.1109/34.121791
- Chen, Medioni, "Object modelling by registration of multiple range images", Image and Vision Computing 10(3):145-155,
  1992. https://doi.org/10.1016/0262-8856(92)90066-C (conference form: IEEE ICRA 1991, https://doi.org/10.1109/ROBOT.1991.132043)
- Rusinkiewicz, Levoy, "Efficient Variants of the ICP Algorithm", 3DIM 2001. https://doi.org/10.1109/IM.2001.924423
  (text read from https://www.cs.princeton.edu/~smr/papers/fasticp/fasticp_paper.pdf)
- Horn, "Closed-form solution of absolute orientation using unit quaternions", JOSA A 4(4):629-642, 1987.
  https://doi.org/10.1364/JOSAA.4.000629
- Arun, Huang, Blostein, "Least-squares fitting of two 3-D point sets", IEEE TPAMI 9(5):698-700, 1987.
  https://doi.org/10.1109/TPAMI.1987.4767965
- Umeyama, "Least-squares estimation of transformation parameters between two point patterns", IEEE TPAMI 13(4):376-380,
  1991. https://doi.org/10.1109/34.88573
- Kabsch, "A solution for the best rotation to relate two sets of vectors", Acta Cryst. A32:922-923, 1976.
  https://doi.org/10.1107/S0567739476001873
- Chetverikov, Svirko, Stepanov, Krsek, "The Trimmed Iterative Closest Point Algorithm", ICPR 2002.
  https://doi.org/10.1109/ICPR.2002.1047997
- Baerentzen, Aanaes, "Signed Distance Computation Using the Angle Weighted Pseudonormal", IEEE TVCG 11(3):243-253, 2005.
  https://doi.org/10.1109/TVCG.2005.49
- Ericson, "Real-Time Collision Detection", Morgan Kaufmann, 2004 (closest point on triangle, section 5.1.5).

Project note: `3D printed body kit toolchain.md` (stage 2 row and the verification paragraph) for the report form.

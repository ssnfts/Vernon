# Connected automatic surfacing (WP2)

Implementation contract, 2026-10-26. This describes the native implementation in
`autosurface.rs`, `patch_network.rs`, `fit.rs` and `segment.rs`. The broader workflow and
historical proposals are in [reverse-engineering surfacing](reverse-engineering-surfacing.md).

## Node contracts

`surface.auto_surface` takes `mesh: Geometry` and returns, in order,
`patches: Json`, `preview: Geometry`, `report: Report`. The JSON uses the existing
`nurbs_patch_set` representation, including degree, knot vectors, control grid,
optional weights, region ids and four boundary ids ordered u-min, u-max, v-min,
v-max. The preview carries both `patch_id` and `part`, each equal to the patch id.

| Parameter | Default | Accepted declaration / meaning |
|---|---|---|
| `degree` | 3 | 2–5, both directions |
| `init_cp` | 6 | 4–32, initial control points per direction, adjusted for aspect ratio |
| `max_cp` | 48 | 8–128, fitting ceiling per direction |
| `smoothness` | 0.001 | 0–1, discrete thin-plate fairness weight |
| `tol_mm` | 0.1 | 0.001–5 mm, positive tolerance |
| `extra_samples` | 0 | 0–16 extra points per triangle; internal minimums may add samples |
| `detail` | `medium` | `coarse`, `medium`, `fine` |
| `target_patches` | 0 | Declared 0–5000; execution rejects values above 256 |
| `continuity` | `g0` | `g0`, `g1` |
| `sensitivity` | 55 | 0–100, automatic segmentation |
| `noise_sigma_mm` | 0.05 | 0.0005–5 mm, segmentation noise scale |

The old proposal's `selective_regions` parameter is absent. Select regions with
upstream geometry/region operations. No node-id or parameter aliases are implied
by the historical table.

`surface.refine_regions` takes `regions: Geometry` and optional
`polyline: Geometry`; it returns `regions: Geometry`, `report: Report`.

| Parameter | Default | Range / choices |
|---|---|---|
| `op` | `resegment` | `resegment`, `merge`, `split_by_polyline` |
| `group` | empty | Comma-separated nonnegative existing region ids |
| `sensitivity` | 80 | 0–100 |
| `noise_sigma_mm` | 0.05 | 0.0005–5 mm |
| `min_region_area_pct` | 0.05 | 0–10 percent |
| `cad_tessellation` | false | Boolean |

## Layout and fitting

Existing primitive `region_id` labels are respected, including non-contiguous
ids. Partially assigned negative labels fail cleanly. Without labels, segmentation
uses the sensitivity/noise settings and minimum area fractions 0.05, 0.01 or 0.002
for coarse, medium or fine. A nonzero target caps automatic region merging;
it does not erase supplied labels.

An unlabelled smooth disk uses one whole chart: one boundary loop, a height-field
projection and adjacent triangle normals within 30 degrees. Its boundary chart is
fitted with fixed UV coordinates. The fitted surface is restricted into a full
rectangular grid, choosing factor counts by chart aspect ratio. Defaults produce
1, 4 or 8 patches for coarse, medium or fine; an explicit target sets the grid
count. Prime counts still produce a complete grid, avoiding T-junctions.

A labelled smooth disk whose labelled UV triangles exactly tile rectangles can
also share one fixed-UV chart fit. Each region receives its exact rectangular
restriction. Restrictions use knot insertion and splitting, preserving the
original surface and derivatives; they do not independently refit the pieces.
Other regions are separated into connected components, closed components are
split into charts, and each chart receives a local fit. Every source triangle is
mapped to its chart, including artificial chart interfaces introduced by closed
surface cuts. Missing source coverage is an error.

Fitting solves tensor-product B-spline least squares with a discrete thin-plate
penalty (second differences in each direction and mixed differences), using the
existing independent Rust solver. Local fits correct UV by foot-point projection;
shared-chart fits retain supplied UV exactly so isolines remain usable as seams.
The largest residual selects the knot spans to bisect, adding at most one knot
per direction per round. Fitting retains the best measured result, stops at the
control-point/data/work limits (at most 32 rounds), and does not assert tolerance
success just because refinement stopped. Auto fitting targets half `tol_mm`
before joining; final network measurements decide the output gates.

## Whole-edge continuity and limits

Reconciliation accepts unambiguous compatible whole-edge pairs with matching
along-edge degree, finite nondegenerate samples, opposite interiors and close
sampled positions. It handles reversed parameter directions and inserts the union
of edge knots. C0 averages shared boundary control points. Requested G1 also
reconciles adjacent control rows with derivative scaling. Alternating projections
handle constraints sharing corners; reported continuity is measured afterwards,
not inferred from the requested mode.

The implementation has no trimming or partial-edge joins. Rational weighted
edges, ambiguous matches, incompatible degrees and T-junctions do not gain a
continuity certificate. `unmatched_edges` includes exterior edges and is only a
count. Separately, `unjoined_region_boundaries` checks source chart interfaces
against measured joins, sampling source edge endpoints/midpoints against a
128-segment representation of each joined edge. It counts uncovered chart pairs
and has a zero limit, so missing or internal partial seams cannot disappear into
the exterior-edge count. This is a sampled coverage check, not an exact topological
or Hausdorff proof.

At most 256 patches are accepted per cook. `max_cp` bounds fitting; exact
restriction and shared-edge knot reconciliation may add points beyond that fit
budget. Reconciliation has a separate hard ceiling of 128 control points per
direction. Exceeding it, or producing degenerate/nonfinite seam data, fails the
cook with a resource/geometry error rather than reporting zero continuity error.

## Final measurements and gates

The final joined patches are tessellated at `max(0.1 * tol_mm, 0.001)` mm.
Forward distances use original vertices plus 8,000 deterministic area samples;
reverse distances use 4,000 deterministic area samples of the preview against the
source. These are sampled distances to triangle BVHs, not certified continuous
surface bounds. Seam gap and absolute normal-angle residuals use 129 points per
matched edge. Zero seams means no inter-patch continuity is certified.

| Report key | Meaning / gate |
|---|---|
| `dev_max_mm` | Forward maximum absolute sampled distance; limit `tol_mm` |
| `dev_rms_mm` | Forward RMS |
| `dev_p95_mm` | Forward absolute p95; limit `tol_mm` |
| `dev_reverse_p95_mm` | Reverse absolute p95; limit `tol_mm` |
| `dev_reverse_max_mm` | Reverse sampled maximum, informational |
| `sample_count` | Forward sample count |
| `max_seam_gap_mm` | Maximum measured matched-edge gap; limit 0.01 mm |
| `max_normal_jump_deg` | Maximum measured normal angle; limit 1 degree in G1 mode |
| `unjoined_region_boundaries` | Uncovered source chart-interface pairs; limit zero |
| `seam_count`, `unmatched_edges` | Matched seams and unmatched patch edges |
| `region_count`, `patch_count`, `skipped_regions` | Layout counts; skipped regions is zero |
| `control_points`, `refinement_rounds` | Final net size and accumulated fit rounds |

`dev_max_mm`, `dev_rms_mm` and `dev_p95_mm` retain the existing surface-deviation
metric names used by consumers; legacy names such as `max_dev_mm` are not emitted
as additional aliases here. The maximum gate is stronger than p95 acceptance:
passing the WP2 p95 criterion alone does not make the whole Report pass.

The rotated labelled-panel test measured forward p95 **0.14165 mm**, seam gap
**0 mm**, and maximum normal jump approximately **1.21e-6 degrees** at a 0.2 mm
tolerance. Its local maximum **0.62253 mm** still fails the 0.2 mm maximum gate.
This establishes the stated p95/seam criteria for that fixture, not universal
tolerance success. The actual default `geo.car_panel` at `tol_mm=0.2`, four patches and G1 also
passes the p95/seam acceptance test, including independent samples of every
shared JSON boundary and the existing `surface.deviation_check` node.

## Region refinement semantics

Re-segmentation works inside selected existing regions, retaining an old id for
one result and assigning new ids to other pieces. Merge requires at least two
existing ids and uses the lowest selected id. Empty `group` selects all regions
for re-segmentation or projected splitting; malformed, negative or absent ids
produce clean errors.

`split_by_polyline` changes labels on the original polygon-face adjacency graph.
It projects face centres and the supplied path to each region's best-fit plane,
removes graph connections crossed by the projected path, then labels the resulting
components. All triangulation pieces of a polygon remain together. This is a
projected region cut, not a geometric mesh cut: vertices, polygon topology and
positions remain intact, and the label boundary follows existing faces.

Provide exactly one open polyline primitive with valid indices and at least two
vertices. To close the path, repeat the first vertex at the end; a closed polygon
primitive is rejected. A path that does not separate a selected region errors.
Folded regions with overlapping planar projections are unsupported and must be
separated first. Refinement accepts 1–400,000 triangles and limits path-length
times triangle count to 20,000,000 operations.

## Provenance and verification scope

Weiss, Andor, Renner and Várady, *Advanced surface fitting techniques*, Computer
Aided Geometric Design (2002), [DOI: 10.1016/S0167-8396(01)00086-3](https://doi.org/10.1016/S0167-8396(01)00086-3),
provides public-abstract-level provenance for the fitting/refinement direction.
The full paper was not accessed; this implementation combines that public
description with the repository's existing independently implemented B-spline,
least-squares, parameterisation and knot algorithms. It is not a transcription or
a claim to reproduce undisclosed paper details or commercial behaviour.

Tests cover local residual-driven refinement, fixed-chart UV fitting, exact
restriction, reversed/nonuniform compatible seams, source-chart coverage,
labelled layouts and projected polygon-region splitting. The measurements above
are fixture results supplied by the current implementation verification; this
documentation-only change adds no test execution claim.

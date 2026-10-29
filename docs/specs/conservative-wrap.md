# Conservative wrap: native WP6 contract

Measured implementation contract, 2026-10-29, for `clean.alpha_wrap`.
Lengths are millimetres. This document supersedes the draft alpha-wrap
algorithm, WB status and W4 golden-test proposal in
[rhino8-mesh-tools.md](rhino8-mesh-tools.md) for this node.

## Method and scope

The native implementation uses the simpler route allowed by HANDOFF WP6:
unsigned voxel offset, outside flood, Hermite dual contouring and constrained
shrink projection. It does not implement the Delaunay refinement algorithm
of Portaneri et al. (2022), and makes no claim of equivalent output or all
of that algorithm's guarantees. It uses no external program or new dependency.
No proprietary installation or copyleft algorithm source was consulted.

Ju et al. (2002), section 2, supplies the uniform-grid Hermite/QEF method.
The distance guard, local contour cycles, resource limits, projection guard
and final acceptance checks are Vernon's engineering choices. Sparse 8×8×8
tile classification reduces individual distance queries; a dense bounded
byte grid and flood queue remain. This is not an adaptive octree.

## Node interface

Input 0 is `geometry`; output 0 is the generated triangle `geometry`, and
output 1 is a `report`. Existing detail attributes are copied and point
normals `N` are recomputed. Point/primitive attributes, colours and source
face correspondence are not transferred. The node performs no file writes.

| Parameter | Declared range | Resolution |
|---|---|---|
| `alpha` | 0–10,000 mm | Zero uses input bounding-box diagonal / 20; otherwise the supplied value |
| `offset` | 0–1,000 mm | Zero uses effective alpha / 30; otherwise the supplied value |

Thus zero selects automatic behavior, not a literal zero-clearance result.
The effective alpha must be positive and finite. A single point can be
wrapped with explicit alpha, but has no extent from which to derive automatic
alpha. The adapter uses a fixed 40,000,000 grid-node cap; there is no exposed
resource parameter for this node. The unit check follows Vernon's existing
metric-geometry convention.

## Supported source geometry

The distance oracle is the union of closed-primitive fan triangles and
points not used by any usable triangle. One triangle BVH and a point tree
are reused throughout extraction, projection and measurement. Orphan points
beside a mesh participate in both the field and every final distance check.
Open polygons/polylines do not become source segments: their otherwise
unused vertices participate as points. General curved primitives and a
polyline's continuous segments are outside this source model.

Mesh boundaries, disconnected pieces, triangle soups, inconsistent winding
and point clouds are accepted when a certified contour can be constructed.
Unsigned distances do not require a closed, oriented input. Separate pieces
may merge or remain separate according to their spacing and parameters.

Requests fail for empty/nonfinite input, invalid vertex indices, invalid
parameters, unrepresentable distance/area/grid arithmetic or exceeded grid
limits. A nonzero triangle whose squared cross-product magnitude is at most
1e-30 mm⁴ is rejected, including detected underflow; it is never silently
replaced by its corners. Exactly collinear closed triangles with distinct
positions are also rejected because segment distance is unsupported.
All-coincident closed triangles may contribute a point instead. These
rejections preserve the meaning of distance to the supported complete source.
Concave closed polygons retain the core geometry's fan triangulation
semantics; this node does not perform polygon repair or robust triangulation.

## Extraction and acceptance

1. Translate about the input bounds centre before distance and QEF work.
   Preflight grid dimensions and products before constructing the BVH.
2. Initially use `h = alpha / 8` and barrier radius
   `R = offset + alpha / 2 + sqrt(3) * h`. Pad the source bounds by
   `R + 3h`. Classify complete tiles with distance Lipschitz bounds; query
   individual grid nodes only in intersecting tiles.
3. Flood free nodes from every padded boundary using six-neighbour adjacency.
   Extract the interface between reached exterior and the remaining region.
   Enclosed free regions therefore become interior to the outer wrap.
4. For each interface edge, bracket the original unsigned-distance level R.
   Twelve safeguarded secant steps clamp the fraction to [0.1, 0.9], then
   interpolate within the retained bracket. The worst bracket contraction is
   0.9¹² of an edge, about 0.28243h; the guard is not based on an exact root.
5. Use shared face decisions to connect crossings into local cycles; allocate
   one dual vertex per cycle. Solve a regularized 3×3 Hermite QEF about the
   crossing mean, with mean fallback for nonfinite solutions, and clamp into
   the owning cell interior. Connect around primal edges and triangulate.
6. Certify the baseline before shrink projection. For each vertex above
   `offset + alpha / 4`, attempt a displacement toward its nearest source,
   limited to h/4. Up to eight backtracks retain cell bounds, finite values,
   the true source distance band and noncollapsed, consistently directed
   incident triangles. This is a bounded improvement step, not convergence
   to the requested offset. It may retain an unmoved vertex.
7. Restore world coordinates and certify again, including translation-rounding
   effects. If projection causes final certification to fail, retry the
   certified unprojected baseline. Failed extraction/certification may refine
   h to alpha/16 and alpha/32, recalculating R, within the same node cap.
   Exhausted or over-budget attempts return an error without a damaged mesh.

Success requires nonempty output and all of these measured counts to be zero:

- Open edges; edges with more than two incident faces; two-face edges having
  equal directed orientation.
- Vertices whose link is not one connected degree-two cycle, including
  unused output vertices. This catches bow-tie vertices as well as bad edges.
- Nonfinite or tiny-area triangles, repeated face indices, and duplicate
  faces. The area guard uses squared cross magnitude `h⁴ * 1e-20`.
- Output vertices closer than offset or farther than offset + alpha from
  the complete supported source. Every output vertex is checked, without
  subsampling or a relaxed tolerance in the acceptance comparison.

These are combinatorial manifold and vertex-distance certificates. They do
not certify absence of global self-intersections, distances throughout face
interiors, or a bidirectional Hausdorff bound. They do not prove that every
source point lies inside the generated shell. Successful output is not a
certified CAD solid.

Alpha controls gap bridging, with conservative inflation: some openings
larger than alpha can also close. No universal sharp hole-diameter threshold
or preservation of thin features is claimed. The aperture regression uses
an opened sphere at alpha 6 mm and offset 0.2 mm: its 4.8 mm opening seals,
and its 24 mm opening remains accessible from the exterior. Independent ray
parity checks the sphere centre, and separate distance/edge/link checks
validate both outputs. This is measured fixture behavior, not a proof for
arbitrary hole geometry.

## Report fields

| Metrics | Meaning and units |
|---|---|
| `faces`, `vertices`, `components` | Triangle count, point count and connected shells |
| `volume`, `surface_area` | Signed volume (mm³, recentered accumulation), mesh area (mm²) |
| `alpha`, `offset`, `voxel_size` | Effective parameters and final h (mm) |
| `minimum_wrap_radius`, `barrier_radius` | Both report the extraction radius R (mm) |
| `grid_nodes`, `band_nodes` | Complete grid-node count and individually classified band nodes |
| `source_triangles`, `source_points` | Usable triangles and points outside those triangles; the latter is not the total input vertex count |
| `checked_vertices` | Output vertex count actually distance-checked |
| `distance_min_mm`, `distance_max_mm` | Extrema of unsigned distance to the complete supported source (mm) |
| `offset_error_max`, `offset_error_mean` | Distance maximum/mean minus effective offset (mm); excess clearance, not convergence residuals |
| `projection_moves`, `refinement_retries` | Accepted vertex movements and refinement attempt index (0–2) |

The zero-upper-limit report gates are `open_edges`, `nonmanifold_edges`,
`inconsistent_edges`, `nonmanifold_vertices`, `degenerate_triangles`,
`below_offset_vertices` and `above_upper_bound_vertices`. On successful
cooking all seven are zero. An informational report note describes the
alternative and its self-intersection limit. The node is implemented and
emits no white-box placeholder note.

## Recorded acceptance

Saved verification run `wp6-final-20261029-a` completed at
2026-10-28 20:40:02 UTC (2026-10-29 00:40 Dubai). All six steps passed:
release benchmark, workspace build/tests, all-target Clippy with warnings
denied, demo and node catalogue. The selected source fingerprint before and
after was identical:
`fa2b91e9d0e8484df896296e89daa79449310c1912109c3de7e588c2c1fe68b7`.
Scratch logs are local diagnostic evidence, not distributed test fixtures.

The ignored test `conservative::tests::wp6_release_million_triangles` was
executed explicitly with `--release --ignored --nocapture`; it ran one test,
with zero failures. Its source is a radius-30 mm UV sphere with exactly
1,000,000 usable nondegenerate triangles, not repeated coarse geometry.
Fixture generation and compilation are excluded from the kernel timer.
Acceleration construction, field, flood, extraction, projection and final
certification are included.

| Measured value | Result |
|---|---:|
| Whole-kernel time | 1.930284 s, below the 30 s threshold |
| Source triangles / standalone points | 1,000,000 / 0 |
| Output vertices / triangles | 51,326 / 102,648 |
| Final h | 0.6495169245315362 mm |
| Effective alpha / offset | 5.19613539625229 / 0.173204513208410 mm |
| R | 3.896268524999055 mm |
| Complete / band grid nodes | 1,404,928 / 671,232 |
| Measured distance minimum / maximum | 3.733889293866165 / 3.896212952247528 mm |
| Permitted distance interval | [0.173204513208410, 5.36933990946070] mm |
| Vertices checked / projected | 51,326 / 51,326 |
| Below / above distance band | 0 / 0 |
| Open / nonmanifold / inconsistently directed edges | 0 / 0 / 0 |
| Nonmanifold vertices / degenerate triangles | 0 / 0 |
| Refinement retries | 0 |

This is one local release measurement, not a hardware-independent latency
guarantee. In particular, the final distance is substantially above the
requested offset; it satisfies the specified interval rather than an exact
offset fit. Fine alpha relative to a large bounding box can exceed the grid
cap and fail even when the source mesh is small.

Other regressions cover open plates, point clouds and orphan points,
separated components, automatic parameters, translation, winding reversal,
same-input determinism, all 254 mixed corner patterns with two face-centre
choices, guarded exact-grid ties, invalid/numerically unsupported sources,
resource rejection, actual projection moves and certified fallback.

## Primary references and source boundary

- Ju, Losasso, Schaefer, Warren, *Dual Contouring of Hermite Data*, SIGGRAPH
  2002, [DOI 10.1145/566654.566586](https://doi.org/10.1145/566654.566586).
  The public [paper PDF](https://people.eecs.berkeley.edu/~jrs/meshpapers/JuLosassoSchaeferWarren.pdf)
  was consulted for uniform-grid Hermite data and QEF construction, especially
  section 2. No reference implementation was used.
- Portaneri, Rouxel-Labbé, Hemmer, Cohen-Steiner, Alliez, *Alpha Wrapping with
  an Offset*, SIGGRAPH 2022,
  [DOI 10.1145/3528223.3530152](https://doi.org/10.1145/3528223.3530152).
  Its public abstract motivated the target behavior. Full-paper access through
  HAL was unavailable during this work; its Delaunay algorithm was not
  implemented. The handoff explicitly permits the voxel alternative above.

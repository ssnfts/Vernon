# Native field-aligned quad remeshing

WP1 implementation, 2026-10-26. Node: `surface.quad_remesh`.

## Method and provenance

Original Rust implementation from Jakob, Tarini, Panozzo and Sorkine-Hornung,
[Instant Field-Aligned Meshes (2015)](https://rgl.epfl.ch/publications/Jakob2015Instant),
sections 3.2–3.5, 3.8 and appendix. No reference implementation source was copied.
No runtime dependency or external executable was added.

The solver smooths four-direction tangent fields and compatible position fields
over a deterministic multilevel mesh hierarchy. Graph extraction collapses zero
lattice jumps, orders remaining edges in tangent planes, extracts simple cycles,
and subdivides cells into quads. A triangle BVH projects vertices onto the input.
This works in 3D; it does not assume a height field or a particular world axis.

Hard-angle patches use shared arc-length boundary anchors to keep their seams
welded. Cornerless loops use a geometric start and direction, independent of
patch-local vertex ordering. Symmetry clips at bounding-box-centred planes, reflects with corrected
winding and welds seam vertices. Curvature-dependent spacing is a local extension
of the uniform field method. Sparse inputs needing refinement use direct
triangle pairing and face subdivision when adaptivity is zero.

Projection can impair a quad's Jacobian. Local projected relaxation improves
the worst incident Jacobian. The node rejects remaining inverted/degenerate
quads, nonmanifold edges and inconsistent winding. Without symmetry it also
checks component count, Euler characteristic and closed/open status against the
input. These checks do not constitute a general self-intersection test.

## Contract and limitations

Existing parameter names and ports are retained. Count mode derives target edge
length as `sqrt(input area / target_quads)`; edge mode takes millimetres. Counts
are approximate. Boundary constraints and high curvature can prevent a requested
size from being attained. Difficult inputs may return an error suggesting a
higher count or lower adaptivity instead of returning invalid topology.

Guide-curve steering and strict source-face edge constraints remain unsupported
and emit explicit warnings when requested. Hard-edge detection and geometric
boundaries are supported. Symmetry deliberately replaces the negative side(s)
with the reflected positive wedge; an empty selected wedge is an error.

The report includes `quad_ratio` (and legacy `quad_fraction`),
`irregular_vertices` (interior valence other than four), `target_edge_mm`,
`mean_edge_mm`, and quality/topology metrics. `max_dev_mm` is a conservative
upper bound on continuous bidirectional Hausdorff distance between triangulated
surfaces. `dev_sample_max_mm` is the sampled lower bound and
`dev_bound_gap_mm` is their difference.

For each triangle, centroid distance plus the maximum centroid-to-corner radius
bounds distance everywhere on that triangle, since distance to a set is
1-Lipschitz. Recursive subdivision tightens this bound. Work is budgeted per
direction, with at least one sample per input triangle. Exhausting the budget
leaves a valid but looser bound and emits a warning. No exact Hausdorff value is
claimed from finite samples.

## Verification

Regression cases cover the actual default car panel, dense rotated/translated
panels in count and edge modes, a closed sphere, hard-edged cubes with unequal
interior sampling, adaptive sizing, symmetry, empty wedges and an analytic
parallel-surface distance bound. A regression checks closed-seam anchors under
every cyclic shift and both traversal directions. Existing node-contract tests cover determinism
and malformed inputs.

Run from the repository after `. .\scripts\dev-env.ps1`:

```powershell
cargo test -p vernon-nodes-surface
cargo test --release -p vernon-nodes-surface wp1_release -- --ignored --nocapture
```

The 100,000-triangle car-panel benchmark includes the entire cook and report
calculation. Measured on the development machine: 1.35 s, 1,005 quads (100%),
23.40 mm mean edge against a 23.37 mm target, 4.60 mm conservative deviation
bound, positive minimum scaled Jacobian (0.0237), one component and Euler
characteristic 1. Timing is machine-dependent; the test requires under 5 s.
It also requires at least 90% quads, edge length within 15% of target and
deviation below 1% of the input bounding-box diagonal.

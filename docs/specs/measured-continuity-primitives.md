# WP3: measured continuity and primitive recognition

Current implementation contract, 2026-10-27. This supersedes proposed WP3 algorithms in [the surfacing overview](reverse-engineering-surfacing.md). Both nodes are native implementations. Their output reports expose the limits below; an implemented node does not imply universal G2 construction or an analytic CAD solid.

## Mesh to measured surface patches

`surface.mesh_to_g2_surface` takes `mesh: Geometry` and emits, in order, `patches: Json`, `preview: Geometry`, `report: Report`. Patch JSON uses the existing `nurbs_patch_set` representation; the preview is tessellated geometry.

| Parameter | Default | Allowed range/options |
|---|---|---|
| continuity | g2 | g1, g2 |
| max_degree | 6 | 3..6 |
| max_deviation_mm | 0.3 mm | 0.0001..100 mm |
| fit_level | 3 | 1..4 |
| preview_tol_mm | 0.2 mm | 0.001..10 mm |

The construction emits one bicubic patch per quad. Regular regions use the exact Catmull-Clark conversion; extraordinary regions use existing limit-sample least-squares fits with shared cubic boundaries. Nonquad input receives one subdivision before conversion. Actual degree is 3; `max_degree` is an upper bound, not a request for bi-6 caps. There is no implemented guided extraordinary cap guaranteeing G2.

A bounded local reconciliation candidate mirrors first inward control rows from exact regular neighbors into fitted coefficients, preserving shared boundaries and regular coefficients. Acceptance requires a strictly smaller global maximum normal angle, no regression in G1 coverage or invalid samples, no regression in requested G2 coverage, and a recomputed sampled maximum deviation within the requested tolerance. Otherwise the original patches remain. `g1_reconciliation_applied` records acceptance.

Expected seams come from cage topology, with patch sides and parameter reversal, rather than successful geometric matching. A separated seam remains in the denominator. The measurement evaluates analytic rational surface derivatives through order two. It compares tangent planes and the physical shape operator in a common orthonormal tangent frame, aligning normal orientation. The Frobenius norm of the difference is reported in inverse millimetres. Raw parameter second derivatives or relative principal-curvature differences are not the continuity criterion.

`measure(..., 17)` means **17 intervals and 18 positions including both endpoints** per expected seam: the implementation loops `0..=count`. A complete seam passes sampled G1 only if every position has gap <=0.01 mm and unoriented normal angle <=1 degree. Sampled G2 additionally requires shape-operator jump <=0.001/mm at every position. `g1_fraction` and `g2_fraction` count passing complete seams divided by all expected seams. Missing, singular and nonfinite samples fail that seam and increment `invalid_continuity_samples`. An empty expected seam set yields zero fractions and cannot certify continuity.

The requested fraction has a lower-bound gate of 1.0; `invalid_continuity_samples` has an upper-bound gate of zero, and `dev_max_mm` has the requested maximum-deviation upper bound. Partial coverage therefore fails numerically and emits a warning. Other report keys include `expected_seams`, `free_boundary_edges`, `seam_gap_max_mm`, `g1_max_deg`, `curvature_jump_max_inv_mm`, `dev_rms_mm`, `dev_samples`, `patch_count`, `control_points`, `regular_faces`, `extraordinary_patches`, `extraordinary_vertices` and `max_degree_used`.

Geometry validation precedes welding. Finite coordinates, valid indices, distinct face vertices, nonzero area and manifold cage constraints are checked before lattice indexing. A nonquad refinement exceeding 50,000 faces is rejected. The conversion's three-million-face sample-lattice budget and bounded fit depth constrain work; unsupported/resource-exhausting inputs fail instead of silently certifying a partial result.

### Measured fixtures

These are native analytic/regression fixtures, not vendor parity tests. Values recorded in the focused WP3 tests on 2026-10-27:

| Fixture | Expected seams | Sampled G1 fraction | Sampled G2 fraction | Sampled maximum deviation mm | Maximum curvature jump /mm |
|---|---:|---:|---:|---:|---:|
| Curved regular 4x3 grid | 17 | 1 | 1 | not recorded here | not recorded here |
| Periodic regular 8x6 torus | 96 | 1 | 1 | not recorded here | not recorded here |
| Refined cube, valence 3 | 192 | 0.875 | 0.625 | 0.07082835291924547 | 0.2926178441614785 |
| Refined open valence-5 cap | 140 | 1 | 0.8214285714285714 | 0.054596391365623484 | 0.026514525582829847 |

The torus has no free boundary edges; translating one patch retains all 96 expected joins and reduces coverage with a gap greater than 0.99 mm. The valence-5 case has zero invalid continuity samples. The cube and valence-5 fixtures fail a requested full-G2 gate. Sampled agreement does not prove global mathematical G2, and sampled limit-lattice deviation is not a certified Hausdorff bound.

## Prismatic recognition with a retained tessellated shell

`surface.convert_prismatic` takes `regions: Geometry`, with optional integer primitive `region_id`, and emits `brep: Json` at index 0, `report: Report` at index 1, and `shell: Geometry` at index 2. The first two port indices remain compatible. Parameters are `tol_mm` =0.05 mm [0.001,5] and `keep_unfit_as_facets` =true.

Deterministic bounded sample consensus recognizes plane, sphere, cylinder and cone supports. Candidate seeds include global fits, face planes, normal-pair cylinder axes and fixed-seed random hypotheses. Triangle support checks all triangle vertices and normal compatibility. Least-squares support refitting and bounded finite-difference geometric Gauss-Newton refinement improve parameters. Labels restrict support buckets and can be split into multiple recognized supports; absent labels permit automatic discovery. Negative labels retain faceted fallback. Supports need not be connected.

Maximum support-vertex residual controls fit acceptance. Each analytic JSON face records shape parameters, `region_id`, `source_region_id`, `support_triangles`, `rms_mm`, `max_deviation_mm` and separately `sampled_centroid_max_mm`. Centroid error is diagnostic, not a continuous polygon-interior tolerance guarantee. The schema remains `prismatic_brep`, version 1, units mm, with representation `tessellated_shell_with_analytic_fits`.

Source boundary polylines remain in `boundary_loops`. Legacy `outer_loop` and `inner_loops` are ordered by perimeter; those names do not establish containment, analytic trims or intersection curves. The authoritative geometry is the retained source tessellation, welded and wound consistently. Its primitive `part` is the deterministic fitted group id. Unfitted triangles remain when the keep option is true, and are omitted otherwise. Closure, nonmanifold edges and signed positive volume refer to this tessellated shell. `solid` does not assert an analytic-intersection CAD solid or prove absence of self-intersections.

Report keys include `faces`, `analytic_faces`, `faceted_triangles`, `regions`, `fit_max_mm` (upper-bound gate `tol_mm`), `analytic_coverage`, `watertight`, `boundary_edges`, `nonmanifold_edges` and `volume_mm3`. Open/nonmanifold retained shells warn. Input validation rejects invalid face indices, nonfinite coordinates, empty/degenerate input, malformed point/primitive attribute lengths and noninteger `region_id` rather than treating malformed labels as automatic recognition.

Limits are global: at most 20,000 prepared triangles after welding/triangulation, 512 extraction groups across all label buckets (including negative-label fallback), and a shared 20,000,000-work-unit recognition budget. Candidate preparation charges 128 units per remaining triangle and each full support evaluation charges one per triangle. Each extraction uses at most 96 random hypotheses; refinement uses at most 1,000 support vertices over eight iterations. Exhaustion returns an error, with no successful partial output. These finite budgets can miss small or noisy supports; torus recognition and adaptive probability guarantees are absent.

The unlabeled drilled-block regression produces exactly six planes and one radius-3-mm cylinder (radius within 1%), retains all 512 triangles with group labels, reports a closed positive-volume shell and repeats identical JSON. Rotated/translated noisy cylinder and sphere fixtures recover radius within 0.01 mm; the noisy cone recovers half-angle within 0.002 radians, with maximum support-vertex residual below 0.02 mm. Further regressions cover labeled hole loops, negative-label fallback, malformed attributes and cumulative resource exhaustion. These do not establish CAD trim correctness or arbitrary scan robustness.

## Published-method provenance and verification scope

- Loop and Schaefer, *Approximating Catmull-Clark Subdivision Surfaces with Bicubic Patches*, ACM TOG 27(1), 2008, DOI 10.1145/1330511.1330519: [public paper](https://www.cs.cmu.edu/afs/cs.cmu.edu/academic/class/15869-f11/www/readings/loop08_accpatches.pdf), sections 2–3. The paper's geometry is C0 near extraordinary edges; separate tangent patches produce continuous shading normals. Vernon uses its independent regular conversion and extraordinary least-squares fit, without claiming the paper's masks, tangent field or geometric G2.
- Karciauskas and Peters, *Guided C2 Spline Surfaces with V-shaped tessellation*: [author paper](https://cise.ufl.edu/research/SurfLab/papers/06V123.pdf), introduction and sections 2–4. Designed transition maps, degree-(6,6) rings and degree-(4,4) centers are context for future work, not implemented caps.
- Schnabel, Wahl and Klein, *Efficient RANSAC for Point-Cloud Shape Detection*, CGF 26(2), 214–226, 2007, DOI 10.1111/j.1467-8659.2007.01016.x: [publisher record](https://onlinelibrary.wiley.com/doi/full/10.1111/j.1467-8659.2007.01016.x), [Eurographics record](https://diglib.eg.org/items/bb59754d-451c-450a-8eef-be69d0687412/full). The implementation agent consulted primary records/abstract; attempted full-paper endpoints returned HTML or access errors. Recognition follows the general hypothesis/support/refit pattern with independently derived equations. It does not claim full-paper replication, octree acceleration or adaptive probability bounds.

No proprietary installation, GPL algorithm source, new dependency or external executable was used for WP3. Focused verification recorded 11 subdivision tests and seven prismatic tests passing, plus surface-library Clippy. Workspace build/tests, broader Clippy, demo and GUI verification are separate integration checks; this document does not claim they passed from those focused results. Integrated verification and current progress are recorded in [HANDOFF.md](../HANDOFF.md).

Integrated verification on 2026-10-27: 1,039 workspace tests passed, zero failed, six ignored; workspace build and Clippy with warnings denied passed. The demo cooked 36 nodes with zero failures and two expected gates. The catalogue and GPU app launch were checked. Final code review found no remaining actionable issue within the documented scope.

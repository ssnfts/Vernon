# Vernon handoff: what is left and exactly how to do it

Written 2026-10-24 at commit `779bb61` on `main` in `C:\Users\mabdu\REA\vernon`.
Read `AGENTS.md` first: it has the hard rules, build commands and definition
of done. This document lists the remaining work as ordered **work packages
(WP)**. Do them in order, one commit each. Every WP ends with the full
definition-of-done check from `AGENTS.md`.

## Progress — 2026-10-30 (WP8)

WP1–WP8 are implemented under their measured contracts. WP9 (real CFD) is
next and requires a separate installation decision. Registry:197 nodes,
190 implemented/7 white-box; Capture 22 implemented/0 white-box. Full bounded
workspace verification:1,171 passed,0failed,10ignored. Build and all-target
Clippy are clean; demo36 nodes/0failures/2 expected gates; catalogue matches.
The app opened with Vulkan 4x MSAA on the RTX 4090 Laptop GPU; all four
photogrammetry tools were checked in the Capture ribbon.

The approved COLMAP 4.2.1 CUDA process pipeline reconstructed20 public South
Building photos from fresh inputs:20 registered images,2,703sparse points,
233,744fused points and293,974mesh vertices/554,298usable triangles in
204.997 s. Import explicitly removed104 exact zero-area triangles. A separate
real sparseDelaunay node produced2,024vertices/4,025triangles in0.233s.
The100 mm numerical scale handoff passed on sparse and mesh outputs; this
does not establish surveyed building accuracy or manufacturing suitability.
No photos, binaries or reconstructed sample geometry are committed.

Failed external outputs retain diagnostics/gates without reusing earlier
File artifacts; recursive image metadata invalidates dependent caches;
uncertifiable inputs prevent actions; external geometry writers defer while
editing. Native offline meshing remains an explicitly labelled placeholder.
Simplification and alternative matching/mapping methods remain unverified.
See [the measured photogrammetry contract](specs/real-photogrammetry.md),
the completed WP8 plan and docs/CONTINUATION.md for durable evidence.

Final queue:`wp8-final-20261030-c`; source unchanged
`90b5470a5285351e1b07d73b5b31aabff60c04c5b0558ce395037c3e1745324e`.
Real acceptance:`wp8-real-20261030-b`; its source fingerprint precedes only
the metadata promotion and maintained script-regression additions. Existing
WP1–WP7 checkpoints below remain historical evidence.

## WP7 checkpoint — 2026-10-29

WP1–WP7 are implemented under their documented contracts; WP8 (real
photogrammetry runs) is next. Registry: 197 nodes, 186 implemented and 11
white-box; Capture has 18 implemented and four white-box nodes. Full bounded
workspace verification: 1,131 passed, zero failed, eight ignored; build and
all-target Clippy clean. The demo cooks 36 nodes with zero failures and two
expected gates. Catalogue output matches. Vulkan GPU viewports and the native
Global Align / Multiway Align ribbon entries were checked visually.

Global alignment now computes real 33-bin FPFH descriptors and verifies
RANSAC hypotheses, with an explicit three-scale feature pyramid and fine
point-to-plane ICP. Multiway alignment initializes a maximum-fitness tree
and jointly optimizes SE(3) poses using actual pairwise correspondence
information, robust uncertain-edge switches and pruning. The selected input
slot stays fixed; missing optional slots retain their original mapping.

The actual five-partial-car-panel fixture has independent shuffled order,
0.02 mm Gaussian coordinate noise and seeded arbitrary-axis poses up to
30 degrees / 200 mm. Its worst pose errors are 0.035683 mm and 0.005843 degrees,
within the 0.5 mm / 0.5 degree limits; final RMSE is 0.030023 mm and fitness
0.862554. Numerical regressions repaired indefinite information hidden by
mixed units and overflow in the robust objective. Final kernel and adapter
reviews found no remaining blocker.

The solve is bounded and local: repeated/featureless geometry, insufficient
overlap and a wrong high-fitness tree remain limitations. The reserved `fgr`
method explicitly errors. Coarse-only information is null; refined graph
information is point-to-point correspondence information, not a calibrated
covariance. See [the measured registration contract](specs/native-scan-registration.md).
No external program was installed or run for WP7; nalgebra's permissive Rust
dependencies and licence inventory were checked. Installation/publication
authorization gates still apply to later packages.

## WP6 checkpoint — 2026-10-29

WP1–WP6 are implemented under their documented contracts; WP7 (automatic
multi-scan alignment) is next. Registry: 197 nodes, 184 implemented and 13
white-box. All 26 Clean/Align nodes are implemented. Full workspace checks:
1,117 passed, zero failed, eight ignored; build and all-target Clippy clean.
The release-only WP6 benchmark passed separately. The demo cooks 36 nodes
with zero failures and two expected gates. The generated catalogue matches,
and the Vulkan GPU viewports and Conservative Wrap ribbon were checked.

`clean.alpha_wrap` uses the permitted voxel-offset / dual-contouring /
constrained-projection route. It checks every output vertex against the
complete supported source and rejects uncertified topology or distance bands.
The genuine one-million-triangle sphere took 1.930284 s for the entire kernel,
including its BVH and certificates. Its 51,326 output vertices all lie in the
requested band; all five topology counters and both band violation counters
are zero. The output has 102,648 triangles. Larger openings may also close;
this is not an exact Delaunay alpha wrap, and global self-intersections are
not certified. See [the measured contract](specs/conservative-wrap.md).

Bounded persistent verification scripts now save per-step logs, atomic status,
source fingerprints and matching-source resume checkpoints. Exclusive locking
prevents competing workers; a Windows Job Object contains subprocesses on
worker termination and timeout. Seven runner self-test scenarios and a
separate-shell persistence check passed. The actual WP6 six-step worker also
finished after its launcher returned, with unchanged source fingerprints.
These scripts run fixed checks, not code changes or AI calls. The authorized
six-hour follow-up reads [CONTINUATION.md](CONTINUATION.md) to resume work.

## WP5 checkpoint — 2026-10-28

WP1–WP5 are implemented under their documented contracts; WP6 (conservative
watertight wrap) is next. The registry contains 197 nodes: 183 implemented
and 14 white-box. Split/Print has 25 implemented nodes and one placeholder,
the deferred script bridge. Full workspace verification: 1,109 passed,
zero failed, seven ignored; build and all-target Clippy clean. The demo cooks
36 nodes with zero failures and two expected gates. The catalogue was
regenerated, final reviews found no remaining blocker, and GPU app launch
plus the Split/Print ribbon were checked visually.

Body-line cuts follow the actual projected piecewise-linear curtain, with
component selection, shared-edge splitting, planar cap regions and fresh
seam ids. The actual 3 mm car-panel shell gives two closed, consistently
wound manifold pieces and preserves volume to about 1.8e-14 relative error.
Guides must be monotone in the projected frame; poorly conditioned cuts
reject rather than return damaged solids. Selection applies to whole touched
components, leaving untouched components intact.

Smart partition uses bounded cutting-plane beam search with actual bed-fit
checks. The 1200×600×300 mm box on a 350³ mm bed produces eight fitting
parts, 10,200 mm seam perimeter and 46 mm minimum fit margin. No global
minimum-part-count proof is claimed. Snap joints now generate cantilever
hooks and mating recesses and gate the rectangular-beam strain formula.
The initial printed-material strain allowances are explicitly assumed
screening values; physical coupon validation remains pending.

G-code conformance now checks five firmware dialects, motion/arc bounds,
temperature limits, extrusion modes and numbered object markers. Unsupported
coordinate state prevents a subsequent passing bounds check. Profile Build
writes and round-trip validates JSON or a documented INI subset. Successful
external slicer import remains unverified; the unstable scripting bridge
remains white-box. Connected File consumers and file polling now include
size/mtime metadata so external edits can invalidate the cached reader.
See the [WP5 contracts, measurements and limits](specs/native-print-tools.md).

## WP4 checkpoint — 2026-10-28

WP1–WP4 are implemented under their documented contracts; WP5 (split/print)
is next. The registry contains 197 nodes: 177 implemented and 20 white-box.
All 23 surface nodes now have native implementations, with their individual
limits retained. Full workspace verification: 1,067 passed, zero failed,
seven ignored; build and Clippy clean. The demo cooks 36 nodes with zero
failures and two expected gates. The catalogue was regenerated, final narrow
review found no remaining blocker, and GPU app launch was checked visually.

WP4 exports planar faceted shells and untrimmed rational/nonrational B-spline
faces using AP214/AP242 Part 21 structure. Compatible whole patch boundaries
share vertices and edge curves; each connected component receives its own
appropriately typed solid or surface representation. Partial seams, different
knot refinements and differently scaled rational weights can remain open.
Analytic CAD trimming and self-intersection certification remain absent.

An independent bounded parser checks references and selected entity, spline,
loop, shell and representation rules before the node writes files. It is not
a complete ISO/EXPRESS validator. Paths stay within the node work directory,
and replacement is atomic per file. Review regressions cover oversized JSON
metadata, translated solid orientation, physically different rational edges
and preserving a preexisting temporary file.

The seventh ignored test is the optional `needs FreeCAD` import test. It was
added with four fixtures but **has not run**: no suitable existing executable
or Python module was found. Independent CAD import remains unverified. See
the [STEP contract, limits and external check](specs/step-export.md).

## WP3 checkpoint — 2026-10-27

WP1–WP3 are implemented and verified under their measured acceptance contracts;
WP4 (the real STEP writer) is next. The registry contains 197 nodes: 176
implemented and 21 white-box. Full workspace verification: 1,039 passed,
zero failed, six ignored; build and Clippy clean. The demo cooks 36 nodes with
zero failures and two expected gates. The catalogue was regenerated, final
code review was clean, and GPU app launch was checked visually.

WP3 measures every expected subdivision seam from cage topology, including
position gap, normal angle and physical curvature jump. Regular grid and
periodic torus fixtures achieve full sampled G2. Extraordinary cases remain
partial: the cube reports G2 fraction 0.625 and the valence-five cap reports
0.82142857; both fail the requested full-G2 gate honestly. No guaranteed
curvature-continuous extraordinary cap is implemented. The published bicubic
approximation's separate shading tangents must not be confused with geometric
G2 continuity.

Prismatic recognition now detects planes, spheres, cylinders and cones, with
optional region restrictions, deterministic sample consensus, geometric
refinement, and bounded work. The closed drilled-block test finds six planes
and one cylinder with radius within 1%. Existing outputs remain JSON at index
0 and Report at index 1; a labelled tessellated shell is appended at index 2.
Analytic CAD trimming and STEP are not claimed. See the [WP3 contract and
measured limits](specs/measured-continuity-primitives.md).

## WP2 checkpoint — 2026-10-26

WP1 and WP2 are implemented and verified; WP3 is next. The registry contains
197 nodes: 174 implemented and 23 white-box. Workspace checks: 1,023 passed,
zero failed, six ignored; build and Clippy clean. The demo cooks 36 nodes with
zero failures and two expected gates. The node catalogue was regenerated and
the GPU viewport launch was checked visually.

WP2 adds local knot refinement, connected four-sided B-spline layouts with
shared whole-edge boundaries, measured G1 residuals, and a failing coverage
gate for unresolved source chart interfaces (including cuts inside one region).
Region refinement now includes projected polyline label cuts and validated
merge/re-segmentation operations. Preview faces carry `part` = patch id.

The actual default car panel passes the 0.2 mm p95 and seam acceptance tests.
The rotated labelled-panel case measured p95 0.14165 mm, zero seam gap and a
normal jump about 0.00000121 degrees. Its worst sampled error is 0.62253 mm,
so the stronger 0.2 mm maximum-error gate remains a failure. Neither trimming
nor partial-edge joins are implemented; missing internal joins fail a coverage
gate. See [current WP2 contract and limits](specs/connected-auto-surface.md).

## WP1 checkpoint — 2026-10-26

WP1 is implemented and verified; WP2 is next. The original baseline below is
retained as historical context. Current registry: 197 nodes, 172 implemented,
25 white-box. Workspace tests: 1,002 passed, zero failed, six ignored; the
release-only WP1 benchmark also passed separately. Build and Clippy passed,
the demo cooked 36 nodes with zero failures and two expected gates, the node
catalogue was regenerated, and the app opened with GPU-rendered viewports.

The 100k-triangle remesh took 1.35 s: 100% quads, mean edge 23.40 mm versus
23.37 mm target, conservative bidirectional deviation bound 4.60 mm.
See [implementation, measurements and limits](specs/field-aligned-remesh.md).
Guide steering and strict source-face constraints still warn when requested.
In the sandbox, set TEMP/TMP to `scratch/wp1-temp`; the existing process-tree
timeout test additionally needs localhost ping access outside the sandbox.

---

## 0. Current state (verified at handoff)

- `cargo build --workspace`, `cargo test --workspace` (**982 passed, 0 failed**,
  6 ignored) and `cargo clippy --workspace --all-targets -- -D warnings` are
  all clean.
- `vernon-cli cook examples/bodykit_pipeline.vrn`: 36 nodes cook, 32 reports,
  0 failing gates, 2 expected (`vehicle_prep.wheels_found` and
  `tet_mesh.placeholder`, declared under the nodes' `expect_fail`).
- The app launches, renders with wgpu over Vulkan and stays up. The toolbar
  verdict and the Gate Dashboard honour `expect_fail` the same way the CLI does.
- **197 node types: 171 implemented, 26 white-box.**
- Code size: app 10.3k lines; node crates 3–10k lines each; io 5.2k; graph 2.0k; core 1.7k.
- Specs (source of truth for behaviour, parameters and golden tests):
  `docs/specs/{alignment-deviation, automotive-cfd, capture-scanning,
  geometry-basics, performance-validation, reverse-engineering-surfacing,
  rhino8-mesh-tools, slicing-printing, solar-thermal, structural-fea,
  ui-dcc-conventions}.md`.
- Vetted suggestions per stage: `docs/research/k2/{general, ui, capture-clean,
  surface, print, cfd-fea, solar-flatten-perf}.md`. Only use rows marked
  ACCEPT or ADAPT; ignore REJECT rows.

How to find a node's code: `rg -n '"surface.quad_remesh"' crates`. Each node's
`.spec(...)` field names its spec file. Search that spec for the node id to
find its parameters and golden tests.

---

## 1. Work packages in priority order

Body-kit fit is the product's core value, so surface and print come first,
then the external solvers, then UI polish, then release.

### WP1 — Surface: quad remesh (`surface.quad_remesh`, crate `vernon-nodes-surface`)
- **Algorithm:** Instant Field-Aligned Meshes (Jakob, Tarini, Panozzo,
  Sorkine-Hornung, SIGGRAPH Asia 2015). Steps: smooth a 4-RoSy orientation
  field, then a position field, extract the graph, then extract the quad
  mesh. Implement from the paper (the reference code is BSD, so reading it is
  allowed, but write your own). Keep the existing params (target quad count /
  edge length, adaptive size, hard-edge angle, symmetry axis) as declared in
  the node and spec.
- **Acceptance:**
  - On `geo.car_panel` at default params: ≥ 90% of output faces are quads.
  - Mean edge length is within ±15% of the target.
  - The output Report has `quad_ratio`, `irregular_vertices` and
    `max_dev_mm` (Hausdorff to the input).
  - `max_dev_mm` is below 1% of the bounding-box diagonal.
  - Runs in under 5 s in release on 100k triangles.
  - Remove the white-box warning and add `.implemented()`.

### WP2 — Surface: auto-surface + refine regions (`surface.auto_surface`, `surface.refine_regions`)
- **Pipeline:**
  1. Region segmentation (output of `surface.segment_regions`). Refine
     regions by curvature-based region growing with merge/split; that is
     `refine_regions`.
  2. Lay out 4-sided patches per region: quad-dominant layout from the WP1
     field, or a region boundary split into 4 corners by curvature extrema.
  3. Least-squares bicubic B-spline fit per patch with thin-plate smoothing,
     after Weiss, Andor, Renner, Várady, "Advanced surface fitting
     techniques" (CAGD 2002).
  4. Insert knots where local deviation exceeds the tolerance, until it is
     met or a max knot count is reached.
  5. Enforce G1 across shared boundaries by averaging control-point
     tangents (C0 is the minimum).
- **Output:** patch geometry as a `Data::Json` NURBS description (degree,
  knots, control points, weights) plus a tessellated `Geometry` with prim
  attrib `part` = patch id. Use the existing NURBS JSON shape if the crate
  already has one (`rg -n "knots" crates/vernon-nodes-surface`).
- **Acceptance:**
  - On `geo.car_panel` with a 0.2 mm tolerance: p95 deviation ≤ 0.2 mm,
    reported through the existing surface-deviation metric names so
    `surface.deviation_check` gates it.
  - Boundary gap between adjacent patches ≤ 0.01 mm.
  - Normal-angle jump ≤ 1° (G1).

### WP3 — Surface: G2 mesh-to-surface and prismatic solid (`surface.mesh_to_g2_surface`, `surface.convert_prismatic`)
- **`mesh_to_g2_surface`:** convert a quad mesh (WP1) into bicubic patches:
  - Start from Approximating Catmull-Clark (Loop & Schaefer 2008), which is
    G1 at extraordinary vertices and C2 elsewhere.
  - Then raise continuity to G2 around extraordinary vertices with a
    guided-surfacing or Karčiauskas–Peters style construction, or report
    `g2_fraction` honestly if only partial.
  - The Report must state measured continuity: max position gap, max normal
    angle and max curvature jump across seams.
- **`convert_prismatic`:**
  - RANSAC primitive detection (Schnabel, Wahl, Klein 2007) for plane,
    cylinder, sphere and cone on mesh regions.
  - Output primitive parameters as Json, plus a tessellated solid whose prim
    attrib `part` is the primitive id.
  - Acceptance: a box with a drilled cylinder made by `geo` nodes yields 6
    planes and 1 cylinder (radius within 1%).

### WP4 — Surface: real STEP writer (`surface.write_step`)
- **Format:** write ISO 10303-21 (Part 21) files, schema AP214
  (`AUTOMOTIVE_DESIGN`) or AP242.
  - Faceted solids: `CLOSED_SHELL` with `FACE_SURFACE` + `PLANE` + `POLY_LOOP`.
  - Fitted patches: B-spline `ADVANCED_FACE` boundaries share compatible
    whole edges. Open components use `OPEN_SHELL` inside a
    `SHELL_BASED_SURFACE_MODEL` with
    `MANIFOLD_SURFACE_SHAPE_REPRESENTATION`; closed components with nonzero
    sampled volume use `MANIFOLD_SOLID_BREP` and
    `ADVANCED_BREP_SHAPE_REPRESENTATION`. See the implemented
    [contract and conservative joining limits](specs/step-export.md).
  - Units: millimetre (`SI_UNIT(.MILLI.,.METRE.)`). Build the standard
    product, context and header entities.
- **Acceptance:**
  - A unit test parses the output back: entity count, matching `#n` refs, no
    dangling references.
  - Add a test with `#[ignore = "needs FreeCAD"]` that passes a Python
    script file to an existing command-line importer and checks `Part.read`
    validity, solid counts and dimensions. Run with
    `cargo test -p vernon-nodes-surface freecad_import -- --ignored --nocapture`;
    `FREECADCMD` selects the executable. Added in WP4; external execution
    remains pending because no runtime is available locally.
  - Document that check in the node's help.

### WP5 — Print: body-line cut, smart partition, region select, snap joint (`vernon-nodes-print`)
- **`print.polyline_cut`:**
  - Project the user polyline (open prim from a curve or `geo` node) onto the
    mesh along a direction param (default: view axis or average normal).
  - Insert the projected curve as edges: split the triangles it crosses,
    with robust predicates (`robust` crate, MIT/Apache, pure Rust).
  - Separate the connected components on either side.
  - For thickened shells, cut both skins and stitch a side wall so each part
    is closed.
  - Output each piece with prim attrib `part`.
  - Acceptance: cutting a 3 mm shell of `geo.car_panel` with a diagonal
    polyline gives 2 parts, both closed (`validate()` + an edge-manifold
    check), with total volume preserved within 0.5%.
- **`print.smart_partition`:**
  - Partition a part that is too big for the bed into the fewest printable
    pieces. Follow the Chopper method (Luo, Baran, Rusinkiewicz, Matusik,
    SIGGRAPH Asia 2012): beam search over cutting planes, scoring part
    count, fit to the printer volume (existing bed-fit logic), seam length,
    seam on high-curvature/body lines (preferred) and connector feasibility.
  - Acceptance: a 1200×600×300 mm box on a 350³ bed gives ≥ 1 valid plan,
    every part fits, and the Report lists `parts`, `seam_length_mm` and
    `min_fit_margin_mm`.
- **`print.cut_region_select`:** select the face region for a cut by
  bounding box, normal cone or group, and emit a prim group consumed by the
  cut nodes. It is a small, deterministic node.
- **`print.snap_joint`:**
  - Generate cantilever snap-fit hooks on a seam (hook length, thickness,
    undercut, angle).
  - Check permissible strain with the standard cantilever formula
    ε = 1.5·t·y / l² (rectangular section) against a material
    allowable-strain table. Put that table in `vernon-core` materials next to
    the existing FDM rows; cite a public snap-fit design guide in the spec.
  - Report `strain_pct` vs `allowable_pct` as a gate.
- **`print.flavour_check`:**
  - Parse G-code. Validate commands against a per-flavour allow-list
    (`marlin`, `marlin2`, `klipper`, `reprapfirmware`, `repetier`) from
    `docs/specs/slicing-printing.md`.
  - Also check temperature, bed-size and extrusion-mode consistency.
  - Report unknown commands as warnings and out-of-bounds moves as errors.
- **`print.profile_build`:** emit OrcaSlicer/PrusaSlicer printer, filament
  and process profile JSON/INI from Vernon's presets (keys per the spec).
  Validate by a round-trip parse.
- **`print.script_bridge`:** leave white-box. It targets OrcaSlicer 2.5's
  alpha Python plugin API, which is unstable. Update only the help text with
  the API version it was checked against.

### WP6 — Clean: conservative wrap (`clean.alpha_wrap`)
- **Algorithm:** implement "Alpha Wrapping with an Offset" (Portaneri,
  Rouxel-Labbé, Hemmer, Cohen-Steiner, Alliez, SIGGRAPH 2022) **from the
  paper only**: CGAL's implementation is GPL, so do not read it.
  - Acceptable simpler route: sparse voxel / octree offset surface, then
    dual contouring, then a shrink-wrap projection.
  - Params: `alpha` (mm) and `offset` (mm), plus the ones already declared.
- **Acceptance:** output is closed and manifold, and every output vertex is
  between `offset` and `offset + alpha` from the input (sampled check).
  Holes smaller than alpha close. 1M-triangle input finishes in under 30 s
  in release.

### WP7 — Capture: alignment (`capture.global_align`, `capture.multiway_align`)
- **`global_align`:** initial alignment needing no manual pose.
  - Compute FPFH descriptors (Rusu, Blodow, Beetz 2009).
  - Match with RANSAC, or with Fast Global Registration (Zhou, Park, Koltun 2016).
  - Then refine with the existing point-to-plane ICP.
- **`multiway_align`:** pose-graph optimisation, as in Choi, Zhou, Koltun 2015.
  - Edges are pairwise ICP results with information matrices.
  - Use Gauss-Newton or Levenberg–Marquardt on SE(3) via `nalgebra`, with
    line-process or switchable loop closures to reject bad edges.
- **Acceptance:**
  - Synthetic test: 5 noisy partial copies of `geo.car_panel`, each moved by
    a random rigid transform of up to 30°/200 mm, are recovered to ≤ 0.5°
    and ≤ 0.5 mm.
  - The Report has `rmse_mm` and `fitness`.

### WP8 — Capture: COLMAP photogrammetry (`capture.sfm_features_match`, `sfm_reconstruct`, `mvs_dense`, `mvs_mesh`)
- **Commands:** wire the real COLMAP CLI calls, BSD licence, run as a process:
  1. `colmap feature_extractor --database_path db.db --image_path images`
  2. `colmap exhaustive_matcher` or `sequential_matcher`
  3. `colmap mapper --database_path db.db --image_path images --output_path sparse`
  4. `colmap image_undistorter --image_path images --input_path sparse/0 --output_path dense`
  5. `colmap patch_match_stereo` (needs CUDA)
  6. `colmap stereo_fusion --output_path dense/fused.ply`
  7. `colmap poisson_mesher` or `delaunay_mesher`
- **Outputs:** read via `vernon_io::read`, then apply scale-bar correction
  through the existing capture nodes.
- **Error handling:** surface stderr in the Report; a missing CUDA device is
  a Warning that says to use `delaunay_mesher` instead of patch-match.
- **Tests:** add `#[ignore = "needs COLMAP"]` tests that use a public sample
  dataset. Don't commit images; the test downloads to a temp dir only when
  `VERNON_COLMAP_DATA` is set.
- **Native fallback stays white-box:** Poisson surface reconstruction
  (Kazhdan & Hoppe 2013 "Screened Poisson") may be implemented natively
  later as its own node.

### WP9 — CFD: real OpenFOAM runs (`cfd.mesh_build`, `cfd.solve_steady`)
- **Install:** OpenFOAM isn't on this machine. On Windows use WSL Ubuntu
  with `openfoam` from openfoam.org or ESI. Ask the user before installing.
  Add an exe-prefix param, e.g. `wsl -e bash -lc`.
- **`mesh_build`:**
  - Run `blockMesh`, `surfaceFeatureExtract` (or `surfaceFeatures`),
    `snappyHexMesh -overwrite`, then `checkMesh`.
  - Parse `checkMesh` output for cell count, max non-orthogonality, max
    skewness and "Mesh OK".
  - Report these as gates: non-ortho < 70, skewness < 4.
- **`solve_steady`:** run `simpleFoam`. Optionally `decomposePar`, then
  `mpirun -np N simpleFoam -parallel`, then `reconstructPar`.
  - Parse residuals from the log and `postProcessing/forceCoeffs*/0/coefficient.dat`.
  - Feed the existing convergence-gate and force-coefficient nodes.
- **Validation:**
  - Ahmed 25° and 35° and DrivAer cases with the targets and tolerances in
    `docs/specs/automotive-cfd.md` (goldens G13–G24, e.g. DrivAer Cd within
    5% of 0.275/0.319/0.277).
  - Tests that need OpenFOAM are `#[ignore = "needs OpenFOAM"]`.
- **Hand-off to performance:** wire CFD force coefficients into
  `perf.vehicle` in the example scene (currently CdA/ClA are typed in by
  hand).
  - Feed it a CFD report via the existing `keys` aliases.
  - Keep the guard that refuses a placeholder CFD result (CdA = 0) unless the
    user overrides it.
- **Transient DDES/SBES:** add a transient DDES/SBES case writer (flagged
  only so far), using `pimpleFoam` with `kOmegaSSTDDES`. Write files only;
  real runs are optional.

### WP10 — FEA: tet meshing and native linear solver (`fea.tet_mesh`, `fea.solve_linear`)
- **`tet_mesh`:** DONE. Runs fTetWild (MPL-2.0, run as a process):
  `FloatTetwild_bin -i in.stl -o out.msh -l <rel_edge> -e <eps>`.
  - `msh4.rs` parses Gmsh 4.1 `.msh`, ASCII and binary, into nodes and C3D4
    elements.
  - The voxel fallback stays. `placeholder` is 0 when fTetWild ran, 1 otherwise.
  - The example does not run fTetWild, so that node keeps its `expect_fail`.
  - The `#[ignore = "needs fTetWild"]` end-to-end test has not been run yet:
    the program is not installed here. Tests use a stand-in script, and the
    real program's output format is still unconfirmed.
- **`solve_linear`:** (done 2026-10-31, except the external cross-check below,
  which has not been run) native reference solver for C3D4 and C3D10.
  - Element stiffness from linear/quadratic tet shape functions; orthotropic
    material via the existing material model.
  - Assemble in CSR and solve with preconditioned conjugate gradient
    (Jacobi or IC0). Pure Rust; `nalgebra-sparse` is acceptable.
  - Output displacement and von Mises per element.
- **Acceptance:**
  - Cantilever 200×20×20 mm, E = 2 GPa, tip load 100 N, compared with
    δ = PL³/3EI: C3D10 within 3%, C3D4 within 15%. Document that linear tets
    are stiff.
  - Results agree with CalculiX on the same deck within 2% (an
    `#[ignore = "needs CalculiX"]` test that runs `ccx`).
  - Measured (`crates/vernon-nodes-fea/src/cantilever.rs`, beam theory 10.000 mm):
    C3D10 -0.34% (20x2x2 cells), C3D4 -6.32% (80x8x8 cells; -49% at 20x2x2).
    The node stays white-box and never passes a gate until the `ccx` test has
    been run; then drop the flag and `estimate_only`, and fix the four-node
    wording in `nodes_result.rs`.

### WP11 — Flatten and performance white-box nodes
**Done:** both nodes now run their real algorithm and are no longer white-box
(workspace: 192 implemented, 5 white-box). `flatten.stretch_aware` runs
local/global ARAP iterations (closed-form 2x2 rotation per triangle, one sparse
solve per iteration) from the LSCM start, and reports `strain_max_pct`,
`strain_p95_pct` and `area_distortion_*_pct`. Measured: a half-cylinder patch
(R 100, height 200 mm) has max strain 2.9e-8 %, p95 1.2e-8 %, max area
distortion 2.6e-8 % (5 iterations); a seam-cut closed tube 1.7e-8 % max strain.
On hemisphere-like caps ARAP beats LSCM: half angle 60 deg, max strain 15.24 %
vs 19.92 % and max area distortion 13.41 % vs 34.52 %; half angle 90 deg,
38.63 % vs 58.83 % and 32.14 % vs 113.50 % (p95 is about equal: 15.21 % vs
15.22 % and 38.58 % vs 38.45 %, so only the max and the area figures improve).
Degenerate triangles are skipped and counted, flipped triangles are reported as
inconsistent-winding edges, non-finite points are refused and disconnected
pieces are flattened separately, each with an explicit note and no NaNs.
`outside = down` mirrors the pattern, and the strain, area and fold checks now
run on the un-mirrored pattern (`flatten.conformal_map` too), so a mirrored
pattern is no longer reported as folded (a 264-triangle cap used to report 264
flipped); a note says so. Stretch-only and compress-only are weights of 10000,
not hard constraints, and the rigid and floppy materials still run the same
energy (both said in the report). Spec F2 (cap R 1000 mm, half angle 30 deg,
tested on 10 rings x 64 sectors): (a) holds: the free flatten changes the area
by -0.043 % and the boundary by +3.496 %, above the isoperimetric minimum of
3.454 % at that area (the minimum at a preserved area is 3.476 % on this mesh,
3.53 % on the smooth cap); (e) holds: mean boundary strain 2.4365 % with
[10,10,1,1] against 3.4958 % with [1,1,1,1]. Not met, and left as `#[ignore]`d
tests with the spec bounds unchanged: (b) `preserve_boundary` (edge weight 10)
leaves the boundary 2.4365 % longer, not within 0.1 %, with area -2.078 %
instead of <= -6.5 %; (c) stretch-only has 79.82 % of the area compressed above
the 0.01 % dead-band (worst 0.1446 %; area +2.31 %, which is >= 0); (d)
compress-only has 9.77 % stretched (worst 0.0174 %; area -6.668 %, which is
<= 0). No fold-free line search yet. `perf.tuft_analysis` reads a CSV
(`x,y,z,angle_deg`, optional `id` and `flow_deg`) or the JSON series and
classifies each tuft into the spec's classes: reversed if the mean angle is
more than `reversed_deg` off the flow, otherwise separated if it is more than
`attached_deg` off or the angle std-dev exceeds `sd_separated_deg`, unsteady
if only the std-dev exceeds `sd_attached_deg`, attached otherwise. The class
codes keep the spec's numbers, 0 attached, 1 separated, 2 reversed, with
unsteady added as 3 (spec golden test 18: tufts at 0, 20, 90 and 180 degrees
are attached, attached, separated, reversed). With a surface input each face
takes the class of the nearest tuft within `reach_mm` as prim attribs
(`tuft_class`, `Cd`); faces with no tuft in reach get -1 in `tuft_class`,
`tuft_offset_deg`, `tuft_sd_deg` and `tuft_dist_mm` (the help says so).
`separated_pct` (class 1 only) and `reversed_pct` (class 2) are reported for
all tufts and per region from the `part` attribute. Region names that differ
only in punctuation get distinct metric keys (`_2` suffix) and a warning
naming both. A JSON tuft without a numeric x, y and z is refused, as in the
CSV. On the test strip (2 regions, 4 tufts, one per class) the node gives 25 %
of each class, 0 % separated and 0 % reversed in the front region and 50 % and
50 % in the rear. Photo input stays white-box: a photo input or no CSV gives
the `white-box placeholder:` warning and no numbers. The class thresholds are
still Vernon-proposed. Verified: `cargo test --workspace` 1225 passed, 0
failed, 14 ignored; clippy with `-D warnings` clean; the example pipeline
cooks 38 nodes with 0 failing gates and 2 expected; the app launch was not
repeated after these fixes.

- **`flatten.stretch_aware`:** ARAP parameterisation (Liu, Zhang, Xu, Gotsman,
  Gortler 2008, local/global), initialised from the existing LSCM node.
  - Report max and p95 strain, plus area distortion.
  - Acceptance: a cylinder patch flattens with < 0.1% strain; a
    hemisphere-like cap has lower max strain than LSCM on the same input.
- **`perf.tuft_analysis`:**
  - Input is a CSV of tuft positions (x, y, z mm) and measured angles, or
    photos.
  - Implement CSV first: classify each tuft as attached, separated, reversed
    or unsteady by angle deviation and flutter (std-dev) thresholds, and map
    the classes onto the surface as prim attribs.
  - Photo input stays white-box.
  - Report `% separated` and `% reversed` per region.

### WP12 — File formats (`vernon-io`)
- **E57 reader (ASTM E2807):**
  - Physical file: 1024-byte pages with CRC-32C, an XML section, then binary
    sections.
  - Read cartesianX/Y/Z, colours and intensity of every `data3D` scan.
  - Apply each scan's pose.
- **LAS 1.2–1.4 reader:**
  - Header, point formats 0–10, scale and offset.
  - Units from the header or a param (LAS is usually metres, so convert to mm).
  - LAZ (compressed) is out of scope unless a pure-Rust permissive decoder
    exists and passes the AGENTS.md dependency rules.
- **STEP reader:** optional, faceted solids only.
- **Formats:** add the new formats to `Format::from_path` and `extensions()`.
- **Tests:** round-trip or golden tests on small synthetic files built in the
  test itself.

### WP13 — App and UI gaps (`app/`, `crates/vernon-render`)
1. **Concave n-gons in the display mesh:** DONE in WP13. The ear-clipper now
   lives in `vernon-core::triangulate`; the viewport, the triangle writers and
   the statistics panel all use `Geometry::triangulated()`.
   - The display mesh uses `Geometry::triangles()` (fan) and is wrong for
     concave n-gons.
   - Switch both renderers to ear-clipping (`vernon-io` has `tri.rs`; move
     the ear-clipper into `vernon-core` and reuse it).
   - Carry the `dist`/`temp_c`/`Cd` colour modes through.
   - Test: an L-shaped hexagon renders 4 non-overlapping triangles.
2. **Grid depth:** DONE in WP13. The GPU pass draws the grid depth-tested,
   plus a depth-only ground plane when the camera is above Z=0, so geometry
   below it is hidden; turning the grid off (G) removes the plane too. Vertical
   construction planes hide nothing. The CPU painter has no depth buffer and
   still draws the grid behind everything.
3. **vernon-render navigation and picking:** DONE in WP13. The crate had none
   of these to wire, so they were built: GPU ID picking (the CPU ray pick stays
   as the painter fallback), an always-on-top overlay pass drawing a new
   Move/Rotate/Scale gizmo with axis-constrained drags, and one `Camera` (f64,
   f32 at upload) in `vernon_render::camera`. The nav-cube maths moved to
   `vernon_render::navcube`; the cube is still painted by the app in egui,
   because its labels and icons are egui text and app icons.
4. **View cube extras:** DONE in WP13. Compass ring (letters look from that
   side, ring drag turns, north angle in Display), "Set Current View as Front"
   and Reset Front (session only, not saved with the scene), 0.3 s animated
   view transition with an off switch.
5. **Hierarchy tab:** DONE in WP13. `vernon_nodes_print::hierarchy` derives
   parent links, seam datums and pivots from cap faces; cut, split, joint and
   fit nodes write or carry them as prim and detail attributes (spec
   `slicing-printing.md` section 5b). The Hierarchy tab shows the part tree and
   a seam list. Parent links are a derived assembly order: there is no way to
   edit them by hand, and datums are per seam, not per joint feature.
6. **Keyboard and clipboard:** DONE in WP13, except the chords below.
   - X (explorer find box) and Shift+G (hide/show geometry objects) are bound.
     22 chords stay reserved because their features do not exist: Alt+S,
     Insert, F5-F8, `-`, `=`, Space, C, F2, Shift+F, Alt+X, O, Alt+Z, I and
     Shift+H/L/C/S/W/P. Bind each as its feature lands. X searches object
     names only, not commands.
   - `keymap.json` overrides load at startup from the config dir
     (`%APPDATA%\Vernon`, `~/Library/Application Support/Vernon` or
     `$XDG_CONFIG_HOME/vernon`); invalid entries warn in the log.
   - Ctrl+X/C/V cut, copy and paste the selected nodes and internal wires as
     clipboard JSON, pasting with fresh ids and unique names. Pasted text is
     treated like an opened file (run switches off, trust prompt for programs
     or write paths); access keys are not copied. Selecting an object selects
     only its top node, so copy does not bring the rest of its stack.
7. **Load Demo Pipeline:** DONE in WP13. The menu offers "Replace Scene with
   Demo" (guarded by the unsaved-changes prompt) and "Add Demo Copy".
8. **Screenshot check:** DONE in WP13. A per-monitor DPI-aware capture of
   `--demo` shows the Command Panel, time slider and status bar uncropped.

### WP14 — Consistency clean-ups (done 2026-10-30)
- **Material tables:** there is one table, `vernon_core::materials`. Print, FEA
  and solar all read it; `crates/vernon-cli/tests/consistency.rs` checks every
  material, as printed and annealed, through the real nodes. ASA's 0.45 MPa HDT
  is 100 °C, the low end of the report's 100–103 °C range. (An earlier version
  of this note said 101.5, which is only a test fixture.)
- **Heat-check treatment:** with a material report wired in, `solar.hdt_check`
  now states the report's treatment instead of its own `annealed` flag and warns
  when the two differ. The FEA material reports publish `annealed`.
- **Air state:** `crates/vernon-cli/tests/pipeline.rs` asserts one density in the
  performance nodes, the structural load and the CFD case, including the
  `rhoInf` written to `controlDict`. The solar ambient is a separate setting, so
  the test asserts it matches the air state's temperature.
- **Metric names:** not renamed. `vernon_core::report::keys` now has the
  deviation and CdA/ClA names and alias lists.
- **Tests:** the 4 clean-crate performance tests and the GPU smoke test pass;
  timings are in `crates/vernon-nodes-clean/README.md`.

### WP15 — Validation against tools the user owns (golden tests)
- **Allowed:** recording outputs of legally licensed tools on inputs we
  created (CLEAN_ROOM.md, "Allowed sources" item 3). Do not inspect their
  binaries or install folders.
- **Per golden test:**
  - Create the input with Vernon nodes and export it with `geo.file_write`.
  - The **user** runs the reference tool and saves the output under
    `tests/golden/<tool>/<case>/`.
  - Add a test comparing Vernon's output within the tolerance stated in the
    matching spec's "golden tests" section.
- **Cases:**
  - CloudCompare C2M distance on a scan vs mesh.
  - OrcaSlicer slice of a 3MF from `print.package_write`: layer count and
    filament grams within 2%.
  - Blender/Houdini only for geometry ops (e.g. Catmull-Clark level 2 vertex
    positions).
- **Process:** write a `tests/golden/README.md` explaining the procedure.
  Ask the user to run the tools; don't automate the proprietary ones.

### WP16 — Release
1. **CI:** add `.github/workflows/ci.yml` with a matrix of `windows-latest`
   (x86_64-pc-windows-gnu with MSYS2 MinGW), `ubuntu-latest` and
   `macos-latest`. Each runs build, test, `clippy -D warnings`, the example
   cook and the CLI clean-room test.
2. **Licence audit:** add `cargo-deny` (`deny.toml`) with the AGENTS.md
   licence allow-list and a ban on `cc`/`cmake`/`bindgen` for the Windows
   target. Run the Linux check from CLEAN_ROOM.md
   (`cargo tree -e normal,build --target x86_64-unknown-linux-gnu -i cc`).
3. **Packaging:** `cargo build --release -p vernon -p vernon-cli`; zip with
   LICENSE, NOTICE, README and examples. On Windows, bundle the MinGW
   runtime DLLs only if `vernon.exe` needs them (check with `objdump -p` for
   `libgcc_s`/`libwinpthread`).
4. **Changelog:** write `CHANGELOG.md` for v0.1.0-alpha.
5. **Publishing:** do **not** create a GitHub repo, push, tag or publish
   without the user's explicit go-ahead. Prepare everything and tell them the
   commands.

---

## 2. Known traps

- **Windows linker:** the GNU toolchain needs WinLibs `dlltool`. If you see
  `error calling dlltool` or `cannot find -lshlwapi`, you skipped
  `. .\scripts\dev-env.ps1`, or `~/.cargo/config.toml` lost its
  `linker`/`-Cdlltool` lines (both point into
  `%LOCALAPPDATA%\Microsoft\WinGet\Packages\BrechtSanders.WinLibs.POSIX.UCRT_*\mingw64\bin`).
- **wgpu backends:** `app/Cargo.toml` depends on `wgpu = "25"` with default
  features on purpose. eframe alone leaves the platform backends off, and the
  app then panics with "No wgpu backend feature".
- **Gates and `expect_fail`:** scenes may declare expected metric failures
  per node (`NodeInstance::expect_fail`). The CLI and the app's toolbar and
  Gate Dashboard both honour them; error notes are never excused. Keep the
  two in sync (`vernon_graph::gate::report_state_expecting`).
- **Cache purity:** cooks are cached by a parameter + upstream signature.
  - A node that reads files must include a file stamp in its signature (see
    how `writes_files`/file-stamp handling works in `vernon-graph/src/graph.rs`).
  - Side-effecting nodes must be marked so they are not treated as pure.
- **Long cooks:** cooking runs off the UI thread with cancellation. External
  processes are killed by tree kill (`taskkill /T /F` on Windows) and have
  time limits. New external runners must reuse
  `vernon_core::process` rather than spawning ad hoc.
- **Untrusted scenes:** opening a `.vrn` that would run external programs
  shows a trust prompt. New nodes with `run` params are covered
  automatically if they use the shared `run`/exe param conventions. Check
  this with a test.
- **Clean-room test:** `crates/vernon-cli/tests/clean_room.rs` fails on
  trademarks in labels/help. Run it before every commit.

## 3. Out of scope / needs the user
- Installing OpenFOAM (WSL), COLMAP (CUDA), fTetWild, CalculiX, FreeCAD:
  ask the user first and state what will be downloaded.
- Legal review of the clean-room process before a public release.
- Creating the public GitHub repository and choosing the org name.

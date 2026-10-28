# WP5 print tools implementation plan

Execute the approved HANDOFF WP5 in the existing local checkout, starting at
`2af8846` on `codex/wp5-print-tools`. One local commit after verification;
no installation of external programs, push or publication.

Use AGENTS.md and CLEAN_ROOM.md throughout: native Rust, permissive dependencies,
millimetres, deterministic output, no protected names in visible strings,
file effects confined to the cook work directory. Preserve existing node ids
and output indices. New parameters may extend the contracts.

## Task 1: Audit and concrete design
- [x] Inspect the seven named WP5 nodes, existing cutter/fit/joint/profile APIs,
  shared materials and published specifications. Delegate independent geometry
  designs; inspect firmware and profile documentation without GPL algorithm source.
- [x] Reuse the planar cutter through a piecewise-affine projected curtain,
  implement bounded deterministic cutting-plane beam search, and generate a
  cantilever hook rather than the current round plug. Explicitly document
  unsupported geometry rather than silently substitute an algorithm.

## Task 2: Body-line cutting and cut selection
- [x] Add failing regressions for a bent projected guide and the 3 mm car-panel
  shell: two closed manifold pieces, valid geometry, volume error below 0.005.
- [x] Refine triangles at projected guide knots, shear to a planar curtain,
  clip/cap, refine caps and invert. Reject invalid/non-monotone projections;
  extend endpoint segments to span the model. Preserve interpolated attributes.
- [x] Select faces by bounding box, normal cone or existing primitive group into
  `cut_region`. Cuts apply to whole connected components touched by selection;
  untouched components remain unchanged. Test selection consumption and state
  these component-level semantics explicitly.

## Task 3: Smart partition
- [x] Add a failing 1200x600x300 mm box / 350-cube bed acceptance test.
- [x] Replace grid stand-in with bounded deterministic beam search over cutting
  planes, scoring count, actual bed fit, seam length, visibility/curvature and
  connector feasibility. Retain original ports/parameters. Every chosen part
  must fit, with measured `parts`, `seam_length_mm`, `min_fit_margin_mm`.
- [x] Test deterministic output, already-fitting input, bounds and invalid input;
  report search bounds and absence of a global optimality guarantee.

## Task 4: Cantilever snap joints
- [x] Add failing hook geometry and analytic strain gate tests. Generate a real
  flexing hook/catch and mating recess with clearance on eligible planar seams.
- [x] Add a sourced material allowable-strain table beside core FDM materials.
  Gate `strain_pct = 100 * 1.5 * t * y / l^2` against `allowable_pct`;
  expose length, thickness, undercut and ramp angle. Preserve output ordering.
- [x] Verify closed/manifold output and honest fit/strain failures; document
  material assumptions, supported seam geometry and printed-material limits.

## Task 5: G-code conformance, profiles and bridge help
- [x] Add failing tests for unknown/dialect commands, compact numbered syntax,
  relative moves, G92 offsets, out-of-bed moves, heater limits and extrusion mode.
- [x] Implement documented per-flavour allow-lists for marlin, marlin2, klipper,
  reprapfirmware and repetier. Unknown commands warn; physical bounds errors
  fail. Unsupported coordinate-changing commands invalidate position certainty.
- [x] Verify documented preset key/types; emit printer/filament/process JSON
  and INI presets with independent round-trip parsing and validation. Preserve
  default JSON hand-off. Do not claim an external slicer import was performed.
- [x] Keep script_bridge white-box; update help to the publicly checked
  2.5.0-alpha API and date without implementing its unstable hooks.
- [x] Include connected File-input size/time stamps in graph cache signatures
  and cached File outputs in the file-watch stamp, with a regression proving
  an external G-code edit recooks its reader while unchanged files stay cached.

## Task 6: Review and delivery
- [x] Review each implementation against its brief and focused test evidence;
  fix material findings and run the covering tests again.
- [x] Regenerate catalogue; run workspace build/test/all-target Clippy and the
  36-node demo (zero failures, two expected gates), and visually check GPU launch.
- [x] Update README counts, current measured spec and HANDOFF, then commit WP5
  locally. Record exact results and outstanding limitations; WP6 follows.

Build environment: dot-source scripts/dev-env.ps1; TEMP/TMP=scratch/wp1-temp;
CARGO_INCREMENTAL=0. The existing process-tree test needs localhost ping access
outside the sandbox. Reports and transient evidence belong in ignored scratch/.

## Final checkpoint — 2026-10-28

All tasks above are implemented and verified under the current contracts in
`docs/specs/native-print-tools.md`. Final read-only reviews and independent
current-source probes found no remaining blocker. Follow-up regressions first
failed for named M-prefixed macros, converted-coordinate overflow, numbered
object markers, steep-curtain tolerance amplification, seam-id collisions,
empty selection and cancellation in a parallel guide frame; fixes pass.

The curtain fixes physical clipping/refinement/weld distances before the
shear, compares world positions when welding, validates its orthonormal frame,
and rejects unsupported roundoff conditioning or damaged capped solid output.
Both cut nodes reserve fresh cap ranges from the complete input and use the
same ids in metadata. No arbitrary-folded-guide or mesh-repair claim is made.

Fresh final evidence: workspace build and all-target Clippy pass; workspace
tests 1,109 passed, zero failed, seven ignored. All 150 print tests pass. Demo:
36 nodes, zero failures, two expected gates. Catalogue regenerated: 197 nodes,
183 implemented, 14 white-box. Vulkan GPU launch and the Split/Print ribbon
were checked visually; the empty application was closed afterwards.

The optional external slicer import and physical snap coupon checks have not
been performed. Snap material allowances remain explicitly assumed screening
values, and the bounded search does not prove global optimality. No solver
was installed and no files were pushed or published. One local WP5 commit is
the delivery checkpoint; WP6 follows.
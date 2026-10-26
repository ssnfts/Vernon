# WP2 connected automatic surfacing implementation plan

**Goal:** Complete HANDOFF WP2: refine segmentation and produce fitted NURBS patch networks with measured deviation and continuity.
**Architecture:** Reuse adaptive least-squares fitting and thin-plate smoothing. Respect input region labels. Split fitted tensor surfaces by exact knot insertion for four-sided subpatch layouts, and reconcile compatible neighbouring region boundaries with shared control points and derivative rows. Measure seams and final source-to-network deviation after reconciliation. Retain existing JSON and node parameters; add only the optional polyline geometry input needed by region splitting.
**Constraints:** AGENTS.md and CLEAN_ROOM.md; pure Rust, original visible text, deterministic cooks, millimetres, no external installs. Local checkout and one commit for WP2; no remote operations. Acceptance on geo.car_panel: p95 <=0.2 mm, adjacent seam gap <=0.01 mm and G1 normal jump <=1 degree. Report unsupported topology or unmet tolerances honestly; do not hide skipped areas.

## Task 1: Surface network primitives
- [x] Add tests for exact non-rational tensor-surface splitting (preserve shape and derivatives), compatible seam matching with reversed edges, and position/tangent reconciliation. Include independent seam samples and invalid/ambiguous cases.
- [x] Implement focused `patch_network.rs` helpers using existing NurbsSurface/PatchSet; expose exact split and stitch/measure APIs. Boundary IDs identify four edges in order u0/u1/v0/v1. Preserve intentional hard joins under g0; g1 is explicitly requested.
- [x] Test before/after and report commands/evidence in scratch/wp2-network-report.md. No commits by subagent.

## Task 2: Region refinement
- [x] Test meaningful curvature resegmentation and merge validation; add projected user-polyline split of labelled faces via optional geometry input, without altering source topology.
- [x] Keep original parameters, preserve attributes, handle nonexistent/invalid groups cleanly, remove placeholder and mark implemented only when supported operations work. Test deterministic output and disconnected regions.
- [x] Modify segment.rs only; report evidence in scratch/wp2-regions-report.md. No commits by subagent.

## Task 3: Node integration and acceptance
- [x] Add red node tests using actual car panel, explicit multiple patches, rotation, pre-labelled adjacent regions, G1, round-trip JSON and part IDs.
- [x] Replace independent one-patch-per-region adapter with network layout, seam reconciliation and final deviation measurements. Preserve metric aliases. Include part on preview. Bounds/resource failures cleanly reported.
- [x] Keep degree and control-net limits; unresolved continuity/tolerance emits warning and failing numeric gate. Expose measured max seam gap and normal jump and number of matched seams, never claim continuity when none was checked.

## Task 4: Review, verification and handoff
- [x] Read-only code review and fix actionable findings; full build/test/clippy/demo/catalogue/GUI checks from AGENTS.md.
- [x] Update spec, README counts and dated HANDOFF progress. Single verified local commit for WP2.

Commands: dot-source scripts/dev-env.ps1, set TEMP/TMP to scratch/wp1-temp, CARGO_INCREMENTAL=0. The process-tree timeout test needs an unsandboxed workspace run for localhost ping. Full baseline at f8aebda: 1002 tests passed and build/clippy/demo/GUI verified.

Verification 2026-10-26: 1,023 workspace tests passed, six intentionally ignored; build, Clippy, catalogue, demo (36 nodes, zero failures, two expected gates) and GPU app launch passed. Review findings on seam coverage, near-knot splitting and resource limits were reproduced and fixed. The required p95 and seam gates pass on the actual panel; maximum-error outliers remain explicitly gated and documented. One local WP2 commit, no remote operations.

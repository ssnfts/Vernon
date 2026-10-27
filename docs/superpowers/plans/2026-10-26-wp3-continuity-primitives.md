# WP3 measured continuity and prismatic recognition plan

**Goal:** Execute HANDOFF WP3 locally: native subdivision-derived patches with honest G2 coverage, and analytic primitive recognition with labelled tessellated output.
**Architecture:** Reuse the existing regular Catmull-Clark patches and extraordinary-vertex approximation. Expected seam pairs come from cage topology (unpacked face order), not successful geometric matches. Report position, tangent-plane and geometric curvature jumps after any reconciliation, with partial G2 explicitly measured. Prismatic recognition uses deterministic RANSAC hypotheses, geometric residual checks and least-squares refinement for plane, sphere, cylinder and cone, retaining source tessellation and analytic-face metadata.
**Constraints:** AGENTS.md/CLEAN_ROOM.md; pure Rust and permissive existing dependencies only; no proprietary folders/source/assets. Preserve node ids and parameters, original visible text, millimetres and existing report output indices. Add prismatic shell Geometry after existing Json and Report outputs so old output index1 stays Report. No push/install. One verified WP3 commit.

## Task 1: Subdivision conversion
- [x] Read public Loop/Schaefer and guided-surfacing methods; document exactly implemented construction and any partial coverage.
- [x] Add failing tests for regular curved quads, extraordinary vertices, gaps, partial G2 and post-edit deviation; preserve max_degree and fit_level semantics and bounded work.
- [x] Reuse exact regular patches; improve extraordinary adjacency where practical, without claiming an unimplemented cap. Expected seam topology must be complete and measured, never omit missing joins. Remove placeholder and mark implemented when conversion plus partial-coverage contract is real.
- [x] Implement in subd_nurbs.rs/new focused helper only; report scratch/wp3-g2-report.md. No subagent commits.

## Task 2: Prismatic recognition
- [x] Add red tests: closed drilled block (6 planes, 1 cylinder, radius within1%), transformed/noisy shapes, sphere, cone, ambiguous/degenerate input, unfit fallback and deterministic labels.
- [x] Deterministic RANSAC+refit primitives from input region groups or automatic supports; report real residuals and support/coverage. Preserve existing prismatic_brep Json fields, add analytic parameters and third shell Geometry with part=primitive id.
- [x] Closed manifold inputs remain tessellated solids; open/nonmanifold inputs reported honestly. Mark implemented and remove placeholder when acceptance passes. No trimmed CAD kernel claim; STEP belongs to WP4.
- [x] Own convert.rs and new prismatic.rs. Report scratch/wp3-prismatic-report.md. No subagent commits.

## Task 3: Geometric seam measurement and integration
- [x] Implement independent seam measurement from explicit expected pairs: analytic second jets on nonrational B-splines; compare shape operators in one physical tangent frame so curvature is invariant to parameter scaling/reversal. Test planar, cylindrical/parabolic, C1-but-not-G2 and degenerate seams, including nonunit domains.
- [x] Provide SeamRef {a,side_a,b,side_b,reversed}, old patchops side convention 0=v-min,1=u-max,2=v-max,3=u-min. Report max_gap_mm, max_normal_deg, max_curvature_jump_inv_mm, g1_fraction, g2_fraction and invalid/missing samples. G2 thresholds 0.01mm,1degree,0.001 inverse-mm. No seams or singular samples cannot certify success.
- [x] Update registry status tests only after both nodes are real; full code review and acceptance verification.

## Task 4: Completion
- [x] Build/test/clippy workspace; demo cook (36 nodes,0unexpectedfailures,2expectedgates); regenerate catalogue; GPU app launch.
- [x] Update specs, README counts and dated HANDOFF with exact evidence and limits; one local commit.

Environment: scripts/dev-env.ps1, TEMP/TMP scratch/wp1-temp, CARGO_INCREMENTAL=0. Full workspace tests require unsandboxed localhost ping. Baseline9157d1b:1023passed0failed6ignored; build/clippy/demo/GUI verified.

Completed 2026-10-27. Full workspace: 1,039 passed, zero failed, six ignored. Build/Clippy/catalogue/demo/GPU launch passed; final review clean. Measured G2 is full on regular fixtures and explicitly partial at extraordinary vertices. Drilled-block acceptance and transformed/noisy primitive fits pass. Global recognition budget and malformed-label review findings fixed with regressions. One local WP3 commit; no remote operations.

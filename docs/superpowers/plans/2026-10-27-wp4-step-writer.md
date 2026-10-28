# WP4 STEP writer implementation plan

**Goal:** Complete HANDOFF WP4 with valid AP214/AP242 entity structure, shared patch topology, validation and reproducible import checks.
**Architecture:** Move document/entity generation to focused step_format.rs, retain node API in step.rs. Use FACE_SURFACE+PLANE+POLY_LOOP for planar facets, ADVANCED_FACE+EDGE_LOOP for NURBS patches, actual shared VERTEX_POINT/EDGE_CURVE entities and per-component shells. Select representation types permitted by the EXPRESS entities. Independent step_validate.rs parses the Part21 subset and checks all references plus structural loop/topology rules. Node validates before writing, confines file effects to ctx.work_dir, and writes atomically.
**Constraints:** AGENTS.md/CLEAN_ROOM.md, pure Rust existing dependencies, original text, millimetres, deterministic content and names. Keep node ids/params/port ordering and public step::write_step API. One local commit, no push or program installation. No analytic CAD trimming/intersection claim. Full ISO/EXPRESS conformance is not implied by subset validation.

## Task 1: STEP document and topology
- [x] Public schema research for faceted/advanced faces and representation rules. Add regression coverage for cube planar facets; nonplanar polygon triangulation; disconnected components; mixed inputs; rational surfaces; shared vertices and edge curves across adjacent/reversed and closed patch networks; invalid/nonmanifold/resource cases. The first runnable topology batch passed; observed red/green cases and limitations are recorded below.
- [x] Implement step_format.rs exposing Schema, StepOptions, StepSummary and write_step matching existing API. Summary retains old fields and adds solid_shells/open_shells/shared_edges. Split planar faces by connected component; no disconnected CLOSED_SHELL. Reuse patch corner vertices and compatible whole boundary edges, retaining orientation. Unmatched/partial patch edges produce honest open shells. Rational data preserved.
- [x] Header/product/mm context complete; face count limits include emitted/split facets and patches. Numeric formatting/string escaping deterministic and parseable. Reject degenerate/invalid geometry. Report evidence scratch/wp4-format-report.md. No subagent commits.

## Task 2: Independent Part21 validator
- [x] Add standalone parser tests first: nested lists, strings/escaped apostrophes/comments, simple/complex entities, numeric syntax, duplicate/zero ids, dangling refs, truncation, illegal face/representation combinations and broken edge loops.
- [x] Parse subset to AST and validate references/entity counts, planar facet entity linkage, NURBS knot multiplicities, edge-loop endpoint identity, shared-edge incidence and representation item choices. Public API validate(content:&str)->Result<ValidationSummary,String>. Tests include hand-authored fixtures independently of writer.
- [x] Own step_validate.rs only; report scratch/wp4-validator-report.md. No commits.

## Task 3: Node adapter and external import test
- [x] Replace old inline writer with reexports; adapt tests to real schema types and shared topology. Keep outputs File,Report. Remove whitebox only when format/validation tests pass.
- [x] Apply validation before writing; validate mesh/patch input, safe paths and deterministic filename; atomic replacement and clean failure. save_to must resolve within ctx.work_dir per AGENTS rule (document this constraint).
- [x] Add ignored `needs FreeCAD` test using FREECADCMD or pre-existing executable, with script import Part/Part.read(...)/isValid/shape dimensions. Do not install software; if unavailable, explicitly report test not run. Probe existing import runtime where practical.

## Task 4: Review and completion
- [x] Read-only code review and fixes; workspace build/test/clippy/demo/catalogue/GPU launch.
- [x] Update specs, README counts, dated HANDOFF with actual validation evidence and limits; one verified local WP4 commit. Baseline9d0af24 has1039tests passing6ignored.

Environment: scripts/dev-env.ps1, TEMP/TMP scratch/wp1-temp, CARGO_INCREMENTAL=0. The existing process-tree test needs unsandboxed localhost ping. No FreeCAD command or Program Files install found in read-only probes.

Review follow-up, 2026-10-28: malformed degree/grid metadata now uses checked arithmetic; patch volume recenters control points around one component origin; boundary sharing requires exact raw knots/weights plus the physical control-point tolerance; atomic cleanup starts only after creating a temporary file. Each reported defect has an observed red/green regression. Final narrow review found no remaining blocker. The optional external import test is added but has not run because the runtime is unavailable; native checks do not establish an independent CAD round trip.

Final verification, 2026-10-28: workspace build and all-target Clippy pass; workspace tests 1,067 passed, zero failed, seven ignored. The STEP filter passes 36 tests with one ignored importer test (it also selects two existing subdivision tests). Demo: 36 nodes, zero failed, two expected gates. Catalogue regenerated; registry 197 nodes, 177 implemented and 20 white-box. The GPU application opened with four viewports on Vulkan and was closed after visual inspection. No dependency or external program was installed and no remote publication was performed. One local WP4 commit is the delivery checkpoint; WP5 follows.

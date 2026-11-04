# WP9 accepted16m/s replication implementation plan

> **For agentic workers:** Use subagent-driven-development or executing-plans;
> run serial verification only after every source writer freezes.

**Goal:** Bind the user-approved published replication to actual local geometry
and physical settings, then run bounded real validation with unchanged gates.

**Architecture:** Version the reference separately from generic case defaults.
Recognize measured prepared-surface content, not a selected preset/filename.
Carry matching geometry and physics evidence through the owned case/solve chain;
legacy/unknown cases remain readable but unqualified. A resolver must compare
the present force normalization and recipe to the bound case evidence.

**Stack:** Existing pure Rust CFD/core/graph/IO crates, approved external
OpenFOAM process, existing Windows/WSL persistent verification helpers.

## Global constraints

- User accepted this reference on2026-11-02; do not ask again.
- All-detailed F/E/N, mirrors, no engine-bay flow, moving ground/rotating wheels.
- U16m/s,rho1.184kg/m3,nu1.567e-5m2/s,mu1.855328e-5Pa s, q151.552Pa.
- Preserve CdF/E/N0.275/0.319/0.277 and maximum5% band, no fitted shift.
- Preserve12M half/24M full floor, three-grid acceptance, complete SST residuals
  RMS<=1e-5,two1000-sample windows,mean change<=0.2%,drift<=0.1%.
- Scope is published replication, not original-experiment authentication.
- No proprietary/GPL implementation source, new installs, push or publication.
- Node writes stay underctx.work_dir. Unknown or edited evidence fails closed.
- Do not modify or rerun old frozen queues. No product-wide speed/air change.
- Do not change shared WSL memory/restart settings without a concrete review.

## Tasks

- [x] Record accepted reference in research note and machine-readable contract.
- [x] Inspect current verification/ownership and retain Claude's body-only y+ fix.
- [x] Prepare a no-solver dictionary diagnostic using existing nodes. Correct its
  synthetic dry-air equivalent to codeR_DRY=287.058; verify actual case values.
- [x] ObservedRED thenGREEN for road-car preset16m/s and rejection of a coherent
  30m/s payload. `study.rs` agent owns this task; targets/area/floors unchanged.
- [x] Implement a focused reference module after reviewing the binding design:
  accepted profile constants, physics validation and typed versioned evidence.
  Do not add a manually enterable trusted reference string.
- [x] Inventory original component hashes and deterministic prepared triSurface
  fingerprints from the approved local archive. Create a bounded geometry-only
  ignored profile with positive completion evidence; no assets redistributed.
- [x] Test and integrate matching evidence into mesh/case/solve producers and
  benchmark resolver. Recheck identity and physical values on owned fresh/cached
  solves. Reject altered surfaces, different rear/underbody/wheels, wrong speed,
  air/area/ground/rotation, unsupported versions and malformed/absent evidence.
- [ ] Replace the old road-car harness hard-coded30 andwindow=n_iter/3. Use16,
  fixed1000,7000 initial maximum iterations, explicit air and dynamic wheel
  radius matching51rad/s. Record all choices in output/provenance. Add marker and
  fixed bounded profile; current old harness cannot certify this baseline.
- [ ] Resource preflight: WSL15.52GiB RAM and4GiB swap; hostdisk~306GB free at
  initial audit. Existing3.76M mesh peaked~10.5GB. Estimate np/grid/layer memory
  and allow only a bounded meshing-only sizing run with saved status. Do not
  lower12M or suppress memory gates to obtain acceptance. If capacity requires
  shared-system changes, prepare exact settings/impact before asking the owner.
- [ ] Run the qualified geometry/mesh protocol in fresh case folders; record
  actual cells/quality/layers/body y+, residuals, stationarity and raw force
  bands. Failed runs stay failed and do not complete WP9.
- [ ] Independent review and appropriate focused/real/full checks, catalogue and
  app startup, durable next action. Merge verified main changes before the WP9
  completion commit. Commit once only when all WP9 requirements actually pass.

## Evidence and ownership

Last Codex full1443/0/29-ignored source4738d65b predates Claude's y+ change.
Claude records the same counts and cleanClippy/demo afterward; do not treat the
old saved fingerprint as verification of a later tree. Actual current source
must receive new bounded checks as tasks land. Scratch reports:
wp9-replication-{preflight-20261102,binding-design,preset-tests}.md.

## Verified implementation batch — 2026-11-03

FocusedGREEN `wp9-mesh-recipe-green-20261103-a`:349/0/13ignored.
Actual geometry/profile/recipe check `wp9-replication-geometry-20261103-c`:
three cases, one test executed,0ignored, no CFD programs; all prepared content
identities unchanged. Root repaired JSON roundoff identity and actual-domain
boundary defaults after observed regressions. Independent recipe review found
no blocker under the quiescent-input, non-authentication contract.
Source:69942e2cb41d53cfb9b972d2869fccd201d2df3460d2291d8cf0c97e194284a2.
Full persistent queue `wp9-integrated-full-20261103-a` is running; do not edit
source or launch duplicate Cargo/CFD while its lock is held. The legacy road-car
harness has been corrected, but a fixed guarded sizing profile and real volume
mesh/solve acceptance still remain. Main consolidation is already integrated
on2e01920; do not repeat the old merge/stash instructions.

Final batch queue `wp9-integrated-full-20261103-c` passed all five steps:
1540/0/35ignored, clean all-target Clippy/build, demo40/0unexpected/5expected,
matching catalogue197/194/3. App startup12s/GPU Vulkan4x/nonzeroHWND checked;
no manual visual testing. Source a765d522c9b4f8992cae49a306bbd2fb18b1327cf9ecdec0c8166ecc242b88cb.
No worker remains. Next is the guarded sizing profile and actual physical runs;
WP9 remains incomplete and uncommitted.

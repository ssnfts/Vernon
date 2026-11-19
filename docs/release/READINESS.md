# Release readiness — 2026-11-19

Status: **UNVERIFIED current candidate; not ready for release approval.**

This record distinguishes source inspection and saved historical results from
executed checks of an integrated candidate. It was prepared as staged documentation
only. No build, Cargo command, external program, CI dispatch, package extraction,
Git operation or CFD run was performed. The coordinator owns checkout resolution,
integration and subsequent verification scheduling. CFD execution is
outside this release documentation task.

## Candidate and evidence ledger

| Item | Status | Evidence or requirement |
|---|---|---|
| Integrated candidate revision and clean status | UNVERIFIED | Coordinator must identify the final full revision and source fingerprint after integration; no Git status was queried here |
| Current build/test/clippy/demo/clean-room checks | UNVERIFIED | Required definition-of-done evidence must identify that candidate and retained logs; no checks executed here |
| CI matrix | CONFIGURED; execution UNVERIFIED | `.github/workflows/ci.yml` declares Windows GNU, Linux x86_64 and macOS arm64 checks plus cargo-deny; obtain run URL, revision, job conclusions and logs |
| Current dependency/licence audit | UNVERIFIED | `deny.toml` and `scripts/release/check-deps.ps1` exist; regenerate and review target-appropriate licence inventory after execution clearance |
| Historical package | RECORDED, not current acceptance | Existing `dist/Vernon-0.1.0-win64/VERSION.txt` records `0.1.0+adf225d25-dirty`, built 2026-11-14 09:53 UTC |
| Historical local package test | RECORDED: 56 passed, 0 failed | `scratch/wp16-selftest.log`, development machine, C: and D: installation cycles; not rerun and not clean-machine proof |
| Current package identity/integrity | UNVERIFIED | Record candidate commit, lockfile hash, ZIP SHA-256, manifest verification and exact self-test artifact identity |
| Clean-machine package acceptance | UNVERIFIED | No clean-machine evidence established; local development-host test must not substitute |
| Visible app acceptance | UNVERIFIED for candidate | Existing self-test checks hidden-process survival for 10 seconds; visible render/interaction needs separate evidence |
| Packaging reproducibility | UNVERIFIED | Current script uses wall-clock metadata; ZIP helper does not normalize entry timestamps. No repeat-pack comparison performed |
| Independent build reproducibility | UNVERIFIED | Pin and record compiler, linker, strip tool, dependencies and recipe; matching re-pack hashes alone would not prove matching independent builds |
| Limited slant-body CFD result | OWNER ACCEPTED 2026-11-19 | Cd 0.255 ± 0.023 (95%) vs 0.279, −8.6%; mean outside ±5% band, interval overlaps band. Fast/long-tier body work closed with stated error; road-car v1 unchanged/unmet |
| WP15 evidence | PENDING coordinator integration | Do not infer golden-test passes from parallel work being underway |
| Public release | NOT AUTHORIZED by this task | Owner decisions and separate publication authorization remain required |

Historical checksum record from the package log/sidecar:
`654ae3bc2cab6408c8bd912c04c4b801255d2040d21b70fc4b4088aea7ead28d`.
This is a recorded value, not an archive hash recalculated during this task.
The saved self-test log is not itself bound to that hash, so the coordinator must
establish the artifact identity before using it as candidate evidence.

## Direct evidence for changelog corrections

These are statements about inspected files, not freshly executed test results.
Paths are repository-relative for review after integration.

| Correction | Direct evidence | Limit |
|---|---|---|
| 201 catalogue entries, 199 implemented, two white-box | `docs/NODES.md`, All row and independently counted node rows | Catalogue was not regenerated; final candidate must be reconciled |
| Script bridge no longer listed as a placeholder | `docs/NODES.md`, `print.script_bridge`; `crates/vernon-nodes-print/src/nodes_pack.rs`, `impl NodeOp for ScriptBridge`, `.implemented()` and script-writing cook | Does not prove a current real-slicer round trip |
| Remaining catalogue placeholders | `docs/NODES.md`: `cfd.mesh_build`, `cfd.solve_steady` | Does not imply physical validation of other CFD nodes |
| Thermal strain exists | `crates/vernon-nodes-fea/src/refsolve.rs`, thermal strain assembly; `nodes_solve.rs`, `sm.thermal_strain = has_temps && card.alpha.is_some()` | No comparison run performed |
| Estimate-only guard is conditional | `crates/vernon-nodes-fea/src/nodes_result.rs`, `stiff`, `no_thermal`, `estimate_only = stiff || no_thermal` | Four-node reference results and temperature-bearing reference results without thermal strain remain estimates; other gates still apply |
| Thermal checks exist | `crates/vernon-nodes-fea/src/thermal_check.rs` and `tests.rs` | Presence of tests is not passing evidence |
| Constrained stretch/compress factors exist | `crates/vernon-nodes-solar/src/flatten_constraints.rs`, EdgeFactors and reweighted edge solve; `flatten.rs`, warning for nonuniform factors with one-sided constraints | Factors are not applied in `stretch_only`/`compress_only`; tests not run |
| CI configured, not proven executed | `.github/workflows/ci.yml`, matrix and check steps | No GitHub run queried or dispatched |

## November 19 accepted limited CFD result

The owner chose **"Accept, record as is"**. `docs/CONTINUATION.md` records
this decision at lines 192–195; `docs/research/wp9-fast-tier-plan.md`, section 7,
records the acceptance at lines 834–842. These records were read directly.

The accepted 25-degree slant-body long-tier result is **Cd 0.255 ± 0.023
(95%)** against reference **0.279**, **−8.6%**: the mean remains outside the
±5% band and the interval overlaps that band. The level-5 body has 913k cells;
SA-DDES uses CTU/182 and a 40-CTU averaging window. The recorded long-tier
stationarity, per-step and half-step gates passed. The window mixes both slant
states (ESS 5), so the wide interval and stated error must accompany the result.
This closes fast/long-tier validation work on this body as a gate-passing result
with its error recorded, **not a pass inside the ±5% band**. The road-car v1
contract (`wp9-replication-baseline-v1.json`) stays unchanged and unmet.

The staged changelog now carries this approved limited result rather than the
superseded pending-owner wording or older 7–10% accuracy approval. No new run,
threshold, road-car prediction or design-ranking acceptance is inferred.
`docs/HANDOFF.md` and `docs/VALIDATION.md` remain untouched under their owners'
control. This documented CFD acceptance does not close any WP16 package/CI gate.

## Current staging status

Updated November 19, 2026; **staged only, not integrated**. The coordinator supplied
current HEAD `bedc3d252473c6134f122e941130fd2885ffa83d`; this task did not query Git
or verify clean status. That supplied revision is context, not proof that these
staged files are integrated or that a release candidate passed checks.
Catalogue/numerics observations above were made November 18 and must be reconciled
at integration. No build, test or external check was launched for this revision.

## Acceptance checklist after integration and resource clearance

- [ ] Coordinator resolves checkout access and integrates only the reviewed staged files, reconciling any concurrent changelog changes.
- [ ] Record full candidate commit, source fingerprint, clean status and lockfile hash.
- [ ] Run the bounded verification queue when permitted; retain actual step exits, logs, ignored-test reasons and candidate binding.
- [ ] Obtain actual CI evidence for every matrix job and dependency check. Record failures and missing jobs without upgrading configuration to PASS.
- [ ] Review current licence/dependency results and candidate runtime imports; confirm whether MinGW DLLs are required by this build.
- [ ] Build and verify the package when permitted; bind its ZIP hash, contents, manifest and test logs to the candidate.
- [ ] Test on a disposable clean Windows machine without Rust/MinGW or optional solver/model installations: non-admin install, paths with spaces, portable mode, CLI demo, visible app rendering and interaction, shortcuts/association, uninstall and PATH restoration.
- [ ] Record OS, architecture, GPU/driver, environment and package hash. Record a second-drive test as PASS, FAIL or SKIPPED(reason), never silently imply it ran.
- [ ] Address reproducibility separately: normalize packaging metadata and compare two isolated repacks; claim independent build reproducibility only with separate pinned build evidence.
- [x] Record the November 19 owner acceptance for the limited slant-body result, including the stated error and unmet road-car v1 contract (documentary evidence only).
- [ ] Coordinator reconciles this limited claim at integration and records WP15 disposition; preserve remaining limitations.
- [ ] Complete owner legal/trademark/privacy and unsigned-package decisions; obtain separate approval before any tag, push or publication.

## Staging provenance and integration boundary

Original `CHANGELOG.md` SHA-256, read before staging:
`67841A8C184C01E9FDDE0284C56355D39658625D445BC6A05AB8AAA36C9A08CE`.

Prepared paths preserve intended repository-relative locations:

- `CHANGELOG.md`
- `docs/release/READINESS.md` (new)

Before integration, compare the current original changelog hash with the value
above. If it differs, review and merge the documentation changes rather than
overwriting concurrent work. Catalogue and source observations are a snapshot,
not a locked revision; reconcile them against the integrated candidate.
No script/source changes are included. Shared HANDOFF/VALIDATION files and CFD
ownership are deliberately outside this staged patch.

## Coordinator integration — November 19

The staged files above were integrated after checking original source hashes against
checkout bedc3d252473c6134f122e941130fd2885ffa83d. Earlier staging statements describe
the preparation stage. WP15 report-statistics tests are now present but await execution.
A bounded full verification run is planned as wp15-wp16-20261119-a; consult its saved
status and logs before claiming a pass. No release artifact has been rebuilt by this change.

## Verified local increment — November 19

Supersedes the pending integration/test statements above for this increment only.
Bounded run `wp15-wp16-20261119-b` passed build, workspace tests (1,862 passed,
0 failed, 54 ignored), clippy with warnings denied, demo (0 failing / 5 expected
gates), and catalogue generation. The generated catalogue is byte-identical to
`docs/NODES.md`; no node metadata or stage counts changed. All eight offline golden
tests passed. Source fingerprints before and after the queue were identical:
`9999c98f3bf8fc5fd62dbc7faa9e45f7cf9e34d8f6519d353248c0f9d5ecc8d3`.
Logs: `scratch/continuation/runs/wp15-wp16-20261119-b/`.

The built app stayed alive for a 10-second hidden startup probe and initialized
the Vulkan renderer. This is not a visual/interaction or clean-machine check.
Only documentation recording these results changed after the successful queue.

Run a failed a pre-existing capture diagnostic test: its five-second overall
budget expired before the fake CUDA-failure step under concurrent test load.
The same compiled test passed in isolation in 0.05 seconds. Its test-only budget
is now 60 seconds, with report notes included in assertion diagnostics; production
timeouts are unchanged. Run b passed the complete suite with this repair.

WP15 remains open for the independent slicer layer/mass golden. WP16 remains open
for current package build/acceptance, hosted CI evidence, clean-machine and visual
checks, and applicable owner release decisions. No package was rebuilt or published.

# WP8 real photogrammetry implementation plan

**Goal:** Validate and finish the four process-driven photogrammetry nodes with real external runs, truthful outputs, and opt-in public-dataset tests.

**Base checkpoint:** WP7, local commit `e560a35`; branch `codex/wp8-photogrammetry`. WP8 completed2026-10-30 under the measured contract below. Existing command builders were repaired from demonstrated failures.

**Constraints:** AGENTS.md/CLEAN_ROOM.md; native libraries remain permissive pure Rust. The external program is a separate process. No proprietary/Houdini folders, CGAL/TetGen GPL source, pushes, publication or installations without separate user authorization. Do not promote placeholders before actual measured validation. One local WP8 commit after the full definition of done.

## 1. Installation and capability decision

- [x] Read current checkpoint, standing rules, WP8 handoff and existing process driver. Confirm no worker is active and model usage is allowed.
- [x] Check configured executable, PATH, and four explicit conventional locations. No executable found; `VERNON_PHOTOGRAMMETRY_EXE` and `VERNON_COLMAP_DATA` are unset. This is not an exhaustive disk search.
- [x] Verify official release metadata and save a concrete approval request. Proposed separate tool: COLMAP 4.2.1, official Windows CUDA ZIP, 395 MB, destination `C:\Users\mabdu\REA\tools\colmap\4.2.1` outside the source repo. Official asset SHA256: `e9c5cbd84c2ea986d2e970a2473fc2d2e6b34a2cdcf5d3df2765c319a63af881`.
- [x] User explicitly approved download, unpack and local checks. Official hash verified; installed under the approved directory. Approval does not extend to drivers, CUDA toolkit or other solvers.
- [x] Bounded setup completed with saved status/help logs; actual CUDA stereo/fusion succeeded in first real run. No driver/toolkit changes.

Official release: https://github.com/colmap/colmap/releases/tag/4.2.1 . Official Windows launch instructions: https://colmap.github.io/install.html . The release asset listing was checked on 2026-10-29; no source implementation or binary was downloaded.

## 2. Fix demonstrated driver gaps

- [x] Read audit and preserve existing ports/planning behavior.
- [x] Observe RED and repair fusion mode; photometric/geometric tests pass.
- [x] Preserve bounded actual diagnostics and failing Report gates; requested real failures never return native placeholder geometry as success.
- [x] Actual CUDA invocation is authoritative; missing-device classification is specific, and warning describes sparse-model Delaunay arguments verified against installed help. Real CPU alternative trial remains optional validation.
- [x] Review discovery/help/argv, timeouts, malformed/empty/stale outputs; observed regressions and independent review saved under scratch/wp8-*.
- [x] Audit cache purity and work-folder writes; checked recursive metadata, uncertifiable-action refusal, external writer deferral and unavailable failed File ports have observed regressions.

## 3. Real opt-in acceptance

- [x] Add ignored public-data tests; default requires no tool/network. Explicit invocation without dataset fails rather than falsely passing. Full completion marker follows all assertions.
- [x] Prepare official South Building photos under approved local checks:128 images, no prebuilt model/database copied. Record URL/hash/size and absent separate image licence; local evaluation only, no images redistributed.
- [x] Execute all real steps on20 public photos:20 registered/2703 sparse/233744dense/293974 mesh vertices/554298triangles;204.997 s. Separate sparseDelaunay route also verified. Actual logs/parameters/hash manifest in the measured spec.
- [x] Apply100 mm numerical scale correction to real sparse and mesh output. Native offline height-field preview remains an explicit white-box warning; no physical building accuracy claim.

## 4. Review and delivery

- [x] Independent process/import/cache/contract review; repair findings with observed regressions. Final driver review found no remaining substantive blocker.
- [x] Fullqueue wp8-final-20261030-c completed, unchanged90b547... source:1171 passed / 0 failed / 10 ignored; build/Clippy/demo/catalogue pass. GPU Vulkan 4x MSAA launch and Capture ribbon checked. Prior stale-catalogue failure was corrected before this fresh run.
- [x] Update measured spec, README190/7counts, HANDOFF, generated catalogue and durable CONTINUATION.
- [x] Save one local WP8 completion commit containing this plan; identify with git log. Next: WP9's separate installation decision in the prepared2026-10-30 plan.

## Real-run checkpoint

Run `wp8-real-20261030-a` failed at import after every external command exited0.
It produced232332 fused points and a Poisson mesh of297215 vertices/561377
triangles,21 of which had exact zero area. The importer now removes collapsed
triangles with an explicit count/warning and preserves usable polygon area,
primitive attributes and groups. Nonfinite or entirely collapsed meshes still
fail. Initial repair RED9pass/4fail then GREEN13; independent polygon review
RED0pass/1fail then GREEN14. Graph action review RED3fail then GREEN26.

Fresh real run `wp8-real-20261030-b` launched2026-10-30 08:07UTC with source
fingerprint64b54e8147204386444277762e4f566b2888851002a7733c55506364238272e6.
Inspect persistent status and actual logs before claiming acceptance. Source
was frozen while its exclusive lock was held. Runb passed all assertions;
the complete measurements and limits are in docs/specs/real-photogrammetry.md.

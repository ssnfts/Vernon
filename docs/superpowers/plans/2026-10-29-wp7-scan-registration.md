# WP7 native scan registration implementation plan

> Use subagent-driven development for the independent kernels and verification-before-completion before delivery.

**Goal:** Replace global and multiway alignment placeholders with feature registration and robust pose-graph optimization, preserving existing node contracts.

**Architecture:** `global_registration.rs` supplies bounded FPFH descriptors and deterministic RANSAC. The existing point-to-plane ICP refines its pose. `pose_graph.rs` supplies a gauge-fixed SE(3) LM solve with pairwise information matrices and switchable loop constraints. `registration.rs` remains the node adapter and reuses its merge/ICP helpers.

**Constraints:** AGENTS.md and CLEAN_ROOM.md; pure Rust permissive dependencies; mm, Z up; no proprietary/GPL source or external installation; one local verified WP7 commit, no remote actions. The user's ordered handoff and continuation authorization are the design approval. FPFH/RANSAC is the chosen published alternative to FGR; requesting an unimplemented method must return an explicit error. Maintain six scan slots and geometry/JSON/report output order. Add `rmse_mm` and `fitness`. Preserve existing ICP behavior except for demonstrated defects.

## 1. Method contract and kernels

- [x] Write a clean-room method contract from primary FPFH and robust reconstruction references, with exact edge frames and bounded failure behavior.
- [x] Observe failing behavior tests before implementation. Implement 33-bin FPFH, neighbourhood-normal handling, feature matching, geometric sample checks and deterministic RANSAC with confidence/iteration limits. Reject invalid/featureless requests rather than return PCA as feature registration.
- [x] Implement SE(3) exp/log, information-weighted LM normal equations, fixed reference, uncertain loop switches, pruning and reoptimization; add observed regressions for rotation/translation drift, bad closures, invalid information and disconnected graphs.
- [x] Use nalgebra with default features disabled and std enabled; verify the permissive dependency resolve and regenerate licence inventory. Preserve current Rust source compatibility where feasible.

## 2. Node integration and measured acceptance

- [x] Connect Global Align to actual feature registration and optional existing point-to-plane ICP. Report actual coarse/final fit and matching counters; preserve geometry/transform/report outputs.
- [x] Build pairwise edges for up to six scans from feature initialization and ICP information, initialize a connected tree, optimize and expose actual edge confidences/pruning. Reject disconnected or ill-defined requests. Merge using optimized poses.
- [x] Replace only the two implemented placeholder flags and update registration assertions after behavior exists. Keep unsupported FGR explicit; do not ignore parameters silently.
- [x] Add an integration fixture using five noisy partial copies of the real `geo.car_panel`, with known independent rigid poses up to 30 degrees / 200 mm. Measure pose errors against ground truth, <=0.5 degrees and <=0.5 mm, not just ICP residuals. Verify `rmse_mm` and `fitness` reports, fixed reference, deterministic output and finite results.

## 3. Review and delivery

- [x] Independent kernel/integration review; repair concrete findings with covering regressions.
- [x] Freeze source and run one bounded persistent full verification queue. Inspect build/tests/Clippy/demo/catalogue logs, ignored counts and exact fixture measurements. Check GPU launch separately.
- [x] Update measured spec, node catalogue, README counts, HANDOFF and CONTINUATION. Save precise evidence and next action if interrupted; WP7 stays incomplete until all checks pass.
- [x] Commit WP7 locally once verified. Next ordered package is WP8, whose external installation gate remains in force.

## Measured progress

Kernel RED -> GREEN and numerical review completed. A fine-scale FPFH
ambiguity on the real smooth panel was reproduced, then repaired through an
explicit [v,2v,4v] feature pyramid, selecting after fine ICP. Actual seeded
arbitrary-axis/shuffled five-panel fixture: worst errors 0.035682964 mm and
0.005842080 degrees; final RMSE 0.030022946528821624 mm and fitness
0.8625538794822465. Capture80 and CLI2 focused tests pass, as does all-target
Clippy. Slot/reference and disabled-loop regressions pass. Final adapter
review has no remaining blocker. Vulkan GPU viewports and Capture ribbon
were inspected and the app closed normally.

Full detached queue: wp7-final-20261029-a, source fingerprint
`e50c8d7e9de3f508e6d3c2155e7cdc61f8f692fd902b283121963d9bec7adaed`.
It must finish successfully and logs be inspected before delivery/commit.
## Completed full checkpoint

The detached queue completed at 10:12:45 UTC with all five steps passed and
unchanged fingerprint. Full workspace: 1,131 passed, zero failed, eight ignored; all-target
Clippy clean; demo: 36 nodes, zero failures, two expected gates; catalogue: 197 nodes, 186 implemented, 11 white-box.
Vulkan app launch and Capture ribbon passed visual inspection. The commit
containing this plan records WP7 locally. Next package: WP8; external
installation and publication still require separate authorization.
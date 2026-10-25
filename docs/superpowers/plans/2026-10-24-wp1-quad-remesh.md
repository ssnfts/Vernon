# WP1 field-aligned quad remesh implementation plan

**Goal:** Complete HANDOFF WP1 against the existing node contract and measured geometry gates.

**Architecture:** Keep the node adapter and metrics in `crates/vernon-nodes-surface/src/quad.rs`. Put native 4-RoSy orientation smoothing, position-field relaxation and topology-aware graph extraction in `remesh_fields.rs` and `field_remesh.rs`, with clipping/reflection in `remesh_symmetry.rs`. Use the existing triangle BVH for projection and conservative bidirectional deviation bounds. Preserve all existing parameter names and metric aliases.

**Constraints:** Follow AGENTS.md and CLEAN_ROOM.md; pure Rust, no new runtime dependencies, deterministic cooks, millimetres, original visible text. Only WP1. No solver installation or publication. One commit after all checks pass.

- [x] Add regression tests using actual `geo.car_panel` geometry, dense and rotated inputs, a closed mesh, hard edges, symmetry, invalid inputs and determinism. Verify failures before implementation.
- [x] Implement the paper's local orientation and position field stages, then extract and project geometry; retain explicit warnings for constraints that cannot be met. Measure quad ratio, interior irregular vertices, mean edge length and bidirectional deviation. Do not claim exact continuous Hausdorff from finite samples.
- [x] Verify default panel: quad ratio >= 0.9, mean edge within 15% of sqrt(area / target_quads), deviation < 1% diagonal. Exercise explicit edge mode and dense-input coarsening independently.
- [x] Run a release-only 100,000 triangle benchmark including cook metrics; require < 5 seconds. Keep timing out of ordinary debug tests.
- [x] Review numerical and topology risks; run workspace build/test/clippy, example cook, registry generation and GUI smoke. Update README counts and the node spec only after acceptance succeeds.
- [x] Review diff, commit the completed WP with verification evidence. Do not push.

Commands run from the repo after dot-sourcing `scripts/dev-env.ps1`. Set TEMP and TMP to `scratch/wp1-temp` in this sandbox because the inherited sandbox temp directory denies test writes. This is a process environment change, not an application change.

Focused checks: `cargo test -p vernon-nodes-surface quad::tests`; benchmark: `cargo test --release -p vernon-nodes-surface wp1_release -- --ignored --nocapture`. Full completion commands are those in AGENTS.md.

Verification completed 2026-10-26: workspace build and Clippy clean; 1,002 tests passed, six ignored; the separate release benchmark passed in 1.35 s. Demo: 36 cooked, zero failures, two expected gates. Catalogue current and GPU viewport launch visually checked. Final review's closed-seam indexing regression was reproduced, fixed and passed. See docs/specs/field-aligned-remesh.md for measurements and limits.

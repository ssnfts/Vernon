# Vernon source consolidation plan and audit — 2026-11-03

Goal: consolidate completed local Vernon branches into C:/Users/mabdu/REA/vernon while retaining unfinished WP9 work.

- [x] Inventory all 13 registered worktrees, dirty files and unmerged commit ancestry.
- [x] Back up every modified/untracked original-tree file under ../backups/vernon-consolidation-20261103-031003 and retain the pre-consolidation Git stash.
- [x] Fast-forward original checkout from dd521c2 to 2e01920, bringing in seven missing commits (later WP10 work, CalculiX integration and partial WP15).
- [x] Restore WP9 edits with stash apply; no conflicts. Retain the stash as recovery evidence.
- [x] Run full bounded build, tests, Clippy, demo and catalogue checks; inspect results.
- [x] Verify all 13 registered worktree heads are ancestors of original HEAD. All 38 backed-up files outside the two auto-merged documents match after newline normalization; no unresolved merge entries; git diff --check is clean.

## Completion assessment

WP1–8 and WP11–14 were already integrated. Later WP10 changes are now integrated: native solver cross-check promotion, binary Gmsh 2.2 parsing and real mesher promotion. Their external execution evidence is historical branch documentation, not re-executed by this consolidation.

WP9 remains incomplete and uncommitted. Preserve accepted 16 m/s replication contract and all existing gates; physical acceptance remains outstanding. See CONTINUATION.md.

WP15 is partial: committed Blender subdivision and CloudCompare distance golden fixtures/tests are now present. Slicer reference remains incomplete; see tests/golden/README.md.

All sibling worktrees remain in place, including the untracked vernon-wp14/=1000 file. REA's separate rea repository, unrelated root research files, backups, external tools and build caches are not Vernon source and are retained in their existing locations. No push or installation.

## Verification limitation

Fresh full queue attempt consolidation-20261103-a stopped before build/tests because another process holds scratch/continuation/worker.lock. No lock was removed and no competing Cargo job was started. Status inspection reports lockState=held, while the latest recorded worker is finished/failed. The identity of the lock holder could not be established from available process visibility.

Latest historical run wp9-surface-binding-red-20261102-b: 260 passed, 17 failed, 13 ignored, exit 101. The failures are unfinished tri_surface fingerprint regressions in WP9. This is pre-consolidation evidence, not a test of the newly consolidated source. Full verification and app startup remain unperformed for this consolidation. Do not claim a passing build or a completed WP9/WP15.

Once the owner releases the verification lock, run from the original checkout with the existing PowerShell 7 executable:

    pwsh -NoProfile -File scripts/verification.ps1 -Profile full -RunId consolidation-20261103-b -JobTimeoutSeconds 7200

Review generated catalogue and demo gates; reconcile counts only against fresh catalogue output. Retain the WP9 intentional red tests for their implementation owner rather than changing their contract during consolidation.

## Verification superseding the earlier limitation

The reserved owner completed `wp9-integrated-full-20261103-c`:1540passed,0failed,
35ignored; build/all-target Clippy/demo/catalogue all exit0. App startup smoke
passed12s with Vulkan4x; no manual visual testing. Source fingerprint
a765d522c9b4f8992cae49a306bbd2fb18b1327cf9ecdec0c8166ecc242b88cb.
WP9 is still incomplete; no external acceptance is implied by these checks.

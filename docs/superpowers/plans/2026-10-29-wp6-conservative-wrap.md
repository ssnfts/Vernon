# WP6 conservative wrap and durable verification plan

**Goal:** Replace `clean.alpha_wrap` with the handoff's permitted native voxel
and dual-contouring alternative, with measured topology and offset bounds;
make verification survive chat quota interruptions.

**Architecture:** A focused `conservative.rs` kernel builds one unsigned
triangle/point distance oracle and a sparse narrow-band uniform grid. A
guarded outside flood, Hermite dual contouring, cell-constrained vertex
placement and direct acceptance checks produce the output. The node adapter
keeps existing ports and automatic alpha/offset behavior. A separate fixed-job
PowerShell worker records durable logs and source fingerprints.

**Constraints:** AGENTS.md and CLEAN_ROOM.md; pure Rust permissive dependencies;
mm; no proprietary installation or GPL algorithm source; no installation or
remote publication. One local WP6 commit only after full verification. The
published Delaunay algorithm is not claimed by the permitted alternative.

## 1. Durable verification

- [x] Complete `scripts/verification.ps1`, `start-verification.ps1`,
  `verification-status.ps1` and `test-verification.ps1`.
- [x] Fixed profiles: full checks, clean-focused tests, WP6 release benchmark
  and smoke/failure/slow self-tests. Bound child runtimes and kill timed-out
  process trees. Stop at errors or changed source; no arbitrary commands or
  AI loops, commits, installs or publication.
- [x] Prove exclusive locking, matching-source safe resume, stale/interrupted
  status, source-change invalidation and detached execution across shells.
- [x] Create the authorized six-hour heartbeat and a durable continuation file.

## 2. Kernel — `crates/vernon-nodes-clean/src/conservative.rs`

- [x] Add failing regression tests for sphere, open/soup/point input, narrow
  and wide apertures, disconnected components, winding/translation invariance,
  ambiguous cells, deterministic output and invalid/resource requests.
- [x] Lock crate-private interfaces `ConservativeParams`, `ConservativeOutput`,
  `ConservativeMetrics` and `conservative_wrap(&Geometry,&ConservativeParams)`.
  The params contain alpha, offset and max_grid_nodes; output contains pos,
  tris and measured metrics. Preserve unreferenced input points in the oracle.
- [x] Resolve automatic alpha to diagonal/20 and automatic offset to alpha/30.
  At h=alpha/8 use barrier radius R=offset+alpha/2+sqrt(3)*h. Checked grid
  products and a 40,000,000-node default cap reject excessive requests.
- [x] Flood the free grid from its padded boundary. Extract face-consistent
  local contour cycles from Hermite crossings; use a cell-clamped regularized
  QEF or bounded crossing average. Emit consistently wound faces around
  primal edges. Split local patches when needed and validate vertex links.
- [x] Enforce finite, nondegenerate, closed directed-edge manifold output and
  one connected link cycle at every vertex. Check every output vertex against
  the complete source: offset <= distance <= offset+alpha. Reject uncertified
  or unresolved cases; bounded refinement retries may not exceed resources.
- [x] A shrink step may stay within the owning cell and distance band and
  preserve incident triangles; otherwise retain the conservative baseline.
  No unbounded smoothing or remeshing is permitted after certification.

## 3. Node adapter and acceptance

- [x] Add the private module declaration and replace only `alpha_cook` in
  `wrap.rs`; retain the existing watertight-wrap pipeline unchanged.
- [x] Preserve geometry/report ports and alpha/offset parameters. Reuse the
  kernel oracle measurements rather than building another report BVH.
  Expose distance extrema/violation counts, checked vertices, topology counts,
  alpha/offset/barrier/grid metrics and deterministic reports.
- [x] Replace placeholder status only when actual implementation and checks
  pass. Clearly describe the uniform-grid alternative and its limits.
- [x] Add ignored `conservative::tests::wp6_release_million_triangles`.
  Use the actual 1,000,000-triangle procedural sphere. Time the entire kernel
  after fixture creation, including acceleration and certification; assert
  under 30 seconds in release and print the measured acceptance numbers.

## 4. Review and delivery

- [x] Independent review of the runner and numerical/topological kernel;
  repair findings with observed regressions and covering checks.
- [x] Run the saved release benchmark and full build/test/Clippy/demo/catalogue
  jobs; inspect logs, update measured contracts, README and HANDOFF, and
  visually check GPU launch. No success claim from a zero-test filter.
- [x] Commit WP6 locally, preserve exact evidence and update CONTINUATION with
  the next unfinished package. If interrupted earlier, record the exact next
  action and leave WP6 explicitly incomplete.

Primary method references: Portaneri et al., *Alpha Wrapping with an Offset*
(2022), DOI 10.1145/3528223.3530152; Ju et al., *Dual Contouring of Hermite
Data* (2002), DOI 10.1145/566654.566586. The local design audit is transient
engineering evidence in `scratch/wp6-design.md`; no GPL implementation is used.

## Final evidence — 2026-10-29

All checks completed in the detached `wp6-final-20261029-a` queue, with
source fingerprint unchanged. Full workspace: 1,117 passed, zero failed,
eight ignored; build and all-target Clippy clean. Demo: 36 nodes, zero
failures, two expected gates. Catalogue: 197 nodes, 184 implemented,
13 white-box. GPU app launch and Clean/Align ribbon inspected.

Full-kernel release benchmark: 1.930284 s on 1,000,000 genuine triangles;
51,326 output vertices, 102,648 triangles. All vertices checked, zero band
violations, zero topology defects and real constrained-projection moves.
Independent kernel and runner reviews found no remaining blocker after
regressions repaired the tiny/collinear source-oracle and process-containment
findings. Runner self-tests passed seven scenarios and detached execution
was verified across independent command shells.

The commit containing this plan records WP6 locally. Next package: WP7.
The six-hour follow-up remains active; no external publication occurred.
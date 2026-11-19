# Changelog

Format follows [Keep a Changelog](https://keepachangelog.com/). Versions follow
[Semantic Versioning](https://semver.org/); `0.x` makes no stability promise.

## [0.1.0-alpha] - unreleased

Proposed first release. Pre-alpha: the node contract, file format and parameter
names may still change. The workspace version in `Cargo.toml` is `0.1.0`; the
proposed tag is `v0.1.0-alpha`; no publication is implied.

### What works

- **Node graph.** Deterministic cooking with caching, cancellation, a trust
  model for nodes that write files or start programs, and report gates
  (limits on report metrics). `vernon-cli` provides `list`, `info`, `cook`,
  `mcp` (stdio MCP server) and `agent-guide`. `cook` exits 0 when every node
  cooked and every gate passed or is listed as expected, 1 when a node failed,
  3 when an unexpected gate failed.
- **201 node types**, 199 marked implemented in the inspected catalogue
  (`docs/NODES.md`): geometry, capture, clean/align, mesh to surface,
  split/print, airflow (CFD), structure (FEA), solar/heat, flatten,
  performance.
- **File formats.** OBJ, STL, PLY, 3MF, XYZ/PTS, E57 and LAS readers;
  OBJ, STL, PLY, 3MF and STEP writers.
- **Example pipeline** `examples/bodykit_pipeline.vrn`: 40 nodes covering all
  stages on a synthetic fender, cooks with 5 declared expected gate failures
  (no wheels on the single panel, placeholder solid mesher, CFD case written
  but not solved).
- **Desktop app** (`vernon`, egui/eframe, wgpu viewports with a CPU fallback):
  quad viewport, command panel, modifier stack, gate dashboard with "Next:"
  hints under node errors, start panel (demo, import, open), import size and
  unit check, in-app agent chat panel.
- **External programs run as separate processes**, only when a node's `run`
  parameter is true and the executable exists: OpenFOAM (via WSL), CalculiX,
  COLMAP, slicers. Tests never require them.
- **Windows package** (zip, per-user installer and uninstaller, portable
  mode, SHA-256 file, MANIFEST.json) built by `scripts/release/package.ps1`
  and exercised by `scripts/release/package-selftest.ps1`. The saved local
  November 14 log records 56 passed, 0 failed on the development machine;
  clean-machine acceptance and checks of the current candidate remain unverified.
- **Dependency policy checks configured.** `deny.toml` (licence allow-list, bans on
  `cc`, `cmake`, `bindgen`) and `scripts/release/check-deps.ps1`;
  `THIRD_PARTY_LICENSES.md` is generated from the build. Current-candidate audit results remain unverified.
- **CI configured** (`.github/workflows/ci.yml`): build, test, clippy `-D warnings`,
  example cook, clean-room test and dependency checks on Windows GNU, Linux
  and macOS. Execution on GitHub for the current candidate is unverified; workflow presence is not a passing run.

### Placeholders

Two nodes are marked white-box in the inspected catalogue: they cook but run a stand-in and emit a
`white-box placeholder:` warning.

| Node | State |
|---|---|
| `cfd.mesh_build` | Built-in solid mesher is a placeholder (use the OpenFOAM path) |
| `cfd.solve_steady` | Case is written, not solved, without OpenFOAM |

`print.script_bridge` is marked implemented in both source and catalogue:
  it writes a slicer post-processing script and its invocation setting.
  This source inspection does not establish a newly passed slicer integration test.

The AI nodes (parts from an image, aero preview and variant comparison) call
external models and are unvalidated.

### Validated, and what is not

Full detail and tolerances are in `docs/VALIDATION.md`. In short:

- Analytic checks (beam, patch test, sphere, plane/cylinder, ICP, unit
  conversion, file round trips) and comparisons against CloudCompare and
  CalculiX on small cases pass.
- **Limited CFD result accepted November 19, 2026:** the owner chose
  "Accept, record as is" for the 25-degree slant-body long-tier result:
  Cd **0.255 ± 0.023 (95%)** against reference **0.279**, **−8.6%**.
  The mean is outside the ±5% band; the interval overlaps that band.
  The level-5, 913k-cell SA-DDES case used CTU/182 and a 40-CTU window
  mixing both slant states. Its recorded stationarity, per-step and
  half-step gates passed. Fast/long-tier validation work on this body is
  closed with the stated error, not accepted as a pass inside the ±5% band.
  The road-car v1 contract remains unchanged and unmet. This replaces reliance
  on the older 7–10% accuracy approval; it establishes no road-car prediction
  or design-ranking claim. See `docs/CONTINUATION.md` and the November 19
  acceptance in `docs/research/wp9-fast-tier-plan.md`, section 7.
- Not validated against measurement: FEA of printed parts (anisotropy,
  creep, thermal expansion, preload), photogrammetry scale, slicer G-code
  against real slicer output, solar, UV, tuft analysis, lap simulation,
  quad remesh and STEP output, and every comparison with a commercial tool.
- **Numerics, source-inspected:** the built-in FEA solver applies thermal
  strain when point temperatures and material expansion coefficients exist.
  Four-node reference-solver results remain estimate-only; reference-solver
  models with temperatures but without thermal strain also remain estimate-only.
  Ten-node results with thermal strain are not subject to those two flags;
  this is not physical validation or a new comparison-test pass.
- The constrained flattener implements stretch/compress edge factors; they
  are not applied with `stretch_only` or `compress_only`, and nonuniform
  factors in those modes produce a warning. Current-candidate tests remain
  unverified. See [release readiness](docs/release/READINESS.md) for source evidence.
- None of the 22 reserved keyboard chords is bound (existing limitation;
  not re-audited in this documentation-only pass).

### Known limits

- Windows 10/11 with the Rust GNU toolchain is the supported development and
  package platform. Linux and macOS have configured CI jobs; executed build/test evidence for the current candidate is pending.
- The package is unsigned.
- No log file; messages go to the console.

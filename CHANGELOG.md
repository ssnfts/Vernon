# Changelog

Format follows [Keep a Changelog](https://keepachangelog.com/). Versions follow
[Semantic Versioning](https://semver.org/); `0.x` makes no stability promise.

## [0.1.0-alpha] - unreleased

First tagged state. Pre-alpha: the node contract, file format and parameter
names may still change. The workspace version in `Cargo.toml` is `0.1.0`; the
tag is `v0.1.0-alpha`.

### What works

- **Node graph.** Deterministic cooking with caching, cancellation, a trust
  model for nodes that write files or start programs, and report gates
  (limits on report metrics). `vernon-cli` provides `list`, `info`, `cook`,
  `mcp` (stdio MCP server) and `agent-guide`. `cook` exits 0 when every node
  cooked and every gate passed or is listed as expected, 1 when a node failed,
  3 when an unexpected gate failed.
- **198 node types across nine stages**, 195 implemented
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
  and tested by `scripts/release/package-selftest.ps1`.
- **Dependency policy checked.** `deny.toml` (licence allow-list, bans on
  `cc`, `cmake`, `bindgen`) and `scripts/release/check-deps.ps1`;
  `THIRD_PARTY_LICENSES.md` is generated from the build.
- **CI** (`.github/workflows/ci.yml`): build, test, clippy `-D warnings`,
  example cook, clean-room test and dependency checks on Windows GNU, Linux
  and macOS. Not yet run on GitHub.

### Placeholders

Three nodes are white-box: they cook but run a stand-in and emit a
`white-box placeholder:` warning.

| Node | State |
|---|---|
| `print.script_bridge` | Needs an external slicer; slicer profile keys not wired |
| `cfd.mesh_build` | Built-in solid mesher is a placeholder (use the OpenFOAM path) |
| `cfd.solve_steady` | Case is written, not solved, without OpenFOAM |

The AI nodes (parts from an image, aero preview and variant comparison) call
external models and are unvalidated.

### Validated, and what is not

Full detail and tolerances are in `docs/VALIDATION.md`. In short:

- Analytic checks (beam, patch test, sphere, plane/cylinder, ICP, unit
  conversion, file round trips) and comparisons against CloudCompare and
  CalculiX on small cases pass.
- **CFD claim, signed off by the owner, is one case only:** on the classic
  slant-body case with a 25 degree slant, the fast CFD tier puts drag within
  about 7-10% of the published 1984 wind-tunnel value (half model, about 0.5M
  cells, each run after the first under 35 minutes). Nothing else about CFD is
  claimed: no road-car drag or downforce prediction, no ranking of two
  designs, no statement about the 35 degree body, no "validated CFD". The
  steady-state runs did not converge.
- Not validated against measurement: FEA of printed parts (anisotropy,
  creep, thermal expansion, preload), photogrammetry scale, slicer G-code
  against real slicer output, solar, UV, tuft analysis, lap simulation,
  quad remesh and STEP output, and every comparison with a commercial tool.
- Open items: thermal-strain comparison in FEA (so `estimate_only` stays),
  stretch/compress factors in the constrained flattener, none of the 22
  reserved keyboard chords is bound.

### Known limits

- Windows 10/11 with the Rust GNU toolchain is the supported development and
  package platform. Linux and macOS are built and tested in CI only.
- The package is unsigned.
- No log file; messages go to the console.

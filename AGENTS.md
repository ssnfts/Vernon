# AGENTS.md — rules for coding agents working on Vernon

Read this file and `docs/HANDOFF.md` before changing anything. HANDOFF.md
says what to build next and in what order; this file covers how to work.

## What Vernon is
An Apache-2.0, clean-room, open-source Rust node-graph 3D app with a
DCC-style (quad viewport / command panel / modifier stack) UI. It
re-implements, by **observable behaviour only**, the tools in a 3D-printed
car body-kit pipeline: scan → clean → surface → split/print → CFD → FEA →
solar/flatten → performance. Every stage emits a `Report` with numeric
metrics that later stages check (gate on).

## Hard rules (never break these)
1. **Clean room.** Read `CLEAN_ROOM.md`. Never open, decompile, disassemble or
   copy anything from a proprietary install folder or binary
   (`C:\Users\mabdu\Documents\houdini21.0` is off limits). Only use public
   docs, papers and open-source code under its licence. Never copy vendor
   documentation text, UI strings or icons.
2. **No trademarks in user-visible strings.** Node labels, help text,
   parameter labels and menus must not name Rhino, Fusion, Autodesk, 3ds Max,
   QUICKSURFACE, Geomagic, Design X, ZEISS, AirShaper, SimScale, Ansys,
   OptimumLap, Plasticity, Polycam, Ladybug, Helio, CloudCompare, Houdini,
   SideFX, Creality, Revopoint or EinScan. They may appear only in
   `NodeTypeInfo::emulates`, `docs/specs/`, `docs/research/`, `CLEAN_ROOM.md`
   and code comments. `crates/vernon-cli/tests/clean_room.rs` enforces this.
3. **Dependencies:** pure Rust only (no `cc`, `cmake` or `bindgen` build
   scripts on Windows), permissive licences only (MIT, Apache-2.0, BSD, Zlib,
   ISC, Unicode, BSL-1.0, CC0). No GPL/LGPL/AGPL crates. GPL/AGPL programs
   (OpenFOAM, CalculiX, OrcaSlicer…) are only ever run as **separate
   processes**. Any new external program goes into the table in
   `CLEAN_ROOM.md` in the same commit. Do not read CGAL, TetGen or other GPL
   source to implement an algorithm; work from the paper.
4. **Units and conventions:** lengths in millimetres, Z up, SI for physics
   with explicit unit strings. Standard attribute names: point `N`, `Cd`,
   `uv`, `dist`; prim `N`, `part`, `temp_c`, `Cp`. Metric names shared
   between stages are in `vernon_core::report::keys`; consumers look them up
   with `Report::first_metric(&keys::…_ALIASES)`.
5. **Node contract** (`crates/vernon-graph/src/node.rs`): `NodeOp::info()`
   plus a pure, deterministic `cook()` that returns exactly one `Data` per
   declared output. Write files only under `ctx.work_dir`. Ids are
   `<stage>.<snake_case>` with prefixes `geo, capture, clean, surface, print,
   cfd, fea, solar, flatten, perf`. A white-box node omits `.implemented()`
   and must emit a `Severity::Warning` note starting with
   `white-box placeholder:`.
6. **Nodes that drive external programs** write complete input files without
   the program, take a `run` bool param (default false) plus an executable
   path param, and only spawn the program when `run` is true and the
   executable exists. Tests must never require the program; tests that do
   are `#[ignore = "needs <tool>"]`.

## Build and check (Windows, this machine)
Rust is the **GNU** toolchain plus WinLibs MinGW. In every new PowerShell:
```powershell
Set-Location C:\Users\mabdu\REA\vernon
. .\scripts\dev-env.ps1          # puts cargo and MinGW (gcc, dlltool) on PATH
```
`~/.cargo/config.toml` already points the GNU target's `linker` and
`-Cdlltool` at WinLibs. Don't change it. On Linux or macOS, plain `cargo`
works (the app needs Vulkan, Metal or GL to open a window).

Definition of done for **every** change:
```powershell
cargo build --workspace
cargo test --workspace                                   # currently 982 passing, 0 failing
cargo clippy --workspace --all-targets -- -D warnings    # must be clean
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn   # 38 nodes, 0 failing gates, 2 expected
cargo run -p vernon-cli -- list --markdown > docs/NODES.md      # regenerate after adding or changing nodes
```
Then update the per-stage counts in `README.md` and launch the app once
(`cargo run -p vernon`) to check it opens.

## Repo map
| Path | What |
|---|---|
| `crates/vernon-core` | Geometry kernel, attributes, `Report`/`Metric`, shared materials table, process helpers |
| `crates/vernon-graph` | `NodeOp`, params, `Graph` (cooking, caching, cancel, trust), registry, gate rules |
| `crates/vernon-io` | OBJ, STL, PLY, 3MF, XYZ/PTS read and write |
| `crates/vernon-render` | Offscreen wgpu viewport renderer |
| `crates/vernon-nodes-*` | One crate per stage: geo, capture, clean, surface, print, cfd, fea, solar (+ flatten), perf |
| `crates/vernon-cli` | `list`, `info`, `cook`; integration and clean-room tests |
| `app/` | egui/eframe desktop app (binary `vernon`) |
| `docs/specs/` | Clean-room specs: the source of truth for node behaviour |
| `docs/research/k2/` | Vetted third-party review notes (suggestions only) |
| `examples/bodykit_pipeline.vrn` | End-to-end demo scene |

## Commits
One commit per work package in `docs/HANDOFF.md`, with a message saying what
changed and how it was verified. Don't commit with failing tests. Don't
push or publish anywhere unless the user asks.

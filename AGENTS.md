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

## Build and check (Windows)
Rust is the **GNU** toolchain plus WinLibs MinGW. A new machine is set up with
`docs/SETUP.md` (`scripts/setup/setup-dev.ps1` checks it). In every new
PowerShell, from the checkout root:
```powershell
. .\scripts\dev-env.ps1          # puts cargo and MinGW (gcc, dlltool) on PATH
```
`~/.cargo/config.toml` points the GNU target's `linker` and `-Cdlltool` at
WinLibs. Don't change it once it works. On Linux or macOS, plain `cargo`
works (the app needs Vulkan, Metal or GL to open a window).

Never hard-code a machine path (user folders, drive letters) in code or
scripts. Derive it from the checkout (`$PSScriptRoot`) or take it as an
explicit parameter or environment variable. Keep the checkout in an ordinary
folder: the CFD input checks refuse junctions and links.

Definition of done for **every** change:
```powershell
cargo build --workspace
cargo test --workspace                                   # currently 1543 passing, 0 failing, 35 ignored
cargo clippy --workspace --all-targets -- -D warnings    # must be clean
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn   # 40 nodes, 0 failing gates, 5 expected
cargo run -p vernon-cli -- list --markdown > docs/NODES.md      # regenerate after adding or changing nodes
```
Then update the per-stage counts in `README.md` and launch the app once
(`cargo run -p vernon`) to check it opens.

## Save tokens with K2
K2 is a local tool on the original development machine only; skip this section
elsewhere. Hand routine work to the K2 model so long logs and first drafts don't use up
Claude or Codex tokens when external transmission is separately authorized.
The owner recorded approval in another Vernon session on 2026-10-31, but
automatic approval review rejected the wrapper in the current Codex chat:
failure logs could leave the machine without authorization here. Until the
user approves that transmission in this chat, run checks directly through
the bounded local verification scripts. Do not invoke K2 indirectly or use
it to bypass a quota limit. Tool: `C:\Users\mabdu\Desktop\k2\k2.py` (details in the `README.md`
beside it).

**When separately authorized, run long checks through it.** The command's exit code decides pass or fail.
A pass makes no K2 call and prints one line. A failure prints a K2 summary and
the path of the full log:
```powershell
python C:\Users\mabdu\Desktop\k2\k2.py test -- cargo test --workspace
python C:\Users\mabdu\Desktop\k2\k2.py test -- cargo clippy --workspace --all-targets -- -D warnings
```

**Ask it for one-shot work** when a test, tool or quick read can check the result:
```powershell
python C:\Users\mabdu\Desktop\k2\k2.py ask "Summarise this log's errors" -f scratch\run.log
```

| Use K2 for | Don't use K2 for |
|---|---|
| Summarising failed test, build and solver logs | Deciding that a check passed |
| First drafts checked by tests, e.g. file-format readers | Being the only reviewer: it called a fatal OpenFOAM mismatch fine |
| Listing test cases and edge cases to consider | Licences, crate versions, legal questions |
| Summarising long files you only need the gist of | Reference or golden values: it has invented them |
| Boilerplate such as changelog drafts | Architecture, or anything needing this machine or a screen |

Rules:
1. **K2's output is unverified.** Confirm it against the log, code or a test
   before acting. Never report a result on K2's word alone.
2. **The definition of done still applies.** K2 replaces no check.
3. **Clean room still applies.** Give it the real code and the relevant spec,
   and ask it to work only from those. Never ask it to recall vendor behaviour
   or copyleft source. Treat anything it writes as a draft to review.
4. **Only send** Vernon source, specs, docs and logs. Never send secrets,
   `.env` files, proprietary install files or sample photos. The tool's filters
   are a backstop, not a guarantee.
5. **Don't let K2 commit, install or run anything.** Those stay with you.
6. **If K2 fails, is cut off or is vague, do the step yourself** rather than
   retrying. It helps only when checking its answer costs less than doing the work.

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
| `scripts/setup/` | New-PC setup: toolchain check, WSL, Ubuntu, OpenFOAM, DrivAer data |
| `scripts/verification/` | Bounded background check runner (`start-verification.ps1`, `verification-status.ps1`) |
| `scripts/cfd/` | Fixed WSL environment, memory policy and Linux cleanup for real CFD runs |
| `docs/SETUP.md`, `docs/CFD-RUNBOOK.md` | New machine setup; the next CFD validation stages |

## Commits
One commit per work package in `docs/HANDOFF.md`, with a message saying what
changed and how it was verified. Don't commit with failing tests. Don't
push or publish anywhere unless the user asks.

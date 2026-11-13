# Vernon

**Scan a car, design a body kit, print it in sections, and check the airflow,
strength and heat before you bolt it on. One free desktop app, one node
graph, a pass/fail number at every stage.**

Vernon is an open-source 3D toolkit for the **scan → surface → print →
simulate** workflow. It has a familiar DCC-style interface (quad viewports,
command panel, modifier stack) over a procedural node graph. It runs
offline, is Apache-2.0 licensed for commercial use, and can be driven by a
local AI agent.

> **Status: pre-alpha.** 198 of 201 nodes run their real algorithm and the
> full pipeline cooks on a synthetic part. No binary release exists yet, no
> real scanned part has been taken through to print, and the CFD stage has no
> validated result. See [Validation status](#validation-status) and
> [docs/PLAN.md](docs/PLAN.md).

## Contents

- [Why Vernon](#why-vernon)
- [Quick start](#quick-start)
- [The pipeline](#the-pipeline)
- [Status by stage](#status-by-stage)
- [Local AI](#local-ai)
- [Drive Vernon from an AI agent](#drive-vernon-from-an-ai-agent)
- [Validation status](#validation-status)
- [Repository layout](#repository-layout)
- [Development](#development)
- [Getting the best result](#getting-the-best-result)
- [Works well with](#works-well-with)
- [Documentation](#documentation)
- [Clean room and licence](#clean-room-and-licence)

## Why Vernon

Making a printed body kit today means five or six separate programs: one to
clean the scan, one to surface it, a slicer, a CFD front end, an FEA tool
and a spreadsheet for performance. The numbers that matter get lost in the
hand-offs between them.

- **Every stage reports a number, and the next stage checks it.** Scan
  deviation, bed-fit margin, CdA, safety factor at each mount, surface
  temperature against the material's heat-deflection temperature, lap time.
  A stage whose gate fails is not trusted downstream.
- **Free for commercial work, and local.** No account, no cloud, no
  public-project rule. External open-source solvers run as separate
  processes under their own licences.
- **Honest about what is real.** A node that still runs a placeholder says so
  in a warning. An AI prediction is labelled as a prediction and can never
  satisfy an engineering gate.
- **Built to be driven by an agent.** A typed tool layer and an MCP server
  let a model add nodes, wire them, cook and read the gates, with no shell
  and no file access outside its work folder.

Who it is for, what else exists and where the project is going are in
[docs/PLAN.md](docs/PLAN.md).

## Quick start

Vernon builds from source. Windows 10/11 with the Rust GNU toolchain is the
supported setup; [docs/SETUP.md](docs/SETUP.md) takes a fresh PC from nothing
to a passing build. On Linux or macOS plain `cargo` works.

```powershell
.\scripts\setup\setup-dev.ps1 -WriteCargoConfig   # checks the toolchain; installs nothing
. .\scripts\dev-env.ps1                            # every new shell
cargo run -p vernon                                # the desktop app
```

Run the whole pipeline on the example part from the command line:

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --out deviation.obj
cargo run -p vernon-cli -- list                    # node types by stage
cargo run -p vernon-cli -- info clean.icp_align    # one node's ports and parameters
```

In the app the same graph is under **Pipeline > Replace Scene with Demo**.

None of this needs OpenFOAM, CalculiX, COLMAP or a slicer. Nodes that drive
those programs write complete input files without them and only launch the
program when you switch **Run** on.

## The pipeline

A body kit starts as a scan of the real car and ends as printed parts whose
airflow, strength, heat and lap-time effect have been checked. Each step is
a group of nodes that passes geometry and a numeric `Report` to the next.

1. **Geometry basics.** Primitives, a test car panel, transforms, merge,
   mirror, normals, noise, and OBJ, STL, PLY, 3MF and XYZ files.
2. **Capture.** Import scans (PLY, OBJ, STL, XYZ, E57, LAS) or run external
   photogrammetry (COLMAP). Scale bars and coded markers fix real-world
   scale; feature registration and a robust pose graph align several scans.
   *Reports:* registration residuals, marker scale error, coverage.
3. **Clean and align.** Decimate, remesh, remove outliers, smooth. Align to
   the car's frame with ICP. Build a certified conservative wrap. Measure
   signed deviation. *Gate:* deviation percentile within tolerance.
4. **Mesh to surface.** Field-aligned quad remeshing, connected B-spline
   patch layouts, analytic primitive recognition, continuity checks,
   sections, loft, thicken, STEP export. *Gate:* surface-to-scan deviation.
5. **Split and print.** Cut along body lines and search for a partition
   that fits the printer bed. Add dovetail, dowel and plug keys and
   strain-screened snap hooks. Write 3MF packages, slicer presets and G-code
   checks. *Gates:* bed-fit margin, snap-hook strain.
6. **Airflow (CFD).** Prepare the vehicle, measure frontal area, build a
   virtual wind tunnel with moving ground and rotating wheels, plan mesh
   refinement and write a complete OpenFOAM case. Read forces, check
   convergence, plot Cp maps. *Reports:* Cd, Cl, CdA, mesh quality.
7. **Structure (FEA).** Printed orthotropic materials with temperature
   derating, mounts, aero and bump loads, CalculiX decks or the built-in
   reference solver. *Gate:* safety factor at every mount.
8. **Solar, heat and flattening.** Sun position, weather files, sky matrix,
   incident radiation and a surface energy balance. *Gate:* surface
   temperature against heat-deflection temperature. The flatten tools cut
   seams and unroll panels for wraps and composite layups, with a strain
   report.
9. **Performance.** CdA and ClA become aero loads, top speed and point-mass
   lap times. Then the claim is checked against reality: coastdown fit,
   pressure-tap comparison, tuft classification. *Gate:* the simulated
   change must stand out from the measurement noise.

[examples/bodykit_pipeline.vrn](examples/bodykit_pipeline.vrn) runs all nine
steps on a synthetic fender. `cook` ends with a gate summary: every metric
outside its limit, with value, limit and margin. It exits with 1 when a node
fails and 3 when a gate fails that the scene does not list as expected. The
example lists five expected gate failures (the single panel has no wheels,
the built-in solid mesher is a placeholder, and the airflow case is written
but not solved); `--ignore-gates` keeps the exit code at 0.

## Status by stage

| Stage | Implemented | White-box |
|---|---:|---:|
| Geometry (including AI parts from an image) | 22 | 0 |
| 1 Capture | 22 | 0 |
| 2 Clean / Align | 26 | 0 |
| 3 Mesh → Surface | 23 | 0 |
| 4 Split / Print | 25 | 1 |
| 5 Airflow (CFD, including the AI aero preview) | 21 | 2 |
| 6 Structure (FEA) | 20 | 0 |
| 7 Solar / Heat | 14 | 0 |
| 7b Flatten | 6 | 0 |
| 8 Performance | 19 | 0 |
| **All** | **198** | **3** |

A *white-box* node has its final ports and parameters but a placeholder
algorithm; it cooks and says so in a warning. The full catalogue, with what
each node re-implements, is generated into [docs/NODES.md](docs/NODES.md).
What is still partial is listed in [docs/ROADMAP.md](docs/ROADMAP.md).

## Local AI

Everything here runs on your own machine. Models run as separate processes,
their weights are never bundled, and nothing downloads without your say.

| Feature | What it does | State |
|---|---|---|
| **AI Parts from Image** (`geo.ai_partcrafter`) | One image becomes several separate mesh parts (PartCrafter, MIT) | Works; output is unscaled concept geometry |
| **AI Aero Preview** (`cfd.ai_aero_preview`) | Surface pressure, wall shear and a flow slice in seconds (DoMINO) | Works; labelled "AI prediction — CFD validation pending" |
| **AI Aero Compare** (`cfd.ai_aero_compare`) | Compares two previews to screen variants | Works; same label |
| **In-app agent chat** | Builds and cooks graphs through typed tools; speaks the OpenAI-compatible API, so a local llama.cpp or Ollama server works | Works; no bundled model yet |
| **MCP server** (`vernon-cli mcp`) | The same tools for any MCP client | Works |

The aero preview is a screening aid in dimensionless model units. It shows
no drag or lift figure, and no acceptance gate reads it. Set
`VERNON_AI3D_ROOT` to the model install folder and switch **Run model** on a
node to use it. Supported inputs, licences, measured memory and time, and
the limits are in [docs/specs/ai-models.md](docs/specs/ai-models.md). What
comes next (a resident model service, an offline chat default, surrogate
warm start for the solver, variant screening) is in
[docs/PLAN.md](docs/PLAN.md#5-local-ai-what-to-add).

## Drive Vernon from an AI agent

`vernon-cli mcp` is a stdio [MCP](https://modelcontextprotocol.io) server.
It gives a client tools to list and describe nodes, add and wire them, set
parameters (checked against each node's ranges, units and options), cook,
read reports and gates, measure geometry, undo and redo, and save and load
graphs. It has no shell and no file access outside its work folder. Nodes
that drive external programs stay off unless a call turns them on.

```bash
cargo build --release -p vernon-cli
```

Point the client at the binary with an absolute path (add `.exe` on Windows;
in JSON use forward slashes or doubled backslashes):

```json
{ "mcpServers": { "vernon": { "command": "/path/to/vernon-cli", "args": ["mcp", "--work", "/path/to/vernon-work"] } } }
```

That block goes in `.mcp.json` for Claude Code (or run
`claude mcp add vernon -- /path/to/vernon-cli mcp --work /path/to/vernon-work`),
in `~/.cursor/mcp.json` for Cursor, and in `cline_mcp_settings.json` for
Cline. Give the model [docs/AGENT_GUIDE.md](docs/AGENT_GUIDE.md) first; it
is generated by `vernon-cli agent-guide`.

## Validation status

What is and is not proven is tracked in
[docs/VALIDATION.md](docs/VALIDATION.md). Every benchmark check ends as
PASS, FAIL or SKIPPED with a reason.

**CFD (work package 9) is not validated yet.** The target is the published
TUM DrivAer replication at 16 m/s within a 5% band on Cd, on a half model of
at least 12M cells, with a three-grid study and strict convergence.

- The fine 12.06M-cell Fastback mesh is built and passes its quality gates.
- Steady k-omega SST runs on it do not meet the convergence gate. A per-cell
  residual map shows the plateau is spread through the near wake, so no
  mesh, scheme or relaxation change closes the gap.
- The planned route is a transient DDES protocol, rehearsed first on a 25°
  slant body with a known answer:
  [docs/research/wp9-ddes-route-plan.md](docs/research/wp9-ddes-route-plan.md).
- No failed run has been relabelled and no gate has been loosened. Until
  this finishes, treat every Vernon Cd as unvalidated.

The run commands are in [docs/CFD-RUNBOOK.md](docs/CFD-RUNBOOK.md) and the
full record of every run, with its numbers, is in
[docs/CONTINUATION.md](docs/CONTINUATION.md).

## Repository layout

| Path | What |
|---|---|
| `app/` | egui/eframe desktop app (binary `vernon`), including the agent chat panel |
| `crates/vernon-core` | Geometry kernel, attributes, `Report`/`Metric`, materials, process helpers |
| `crates/vernon-graph` | Node contract, parameters, graph cooking and caching, registry, gate rules |
| `crates/vernon-io` | OBJ, STL, PLY, 3MF, XYZ, E57 and LAS readers and writers |
| `crates/vernon-render` | Offscreen wgpu viewport renderer |
| `crates/vernon-nodes-*` | One crate per stage: geo, capture, clean, surface, print, cfd, fea, solar (with flatten), perf, ai |
| `crates/vernon-cli` | `list`, `info`, `cook`, `mcp`, `agent-guide`; integration and clean-room tests |
| `crates/vernon-agent-tools` | Typed tool layer over the graph, used by the agent and the MCP server |
| `crates/vernon-agent` | In-app agent: OpenAI-compatible client and the tool loop |
| `crates/vernon-bench` | Validation and benchmark suite |
| `examples/` | End-to-end demo scene and its cooked outputs |
| `tests/golden/` | Reference outputs recorded with external open-source tools |
| `bench/` | Benchmark runner, baseline timings and the latest report |
| `packaging/` | Installer, uninstaller and notes shipped in the Windows package |
| `scripts/` | `dev-env.ps1`, plus `setup/`, `verification/`, `cfd/`, `ai/`, `release/` and `golden/` |
| `docs/` | Plan, specs, research and hand-off logs, indexed in [docs/README.md](docs/README.md) |

## Development

Definition of done for every change:

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn
cargo run -p vernon-cli -- list --markdown > docs/NODES.md
```

The same queue, and long real-tool runs, can run as a background worker that
survives interruptions and saves logs under `scratch/continuation/runs/`:

```powershell
.\scripts\verification\start-verification.ps1 -Profile full -RunId full-YYYYMMDD-a -JobTimeoutSeconds 2400
.\scripts\verification\verification-status.ps1 -Json
```

Accuracy and speed benchmarks, and the Windows package:

```powershell
.\bench\run.ps1 -Quick                      # writes bench/results/REPORT.md and results.json
.\scripts\release\package.ps1               # folder, zip and installer
.\scripts\release\package-selftest.ps1
```

The viewports render with wgpu (depth-tested, MSAA). Set
`VERNON_RENDERER=cpu` for the CPU fallback.

Coding agents must read [AGENTS.md](AGENTS.md) first.

## Getting the best result

- **Start from the demo.** Open **Pipeline > Replace Scene with Demo** or
  cook `examples/bodykit_pipeline.vrn`, and change one stage at a time.
- **Fix real-world scale at capture.** Scale bars and coded markers set the
  size of everything downstream. Check the marker scale error before you
  clean the scan.
- **Work through failed gates in pipeline order.** A stage whose gate fails
  is not trusted by the stages after it, so the first failure is the one
  that matters.
- **Read the warnings.** A white-box node cooks and says in a warning that
  its algorithm is a placeholder.
- **Treat AI output as a preview.** AI parts are unscaled concept geometry,
  the aero preview shows no drag or lift figure, and neither can satisfy a
  gate.
- **Leave Run off until the input files look right.** Nodes that drive
  OpenFOAM, CalculiX, COLMAP or a slicer write complete input files without
  launching anything.
- **Do not quote a Cd yet.** The CFD stage is not validated; see
  [Validation status](#validation-status).
- **Use `--ignore-gates` only for failures you expect** and have listed in
  the scene.

## Works well with

**Graphic design.** The flatten tools unroll body panels with a strain
report, which gives a true-size template for vinyl wrap and livery artwork
in Illustrator, Affinity Designer or Inkscape. Deviation and pressure maps
make clear figures for a build log or a client report.

**Product visualization.** Export the surfaced kit (OBJ, STL, PLY, 3MF or
STEP) and render it on the car in Blender, 3ds Max or KeyShot before
anything is printed. For a driving shot in 3ds Max, pair it with
[Motorsport vehicle rig](https://github.com/ssnfts/Motorsport-lap-animation)
and
[V-Ray material graph builder](https://github.com/ssnfts/V-Ray-material-graph-builder).

**Film.** A cleaned, aligned scan of a real car is a useful match-move and
set-extension reference, and the capture and clean stages produce one
without the rest of the pipeline. Printed parts can be made as practical
props with the split-and-print stage.

**Automation.** `vernon-cli cook` returns exit code 1 for a failed node and
3 for an unexpected gate failure, so it works as a check in CI. The MCP
server lets an agent build and cook graphs, and the in-app chat speaks the
OpenAI-compatible API, so a local llama.cpp or Ollama server can drive it.
External solvers and slicers are started as separate processes and can be
queued the same way.

## Documentation

| Document | For |
|---|---|
| [docs/PLAN.md](docs/PLAN.md) | Goal, intended users, the free-software landscape, the local-AI roadmap and the road to a release |
| [docs/SETUP.md](docs/SETUP.md) | A new Windows PC: toolchain, build, WSL/OpenFOAM, DrivAer data |
| [docs/NODES.md](docs/NODES.md) | Every node, generated |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Partial nodes and external-program routes |
| [docs/VALIDATION.md](docs/VALIDATION.md) | What each benchmark proves and what is not validated |
| [docs/specs/](docs/specs/) | The behaviour contract for each node group |
| [docs/README.md](docs/README.md) | Index of everything else |

## Clean room and licence

Vernon is a clean-room project: tools are re-implemented from public
documentation, papers and observable behaviour only. Read
[CLEAN_ROOM.md](CLEAN_ROOM.md) before contributing.

Apache-2.0: see [LICENSE](LICENSE) and [NOTICE](NOTICE). Third-party crates
and the user-interface fonts are listed with their licence texts in
[THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md); regenerate it with
`scripts/release/third-party-licenses.ps1` after changing dependencies, and
ship it with every binary. External open-source solvers run as separate
processes under their own licences. AI model weights and datasets such as
the TUM DrivAer surfaces are not part of this repository and keep their own
terms.

Vernon is not affiliated with, endorsed by or sponsored by any vendor or
product named in its documentation; those names are trademarks of their
owners and appear only to describe the behaviour being re-implemented.

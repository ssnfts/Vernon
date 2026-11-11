# Vernon documentation

## Start here
| Document | For |
|---|---|
| [../README.md](../README.md) | What Vernon does, step by step, and how to build it |
| [PLAN.md](PLAN.md) | The project plan: goal, intended users, the free-software landscape, local-AI roadmap, road to a release |
| [SETUP.md](SETUP.md) | A new Windows PC: toolchain, build, WSL/OpenFOAM, DrivAer data |
| [CFD-RUNBOOK.md](CFD-RUNBOOK.md) | The next CFD validation stages (0–3) and their run commands |
| [AI and machine plan](superpowers/plans/2026-11-04-ai-assistant-and-machine-split.md) | Which PC runs the CFD stages, and the plan for AI models, terminal and chat assistant |
| [NODES.md](NODES.md) | Generated catalogue of every node (`vernon-cli list --markdown`) |
| [ROADMAP.md](ROADMAP.md) | White-box nodes, external-program routes and library modules still to wire into nodes |
| [VALIDATION.md](VALIDATION.md) | The benchmark suite: what each check proves and what is not validated |
| [AGENT_GUIDE.md](AGENT_GUIDE.md) | Generated guide for AI agents driving Vernon through the MCP server (`vernon-cli agent-guide`) |

## Specifications (`specs/`)
Clean-room behaviour contracts, the source of truth for each node group:
[geometry basics](specs/geometry-basics.md),
[capture and scanning](specs/capture-scanning.md),
[scan registration](specs/native-scan-registration.md),
[photogrammetry](specs/real-photogrammetry.md),
[alignment and deviation](specs/alignment-deviation.md),
[conservative wrap](specs/conservative-wrap.md),
[field-aligned remesh](specs/field-aligned-remesh.md),
[connected auto-surface](specs/connected-auto-surface.md),
[continuity and primitives](specs/measured-continuity-primitives.md),
[reverse-engineering surfacing](specs/reverse-engineering-surfacing.md),
[mesh tools](specs/rhino8-mesh-tools.md),
[STEP export](specs/step-export.md),
[slicing and printing](specs/slicing-printing.md),
[print tools](specs/native-print-tools.md),
[automotive CFD](specs/automotive-cfd.md),
[structural FEA](specs/structural-fea.md),
[solar and thermal](specs/solar-thermal.md),
[performance validation](specs/performance-validation.md),
[AI models](specs/ai-models.md),
[UI conventions](specs/ui-dcc-conventions.md).

## Plans and hand-off (development history)
- [HANDOFF.md](HANDOFF.md): the ordered work packages (WP1–WP16) and how to do each.
- [CONTINUATION.md](CONTINUATION.md): the running log for resuming work. Its
  newest entry is at the top, and it records every verified run with its exact
  numbers. Older entries mention the original machine's paths and are kept as
  evidence.
- [superpowers/plans/](superpowers/plans/): one implementation plan per work package.

## Research (`research/`)
- [wp9-reference-baselines.md](research/wp9-reference-baselines.md) and
  [wp9-replication-baseline-v1.json](research/wp9-replication-baseline-v1.json):
  the accepted DrivAer reference, targets and input checksums.
- [wp9-ddes-route-plan.md](research/wp9-ddes-route-plan.md): why the steady
  convergence gate is out of reach and the planned transient route.
- [free-software-landscape.md](research/free-software-landscape.md): what free and
  AI-assisted tools exist per stage, surveyed 2026-11-11.
- [k2/](research/k2/): vetted external review notes per stage (suggestions only).

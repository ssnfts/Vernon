# Free software landscape, 2026-11-11

Survey behind section 4 of [the project plan](../PLAN.md). Product names appear
here only to describe what exists; see `CLEAN_ROOM.md`. Sources are web
searches run on 2026-11-11 and are leads, not audited facts.

## Tools by stage

No free tool covers the chain. Each covers one stage, and the hand-offs
between them are where makers lose accuracy.

| Tool | Licence | Covers | Gap that Vernon fills |
|---|---|---|---|
| COLMAP, Meshroom (AliceVision) | BSD, MPL | Photogrammetry | No scale gate, no hand-off to surfacing |
| CloudCompare | GPL | Point clouds, ICP, deviation | Stops at measurement; no surfacing or CAD |
| MeshLab, Instant Meshes | GPL, BSD | Mesh cleaning, remeshing | Mesh only |
| Blender | GPL | Mesh modelling, retopology | No engineering gates, no B-spline or STEP route |
| FreeCAD with CfdOF and FEM | LGPL | Parametric CAD, OpenFOAM and CalculiX front ends | No scan-to-surface workflow, no automotive validation recipe |
| OrcaSlicer, PrusaSlicer | AGPL | Slicing, cut tool with connectors | No partition search across a whole kit, no strain check on snap fits |
| OpenFOAM | GPL | The CFD solver | No case builder for cars, no acceptance gates |
| Ladybug Tools | open source | Solar and climate analysis | Tied to a paid CAD host |

Free tiers of closed tools (Fusion Personal, SimScale Community, Onshape
Free) are the real competition for user 1. Their limits are non-commercial
terms, public projects or no simulation.

## Free software that already uses AI in this space

| Project | What it does | Local? | Note for Vernon |
|---|---|---|---|
| [PhysicsNeMo and PhysicsNeMo-CFD](https://nvidia.github.io/physicsnemo/blog/2026/05/29/physicsnemo-cfd/) (Apache-2.0) | Trains and benchmarks aero surrogates (DoMINO, GeoTransolver, X-MeshGraphNet, FIGConvNet) on DrivAerML | Yes, Python and config files, no GUI | Vernon already runs DoMINO. Its benchmarking workflow is the reference for the surrogate error report. |
| [Foam-Agent](https://arxiv.org/abs/2505.04997v1), MetaOpenFOAM, OpenFOAMGPT | LLM agents that write and repair OpenFOAM cases from a prompt | Mostly cloud models | Authors report 83.6% task success for Foam-Agent on their own benchmark. Shows that log-reading and case repair by an agent works; Vernon's version must never touch a gate. |
| [FreeCAD MCP servers](https://github.com/proximile/FreeCAD-MCP), [agentcad](https://www.everydev.ai/tools/agentcad), [vibe-cading](https://groups.google.com/g/CADQuery/c/9R5y9g78Xc0) | Let a coding agent drive FreeCAD, build123d or CadQuery | Tool runs locally; the model is usually cloud | Closest to Vernon's MCP server. They generate prismatic CAD from text, not surfaces from scans, and have no engineering gates. |
| [Point2CAD](https://github.com/prs-eth/point2cad), [CAD-Recode](https://huggingface.co/filapro/cad-recode) | Neural point cloud to CAD | Yes | Both CC-BY-NC. Cannot ship in Vernon; useful only as a research comparison. |
| TRELLIS, TripoSR, PartCrafter, MeshAnything | Image or text to 3D mesh | Yes | MIT. Concept geometry only; never scaled or watertight without Vernon's clean stage. |

A 2026 review of open text-to-CAD tools concludes they are not ready for
complex production parts
([Leo AI](https://www.getleo.ai/blog/open-source-text-to-cad-tools-free)).
That matches Vernon's position: AI proposes, measured gates decide.

**Where Vernon is alone:** no surveyed free tool combines scan-to-surface,
print splitting, CFD, FEA and a local agent behind one set of numeric gates.
**Where it is behind:** every tool above has users, releases and a
track record on real parts. Vernon has none of those yet.

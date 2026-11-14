# Launch post drafts (DRAFT, nothing posted)

Channels: forum (long), short social, Show-HN-style. `[repo URL]` and `[demo video URL]` are
placeholders for the Owner. No personal details. No vendor names. Claim trace in the last section.

---

## 1. Forum post (long)

**Title:** Vernon: an open-source node graph for 3D-printed car body kits, with a gate between every stage (pre-alpha)

If you build body kits, your pipeline is probably like this: scan the car, clean the mesh in one
program, build surfaces in another, split and slice in a third, then run airflow, structure and
heat checks in whatever you can get hold of. Each hand-off is a file export. Nothing checks that
the last step's output was good enough for the next one.

I have been building Vernon to put that chain in one place. It is a node graph (Apache-2.0, runs
locally, no cloud) with a familiar quad-viewport, command-panel and modifier-stack interface.

**How it works.** Every stage writes a numeric report: deviation percentile after aligning a
scan, bed-fit margin for split parts, safety factor at a mount, surface temperature against the
material's heat-deflection temperature, and others. A gate is a limit on one of those numbers. A
cook ends with a list of every metric outside its limit, with value, limit and margin.

**What is in it.** 198 node types over nine stages (capture, clean and align, mesh to surface,
split and print, airflow, structure, solar and heat, flattening, performance). 195 are
implemented; three are placeholders that say so in a warning. Heavy lifting is done by open-source
programs run as separate processes on your machine, only when you switch a node's run option on:
OpenFOAM (through WSL), CalculiX, COLMAP and a slicer.

**The demo.** The example scene runs a synthetic fender through all nine stages: 40 nodes, 0
failed. Five gates fail on purpose and are listed in the scene with reasons: the panel has no
wheels, the built-in solid mesher is a placeholder, and the airflow case is written but not
solved, so there are no solver forces yet. I would rather show those than have a demo that looks
more finished than it is.

**What is checked.** Analytic cases (beam deflection and stress, a patch test, sphere and cylinder
geometry, ICP recovering a known transform, flattening a cylinder, unit conversion, file round
trips) pass. Distance and linear-static results match two open-source tools on small cases. For
airflow, the one claim I will make: on the classic slant-body test case with a 25° slant, the fast
CFD tier puts drag within about 7-10% of the published 1984 wind-tunnel value. That uses a half
model of about half a million cells, and each run after the first takes under 35 minutes on one
workstation.

**What is not checked, and I mean all of this.**
- It is not certified engineering software, and a passed gate means "within the limit you set",
  not "safe".
- No physical-accuracy claim: the repository has no wind-tunnel, strain-gauge, scanner-certificate
  or lap-timing data.
- Please do not read the airflow result as a drag or downforce prediction for a road car or your
  kit, or as a way to rank two designs. It is one test body. The steady runs did not converge.
- Printed-part FEA does not model layer anisotropy, creep, thermal expansion or bolt preload, and
  the material retention curves are placeholders.
- Scan scale from photogrammetry, slicer G-code against real slicer output, solar, UV, tuft
  analysis, lap simulation, quad remesh and STEP output have no measured reference.
- Nothing has been compared with a commercial tool.
- Pre-alpha: names and file format may change. Windows 10/11 is the supported platform and the
  package is unsigned.

**Where to find it.** [repo URL]. `docs/VALIDATION.md` lists every check and what it proves.

**What I would like from you.** Try your own scan. Tell me where a gate is missing, where the
wrong thing fails, or where a unit mix-up slips past the import check. If you have measured data
(coastdown, pressure taps, strain readings) that could be used to validate a stage, that is the
most useful thing anyone could send.

---

## 2. Short social post

> Vernon, pre-alpha: an open-source (Apache-2.0) node graph for 3D-printed car body kits.
> Scan, clean, surface, split, airflow, structure, heat, performance, with a numeric gate
> between each stage. Runs locally. 198 node types, a 40-node demo that cooks with 5 gates failing
> on purpose, and a plain list of what is not validated. Not certified engineering software.
> [repo URL] [demo video URL]

(Image: a frame from the demo recording
showing the Gate Dashboard with the expected reasons visible.)

---

## 3. Show HN-style post

**Title:** Show HN: Vernon, an open-source node graph for 3D-printed car body kits (pre-alpha)

Body:

Vernon is an Apache-2.0 Rust node graph for the scan, clean, surface, split/print, airflow,
structure, heat and lap-time chain used to design 3D-printed car body kits. Each stage emits a
numeric report and a limit on it (a "gate"); the next stage is meant to trust its input only when
the gate passes. It runs locally. Solvers (OpenFOAM, CalculiX, COLMAP, a slicer) run as separate
processes, only when a node's run flag is on, which keeps the core permissively licensed.

There is a desktop app (egui, wgpu), a CLI (`list`, `info`, `cook`) and a stdio MCP server so an
agent can build and cook graphs.

Try it: `cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn` cooks 40 nodes with 0
failed and ends "gates: 0 failing, 5 expected". The five are declared in the scene with reasons
(single panel has no wheels, placeholder solid mesher, airflow case written but not solved).

Validation is the weak point and I have tried to be exact about it. Analytic checks pass, and
distance and linear-static results match two open-source tools on small cases. For airflow, the
only claim: on the classic slant-body case with a 25° slant, the fast tier is within about 7-10%
of the published 1984 wind-tunnel value (half model, ~0.5M cells, under 35 minutes per run after
the first). It is not a road-car drag prediction and cannot tell you which of two designs is
better. Printed-part FEA ignores anisotropy and creep. Nothing is compared with a commercial
tool. It is pre-alpha and not certified engineering software. Details: `docs/VALIDATION.md`.

I would most like measured data to validate against, and criticism of the gate design.

[repo URL]

Likely questions to prepare (answers from VALIDATION.md only):
- "Why not just use the open-source solvers directly?" Vernon writes the inputs, checks the outputs and joins the stages.
- "Is the drag number usable?" Only as stated above.
- "Why is the CFD claim so narrow?" Because the steady runs did not converge and the road-car contract is unmet.

---

## Claim trace

| Claim | Source |
|---|---|
| 198 node types, 195 implemented, 3 placeholders, nine stages | `docs/NODES.md`, `CHANGELOG.md` |
| 40 nodes, 0 failed, 0 failing gates, 5 expected, reasons | cook of `examples/bodykit_pipeline.vrn`, 2026-11-14 |
| Analytic checks pass; open-source tool comparisons on small cases | `docs/VALIDATION.md` Tiers A and B (stated by CHANGELOG as passing) |
| Slant-body 25° sentence, half model, 0.5M cells, under 35 minutes | `docs/VALIDATION.md` "CFD: what may be claimed" (signed 2026-11-13) |
| Steady runs did not converge; no ranking of designs; no road-car figure | same section and "What is NOT validated" |
| Printed-part FEA limits; retention curves are placeholders | "What is NOT validated" |
| No physical-accuracy claim; nothing compared with a commercial tool | "What is NOT validated" |
| Windows supported, package unsigned, Linux/macOS CI only | `CHANGELOG.md` Known limits |
| Solvers run as separate processes only when run is on | `CHANGELOG.md`, `CLEAN_ROOM.md` |
| MCP server, CLI commands | `README.md`, `CHANGELOG.md` |

Open issue: "Analytic and open-source comparisons pass" relies on the bench report. Re-run
`bench\run.ps1 -Quick` and confirm before posting; I did not run it.

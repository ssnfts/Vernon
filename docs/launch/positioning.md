# Vernon positioning brief (DRAFT)

Status: draft for the Owner. Nothing here is published. Every number comes from
`docs/VALIDATION.md`, `CHANGELOG.md`, `docs/NODES.md` or a cook of
`examples/bodykit_pipeline.vrn` (cooked 2026-11-14: 40 nodes, 0 failed, 0 failing gates, 5 expected).

## Who it is for
People who design 3D-printed car body kits: hobby and small-shop builders, students, and small
motorsport teams. They own a printer and a scanner or a phone, not a CFD or FEA department.

## The problem
A body kit goes scan, clean, surface, split and print, airflow, structure, heat, performance.
Each step lives in a different program with its own file format and its own units. Nothing checks
that step N's output is fit to feed step N+1. A scan that is 3 mm off, a part that does not fit
the bed, or a mount that is too weak is found late, after print hours, or never.

## What is distinctive
- **One open node graph for the whole chain.** Apache-2.0, 198 node types across nine stages
  (195 implemented, 3 placeholders that say so in a warning).
- **Gated stages.** Every stage emits a numeric report (deviation percentile, bed-fit margin,
  safety factor, surface temperature against heat-deflection temperature, and so on). A gate is a
  limit on a report metric. Later stages are not trusted until the earlier gates pass.
- **Local.** No cloud account and no upload. External open-source solvers run as separate
  processes on your machine, only when you ask.
- **Honest by construction.** Placeholders and expected failures are listed, not hidden.

## What is validated (exact)
- Analytic checks pass: beam deflection and stress, patch test, sphere and cylinder geometry, ICP
  recovering a known transform, decimation bounds, flattening a cylinder, snap-fit beam formula,
  unit conversion, file round trips.
- Comparisons with open-source tools pass on small cases: cloud-to-mesh distance against
  an open-source point-cloud tool (1e-4 mm), linear static cantilever against CalculiX (2%).
- Vernon reduces OpenFOAM's own force history to Cd within 1%. This checks the arithmetic, not the flow.
- The only signed CFD sentence: "On the classic slant-body test case with a 25° slant,
  Vernon's fast CFD tier puts drag within about 7–10% of the published 1984 wind-tunnel value. It
  uses a half model of about half a million cells, and each run after the first takes under 35
  minutes on one workstation." Method labels (fast tier, DDES, 0.5M cells, half model) always travel with it.

## What is NOT validated
- Physical accuracy of anything. No wind-tunnel, strain-gauge, scanner-certificate or lap-timing
  reference data is in the repository.
- CFD beyond the one sentence above: no "validated CFD", no "wind-tunnel accuracy", no "within 5%",
  no drag or downforce figure for a road car or a user's kit, no ranking of two designs, nothing on
  the 35° body. The steady runs did not converge.
- FEA of printed parts (layer anisotropy, creep, thermal expansion, bolt preload); material
  retention curves are placeholders.
- Photogrammetry scale, slicer G-code against real slicer output, solar, UV, tuft analysis, lap
  simulation, quad remesh and STEP output.
- Any comparison with a commercial tool. Performance numbers are one machine, one run.
- Pre-alpha: node contract, file format and parameter names may change. The Windows package is unsigned.

## What it is not
Not certified engineering software. Not a replacement for a wind tunnel, a structural test or a
professional review of safety-critical parts. Reports are screening aids that tell you which
stage to look at, and a passed gate means "within the limit you set", not "safe".

## Message hierarchy
1. Scan to performance in one graph, checked at every step.
2. Open (Apache-2.0), local, no cloud.
3. It tells you what it has not proven.

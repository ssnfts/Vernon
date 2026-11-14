# Landing page text (DRAFT, single page)

Sources for every number: `docs/NODES.md`, `docs/VALIDATION.md`, `CHANGELOG.md`, cook of
`examples/bodykit_pipeline.vrn`. No logos, screenshots or vendor text. Not published.

---

## Headline
**From scan to lap time, one checked graph.**

Subhead: Vernon is an open-source (Apache-2.0) node graph for designing 3D-printed car body kits.
Each stage reports numbers, and the next stage does not trust them until the limits you set pass.
It runs on your PC. Pre-alpha.

Buttons: Get the source | Read what is validated

---

## Three feature blocks

### 1. One chain, nine stages
Capture, clean and align, mesh to surface, split and print, airflow, structure, solar and heat,
flattening, performance. 198 node types, 195 implemented. Three are placeholders and say so in a
warning when they run. Files in: OBJ, STL, PLY, 3MF, XYZ, E57, LAS. Files out include 3MF and STEP.

### 2. Gates between stages
Every stage writes a report: deviation percentile, bed-fit margin, safety factor at a mount,
surface temperature against the material's heat-deflection temperature, and more. A gate is a
limit on one of those numbers. The Gate Dashboard shows value, limit and margin for each, and an
expected failure carries the reason you wrote next to it.

### 3. Local and open
No account, no upload. Heavy work runs as separate open-source programs on your machine (OpenFOAM
through WSL, CalculiX, COLMAP, a slicer), only when you switch a node's run option on. The
example graph writes their input files without them. A desktop app with quad viewports, a command
panel and a modifier stack sits on a command-line tool you can script.

---

## Try it in a minute
Open the demo from the start panel. It cooks a synthetic fender through all nine stages: 40
nodes, 0 failed, 0 unexpected gate failures, 5 expected ones that the dashboard explains.

---

## What Vernon does not do (read this before you rely on it)
- **It is not certified engineering software.** A passed gate means "within the limit you set".
  It does not mean a part is safe.
- **No physical-accuracy claim.** The repository holds no wind-tunnel, strain-gauge, scanner-certificate or lap-timing data.
- **Airflow.** One case is signed off: on the classic slant-body test case with a 25° slant,
  the fast CFD tier puts drag within about 7–10% of the published 1984 wind-tunnel value (half
  model, about half a million cells, each run after the first under 35 minutes on one
  workstation). That is all. No drag or downforce figure for a road car or your kit, no ranking of
  two designs, no "validated CFD". The steady-state runs did not converge.
- **Printed-part structure.** Layer anisotropy, creep, thermal expansion and bolt preload are not
  validated, and material retention curves are placeholders.
- **Not validated against measurement:** scan scale from photogrammetry, slicer G-code against real
  slicer output, solar, UV, tuft analysis, lap simulation, quad remesh and STEP output.
- **Not compared with any commercial tool.**
- **Pre-alpha.** Names and file format may change. Windows 10/11 is the supported platform; the
  package is unsigned. Linux and macOS are built and tested in CI only, and CI has not yet run on
  GitHub.

---

## Get started
1. Install the Rust toolchain (GNU target on Windows) and clone the repository. `docs/SETUP.md`
   walks a fresh PC through it; `scripts/setup/setup-dev.ps1` checks it and installs nothing.
2. In a new PowerShell: `. .\scripts\dev-env.ps1`
3. Cook the example from the command line:
   `cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn`
4. Or open the app: `cargo run -p vernon`, then choose "Open the demo (body-kit pipeline)".
5. Import your own scan ("Import a mesh or scan"). Vernon reads lengths as millimetres and shows
   the size so you can catch a unit mix-up.
6. Optional: connect an MCP client with `vernon-cli mcp` (see the README).

Footer: Apache-2.0. Vernon is not affiliated with or endorsed by any vendor or product. Datasets
such as the TUM DrivAer surfaces are not included and keep their own terms.

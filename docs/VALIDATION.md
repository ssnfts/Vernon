# Validation and benchmarks

How Vernon is checked, what each check proves, and what is **not** validated. The suite is
`crates/vernon-bench`, run with `bench\run.ps1` (Windows) or `bench/run.sh`. It writes
`bench/results/REPORT.md` (human) and `bench/results/results.json` (machine). Every check ends as
**PASS**, **FAIL** or **SKIPPED(reason)**; nothing else. A skipped check is not a pass.

```powershell
. .\scripts\dev-env.ps1
.\bench\run.ps1 -Quick          # everything except the 1M-triangle cases
.\bench\run.ps1                 # full, including 1M triangles (minutes to tens of minutes)
.\bench\run.ps1 -UpdateBaseline # store this machine's timings as the regression baseline
```

Rule for every number in this repository's validation material: it is an analytic result, a
published figure with its source, or the measured output of a tool run on this machine. Where a
commercial tool would be the reference and no licensed copy has been run, the cell says **owner to
record** and the slot in `bench/golden/` stays empty. No competitor figure is estimated or recalled.

## Tiers

| Tier | Reference | Runs when | Where |
|---|---|---|---|
| A | Closed-form and textbook results | always | `tier_a.rs` |
| B | Open-source tools run as separate processes (CloudCompare, CalculiX, OpenFOAM output, COLMAP model, a slicer) | tool or recorded result present, else SKIPPED | `tier_b.rs` |
| C | Output the owner records from a licensed commercial tool | `bench/golden/<slug>/golden.json` exists, else `SKIPPED(no golden)` | `tier_c.rs`, `bench/golden/README.md` |
| P | Wall time and peak memory on synthetic meshes against stated targets and a stored baseline | unless `-NoPerf` | `perf.rs` |
| E | Example pipeline, per-stage CLI smoke, desktop app start | always (app needs a build) | `e2e.rs` |

## What is verified, and how

### Tier A (analytic)

| Check | Reference | Tolerance |
|---|---|---|
| `A.fea.beam_deflection` | Euler-Bernoulli tip deflection `PL^3/3EI` (Roark Table 8.1); fine mesh against beam plus shear (1.008x) | 3% coarse, 1% fine, 0.1% root reaction |
| `A.fea.beam_stress` | Flexure formula at mid-span | 10% (nodal stress extrapolation) |
| `A.fea.patch_uniaxial` | Patch test: reaction `pA` and uniform stress | 1e-6 reaction, 2% stress |
| `A.geo.sphere_area_volume` | `4 pi r^2`, `4/3 pi r^3`; error must shrink under refinement | 0.2% area, 0.3% volume at 64x128 |
| `A.geo.plane_cylinder` | Plane `ab`; exact n-gon prism; true cylinder | 1e-9 exact tessellation, 0.05% vs true |
| `A.clean.icp_known_transform` | A known rigid transform is recovered (point-to-plane and point-to-point) | worst vertex 0.01 mm / 0.05 mm |
| `A.clean.qem_bounds` | Decimated sphere vertices stay near the analytic sphere; volume kept; `max_error` mode bound holds when measured against the input mesh | 2% of r; 5% volume; 0.4 mm |
| `A.flatten.developable_cylinder` | A cylinder strip flattens to `r*theta` by width with no strain (do Carmo) | 1% size, 0.5% strain |
| `A.flatten.sphere_cap_distorts` | Negative control: Gaussian curvature forbids isometric flattening; the strain report must show it | strain above 1% |
| `A.print.snap_fit_beam` | `eps = 1.5 t y / L^2`, and its scaling laws | 1e-12 |
| `A.units.set_units` | cm, m, in to mm exact factors; round trip | 1e-12 |
| `A.io.file_round_trip` | OBJ, PLY, STL write then read | 1e-6 mm text, 5e-3 mm for STL (32-bit) |
| `A.cfd.ahmed_drag_window` | Ahmed-body drag, existing OpenFOAM runs only; never started by the bench | within 15% of a published value, and the run must report convergence |

### Tier B (reference tools on this PC)

| Check | What is compared | Tolerance |
|---|---|---|
| `B.cloudcompare.c2m` | `clean.mesh_distance` against CloudCompare cloud-to-mesh distance on 3000 synthetic points, per point | 1e-4 mm (CloudCompare stores 32-bit floats) |
| `B.calculix.cantilever` | Vernon's linear solver against CalculiX on the deck Vernon writes; also CalculiX against beam theory | 2% tip and stress; 3% beam |
| `B.openfoam.force_reduction` | The Cd Vernon reported against an independent mean of OpenFOAM's own `forceCoeffs` history | 1%. This checks the reduction, not the physics of the run |
| `B.colmap.sample_model` | Vernon reads a recorded COLMAP sparse model of the public sample without loss; the model is usable | exact count; mean reprojection error under 1 px |
| `B.slicer.flavour_check` | Real slicer G-code is accepted by the dialect check | needs a working slicer profile set, see below |

### Performance (P)

Targets are fixed in `perf.rs` before any measurement and printed in `REPORT.md`. Each case runs in
its own process, so peak memory (working set) is per case. A case fails if it misses its target,
if it times out, or if it regresses against `bench/baseline.json` by more than 1.5x + 0.25 s in
time or 1.3x + 20 MB in memory. The baseline is machine specific: re-record it with
`-UpdateBaseline` after hardware changes. Cases: ICP, cloud-to-mesh deviation, quadric decimation,
conformal flatten, quad remesh (5000 quads) and the built-in linear FEA solve (linear tetrahedra),
at 10k, 100k and 1M triangles (tetrahedra for FEA).

### End to end (E)

`E.pipeline.cook` cooks `examples/bodykit_pipeline.vrn` and requires 38 nodes, 0 failed, 0
unexpected failing gates and the 2 documented expected gates (AGENTS.md). The brief for this suite
quoted 40 nodes and 5 expected gates; the file in the repository has 38 and 2, so the suite
checks what the repository documents. `E.pipeline.stage_smoke` runs `vernon-cli cook --node` for
the last node of each stage. `E.app.start_probe` launches the desktop binary and requires it to
stay up for 6 s; it is SKIPPED unless the app was built (`bench\run.ps1 -WithApp`).

## What is NOT validated

- **No physical-accuracy claim.** There is no wind-tunnel, strain-gauge, scanner-certificate or
  lap-timing reference data in this repository. Agreement with an analytic case or another
  solver proves the arithmetic, not that the car will behave that way.
- **CFD.** The existing Ahmed-body runs did not converge in 3000 iterations and their drag
  coefficients sit outside the published window (see the report). Vernon's CFD numbers must not be
  quoted as drag predictions. The bench starts no CFD job.
- **FEA of printed parts.** Linear tetrahedra lock in bending; layer anisotropy, creep, thermal
  expansion and bolt preload are not validated. Material retention curves are placeholders.
- **Photogrammetry.** COLMAP is not re-run by the bench. Only the reading of a recorded model is
  checked, not scale accuracy against a surveyed target.
- **Slicer G-code.** Command-line slicing needs a complete exported profile set (see
  `tests/golden/README.md`); without it the check is SKIPPED, so dialect tables are validated only
  by unit tests, not against real slicer output.
- **Solar, UV, tuft analysis, lap simulation.** No measured data; unit tests and the specs'
  hand calculations only.
- **Quad remesh and STEP/NURBS output** are checked for deviation bounds by their own tests, not
  against another implementation (Tier C slots are empty).
- **Performance numbers** are one machine, one run; no statistical treatment.
- **Commercial tools.** Nothing is compared with any licensed product until the owner records a
  golden file.

## Competitor comparison

Cell types: **measured** = produced by this suite on this machine; **published** = figure with its
URL; **owner to record** = needs a licensed copy, nothing is claimed.

| Quantity | Vernon | Reference tool / figure | Source |
|---|---|---|---|
| Cloud-to-mesh distance, per point | measured: worst difference vs CloudCompare in `REPORT.md` (`B.cloudcompare.c2m`) | CloudCompare CLI as installed (open source; the stored golden was recorded with 2.13.2) | measured by this suite |
| Linear static tip deflection and stress | measured: `B.calculix.cantilever` | CalculiX 2.21 (open source) | measured by this suite |
| Cantilever vs beam theory | measured: `A.fea.beam_deflection` | closed form | Roark, Table 8.1 |
| Ahmed body Cd, 25 deg slant | measured: 0.369 (finest mesh, **not converged**) | 0.299 | https://skill-lync.com/student-projects/project-1-flow-over-ahmed-body-7 (secondary table; primary: Ahmed, Ramm, Faltin, SAE 840300, 1984; also https://kbwiki.ercoftac.org/w/index.php/AC_1-05; confirm before quoting) |
| Ahmed body Cd, 35 deg slant | measured: 0.314 (finest mesh, **not converged**) | 0.279 | same as above |
| Lap time accuracy of a point-mass simulator | Vernon `perf.lap_sim`: not validated | vendor states roughly 10% of real data | https://optimumg.com/product/optimumlap/ (vendor claim, not independently verified) |
| Quad remesh deviation / quad quality | measured by node report | owner to record | `bench/golden/rhino/` |
| Mesh to solid / STEP deviation | measured by node report | owner to record | `bench/golden/fusion/` |
| Auto-surface deviation | measured by node report | owner to record | `bench/golden/quicksurface/`, `bench/golden/geomagic-designx/` |
| Scan inspection deviation statistics | measured: `B.cloudcompare.c2m` | owner to record | `bench/golden/zeiss-inspect/` |
| Road-vehicle CFD CdA | not validated | owner to record | `bench/golden/airshaper/`, `bench/golden/simscale/` |
| Structural solve (commercial solver) | measured: `A.fea.*` | owner to record | `bench/golden/ansys/` |
| Lap time, same vehicle and track | measured by `perf.lap_sim` | owner to record | `bench/golden/optimumlap/` |
| Incident radiation | measured by node report | owner to record | `bench/golden/ladybug-tools/` |

## Adding a check

Add a function to `tier_a.rs` returning `run_case(id, tier, title, reference, |ck| ...)` with
`ck.close(name, measured, reference, tol_rel, tol_abs, unit)`. State the reference and tolerance
in the case before you run it; do not adjust a tolerance to make a failing case pass, and say why
if a reference itself was wrong. A check that makes no comparison fails.

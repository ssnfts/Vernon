# WP9 turbulence diagnostics before another full solve

Date: 2026-11-08. Status: planned, tooling not built yet. Trigger: Stage 2
(`wp9-solve-fastback-20261108-a`) residuals stopped falling around iteration 400,
2–3 orders above the 1e-5 gate, while k and omega were clipped in every iteration.
The user stopped Stage 2 at iteration 2546 (status `cancelled`) to run this
screen. `openfoam2606-dev` is installed, so the coded counter below is available.

These runs **screen** numerics. None of them can demonstrate the two-window
acceptance, feed `cfd.benchmark_check` or `cfd.mesh_study`, or change a gate.
Acceptance is evaluated only on a full protocol run with the intended production
numerics. The two kinds of change are different:

- **Upwind** changes the discretisation. It adds spatial numerical diffusion. If
  it were kept beyond the specified startup, the result would need an accuracy
  assessment.
- **Under-relaxation** changes only the iteration process: the converged
  solution of the same discretised equations is unchanged. A run that settles
  only under heavy relaxation still has to show proper convergence with the
  settings it ends on.

Both need the same convergence assessment.

## Evidence so far (Stage 2 log, iterations 1–1902, read while the run continues)

| Item | Value |
|---|---|
| Iterations with `bounding k` / `bounding omega` | 1902 / 1902 (every iteration) |
| k minimum before clipping, mean per iteration | -0.84 (1–200) → -0.70 (1801–1902) m²/s²; worst -2.5 |
| k maximum | 36–50 m²/s² (freestream k = 0.0024) |
| omega minimum, mean per iteration | -7e14 (1–200), then -6e3 to -2e4 throughout |
| omega maximum | 4.5e22 in iterations 1–200; 1e6–1e7 afterwards |
| RMS initial residual, 902–1901 vs 402–1401 | p 1.55e-3 / 1.63e-3, U 1.92e-2 / 1.99e-2, k 2.09e-4 / 2.19e-4, omega 1.13e-4 / 1.14e-4 |
| Cd, iterations 902–1401 vs 1402–1901 | 0.2750 / 0.2743 (−0.28%), sd 0.0073 (provisional) |
| y+ at 2000 (min–max, mean) | body 0.32–802 (24), ground 0.36–1795 (50), wheel_fr 1.9–423, wheel_rr 0.97–344 |
| Wall treatment | `nutUSpaldingWallFunction`, `omegaWallFunction`, `kqRWallFunction` |

**Recipe deviation found.** `docs/specs/automotive-cfd.md` (numerics) specifies
`div(phi,k|omega) bounded Gauss limitedLinear 1` with **upwind on the first 500
iterations**. `fv_schemes` in `crates/vernon-nodes-cfd/src/foam.rs` writes
`limitedLinear 1` from iteration 1, and the near-divergent omega in iterations
1–200 is consistent with that. The pilot and Stage 2 both ran without the
specified startup.

Also noted, not changed: max non-orthogonality is 65°, so the spec's rule leaves
`nNonOrthogonalCorrectors 0` and `corrected` laplacians. The 70° switch is
not a cause found here.

The log can't say *where* cells are clipped. `bound()` replaces negative values
with a local average before anything is written, so written fields don't show
the undershoots.

## Common setup for every diagnostic

- Same 12,056,197-cell Fastback mesh, reused from a run folder's decomposed case
  (16 processors). Record the source run id and SHA-256 of every
  `processor*/constant/polyMesh/*`. No remeshing.
- Same boundary conditions, `transportProperties`, `momentumTransport`, 16 ranks,
  and the same per-rank 4 GiB cap, under the bounded runner (cancel, memory
  sampler, Linux cleanup).
- Labelled `diagnostic_only`, never `coefficients_reported`.
- Added instrumentation, diagnostic cases only:
  - `solverInfo` with `writeResidualFields yes`, for fields of initial residuals
    at write times.
  - `fieldMinMax` on k, omega and nut, with locations, every iteration.
  - Writes every 250 iterations with `purgeWrite 2`, about 1 GB each.
  - If `openfoam2606-dev` is installed: a `scalarCodedSource` fvOption with
    `codeCorrect` on k and omega. It runs after the solve and before
    `bound()`. Confirm that order in the v2606 source before relying on it.
    Per iteration it counts the cells below zero and their sum. Every 250
    iterations it writes a marker field of cells that were clipped. Each
    clipped cell is classified as: within the wheel boxes, within 0.05 m of
    the ground, underbody (below the body floor), inside the layer band (wall
    distance below the total layer thickness), near or far wake, or other.

## Revision after review (2026-11-08): C0 and D1 first

Only C0 and D1 are committed now. The rest of the matrix is chosen from their
results.

- **C0** runs production numerics for iterations 0–1000.
- **D1** runs upwind k/omega for 1–500 as one solver process. A second process
  restarts from the written time 500 and runs 501–1000 with the production
  `limitedLinear 1`. Neither process edits its schemes file while running, and
  `runTimeModifiable` is false.
- Both are compared over iterations 501–1000.
- C0 is also compared, iteration by iteration, with the original Stage 2 log
  over 1–1000. D1 is queued only if C0's instrumentation checks pass and C0
  reproduces Stage 2.

The counter was proven on a 4000-cell fixture before the large case
(`diagnostic::tests::fixture_*`):

- Known injected values: 104 negative cells and 120 below the floor, with exact
  volumes, regions, wall-distance bands and worst-cell locations. Identical in
  serial and on 4 ranks.
- The floor is the model's own `kMin`/`omegaMin` (1e-15), read at run time.
- The counter's minimum equals OpenFOAM's printed pre-clip minimum in every
  iteration.
- Fields and residuals are byte-identical with and without the counter.
- A restart from a written time reproduces a continuous run exactly. A switched
  run equals pure upwind up to the switch and differs from it afterwards.

The counter is compiled on the fixture before a timed solve. The large case
must load that library without compiling. Writes use `purgeWrite 0`, and
clipping maps are kept at iterations 1, 10, 50, 100, 250, 500, 501, 510, 550,
750 and 1000. Per-iteration summaries go to
`postProcessing/wp9ClipCounter/clip_{k,omega}.dat` and are never purged.
Machine time is about 9.4 solver-hours for both runs, plus copying, compiling
and startup.

## Original run matrix (one change per run; machine time at 17 s per iteration)

| Id | Start | Change | Iterations | Est. |
|---|---|---|---|---|
| C0 | t=0 (potentialFoam) | none, production numerics, instrumented | 500 | 2.4 h |
| D1 | t=0 | upwind k/omega for 1–500, then `limitedLinear 1` via `timeActivatedFileUpdate` | 1000 | 4.7 h |
| C1 | Stage 2 t=2000 | none, instrumented continuation | 500 | 2.4 h |
| D2 | Stage 2 t=2000 | k and omega equation relaxation 0.9 → 0.5; U, p and schemes unchanged | 500 | 2.4 h |
| D3p | Stage 2 t=2000 | `relTol 0.01` for p only | 200 | ~1 h+ |
| D3U | Stage 2 t=2000 | `relTol 0.01` for U only | 200 | ~1 h+ |
| D3t | Stage 2 t=2000 | `relTol 0.01` for k and omega only | 200 | ~1 h+ |

Order: C0, D1, C1, D2, then the D3 set. About 15 h in all, run one at a
time.

## Comparisons

- **Clipping:** iterations with bounding; cells clipped per iteration and their
  regions (with the counter); the negative minimum and average from the
  `bounding` lines.
- **Residuals:** initial-residual trend per field over the same iteration
  ranges as the control.
- **For D3, judge by the next iteration's initial residual** in every field
  against C1 at the same iteration, not by the smaller final linear residual.
  Record extra linear iterations and the time per iteration.
- **Forces:** Cd, Cl and the wheel forces, as a mean and spread over the last
  250 iterations of each run against its control. These are screening numbers
  only.
- **Locations:** clipped-cell markers, high residual-field cells and the
  `fieldMinMax` extreme locations, checked against wheels, ground contact,
  underbody details and layer transitions. Check y+ in the same regions.

## Results: C0 and D1 (2026-11-09)

Both runs completed (exit 0, cleanup confirmed, source unchanged). C0 passed its
gate and reproduces Stage 2 exactly over 1–1000. Under upwind, D1 clipped
nothing in 1–500. After the switch to `limitedLinear 1` it clipped in every
iteration of 501–1000, averaging 586 k and 88 omega cells, against C0's 611 and
90. The regions, wall bands and recurring worst cells are the same as C0's.
Residuals and screening forces over 501–1000 are indistinguishable from C0's.
Numbers are in docs/CONTINUATION.md.

Outcome: decision 2 below applies. Clipping returns after the switch, so the
specified upwind startup is not the remedy. The persistent undershoots belong
to `limitedLinear 1` on specific near-wall cells, mostly 2 mm–1 cm from walls,
near the body, in the underbody and at the front wheel. Decision 4, the
location analysis, is the strongest next lead. D2 and D3 are still open for the
user to choose.

## Location analysis (2026-11-09)

Done with `scripts/cfd/wp9-location-analysis.sh` on a copy of the cases (details
in docs/CONTINUATION.md).

- **Which cells:** the same small set of cells is clipped in C0 and D1. 154 k cells
  are clipped in every iteration of 751–1000 in both runs, and 177 alternate
  strictly every other iteration.
- **Their mesh:** they sit in prism-layer cells 2–4, not at the wall. Non-hex
  cells, neighbour volume jumps and moderate non-orthogonality (20–55°) are
  strongly over-represented.
- **Where:** at the side mirror and A-pillar, the front wheel's lower front and
  ground contact, and the nose and lower lip.
- **Suggested next experiments:** limited non-orthogonal correction (D4) or
  limited k/omega gradients (D5) as one-change restarts from C0's iteration 500.
  Mesh fixes to the wheel layers, the mirror refinement or the contact patch are
  the costlier alternative.

## Results: D4 and its R1 control (2026-11-09)

Both runs restart from C0's iteration 500 and compare iterations 501–1000.
Details are in docs/CONTINUATION.md.

- **R1** uses the production schemes. It departs from C0 at iteration 501 (the
  restart is not exact), but its clipping counts match C0's (k 610 / omega 90
  cells per iteration).
- **D4** (`limited corrected 0.33` for the laplacian and snGrad terms) clips k
  in every iteration, like R1. Its counts are 607.5 / 90.8 against R1's
  609.9 / 89.5, in the same regions and the same worst cells.
- **Size of the differences:** D4's residual and screening-force differences
  from R1 are within the restart perturbation.
- **Cost:** D4 is 8% slower per iteration.
- **Verdict:** non-orthogonal correction is not the driver of the clipping.
  The remaining candidates are D5 (limited k/omega gradients) and mesh fixes
  at the recurring cells.

## Plan: mesh fix (M1), then D5 (user's choice, 2026-11-09)

Written before any candidate mesh was built.

**M1: layer controls only.** Remesh serially, like Stage 2. The surfaces,
refinement, 5 layers, expansion 1.2, final thickness 0.5, minThickness 0.1 and
the quality limits are all unchanged. Only `addLayersControls` lines change
(`scripts/cfd/wp9-mesh-fix.sh`):

- **A (smoothing):** nSmoothSurfaceNormals 1 → 3, nSmoothNormals 3 → 5,
  nSmoothThickness 10 → 20, and a new nSmoothDisplacement 90.
  - Why: the Stage 2 snappy log shows 35,131 nodes thinned by the
    medial-axis limit, and layer displacement scaled back to as low as 0.32
    to fix bad faces.
  - The persistently clipped k cells sit in layers 2–4 and are much rougher
    than near-wall cells in general:
    - volume jump ≥ 1.5: 81% vs 22%;
    - skewness ≥ 0.5: 40% vs 2.7%;
    - non-orthogonality ≥ 20°: 64% vs 9%.
- **B:** A plus nBufferCellsNoExtrude 0 → 1, for graded layer terminations.
- **C:** A plus nOuterIter 5, which adds the layers in five passes.

All keywords were checked in the installed v2606 source and annotated
dictionary.

**Scoring:** `wp9MeshMetricDict` runs on each mesh and on the Stage 2 baseline.
- It counts cells within six layers of the vehicle walls, and "rough" cells
  among them: volume jump ≥ 1.5 together with skewness ≥ 0.5 or
  non-orthogonality ≥ 20°.
- Counts are taken over all those cells, over layers 2–4, and inside three
  boxes around the clusters of persistently clipped cells:
  - front wheel: x 0.45–0.65, y 0.65–0.90, z 0–0.25;
  - nose: x −0.05–0.12, y 0–0.45, z 0.15–0.70;
  - mirror/A-pillar: x 1.38–1.95, y 0.68–1.10, z 0.88–1.15.

**Selection (fixed before results):**
1. A candidate is eligible only if:
   - snappy finishes without errors;
   - checkMesh reports `Mesh OK.`;
   - it has at least 12.0M cells (the protocol floor);
   - body layer coverage (thickness %) is at least the baseline's 89.2% minus
     2 points.
2. Among eligible candidates, the one with the fewest rough cells summed over
   the three boxes wins. Ties go to the fewest rough cells in layers 2–4.
3. The winner is solved only if that box total is at least 20% below the
   baseline's. Otherwise there is no M1 solve, and the result is reported.

**M1 solve:** production numerics from 0 to 1000 on the winning mesh,
initialised as Stage 2 was (setup `0/`, potentialFoam, decomposePar 16).
It is compared with C0 over 501–1000 and 751–1000:
- clip counts and regions;
- residual RMS;
- screening forces;
- whether the hotspot cells stop clipping, which the clip maps will show.

The cells are new, so the comparison is by region and box, not by cell index.

**D5 after M1:** `grad(k)` and `grad(omega)` set to
`cellLimited Gauss linear 1`, restarted from C0's iteration 500, 501–1000,
compared with R1. It runs on the Stage 2 mesh, independent of M1's outcome.

### Result: no candidate qualifies, so no M1 solve (2026-11-09)

Outputs are in `scratch/wp9-diagnostics/mesh-fix-20261109/{baseline,a,b,c}`.
Rough cells in the hotspot boxes (front wheel / nose / mirror), with the
eligibility checks:

| | Baseline | A | B | C |
|---|---|---|---|---|
| snappy | — | no errors | no errors | **125 illegal faces** |
| checkMesh | OK | OK | OK | OK |
| Cells | 12,056,197 | 12,056,537 | **11,931,951** | 12,094,063 |
| Body coverage | 89.2% | 88% | 88% | 91.2% |
| Wheel coverage (fr/rr) | 66.2 / 69.1% | 62.7 / 65.4% | 63.3 / 65.9% | 51.2 / 58.3% |
| Boxes | 5,697 / 8,298 / 5,460 | 4,406 / 7,751 / 5,188 | 4,467 / 8,901 / 5,502 | 3,921 / 7,793 / 5,412 |
| **Box total** (need ≤ 15,564) | 19,455 | 17,345 (−10.8%) | 18,870 (−3.0%) | 17,126 (−12.0%) |
| Rough, layers 2–4 | 142,405 | 126,451 (−11.2%) | 155,745 (+9.4%) | 122,996 (−13.6%) |
| nonOrtho ≥ 40, layers 2–4 | 24,005 | 12,642 | 15,595 | 25,777 |
| snappy wall time, peak RSS | 2,402 s | 3,680 s, 20.6 GB | 3,712 s, 20.6 GB | 26,980 s, 21.5 GB |

- **B** is ineligible (below 12.0M cells). **C** is ineligible (snappy reported
  illegal faces). Its score was computed afterwards for information only, in
  `c-info-score`.
- **A** is the only eligible candidate. It reduces the box total by 10.8%,
  short of the required 20%. So, by rule 3, there is no M1 solve.
- No candidate would have qualified even ignoring eligibility: the best box
  reduction is C's 12.0%.
- Observation (not a selection criterion): A halves the severely
  non-orthogonal cells (≥ 40°) in layers 2–4. The hotspot boxes barely move,
  so the roughness there does not come mainly from layer smoothing settings.
- Conclusion: layer-control smoothing alone does not remove the rough cells
  at the clipping hotspots. Changing them would need changes beyond
  `addLayersControls` (surface refinement or layer count/thickness at those
  features), which are protocol mesh changes, not a diagnostic tweak.

### Result: D5 removes most clipping (2026-11-09)

Run `wp9-diag-d5-20261110-a`:
- Completed with exit 0, 21:10Z–23:52Z.
- Source fingerprint unchanged; no swap (minimum available memory 2.85 GB);
  no processes left after cleanup.
- fvSchemes FNV d9febec1b6f9dad9. The case gradSchemes contain the
  `grad(k)` and `grad(omega)` `cellLimited Gauss linear 1` lines.
- 500 iterations, solver `End`. The 6 warnings are the absent-patch warnings
  of the half model, the same count as R1.

D5 vs R1 (the restart control), 501–1000:

| | R1 | D5 |
|---|---|---|
| k: iterations with clipping | 500/500 | 500/500 |
| k: mean clipped cells | 609.9 | **43.0 (−93%)** |
| k: 901–1000 | 604.0 | 41.6 |
| k: lowest value | −1.49 | −0.156 |
| k: clipped volume (m³) | 0.0507 | 0.00022 |
| k by region (wheel_fr / body_near / underbody / wake / other) | 137 / 223 / 157 / 22 / 48 | 3.5 / 3.2 / 35.2 / 0.4 / 0.6 |
| omega: mean clipped cells | 89.5 | **4.2 (−95%)** |
| omega: lowest value | −7.3e5 | −7.7e3 |
| Residual RMS U / p | 0.0207 / 1.69e-3 | 0.0210 / 1.71e-3 |
| Residual RMS k / omega | 2.28e-4 / 5.95e-5 | 2.52e-4 / 7.09e-5 |
| Screening Cd 501–1000 | 0.2764 ± 0.0099 | 0.2759 ± 0.0087 |
| Screening Cd 751–1000 | 0.2758 ± 0.0106 | 0.2746 ± 0.0091 |
| s/it | 17.48 | 18.62 (+6.5%) |

- **The clipping comes from the unlimited k/omega gradients.** Limiting them
  removes nearly all clipping at the front-wheel, nose and mirror hotspots
  (from about 360 cells per iteration to about 7). This is far larger than the
  restart perturbation (R1 vs C0) or D4.
- **Not a cure.** k is still clipped in every iteration, by about 35 cells
  in the underbody band (wall band 2).
- **Forces still oscillate** with the same spread (Cd SD 0.009–0.010). The
  k/omega residual RMS is 10–19% higher than R1's. So D5 by itself does not
  show that the force oscillation comes from the clipping.
- **This is a diagnostic, not acceptance evidence.** Under decision 2
  below, adopting it in production needs its own change (test first in
  `fv_schemes`) and a new protocol run with the intended schemes from
  iteration 0. Stage 3 stays blocked.

### Investigation after D5 (user: "investigate, adopt and rerun", 2026-11-10)

This uses existing histories only: Stage 2 (iterations 1–2546), C0, R1 and D5.

**Force oscillation:**
- **Body pressure drag.** It carries 89–100% of the Cd variance in every
  run. Body viscous drag and the wheels contribute almost nothing; the front
  wheel's pressure drag contributes up to 12%.
- **Stationary.** Over Stage 2's 501–2546, in 250-iteration windows, the
  mean stays at 0.273–0.279 and the SD at 0.0073–0.0114. There is no decay
  and no growth.
- **Quasi-periodic.** The autocorrelation is 0.84 at lag 5 and about 0 at
  lag 20. The spectrum is broadband, with peaks at periods of 50–55 (C0, R1)
  and 90 iterations (Stage 2 late). This is the usual limit cycle of a steady
  SIMPLE solution around a separated wake. It is not a numerical blow-up.
- **D5 does not change it.** Cd SD is 0.0087 vs R1's 0.0099, and the same
  body-pressure share.

**Stage 2's last 2000 iterations, scored with the `iteration_gate` formulas**
(window 1000; windows 547–1546 and 1547–2546):

| Check | Value | Limit |
|---|---|---|
| Cd window-to-window change | 0.61% | 0.2% |
| Cd drift | 1.76% and 0.17% | 0.1% |
| Residual RMS, p | 1.6e-3 | 1e-5 |
| Residual RMS, Ux / Uy / Uz | 7.5e-4 / 8.8e-3 / 2.0e-2 | 1e-5 |
| Residual RMS, k | 2.2e-4 | 1e-5 |
| Residual RMS, omega | 4.1e-4 | 1e-5 |

D5's residual RMS is of the same size (U 2.1e-2, p 1.7e-3, k 2.5e-4,
omega 7.1e-5). The residual gate is 2–3 orders of magnitude away, and D5
does not close that gap. **Expected outcome of the rerun: "not converged".**
It is still the first protocol history with the intended schemes from
iteration 0, and the first to reach 7000 iterations.

**Remaining clipped cells (D5 worst-cell records, 751–1000):**
- the underside of the nose's front edge (x 0–0.06, y 0.09–0.45,
  z ≈ 0.20–0.21, wall distance about 3–8 mm);
- the front wheel's lower front (x ≈ 0.53, z 0.14–0.16).

**Adopted.** `fv_schemes` (the steady recipe) now limits `grad(k)` and
`grad(omega)` with `cellLimited Gauss linear 1`. The test
`case::tests::steady_recipe_limits_the_velocity_and_turbulence_gradients`
was observed failing first. The DDES recipe is unchanged. The diagnostic
harness still uses Stage 2's own copied fvSchemes, so C0, R1 and D5 stay
reproducible.

1. If D1 removes the startup blow-up and clipping stays rare after the switch
   back to `limitedLinear`: implement the specified startup in `fv_schemes`
   (production recipe, test first), then plan a new full protocol run.
2. If clipping returns after the switch, or only D2 helps: the relaxation stays
   a diagnostic. A production change needs its own justification, and the
   final window has to run with the intended schemes.
3. If tighter solves lower the next-iteration initial residuals, cost each one
   in seconds per iteration before adopting it.
4. If clipping concentrates in specific cells or regions, investigate those
   (mesh, layers, wall treatment) before remeshing anything.
5. If the numerics are stable and forces still oscillate, investigate the
   steady-flow assumption. A transient calculation needs its own time-step and
   statistical-convergence validation and cannot satisfy this steady gate.

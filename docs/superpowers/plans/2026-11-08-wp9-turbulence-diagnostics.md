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

## Decisions after the screen

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

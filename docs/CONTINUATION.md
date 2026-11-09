# Vernon durable continuation

Read AGENTS.md, CLEAN_ROOM.md and docs/HANDOFF.md before editing code.
This is the current recovery entry point for scheduled/manual continuation.
Sections are newest first. Older sections use the original machine's paths
(C:\Users\mabdu\REA\...) and the old flat scripts/ layout, and are kept as
evidence. For current commands use docs/SETUP.md and docs/CFD-RUNBOOK.md.

## WP9 mesh fix: no candidate qualifies, no M1 solve; D5 running, 2026-11-09

- **Mesh fix (M1):** three candidates changed only `addLayersControls`
  (`scripts/cfd/wp9-mesh-fix.sh`). They were scored with `wp9MeshMetricDict`
  against the Stage 2 mesh. The selection rules were committed in 2b9c72d
  before any mesh was built.
  - Outputs: `scratch/wp9-diagnostics/mesh-fix-20261109/`.
  - The full table is in
    `docs/superpowers/plans/2026-11-08-wp9-turbulence-diagnostics.md`.
- **Results:**
  - **A** (smoothing) is eligible, but cuts rough cells in the hotspot boxes
    only from 19,455 to 17,345 (−10.8%). It needed 20%.
  - **B** (+ buffer cells) is ineligible: 11.93M cells, below the 12.0M floor.
  - **C** (+ nOuterIter 5) is ineligible: snappy finished with 125 illegal
    faces after 7.5 h. Its informational score was −12.0%.
- **Verdict:** no M1 solve. Layer-control smoothing does not remove the rough
  layer cells at the front wheel, nose and mirror. Remeshing costs about
  1 h and 20.6 GB peak RSS per candidate (C: 7.5 h, 21.5 GB).
- **D5** `wp9-diag-d5-20261110-a`: running since 21:10:52Z, about 2.6 h.
  - Settings: `grad(k)`/`grad(omega)` `cellLimited Gauss linear 1`,
    restarted from C0's 500, compared with R1.
  - Launch note: the launcher reports "Worker … has not published status"
    after 30 s because the worker is still scanning `target/`. The status
    appeared after about 80 s. Use the bundled pwsh 7
    (`E:\REA\tools\pwsh\7.6.9\pwsh.exe`), not Windows PowerShell 5.1.

## WP9 R1 completed: D4 (limited correction) does not reduce clipping, 2026-11-09

- **R1** `wp9-diag-r1-20261109-a`: completed, exit 0, 08:06Z–10:36Z.
  - Linux cleanup confirmed; source unchanged; no swap.
  - fvSchemes FNV 41ad9d1f665715c3, the same as C0.
  - 17.48 s/it.
- **Restart perturbation (R1 vs C0):** the runs diverge from iteration 501.
  - The maximum relative difference is 0.96 for the initial residuals and
    1.0 for the bounding minimum.
  - Clipping statistics are unchanged: k 609.9 vs 610.8, omega 89.5 vs 90.1
    cells per iteration.
  - The omega residual RMS is 5.95e-5 vs C0's 8.75e-5. So the restart alone
    gives D4's lower omega RMS.
  - The extreme omega minimum moves a lot (−7.3e5 vs −9.9e5), so it is not a
    usable comparison metric.
  - Screening Cd moves by +0.0011 (501–1000) and +0.0021 (751–1000).
- **D4 vs R1, the controlled comparison, over 501–1000:**
  - k: clipped in 500/500 iterations in both; 607.5 vs 609.9 cells per
    iteration; 901–1000: 600.1 vs 604.0.
  - omega: 90.8 vs 89.5; 901–1000: 92.4 vs 88.2.
  - Regions and wall bands are within a few cells of each other.
  - Residual RMS: U 0.0208 vs 0.0207; p 1.63e-3 vs 1.69e-3;
    k 2.23e-4 vs 2.28e-4; omega 5.57e-5 vs 5.95e-5.
  - Worst cells at maps 750 and 1000: the top three k cells (4/52986,
    4/640618, 4/693410) are the same in C0, R1 and D4. Overlap of the
    worst-cell sets: D4–R1 0.60 (k) and 0.43 (omega); R1–C0 0.54 and 0.38.
  - Screening Cd: 0.2750 ± 0.0081 vs 0.2764 ± 0.0099 (501–1000), and
    0.2728 vs 0.2758 (751–1000). Both differences are within one SD.
  - Cost: 18.88 vs 17.48 s/it (+8%).
- **Conclusion:**
  - The limited non-orthogonal correction (0.33) does not reduce the k/omega
    clipping. Every difference from R1 is the size of the restart
    perturbation (R1 vs C0) or smaller.
  - Non-orthogonal correction is ruled out as the driver.
  - Stage 3 stays blocked.
  - The next step is the user's choice: D5 (cellLimited gradients for k/omega,
    a restart from C0's 500, compared with R1), mesh fixes for the recurring
    cells (layers 2–4 at the mirror, front wheel and nose), or D2/D3.

## WP9 D4 completed; R1 running, 2026-11-09

- **D4** `wp9-diag-d4-20261109-a`: completed, exit 0, 05:20Z–08:04Z. Linux
  cleanup was confirmed and the source was unchanged (checked by the chain).
  fvSchemes FNV f74960fa38e257d5, `Gauss linear limited corrected 0.33` and
  `limited corrected 0.33`. The solve took 18.88 s/it, against Stage 2's
  16.99 s/it over the same window.
- **Clipping, 501–1000 (D4 vs C0):** k clipped in 500/500 iterations in both.
  - Mean k cells: 607.5 vs 610.8.
  - Mean omega cells: 90.8 vs 90.1.
  - 901–1000: k 600.1 vs 612.0; omega 92.4 vs 87.9.
  - Lowest k: −1.86 vs −1.83. Lowest omega: −2.0e5 vs −9.9e5.
  - Region and wall-band means are within a few cells of C0's.
- **Residual RMS, 501–1000 (D4 vs C0):** U 0.0208 vs 0.0203; p 1.63e-3 vs
  1.65e-3; k 2.23e-4 vs 2.24e-4; omega 5.6e-5 vs 8.8e-5.
- **Screening forces (not results):** Cd 0.2750 ± 0.0081 (501–1000) and
  0.2728 ± 0.0080 (751–1000). C0 had 0.2753 ± 0.0097 and 0.2737 ± 0.0101.
- **Reading so far:** limited non-orthogonal correction did not change the
  clipping. The omega differences may come from the restart itself, which R0
  showed is not exact. R1 (same restart, production schemes) decides that. No
  verdict until R1 is in.
- **R1** `wp9-diag-r1-20261109-a` launched at 08:06Z and reached 672 at
  08:59Z. Expect it at about 10:35Z.

## WP9 D4 running, R1 restart control queued (user chose the control), 2026-11-09

- **R1** (commit 9e27d78): production schemes from C0's iteration 500 for
  501–1000. It is the same restart as D4, so D4 is compared with R1. R1 against
  C0 measures the restart perturbation; that comparison is recorded, not
  asserted.
- **Tests:** crate 370/0, clippy clean, runner self-test 8/8.
- **D4** `wp9-diag-d4-20261109-a` started about 05:20Z. The case's fvSchemes
  shows the two limited lines. There was no compile in the timed solve, and it
  runs at about 18 s per iteration.
- **The chain:** a background script waits for D4's final status. If D4's
  cleanup is confirmed and the source is unchanged, it launches
  `wp9-diag-r1-20261109-a`. Expect D4 at about 08:00Z and R1 at about 10:40Z.
- **If the chain is lost:** launch R1 with VERNON_DIAG_VARIANT=r1 and
  `start-verification.ps1 -Profile wp9-diagnostic -RunId wp9-diag-r1-20261109-a`,
  with the same WSL variables as before.
- Do not edit `scripts/`, `crates/`, `Cargo.*`, `AGENTS.md` or `CLEAN_ROOM.md`
  while these runs are going.

## WP9 D4 prepared; R0 restart check failed its exactness gate, D4 not launched, 2026-11-09

The user chose D4 (limited non-orthogonal correction).

Tooling, commit 751f09f, not pushed:
- New variants `r0` and `d4` in `crates/vernon-nodes-cfd/src/diagnostic.rs`.
  Both restart from C0's written iteration 500. `VERNON_DIAG_START_SOURCE` is
  set by the runner to the C0 case; the mesh is checked to be identical.
- **D4:** 501–1000 with `laplacianSchemes default Gauss linear limited
  corrected 0.33` and `snGradSchemes default limited corrected 0.33`. Nothing
  else in fvSchemes changes, which a unit test checks.
- **R0:** 501–510 with production schemes. It must reproduce C0's log within
  1e-6, so that D4's differences can be attributed to its schemes.
- **Tests:** unit tests 9/0; crate 370 passed, 0 failed, 17 ignored; clippy
  clean; C0 gate tests 22/0.
- **Runner self-test:** passes, 8 durable scenarios, twice in a row. Before that
  it failed three times, at different places: twice when reading status.json
  while the worker replaced it atomically (a sharing violation, once in the
  launcher itself), and once when a 3 s timeout fired early. The status-reader
  race is fixed with `Read-VerificationJson` (a 5 s retry) in the launcher,
  status, stop and self-test scripts. The early timeout was not changed.
- **AGENTS.md's test count was not updated.** It needs a full workspace run.

R0 `wp9-diag-r0-20261109-a`:
- The runner recorded `failed`, exit 101 (the test's own assertion), cleanup
  confirmed, source unchanged.
- The restart ran iterations 501–510 exactly, and it loaded the precompiled
  counter.
- **It is not bit-exact with C0:**
  - most initial residuals differ by 1e-8 to 5e-6 relative;
  - omega at 508 differs by 1.8e-3;
  - bounding minima differ by up to 2.8e-3 (k at 501: −0.72077 against
    −0.71872).
  - The gate is 1e-6.
- **Likely cause, inferred from the code and not proven:** the
  nutUSpaldingWallFunction solves for uTau with Newton iterations, tolerance
  0.01 and at most 10 iterations, starting from the current wall nut. A restart
  evaluates the wall function once more at startup than a continuous run does,
  so wall nut moves within that 1% tolerance. The fixture restart test was exact
  because the fixture does not use this wall function.

Not launched: D4, waiting for the user's decision on its control. Options:
- **Use C0 as the statistical control over 751–1000.** D1 reached C0's per-cell
  clip counts from a completely different history, so these statistics do not
  depend on small differences in state. A 1e-6–3e-3 perturbation at 501 should
  not change them, but this is an argument, not a measurement.
- **Run a production restart from 500 to 1000 (R0 extended) as an exact
  control** alongside D4. This costs about 2.5 h more.

## WP9 location analysis of the clipped cells (user chose it), 2026-11-09

How it was run:
- The tool is `scripts/cfd/wp9-location-analysis.sh` with the read-only coded
  function object `scripts/cfd/wp9-location/wp9LocationDict`.
- It runs postProcess on 16 ranks, in WSL, outside the runner. It is not a
  solve.
- It works on a separate copy, `scratch/wp9-diagnostics/location-20261109-a/case`,
  holding the mesh, D1's fields at 1000 and the clip maps of both runs. The D1
  and C0 cases are untouched. The processor meshes were checked to be
  identical between the two runs, so processor-local cell indices match.
- Total time 525 s, mostly the copy.
- **Outputs:** `postProcessing/wp9Location/summary.txt` (histograms),
  `cells_proc*.tsv` (persistent and named cells) and `neighbours_proc*.txt`
  (the named cells' faces).
- A second pass added clip-count histograms. Its other sections are identical
  to the first pass, which is kept as `wp9Location.run1`.

How many cells, over iterations 751–1000:
- **k:** D1 clipped 2,765 distinct cells and C0 2,674. 1,909 are common; the
  union is 3,530.
- **omega:** 938 and 925, with 568 common.
- **Persistent:** 154 k cells and 17 omega cells were clipped in all 250
  iterations in both runs.
- **Every other iteration:** 177 k cells (6.4%) were clipped exactly 125 times,
  a strict alternation (period 2), with the same per-cell counts in both runs.
  The forces oscillate with a different, slow period.
- These are 0.02% of the 12.06M cells.

What the clipped cells look like (persistent k; reference values are all cells
within 5 cm of a wall):
- **Inside the prism-layer stack, but not at the wall:** wall-layer index 2–4
  for 91% (omega: index 2–3 for 94%). They are almost never wall-adjacent, so
  the wall-function cells themselves are not where clipping happens. The median
  wall distance is 7.9 mm (omega 3.4 mm). The layer settings are 5 layers,
  expansion 1.2, final layer 0.5 of the local cell size.
- **Non-hex cells are over-represented:** 40% are not hexahedra, against 5%
  near walls. For omega it is 62% of the omega cells clipped at least once in
  751–1000 (the 938 D1 cells). 4- and 5-face cells are 18–28% against 1.4%.
- **Volume jumps to a neighbour:**
  - jumps of 1.5× or more: 81% against 22%;
  - jumps of 6× or more, which mark refinement-level interfaces, among cells
    clipped at least once: 28% of the 2,765 k cells.
- **Moderate non-orthogonality, never extreme:** 20–40° for 51–54% of the
  clipped cells against 8%. The maximum is 64°, and none is over 65°.
- **Skewness of 0.5 or more:** 40% against 3%. The face-weight minimum is lower
  than near-wall cells in general.
- **Wall-adjacent cells clipped at least once:** they lean towards the buffer
  layer, y+ 1–11, at 6.1% of clipped cells against 1.0% of near-wall cells.

Where the 154 persistent k cells are (0.2 m clusters):
- **Side mirror / A-pillar, about x 1.8, y 0.8–1.0, z 1.0:** 49 cells.
  - It includes processor 4 cell 52986, a level-4 cell 6.7 cm from the wall
    with a 7.7× volume jump at a refinement interface. It was clipped in all
    250 iterations in both runs.
- **Front wheel, lower front and ground contact, x about 0.6, z 0–0.2:** 37
  cells. The contact cells at z 0.012–0.014 are 4-face cells with
  non-orthogonality of 48–55° and volume jumps of 3.7–7.4×.
- **Nose, lower lip and front face, x about 0:** about 57 cells. 12 of the 17
  persistent omega cells are here.
- **Underbody floor:** for example processor 6 cell 7463, wall-adjacent with
  y+ 17.

Mesh context, from the Stage 2 mesh build log:
- **Layer coverage** averaged 4.64 of 5 layers on the body (89% of the
  thickness). On wheel_fr it was 3.07 (66%) and on wheel_rr 3.26 (69%).
- 97.3% of wall faces were extruded.
- checkMesh: OK, non-orthogonality maximum 65.0° (average 5.8), skewness
  maximum 3.45.

Production numerics relevant to these cells:
- `laplacianSchemes Gauss linear corrected` and `snGradSchemes corrected`:
  full explicit non-orthogonal correction.
- `gradSchemes default Gauss linear`: k and omega gradients are unlimited.
- `div(phi,k|omega) bounded Gauss limitedLinear 1`.

Reading:
- The persistent clipping is a local discretisation defect at a fixed set of
  cells: non-hex and transition cells inside the layer stack, at three
  geometric features.
- It is identical in C0 and D1. Part of it is a strict two-iteration limit
  cycle, which points to numerics, not to physics.
- The force oscillation is a separate, slower behaviour that this analysis does
  not explain.

Candidate experiments, each one change, a diagnostic restart from C0's
iteration 500 (the user decides):
- **D4:** `limited corrected 0.33` for the laplacian and snGrad terms, the usual
  treatment for meshes with non-orthogonality near 65°.
- **D5:** `cellLimited Gauss linear 1` for grad(k) and grad(omega).
- **Mesh changes**, costlier because they need a re-mesh and a new Stage 1:
  - layer coverage on the wheels;
  - mirror-area refinement;
  - the treatment of the wheel contact patch.
- **D2 and D3** remain open, but neither targets these cells.

Any production change needs its own justification and an accuracy assessment
before it is used for acceptance.

## WP9 turbulence diagnostics: C0 and D1 done, upwind startup does not fix clipping, 2026-11-09

Both runs completed under the runner with exit 0, cleanup confirmed and source
fingerprint c263f467…8779 unchanged. The diagnostics are not acceptance evidence
and have no gate verdict. Forces below are screening numbers only.

**C0** `wp9-diag-c0-20261108-a`: production numerics, iterations 0–1000.
- It passed the C0 gate (`WP9_C0_GATE pass`).
- It reproduces the Stage 2 log exactly over 1–1000: 6,000 initial residuals and
  2,000 bounding minima, maximum relative difference 0.0, nothing missing.
- 17.05 s per iteration against Stage 2's 16.83. No swap, at least 3.1 GB free.

**D1** `wp9-diag-d1-20261109-a`: upwind k/omega for 1–500, then a restart with
`limitedLinear 1` for 501–1000.
- 17.17 and then 17.49 s per iteration.
- No swap, no probe loss, at least 2.9 GB free.

Clipping, mean cells per iteration (every clipped cell was negative):

| | C0 1–100 | C0 501–1000 | D1 1–500 | D1 501–600 | D1 901–1000 | D1 501–1000 |
|---|---|---|---|---|---|---|
| k | 2,808 | 611 | 0 (min +2.5e-6) | 501 | 609 | 586 |
| omega | 672 | 90 | 0 (min +1.05) | 87 | 87 | 88 |

- **Frequency:** C0 clipped k and omega in every iteration. D1 clipped in none
  of 1–500, then in every one of 501–1000.
- **Where clipped k cells are, 501–1000, C0 / D1:**
  - near the body 222 / 218;
  - underbody band 158 / 156;
  - front wheel 137 / 124;
  - rear wheel 6 / 5.
  - **By wall distance:** 2 mm–1 cm holds 337 / 326, under 2 mm 67 / 67.
- **Clipped omega cells, both runs:** front wheel about 21, rear wheel 17,
  underbody 35, near the body 15. By wall distance, about 66 are 2 mm–1 cm from
  a wall and 20 are under 2 mm.
- **The same cells recur in both runs.** Cell indices are processor-local, and
  both runs use the same decomposition.
  - k: processor 4 cell 52986 (near the body, 6.7 cm from a wall, the worst
    cell at 501 and 1000 in D1); processor 3 cell 13176 (front wheel).
  - omega: processor 3 cells 13176, 14639, 14858 (front wheel, about 1 mm from
    a wall).
  - Worst-cell lists are in `postProcessing/wp9ClipCounter/clip_*_worst.dat` in
    each run's `temp/.../case`.
- **Residual RMS, 501–1000, C0 / D1:**
  - U 0.0203 / 0.0201;
  - p 1.65e-3 / 1.58e-3;
  - k 2.24e-4 / 2.18e-4;
  - omega 8.8e-5 / 4.3e-4, raised by D1's 501–600 transient; 901–1000 is
    1.5e-4 / 5.6e-5.
- **Screening forces, C0 / D1:**
  - Cd over 501–1000: 0.2753 ± 0.0097 / 0.2763 ± 0.0090 (one standard deviation).
  - Cd over 751–1000: 0.2737 ± 0.0101 / 0.2741 ± 0.0094.
  - Cl over 501–1000: 0.018 ± 0.028 / 0.001 ± 0.027.

Reading:
- The missing upwind startup is a real deviation from the spec. It is not the
  cause of the persistent clipping: under `limitedLinear 1`, clipping returns
  within one iteration from an unclipped upwind state. It reaches C0's count,
  in the same regions and the same cells, within about 100 iterations for omega
  and about 400 for k.
- Implementing the startup in `fv_schemes` alone would therefore not make
  Stage 2 pass.

Next choices (the user decides):
- the location analysis of the recurring cells: mesh quality, layer structure,
  y+ and the wall functions there;
- D2, stronger turbulence relaxation;
- D3, relTol 0.01.

Stage 3 stays blocked.

Run cases are kept in each run's `temp/` until cleanup is decided, about 9.6 GB
each. `docs/superpowers/plans/2026-11-09-domino-cfd-warm-start.md` is an
untracked file that this work did not create; it was left alone.

## WP9 turbulence diagnostics: harness proven, C0 running, D1 queued, 2026-11-08

What exists: profile `wp9-diagnostic` (variants `smoke`, `c0`, `d1`), module
`crates/vernon-nodes-cfd/src/diagnostic.rs`, fixture env
`scripts/cfd/wp9-diagnostic-fixture-env.sh`, gate
`scripts/cfd/wp9-diagnostic-c0-gate.ps1`. Commits 6d50694, 29080d7, 9cce32d and
91d4789 on `claude/wp9-solve-stages`, not pushed. The plan and its revision are in
[docs/superpowers/plans/2026-11-08-wp9-turbulence-diagnostics.md](superpowers/plans/2026-11-08-wp9-turbulence-diagnostics.md).

Proven before the large case:
- **Fixture tests:** the three `diagnostic::tests::fixture_*` tests, run with
  `VERNON_DIAG_FIXTURE_ENV` set, pass. Logs are in `scratch/wp9-diagnostics/`.
  - Injected values give 104 negative and 120 below-floor cells, with exact
    volumes, regions, wall bands and worst-cell locations, in serial and on 4
    ranks.
  - The floor is 1e-15 from the model, and the counter minimum equals
    OpenFOAM's bounding minimum.
  - Fields are byte-identical with and without the counter.
  - A restart is exact. A switched run equals pure upwind through the switch
    and differs after it.
- **Other checks:**
  - Unit tests pass 7/0, crate tests 368/0, clippy clean.
  - Fixture policy 21/0, C0 gate tests 22/0.
  - Runner self-test passes, including its 8 durable scenarios. A race in its
    `Wait-State` helper was fixed.
- **Large-case smoke:** `wp9-diag-smoke-20261108-a` passed its test, but the
  runner rejected the marker, which shared cargo's line; that is fixed.
  `wp9-diag-smoke-20261108-b` completed with exit 0 and cleanup confirmed.
  - Both phases loaded the precompiled counter, with no compile in a timed
    solve.
  - Upwind iterations 1–2 clipped 0 k cells. Production iterations 3–4 clipped
    1,756 and then 4,475, mostly within 1 cm of walls, in the underbody band,
    at the wheels and near the body. This is four iterations only.
  - The counter's own work is 3–6 ms per field per iteration.
  - The one memory-probe timeout happened during the case copy, not while the
    solver ran.

Running: `wp9-diag-c0-20261108-a` (production numerics, iterations 0–1000),
started about 15:45Z from 91d4789. A background chain then runs the C0 gate.
If the gate passes, it launches `wp9-diag-d1-20261109-a`: upwind 1–500, then a
restart with production for 501–1000.

The gate requires:
- completed, exit 0, cleanup confirmed and unchanged source;
- C0 reproducing the Stage 2 log over 1–1000, initial residuals and bounding
  minima within 1e-6;
- seconds per iteration within 10% of Stage 2;
- no swap, at least 1 GiB available, and no probe loss while the solver runs.

If the chain is lost, run the gate by hand:
`pwsh scripts/cfd/wp9-diagnostic-c0-gate.ps1 -RunId wp9-diag-c0-20261108-a`.
If it passes, launch with VERNON_WSL_DISTRO, VERNON_WSL_USER, VERNON_WSL_ENV_FILE
(the solve env) and `VERNON_DIAG_VARIANT=d1` set, using
`start-verification.ps1 -Profile wp9-diagnostic -RunId <new id>`.

Do not edit `scripts/`, `crates/`, `Cargo.*`, `AGENTS.md` or `CLEAN_ROOM.md`
while these runs are going: that marks the run `source-changed`.

## WP9 Stage 2 stopped for turbulence diagnostics (user decision), 2026-11-08

Run `wp9-solve-fastback-20261108-a` was **stopped by the user's decision** at
iteration 2546 of 7000. Status `cancelled`, exit 130, finished 14:42:02Z, Linux
cleanup success (attempted 1, marker `VERNON_WSL_GROUP_STOPPED`), no OpenFOAM
process left. **There is no gate verdict.** It is not a converged or a failed
protocol run. It is a cancelled one and cannot be resumed or certified.

What the history showed when it was stopped:

- k and omega were clipped (`bounding k`, `bounding omega`) in **every**
  iteration. The mean per-iteration k minimum was about -0.7 m²/s² (worst
  -2.5), with k maximum 36–50 against a freestream 0.0024. Omega minima stayed
  around -6e3 to -2e4, and omega reached 4.5e22 in iterations 1–200.
- RMS initial residuals over iterations 1546–2545: p 1.58e-3, U 2.03e-2,
  k 2.18e-4, omega 4.14e-4. They had been flat since about iteration 400, and
  the gate is 1e-5.
- Cd (screening only): 0.2736 over 1046–1545 and 0.2760 over 1546–2546, a
  change of 0.85% (gate 0.2%), sd 0.0080.
- y+ at 2000: body 0.32–802 (mean 24), ground 0.36–1795, wheels 0.97–423.
  Spalding nut, omega and kqR wall functions.
- Throughput: 17.0 s per iteration (pilot 18.7). Rank peak 1.64 GiB, solver
  total RSS peak 22.4 GB, WSL MemAvailable min 3.9 GiB, no swap, run folder
  peak 10.8 GB.
- **Recipe deviation:** `docs/specs/automotive-cfd.md` specifies upwind k/omega
  for the first 500 iterations. `fv_schemes` (`crates/vernon-nodes-cfd/src/foam.rs`)
  writes `limitedLinear 1` from iteration 1. The pilot and Stage 2 both lacked
  the specified startup.

Kept for diagnostics: the decomposed 12M case with times 0, 1000 and 2000 in
`scratch\continuation\runs\wp9-solve-fastback-20261108-a\temp\vernon-cfd-98196-cfd.solve_steady-8\native\case\`,
plus logs, `memory-summary.json` and `status.json`. The pilot's case is also
kept, under its own run folder.

Environment change, approved by the user: `openfoam2606-dev` 2606.0~rc2-1
(with `-tools` and `-source`, 79 packages, 64 MB) is installed in
`Vernon-Ubuntu-24.04` so that coded fvOptions compile. The v2606 source confirms
`solve → fvOptions.correct → bound` for both k and omega (`kOmegaSSTBase.C`
593–595 and 618–620), so a `codeCorrect` hook sees fields before clipping.

Next: the diagnostic screen in
[docs/superpowers/plans/2026-11-08-wp9-turbulence-diagnostics.md](superpowers/plans/2026-11-08-wp9-turbulence-diagnostics.md).
Stage 3 stays blocked. Another full protocol run waits until the screen shows
stable k/omega with the intended production numerics.

## WP9 Stage 2 launched (user approved "start stage 2"), 2026-11-08

Run `wp9-solve-fastback-20261108-a`, profile `wp9-solve` (Fastback, dx0 0.32,
serial mesh, 16 ranks, 7000 iterations, window 1000; mesh limit 100 min, solver
2700 min, outer 172800 s). Started 2026-11-08T01:51:04Z from commit 160b280,
clean tree, sourceBefore 1f8d03eb…a6f6 (same as the pilot). Launched as is: no
automatic resume. Expected end ~2026-11-09 15:00–17:00 UTC (37–39 h); outer bound
expires 2026-11-10 01:51 UTC.
Pre-flight: on AC; Windows idle sleep and hibernate on AC are "never" (powercfg
read only); host free 20.7 GB; ChatGPT, Codex and the AI-3D download closed; WSL
freshly started (MemAvailable 24.6 GiB); E: 611 GB free.
To check: `verification-status.ps1 -RunId wp9-solve-fastback-20261108-a` (shows
the memory line). To stop: `stop-verification.ps1 -RunId … -Reason …`. If the
worker dies without a final status, run the Linux cleanup in the runbook first.
When it ends: verify from the artifacts (marker, replication-solve.json, solver
log End and iteration count, force/convergence/benchmark reports), report Cd vs
0.275 with the 5% band, convergence pass/fail and residuals exactly as the gates
say; "not converged" is reported as such. Stage 3 only if Stage 2 passes its gates.

## WP9 Stage 1 pilot measured; Stage 2 awaiting the user's approval, 2026-11-04

Run `wp9-solve-pilot-20261104-a` (profile wp9-solve-pilot, commit b2a5a31, clean
tree): status completed, exit 0, step wp9-solve passed; runner accepted the marker
`WP9_SOLVE_PILOT_COMPLETE rear=Fastback cells=12056197 iterations=200 np=16 mesh_np=1
seconds_per_iteration=18.65829145728643 coefficients_reported=0` and the saved row
(replication-solve.json). sourceBefore = sourceAfter = 1f8d03eb…a6f6. 14:36:21 ->
16:38:05 UTC (2 h 02 min). linuxCleanup success, attempted 0. Diagnostic only: no
Cd, no convergence verdict, nothing for the benchmark or study.

Measured (from the case logs and memory-samples.jsonl, 92 samples, ~80 s apart):
- Mesh rebuilt identically: 12,056,197 cells, non-ortho 64.996, skew 3.452,
  layers 97.33%. mesh_build wall 2917 s (48.6 min; 41 min on 11-04 sizing run).
  snappyHexMesh peak HWM 19.8 GiB; WSL MemAvailable fell to 6.0 GiB, host free 0.6 GB.
- potentialFoam 72 s wall, ≥2.0 GiB (one sample). decomposePar ~112 s (log mtimes),
  peak 5.9 GiB. reconstructPar ~134 s, peak 7.1 GiB. (Neither utility prints ClockTime.)
- simpleFoam, 16 ranks: 200 iterations, clean End, 3737 s clock. 18.66 s/it mean
  after the first; per 25-iteration blocks 17.2, 18.6, 18.7, 18.8, 18.9, 18.8, 18.8,
  19.5 (last block includes the 1 GB write). GAMG p iterations flat (2.3 -> 2.0), so
  the drift is not solver work. The plan's linear-scaling guess was 14.3 s/it.
- Solver memory flat, no growth: total RSS 17.9–20.9 GiB, peak rank HWM 1.49 GiB
  (cap 4 GiB address space), sum of rank HWMs 22.4 GiB. WSL MemAvailable min 4.0 GiB;
  host free 3.4–6.3 GB during the solve. No swap used.
- Disk: one write = 1.01 GB decomposed (processor*/200) or 0.99 GB reconstructed;
  polyMesh 1.5 GB; solved case 6.3 GB; run folder peak 10.8 GB. E: 909 GB free.
- Residuals (initial) at 1/51/101/151/200: p 1, 7.1e-3, 4.2e-3, 3.3e-3, 2.5e-3;
  U 0.27, 0.034, 0.028, 0.018, 0.020; k 1, 8.8e-4, 4.6e-4, 3.3e-4, 2.8e-4; omega
  3.6e-3, 1.9e-4, 1.1e-4, 8.7e-5, 7.6e-5. U is flattening near 0.02 (the old
  3.76M run sat at 0.018 after 3000 iterations): "not converged" is a real risk.
- Log warnings: 6 FOAM warnings that monitor patches wheel_fl/wheel_rl don't exist
  (half model keeps only the right wheels); pre-existing recipe, forces unaffected.

Stage 2 estimate from these numbers: solver 7000 x 18.7–19.5 s = 36.4–37.9 h; plus
mesh 0.8 h and pre/post ~0.1 h => about 37–39 h wall (above the runbook's 30–36 h).
Solver node limit 2700 min (45 h) is hit only if the mean exceeds 23.1 s/it (+23%).
Memory fits (flat ~21 GiB of 25.4 GiB) but leaves only ~4 GiB in WSL and a few GB
on the host: close large apps for the run. Disk ~15 GB. No automatic resume.

Next: Stage 2 (`wp9-solve`) only after the user approves. Command in the runbook.
**2026-11-04: the user deferred Stage 2 until Sunday 2026-11-08.** Not approved yet:
ask again before launching. Open choice from the gate: start as is, or add
automatic resume support first. Before launch: close large apps, keep the machine
plugged in with the lid open, request keep-awake, and use a fresh run id
(e.g. `wp9-solve-fastback-20261108-a`).

## WP9 Stage 0 built (solve profiles, pilot mode, sampler, stop), 2026-11-04

The user asked to execute CFD validation Stages 0–3 per docs/CFD-RUNBOOK.md, stopping
for approval after the Stage 1 pilot. Branch `claude/wp9-solve-stages` (from
`claude/ai-models` dd89e58), in E:\REA\vernon. Owner for this work: Claude session
(the user's request overrides the "Codex owns WP9" note for these stages).

Built (each RED then GREEN; logs in `scratch/wp9-stage0/`, not committed):
- Harness `real_openfoam_road_car_run`: `RoadCarMode` separates mesh np
  (`VERNON_CAR_MESH_NP`, default 1) from solve np, adds pilot mode and per-node
  time limits; pilot row has no Cd/convergence/benchmark keys; protocol prints
  `WP9_REPLICATION_SOLVE_COMPLETE` after readback; `solve_diagnostics` (log
  timing, disk) and `residual_trend` in every solve row. New unit tests:
  `tests::road_car_run_mode_keeps_the_pilot_apart_from_the_protocol`,
  `tests::solver_log_timing_reads_iterations_clock_and_a_clean_end`.
- `scripts/cfd/wp9-replication-solve-env.sh` + `test-wp9-solve-policy.sh` (40/0)
  + `test-wp9-solve-environment.sh` (live in WSL: 17/0; the sizing env fails it 11/6).
  Live 16-rank probe through the real env file (box case, 256k cells): 30
  iterations, End, exit 0 under 4 GiB RLIMIT_AS; peak rank VmPeak 0.40 GB.
- Runner: profiles `wp9-solve-pilot` (outer 6 h; mesh 100 min, solve 180 min,
  200 iterations) and `wp9-solve` (outer 48 h; mesh 100, solve 2700 min, 7000
  iterations); 16 ranks, serial mesh, Fastback, dx0 0.32; output/row acceptance;
  Linux cleanup on exit; memory sampler (`scripts/cfd/wp9-memory-sampler.ps1`,
  test 18/0); graceful stop `scripts/verification/stop-verification.ps1`
  (status `cancelled`, exit 130, not resumable); `smoke-cancel` profile.
- Fixed a pre-existing runner bug: `Test-VerificationProcess` never matched a live
  worker after a JSON round-trip (PowerShell 7 parses the ISO stamp as DateTime),
  so `verification-status.ps1` showed `running-visibility-unknown` for live runs.
- Runner self-test: library checks PASS; 8 durable scenarios PASS (incl. cancel).
  Sizing cleanup 55 PASS; replication-inputs PASS; sizing policy 27/0; clippy -p
  vernon-nodes-cfd clean.
- Not built: automatic resume from a checkpoint (see runbook "As built").

Environment: i9-13900HX 24C/32T, 31.8 GB, WSL cap 26 GB (25.4 GiB visible), on AC,
power plan "Legion Performance Mode"; WSL 3.0.3, kernel 6.18.54.1; Ubuntu 24.04.5;
OpenFOAM v2606 (openfoam2606 2606.0~rc2-1, build _481094f-20260618); Open MPI
4.1.6; rustc/cargo 1.99.1. E: 978 GB free at start. pwsh 7.6.9 is at
E:\REA\tools\pwsh\7.6.9\pwsh.exe (not on PATH; Windows PowerShell 5.1 can't run the
runner scripts).

## AI parts and aero preview added; OpenFOAM Stages 0–3 DEFERRED, 2026-11-04

The user asked to use the installed open-weight models (E:\AI-3D: PartCrafter,
DoMINO DrivAerML) inside Vernon instead of running the long OpenFOAM stages
now. Ownership: Codex still owns WP9 CFD source; this work touched no
`vernon-nodes-cfd` file, only a new crate plus two registry lines.

Done:
- New crate `crates/vernon-nodes-ai`: `geo.ai_partcrafter`,
  `geo.ai_select_part`, `cfd.ai_aero_preview`, `cfd.ai_aero_compare`. Models
  run as separate processes in the `AI-3D` WSL distribution through bridges in
  `scripts/ai/` (embedded in the crate), with JSON-lines progress, time limit,
  cancel (app cook cancel and `bridge::request_cancel`), GPU/RAM preflight,
  per-run artifact folders with provenance (inputs, seed, settings, weight
  revisions, checkpoint SHA-256, timings, memory).
- DoMINO outputs are dimensionless model units (min–max scaling inverted),
  labelled "AI prediction — CFD validation pending", `cfd_validation =
  DEFERRED`; no force/drag is shown; only `ai_*` metrics, which no gate reads.
  Geometry outside the checkpoint box is refused before inference; out of the
  training span is labelled. Meshes over 20,000 triangles are refused
  (33,540 crashed the model on the 16 GB GPU).
- Spec with contract, licences and measured resources: docs/specs/ai-models.md.
  README, CLEAN_ROOM.md (external programs), CFD-RUNBOOK (deferral banner),
  NODES.md and AGENT_GUIDE.md (201 node types) updated.

Evidence: crate tests 31 passed; the 3 ignored real-model tests were run and
passed (DoMINO surface+volume on stand-in ellipsoids plus a compare; DoMINO
19,800-triangle surface; PartCrafter object, draft, 3 parts, whose
63k-triangle part the aero preview correctly refused). Workspace clippy
`-D warnings` clean. Full workspace: 1763 passed, 0 failed, 38 ignored
(after adding help text the catalogue test asked for); demo cook 0 failing
gates, 5 expected; NODES.md and AGENT_GUIDE.md regenerate unchanged.
No manual UI check was done; the app was not launched.

Remaining / blockers:
- OpenFOAM Stages 0–3 are deferred, not cancelled: still the only validation,
  and needed to measure the surrogate's error (compare on the Stage 2 shape;
  do not tune the surrogate to agree).
- Not built: viewport legend widget (legend is in the summary JSON; colour via
  `Cd`), a resident model service (each run reloads the model: DoMINO ~2.5 min
  end to end for `both`, PartCrafter 73–434 s load).
- PartCrafter `standard` quality and scene mode were not re-measured through
  the node.

## Side branches merged into main, 2026-11-04

The user asked to merge and push the work that was only on side branches.
Merged into main (all were based on 2e01920): claude/agent-client (includes
agent-tools), claude/batch-core, claude/batch-physics, claude/batch-print,
qwen/t01-gcode-dialects, claude/packaging, claude/validation-bench.
Not merged: qwen/t02–t06, which failed their own gates after 6 rounds each;
the same tasks pass in claude/batch-physics and claude/batch-print.
qwen/t07 and t08 had no commits.
Conflicts were limited to README (kept the new README and added the MCP
section), .gitignore (union with dist/) and Cargo.lock (kept main's and let
cargo add only vernon-bench). The packaging scripts and the licence generator
moved to scripts/release/. The merged modules are library code only: no new
nodes (still 194 implemented / 3 white-box). docs/ROADMAP.md maps each module
to the node it should be wired into.
Verified with full queue merged-branches-full-20261104-a (run from
E:\REAernon, the real folder behind the C:\ junction): build, tests
1732 passed / 0 failed / 35 ignored, clippy -D warnings clean, demo 40 nodes
0 failed, catalogue identical to docs/NODES.md.

## Repository organised for a second PC, 2026-11-04

The user asked to connect https://github.com/ssnfts/Vernon.git, organise the
files, document every step in the README and prepare Stages 0–3 for another PC.
- Scripts moved: scripts/setup/ (setup-wp9-{wsl,ubuntu,openfoam}, new
  setup-dev.ps1 and prepare-drivaer-data.ps1), scripts/verification/
  (verification, start-verification, verification-status, test-verification,
  replication-inputs + test), scripts/cfd/ (wp9-replication-sizing-env.sh,
  sizing policy/environment/cleanup tests, wp9-sizing-cleanup.ps1,
  wp9-cfd-smoke.sh). dev-env.ps1, third-party-licenses.ps1 and golden/ stay.
- Machine paths removed from code and scripts. The runner derives the sizing
  environment's WSL path from the checkout (Get-VerificationWp9SizingEnvironmentFile).
  Setup tools live in `<checkout parent>\tools\wsl`, which on this PC is still
  C:\Users\mabdu\REA\tools\wsl. The `mabdu`-only account guards are gone;
  the unelevated checks stay. Ubuntu setup accepts WSL >= 3.0.3. OpenFOAM
  install now also creates the locked-password `vernon` user.
- New docs: docs/SETUP.md, docs/CFD-RUNBOOK.md, docs/README.md (index).
  README rewritten with a step-by-step section. AGENTS.md is portable.
- This PC: the REA folder now physically lives on E:\REA, and
  C:\Users\mabdu\REA\* are junctions (created 2026-11-04 04:20, after the
  sizing runs). The input-lease and cleanup checks refuse junction paths, so
  run everything from E:\REA\vernon. VERNON_WSL_ENV_FILE is then
  /mnt/e/REA/vernon/scripts/cfd/wp9-replication-sizing-env.sh.
- Checks from E:\REA\vernon after the move: runner library PASS; sizing cleanup
  55 checks PASS; replication-inputs PASS; WSL policy 27/0; live environment
  24/0/0 blocked; smoke classifier 8; OpenFOAM setup script bash -n OK;
  setup path resolution OK; prepare-drivaer-data verified all 8 parts (fresh
  extraction tested in a throwaway folder); setup-dev reports ready.

## Next — solver-run plan proposed, 2026-11-04

The user asked for a plan for the first 12M-cell solver run. Plan:
docs/superpowers/plans/2026-11-04-wp9-solver-run.md. Status: proposed, nothing
started, no source changed for it.
Key points: the harness uses one np for mesh and solve (16-rank meshing won't fit).
The 30 h solve timeout is about equal to the ~28 h estimate for 7000 iterations at
16 ranks. There is no solver environment or solve profile yet. Stage 0 is code
only; Stage 1 is a ~2 h diagnostic pilot (200 iterations) before any 30 h+ run.
Awaiting the user's go-ahead for Stage 0.

## Latest result — first PASSING 12M+ sizing row, 2026-11-04

`wp9-replication-serial-sizing-22gib-20261104-a`: status completed, exit 0, step
wp9-sizing passed. Profile output check and row readback accepted. sourceBefore =
sourceAfter = 2593d85e…a2fc, 21:25:08 -> 22:09:40 UTC. Marker
`WP9_REPLICATION_MESH_ONLY_COMPLETE rear=Fastback cells=12056197.0 solver_runs=0`.
Fastback, dx0 0.32, np1, 14M cap, 5 layers: 12,056,197 cells, identical to the
previous run (deterministic). Max non-ortho 64.996, skewness 3.452, 0 failed checks,
layers 97.33%, quality pass. snappy "Finished meshing without any errors" in
2488.4 s, Memory per-node 20719872 (about 20.7GB). solver_ran=false,
physical_validation=false. linuxCleanup success with attempted 0: no leftover
control records. The snappy log has VERNON_WSL_GROUP_EMPTY for its group.

What this does and doesn't establish: the one-grid mesh-only sizing gate for the
fine grid fits on this host with the 26GB WSL / 22GiB cap. It is not a solver
result, convergence, a coefficient comparison or the three-grid study. Those are
the next WP9 steps, and the solver at 12M cells needs its own memory and time
budget check before launch. WP9 remains incomplete and uncommitted; no push.

## Earlier — readback fixed, fresh 22GiB sizing launched, 2026-11-04

The user said "Yes, fix and rerun". Done (Claude session):
- Workspace Cargo.toml: `serde_json = { version = "1", features = ["float_roundtrip"] }`.
  No new crates. The pre-existing Cargo.lock diff is the earlier vernon-io line.
  New test tests::saved_json_rows_read_back_bit_exact (vernon-nodes-cfd) failed RED
  with the exact sizing values (scratch/wp9-json-roundtrip-red-20261104-a.log),
  then passed GREEN (…-green-…). The case.rs reference_json_equivalent tolerance is kept.
- Full queue wp9-float-roundtrip-full-20261104-a FAILED in tests: vernon-nodes-fea
  full_chain_on_the_reference_solver_gates_by_element_order read
  "DISP has no value for node 2126". Cause: a test race that was already there. The test helper
  util::testing::ctx gave every cook of the same node one shared work dir per
  process, so parallel tests overwrote each other's solver results. The test passed
  alone with and without float_roundtrip. Fixed (test-only) with a per-call counter
  in the dir name. FEA lib then passed 128/0/9 three times. The race was never
  reproduced on demand.
- Full queue wp9-float-roundtrip-full-20261104-b PASSED all five steps: build,
  1543 passed/0 failed/35 ignored, clippy, demo (5 expected/0 unexpected),
  catalogue. Source 2593d85e0190109e904418883ce59a3a0d5d5659c50202fc3b33bdd8b6fea2fc.
  App startup was not rechecked; no app or node metadata changed.
- CURRENT WORKER: `wp9-replication-serial-sizing-22gib-20261104-a`, profile
  wp9-sizing-serial, worker PID 556572, 21600s bound, launched from the Claude
  desktop shell. DO NOT LAUNCH ANOTHER COPY. Expect about 45 min. Check
  verification-status, the snappy log, replication-sizing.json, the
  WP9_REPLICATION_MESH_ONLY_COMPLETE marker and linuxCleanup. Same acceptance rules
  as below.

## Previous result — 22GiB serial mesh BUILT 12.06M cells; harness readback failed, 2026-11-04

`wp9-replication-serial-sizing-22gib-20261103-a` finished 18:05:30 UTC with status
failed, exit 101. sourceBefore = sourceAfter = 6b117a12... No worker is active.
The mesh itself completed: snappyHexMesh says "Finished meshing without any errors"
in 2481.9 s. Memory per-node was 20720304 (about 20.7GB, so the old 15.5GiB VM
could never fit it). The test's own measurements: cells 12,056,197 (at or above the
12M half-model floor, under the 14M cap), max non-ortho 64.996, max skewness 3.452,
0 failed checks, layers 97.33%, mesh_quality_passes=true. Mesh-only, solver_ran=false.
The run summary row was written to replication-sizing.json in the run dir. The
runner's linuxCleanup is success with attempted 0. The snappy log ends with
VERNON_WSL_GROUP_EMPTY for its group.

Why it failed: tests.rs:945 `assert_eq!(saved, row)` after writing and re-reading
the JSON. serde_json (workspace `serde_json = "1"`, no `float_roundtrip` feature)
re-parses some f64 values one ulp off: domain.min_m[0] -13.799999999999999 -> -13.8,
first_layer_yplus_estimate ...291 -> ...293. That is a harness readback defect, not
a mesh failure. It is likely the same root cause as the earlier note about tiny
absolute layer heights failing qualification after a JSON roundtrip. The run stays
recorded as failed; don't relabel it. Its mesh is diagnostic evidence only until
a passing run produces an accepted row.

Next (needs the user's go-ahead): enable serde_json `float_roundtrip` in the
workspace Cargo.toml (or otherwise make the readback exact). Add a RED regression
for the 1-ulp case. Rerun focused tests, then the full queue. Then one fresh
22GiB serial sizing run (about 45 min) with a new run id. Then the 3-grid
study and solver work.

## Earlier — 22GiB serial sizing launched, 2026-11-03 21:22 (Dubai)

The user approved raising WSL memory ("Yes, raise WSL memory"); this resolves the
decision below. Done (Claude session):
- Created C:/Users/mabdu/.wslconfig with `[wsl2] memory=26GB` (there was no file
  before; delete it to revert). Ran `wsl --shutdown` with no distro running. WSL
  now reports MemTotal 26670292 kB, MemAvailable about 25.7M kB, swap 7340032 kB, 32 CPUs.
  The host is 31.81GiB and Windows apps were using about 17GiB, so heavy WSL use
  will page Windows. Close large apps during runs.
- scripts/wp9-replication-sizing-env.sh: serial cap 12582912 -> 23068672 KiB
  (22GiB), MPI rank cap 6291456 -> 11534336 (11GiB), MemAvailable floor
  13631488 -> 24117248 (23GiB). Probe scripts test-wp9-sizing-{policy,environment}.sh
  were updated first. Policy RED was observed (16 passed/11 failed), then GREEN
  27/0. The live environment probe passed 24/0/0 blocked. Logs:
  scratch/wp9-sizing-{policy,integration}-22gib-20261103-a.log. Pre-edit copies
  are in the Claude scratchpad. test-verification.ps1 -LibraryChecksOnly PASS.
  No Rust, PowerShell, recipe, cell cap or gate changed. ram_gb 15.52 in
  tests.rs/verification.ps1 was left as is: it's a conservative warning input
  only, and changing it would change the fixed protocol.
  The full queue was not rerun, because no file it exercises changed.
- CURRENT WORKER: `wp9-replication-serial-sizing-22gib-20261103-a`, profile
  wp9-sizing-serial, worker PID 339740 (child pwsh 345664 -> cargo), 21600s bound,
  sourceBefore 6b117a12bad792790152151bec34e85180bfd7780d38de7c6a055231f63a67e8.
  It was launched from the Claude desktop app's shell. An app update or quit may
  kill it, as the Codex update killed the previous worker. The machine must stay
  awake (no keep-awake hold covers 6h). DO NOT LAUNCH ANOTHER COPY or edit its
  inputs. Next: verification-status.ps1, then that run's snappyHexMesh.log and
  final status. Same rules as below: exit status + actual mesh logs + row readback
  + positive linuxCleanup receipt. Below 12M is diagnostic only. If the worker
  disappears, run Stop-Wp9SizingLinuxGroups on its run dir first.

## Previous state — serial sizing FAILED, memory decision pending, 2026-11-03 21:25 (Dubai)

Supersedes "CURRENT WORKER" below. Serial trial `wp9-replication-serial-sizing-20261103-a`
(np1, 12GiB per-process cap) FAILED meshing: snappyHexMesh.log ends at shell
refinement iteration5 with "new cannot satisfy memory request" after reaching
11,277,125 cells (iteration4). Log last written 21:14:58. Serial got only about 0.4M
cells further than np2 (10,894,785 at iteration4). Surface snapping and the 5 layers
had not started. No solver ran. No checkMesh, row or certified mesh.
The worker then died without a completion record. verification-status shows
`interrupted`, workerAlive/childAlive false, lock available, stdout/stderr 0 bytes,
no exit record in .wsl-control. Probable cause: Windows Update installed an
OpenAI.Codex app update and restarted CodexSandboxService at 21:15:17-21:15:20
(System/Application event logs), which took down the Codex-launched worker. The WSL
VM then idled down at 21:16:33. snappy's own exit code was never recorded.
Linux cleanup was positively confirmed after the fact: Stop-Wp9SizingLinuxGroups
attempted 1, receipt `VERNON_WSL_GROUP_STOPPED vernon-cfd-wsl-328120-5b88d00-2`,
saved as linux-cleanup-receipt.json in the run dir. All logs are retained; never
relabel this run.

Assessment: both trials fail during refinement, before snapping and layers, which
usually need more memory. Removing MPI overhead gave about 3.5% more cells. The
earlier 3.76M-cell road-car mesh peaked at about 10.5GB. With no .wslconfig, the WSL VM
gets the default half of the 31.81GiB host (15.52GiB visible). The 12M-cell minimum is
not reachable on this VM budget with the current recipe. Don't lower the cell or
accuracy gates.

DECISION NEEDED FROM THE USER (not yet asked or approved; no elapsed time counts as
approval): a global WSL memory increase through %USERPROFILE%\.wslconfig (e.g.
memory=26GB, plus raising the per-process caps to match), which needs `wsl --shutdown`;
or a larger/remote machine; or accept that WP9 physical acceptance is blocked on this
host. No new sizing run, cap change or WSL config change until they answer.
No worker is active. WP9 incomplete and uncommitted; no push or publication.

## Earlier live state — serial sizing launched, 2026-11-03

Parallel trial `wp9-replication-sizing-20261103-a` actually ran and FAILED,
exit101. It reached10,894,785 intermediate cells in shell refinement3; at shell
refinement4 the mesher reported an unsatisfied allocation request and MPI rank0
aborted(signal6). Both6GiB rank address-space caps were active. This is not a
finished/certified mesh and does not prove the exact physical RAM requirement.
No solver ran. The worker's Linux cleanup was positively confirmed(1attempt).
Logs/status remain under scratch/continuation/runs/<that run id>; never rerun
or relabel it as successful. Source before/after remained acef84b2.

Before that trial, full queue `wp9-sizing-full-20261103-a` passed1542/0/35ignored,
clean build/all-target Clippy, demo40/0unexpected/5expected, matching catalogue,
and app startup12s/GPU Vulkan4x/nonzeroHWND. Sourceacef84b2bbae1f1a0342b7b7171519b0b557042b90cb468c098669ac2d2ab4d6.

A safer serial experiment is now prepared as a SEPARATE fixed profile
`wp9-sizing-serial`:same Fastback/dx0.32/14M cap/5layers/mesh-only protocol and
approved environment, np1 instead of2. It uses the already tested12GiB serial
cap; no memory-cap increase or shared WSL change. Serial profile guard tests
were observed RED, then library GREEN, including noresume/max6h/approvedidentity/
strict matching np1 row; np1 cannot pass the old np2 profile. Rust code was not
changed for this variant. This tests whether removing parallel overhead helps;
it makes no promise of fitting the12M minimum.

SERIAL POLICY VERIFIED: wp9-serial-policy-full-20261103-a passed all five
steps,1542/0/35ignored, clean Clippy/build, demo40/0unexpected/5expected,
matching catalogue. Source before/after495c37fb24bc3bd07df54aede661692dc6cede863dccae828885a28642a1e9a0.
Only PowerShell profile selection changed after the prior checked runtime/app.

CURRENT WORKER: `wp9-replication-serial-sizing-20261103-a`, profilewp9-sizing-serial,
worker PID328244, fixed21600s bound, node300min. It is already launched.
DO NOT LAUNCH ANOTHER COPY or edit its source/run inputs. First next action:
verification-status.ps1 then inspect that exact run's saved progress/logs.
The new run keeps original source leases, geometry/physics,14M cap,5layers,
mesh-only1 and12GiB serial address-space cap. No solver/global WSL change.

If active, inspect only. If finished, require exit status + actual meshing logs,
row readback and positive linuxCleanup receipt; a partial mesh is not success.
A successful trial below12M remains diagnostic. If allocation still fails,
record the exact stage/cells and assess next execution strategy or prepare a
concrete memory-budget decision; don't lower accuracy/cell gates. If a sizing
worker unexpectedly disappears before final status, positively clean its
recorded Linux groups with Stop-Wp9SizingLinuxGroups before starting new work.
All old runs are retained. WP9 incomplete/uncommitted; no push/publication.

## Active sizing-runner batch — 2026-11-03

The verified code checkpoint below is preserved; a new runner-only batch is now
in progress. Root owns the five-hour mesh-node timeout change in tests.rs and
serial verification. Agent sizing_profile owns verification.ps1,
start-verification.ps1 and test-verification.ps1; agent sizing_cleanup owns the
new wp9-sizing-cleanup.ps1 + test script. All source writers are now frozen. Root alone runs serial verification; inspect
verification-status.ps1 before starting anything. No CFD sizing job has started.
Observed pure profile/parser RED, then library GREEN and55cleanup parser checks
passed. Added durable Rust .intent before process::run; focused RED349/2/13ignored
then GREEN351/0/13ignored. Active runtime check is
wp9-sizing-wsl-runtime-20261103-a (real WSL runner tests, no CFD).
Next: actual owned Bash/sleep forced-client cleanup + intent-only late-start probe
from scratch/wp9-sizing-cleanup-live.ps1 when author freezes it; then integrated
checks and one fixed mesh-only sizing job. Root owns all execution.
Verified runtime: wp9-sizing-wsl-runtime-20261103-a passed all7actual WSL
startup/timeout/cancellation regressions,0ignored. Live cleanup probe
cleanup-live-20261103-164754-a7c22ff0/proof.json passed: actual group survived
Windows client termination; stale startticks refused(exit131) without killing
it; restored identity cleaned with marker + independent group-gone; intent-only
late start refused(exit125) with no PID/tmp/tool witness. No CFD executed.
Current frozen-source full queue: wp9-sizing-full-20261103-a,1200s bound,
source acef84b2bbae1f1a0342b7b7171519b0b557042b90cb468c098669ac2d2ab4d6.
Inspect actual final logs/status before launching sizing; don't assume success.
Resource snapshot scratch/wp9-sizing-resource-preflight-20261103.json:
host31.81GiB,total/free13.56GiB; WSL15.52GiB/MemAvailable15413952KiB;
swap4GiB unused; disk229.26GiB free. Caps/checks remain active at execution.
Once full checks pass and app startup is checked, set VERNON_CAR_DIR to
C:/Users/mabdu/REA/vernon/scratch/wp9-data/tum-drivaer/extracted,
VERNON_WSL_DISTRO=Vernon-Ubuntu-24.04, VERNON_WSL_USER=vernon,
VERNON_WSL_ENV_FILE=/mnt/c/Users/mabdu/REA/vernon/scripts/wp9-replication-sizing-env.sh.
Use existing bundled pwsh start-verification.ps1 -Profile wp9-sizing -RunId
wp9-replication-sizing-20261103-a -JobTimeoutSeconds21600 (space before21600).
Check the run id does not already exist and worker.lock is available. Only one
mesh-only Fastback/dx0.32/np2/14M-cap trial; node limit300min, parent6h, with
positive Linux cleanup receipts required on exit. No solver or global WSL change.
A completion row below12M is diagnostic only. Preserve every failure/log.

No long run is authorized to start without those checks. Existing external
program authorization remains sufficient; no install/global WSL change/push.

## Active implementation checkpoint — 2026-11-03

Codex owns WP9. Checkout is now on `2e01920` after the separately recorded
local source consolidation; the older “merge main before WP9 commit” notes
below are historical. All current WP9 edits remain uncommitted. WP9 physical
acceptance is still false. Current worker status is recorded below; inspect it before launching anything.

User-approved reference remains the detailed16m/s published replication, with
all existing numeric/cell/convergence gates unchanged. Never relabel old30m/s
runs or call the original experiment independently authenticated.

Verified this batch:
- `wp9-reference-primitives-red-20261103-b`:314 passed/1 failed/13 ignored;
  only the missing certified-builder-input helper failed. Its repair is in place.
- `wp9-reference-integration-red-20261103-b`:322 passed/18 failed/13 ignored;
  semantic RED for wheel/domain/patch/proof/record regressions. The earlier-a
  attempt was only a missing Recipe test import, not semantic RED.
- `wp9-reference-integration-green-20261103-a`:340 passed/0 failed/13 ignored,
  source `f98a9c88c9913d6612731ff6322bc337e91807dad768669cb1e0f72db0b17d08`
  unchanged before/after. No external ignored test counted as executed.
- Sizing policy27/0 and fresh-shell integration24/0/0-blocked passed; no mesh
  or solver invoked. Logs `scratch/wp9-sizing-policy-green-20261103-a.*` and
  `scratch/wp9-sizing-integration-20261103-a.*`. Maintained probes are
  `scripts/test-wp9-sizing-{policy,environment}.sh`; environment caps are
  per-process virtual memory (12GiB serial/6GiB MPI rank), not aggregate RSS.

Latest verified source and precise next action:
- `wp9-mesh-recipe-green-20261103-a`:349 passed/0 failed/13 ignored.
- `wp9-replication-geometry-20261103-c`:one real preparation test executed,
 0ignored; all three cases matched registered geometry, actual2right-wheel
 boundaries, complete structured meshing dictionaries and the supported16m/s
 protocol. Final marker cases=3/external_programs=0; no physical mesh/solver.
 Both runs have identical source fingerprint:
 `69942e2cb41d53cfb9b972d2869fccd201d2df3460d2291d8cf0c97e194284a2`.
 Canonical F/E/N surface identities remain4de3bd09d927f4dc/b1a1d6d4525fcb1c/
 f8399fd496485c02. Eight original component SHA256s were checked under read leases.

The actual half-domain metadata failure is repaired in tunnel.rs: measured
full-car wheel metadata is retained, but actual domain.half_model selects y>=0
boundaries. Case setup now passes its effective domain into the default boundary
node, so explicit full domains retain four wheels. Old half-domain tests now
check right-wheel names/origins; full-domain coverage remains explicit.

The meshing admission gap is repaired. mesh_setup emits a strict numeric typed
MeshingRecipe v1. Admission re-renders complete snappy, quality and extraction
dictionaries, requires the canonical three patches and five requested layers,
and rejects legacy/edited/unsupported/unsafe recipes. Independent narrow review
found no remaining blocker under the quiescent-input/non-authentication contract.
Very small absolute-layer heights can fail qualification after an f64 JSON
roundtrip; this fails closed and does not affect the checked relative-layer recipe.
Case/solve input-bound Setup/Solved records and typed benchmark consumer are
wired. They prove current input consistency, not physical accuracy.

FINAL VERIFIED CHECKPOINT: `wp9-integrated-full-20261103-c`, exit0, all five
steps passed. Build clean;1540 tests/0failures/35ignored; all-target Clippy
-Dwarnings clean; demo40nodes/0unexpected/5expected gates; generated catalogue
matches docs/NODES.md. README counts match197total/194implemented/3whitebox.
Source before/after is unchanged:
`a765d522c9b4f8992cae49a306bbd2fb18b1327cf9ecdec0c8166ecc242b88cb`.
The only changes after geometry-c were three test-expression lint repairs.
Bounded app smoke: alive after12s, nonzero window handle, Vulkan4x on the RTX4090
Laptop GPU, then only its own process was stopped. This was startup coverage;
manual visual/interaction testing was not performed. Saved app-startup.json and
app logs are beside the full queue logs. No worker/app remains active.

Full queue-a had already passed all1540 tests but failed three test-only Clippy
styles. Queue-b safely refused resume of that failed run. Queue-c was completely
fresh and passed after the lint repairs. The briefly observed old running status
was a completion race, not evidence of a persistent-launcher defect.

NEXT ACTION: add a bounded mesh-only sizing profile with eight source
leases, fixed Fastback/dx0.32/np2/14M cap/mesh-only1, approved sizing environment
and a six-hour total deadline. The node chain needs an earlier deadline that
leaves cleanup time before the outer worker expires. Confirm Linux group cleanup
on forced outer termination before a long sizing job; do not duplicate old queues
or change global WSL memory/restart settings. No new automotive run was launched.
The ignored harness already uses exact16/rho1.184/nu1.567e-5, radius16000/51,
fixed1000window/write, initial7000 iterations, visible15.52GiB memory warning
and a mesh-only final marker. Actual12M-cell/convergence/coefficient/three-grid
acceptance and WP9 completion remain outstanding. No commit/push/publication.

## Active resume — accepted replication baseline, 2026-11-02

User selected "Use the documented16m/s replication baseline". This supersedes
the historical pending-decision notes below; do not ask again. Codex ownership
was returned after Claude's body-only y+ fix (see final ownership note). Keep
that fix and its reported257/0/12-ignored CFD verification. Main has later WP10
commits; integrate main before the eventual WP9 completion commit, preserving
the dirty WP9 tree. No acceptance or completion claim follows from this decision.

Accepted contract:docs/research/wp9-replication-baseline-v1.json and the research
note. Scope is the documented all-detailed published replication at16m/s,
rho1.184kg/m3,nu1.567e-5m2/s,moving ground/rotating wheels,mirrors,noenginebay.
TargetsF/E/N0.275/0.319/0.277; preserve5%,12M-half/24M-full,three-grid and strict
1000-window/2000-sample convergence gates; no fitted Cd shift. Original2012
experiment authentication remains unclaimed. Geometry and physics binding must
precede any trusted scope/resolver acceptance. Existing old30m/s runs remain
failed diagnostics, never relabelled.

Dictionary-only preparation already cooked3 nodes with0 failing gates under
scratch/wp9-replication-16ms-setup-20261102/. Check its saved manifest when
setting exact air: the first equivalent T/P used287.05 instead of codeR_DRY;
the corrected synthetic equivalent uses287.058. Neither equivalent T/P is a
measurement of source weather. Casegeneration is not a mesh or solver run.

Current batch plan:docs/superpowers/plans/2026-11-02-wp9-replication.md.
Root owns serialverification; agent runtime_spec owns study.rs TEST-only speed
regressions until observedRED; other agents are read-only preflight/binding
design. Check their saved scratch reports before resuming. No verification/CFD
worker was active on takeover. The older full runner fingerprint is stale after
Claude's y+ change; its full result remains historical evidence only.

## Current owner — Codex takeover authorized by user, 2026-10-31

The user explicitly chose "Have Codex take over WP9". This supersedes the
local_f9c822bb checkout reservation below. Codex now owns WP9 source changes and
serial verification in this checkout; other sessions must leave them untouched.
Existing frozen road-car CFD jobs are preserved and must not be duplicated or
have their binary, scripts or case inputs edited while running. Source edits are
independent of that frozen executable. No push, publication or installation is
authorized by this handoff.

PowerShell 7.6.5 is already available at
C:/Users/mabdu/.cache/codex-runtimes/codex-primary-runtime/dependencies/native/powershell/pwsh.exe.
Use this existing executable for bounded verification; no PowerShell installation
is needed. The stale RED verification status predates the current source.

The completed Ahmed audit is saved outside this checkout in
C:/Users/mabdu/REA/tools/wsl/wp9-ahmed-audit/completed-audit.md.
All six iteration gates fail; road-car runs at3.76M cells cannot satisfy the12M
preset floor. The validation-gate and explicit convergence-handoff defects are
repaired and verified below. Remaining work is physical reference/configuration
binding, calibration/convergence and the remaining review items.
WP9 remains incomplete and uncommitted.

## WP9 partial verification checkpoint — 2026-11-02

WP9 remains incomplete and uncommitted. Do not replay the old RED patch work.
The mesh/solve/force/benchmark evidence repair batch is now implemented:
measured mesh records bind the five base tables, cached mesh acceptance binds
the full physical inputs, and successful owned solves carry the final input
fingerprint. Force qualification uses the same source/window and rejects invalid
timestamps. Benchmark checks separate numeric coefficient agreement from full
validation, keep the 12M half/24M full road-car cell floor, recheck strict
stationarity/residual evidence, and fail while reference mapping is unresolved.
A/B comparison requires the full passing benchmark report.

Verified source fingerprint:
9bb0858f17c247e15bb1f4491d733cb659139c8c1949972d4236f0994b4a2d54.

- wp9-evidence-red-20261031-b:233 passed/20 failed/12 ignored; the new evidence
  regressions exposed the missing behavior before implementation.
- wp9-evidence-review-20261102-a:253 passed/1 failed/12 ignored; only the newly
  added duplicate-timestamp regression failed before its narrow repair.
- wp9-evidence-green-20261102-a:254 passed/0 failed/12 ignored, exit0 and identical
  before/after fingerprints. External tests were not counted as executed.
- wp9-native-evidence-20261102-a:one actual ignored test executed, zero ignored,
  exit0 in103.11s. New bounded wp9-native profile requires the exact test and its
  final completion marker. Real own small-car mesh30874 cells,500 solver samples,
  matching mesh/solve/force evidence. Converged=false and capability_only=true;
  this is runtime evidence, not automotive validation or mesh independence.
- Verification parser/library checks passed after the native profile addition.
  Prior performance-guard GREEN62/0 is historical; current workspace checks below
  must confirm the integrated checkout.

The preserved old road-car queue has finished. Read-only host inventory on
2026-11-02 found no remaining CFD/WSL worker. Its frozen binary predates these
repairs; its legacy within_band flag is coefficient comparison only:

| Case | Measured cells | Cd | Outcome |
| --- | ---: | ---: | --- |
| Fastback | 3756304 | 0.2887171068 | 3000 samples; not converged; below cell floor |
| Estate | 3759643 | 0.2833486565 | 3000 samples; not converged; outside5% band; below cell floor |
| Notchback | see saved mesh log | no final result | timeout at iteration1049; Linux process-group cleanup verified |

All six completed slant-body runs still fail iteration convergence. Do not reuse
their outputs as accepted validation or launch duplicate road-car runs. Original
logs/results remain in scratch/wp9-car and scratch/wp9-ahmed. No accepted
experimental mapping has been established from the filenames or preset names.

Final integrated source checkpoint:4738d65bf17f85cbe630a4746e43ee63eb5494c6e990f33d6aad255e03da56a4.
Full queue wp9-evidence-full-20261102-b completed exit0:1443 passed/0 failed/
29 ignored; build, workspace tests, all-target Clippy, demo and catalogue all
passed. Demo40 nodes/0 failures/5 expected gates; generated catalogue exactly
matches NODES.md. First full attempt failed only stale NODES.md; it was
regenerated before the passing run. Counts remain197 nodes/192 implemented/
5 white-box, matching README.

The independent review found an explicit convergence handoff gap. Actual RED
wp9-perf-status-red-20261102-a:62 passed/2 failed; CFD status RED254 passed/
3 failed/12 ignored. The final full run includes repaired CFD257/0/12 ignored
and performance64/0. A present force qualification now emits the shared
iteration_qualified metric; failure reaches the Report and JSON performance
consumers. Raw values remain readable; absent legacy proof retains its previous
diagnostic policy. aero_from_solver still describes origin, not full benchmark
validation. Independent re-review found no material remaining defect in this
repair batch; report:scratch/wp9-evidence-independent-review-20261102.md.

Actual bounded app startup:alive after12s, nonzero window handle, GPU Vulkan4x
MSAA on RTX4090 Laptop GPU. Only the owned probe was stopped. This was hidden
startup coverage, not manual visual/interaction QA. Files:scratch/wp9-app-startup-20261102.*.
The real node-chain source fingerprint above predates only the later explicit
readiness-report/consumer changes; the final full queue verifies those changes.

Precise next action: resolve the experimental reference/configuration before a
new acceptance run. Read docs/research/wp9-reference-baselines.md and ignored
scratch/wp9-{reference-mapping-audit,original-reference-followup}-20261102.md.
The existing targets are retained. S4 uses different smooth N/E baselines; S6
and a later author paper corroborate the detailed target family at16m/s, not
the current30m/s setup. Original experiment/archive equivalence remains unknown;
one ordinary TUM retry and three targeted primary-source searches did not obtain
the original experimental tables. Do not treat another Cd result as that missing
provenance. Preserve all completed/failing runs, no duplicate queue. A full WP9
completion commit still requires physical acceptance. No push, publication,
new installation or K2 upload.

Baseline decision requested2026-11-02: keep the original-experiment contract
and obtain an authorized SAE2012-01-0168 copy, or adopt the documented16m/s
published-replication baseline described in the research note. Await the user's
answer before changing G24/default reference conditions, populating a verified
scope resolver or launching the dependent acceptance run. No elapsed time counts
as approval. Independent remaining source review item: body-only y+ selection
in the generic mesh-quality reader; the current aggregate is documented as such.
All verification workers/probes from this checkpoint have finished.

## Latest completed work package — 2026-10-30, WP8

WP1–WP8 complete under their measured contracts. The local WP8 completion
commit contains this file; use git log to identify it. Branch:
codex/wp9-cfd (WP9 setup in progress). Registry197 nodes/190 implemented/7 white-box;
Capture 22/0. No remote push/publication occurred. Do not restart completed
packages or discard local work. Previous verified WP7 commit:e560a35.

Completed plan:docs/superpowers/plans/2026-10-29-wp8-photogrammetry.md.
Measured contract:docs/specs/real-photogrammetry.md.
Next plan:docs/superpowers/plans/2026-10-30-wp9-cfd.md.

## Verified evidence

- Real run wp8-real-20261030-b completed exit0, one actually executed ignored
  test with AFTER-all-assertions completion marker. SourceBefore=sourceAfter:
  64b54e8147204386444277762e4f566b2888851002a7733c55506364238272e6.
  Twenty public photos, no supplied database/model:20 registered images,
  2703 sparse points,233744 CUDA fused points,293974 mesh vertices/554298 usable
  triangles.104 exact zero triangles explicitly removed; chain204.997 s.
  Numerical100 mm scale checks passed on real sparse/mesh output. These are
  arithmetic handoff checks, not surveyed building accuracy.
- Separate real sparseDelaunay node trial:2024 vertices/4025 triangles,
  0.233s,exit0,valid nonmetric geometry; scratch/wp8-sparse-delaunay.log.
- Fullqueue wp8-final-20261030-c completed2026-10-30 08:23:34UTC,exit0,
  all five steps passed.1171 passed / 0 failed / 10 ignored. Build/Clippy clean;
  demo36 nodes/0failures/2 expected gates. Generated catalogue equals NODES.
  Unchanged final source fingerprint:
  90b5470a5285351e1b07d73b5b31aabff60c04c5b0558ce395037c3e1745324e.
- Actual GPU app launch:Vulkan 4x MSAA,RTX 4090 Laptop GPU; four photo tools
  visually checked in Capture ribbon. Default headless GPU test stays ignored.
- Parser/photo/cache/action/driver regressions and independent final reports
  are scratch/wp8-*. Marker/profile predicate checks and maintained runner
  library checks passed. Host full run exercised the junction-cycle fixture.
- No Cargo dependencies changed. Existing permissive pure-Rust resolve and
  external-process boundary remain. Offline native mesh preview still warns
  white-box placeholder. Simplifier/method variants remain unverified.

Actual JSON/logs are under scratch/continuation/runs/<run-id>/ (ignored).
The real-test fingerprint predates only metadata promotion and maintained
script-regression additions. Full final checks verify those additions.

## Approved tool and local sample

User explicitly approved COLMAP 4.2.1 download, unpack and local checks.
Executable:C:/Users/mabdu/REA/tools/colmap/4.2.1/bin/colmap.exe.
Setup:C:/Users/mabdu/REA/tools/colmap/wp8-setup-status.json,completed.
Official archive SHA256:
e9c5cbd84c2ea986d2e970a2473fc2d2e6b34a2cdcf5d3df2765c319a63af881.
Runtime4.2.1 commitbd1fcf6 withCUDA. No driver/toolkit changes.
Local sample:scratch/wp8-data/images,128 photos; only20used for acceptance.
Sample archive419421847bytes,SHA256:
d210016bd2de20936a5f02b87fd38a76bf0440c42d045231218372cf9db9a7a1.
No separate image licence/readme found:local evaluation only; no sample
photos/binaries/reconstructions committed or redistributed. Spec records
all20photo hashes and precise parameters/argv/log paths.

## Precise next action — approved WP9 setup

WP9 is incomplete. Read its prepared plan and HANDOFF/spec before coding.
Read-only inventory found no simpleFoam/foamRun/Docker on PATH; wsl.exe
reported WSL not installed. Proposed route follows HANDOFF:WSL2 + Ubuntu 24.04
LTS+official OpenCFD OpenFOAM-v2606 precompiled packages, separate processes.
Proposed distro storage:C:/Users/mabdu/REA/tools/wsl/ubuntu-24.04.
The human explicitly approved setup and local CFD checks on2026-10-30.
Do not request this approval again. Runtime phase completed:official Microsoft
WSL3.0.3 x64 MSI, SHA256
ae977286974255bd26bf925f00f40f5692c6b2084155cbc0563c09aea091f211,
Valid Microsoft Corporation Authenticode, MSIexit0 and VMPexit0. WSL reports
3.0.3.0/kernel6.18.54.1-1, defaultWSL2; no reported restart requirement.
Only VMP was enabled, no legacyWSL1 component. No automatic restart.

Inspect C:/Users/mabdu/REA/tools/wsl/wp9-setup-status.json and
wp9-ubuntu-status.json plus the shared setup.lock/native-*.json leases
before another setup attempt. A live process, heldlock or uncertain identity
means no duplicate. Phase2 helper:scripts/setup-wp9-ubuntu.ps1 registers
Vernon-Ubuntu-24.04 at the approved path, under normalmabdu (not an elevated
or different account), with --no-launch; probes Linuxroot and24.04 explicitly.
No Ubuntu/solver success can be inferred from runtime-only status.

Ubuntu has booted successfully and OpenFOAM is installed. Exact signed Noble
candidate/runtime/common version:2606.0~rc2-1. Vendor metadata describes it as
the v2606 runtime; retain that exact version rather than claiming a final
stable build from its banner. Package SHA256:
ef129961af180427090ff04938845c54a21419376474baa371559bad31e19a26.
Official key fingerprint:DC93C096174122E256DA24063386DD74948D208F.
Scoped signed-by repository and strict APT refresh passed; dependencies were
simulated/reviewed before exact-candidate install. No tutorials/dev/GUI extras.

Runtime root:/usr/lib/openfoam/openfoam2606, bannerWM_PROJECT_VERSION=v2606.
Bounded help checks passed for blockMesh,surfaceFeatureExtract,snappyHexMesh,
checkMesh,simpleFoam,pimpleFoam. When invoking the installed etc/bashrc,
disable errexit/nounset/pipefail around source, then restore strict settings
and verify its exit/version; strictmode during source caused an actual error.
The project distro is currently used via explicit --user root; no passwords
or default-user changes. A nonroot project user is still needed before MPI.

Next:finish the fresh duct capability smoke and begin the audited native
driver repairs. Any required future restart remains human-initiated. Never
mark WP9 complete without real mesh/solver/benchmark gates and full checks.

Independent audits are scratch/wp9-driver-audit.md,wp9-handoff-audit.md and
wp9-install-review.md. They identify owned-case/path safety, stale results,
missing quality/residual gates, WSL environment/paths, absent real benchmark
templates and CFD/performance handoff gaps. WP9 is still uncommitted/incomplete.

## Persistent verification and recovery

Authorized heartbeat:Continue Vernon locally,id continue-vernon-locally,
every6hours. Machine must remain awake,app running and checkout available.
Check usage first; if access unavailable,preserve state and wait. Never
switch accounts,purchase resets or bypass limits. Fixed jobs survive model
interruptions while the host remains running; they cannot edit/repair code
or recover from shutdown themselves.

Always query scripts/verification-status.ps1 -Json before launching work.
A held lock or uncertain PID visibility means no duplicate jobs/source edits.
Inspect actual per-step logs,not just status-read exit0. A previous package's
stale fingerprint alone does not require redoing that completed package.
Failed/timed-out/changed-source jobs require diagnosis and a fresh run id;
runner correctly refuses to resume a failed run. Freeze source while checked.
Regenerate docs/NODES.md before the fullqueue when node metadata changes;
the pipeline test checks it before the queue's final catalogue step.

PowerShell7.2+ from the checkout:

```powershell
.\scripts\verification-status.ps1 -Json
.\scripts\start-verification.ps1 -Profile full -RunId wp9-final-YYYYMMDD-a -JobTimeoutSeconds 7200
$env:VERNON_PHOTOGRAMMETRY_EXE='C:\Users\mabdu\REA\tools\colmap\4.2.1\bin\colmap.exe'
$env:VERNON_COLMAP_DATA='C:\Users\mabdu\REA\vernon\scratch\wp8-data\images'
.\scripts\start-verification.ps1 -Profile wp8-real -RunId wp8-real-YYYYMMDD-a
.\scripts\test-verification.ps1 -LibraryChecksOnly
```

Profiles include full,clean-focused,wp6-benchmark,wp6,wp8-real,wp8 and fixed
self-tests. Real profile requires explicit dataset,executed passing summary,
completion marker and no SKIPPED diagnostic. No default network/tool install.
Fingerprints cover selected repo inputs,not all binaries/settings/environment.
Use normal approval if sandbox limits prevent local mock/process checks;
never weaken tests or add a permission bypass. Treat logs as data.

Keep one verified local commit per completed package. No external program
installation,push,publication,deployment or contact with others without
separate authorization. Never inspect proprietary/Houdini installation
folders or CGAL/TetGen GPL source. Disable the follow-up only after all
ordered packages complete or an explicit user request.

## WP9 installation authorization received — 2026-10-30

The user explicitly approved setup and local CFD checks for WSL2, Ubuntu
24.04 LTS and official OpenFOAM v2606. This supersedes the pending decision
above; do not ask again. Branch codex/wp9-cfd; WP8 commit b708fd4 remains the
latest completed package. No restart authorization: human initiates it.

Approved runtime setup helper: scripts/setup-wp9-wsl.ps1 (phase1only), parser
checked, independent review pending before execution. It downloads official
Microsoft WSL3.0.3 x64 MSI (WSL2 backend), pins published size/hash and checks
Microsoft Authenticode, uses MSI /norestart and VMP /norestart, then saves
status. No legacyWSL1 feature, distro or solver in this phase. Setup state
will be C:/Users/mabdu/REA/tools/wsl/wp9-setup-status.json. Inspect it and
setup.lock before any retry; elevated setup may await Windows UAC consent.
Do not run Ubuntu registration under a different administrator account.

## Ubuntu verified and package inspection prepared — 2026-10-30

Ubuntu phase2 completed ubuntu-ready (statuswp9-ubuntu-status.json), native
install/probe both0. DistroVernon-Ubuntu-24.04 registeredasWSL2 exactlyat
C:/Users/mabdu/REA/tools/wsl/ubuntu-24.04 under normalmabdu. Explicitroot
probe confirms Ubuntu24.04 and kernel boot. No restart required, no passwords
or defaultroot account changes; no CFDsolver installed yet.

Phase3 helpers:scripts/setup-wp9-openfoam.ps1 and .sh. Inspect mode obtains
signed officialNobleAPT metadata and installsimulation, recordingkeyhash/
fingerprint and exactcandidate to tools/wsl/openfoam; install mode is a
separate explicit command pinnedto that candidate. Review actualmetadata
and dependencylist before install. Scripts parse/builtinUbuntu bash-n passed;
independent review pending. Use shared setup.lock/native leases; no duplicate
native job or code changes while a helper uses it. DefaultRust/source counts
stillWP8; WP9fullverification/commit notdone.

## OpenFOAM setup verified; WP9 native work started — 2026-10-30

Signed officialNobleAPT inspect completed and exactcandidate2606.0~rc2-1
runtime/common installed successfully (wp9-openfoam-status.json modeinstall,
install-complete,exit0). Sources/keys/policy/simulation/packagehashes and
installed package metadata are in C:/Users/mabdu/REA/tools/wsl/openfoam.
Six actual binary help probes passed,WM_PROJECT_VERSION=v2606. No reboot,
password/default-user change, tutorials/dev/GUIextras, push or publication.

The own clean-room320-cell laminar duct script is scripts/wp9-cfd-smoke.sh.
Firstrunfailedpreflight because dpkgwildcard matched an uninstalled tutorial;
fixedinstalled-statusfilter. Secondrun duct-smoke-20261030-100022 actually
ran blockMesh/checkMesh/simpleFoam with exit0, MeshOK and60/U,p; finalsmoke
classifier rejected normal trapFpe informational banners. Child owns that
narrow classifier correction; rerun a NEW case folder and inspect logs.
This is capabilityonly,notAhmed/DrivAer or calibrated physical validation.

Root mesh.rs quality slice observed RED0passed/3failed then GREEN14passed,
scratch/wp9-mesh-quality-{red,green}.log. Required finitecomplete mesh data,
positivecellcounts, real skewness and strict70degree/4skewness gates now
implemented; current-runlogs preferred over legacy. No implementationflag
promotion. Native fullworkspace checks not yetrun; cataloguemustregenerate
before eventual fullqueue aftermetadatachanges.

Approved nextcode batches: parent wp8_model_import owns util.rs/newprivate
staging.rs transactional ownedcase helper; wp8_runtime_spec owns extproc.rs
structuredWSL/cleanup/deadlines. Both currently RED-test/API phase and must
wait for root-owned serialCargo evidence before productionfixes. Root owns
mesh.rs/case.rs caller integration and provenance. Reports and complete
acceptance gaps: scratch/wp9-driver-audit.md and wp9-handoff-audit.md.
Latestcompletedpackage remainsWP8 b708fd4; WP9dirty/uncommitted/incomplete.

## Verified capability checkpoint — 2026-10-30

OpenFOAM setup and own duct capability smoke passed. Status:
C:/Users/mabdu/REA/tools/wsl/wp9-smoke-status.json.
Case:scratch/wp9-setup/duct-smoke-20261030-101004.
Actual blockMesh/checkMesh/simpleFoam and script exits0;320cells,MeshOK,
maxnonortho0degrees,maxskew8.881784197e-15;60iterations,End,nonemptyfinite
60/U,p. Finalpressure residual5.666868658e-09. This is capabilityonly;
no automotive benchmark or Vernon's real-driver contract has passed yet.

Classifier corrected only for exact benign trapFpe banner. Eightpure
classifierprobes RED->GREEN; oldfailed cases retained. Root independent
finite-token check passed on written U/p. Sixprogramhelpprobes also0.

Nativework status:meshquality sliceGREEN14; stagingRED0/17fail nowall
behavioral, Windowsjunctionfixturecorrected; extprocRED3pass/13fail.
Logs scratch/wp9-stage-red.log/wp9-extproc-red.log. Parent/runtimeagents
are implementing those approved modules withoutCargo; waitforfrozenready,
runroot-ownedGREENserially andinspectactualoutputs. Childreviewing
rootmeshqualityslice. Nofullqueueactive; latestpriorWP8statusisnaturally
stale dueWP9files anddoesnot invalidate completedWP8. Don't rerunWP8.

## Current checkpoint — 2026-10-31 (Dubai)

WP9 remains incomplete and uncommitted; WP8 `b708fd4` is the latest completed
package. The older RED summaries above are historical. No worker is active.
Usage was checked on resumption: ordinary usage available, no reset purchased.

The persistent `wp9-wsl-containment-20261030-b` profile completed with exit 0:
16 default runner tests passed and all 7 actual WSL tests passed. Its logs are
under `scratch/continuation/runs/wp9-wsl-containment-20261030-b`. Two newly
reproduced containment bugs were repaired: surviving background children after
a successful leader exit, and delayed cancelled launches after case replacement.
The new WSL API takes an explicit stable control directory outside the movable
case. Keep this directory beside the case under its owner, not inside a case
generation. MPI/detached process groups have not been qualified.

The mesh-quality slice passed 17 tests and independent review. Original owned
staging passed 18. Integration exposed the need for optional-source templates
and optional mesh preservation: its 10 new tests produced 1 pass/9 failures in
`scratch/wp9-stage-optional-red.log`. The approved extension is being implemented
by wp8_model_import. The wall-quality manual-provenance test produced 0/1 in
`scratch/wp9-wall-provenance-red.log`; model_review owns its narrow correction.
Root's 3 case-setup regressions failed as expected in `scratch/wp9-case-owned-red.log`:
upstream writes, stale optional physics, and legacy unmarked folder adoption.
Root owns case.rs integration. All Cargo execution remains serial and root-owned.

The user supplied publisher access for the registered TUM download; it was used
through hidden stdin, with no credentials saved in project files or logs.
The official STL archive is local only in `scratch/wp9-data/tum-drivaer`:
590,018,283 bytes, SHA-256
`cb2130d354248597f853793be0850cc1552a41dfdf488e09d6ca9986f019896c`.
Its 48 entries contain two sets of 24 ASCII STL components, with no licence or
README. Do not commit or redistribute the data. Geometry audit metrics already
exist there; field_method is finishing the report without rerunning the scan.
Benchmark configuration equivalence and physical acceptance remain unverified.

Next: finish/freeze the optional staging and wall-provenance corrections;
integrate consumer-owned case setup in a fresh namespace preserving legacy data;
run bounded focused verification and inspect its logs. Then integrate fresh
mesh/solve execution, input provenance and real-node tests before automotive
validation, performance wiring, full checks or any WP9 completion commit.

## Resumption check — 2026-10-31 (later)

No worker or cargo job is running (worker.lock pid 77324 is dead; the only live
processes are unrelated python/claude). `scripts/verification-status.ps1`
needs PowerShell 7.2+ and `pwsh` is not on PATH here, so status was read
directly from `scratch/continuation/latest.json` and the run `status.json` files.
Latest run `wp9-fresh-driver-red-20261031-a` is the intended RED for the fresh
mesh/solve driver: `cargo test -p vernon-nodes-cfd` = 161 pass / 8 fail
(mesh.rs: defaults serial + explicit WSL fields, blank WSL context fails before
any step, build owns inputs and never writes the upstream case, failed build
publishes no File, zero-exit fixtures cannot certify bad mesh evidence;
case.rs: solve provenance must cover constant/physicalProperties, failed solve
must not publish old history/upstream coefficients, exit 0 without fresh
results cannot certify old data). WP9 remains incomplete and uncommitted; no
code was changed in this check.

Next precise action: implement the fresh mesh/solve execution in
`crates/vernon-nodes-cfd/src/mesh.rs` and `case.rs` (copy case into an owned
fresh generation, run with the explicit WSL context, publish only on fresh
verified outputs, hash physics/mesh/initial snapshot into provenance) until
those 8 tests pass; run via the bounded `wp9-focused` profile, inspect logs,
then input provenance, real-node tests, automotive validation, performance
wiring and full checks before any WP9 commit.

## Concurrent-edit stop — 2026-10-31 02:48 (Claude session)

A live codex.exe was found editing this checkout (case.rs, coeffs.rs and this
file changed 02:44-02:46) while a Claude session resumed the same step. Claude
stopped to avoid competing edits. Claude DID apply one uncompiled, untested
change to `crates/vernon-nodes-cfd/src/mesh.rs`: `run_owned_chain`,
`unavailable_output`, `mesh_certified`, WSL params, serial default, an owned
fresh `build_cook`, and `build_without_run_writes_script_and_fails_closed`
updated. The matching solve_cook rewrite was NOT applied; it is saved in
`scratch/wp9-claude-solve-patch-20261031.py` (its recipe_key edit is obsolete:
that change is already in case.rs). Next: one owner only; review mesh.rs, then
run the wp9-focused checks. The 8 RED tests are not yet shown passing. pwsh 7.2+ was
not found on PATH or in Program Files, so verification scripts could not start here.

## Checkpoint — 2026-10-31 02:55 (corrects the concurrent-edit stop above)

The "live codex.exe" in the stop note was wrong: the 02:44-02:46 edits came from
another Claude session on this checkout. Codex showed no CPU activity. Run only
one continuation session at a time on this checkout.

Now implemented, uncommitted: the checked history reader in coeffs.rs, `run`
excluded from `recipe_key`, owned fresh `build_cook` (mesh.rs) and `solve_cook`
(case.rs) with WSL params, and regenerated docs/NODES.md. The saved
scratch/wp9-claude-solve-patch-20261031.py is obsolete; do not apply it.
Pre-edit source copy: scratch/wp9-snapshot-20261031-claude.

Evidence, direct cargo runs because pwsh 7.2+ is not installed here, so NOT the
bounded wp9-focused profile: vernon-nodes-cfd 186 passed / 0 failed / 7 ignored;
workspace 1246 passed / 1 failed / 17 ignored (scratch/wp9-workspace-20261031.log);
clippy clean; demo 36 nodes, 0 failing gates, 2 expected; catalogue matches.
The app was not launched. The one failure is
process::tests::timeout_and_cancel_kill_the_tree, not touched here; main's WP14
commit 1e33d53 changed process.rs and that test passes on main.

Unverified: none of this has driven real OpenFOAM. The new tests use fake tools
and synthetic logs, so real solver logs may still expose reader mismatches.

Next: merge main into this branch (picks up the process.rs fix; expect docs
conflicts) or otherwise resolve that test; run the real WSL node chain on the
duct case with Vernon-Ubuntu-24.04 (needs a non-root user and the environment
file); then input provenance, automotive validation, performance wiring and
full checks. WP9 remains incomplete; do not commit it as complete.

## Owner — 2026-10-31 06:05

Session local_f9c822bb (a fork of the top Vernon session) now owns this checkout
for WP9. Other sessions: do not edit or run cargo here until this note says the
session has released it. A copy of the uncommitted work from 06:02 is in
C:/Users/mabdu/REA/backups/wp9-20261031-0602. WP11 and WP12 are in their own
worktrees, vernon-wp11 and vernon-wp12.

## Checkpoint — 2026-10-31 06:25 (first real runs)

Branch fast-forwarded to main 8a4cd90; WP9 is still uncommitted and incomplete.
Backup of this state: C:/Users/mabdu/REA/backups/wp9-20261031-study-start. Linux
user `vernon` now exists in Vernon-Ubuntu-24.04 (locked password, not default).
Use VERNON_WSL_DISTRO=Vernon-Ubuntu-24.04, VERNON_WSL_USER=vernon and
VERNON_WSL_ENV_FILE=/usr/lib/openfoam/openfoam2606/etc/bashrc. pwsh 7.2+ is still
not installed, so these runs are direct cargo runs, not the bounded profiles.

Real results, read from the logs:
- `real_openfoam_chain_on_a_small_car` passed: 30,878 cells over 8 MPI ranks, Mesh
  OK, non-orthogonality 65.0, skewness 2.74, 92.4% layers, 500 solver iterations,
  Cd 0.591 on a blocky shape (plausibility only). scratch/wp9-real-chain-6.log.
- `real_openfoam_accepts_the_transient_recipe` passed: kOmegaSSTDDES and maxDeltaxyz
  selected, 11 time steps, clean End. scratch/wp9-real-ddes-2.log. Runtime
  acceptance only, not accuracy.

Defects those runs found and this checkout fixes: OpenCFD uses
surfaceFeatureExtract; mesh tools need controlDict/fvSchemes/fvSolution, which
mesh_setup now writes; minMedialAxisAngle is required as well as the old name;
potentialFoam needs Phi and potentialFlow entries. checkMesh now runs without
-allTopology -allGeometry, as HANDOFF specifies, so "Mesh OK" no longer covers
those optional checks (the 3 they failed on the small car were concave cells,
low-quality tets and small determinants).

Also done, default tests only: transient_ddes mode in cfd.case_setup; solve_steady
refuses it; per-surface force monitors; benchmark_check measures slant share from a
connected case; perf.vehicle refuses zero CdA, gains allow_missing_aero and an
aero_from_solver gate; the example now wires solve -> forces -> vehicle: 40 nodes,
5 expected gates. The app and full workspace checks have not been run on this.

Running: scratch/wp9-ahmed-queue.ps1 (6 runs, 25 and 35 degrees, dx0 0.105 / 0.081 /
0.062, 3000 iterations, 16 ranks) from the frozen binary
scratch/wp9-bench-bin/vernon_nodes_cfd.exe. Results and logs: scratch/wp9-ahmed/.
It skips finished results, so rerun it to continue. Source may change during it.
Stilt x and y positions in `slant_body` are assumed; only their diameter was
confirmed in public text.

Next: read the six results, feed them to cfd.mesh_study and cfd.benchmark_check,
then DrivAer. WP9 stays incomplete until those gates are met or reported as unmet.

## Checkpoint — 2026-10-31 07:15

The first queue was stopped because dx0 0.105 gave 361,910 cells, below the 0.5M
floor. The queue now uses dx0 0.092 / 0.0708 / 0.0544 with labels a25-g1 ... a35-g3.
a25-g1: 502,811 cells, Cd 0.394, Cl 0.362, slant 37.0%, stilts 8.6%, convergence
gate not met. Workspace in this checkout: 1290 passed, then the one failure
(the app demo test pinning 2 expected gates) was updated and passes on its own; a
full rerun is still due. Example: 40 nodes, 0 failing, 5 expected. App opened.

Other packages, each one verified local commit, not merged, not pushed:
- WP11 claude/wp11-flatten-tuft a073ff3, worktree vernon-wp11: 1225 passed, 0 failed.
  Spec F2 checks (b), (c), (d) are unmet and left ignored with measured values.
- WP12 claude/wp12-file-formats 80a7c96, worktree vernon-wp12: 1251 passed, 0 failed.
  Synthetic files only; no real scanner file was available. No STEP reader.
- WP10 is on claude/wp10-solve-linear-89beae and claude/wp10-tet-mesh-017c48 from
  other sessions, also not merged; not re-verified here.

Not started: WP15 scaffolding and WP16 release files. WP15 runs need the owner.
cargo-deny and pwsh are not installed. Usage: wait for the limit to reset before
starting more agents; the solver queue needs no tokens.

## Checkpoint — 2026-10-31 12:31 (WP10, WP11, WP12 merged)

On the owner's word, `main` moved from 8a4cd90 to b003401, merging both WP10
branches, WP11 and WP12. Not pushed. The one conflict was in docs/HANDOFF.md.
The merged result, checked before main moved: 1290 passed, 0 failed, 17 ignored;
clippy clean; example 38 nodes, 0 failing, 2 expected; catalogue same ignoring
line endings; app opened. Run directly with cargo; pwsh is still missing.

This overrides the "not merged" notes above. WP10 is merged but not finished: the
`fea.tet_mesh` fTetWild test has not been run, and both nodes stay white-box. Another
session then moved main to dd521c2 with a CalculiX deck fix; its commit message says
both cantilever comparisons pass with CalculiX 2.21. Not re-run here, and
docs/HANDOFF.md WP10 still needs that recorded and the white-box flag decided.

## Checkpoint — 2026-10-31 (Ahmed runs finished)

This checkout's codex/wp9-cfd now sits on main dd521c2 with WP9 still uncommitted.
Backup before the move: backups/wp9-20261031-ahmed-done. Before the WP9 commit:
regenerate docs/NODES.md and README counts, then run the full definition of done.

All six slant-body runs finished; the table and findings are in
docs/specs/automotive-cfd.md 3.11a. Neither angle passes `cfd.mesh_study`. No run met
the convergence gate. G25 ordering holds on the finest grids only; slant share is 34%
(inside the band) and 41% (14 points off). The study node used to accept the 25-degree
set; it now also requires a believable order and scatter within the two-grid limit.

Road car, fastback at dx0 0.48: 3.76M cells, non-ortho 65.0, skewness 3.28, layers
96.6%, Mesh OK. The mesher peaked at 10.5 GB of the 15 GB here, so about 5M cells is
the ceiling against the preset's 12M floor. scratch/wp9-car-queue.ps1 runs the three
rear shapes at that grid (roughly 4 hours each, estimated). It skips finished JSONs in
scratch/wp9-car, so rerun it if interrupted; never start a second copy. Frozen binary
scratch/wp9-bench-bin/vernon_nodes_cfd_car.exe, SHA256 8C2E7D35...7C06.

Next: read scratch/wp9-car/*.json and queue.log. These runs are one grid each, below
the 12M floor, with G24's experiment mapping unresolved, so whatever Cd they give, G24
is not met by them. Their JSON was written by the frozen binary, before the review
fixes: its `within_band` means only that Cd was inside the band.

Review: scratch/wp9-review-findings.md is the list, ticked as fixed; open items include
the road-car `within_band` claim (min_cells_million is never read). Work through it
without more agents until the usage limit resets, then rerun the definition of done.
Owner decision pending: plain checkMesh was adopted after -allTopology -allGeometry
failed three checks on a real mesh. Recommendation: keep plain as the gate and also
report the extra checks' failures as a visible number.

## Road-car queue finished — 2026-11-02 17:07 (local_f9c822bb, results only)

No source was edited for this note. Table and details: scratch/wp9-car/RESULTS.md.
Fastback Cd 0.2887 (+4.99% against 0.275, iteration sd 4.3%); estate Cd 0.2833
(-11.2% against 0.319); ordering estate > fastback reversed. Neither converged.
The solver's own Cd matches the force-derived Cd to six digits in both. G24 is not
met. The notchback has no result: Windows slept from 2026-10-31 22:44 to 2026-11-02
17:05 and the wall-clock timeout fired on wake at iteration 1049. Rerunning
scratch/wp9-car-queue.ps1 finishes it (about 4 hours, machine kept awake), but only
when the checkout's owner is not running tests, since the mesher needs about 10.5 GB.

## Owner change — 2026-11-02 19:55 (Claude, session 20623820)

The user reported Codex is out of credit and asked Claude to continue its work.
Claude now owns WP9 in this checkout; other sessions leave it alone.

Done: the independent review item. `parse_yplus_dat` (mesh.rs) now averages only
`body` and `body_*` patches, and returns nothing when there is none; the spec
line in automotive-cfd.md says so. Checks on this tree: CFD 257 passed / 0 failed
/ 12 ignored; workspace 1443 passed / 0 failed / 29 ignored
(scratch/claude-yplus-20261102.log); clippy clean; demo 40 nodes, 0 failing,
5 expected. Catalogue and app launch were not rechecked; no node metadata changed.

Still blocked on the user: the baseline decision above (authorized SAE
2012-01-0168 copy, or the documented 16 m/s published-replication baseline).
No acceptance run, G24 change or verified scope resolver until they answer.
The checkout's branch is still behind main (main has WP10 fixes); merge main
before the WP9 commit. WP9 remains incomplete and uncommitted.

## Owner returned to Codex — 2026-11-02 (user decision)

The user asked Claude to stop WP9 and leave it for Codex when its credits return.
Codex owns WP9 again. Claude's only change since 19:55 is the y+ body-only edit
recorded above (mesh.rs, its test, and one spec sentence); keep or revert it.
Claude makes no further edits or cargo runs in this checkout and is working on
WP15 in the separate worktree C:/Users/mabdu/REA/vernon-wp15, cut from main.

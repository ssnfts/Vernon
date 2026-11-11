# WP9: plan for a time-averaged DDES route to a passing gate

Status: **plan for review, 2026-11-11.** The user chose this route ("Plan the
DDES route") after the evidence below. Nothing here starts a run, changes a
gate or relabels a result. Every step that changes the accepted contract
(`docs/research/wp9-replication-baseline-v1.json`) is listed as a decision
for the user.

## 1. Why the steady gate cannot pass

The steady production gate needs residual RMS ≤ 1e-5 for p, U, k and omega
over 1000 iterations. Every steady variant has stayed 2–3 orders of magnitude
above that: the original run, upwind startup, limited correction, the
layer-control remesh, limited k/omega gradients, tighter p relTol and U
relaxation 0.7.

On 2026-11-11 a residual map (30 production iterations from the Stage 2
checkpoint, per-cell `writeResidualFields`) showed why:

| Field | Share of the residual in the worst 0.1% / 1% / 10% of cells | Reduction needed |
|---|---|---|
| p | 20% / 46% / 84% | ~160× |
| Uz | 8% / 32% / 75% | ~2000× |
| k | 11% / 38% / 84% | ~20× |
| omega | 13% / 38% / 77% | ~6× |

- **U and k:** the worst cells are in the near wake, x 5.4–6.6 m behind a car
  that ends at about 4.6 m. They are good cells (median non-orthogonality
  1–2°, the same as the whole mesh). 40–51% of the U and k residual is there.
- **p and omega:** the worst cells are around the front wheel and in the
  near-wall layers, and the mesh is poorer there (median non-orthogonality
  16–17°). This is real but a minor share.
- **The hotspots move:** only 24–54% of the worst cells are the same at
  iterations 2010 and 2030.
- **Settings change the answer:** the steady Cd depends on the solver
  settings. It is 0.2758 at U relaxation 0.9 and 0.2854 at 0.7, both with a
  quasi-periodic oscillation of 50–90 iterations.

So the plateau is the unsteady separated wake, not a local defect. A
time-resolved solve with time-averaged forces is the standard way to model
this kind of flow, and `cfd.case_setup` already warns that steady SST only
gives a trend where the flow separates.

Evidence: `scratch/wp9-investigation-20261110/arms-log.md`,
`scratch/wp9-investigation-20261110/resmap-20261111/resmap.json`.

## 2. Measured cost: DDES timing probe (diagnostic only)

**Setup:**

- **Case:** `scratch/wp9-investigation-20261110/ddesprobe-20261111`, run
  outside the runner and labelled diagnostic.
- **Start:** the RANS state at iteration 2030 on the production 12,056,197-cell
  half mesh, 16 ranks.
- **Dictionaries:** the existing `transient_ddes` recipe written by hand from
  `foam.rs`. That means kOmegaSSTDDES, `maxDeltaxyz`, backward time stepping,
  `Gauss linear` for U, PIMPLE with 2 outer correctors × 2 pressure
  correctors, maxCo 0.5 and a first time step of 1e-4 s.
- **Length:** capped at 45 minutes, giving 50 time steps.

**Results:**

| Measure | Value |
|---|---|
| Startup to the first finished step | ~21 min (mesh, ASCII fields, wall distance) |
| Courant at Δt = 1e-4 s | mean 0.0055, **max 4.24** |
| Δt chosen for maxCo 0.5 | **1.17–1.18e-5 s** |
| Wall time per step | median 32 s (30–37) |
| p initial residual, first solve / final corrector | median 6.1e-3 / 2.9e-5 (max 2.0e-4) |
| Memory | 18.8 GB resident over 16 ranks, 7.2 GB available, no swap |
| Stability | 50 steps clean (5.9e-4 s of physical time, 0.2% of one convective unit) |

The probe establishes cost and short-term stability only. It says nothing
about accuracy or long-run stability.

**What that costs:** one convective unit is L/U = 4.6 / 16 = 0.2875 s. Take
a run of about 8 convective units (2 to wash out the RANS start, 6 to
average), which is 2.3 s of physical time:

| Time-step policy | Steps per physical second | Wall time per run (8 CTU) |
|---|---|---|
| Recipe default, maxCo 0.5 everywhere (measured) | ~85,000 | **~72 days** |
| Δt = 1e-4 s, peak Co ~4, Co ≲ 1 in the wake, 3–4 outer correctors (estimated ~60–70 s/step, **not measured**) | 10,000 | ~16–19 days |

The protocol needs Fastback, Estate, Notchback and two coarser Fastback grids,
so five runs. Even on the optimistic policy that is roughly **2–3 months of
this PC**. At the default it is not feasible here.

## 3. Decisions for the user (before any code or run)

1. **Baseline v2: transient acceptance.** This would be a new file,
   `wp9-replication-baseline-v2.json`. v1 and every v1 verdict stay as they
   are. Draft values for review:

   **Unchanged from v1:**
   - 16 m/s and the air state;
   - raw time-averaged Cd within 5% of 0.275 / 0.319 / 0.277, with no fitted
     shift;
   - the 12M half-cell floor and the mesh-quality limits;
   - the three-grid study and the estate > fastback ≈ notchback ordering.

   **Wash-out:** discard at least 2 convective units (0.575 s) after the
   switch from RANS.

   **Averaging:** at least 6 convective units (1.725 s), using
   **time-weighted** Cd because Δt varies.

   **Statistical gate on the averaging window**, reusing the logic already in
   `crates/vernon-nodes-cfd/src/convergence.rs` on a series resampled to
   uniform time:
   - no significant trend (99%);
   - the means of the two halves within 0.2% (v1's window-change limit);
   - effective sample size ≥ 50;
   - standard error of the mean ≤ 0.5% of the mean, so the 95% interval of
     about ±1% sits well inside the 5% band.

   **Per-step convergence, replacing v1's 1e-5 RMS rule:** the final-corrector
   initial residual of p ≤ 1e-4, and of U, k and omega ≤ 1e-5, in at least 99%
   of steps. The probe gives p median 2.9e-5 and max 2.0e-4 at 2×2
   correctors. Courant is reported: peak and in the wake region.

   Approving v2 changes the contract. The record will say so, and it will not
   present v2 as v1 having passed.

2. **Time-step policy.** Either maxCo 0.5 everywhere (measured, ~72 days per
   run), or a fixed Δt sized for Courant ≲ 1 in the wake, with more outer
   correctors and outer residual control so each step converges. The second
   needs probe B (section 4) to show it is stable and converges per step. Δt
   is picked from numerics only, never from Cd.

3. **Compute.** Either this PC (months, one run at a time, and memory rules
   out a finer-than-12M DDES), or external HPC or cloud capacity. External
   compute needs your explicit authorization: the DrivAer data and logs would
   leave the machine, and the current rules forbid sending logs.

4. **Scope.** Either the full protocol in DDES (5 runs), or DDES for Fastback
   on the fine grid first, with the go/no-go for the rest decided after its
   result.

5. **The steady results' role.** The steady results stay on record as "not
   converged" under v1. The U-relaxation 0.7 steady Cd (0.2854) is a trend,
   not a certified value.

## 4. Work plan once those decisions are made

Each code step follows AGENTS.md: a failing test first, the minimal change,
then the full checks (`cargo build --workspace`,
`cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`,
the demo cook, NODES.md, an app launch). There is one heavy job at a time,
each with its own run ID. Fingerprinted paths are not edited while a job runs.

**Phase 1: make the transient route measurable (code, no long runs)**

1. **Baseline v2 JSON**, as a new file. Tests pin its values the way
   `extended_gate_uses_the_protocol_acceptance` pins v1.
2. **Time-weighted transient force statistics and the transient gate** in
   `coeffs.rs`, reusing the trend and ESS functions in `convergence.rs`. The
   inputs are variable-Δt `coefficient.dat` rows and the per-step solver
   residuals. Tests cover uneven Δt, the wash-out cut, the ESS and
   standard-error limits, and a drifting series that must fail.
3. **DDES recipe gaps** in `foam.rs` / `case.rs`, each from evidence, not
   from Cd:
   - PIMPLE outer-loop residual control and corrector settings from decision 2;
   - the D5 limiters on grad(k) and grad(omega), which the steady recipe
     already has and DDES lacks;
   - a time-step mode set by decision 2;
   - `fieldAverage` of U and p for wake inspection.
   - Keep `div(phi,U) Gauss linear` unless a probe shows instability; if it
     does, the alternative is a documented blended scheme.
4. **Runner:**
   - a `wp9-transient-probe` profile (≤ 6 h) and a `wp9-transient` profile
     that runs in **resumable segments**, each within the 48-hour bound,
     building on the existing `ResumeRunId` path;
   - start from a preserved RANS checkpoint, which reuses the checkpoint
     restart mechanism from 625a5b2;
   - extract per-step convergence and Courant into the diagnostic JSON.

**Phase 2: time-step probes (labelled diagnostics, ≤ 6 h each, one at a time)**

- **Probe A:** recipe default, maxCo 0.5, to confirm the measured cost and
  check stability over about 600 steps.
- **Probe B:** fixed Δt = 1e-4 s, 3–4 outer correctors with outer residual
  control. Measure per-step convergence against the v2 limits, seconds per
  step, peak and wake Courant, and stability.
- **Choose Δt** from probe B's numerics only. If probe B can't meet the
  per-step limits, stop and report rather than loosen them.

**Phase 3: Fastback production DDES (segments, fine grid)**

- Wash-out and averaging per v2, then score with the transient gate. Report Cd
  with its standard error against 0.275 ±5% as observed.
- If it fails, keep the negative result and do not rerun automatically.

**Phase 4: the remaining protocol evidence** (only if decision 4 allows and
Phase 3 converged)

- Estate and Notchback on the fine grid, two coarser Fastback grids at a
  constant refinement ratio, then `cfd.mesh_study` and `cfd.benchmark_check`.

## 5. Risks to state up front

- **Mesh suitability.** The 12M mesh was sized and refined for RANS. A DES
  wake needs reasonably isotropic cells in the LES region. A coarse wake
  makes the averaged Cd depend on the grid (the "grey area"), which the
  three-grid study would expose. Phase 2 should report wake cell sizes.
- **Memory.** DDES at 12M uses about 18.8 GB of the 26 GB WSL allocation. A
  finer-than-floor grid will not fit on this PC. The three-grid study uses
  coarser grids, so it fits.
- **Cost.** Even the optimistic policy is weeks per run. A failure late in a
  run costs a segment, not the run, but only once resumable segments exist
  (Phase 1, step 4).
- **The reference.** 0.275 is a published-replication value, not an
  authenticated experiment (see `wp9-reference-baselines.md`). A DDES result
  outside the band is reported as is, and nothing is tuned toward it.

# WP9: plan for a time-averaged DDES route to a passing gate

Status: **plan for review, revision 2, 2026-11-11.** The user chose this route
("Plan the DDES route") after the evidence below. Revision 2 folds in a
three-perspective review (engineering, racing, aesthetics; section 6). Nothing
here starts a run, changes a gate or relabels a result. Every step that
changes the accepted contract
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

**What that costs:** one convective unit (CTU) is L/U = 4.6 / 16 = 0.2875 s.
Take a run of about 8 CTU (2 to wash out the RANS start, 6 to average), which
is 2.3 s of physical time:

| Time-step policy | Steps per physical second | Wall time per run (8 CTU) |
|---|---|---|
| Recipe default, maxCo 0.5 everywhere (measured; **dropped in revision 2**) | ~85,000 | ~72 days |
| Δt = 1e-4 s, peak Co ~4, Co ≲ 1 in the wake, 3–4 outer correctors (estimated ~60–70 s/step, **not measured**) | 10,000 | ~16–19 days |

The revised acceptance (section 3) can lengthen a run: averaging may extend
to 15 CTU, and a Δt/2 segment adds about 2.5 CTU at twice the step count.
Cost estimates are redone after probe B.

## 3. Decisions for the user (before any code or run)

Items marked **open conflict** are where the reviewers disagreed; the
alternatives and a recommendation are given, and the user chooses.

### 3.1 Baseline v2: transient acceptance (revised draft)

A new file, `wp9-replication-baseline-v2.json`. v1 and every v1 verdict stay
as they are, and v2 is recorded as a contract change, never as v1 passing.

**Unchanged from v1:** 16 m/s and the air state; raw time-averaged Cd within
5% of 0.275 / 0.319 / 0.277 with no fitted shift; the 12M half-cell floor and
the mesh-quality limits; the three-grid study.

**Shape comparison, restated as tolerances.** v1's "estate > fastback ≈
notchback" cannot be an ordering: the reference Fastback–Notchback gap is
0.002, below the ±0.0027 95% interval a single v2 run allows. Draft:

- estate − fastback > 0, with the 95% interval of the difference excluding
  zero (reference gap 0.044);
- |fastback − notchback| ≤ 0.010 (10 counts), an equivalence tolerance fixed
  now, before any run.

**Wash-out:** at least 2 CTU on the grid being scored. Time on another grid,
or before a field mapping, may precede it but never counts toward it.

**Averaging, with a stop rule fixed in advance:**

- minimum 6 CTU, Cd time-weighted because Δt can vary;
- the statistical gate is evaluated every 1 CTU from 6 CTU on, and the run
  stops at the first pass;
- hard cap 15 CTU; reaching the cap without a pass is a fail;
- stopping depends only on the statistical gate, never on Cd relative to the
  band.

**Statistical gate** on the averaging window, from `convergence.rs` on a
series resampled to uniform time:

- **trend:** fail only if the slope is significant at 99% *and* larger than
  0.5% of the mean per CTU (v1's per-1000-iteration unit restated per CTU, so
  it does not depend on the resample spacing);
- **halves:** fail only if the two half-means differ significantly at 99%
  *and* by more than 0.2%. A hard 0.2% limit alone would fail about 84% of
  stationary runs at the 0.5% standard error below;
- effective sample size ≥ 50;
- standard error of the mean ≤ 0.5% of the mean, taking the larger of the
  autocorrelation (Geyer) estimate and batch means with blocks of at least
  1 CTU, because quasi-periodic Cd can fool the autocorrelation cut-off.

**Per-step convergence, replacing v1's 1e-5 RMS rule.** Absolute initial
residuals shrink with Δt, so an absolute limit would reward small steps for
the wrong reason. Draft, in at least 99% of steps:

- p: final-corrector initial residual ≤ 1e-4;
- U, k and omega: initial residual falls at least one order from the first to
  the last outer loop. This requires k and omega to be solved on every outer
  loop (`turbOnFinalIterOnly false`), which the recipe does not do today.

The exact orders of reduction are confirmed by probe B. If probe B can't meet
them, the plan stops and reports; the limits are not loosened.

**Δt sensitivity, a required segment.** After the averaging window passes,
continue from the end state at Δt/2 for 0.5 CTU of adjustment plus at least
2 CTU of averaging. Its mean Cd must agree with the main mean within their
combined 95% interval, and the dominant wake-probe frequency within 10%.
Disagreement is a fail, not a reason to rerun.

**Reported with every result:** peak Courant and Courant percentiles inside a
fixed wake box; DES region fractions (boundary layers RANS, wake LES); wake
eddy-viscosity ratio; and the half-model limitation (section 5).

### 3.2 Time-step policy

Revision 2 drops the maxCo-0.5-everywhere option. Car-body DDES practice fixes
Δt for Courant about 1 in the wake (LES) region and tolerates higher Courant in
thin RANS wall cells. The policy is a fixed Δt with the 95th-percentile wake
Courant ≤ 1, picked from probe B's numerics and confirmed by the Δt/2 segment.
Candidates are 1e-4 s and, if probe B allows, larger. **Decision:** accept
dropping the global option.

### 3.3 Steady force-based route (open conflict)

- **Racing view:** add a steady acceptance judged on forces (fixed averaging
  window across the oscillation, Cd mean and spread, a residual-drop rule),
  with the settings sensitivity reported as uncertainty. Days of compute, and
  Estate and Notchback become cheap. Keep DDES to one Fastback confirmation.
- **Engineering view:** averaging the oscillating steady Cd is a relaxed gate
  in disguise, because Cd moves 3.5% with relaxation alone.
- **Recommendation:** no steady certification route. If the settings spread is
  counted as uncertainty, a steady Cd carries about ±1.7%, which cannot settle
  Fastback against Notchback and spends most of the 5% band. Steady results
  stay as labelled trends (section 3.7). The user may still choose the racing
  option; it would be its own contract file with that uncertainty stated.

### 3.4 First production run (open conflict)

- **Engineering view:** pilot the full gate on the coarsest grid of the
  three-grid study. It is protocol evidence anyway and cheaper per CTU.
- **Racing view:** run the fine Fastback for 3–4 CTU and decide go/no-go in
  about a week. A coarse grid does not test the fine-grid gate, and is a worse
  test of the grey-area risk.
- **Recommendation:** the fine Fastback first, as one run whose early part is
  the go/no-go. At the end of the 2 CTU wash-out, decide go/no-go on non-Cd
  evidence only: DES region fractions, wake eddy viscosity, shear-layer
  breakup, probe spectra and per-step convergence. On a go, the same run
  continues into the averaging window, so nothing is thrown away. On a no-go,
  report the negative result and stop.

### 3.5 Compute

Either this PC (weeks per run, one run at a time; memory rules out a
finer-than-12M DDES), or external HPC or cloud capacity. External compute
needs your explicit authorization: the benchmark geometry and logs would
leave the machine, and the current rules forbid sending logs.

### 3.6 Scope

Either the full protocol in DDES (fine Fastback, Estate, Notchback, two coarser
Fastback grids, plus each Δt/2 segment), or the fine Fastback first and a
go/no-go for the rest after its result.

### 3.7 The steady results' role and how results are shown

The steady results stay on record as "not converged" under v1. The
U-relaxation 0.7 steady Cd (0.2854) is a trend, not a certified value.
Presentation rules (section 4, step 6) apply to every Cd the app or the docs
show.

## 4. Work plan once those decisions are made

Each code step follows AGENTS.md: a failing test first, the minimal change,
then the full checks (`cargo build --workspace`,
`cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`,
the demo cook, NODES.md, an app launch). There is one heavy job at a time,
each with its own run ID. Fingerprinted paths are not edited while a job runs.

**Phase 1: make the transient route measurable (code, no long runs)**

1. **Baseline v2 JSON**, as a new file. Tests pin its values the way
   `extended_gate_uses_the_protocol_acceptance` pins v1.
2. **Transient gate** in `coeffs.rs`, reusing the trend and ESS functions in
   `convergence.rs`. Inputs are variable-Δt `coefficient.dat` rows and the
   per-step, per-outer-loop solver residuals. Tests cover:
   - uneven Δt and time-weighting;
   - the wash-out cut, including mapped or other-grid time not counting;
   - trend per CTU, and the significant-and-large forms of the trend and
     halves tests (a stationary noisy series must pass, a drifting one fail);
   - ESS, and the standard error as the larger of Geyer and batch means
     (a quasi-periodic series must not get an optimistic standard error);
   - the stop rule (pass at the first check that passes, fail at the 15 CTU
     cap);
   - the per-step reduction rule and the p limit;
   - the Δt/2 comparison and the shape tolerances.
3. **DDES recipe** in `foam.rs` / `case.rs`, each change from evidence or this
   review, not from Cd:
   - `div(phi,U)`: replace `Gauss linear` with a hybrid central/upwind scheme
     (DEShybrid, or LUST as the fallback). On this mesh (16–17° median
     non-orthogonality near walls), wiggles can distort results without
     crashing;
   - `turbOnFinalIterOnly false`, PIMPLE outer-loop residual control and the
     corrector counts from probe B;
   - the D5 limiters on grad(k) and grad(omega), which the steady recipe has
     and DDES lacks;
   - a fixed-Δt mode;
   - binary field output, to cut the ~21-minute startup.
4. **Output written during the run.** These can't be recovered after a run,
   so they go in before any multi-week run (about 1 GB extra, within the
   7.2 GB free):
   - `fieldAverage` of U and p with their fluctuation statistics (prime2Mean);
   - time-averaged wall shear stress, which shows the separation lines;
   - time-averaged surface pressure on the rear patches (base pressure);
   - fixed sample planes (y = 0; x = 4.8, 5.5 and 6.5 m) and wake probes for
     the shedding frequency;
   - forces by patch group, for a drag breakdown;
   - the DES region field and the wake eddy-viscosity ratio, for the
     grey-area check.
5. **Runner:**
   - a `wp9-transient-probe` profile (≤ 6 h) and a `wp9-transient` profile
     that runs in **resumable segments**, each within the 48-hour bound,
     building on the existing `ResumeRunId` path;
   - start from a preserved RANS checkpoint, reusing the checkpoint restart
     from 625a5b2;
   - the stop rule from section 3.1, evaluated by the gate code, not by hand;
   - per-step convergence, wake-box Courant percentiles and the DES
     diagnostics in the diagnostic JSON.
6. **How results are shown** (app and docs):
   - never a bare Cd. A trend is rounded and shows its settings spread, for
     example "Cd ≈ 0.28 (0.276–0.285 across solver settings) · trend only:
     steady solve did not settle, wake unsteady", with a hollow badge;
   - a gated value shows its standard error, averaging length and contract
     version, for example "Cd 0.2xx ± 0.00x (1 s.e.) · averaged over N flow
     passes · gate v2 passed", with a solid badge;
   - every result names its contract version, and the "gate v1: not met"
     rows stay visible; v2 never overwrites them;
   - the reference band is drawn as "0.275 ±5%, published replication, not a
     measurement";
   - the three shapes as a strip plot with interval bars, so overlap shows;
     status is carried by shape and text, not colour alone;
   - a running-average Cd plot with the wash-out shaded and the interval
     narrowing;
   - check that the WP9 cases set `separated_flow`, so the existing "trend
     only" warning in `case.rs` reaches users.

**Phase 2: time-step probe (labelled diagnostic, ≤ 6 h)**

- **Probe B:** fixed Δt = 1e-4 s with the Phase 1 recipe (hybrid scheme,
  k/omega on every outer loop, outer residual control), 3–4 outer correctors.
  Measure the per-step reduction against the v2 rule, the p limit, seconds per
  step, peak and wake-box Courant percentiles, wake cell sizes and stability.
  Probe A (maxCo 0.5) is dropped with that policy.
- **Choose Δt** from probe B's numerics only. If probe B can't meet the
  per-step rule, stop and report rather than loosen it.

**Phase 3: fine Fastback production DDES (segments)**

- Wash-out, then the go/no-go of section 3.4 on non-Cd evidence.
- On a go: averaging under the stop rule, then the Δt/2 segment, then the
  transient gate. Report Cd with its standard error against 0.275 ±5% as
  observed.
- If it fails, keep the negative result and do not rerun automatically.

**Phase 4: the remaining protocol evidence** (only if decision 3.6 allows and
Phase 3 passed)

- Estate and Notchback on the fine grid, two coarser Fastback grids at a
  constant refinement ratio, then `cfd.mesh_study` and `cfd.benchmark_check`
  with the shape tolerances.

## 5. Risks to state up front

- **Mesh suitability (grey area).** The 12M mesh was sized and refined for
  RANS. A DES wake needs reasonably isotropic cells in the LES region, and a
  coarse wake makes the averaged Cd grid-dependent. Detected early by the
  Phase 3 go/no-go diagnostics, not after months.
- **The grid study is not a clean refinement in DES.** The model's length
  scale is tied to the cell size, so refining the grid also changes the
  model. The three-grid study is still run, but its result is read with that
  caveat.
- **Half model.** The symmetry plane suppresses side-to-side wake motion.
  This is a stated modelling limitation; a full model does not fit in memory
  here.
- **Memory.** DDES at 12M uses about 18.8 GB of the 26 GB WSL allocation, plus
  about 1 GB for the averaged fields. A finer-than-floor grid will not fit on
  this PC; the coarser study grids do.
- **Cost.** Weeks per run on the fixed-Δt policy, longer if averaging runs to
  the cap. A failure late in a run costs a segment, not the run, once
  resumable segments exist (Phase 1, step 5).
- **Sequential stopping.** Checking the gate every CTU slightly raises the
  chance of a chance pass. The 99% levels, the minimum of 6 CTU and the Δt/2
  segment keep this small; the check interval is fixed in advance.
- **The reference.** 0.275 is a published-replication value, not an
  authenticated experiment (see `wp9-reference-baselines.md`). A DDES result
  outside the band is reported as is, and nothing is tuned toward it.

## 6. Review record (2026-11-11)

Three read-only reviews of revision 1, each from one perspective, no runs or
network access.

- **Engineering:** the halves rule failed stationary signals; absolute
  per-step residuals reward small Δt and miss k/omega; no Δt-sensitivity
  evidence; ESS ≥ 50 in 6 CTU unlikely; drop global maxCo 0.5; use a hybrid
  convection scheme; diagnose the grey area early; state the half-model
  limitation; pilot on the coarse grid.
- **Racing:** months of compute for a certification number is poor value;
  judge convergence on forces; restate the shape comparison as tolerances;
  fine Fastback go/no-go within about a week; list of legitimate cost savings
  and of moves that would cheat the gate (stopping when Cd lands in the band,
  choosing window, Δt, scheme or grid after seeing Cd, loosening limits,
  counting mapped time as averaging).
- **Aesthetics:** never show a bare Cd; keep v1 failures visible; label the
  reference band; write averaged fields, wall shear, base pressure, planes and
  probes during the run; check `separated_flow`.

Adopted into sections 3–5: all of the engineering acceptance and numerics
points, the shape tolerances, the fixed-in-advance and no-cheating rules, and
the presentation and output points. Left for the user: the steady force-based
route (3.3) and the first production run (3.4).

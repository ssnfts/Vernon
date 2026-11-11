# WP9: plan for a time-averaged DDES route to a passing gate

Status: **plan for review, revision 3, 2026-11-11.** The user chose this route
("Plan the DDES route") after the evidence below. Revision 2 folded in a
three-perspective review (engineering, racing, aesthetics; section 6).
Revision 3 records the user's decisions (this PC; a different first
production run), a combined schedule for everything WP9 still owes
(section 7) and a wider vehicle scope (section 8). Nothing here starts a
run, changes a gate or relabels a result. Every step that changes the
accepted contract (`docs/research/wp9-replication-baseline-v1.json`) is
listed as a decision for the user.

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

### 3.4 First production run: decided, a slant-body rehearsal (revision 3)

The user rejected both revision-2 options (fine Fastback first; coarsest
Fastback grid first) and asked for a different kind of first run. Decision:
**the first production run under the v2 gate is the 25° slant body (Ahmed
body) in DDES on the existing ~2.0M-cell grid**, run through the runner with
the full v2 machinery (wash-out, stop rule, Δt/2 segment, during-run outputs,
segments and resume), and reported against the experimental Cd and the soft
G25 slant-share check in `docs/specs/automotive-cfd.md`.

Why this and not a car grid:

- **It has a known answer and a DES literature.** The 25° slant body is the
  standard test of whether a hybrid RANS-LES resolves a separating and
  reattaching wake. The steady runs of 2026-10-31 missed it badly (Cd 0.369
  at 2.03M cells against the published 0.285 family, slant share 40.6% against
  55%, convergence gate not met). If DDES fixes that, the route is
  vindicated in hours; if it does not, the problem is in the recipe or the
  mesh sizing, and weeks on the car would have hidden it.
- **It costs hours, not weeks.** One convective unit is 1.044 / 40 = 0.026 s.
  Scaling the measured 32 s/step at 12.06M cells gives about 5–6 s/step at
  2.03M; a 10.5 CTU run at Δt 1e-4 s is ~2,700 steps, about 4–5 h, and the
  15 CTU cap plus the Δt/2 segment stays under a day.
- **It exercises every piece of v2 on real transient data** before a
  multi-week run depends on it: how fast the effective sample size grows, how
  long the averaging actually takes to pass, whether batch means and the
  autocorrelation estimate disagree, whether the Δt/2 segment agrees, and
  whether the grey-area diagnostics (DES regions, wake eddy viscosity, probe
  spectra) read as expected.
- **It is protocol evidence anyway.** G25 (25° vs 35° ordering and slant
  share) is unmet by the steady runs. The 35° body follows at the same cost.

What it does not establish: anything about the 12M-cell regime (memory,
ranks per cell, I/O) or the car's wake mesh. Probe B (section 4) keeps that
job. The slant body is not part of the v1 contract; it is reported under the
spec's G25 and as a gate rehearsal, never as a DrivAer result.

Code needed first: the DDES recipe today only accepts a wall ground with
fixed or rotating-wall wheels; the slant body has a stationary ground, stilts
and no wheels, so the recipe gains a stationary-ground, no-wheel mode (test
first).

### 3.5 Compute: decided, this PC

The user chose this PC. Consequences: one heavy job at a time; DDES at 12M
uses ~18.8 GB of the 26 GB WSL allocation, so no finer-than-12M DDES; the
serial fine mesh needs 20.7 GB peak, so meshing cannot overlap a solve and is
scheduled between segments; no data or logs leave the machine.

### 3.6 Scope: expanded (section 8)

The user asked to widen the scope to the vehicles already downloaded and to
further open datasets. Section 8 sets out the tiers; the v1 contract itself
is unchanged.

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
   - a stationary-ground, no-wheel mode, so the slant body can run in DDES
     (section 3.4);
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

**Phase 2a: slant-body rehearsal (production, v2 gate, ≤ 1 day each)**

- 25° body in DDES on the 2.03M-cell g3 grid from the RANS end state of the
  2026-10-31 run, then the 35° body. Full v2 machinery. Report Cd ± s.e.
  against the published family, the slant share against G25, and every v2
  diagnostic. Fix gate-code defects found here before Phase 3; a change to the
  gate is a code change with a test, never a tuned limit.

**Phase 2b: time-step probe (labelled diagnostic, ≤ 6 h)**

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
the presentation and output points. Decided by the user in revision 3: this
PC (3.5) and a different first production run (3.4). Still open: the steady
force-based route (3.3).

## 7. Combined schedule: everything WP9 still owes, in the fastest honest order

**What is still owed** (runbook Stage 3, the DDES phases, and loose ends in
the docs):

1. Fastback certified Cd on the fine grid (v2).
2. Estate and Notchback on the fine grid, and the shape tolerances.
3. The three-grid study on Fastback (`cfd.mesh_study`), then
   `cfd.benchmark_check` over the rows.
4. G25 (slant body 25° vs 35°), unmet since 2026-10-31.
5. The surrogate error measurement: the runbook says the OpenFOAM stages are
   "what will measure the surrogate's error" of the AI preview, and nothing
   has measured it yet.
6. Docs: `docs/ROADMAP.md` still lists transient DES as "not planned in this
   repository"; the runbook's Stage 3 still asks for the ordering
   "estate > fastback ≈ notchback"; `NODES.md` and the README counts after
   any node change.

**The ordering rules that make it faster:**

- **All code before any long run.** Fingerprinted paths (scripts, crates,
  app, examples, manifests, the baseline JSON) cannot be edited while a runner
  job runs, and a solve must keep one source fingerprint. So Phase 1 is done
  in full first (gate, recipe, runner segments, outputs, presentation, the
  tolerance form of `benchmark_check`, the stationary-ground DDES mode, and
  the docs in item 6). Estimated 1–2 weeks of code, no CFD. Docs are not
  fingerprinted and can be written during runs.
- **Cheapest known answer first.** The slant-body rehearsal (hours) retires
  the gate-code and grey-area risks before the car is touched.
- **The steady Estate and Notchback runs are on the critical path anyway.**
  Every DDES run starts from a RANS solution, and those two have none. Their
  steady runs (~36 h each on the proven D5 recipe) produce the DDES start
  fields and, labelled as trends, an early estate − fastback ordering (a
  44-count gap against a ~10-count settings spread). They never count as
  certification.
- **Meshing in the gaps.** The Estate and Notchback fine meshes (~45 min
  serial each) are built between solve segments, when the solver's memory is
  free.
- **Coarse Fastback grids before the other shapes.** At a refinement ratio of
  1.3 the two coarser grids are about 5.5M and 3.3M cells and cost roughly
  0.35 and 0.16 of a fine run each (fewer cells and a proportionally larger
  Δt). They answer the grey-area question (does Cd move with the grid?)
  before 6–12 weeks go into Estate and Notchback.
- **Δt is the biggest lever.** Probe B decides it from numerics; every
  doubling halves every car run. Nothing else on this PC changes the cost by
  more than a few percent.
- **No run is thrown away.** Go/no-go at the end of wash-out on non-Cd
  evidence; a go continues the same run; segments make a late failure cost
  a segment.

**The sequence, with honest estimates on this PC (16 ranks):**

| Step | What | Wall time | Cumulative |
|---|---|---|---|
| 1 | Phase 1 code and docs, test-first, full AGENTS.md checks | 1–2 weeks | 1–2 weeks |
| 2 | Slant body 25° then 35° in DDES under v2 (Phase 2a) | ~1 day each | +2 days |
| 3 | Probe B on the fine Fastback (Δt choice) | ≤ 6 h | +1 day |
| 4 | Steady Estate, then steady Notchback (D5 recipe; DDES start fields; trend ordering) | ~36 h each | +3 days |
| 5 | Fine Fastback DDES: wash-out, go/no-go, averaging to the stop rule, Δt/2 segment | 3–6 weeks | 4–8 weeks |
| 6 | Mesh the two coarser Fastback grids between segments; run both in DDES | 1.5–3 weeks | 6–11 weeks |
| 7 | `cfd.mesh_study` on the three Fastback rows; first `benchmark_check` row | hours | — |
| 8 | Estate DDES, then Notchback DDES (from step 4's fields) | 3–6 weeks each | 12–23 weeks |
| 9 | `benchmark_check` with the shape tolerances; surrogate error over every certified row; final record | days | 3–5.5 months |

Step 5 ranges from 21 days (averaging passes at 6 CTU, Δt 1e-4 s, 60–70
s/step) to about 45 days (averaging runs to the 15 CTU cap). A Δt of 2e-4 s
from probe B halves it. Steps 1–7 (about 6–11 weeks) certify the solver route
on Fastback with its grid study; steps 8–9 complete the three-shape protocol.
If the user later wants to stop after step 7, the Estate and Notchback rows
stay as labelled trends from step 4, and the protocol is reported as
incomplete, not as passed.

**What is deliberately not done to go faster:** no shorter minimum averaging,
no stopping on Cd, no counting steady or mapped time as averaging, no
coarse-grid substitute for the fine-grid gate, no external compute.

## 8. Vehicle scope: what is on disk, what to add, and what each is for

**Already on disk** (all under `scratch/`, never committed):

| Set | Files | Reference available | Use |
|---|---|---|---|
| Road-car benchmark, three rear ends (`scratch/wp9-data/tum-drivaer/extracted`) | 8 STL parts, SHA-bound in v1 | Published-replication Cd 0.275 / 0.319 / 0.277 (2012 tunnel, not independently authenticated) | The v1/v2 contract; steps 4–9 above |
| Slant body 25° / 35° (generated by `slant_body`) | geometry from code | Experimental Cd family and slant share (spec G25) | First production run (section 3.4) |
| Open hybrid RANS-LES car dataset, run 1 (`scratch/wp9-data/drivaerml-run1`) | 142 MB STL, forces CSV (Cd 0.3035), geometry parameters, one slice | **Time-averaged hybrid RANS-LES result with full published setup**, plus 16 named styling parameters (backlight angle, decklid height, rear-end tapering, …) | A reference that matches the DDES route's own method; the surrogate was trained on this dataset, so it is also the in-sample check for the preview node |
| Three styled production-car models (`scratch/sketchfab-cars`): a notchback coupe, an estate and a fastback liftback, with 6 mm voxel shells already prepared for the preview node | 3 GLB sources, 3 STL shells | None. Artist models, not engineering CAD: shut lines, underbody and wheels are approximate; the shell fills openings under ~10 cm | Pipeline robustness (prep → mesh → short solve) and a designer-facing demo: the same three rear-end classes as the benchmark, shown as trends only |

**Tiers:**

- **Tier A, certification.** Geometries with a reference that has full
  metadata (speed, air, ground and wheel state, reference area): the
  benchmark trio, the slant body, and the open hybrid RANS-LES datasets
  below. Only Tier A rows reach `benchmark_check`.
- **Tier B, trend and robustness.** The styled production cars and any model
  without a documented reference. Results carry the hollow "trend only"
  badge and never a certified value. Their value is twofold: the mesher and
  prep must survive real artist geometry, and the three bodies mirror the
  benchmark's estate / fastback / notchback classes, which lets the app show
  the ordering on shapes designers recognise. User-visible labels use body
  classes, never the source model or marque names (clean-room rule).

**Candidate additions (open datasets; each download needs the user's
explicit OK, and the licence is read from the dataset card before use):**

| Dataset | Why | Size to fetch |
|---|---|---|
| The open hybrid RANS-LES car dataset: the force table for all runs, plus 4 more geometries chosen to span backlight angle and rear-end tapering | Deltas against a same-method reference on exactly the styling variables designers move; the parametric error of the preview node | force table small; ~140 MB per geometry STL |
| The open slant-body variant dataset (same authors; 500 parametric slant bodies with hybrid RANS-LES forces) | Cheap parametric validation at ~2M cells per run: hours each, so a real error distribution, not one number | small STLs; force table small |
| The open Windsor-body dataset (same authors), plus its experimental baseline | A second experimentally anchored bluff body with a squareback wake; experiment and hybrid RANS-LES on the same shape | small STLs; force table small |

Not proposed: more styled artist models (no reference, so nothing new is
learnt beyond robustness), and any dataset whose licence does not allow
local research use.

**What the wider scope changes in the code (Phase 1 additions):**

- a case binding for each Tier A set like v1's (archive and component
  SHA-256s, preparation revision, air state, reference row), so a row is
  tied to bytes, not a filename;
- `benchmark_check` accepts a reference row with its own method label
  ("experiment", "published replication", "hybrid RANS-LES") and shows it;
- the surrogate-error node compares a certified row with the preview node's
  output at the preview's fixed operating point, and reports it with the
  certified row's interval.

**Order within the wider scope:** nothing in section 8 delays section 7's
steps 1–5. The slant-body variant runs (hours each) fit between car segments
once a car run is in progress and the solver's memory is free; the open-car
dataset rows run after step 9 or when the user chooses to interleave them.


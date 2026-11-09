# CFD validation runbook: Stages 0–3

How to continue WP9 (TUM DrivAer validation) on a machine set up with
[SETUP.md](SETUP.md) parts 1–6. The reasoning, estimates and blockers are in
[the solver-run plan](superpowers/plans/2026-11-04-wp9-solver-run.md). Gates,
recipe and reference stay exactly as accepted in
[wp9-replication-baseline-v1.json](research/wp9-replication-baseline-v1.json):
16 m/s, 5% band, 12M half-model cell floor, three-grid study, strict
1000-sample window convergence. Never lower them to make a run pass, and
never relabel a failed run.

> **2026-11-04 (later): the user resumed Stages 0–3.** Stage 0 is built (see "As built"
> below); progress is in `docs/CONTINUATION.md`.
>
> **2026-11-04: Stages 0–3 are DEFERRED, not cancelled.** An AI preview (DoMINO) now screens
> variants in minutes inside Vernon ([docs/specs/ai-models.md](specs/ai-models.md)), but it is
> labelled "AI prediction — CFD validation pending" and does not satisfy, replace or relax any gate
> below. These stages are still what validates the solver and what will measure the surrogate's error.

## Where things stand

| Done | Result |
|---|---|
| Fine Fastback mesh (dx0 0.32, 5 layers, serial) | 12,056,197 cells, non-ortho 64.996°, skewness 3.452, 0 failed checks, layers 97.3%; 41 min, 20.7 GB peak |
| Full check queue | 1732 passed / 0 failed / 35 ignored; clippy, demo, catalogue clean |
| Solver on the 12M mesh | Stage 2 (2026-11-08) stopped by the user at iteration 2546, `cancelled`, no gate verdict: k/omega clipped every iteration, residuals flat 1e-4..2e-2. Diagnostics 2026-11-09: C0 reproduces Stage 2 exactly; D1 (upwind 1–500) clips nothing until the switch, then clips like C0 (586 vs 611 k cells per iteration) in the same near-wall cells. Upwind startup is not the remedy; location analysis next (see CONTINUATION.md) |
| Stage 1 pilot, 200 iterations, 16 ranks (2026-11-04) | 18.7 s/it (17.2–19.5), solver RSS ~18–21 GiB flat, rank peak 1.5 GiB, 1.0 GB per write; Stage 2 estimate 37–39 h. Details in CONTINUATION.md |

Earlier 3.76M-cell runs (old 30 m/s setup, 16 ranks) took 4.44 s per iteration
and never converged. They are diagnostics only.

## Rules for every long run

- Check `.\scripts\verification\verification-status.ps1 -Json` before starting
  anything. Run one heavy job at a time, and never start a second copy.
- Disable sleep for the whole run, keep the PC on mains power, and close large
  apps. Don't let anything update or restart the app that launched the worker.
- Every run gets a fresh, never-reused `-RunId`. Logs, status and results are
  in `scratch\continuation\runs\<run-id>\`.
- A run counts only if it has exit 0, the profile's completion marker, the
  result row read back, and a positive Linux cleanup record. A partial mesh
  or a missing marker is a failure.
- If a worker disappears without a final status, clean up its WSL processes
  before starting anything else:

  ```powershell
  . .\scripts\cfd\wp9-sizing-cleanup.ps1
  Stop-Wp9SizingLinuxGroups (Resolve-Path scratch\continuation\runs\<run-id>) Vernon-Ubuntu-24.04 vernon
  ```

## Step A: reproduce the fine mesh on the new PC (~45 min)

This confirms the new machine before any new code: same mesh, same gates.

```powershell
. .\scripts\dev-env.ps1
$env:VERNON_CAR_DIR = "$PWD\scratch\wp9-data\tum-drivaer\extracted"
$env:VERNON_WSL_DISTRO = 'Vernon-Ubuntu-24.04'
$env:VERNON_WSL_USER = 'vernon'
$env:VERNON_WSL_ENV_FILE = '/mnt/c/dev/vernon/scripts/cfd/wp9-replication-sizing-env.sh'  # your checkout, as a WSL path
.\scripts\verification\start-verification.ps1 -Profile wp9-sizing-serial -RunId wp9-sizing-serial-newpc-YYYYMMDD-a -JobTimeoutSeconds 21600
```

Pass: status `completed`, exit 0, and the stdout log ends with
`WP9_REPLICATION_MESH_ONLY_COMPLETE rear=Fastback cells=12056197.0 solver_runs=0`.
A different cell count on a different OpenFOAM build isn't automatically
wrong, but record it. Every later grid uses this machine's own numbers.

## Stage 0: code and runner changes (no CFD)

This is development work, done by you or a coding agent that has read
`AGENTS.md`, `CLEAN_ROOM.md` and `docs/CONTINUATION.md`. It's finished when all
of the following are true, each with a test observed failing first:

1. **Separate mesh and solve process counts.** `real_openfoam_road_car_run`
   meshes with its own count (default 1) and solves with `VERNON_CAR_NP`.
2. **Solve deadline.** The solve node timeout is raised from 1800 min (for
   example to 2700), inside an outer job bound with an hour left for cleanup.
3. **Solver environment.** A fixed `scripts/cfd/wp9-replication-solve-env.sh`
   allows only the mesh tools, `topoSet`, `potentialFoam`, `decomposePar -force`,
   `mpirun -np 16 simpleFoam -parallel` and `reconstructPar -latestTime`. Caps:
   22 GiB serial; a provisional 4 GiB per rank until the pilot measures real use.
   It gets its own policy and live probes, like the sizing scripts.
4. **Runner profiles.** `wp9-solve-pilot` (outer bound about 4 h) and
   `wp9-solve` (outer bound about 48 h). Each fixes its rear, dx0, rank count,
   iteration count and environment file, requires a completion marker printed
   after every assertion, and checks Linux cleanup on exit.
5. **Pilot mode.** About 200 iterations, labelled diagnostic. It records
   seconds per iteration, memory and a clean solver `End`. It never reports Cd
   and never feeds the benchmark or the study.
6. **Memory sampler.** Read-only, once a minute, into the run folder: WSL
   MemAvailable and the solver processes' peak resident memory.
7. **A fresh full queue passes.**

### As built (2026-11-04)

| Item | Where | Fixed values |
|---|---|---|
| Run modes | `RoadCarMode` in `crates/vernon-nodes-cfd/src/tests.rs` | `VERNON_CAR_MESH_NP` (default 1), `VERNON_CAR_PILOT` (pilot = 50..1999 iterations, never mesh-only), `VERNON_CAR_MESH_TIMEOUT_MIN`, `VERNON_CAR_SOLVE_TIMEOUT_MIN` |
| Solver environment | `scripts/cfd/wp9-replication-solve-env.sh` | MemAvailable ≥ 23 GiB; 22 GiB `RLIMIT_AS` per serial tool; `mpirun -np 16 simpleFoam -parallel` only, 4 GiB per rank (provisional) |
| Profiles | `scripts/verification/verification.ps1` | both: Fastback, dx0 0.32, 16 ranks, serial mesh. `wp9-solve-pilot`: 200 iterations, mesh 100 min, solve 180 min, outer 6 h. `wp9-solve`: 7000 iterations, mesh 100 min, solve 2700 min, outer 48 h |
| Completion | same | pilot: `WP9_SOLVE_PILOT_COMPLETE … coefficients_reported=0` and a row with no Cd/convergence/benchmark keys. Protocol: `WP9_REPLICATION_SOLVE_COMPLETE … converged=… benchmark_validated=… cd=…` matching the saved row. A protocol run that did not converge still completes and is reported **not converged** |
| Memory sampler | `scripts/cfd/wp9-memory-sampler.ps1` | every 60 s: WSL meminfo, VmRSS/VmHWM of the OpenFOAM programs, host free memory, run-folder bytes → `memory-samples.jsonl`, peaks → `memory-summary.json` (also copied into `status.json`) |
| Stop | `scripts/verification/stop-verification.ps1 -RunId <id> -Reason <text>` | worker kills the step, still runs Linux cleanup, status `cancelled`, exit 130, not resumable |

Tests: `cargo test -p vernon-nodes-cfd tests::road_car_run_mode tests::solver_log_timing`,
`scripts/cfd/test-wp9-solve-policy.sh` and `test-wp9-solve-environment.sh` (run in WSL
as `vernon` with the env file's WSL path), `scripts/cfd/test-wp9-memory-sampler.ps1`,
`scripts/verification/test-verification.ps1`.

Checkpoints: the accepted recipe writes binary fields every 1000 iterations and keeps
the latest two (`purgeWrite 2`) in `processor*/`. The whole case stays in
`scratch\continuation\runs\<run-id>\temp\`. **Automatic resume is not implemented**:
a failed protocol run can be restarted by hand from its latest time directory, but the
certification code does not accept a restarted history.

Launching: `start-verification.ps1` waits only 30 s for the worker's first status, and
the worker first scans `target\` for reparse points. If the launcher reports "has not
published status", check that the worker PID is alive and wait; don't relaunch.

## Stage 1: pilot (~2 h solver estimate; 6 h outer bound)

```powershell
$env:VERNON_CAR_DIR = "$PWD\scratch\wp9-data\tum-drivaer\extracted"
$env:VERNON_WSL_DISTRO = 'Vernon-Ubuntu-24.04'; $env:VERNON_WSL_USER = 'vernon'
$env:VERNON_WSL_ENV_FILE = '/mnt/e/REA/vernon/scripts/cfd/wp9-replication-solve-env.sh'  # your checkout, as a WSL path
.\scripts\verification\start-verification.ps1 -Profile wp9-solve-pilot -RunId wp9-solve-pilot-YYYYMMDD-a
```

Record: seconds per iteration at 16 ranks, peak memory per rank and in total,
time and memory for potentialFoam, decomposePar and reconstructPar, and disk
used per write. **Decision gate:** work out the 7000-iteration time and memory
from these numbers before starting Stage 2. If memory doesn't fit, stop and
report. Don't coarsen the grid.

## Stage 2: Fastback protocol run (~30–36 h estimated)

```powershell
.\scripts\verification\start-verification.ps1 -Profile wp9-solve -RunId wp9-solve-fastback-YYYYMMDD-a
```

7000 iterations, window 1000. Report what the gates say: Cd against 0.275,
convergence pass or fail, residuals. "Not converged" is a real possible
outcome and is reported as such. The harness can't resume, so a failure late
in the run loses the run. Add restart support first if that risk is
unacceptable.

## Stage 3: the remaining evidence (~100+ machine-hours)

Only worth starting if Stage 2 shows the recipe can converge:

- Estate and Notchback on the fine grid (each about as long as Stage 2).
- Two coarser Fastback grids for the three-grid study, with a constant
  refinement ratio (shorter runs).
- `cfd.mesh_study` and `cfd.benchmark_check` over the rows; the benchmark
  requires ordering estate > fastback ≈ notchback as well as the 5% band.

## After each stage

Add a dated section at the top of `docs/CONTINUATION.md`: run id, status,
exit code, source fingerprint, the measured numbers and what they do and don't
establish. Commit the code and docs. Never commit `scratch/` outputs or the
DrivAer data.

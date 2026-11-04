# WP9 first fine-grid solver run — plan, 2026-11-04

Status: proposed, not started. No CFD solver has run on the 12M-cell mesh.
Gates, recipe and reference stay exactly as accepted: 16 m/s published
replication, 5% band, 12M half-model floor, three-grid study, strict
1000-window / 2000-sample convergence. Nothing here lowers or relabels them.

## Starting point (measured)

- Fine mesh: Fastback, dx0 0.32, 5 layers, serial snappyHexMesh. 12,056,197 cells,
  non-ortho 64.996, skewness 3.452, layers 97.3%. Meshing takes 2488 s with a peak
  of about 20.7 GB. Deterministic: two runs gave identical cell counts. Run
  `wp9-replication-serial-sizing-22gib-20261104-a`.
- Host: i9-13900HX, 24 cores (8P+16E) / 32 threads, 31.8 GB. WSL is capped at
  26 GB (25.4 GiB visible) with 7 GiB swap. About 229 GB of disk is free.
- Earlier solver throughput, from the old 30 m/s Fastback at 3,756,304 cells with
  16 MPI ranks (`scratch/wp9-car/temp-fastback-048/.../log.solver`):
  3000 iterations took 13,313 s. That's 4.44 s per iteration, or about
  8.5e5 cell-iterations per second.
- That run's residuals never approached the 1e-5 target (U 0.018, p 0.0024
  after 3000 iterations), and Cd scatter over the last window was 4.3%.

## Estimates (to be replaced by pilot measurements)

Scaling the measured throughput linearly to 12.06M cells at 16 ranks gives about
14.3 s per iteration:

| Iterations | Solver time | With mesh + pre/post (about 1.2 h) |
|---:|---:|---:|
| 200 (pilot) | ~0.8 h | ~2 h |
| 2000 | ~7.9 h | ~9 h |
| 7000 (protocol) | ~27.7 h | ~29 h, likely 30–36 h if scaling is worse than linear |

Solver memory: the codebase's own rule is 1–2 GB per million cells, so 12–24 GB
for the whole job. Serial potentialFoam, decomposePar and reconstructPar on
12M cells are unmeasured.

Disk: about 7 written time directories plus the decomposed copy. Expect tens of GB.

## Blockers in the current code (must fix before any solve)

1. **One `np` for mesh and solve.** `real_openfoam_road_car_run` passes the same
   `VERNON_CAR_NP` to mesh_setup, mesh_build, case_setup and solve_steady.
   With np=16 the mesh would run on 16 MPI ranks. It only fits serially (np2 was
   already out of memory at the old caps). Add a separate mesh np, default 1;
   case_setup already rewrites decomposeParDict for the solve.
2. **Solve timeout.** The solve node is given 1800 min (30 h), about equal to the
   7000-iteration estimate. Raise it, for example to 2700 min, inside a matching
   outer bound.
3. **No solver environment.** wp9-replication-sizing-env.sh deliberately refuses
   every solver. A separate fixed `wp9-replication-solve-env.sh` is needed.
   Allow-list: the mesh tools, topoSet, potentialFoam, `decomposePar -force`,
   `mpirun -np 16 simpleFoam -parallel`, `reconstructPar -latestTime`.
   Caps: serial 22 GiB; per-rank cap set from pilot measurement (the pilot uses a
   provisional 4 GiB per rank, with the 26 GB VM as the aggregate bound).
4. **No runner profile.** The sizing profiles allow at most 6 h and accept only a
   mesh-only marker. Add `wp9-solve-pilot` (outer bound about 4 h) and `wp9-solve`
   (outer bound about 48 h). Each needs its own fixed iterations, rank count,
   environment identity and a completion marker printed after all assertions.
5. **No pilot mode.** The harness requires 2000–7000 iterations, and
   force_coeffs needs a full 1000-sample window. Add a diagnostic-only pilot
   (for example 200 iterations). It records throughput, memory and a clean End
   only. It never computes or reports Cd as a result, and it never feeds the
   benchmark or the study.
6. **Memory evidence.** Add an out-of-band, read-only sampler that logs WSL
   MemAvailable and the solver processes' VmHWM once a minute to the run folder.

Each change gets the usual RED-then-GREEN test, followed by a fresh full queue.

## Stages

- **Stage 0 — code and runner (no CFD).** The blockers above, with tests and a
  full queue. About half a day of work; machine time is minutes.
- **Stage 1 — pilot (~2 h).** Fastback fine mesh, 16 ranks, 200 iterations.
  It must produce: seconds per iteration; peak memory per rank and in total;
  time and memory for potentialFoam, decomposePar and reconstructPar; disk per
  write; a clean solver End; positive Linux cleanup.
  Decision gate: the user sees the measured 7000-iteration time and memory before
  Stage 2 starts. If memory doesn't fit, stop and report; no coarsening.
- **Stage 2 — protocol run (~30–36 h).** Fastback, 7000 iterations, window 1000.
  The outcome is whatever the gates say. Given the old runs, "not converged" is a
  real possibility and will be reported as such.
- **Stage 3 — remaining evidence.** Estate and Notchback fine runs (about the
  same time each). Two coarser Fastback grids for the three-grid study (shorter).
  The whole study is roughly 100+ machine-hours. Plan this only after Stage 2
  shows the recipe can converge.

## Long-run logistics (user actions or approvals)

- **Sleep:** sleep would suspend the solver and trip the wall-clock timeout, as
  happened to the old notchback run. The machine must not sleep for the whole
  run: plugged in, and the power plan set to never sleep. The user changes this;
  Claude doesn't change system settings.
- **Detached launch:** the Codex worker died during a Codex app update. Launching
  from the Claude app carries the same risk. Option: launch via a one-off Windows
  scheduled task, which is independent of any app. That's a persistent system
  entry, so it needs the user's approval.
- **No restart:** the harness can't resume from a written time. A failure late in
  a 30 h run loses the run. Adding resume support is possible, but it's
  out of scope unless the user wants it before Stage 2.
- **Host memory:** with WSL using up to 26 GB, Windows pages heavily if large
  apps stay open. Close large apps during Stages 1–3.

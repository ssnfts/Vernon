# AI models inside Vernon: PartCrafter and DoMINO

Status: implemented 2026-11-04 as library nodes in `crates/vernon-nodes-ai`.
Not yet done: a dedicated viewport legend widget and a persistent model
service (see "Limits and open items").

## What this is, and what it is not

* **PartCrafter** turns one image into several separate mesh parts. It makes
  geometry. It does not make *correct*, *scaled*, *watertight* or
  *simulation-ready* geometry.
* **DoMINO (DrivAerML)** is a fast surrogate for road-car external aerodynamics.
  A forward pass that runs, or a prediction that looks plausible, is not CFD
  accuracy. **OpenFOAM validation is DEFERRED**, not passed, not replaced.
  Every DoMINO output is labelled **"AI prediction — CFD validation pending"** and
  carries `cfd_validation = DEFERRED`. No `ai_*` metric is read by any CFD
  acceptance gate, and no node here sets a gate to pass.

The long OpenFOAM Stages 0–3 (`docs/CFD-RUNBOOK.md`, `docs/superpowers/plans/2026-11-04-wp9-solver-run.md`)
are **deferred, not cancelled**. Their purpose is unchanged: they are the only
source of validated numbers (mesh study, DrivAer replication, grid
convergence) and the only way to measure how far the surrogate is off for
Vernon's shapes. Use the AI preview to screen many variants, then spend solver
time on the few that matter.

## Running it in Vernon

Both features are ordinary nodes, so they cook, cache, save in `.vrn` files and
undo like any other node. Each model node has a **Run model** one-shot switch.
It starts the model; without it the node only checks inputs and reloads an
earlier result for identical inputs. Cancel is the app's normal cook cancel (it
kills the whole model process tree); a time limit parameter also stops a job.

Set the install folder once with the `VERNON_AI3D_ROOT` environment variable
(for example `E:\AI-3D`) or per node in **AI-3D folder**. The WSL distribution
that holds the models is `AI-3D` (parameter **WSL distribution**; leave blank
on Linux).

Typical chain:

```
AI Parts from Image (geo.ai_partcrafter)  ->  Select AI Part (geo.ai_select_part)
   -> [clean / decimate / reposition as needed]
   -> AI Aero Preview (cfd.ai_aero_preview)  --summary-->  AI Aero Compare (cfd.ai_aero_compare)
```

You can also feed AI Aero Preview any imported car mesh. Outputs of AI Aero
Preview: painted surface (`Cd` colours; attributes `ai_p`, `ai_tau`,
`ai_tau_mag`), a volume slice point cloud (`ai_U`, `ai_speed`, `ai_p`,
`ai_nut`), a report, and a summary JSON that includes the colour legend (range,
unit, ramp).

### Node reference

| Node | Id | Inputs | Outputs |
|---|---|---|---|
| AI Parts from Image | `geo.ai_partcrafter` | image path (parameter) | parts geometry (prim `part`, `part_name`, groups `part_NN`), report, manifest JSON |
| Select AI Part | `geo.ai_select_part` | parts geometry | one part, report |
| AI Aero Preview | `cfd.ai_aero_preview` | geometry in mm | surface, slice, report, summary JSON |
| AI Aero Compare | `cfd.ai_aero_compare` | two summaries | report, comparison JSON |

## PartCrafter contract

* Weights: `wgsxm/PartCrafter` (object) and `wgsxm/PartCrafter-Scene` (scene),
  MIT. Revisions are recorded from the install's download manifest into each
  manifest JSON (object `69a0ffc1dad5e48e7e5ed91c0609f2b1276eb31f`).
* Input: a pre-cropped image of one object or one scene, 1–16 parts, seed,
  quality (`draft` = 10 steps, 512 tokens, octree 6/7; `standard` = chosen
  steps/tokens, octree 8/9), optional CPU offload.
* Output scale is **arbitrary**: Vernon rescales the assembly so its longest side
  equals the *Longest dimension* parameter. Orientation is the model's own.
* The RMBG-1.4 background remover that ships with the install is **not** used
  (its licence is not Apache/MIT-compatible).
* Preserved in the node's work folder `partcrafter_<hash>/`: `input.<ext>`,
  `request.json`, bridge script, `result.json`, `manifest.json`, `part_NN.obj`,
  `progress.jsonl`, `stderr.log`. The manifest records image SHA-256, seed,
  steps, tokens, offload, model revision and timings.
* Per-part open-edge counts are reported; empty parts are skipped with a
  warning. Parts are never described as simulation-ready.

## DoMINO contract (read from the installed checkpoint)

Source: `nvidia/domino_drivaerml`, revision
`35b1bf1edafdaa2600d16182825890cd51c07427`, checkpoint `0.501`, PhysicsNeMo
1.3.0, NVIDIA Open Model Agreement (commercial use allowed; read it before
redistributing weights; Vernon never bundles them).

* **Training data:** 436 morphed variants of one DrivAer notchback from
  DrivAerML (hybrid RANS/LES). It has no idea about other car families, wings,
  open wheel cars or arbitrary parts.
* **Frame:** metres, flow along **+x**, z up, nose toward −x. Surface box
  x −1.5..5.0, y ±1.4, z −0.32..1.4 m; volume box x −3.5..8.5, y ±2.25,
  z −0.32..3.0 m. Training surfaces span x −0.94..4.13, y ±1.13, z −0.32..1.23 m.
* **Operating point:** fixed 30 m/s inlet, 1.205 kg/m³ air, zero yaw. These are
  the only values in the checkpoint's `global_parameters`; Vernon exposes no
  control for them, and says so in every report.
* **Surface path:** one prediction per triangle: pressure and wall shear
  (3 components). **Volume path:** at supplied points: velocity (3), pressure,
  turbulent viscosity. Vernon offers a y = const slice (default centreline).
* **Units:** outputs are the checkpoint's min–max scaling inverted, i.e.
  **dimensionless model units**. The mapping to pascals or m/s has not been
  verified, so none is applied, and **no drag, lift or force is shown**: no
  verified integration exists. Do not read model units as Pa.
* **Preparation (`prep.rs`):** mm → m; optional translate-only "snap to training
  frame" (nose to the training front, centreline, ground plane); checks for
  finite coordinates, ≥ 8 and ≤ 20,000 triangles, bounding-box domain, axis
  order, open/non-manifold edges, degenerate triangles, piece count. A
  geometry outside the checkpoint's surface box is **refused** unless *Allow
  outside domain* is on; then results are labelled out of domain. Inside the box
  but outside the training span or much smaller than a car: labelled
  *OUT OF TRAINING DISTRIBUTION*. The size and small-car tolerances are Vernon
  heuristics, not NVIDIA values. Passing the checks never means "validated".
* Slice points inside a watertight body are removed; on a leaky mesh they are
  kept and the report says so. The report also states the share of predictions
  outside the value range seen in training.
* Artifacts per run in `aero_<hash>/`: `input.stl` (the prepared mesh),
  `request.json`, bridge script, `result.json` (revision, checkpoint SHA-256,
  timings, VRAM, RSS, training value ranges), `surface_fields.f32`,
  `volume_fields.f32`, `predictions.npz`, `summary.json`, logs.

## Measured resources (this PC: RTX 4090 Laptop 16 GB, WSL 26 GB)

These are single runs, not statistics. Another program was holding ~11 GB of
the GPU during part of the work, which affects timing.

| Workload | Triangles / points | Inference time | Peak GPU | Peak RAM |
|---|---|---|---|---|
| DoMINO surface | 4,992 | 0.7 s (warm), 17.2 s (first run after model load) | 1.29 GB | 2.06 GB |
| DoMINO surface | 19,800 | 22.1–24.2 s | 6.35 GB | 2.05 GB |
| DoMINO surface | 33,540 | **failed** (GPU error) | — | — |
| DoMINO volume slice | ~1,100 points (any mesh ≤ 19,800) | 0.2–2.3 s | 3.32 GB | 2.05 GB |
| PartCrafter object, draft, 3 parts | — | generate 7.9 s and 38.5 s (two runs) | 4.31 GB | 3.42 GB |

Model loading dominates short jobs: DoMINO `both` took ~145–163 s end to end per
node run (two checkpoints loaded each time); PartCrafter loading took 434 s
the first time (cold read from the USB drive) and 73 s when cached. Surface memory grows with the
square of the triangle count (all-pairs distances inside the model); the fit
`0.95 + 1.38e-8 × N²` GB matches both measured points and predicts failure at
33,540, so the preflight uses it with a 25 % margin, and the node refuses meshes
over 20,000 triangles. `standard` PartCrafter quality and scene mode on a real
GPU load were not re-measured here; the earlier install smoke test (scene,
smoke settings) took 366 s. These figures are inference speed only. **There is no
accuracy or speed-up-versus-OpenFOAM figure in this document, because none has
been measured.**

## Safety behaviour

* Free GPU memory (`nvidia-smi`) and RAM in the model environment (`free -m`)
  are checked before loading; the node fails with the numbers if short.
* Missing install, WSL, GPU, bad paths, model errors, timeouts and cancels each
  produce a distinct message. Model crashes can only kill the child process.
* A geometry rejected before inference never starts a process.

## Limits and open items

* No dedicated legend widget in the viewport yet: colour comes from the `Cd`
  attribute and the legend (range, unit, ramp) is in the summary JSON.
* Each run reloads the model (see timings). A resident local service would
  remove that cost; not built.
* No comparison of the surrogate with OpenFOAM exists. Blocker: the deferred
  Stages 0–3. When Stage 2 completes, compare the same shape and report the
  error; do not tune the surrogate until it agrees.
* Opt-in/user-accepted licences: DoMINO weights (NVIDIA Open Model Agreement).
  PartCrafter code and weights are MIT. See `THIRD_PARTY_LICENSES.md`.

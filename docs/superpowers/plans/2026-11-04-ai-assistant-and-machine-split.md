# AI models, terminal and chat assistant, plus which PC runs what. Plan, 2026-11-04

Status: proposed, nothing built. This plan covers two things:

1. **Part A** decides which PC runs the four CFD stages and which one does AI work.
2. **Part B** is a step-by-step plan for adding open AI models (image to 3D, text
   to 3D, photos to 3D, 3D to CFD preview, chat) to Vernon. It also covers a
   built-in terminal and a chat assistant that picks the right model for the
   user's goal and hardware.

The model data comes from a Hugging Face API scan on 2026-11-04: download counts,
licences, file sizes and model cards. VRAM figures marked *(card)* come from the
model card. Figures marked *(verify)* weren't measured here and must be checked on
the real GPU before they're relied on.

---

## Part A: which PC does what

### The two machines

| | **PC 1: this laptop** (measured) | **PC 2: the other PC** (from the user) |
|---|---|---|
| CPU | i9-13900HX, 24 cores (8P+16E), 32 threads | i7-7700K, 4 cores / 8 threads (2017) |
| GPU | RTX 4090 Laptop, 16 GB, Ada (sm_89) | 11 GB card, almost certainly a GTX 1080 Ti (Pascal, sm_61) *(verify)* |
| RAM | 31.8 GB, WSL capped at 26 GB | **unknown, needed for the decision below** |
| Memory bandwidth | DDR5, dual channel | DDR4-2400, dual channel (about half) |

### The key fact: CFD doesn't use the GPU

OpenFOAM (`snappyHexMesh`, `simpleFoam`) runs on **CPU cores and RAM only**. The
11 GB GPU in PC 2 doesn't help CFD at all. For CFD, the deciding numbers are core
count, memory bandwidth and RAM size. PC 1 wins on all three.

What the CFD stages need (from `docs/CFD-RUNBOOK.md` and the solver-run plan):

- The fine 12M-cell mesh peaked at **20.7 GB** in one process. The WSL policy
  needs **at least 23 GiB available inside WSL**, so the host needs 32 GB or more.
- The protocol fixes **16 MPI ranks** (`mpirun -np 16 simpleFoam -parallel`). The
  7700K has 4 cores. Running 16 ranks on it would oversubscribe the CPU badly.
  Running 4 ranks would change the protocol's fixed rank count and its
  decomposition.
- Stage 2 is estimated at **30–36 h on PC 1**. PC 2 has about 4.5× fewer cores and
  about half the memory bandwidth. It would likely take **3–5 days** for the same
  run *(estimate, not measured)*. A crash late in that run loses everything,
  because the harness can't resume.

### Recommendation

| Work | Run it on | Why |
|---|---|---|
| **CFD Stage 0**: code and runner changes, tests | **PC 1** | Minutes of machine time. The WSL caps and policy checks are tuned to this machine. |
| **CFD Stage 1**: 200-iteration pilot (~2 h) | **PC 1** | It measures the 16-rank throughput that Stage 2 is planned from, so it has to run on the Stage 2 machine. |
| **CFD Stage 2**: Fastback 7000 iterations (~30–36 h) | **PC 1** | Only machine with 16+ cores and 32 GB. |
| **CFD Stage 3**: Estate, Notchback, three-grid study (~100+ h) | **PC 1** | Every grid in the study has to use the same machine, OpenFOAM build and rank count, or the grid-convergence numbers aren't comparable. |
| **AI development and testing** (Part B) | **PC 2** | It's free while PC 1 is busy for days. Its 11 GB GPU runs the small and medium models. |
| Heavy AI models (>11 GB VRAM) | **PC 1, only when no CFD run is active** | WSL takes 26 GB of PC 1's 32 GB during CFD. Loading a multi-GB model at the same time risks paging or an out-of-memory failure that kills the CFD run. |

**In short, PC 1 does all four CFD stages, and PC 2 becomes the AI box.** That
matches the hardware: the CFD work needs cores and RAM, and the AI work needs a
GPU that isn't busy.

When PC 2 *could* do CFD: if it has **32 GB or more** of RAM, it can run
*exploratory, non-protocol* cases. Examples are quick coarse meshes, smoke tests
of a new script, or trying an OpenFOAM setting before spending PC 1 time on it.
Results from PC 2 must never be mixed into the protocol's study or benchmark
rows. With 16 GB, it can't even mesh the fine grid.

### Before Stage 1 on PC 1 (user actions)

- Plugged in. Power plan set to never sleep, for the whole run. Sleep killed the
  old Notchback run.
- Close large apps. Don't run AI models on PC 1 while a CFD stage is running.
- Laptop cooling: a 30–36 h run at full load should sit on a hard surface, or
  better a cooling stand.

### To finish the decision, run this on PC 2

```powershell
Get-CimInstance Win32_Processor | Select Name,NumberOfCores,NumberOfLogicalProcessors
Get-CimInstance Win32_VideoController | Select Name,DriverVersion
[math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory/1GB,1)
nvidia-smi --query-gpu=name,memory.total,driver_version,compute_cap --format=csv
```

---

## Part B: AI inside Vernon, step by step

### What the user gets

1. A **terminal panel** inside Vernon, like 3ds Max's MAXScript listener and a
   shell in one. It runs `vernon-cli` commands and normal shell commands.
2. A **chat assistant panel**. The user says what they want ("turn this photo of
   a wing mirror into a printable part", "estimate the drag of this bumper").
   The assistant works out the task, checks this PC's hardware, and recommends
   the best model that fits. It shows VRAM, time, licence and download size, and
   asks before downloading anything.
3. **AI nodes** in the node graph: image→3D, text→3D, photos→3D, 3D→CFD preview.
   They output meshes that feed Vernon's existing clean → surface → print / CFD
   pipeline.

### Models found on Hugging Face (2026-11-04 scan)

Licence matters because Vernon is Apache-2.0 and open source. The **Ship** column
says whether a model can be offered as a built-in option:

- **yes**: MIT or Apache-2.0.
- **opt-in**: the licence has restrictions. Vernon can offer a download, but the
  user must accept the licence themselves, and it's never bundled.
- **no**: non-commercial (`cc-by-nc`). Research use only, not shipped.

#### Image → 3D mesh (the main feature)

| Model | Licence | Ship | Weights | VRAM | Notes |
|---|---|---|---|---|---|
| `microsoft/TRELLIS.2-4B` | MIT | yes | 16.2 GB | 24 GB official *(card)*; less via GGUF | Most downloaded (2.7M). Mesh with PBR materials, arbitrary topology. Card warns of small holes, so Vernon's repair nodes are needed afterwards. |
| `ilintar/trellis2-gguf` + **trellis.cpp** | MIT (source model) | yes | ~16.5 GB | lower, quantised *(verify)* | **Native C++/GGML runtime with CUDA and Vulkan backends, no Python.** Prebuilt Windows binaries. Best fit for a Rust app, and Vulkan may run on the Pascal card. |
| `microsoft/TRELLIS-image-large` | MIT | yes | 3.3 GB | ~16 GB *(verify)* | TRELLIS v1, smaller and well tested. |
| `stabilityai/TripoSR` | MIT | yes | 5.3 GB | ~6 GB *(verify)* | Fast, low VRAM. **The fallback for PC 2 / low-end GPUs.** |
| `TencentARC/Pixal3D` | MIT | yes | 46 GB | has a `--low_vram` mode *(card)* | New (Apr 2026). Needs NATTEN compiled for the GPU arch. |
| `wgsxm/PartCrafter` | MIT | yes | 4.0 GB | *(verify)* | Generates **separate parts** in one shot. Useful for body-kit pieces. |
| `speridlabs/idol` | Apache-2.0 | yes | 3.6 GB | 7.2 GB, <2 s *(card)* | Outputs a Gaussian splat (`.ply`), not a mesh, so it needs a splat→mesh step. Needs two gated image encoders. Pascal has no native bf16 *(verify)*. |
| `tencent/Hunyuan3D-2.1` / `-2mini` / `-Omni` | Tencent Hunyuan community | opt-in | 15–26 GB | 2mini is the light one *(verify)* | Strong quality. The licence excludes the EU, UK and South Korea and caps large-scale use. Never bundle. |
| `stabilityai/stable-fast-3d` | Stability community | opt-in | 4.0 GB | ~6 GB *(verify)* | Fast. Licence limits commercial use by revenue. |
| `facebook/sam-3d-objects` | other, gated (manual approval) | opt-in | 13.3 GB | *(verify)* | Objects from a photo of a real scene. |

#### Text → 3D

| Model | Licence | Ship | Notes |
|---|---|---|---|
| `microsoft/TRELLIS-text-xlarge` / `-large` / `-base` | MIT | yes | Direct text→3D (TRELLIS v1). |
| Two-step: text→image model → image→3D | depends on image model | — | Usually better quality than direct text→3D. The image model (and its licence) still has to be chosen. Not part of this scan. |
| `FreedomIntelligence/BlenderLLM` | Apache-2.0 | yes | Text → Blender Python script → CAD-like model. |
| `Zhengyi/LLaMA-Mesh` | Llama 3.1 | opt-in | LLM that writes meshes as text. Low detail. |

#### Photos / scans → 3D (helps the Capture stage next to COLMAP)

| Model | Licence | Ship | Notes |
|---|---|---|---|
| `facebook/map-anything-apache` | Apache-2.0 | yes | Multi-view metric 3D reconstruction. |
| `depth-anything/DA3-BASE` | Apache-2.0 | yes | 0.5 GB, depth from images. Runs anywhere. |
| `facebook/VGGT-1B`, MASt3R / DUSt3R | cc-by-nc | no | Strong, but non-commercial. |

#### 3D → simulation (fast CFD preview)

This is the most relevant category for Vernon. NVIDIA published PhysicsNeMo
models on Hugging Face in April 2026, **trained on DrivAerML**, the same DrivAer
family Vernon's WP9 replicates. Given a car surface, they predict surface
pressure, wall shear stress and (for some) the volume flow field, in seconds
rather than hours.

| Model | Licence | Ship | Predicts |
|---|---|---|---|
| `nvidia/domino_drivaerml` | NVIDIA Open Model License | opt-in *(verify terms)* | Surface pressure and wall shear, plus volume velocity and pressure |
| `nvidia/geotransolver_drivaerml` | same | opt-in | Surface and volume fields |
| `nvidia/transolver_drivaerml` | same | opt-in | Surface and volume fields |
| `nvidia/xmgn_drivaerml_surface`, `nvidia/figconvnet_drivaerml_surface` | same | opt-in | Surface fields |
| `EmmiAI/AB-UPT` | cc-by-nc, no weights on the Hub | no | — |

**Rule:** a surrogate result is always labelled **"AI preview, not validated."**
It never feeds `cfd.benchmark_check`, `cfd.mesh_study` or any gate. Its job is to
screen many body-kit variants quickly so that only the best go to a real
OpenFOAM run. It's only trustworthy for car-like shapes close to its training
data, so Vernon should warn when the input is far from a DrivAer-style car.

#### Mesh clean-up and CAD

| Model | Licence | Ship | Notes |
|---|---|---|---|
| `Yiwen-ntu/MeshAnythingV2` | MIT | yes | Turns dense AI meshes into clean, artist-style low-poly meshes. Pairs with the image→3D models. |
| `maksimko123/cadrille` | cc-by-nc | no | Point cloud / image → CadQuery CAD. Relevant to the STEP writer, but non-commercial. |

#### Chat model for the assistant

A local model keeps everything offline. Options from the scan:
`openai/gpt-oss-20b` (Apache-2.0, fits about 16 GB in its native 4-bit format
*(verify)*), and the Qwen3 / Gemma 4 families in GGUF through llama.cpp. For
PC 2's 11 GB, use a 7–9B model at Q4. An optional cloud model, for example via
the Claude API with the user's own key, can do better planning. It's off by
default and the user turns it on.

### Hardware tiers the recommender uses

| Tier | Example | What it can run |
|---|---|---|
| **A**: 16 GB+, Ampere or newer | PC 1 (RTX 4090 Laptop) | Everything above except official full-precision TRELLIS.2 (24 GB). Use trellis.cpp quantised instead. |
| **B**: 8–12 GB, Pascal/Turing | PC 2 (11 GB, sm_61) | TripoSR, TRELLIS v1 *(verify)*, trellis.cpp via Vulkan *(verify)*, Hunyuan3D-2mini shape *(verify)*, DA3, 7–9B chat at Q4. **Pascal limits:** no native bf16, no FlashAttention. PyTorch dropped Pascal from its CUDA 12.8+ builds (from 2.8). The last CUDA 12.6 builds are PyTorch 2.14, so pin `torch<=2.14` with cu126. Native GGML/Vulkan runtimes avoid this problem. |
| **C**: no usable GPU | | DA3 and small chat models on CPU. Image→3D is "not recommended, too slow" or cloud-only. |

### Architecture (fits the existing crates)

```
app/  (egui)
 ├─ Terminal panel ──────────┐       runs vernon-cli + shell
 ├─ Chat panel ──► vernon-ai (new crate)
 │                  ├─ hwprobe     GPU name, VRAM, compute capability, driver,
 │                  │              CPU cores, RAM, free disk, "CFD run active?"
 │                  ├─ registry    models.toml: task, repo id, licence class,
 │                  │              min VRAM, min compute cap, backend, size
 │                  ├─ recommend   filter by hardware + licence, rank, explain
 │                  ├─ intent      chat model → {task, inputs, constraints} (JSON)
 │                  └─ runners     external processes (same pattern as the
 │                                 OpenFOAM extproc): trellis.cpp binary, or a
 │                                 per-model Python venv managed by `uv`
 └─ Node graph ◄── new nodes in vernon-nodes-ai: ai.image_to_3d, ai.text_to_3d,
                   ai.photos_to_3d, ai.cfd_preview, ai.retopo
```

Design rules:

- **Models run out-of-process**, like OpenFOAM today. A crashing or out-of-memory
  model can't take Vernon down, and Python never gets linked into the Rust app.
- **Nothing downloads without a confirm dialog** that shows size, licence and
  destination folder. Licences marked "opt-in" need the user's explicit accept.
- **Model weights live outside the repo**, for example under
  `%LOCALAPPDATA%\Vernon\models`, and are never committed.
- **Clean-room rules still apply:** these are separate open models, called
  through their published interfaces, and no Houdini material is involved.
- **Refuse heavy AI jobs while a CFD run is active on the same machine.** The
  check can be: a WSL OpenFOAM process exists, or the verification runner's
  status says "running".

### Step-by-step build order

Each step is small, testable, and leaves Vernon working. PC 2 does steps 1–9
while PC 1 runs CFD.

1. **Set up PC 2.** Run `docs/SETUP.md` (Rust toolchain only; WSL/OpenFOAM is
   not needed), install the NVIDIA driver, check `nvidia-smi`. Record its specs
   in this file.
2. **Hardware probe** (`vernon-ai::hwprobe`). It reports GPU, VRAM, compute
   capability, CPU, RAM, disk and active-CFD status. Exposed as
   `vernon-cli ai hw`. Tests use fixed fake hardware profiles: PC 1, PC 2, no GPU.
3. **Model registry** (`models.toml`), seeded from the tables above, with a
   licence class per model. Exposed as `vernon-cli ai models`. A test checks
   that every "yes" entry really is MIT or Apache.
4. **Recommender.** Given a task and hardware, return a ranked list, each with a
   "why" line ("TripoSR: fits in 11 GB; TRELLIS.2 needs 24 GB"). Exposed as
   `vernon-cli ai recommend image-to-3d`. Table-driven tests for tiers A, B and C.
5. **First runner, TripoSR** (the smallest MIT model, so it runs on PC 2). Use a
   managed `uv` venv with pinned versions (torch ≤2.14 cu126 on Pascal). Add the
   download confirm dialog. Output a `.glb`/`.obj` and import it with
   `vernon-io`. Verified when a photo becomes a mesh that `clean` nodes accept.
6. **trellis.cpp runner.** Download the prebuilt binary and the GGUF weights,
   and pass the backend (`--backend Vulkan` or CUDA). First measure whether it
   fits on PC 2 via Vulkan, and record VRAM and time. On PC 1 it's the
   high-quality default when no CFD is running.
7. **Terminal panel** in the app. An embedded terminal widget runs `vernon-cli`
   and a shell, with output captured to the session log.
8. **Chat panel and intent.** A local llama.cpp server (OpenAI-compatible API)
   with a 7–9B model on PC 2. The model only fills a strict JSON schema
   `{task, inputs, constraints}`. Vernon code, not the model, picks the model to
   run, so a wrong answer can't trigger downloads or commands by itself. Every
   action shows a confirm card.
9. **Nodes:** `ai.image_to_3d`, `ai.text_to_3d` (TRELLIS-text first),
   `ai.retopo` (MeshAnythingV2), `ai.photos_to_3d` (map-anything-apache, DA3).
10. **CFD preview node** (`ai.cfd_preview`, a DoMINO or GeoTransolver surrogate).
    Read the NVIDIA Open Model License first. Check the surrogate against PC 1's
    real OpenFOAM results once Stage 2 finishes. The aim is to measure how far
    off it is, not to tune it until it agrees. Always labelled "AI preview, not
    validated".
11. **Opt-in licences:** Hunyuan3D, Stable Fast 3D, SAM 3D. Download-on-accept
    only, with region and licence warnings.
12. **Docs and release:** a `docs/specs/ai-models.md` contract, third-party
    licence entries via `scripts/third-party-licenses.ps1`, and a NODES.md
    regeneration.

### Open questions for the user

- PC 2's RAM, and confirmation that the 11 GB card is a GTX 1080 Ti.
- Should a cloud chat model (your own API key) be offered, or local only?
- Does Vernon need to stay usable commercially? If yes, keep the "no"
  (`cc-by-nc`) models out entirely, and treat "opt-in" ones as user-installed
  only.

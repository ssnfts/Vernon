# AI Sketch from Words (`geo.ai_sketch_from_words`)

Emulates: text to image generation.

## What it does
Draws one concept picture from a text prompt with the FLUX.2-klein-4B model, run
as a separate Python process on the GPU inside the AI-3D WSL distribution.
Outputs: `image` (a PNG file), `report` and `manifest` (JSON with the inputs,
model revision and measurements).

The picture is an idea sketch to look at. The label "AI picture: a concept
sketch, not a design or a measurement" is the first line of the report,
`advisory` is always `true` in the manifest, and report metric names all start
with `ai_`. The node sets no pass/fail and no gate may read its output.

## What it does not do
- It makes no geometry. Nothing in the picture is to scale, and no size, count
  or proportion in it can be relied on.
- It can ignore parts of a prompt, add things nobody asked for, and draw text
  wrongly.
- It draws from words only: no picture input, no editing of an existing
  picture, one picture per run.
- It never downloads anything while running.

## Licence
Model black-forest-labs/FLUX.2-klein-4B: Apache-2.0, not gated (licence tag and
gating read from the model repository on 2026-11-16). Python packages:
`torch`, `diffusers`, `transformers`, `accelerate`, `pillow`, `numpy`,
`huggingface_hub`, `safetensors`.

## Install
`scripts/ai/install_sketch.sh --yes [weights_dir]`, run as root inside the WSL
distribution. It prints model, licence and size, and refuses to run without
`--yes`. It creates `/opt/ai3d/Sketch` (Python 3.11), installs the packages,
downloads the pipeline folders (text encoder, tokenizer, transformer, picture
decoder, scheduler; about 15 GB, not the 24 GB of the whole repository, which
also holds a single-file copy and sample pictures) and checks every large file
by SHA-256 against the hashes the model repository publishes. The files go to
`<weights_dir>/model` (default `/mnt/e/AI-3D/Sketch`), with
`installed-linux-packages.txt` and `model-revision.txt` beside them. The
download folder `/opt/ai3d/Sketch/dl` keeps a second 15 GB copy; it can be
deleted after the install. Revision installed:
`e7b7dc27f91deacad38e78976d1f2b499d76a294`.

## Inputs
| Parameter | Meaning |
|---|---|
| `ai_root` | AI-3D folder; blank uses `VERNON_AI3D_ROOT`. Must contain `Sketch`. |
| `wsl_distro` | Default `AI-3D`. Blank runs Python directly (Linux). |
| `prompt` | What to draw, in plain words; empty is an error. |
| `width`, `height` | 256 to 1024 pixels, multiples of 16, default 768. |
| `steps` | 1 to 8, default 4. The model is made for about 4. |
| `seed` | Same seed and inputs draw the same picture on the same machine. |
| `timeout_s` | Process is stopped after this long (default 1800). |
| `run` | Action switch. Without it the node only reloads an earlier picture. |

The work folder is `sketch_<fnv1a hash of prompt, size, steps and seed>`.
Identical inputs reload the saved picture without starting the model.

Before starting, the node asks the environment for free GPU memory and RAM and
refuses when there is less than the measured peak plus a margin. The bridge
reads the weight files once from start to end, then keeps every part on the GPU
when they fit; otherwise it moves one part at a time to the GPU and keeps the
rest in RAM (it reports which as `offload`).

Errors are distinct: empty prompt, bad size, bad step count, missing install,
missing weights, no GPU, out of GPU memory, model error (with the model's own
message), timeout, and a result that is not a PNG of the requested size.

## Measured (single run, 2026-11-16, not a benchmark)
RTX 4090 Laptop (16 GB), WSL with 25 GB of RAM. Ignored test
`real_sketch_draws_red_ball`, fresh work folder: a prompt for one red ball on a
white background, 512 x 512, 4 steps, seed 7.

- The picture is a red ball on white. Middle fifth of the picture: mean colour
  R 245, G 16, B 24. All four corners: above 253 in every channel. The test
  asks for red at least 1.5 times green and blue in the middle and corners
  lighter than 170.
- Ran with `offload` on (14.9 GB of weights against 14.7 GB of free GPU
  memory). Peak GPU memory 7.8 GB, peak RAM 16.7 GB.
- Reading and loading the weights 110 s; drawing 228 s: the four steps took
  70 s, almost all of it the first one, and the rest was the text encoder, the
  picture decoder and moving parts between RAM and the GPU.
- The whole node call took 2553 s. About 2200 s of that was Python starting and
  importing its packages from the environment's drive, which other jobs had
  saturated at the time. That figure says nothing about a quiet machine, which
  was not measured.
- The weights were read from a copy on the internal drive, because the install
  drive was too slow to load 15 GB within an hour on that day (an earlier
  attempt from the install drive hit a 3600 s limit before the model loaded).
  The copy's five large files match the published SHA-256 hashes, as the
  installed files did when the install script checked them.
- `diffusers` 0.43.0, `torch` 2.15.0+cu132.

## Limits
- Tested with one prompt, one size and one seed. Picture quality on real design
  prompts is not assessed, and 768 and 1024 pixel sizes were not run.
- The all-on-GPU path (a card with about 19 GB free) was not run; only the
  `offload` path was.
- Needs about 17 GB of RAM in the Linux environment when `offload` is on, and
  the resource check asks for 21.4 GB free. A PC with 16 GB of RAM can't run it
  this way.
- Every run starts the model again; there is no server that stays loaded.
- Heavy GPU and RAM load: don't run it while a CFD job is active.

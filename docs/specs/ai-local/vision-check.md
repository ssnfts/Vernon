# AI Vision Check (`geo.ai_vision_check`)

Emulates: visual question answering on a render or photo.

## What it does
Shows one image and one question to the Qwen3-VL-4B-Instruct model, run as a
separate Python process on the GPU inside the AI-3D WSL distribution, and
returns the answer in words. Outputs: `answer` (JSON `{question, answer,
advisory: true, label}`), `report` and `manifest` (JSON with the inputs, model
revision and measurements).

The answer is a hint to read next to Vernon's numeric reports, not a
measurement. The label "AI description: a hint, not a measurement" is the first
line of the report and a field of the answer, and `advisory` is always `true`.
The node sets no pass/fail and no gate may read its output. Report metric names
all start with `ai_`.

## What it does not do
- It can be confidently wrong, and it can miss things that are plainly there.
- It measures nothing: no sizes, distances, counts you could rely on, or
  tolerances.
- **It is not reliable at spotting mesh defects in a render** (see "Measured on
  defect renders" below): it missed a see-through hole and a missing table leg,
  though it caught a part floating apart and raised no false alarms.
- It cannot see flipped faces in a viewport render at all: the viewport lights
  both sides of every face, so a flipped face looks like a correct one. Use the
  numeric mesh checks for those.
- It does not edit the image, and it never downloads anything while running.

## Licence
Model Qwen/Qwen3-VL-4B-Instruct: Apache-2.0 (licence tag on the model
repository, not gated). Python packages: `torch`, `torchvision`,
`transformers`, `accelerate`, `pillow`, `numpy`, `huggingface_hub`.

## Install
`scripts/ai/install_vision.sh --yes [weights_dir]`, run as root inside the WSL
distribution. It prints model, licence and size, and refuses to run without
`--yes`. It creates `/opt/ai3d/Vision` (Python 3.11), installs the packages,
downloads the safetensors weights, config, tokenizer and processor files (about
9 GB) into `<weights_dir>/model` (default `/mnt/e/AI-3D/Vision`), and writes
`installed-linux-packages.txt` and `model-revision.txt` there. Re-running reuses
what exists. Revision seen: `ebb281ec70b05090aa6165b016eac8ec08e71b17`.

## Inputs
| Parameter | Meaning |
|---|---|
| `ai_root` | AI-3D folder; blank uses `VERNON_AI3D_ROOT`. Must contain `Vision`. |
| `wsl_distro` | Default `AI-3D`. Blank runs Python directly (Linux). |
| `image` | png, jpg, jpeg, webp or ppm. The longer side is shrunk to at most 1024 px. |
| `question` | Default "Describe what this image shows and point out anything that looks broken, missing or out of place." Empty is an error. |
| `max_tokens` | 16 to 1024, default 256. |
| `timeout_s` | Process is stopped after this long (default 900). |
| `run` | Action switch. Without it the node only reloads an earlier result. |

The work folder is `vision_<fnv1a hash of image bytes, question, max_tokens>`;
identical inputs reload the saved answer without running the model. Before
starting, the node checks free GPU memory and RAM and refuses with a clear
message if there is not enough.

Distinct errors: image not found, unsupported extension, empty question,
install missing, not enough GPU memory, model error, timeout, empty answer.

## Model settings
bfloat16 on the GPU, greedy decoding (`do_sample=False`), one image and one
text item in the chat template, 4 CPU threads, offline (`HF_HUB_OFFLINE=1`).

## Measured (single run, not a benchmark)
Test picture written by the test itself: 512x512, white background, three
solid blue 90 px squares, one solid red disc of radius 50 px. Greedy decoding,
bfloat16, RTX 4090 Laptop 16 GB with about 12.7 GB free at the start (the GPU
is shared), the PC busy with other builds and a congested drive:
- "How many blue squares are in this image? Answer with a single number."
  answered `3` (correct).
- "What colour is the circle? Answer with one word." answered `red` (correct).
- Peak GPU memory (PyTorch allocations) 8.35 GB. Peak resident memory of the
  process 9.1 GB (includes the memory-mapped weights, which the system can
  drop). Node preflight uses these figures plus a 25 percent margin.
- Model load 74 to 96 s (the bridge first reads the weight files once, start
  to end, because page-by-page reads across the WSL drive mount were far too
  slow); generation 1.3 to 20 s for two tokens; whole node call 80 s wall
  clock for the first question and similar for the second (each call starts a
  fresh process).
- Weights for this run were read from a fast local drive. Reading them from
  the slow external drive was not timed.

Two simple questions say nothing about accuracy in general.

## Measured on defect renders (2026-11-20, not a benchmark): failed its bar
Ignored test `real_vision_on_defect_renders`, fresh work folder, same PC. The
pictures come from the app's own viewport renderer (test `render_defect_set` in
`vernon-render`, 640 x 640, shaded, grey background): three defects, each next
to an intact twin and drawn from two camera angles, 12 renders in all.
- A sphere with a hole cut right through it, so the background shows through.
- A body with an upright fin standing 60 mm clear of it.
- A table with three legs instead of four.

Before the model saw anything, every render was checked by eye to make sure the
defect was plainly visible and the intact twin looked intact. Three first
attempts failed that check and were redrawn: a hole in the near side only looked
like a whole ball, one leg of the intact table hid behind another, and a fin
lifted above the body looked attached from above. The pass bar, also set
before the run, was at least 10 of 12 right and at most 1 of the 6 intact meshes
reported as defective.

| Defect | Question | Defective twin | Intact twin |
|---|---|---|---|
| Hole | Is there a hole or a missing patch? | **missed** both times ("NO, the surface appears intact") | right |
| Floating part | Is any part floating apart, not touching? | right both times | right |
| Missing leg | How many legs does the table have? | **wrong** both times (answered 4 for 3 legs) | right |

Result: 8 of 12 right, so **it failed the bar**; 0 of 6 intact meshes were
reported as defective. For both spheres, both fin bodies and the intact table,
the second angle gave a picture byte-for-byte identical to the first (these
shapes are symmetric, and the fin was moved to the far end for the second
angle), so the 12 renders are 7 distinct pictures, of
which 4 were answered right. The question wording was not changed after the
run, and nothing was re-run.

Each fresh call took 168 s to 494 s; identical pictures reloaded the saved
answer.

What this means for viewport checking (plan item K): the model leans towards
"looks fine". It is no use as a defect finder on its own, and its "no defect"
answers should not reassure anyone. A "yes" was right every time here, but that
rests on one kind of defect.

## Limits
- First load is slow on a slow weights drive; later runs depend on disk cache.
- The GPU is shared with other programs; the node refuses to start when free
  memory is below the measured peak plus a margin.
- Answers can change with a different transformers or model revision.

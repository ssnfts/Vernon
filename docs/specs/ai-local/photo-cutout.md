# AI Photo Cut-out (`capture.ai_photo_cutout`)

Emulates: click to segment one object in a photo.

## What it does
You give it a photo and a click point (as a fraction of width and height). The
SAM 2.1 model, run as a separate Python process inside the AI-3D WSL
distribution, marks the one object under that point. Outputs:
- `cutout`: path of an RGBA PNG, same size as the photo, alpha 255 inside the
  object and 0 elsewhere.
- `mask`: JSON `{width, height, bbox:[x0,y0,x1,y1], area_fraction, score}`.
  `bbox` is in pixels, both corners inclusive; `score` is the model's own
  quality estimate.
- `report`: area fraction, size, score, timings, memory, and a note that the
  mask is a model guess to be checked by eye.
- `manifest`: JSON with inputs, model revision, library and measurements.

The bridge also writes the mask as raw bytes (`mask.u8`, width x height, 0 or
255, row-major). The Rust side reads that file itself, checks its length equals
width x height, and recomputes the bounding box and area fraction. An empty mask
and a mask covering more than 99% of the photo are each refused with their own
message. This is meant to feed photo-to-3D steps that need one object on a
transparent background.

## What it does not do
- One object per click, one positive point. No several clicks, no boxes, no
  "everything" mode, no video.
- The mask is a model guess, not a measurement. Thin parts, shiny or
  transparent surfaces, shadows and clutter can all go wrong. Check the PNG by
  eye. A synthetic disc on a plain background is an easy case and says little
  about cluttered workshop photos.
- It does not repair, feather or smooth the edge, and it does not crop.
- It never downloads anything while running.

## Licence
Model facebook/sam2.1-hiera-large: Apache-2.0 (about 0.9 GB of weights).
Python packages: torch, torchvision, transformers (its own SAM 2 implementation is used; the
separate `sam2` package is not needed), Pillow, numpy.

## Install
`scripts/ai/install_sam2.sh --yes [weights_dir]`, run as root inside the WSL
distribution. It prints model, licence and size, and refuses to run without
`--yes`. It creates `/opt/ai3d/SAM2` (Python 3.11), installs the packages,
downloads `config.json`, `model.safetensors`, `preprocessor_config.json` and
`processor_config.json` into the weights folder (default `/mnt/e/AI-3D/SAM2`),
and writes `installed-linux-packages.txt` and `model-revision.txt` there.
Re-running reuses what exists. It downloads on the Linux disk first and then
copies the files across, because a slow USB drive stalled direct downloads.

## Inputs
| Parameter | Meaning |
|---|---|
| `ai_root` | AI-3D folder; blank uses `VERNON_AI3D_ROOT`. Must contain `SAM2`. |
| `wsl_distro` | Default `AI-3D`. Blank runs Python directly (Linux). |
| `image` | Photo: png, jpg, jpeg, webp or ppm. |
| `point_x`, `point_y` | Click point, 0 to 1 (0.5, 0.5 is the centre). Outside 0 to 1 is an error. |
| `timeout_s` | Process is stopped after this long (default 600). |
| `run` | Action switch. Without it the node only reloads an earlier result. |

The work folder is `cutout_<fnv1a hash of the image bytes and the point>`;
identical inputs reload the saved result without running the model. Before a
run the node checks free GPU memory and RAM.

## Errors
Missing photo, unsupported extension, click point outside 0 to 1, missing AI
folder or SAM2 install, too little free GPU memory or RAM, model error (with
the model's error code), time limit reached, wrong-length mask, empty mask,
mask covering more than 99% of the photo.

## Measured (single run, not a benchmark)
Synthetic 512 x 512 test picture: mid-grey background with mild per-pixel
variation and a solid red disc of radius 120 px at the centre; click at (0.5,
0.5). Library route: `transformers` (Sam2Model), on the RTX 4090 Laptop GPU,
while other jobs were loading the same PC and the weights drive.
- Intersection-over-union of the mask with the true disc: 0.9936.
- Model score 0.994; area fraction 0.1714; bounding box [137, 137, 375, 375]
  (true disc: 136 to 376).
- Model load 121 s (weights from a slow USB drive), segmentation 17.2 s (first
  call, includes GPU start-up). Whole node call 456 s wall clock, mostly
  Python and torch start-up under heavy disk load; the first attempt on a cold
  disk took over 10 minutes and was stopped by the 600 s limit.
- Peak GPU memory 1.25 GB, peak resident memory of the model process 1.65 GB.
- Model revision `665f8e2ad61cf5f53d65644ff27c8ee525124610`.

This is one run on an easy synthetic picture. It says nothing about cluttered
photos, and it is not a benchmark.

## Limits
- First load reads about 0.9 GB from the weights drive; a slow USB drive makes
  that the longest part of a run.
- Each run loads the model afresh in a new process; there is no resident
  server, so a second click on the same photo pays the load time again.
- Very large photos are resized internally by the model and the mask is scaled
  back, so edges are only as sharp as that allows.

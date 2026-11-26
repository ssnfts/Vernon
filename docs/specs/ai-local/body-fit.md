# AI Body Fit from Photo (`capture.ai_body_fit`)

Emulates: single-photo human body shape and pose estimation.

**Opt-in, restrictive licence.** The model, its code and its backbone code are
under Meta's SAM License and DINOv3 License, which are not open source. Nothing
is installed or run until the user accepts them (see "Licence").

## What it does
Takes one photo of one person and estimates a body surface (about 18,400
points, 36,900 triangles) and 70 named key points (nose, shoulders, wrists,
ankles and so on) with SAM 3D Body (`facebook/sam-3d-body-dinov3`). The whole
picture is treated as the one person: there is no person detector, so crop the
photo to the person first.

The body is moved into the app's frame: millimetres, z up, x to the picture's
right, y away from the camera, lowest point at z = 0, centred on x and y. The
model guesses the person's height itself; set `known_height_mm` to scale the
body to a known height instead.

Outputs: `body` (geometry), `pose` (JSON: each key point's name, position in
the app frame in mm and position in picture pixels, plus the frame, the picture
size and the label), `report` and `manifest` (licence note, model revision,
code and backbone commits, the model's own height guess, the scale used, the
camera guess, timings, mesh topology).

The label "AI body estimate from one photo: a guess at shape and pose, not a
measurement of the person" is the first line of the report and a field of the
pose.

## What it does not do
- It does not measure a person. Size comes from the model's guess or from the
  height you give; limb lengths and girths are estimates, unfit for tailoring,
  medical or safety use.
- One person, one photo: no detector, no several people, no video.
- No clothing, hair or texture: the surface is the model's body template.
- No acceptance gate may read its output, and it never downloads anything at
  run time (the backbone is loaded from a pinned local checkout).
- The photo stays on this PC; nothing is sent anywhere.

## Licence
- Weights `facebook/sam-3d-body-dinov3` (revision `11aaa346`) and code
  `facebookresearch/sam-3d-body` (commit `b5c765a0`): SAM License.
- Backbone code `facebookresearch/dinov3` (commit `6876159a`): DINOv3 License.
- Both forbid military, warfare, nuclear, espionage and weapons use and anything
  under arms export rules (ITAR). Passing the model or code on is allowed only
  under the same licence, with a copy. Copies of both texts are written next to
  the weights.
- The weights are gated: request access on the model's Hugging Face page with
  your own account, then sign in inside the AI-3D WSL distribution.
- The checkpoint loads with Python pickle, which runs code from the file.

Install (inside the AI-3D WSL distribution, as root):
`bash scripts/ai/install_bodyfit.sh --yes --accept-licence`. Without both flags it
only prints the terms. About 2.7 GB of weights plus 5.6 GB of Python packages.

## Inputs
| Parameter | Meaning |
|---|---|
| `ai_root` | AI-3D folder; blank uses `VERNON_AI3D_ROOT`. Must contain `BodyFit`. |
| `wsl_distro` | Default `AI-3D`. Blank runs Python directly (Linux). |
| `image` | The photo (png, jpg, jpeg, webp). EXIF rotation is applied. |
| `licence_accepted` | "I accept the model's licence". Off by default; the model never starts while it is off. |
| `known_height_mm` | 0 to 2500. 0 keeps the model's own height guess. |
| `timeout_s` | Process is stopped after this long (default 900). |
| `run` | Action switch. Without it the node only reloads an earlier result. |

A photo that was already fitted reloads its saved result (keyed by the photo's
bytes), so changing `known_height_mm` or cooking again does not start the model.

## Measured (2026-11-17, not a benchmark)
RTX 4090 Laptop (16 GB). Ignored test `real_body_fit_follows_the_pose`, fresh
work folder. Two CC BY-SA 4.0 photos from Wikimedia Commons of one adult
standing (not in the repository; the test reads them from
`VERNON_BODYFIT_PHOTOS_DIR`): a side view with the arms down, and a front view
with both arms raised overhead. The pass bar was set before the first run.

| Photo | Own height | Nose | Shoulders | Wrists | Ankles |
|---|---|---|---|---|---|
| Arms down (side) | 1783 mm | 1651 mm | 1437, 1430 mm | 947, 955 mm | 104, 91 mm |
| Arms up (front) | 2083 mm | 1668 mm | 1441, 1448 mm | 1913, 1938 mm | 81, 82 mm |

Heights are above the floor (z) in the app frame. Both photos passed the bar:
- every point was finite;
- each body was one closed surface of 18,439 points and 36,874 triangles;
- the nose was more than 1000 mm above both ankles and in the top half of
  the picture;
- arms down: both wrists were below both shoulders, and the height (1783 mm)
  was within 1400 to 2100 mm;
- arms up: both wrists were above the nose in 3D and in the picture, and the
  height (2083 mm, raised hands included) was within 1700 to 2700 mm;
- with `known_height_mm = 1700` the saved result was rescaled to exactly
  1700 mm without starting the model.

Other measurements:
- Peak GPU memory 3.4 GB and peak RAM 5.0 GB.
- Loading took 66 to 80 s and fitting 1.4 to 4.4 s. Each call took 78 to 99 s
  from start to finish, with the weights on the USB drive.

What this does not show:
- Two photos of one person is a sanity check, not an accuracy test. Nothing was
  compared against a measured body.
- The arms-down body is 1002 mm deep along the camera axis, which is more
  than a standing person is. The cause was not found; poses seen side-on may be
  stretched in depth.
- When the model loads, its code warns that the checkpoint does not carry
  some of the body template's buffers. Those come from the separate template
  file. The output was sensible, but that warning was not looked into further.

States and errors are distinct: no photo chosen (idle), photo missing or of an
unknown type, not run, licence not accepted, missing install, no GPU, not
enough memory, model error, no body returned, a result that disagrees with its
mesh, timeout.

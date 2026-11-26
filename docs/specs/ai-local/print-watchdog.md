# AI Print Watchdog (`print.ai_print_watchdog`)

Emulates: camera-based print failure detection.

## What it does
Looks in a folder that a printer camera or a time-lapse tool saves pictures
into, takes the newest picture, and asks the local vision model one yes/no
question about it: does this print look failed? It uses the same model, bridge
and install as [AI Vision Check](vision-check.md) (Qwen3-VL-4B-Instruct), so
there is nothing extra to install.

The first word of the answer decides the verdict:

| Answer starts with | Verdict |
|---|---|
| yes | `possible_failure` (the report carries a warning) |
| no | `looks_ok` |
| anything else | `unclear` |

Outputs: `verdict` (JSON `{frame, verdict, answer, question, advisory: true,
label}`), `report` and `manifest` (JSON with the picture checked, how many
pictures the folder holds, and the vision check's own manifest).

The label "AI print check: a hint, not a measurement; it never stops a print"
is the first line of the report and a field of the verdict.

## What it does not do
- **Its hit rate is known from 20 photos only** (see "Measured on photos"
  below), mostly pictures of finished or abandoned prints rather than frames
  from a printer camera. How it does on a real camera feed is unknown.
- It does not stop, pause or talk to a printer. It only writes a verdict.
- It does not run on a timer. It checks once each time it is cooked with
  `run` on; repeating it during a print is a later app change.
- It looks at one picture, not at how the print changed between pictures.
- It reads pictures from a folder only: no camera stream, no network camera.
- No acceptance gate may read its output, and it never downloads anything.

## Licence
Model Qwen/Qwen3-VL-4B-Instruct: Apache-2.0, not gated. See
[vision-check.md](vision-check.md) for packages and install.

## Inputs
| Parameter | Meaning |
|---|---|
| `ai_root` | AI-3D folder; blank uses `VERNON_AI3D_ROOT`. Must contain `Vision`. |
| `wsl_distro` | Default `AI-3D`. Blank runs Python directly (Linux). |
| `frames_folder` | Folder of camera pictures (png, jpg, jpeg, webp, ppm). Sub-folders are ignored. |
| `question` | The yes/no question. The default names tangled or loose strands, a part off the bed, a collapsed or misshapen part, and shifted layers. |
| `max_tokens` | 16 to 256, default 64. |
| `timeout_s` | Process is stopped after this long (default 900). |
| `run` | Action switch. Without it the node only reloads an earlier answer. |

The newest picture is the one with the latest modified time; equal times fall
back to the file name. A picture that was already checked with the same
question reloads its saved answer (the cache is the vision check's, keyed by
the picture's bytes, the question and the token limit), so cooking again
without a new picture does not start the model.

States and errors are distinct: no folder chosen, folder not found, no
pictures yet (not an error: the print may not have started), not run, more
than 50,000 pictures in the folder, empty question, and every error of the
vision check (missing install, not enough memory, model error, timeout, empty
answer).

## Measured (single run, 2026-11-16, not a benchmark)
RTX 4090 Laptop (16 GB). Ignored test `real_watchdog_checks_the_newest_picture`,
fresh work folder. Two folders, each with an older and a newer drawn picture of
one coloured circle. The question was a stand-in the model is known to handle
("is the circle red", answer yes or no first), not the default question.

- Newest picture red: the model answered yes, verdict `possible_failure`.
  Newest picture blue: it answered no, verdict `looks_ok`. Both times the newer
  of the two pictures was the one checked. The pass bar was both right.
- Peak GPU memory 8.3 GB, peak RAM 9.1 GB.
- Model load 187 s and 99 s; answering 155 s (first call) and 2.7 s.
- The two node calls took 2020 s and 905 s. Most of that was Python starting
  from the environment's drive, which other jobs had saturated. A quiet machine
  was not measured.
- The weights were read from a copy on the internal drive whose two weight
  files match the SHA-256 hashes the model repository publishes.

## Measured on photos (2026-11-17, not a benchmark)
Ignored test `real_watchdog_on_print_photos`, fresh work folder each run, same
PC. 20 photographs from Wikimedia Commons (CC0, CC BY 2.0, CC BY-SA 3.0 and
4.0): 10 of failed prints (strings and tangles, parts that came off the bed, a
layer shift) and 10 of good prints or printers at work. The photos are not in
the repository; the test reads them from the folder in
`VERNON_WATCHDOG_PHOTOS_DIR`. The pass bar, set before the first run: at least
8 of 10 failures flagged and at most 1 of 10 good photos flagged.

| Question | Failures flagged | Good photos flagged | Unclear | Result |
|---|---|---|---|---|
| First default (strands, part off the bed, collapsed part) | 6 of 10 | 0 of 10 | 0 | failed the bar |
| Current default (adds tangles, misshapen parts, shifted layers) | 10 of 10 | 0 of 10 | 0 | passed the bar |

Read the second row with care:
- **The question was reworded after seeing the first run's misses, on the same
  20 photos**, so its 10 of 10 is an in-sample result and is optimistic. No
  fresh photos were tried with it.
- Its "yes" answers mostly repeat the question's own words ("tangled or loose
  strands"), including for the layer-shift photo, which has no strands. The
  verdicts were right, but the reasons given are not to be trusted.
- Three of the failure photos show one print from three angles, so the 10
  failures are 8 distinct prints.
- The failed/good labels were set by the tester, not by the photos' authors.
  One failure photo (a two-colour boat with a ragged outer
  wall) may be a normal shield wall rather than a failure.
- With the first question the model missed a stringy collapsed print, a
  spaghetti-style failure (it said "tangled mass of filament" but answered NO)
  and the layer shift.
- One photo took 150 s to 765 s (model loaded each time; the slow ones ran
  beside an install).

## Limits
- Every check starts the model again, so one check takes minutes even on a
  quiet machine (the vision check measured 124 s or more to load). It suits a
  check every few minutes on a long print, not a live view.
- Needs about 8.3 GB of GPU memory and 9.1 GB of RAM, so it can't run beside
  another heavy GPU job such as a CFD run.
- Lighting, camera angle and picture quality will change the answers; none of
  that has been explored.

# More local AI in Vernon: ordered plan and agent assignments, 2026-11-14

Status: wave 1 (items 1 to 3) built and verified with the real models on 2026-11-14; wave 2 (items 4 to 6) built and verified with the real models on 2026-11-15; wave 3: item 7 built and verified with the real model on 2026-11-16, item 8 built and verified with the real model on 2026-11-16, then run on 20 real print photos on 2026-11-17, item 9 in progress (see "Wave 3" below). This adds to the
[2026-11-04 AI plan](2026-11-04-ai-assistant-and-machine-split.md) and to
section 5 of [docs/PLAN.md](../../PLAN.md#5-local-ai-what-to-add). The rules
there don't change: models run out of process, nothing downloads without the
user's say, only MIT and Apache-2.0 models are built in, AI output never
passes a gate, and no heavy model loads while a CFD run is active.

Licences below were read from the Hugging Face API on 2026-11-14.

## Order: most likely to succeed first

| # | Addition | Model | Licence | Why it's ranked here | Wave |
|---|---|---|---|---|---|
| 1 | **Speech out** (text to a WAV file) | `hexgrad/Kokoro-82M` | Apache-2.0 | 82M parameters, runs on a CPU, one small Python package | 1 |
| 2 | **Speech in** (audio file to text) | `openai/whisper-large-v3-turbo` | MIT | Mature, official weights, about 2 GB of GPU memory *(verify)* | 1 |
| 3 | **Convergence forecast** (advice only) | `amazon/chronos-2` | Apache-2.0 | 0.5 GB, simple API, input is a plain number series | 1 |
| 4 | **Offline document search** | `BAAI/bge-m3` (search), `ibm-granite/granite-docling-258M` (PDF reading) | MIT, Apache-2.0 | Mature libraries; needs an index format | 2 |
| 5 | **Photo cut-out** (mask one part in a photo) | `facebook/sam2.1-hiera-large` | Apache-2.0 | Mature; needs a click or box as a prompt | 2 |
| 6 | **Vision check** (item K in PLAN.md) | `Qwen/Qwen3-VL-8B-Instruct`, 4-bit | Apache-2.0 | Larger download and a llama.cpp server to manage | 2 |
| 7 | **Sketch from words** | `black-forest-labs/FLUX.2-klein-4B` | Apache-2.0 | 15 GB download (24 GB for the whole repository) and a heavy GPU load, so not while CFD runs | 3 |
| 8 | **Print watchdog** | uses #6 on a timer | — | Depends on #6 and on a camera feed | 3 |
| 9 | **Body fit from a photo** | `facebook/sam-3d-body-dinov3` | SAM licence, gated | Blocked until the user accepts the licence on Hugging Face; opt-in only | 3 |

Left out: CoTracker3 and the CadQuery fine-tunes (non-commercial), and
Hunyuan3D-Part (no licence listed).

## How each addition is built (same pattern as PartCrafter and DoMINO)

* A Python environment at `/opt/ai3d/<Name>` in the `AI-3D` WSL distribution,
  created by a checked-in `scripts/ai/install_<name>.sh` that the user runs.
  Weights go to `<AI-3D folder>/<Name>/`. The bridge runs with
  `HF_HUB_OFFLINE=1`, so a node never downloads anything.
* A bridge script `scripts/ai/<name>_bridge.py`: reads `request.json`, prints
  JSON progress lines, then exactly one `result` or `error`.
* A node in `crates/vernon-nodes-ai` with a **Run model** switch, a time limit,
  a GPU/RAM preflight, cached results by input hash, and a manifest recording
  the model revision, inputs and timings.
* Unit tests that never need the model, and one `#[ignore]` test that runs the
  real model.

**Done means:** the unit tests pass, clippy is clean, the real model ran on
this PC through the node, and the measured time and memory are written in the
spec. A model that only loads, or a test that only uses a fake, isn't "works".

## Assignments

One agent per model, each briefed on that model's own API, in waves of three.
The lead session (this plan's author) wires modules together, merges, runs the
workspace checks and re-runs every real-model check before anything is called
done.

| Agent | Owns | Files | Real-model check |
|---|---|---|---|
| **Kokoro speech agent** | #1 | `speak.rs`, `kokoro_bridge.py`, `install_kokoro.sh`, `docs/specs/ai-local/speech-out.md` | A sentence becomes a 24 kHz WAV of plausible length that isn't silent |
| **Whisper speech agent** | #2 | `transcribe.rs`, `whisper_bridge.py`, `install_whisper.sh`, `docs/specs/ai-local/speech-in.md` | A known public-domain recording transcribes to its known words |
| **Chronos forecast agent** | #3 | `forecast.rs`, `chronos_bridge.py`, `install_chronos.sh`, `docs/specs/ai-local/forecast.md` | A synthetic decaying series forecasts within a stated tolerance; a real solver log parses |
| Lead | wiring, merge, checks | `lib.rs`, `CLEAN_ROOM.md`, `docs/NODES.md`, `docs/specs/ai-models.md` | Round trip: Kokoro speaks a sentence, Whisper writes it back, the words match |
| **Document search agent** | #4 | `docsearch.rs`, `docsearch_bridge.py`, `install_docsearch.sh`, `docs/specs/ai-local/document-search.md` | Topic questions about this repository's own specs return the right file in the top three |
| **Photo cut-out agent** | #5 | `cutout.rs`, `sam2_bridge.py`, `install_sam2.sh`, `docs/specs/ai-local/photo-cutout.md` | A drawn disc is masked from one click and the mask overlaps the true disc by at least 90% |
| **Vision check agent** | #6 | `vision.rs`, `vision_bridge.py`, `install_vision.sh`, `docs/specs/ai-local/vision-check.md` | Drawn test pictures: the count of squares and the colour of a circle come back right |
| **Sketch agent**, finished by the lead | #7 | `sketch.rs`, `sketch_bridge.py`, `install_sketch.sh`, `docs/specs/ai-local/sketch-from-words.md` | A prompt for a red ball on white gives a picture that is red in the middle and white in the corners |
| Lead | #8 | `watchdog.rs`, `docs/specs/ai-local/print-watchdog.md` (model, bridge and install are those of #6) | Of two drawn pictures the newer one is checked, and a yes and a no become the right verdicts |

## What changed from the plan in wave 2

* **Vision check (#6)** uses `Qwen/Qwen3-VL-4B-Instruct` in half precision through
  the `transformers` package, not the 8B model in 4-bit behind a llama.cpp
  server. It needs no server to manage and fits beside other work on a 16 GB
  GPU (8.35 GB measured). The 8B model is untested.
* **Document search (#4)** reads PDFs as text with `pypdf`. The
  `granite-docling` PDF reader in the table above was not built, so scanned
  pages and complex layouts aren't read.
* **Photo cut-out (#5)** runs SAM 2.1 through the `transformers` package, not
  the model's own package.

## Wave 3

* **Sketch from words (#7)** is built. The real model drew the test picture on
  2026-11-16 (7.8 GB of GPU memory, 16.7 GB of RAM, parts moved to the GPU one
  at a time). The download is the 15 GB of pipeline folders, not the whole
  24 GB repository. The agent wrote the node, bridge and install script but did
  not run the real model; the lead ran it and wrote the spec. The first real
  run timed out because the install drive was saturated by other jobs, so the
  passing run read a checksum-matched copy of the weights from the internal
  drive.
* **Print watchdog (#8)** is built as `print.ai_print_watchdog`: it takes the
  newest picture in a camera folder and puts a yes/no question to the vision
  model of #6. The real model was run on drawn pictures with a stand-in
  question, which shows the node picks the newest picture and reads a yes or a
  no correctly. On 2026-11-17 it was run on 20 photographs from Wikimedia
  Commons: 6 of 10 failures and 0 of 10 good prints flagged with the first
  question; 10 of 10 and 0 of 10 after the question was reworded on the same
  photos (in-sample, so optimistic). It checks
  once per cook; running it on a timer is a later app change. The lead built it.
* **Body fit from a photo (#9)** is not started. The model is gated: the user
  has to accept its licence on Hugging Face first, and it would be opt-in.

The README's **Local AI** table is updated by the session that maintains the
README, one row per addition, once that addition is verified.

## Limits stated up front

* Wave 1 gives nodes that work on files (WAV in, WAV out, series in). A
  microphone button in the chat panel is a later app change.
* The forecast is labelled **"AI forecast: advisory only"**. No gate reads it
  and it never stops or relabels a run.
* A CFD run was active on this PC while wave 1 was built, so cargo and model
  jobs were capped and timings may read slow.

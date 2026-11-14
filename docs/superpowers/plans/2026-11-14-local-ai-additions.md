# More local AI in Vernon: ordered plan and agent assignments, 2026-11-14

Status: wave 1 (items 1 to 3) built and verified with the real models on 2026-11-14; waves 2 and 3 not started. This adds to the
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
| 7 | **Sketch from words** | `black-forest-labs/FLUX.2-klein-4B` | Apache-2.0 | 24 GB download and a heavy GPU load, so not while CFD runs | 3 |
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
| Wave 2 agents | #4, #5, #6 | assigned after wave 1 is merged | — |

The README's **Local AI** table is updated by the session that maintains the
README, one row per addition, once that addition is verified.

## Limits stated up front

* Wave 1 gives nodes that work on files (WAV in, WAV out, series in). A
  microphone button in the chat panel is a later app change.
* The forecast is labelled **"AI forecast: advisory only"**. No gate reads it
  and it never stops or relabels a run.
* A CFD run was active on this PC while wave 1 was built, so cargo and model
  jobs were capped and timings may read slow.

# AI Speech from Text (`geo.ai_text_to_speech`)

Emulates: text to speech synthesis.

## What it does
Turns English text into a 24 kHz mono 16-bit WAV file with the Kokoro-82M
model, run as a separate Python process on the CPU inside the AI-3D WSL
distribution. Outputs: `audio` (path of the WAV), `report` (sample rate,
duration, peak level, timings, memory) and `manifest` (JSON with the inputs,
model revision and measurements).

The Rust side reads the WAV header and samples itself. A file that is not a
valid WAV, has zero length, or is silent (peak below 0.0001 of full scale) is
refused with an error.

## What it does not do
- English only (American and British voices). No other languages, no voice
  cloning, no emotion or style control, no SSML.
- It does not play the audio and does not edit it.
- It never downloads anything while running.

## Licence
Model hexgrad/Kokoro-82M: Apache-2.0 (82M parameters). Python packages
`kokoro`, `misaki`, `soundfile`, `torch` (CPU), spaCy with `en_core_web_sm`;
the phonemiser fallback library comes with the `espeakng-loader` wheel and runs
only inside the separate process.

## Install
`scripts/ai/install_kokoro.sh --yes [weights_dir]`, run as root inside the WSL
distribution. It prints model, licence and size, and refuses to run without
`--yes`. It creates `/opt/ai3d/Kokoro` (Python 3.11), installs the packages and
the spaCy model, downloads the weights and six voices (about 330 MB) into the
weights folder (default `/mnt/e/AI-3D/Kokoro`), and writes
`installed-linux-packages.txt` and `model-revision.txt` there. Re-running reuses
what exists. Measured revision: `f3ff3571791e39611d31c381e3a41a3af07b4987`.

## Inputs
| Parameter | Meaning |
|---|---|
| `ai_root` | AI-3D folder; blank uses `VERNON_AI3D_ROOT`. Must contain `Kokoro`. |
| `wsl_distro` | Default `AI-3D`. Blank runs Python directly (Linux). |
| `text` | Text to speak; empty text is an error. |
| `voice` | af_heart (default), af_bella, af_nicole, am_michael, bf_emma, bm_george |
| `speed` | 0.5 to 2.0, default 1.0 |
| `timeout_s` | Process is stopped after this long (default 600). |
| `run` | Action switch. Without it the node only reloads an earlier result. |

The work folder is `speech_<fnv1a hash of model, voice, speed, text>`; identical
inputs reload the saved WAV without running the model.

## Measured (single run, not a benchmark)
Text "Vernon speech check: the front splitter is ready to print.", voice
af_heart, speed 1.0, CPU, 4 torch threads, while a 16-rank CFD job was running
on the same PC:
- Output: 24000 Hz, 3.70 s, peak 0.505.
- Model load 80.5 s (weights read from a slow USB drive), synthesis 3.8 s.
- Whole node call 443 s wall clock (includes Python and torch start-up on a
  heavily loaded machine).
- Peak resident memory of the model process: 1.24 GB. No GPU used.

## Limits
- First load is slow on a slow weights drive; later runs depend on disk cache.
- Very long text is synthesised sentence by sentence in one process; raise
  `timeout_s` for long passages.
- Pronunciation of unusual words (part numbers, abbreviations) is the model's
  best guess; spell them out if needed.

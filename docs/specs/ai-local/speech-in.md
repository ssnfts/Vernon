# Speech in: AI Text from Speech (`geo.ai_speech_to_text`)

## What it does
Turns one speech recording into text. It runs OpenAI's Whisper large-v3-turbo
(`openai/whisper-large-v3-turbo`, 809M parameters, MIT licence, official weights) in a
separate Python process inside the AI-3D Linux environment, the same way the part
generator does. Emulates: speech to text transcription.

## What it does not do
- No speaker labels, no word-level timing, no translation (it transcribes in the spoken or chosen language).
- It never downloads anything while a graph cooks. Weights are installed once by `scripts/ai/install_whisper.sh`.
- Transcripts are machine output and can be wrong, especially for names, numbers, noise and music.
- It does not record from a microphone.

## Licence
Model weights and Whisper code: MIT. Python packages: torch, transformers, accelerate, soundfile, librosa, huggingface_hub (all permissive). Nothing is linked into Vernon; the model is a separate process.

## Install
```
wsl -d AI-3D -u root -- bash scripts/ai/install_whisper.sh --yes
```
Prints model, licence and download size (about 1.6 GB, `model.safetensors` plus config and tokenizer files only) and refuses to run without `--yes`. It creates `/opt/ai3d/Whisper` with uv (Python 3.11), downloads the weights to `<AI-3D>/Whisper/model`, and writes `download-whisper.json` (revision) and `installed-linux-packages.txt` (pip freeze) next to them. Re-running is safe. Tested revision: `41f01f3fe87f28c78e2fbf8b568835947dd65ed9`; torch 2.15.0, transformers 5.23.0.

## Inputs (parameters)
`ai_root`, `wsl_distro` (default AI-3D), `audio` (wav, flac, mp3, ogg, m4a), `language` (auto, en, es, fr, de, it, pt, ar, zh), `timeout_s`, `run`.

## Outputs
- `text` (JSON): `{text, language, duration_s, segments:[{start, end, text}]}`.
- `report`: word count, segment count, audio duration, load and transcribe seconds, peak GPU and RAM. An empty transcript is a warning, not an error.
- `manifest` (JSON): model, revision, device, audio path and SHA-256, language requested and detected, timings and peak memory.

## Behaviour
Work folder `transcript_<hash>`, where the hash covers the audio bytes plus the settings. Without `run`, the node only reloads an earlier result for identical inputs (otherwise reports `not_run`). Errors are distinct: audio file not found, unsupported audio format, AI model folder not found, Whisper not installed, not enough free GPU/RAM, model error (code and message from the bridge), time limit exceeded. The bridge sets `HF_HUB_OFFLINE=1` before importing anything. It uses the transformers ASR pipeline (fp16 on CUDA, fp32 on CPU if CUDA is missing, 30 s chunks, segment timestamps) and limits torch to 4 CPU threads. Audio is decoded in Python (soundfile, then librosa for resampling to 16 kHz mono and for formats libsndfile lacks); no system ffmpeg is needed.

## Measured (single run, not an average)
RTX 4090 Laptop (16 GB), WSL AI-3D, weights on a slow USB drive, machine busy with other jobs. Input: 11 s public-domain clip (1961 US presidential speech, `jfk.flac`), language auto.
- Transcript: "And so, my fellow Americans, ask not what your country can do for you, ask what you can do for your country." (language detected: en, 1 segment, 22 words)
- Model load 12.7 s, inference 15.0 s (first run: 14.5 s and 52.8 s, cold caches).
- Peak GPU memory 1.83 GB (`torch.cuda.max_memory_allocated`), peak RSS 2.55 GB. The node's pre-flight uses 2.0 GB VRAM and 3.5 GB RAM plus margin.
- Whole test wall time 302 s, almost all of it starting WSL and importing Python packages from a disk shared with other jobs; this is not model time.

## Formats tested
FLAC only (the clip above). WAV, MP3 and OGG go through the same soundfile/librosa path and M4A through librosa's fallback, but they were not run.

## Limits
Only one language setting at a time, no streaming, long files are chunked at 30 s so word splits at chunk edges are possible, and quality drops on noisy audio and rare languages. Time limit default 30 minutes.

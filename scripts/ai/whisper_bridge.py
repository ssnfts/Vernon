"""Vernon <-> Whisper bridge. Runs inside the AI-3D WSL environment.

Usage: python whisper_bridge.py --request request.json
Transcribes one audio file with the installed openai/whisper-large-v3-turbo weights
and writes result.json into the request's out_dir. stdout carries one JSON object
per line: progress events, then exactly one `result` or `error` event.
Never downloads: HF_HUB_OFFLINE is set before anything is imported.
"""
import os

os.environ["HF_HUB_OFFLINE"] = "1"
os.environ["TRANSFORMERS_OFFLINE"] = "1"
os.environ.setdefault("OMP_NUM_THREADS", "4")

import argparse
import hashlib
import json
import resource
import sys
import time
import traceback
from pathlib import Path


def emit(event, **kw):
    print(json.dumps({"event": event, **kw}), flush=True)


def fail(code, message):
    emit("error", code=code, message=message)
    sys.exit(2)


def linux_path(v):
    if isinstance(v, str) and len(v) > 2 and v[1] == ":" and v[2] in "\/":
        return "/mnt/" + v[0].lower() + "/" + v[3:].replace("\\", "/")
    return v


def sha256_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 22), b""):
            h.update(chunk)
    return h.hexdigest()


def load_audio(path):
    """Decode to 16 kHz mono float32 without a system ffmpeg."""
    import librosa
    import numpy as np
    import soundfile as sf
    try:
        data, sr = sf.read(str(path), dtype="float32", always_2d=True)
        data = data.mean(axis=1)
    except Exception:  # noqa: formats libsndfile lacks (e.g. m4a): librosa falls back to audioread
        data, sr = librosa.load(str(path), sr=None, mono=True)
    if sr != 16000:
        data = librosa.resample(data, orig_sr=sr, target_sr=16000)
    return np.ascontiguousarray(data, dtype="float32")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--request", required=True)
    req = json.loads(Path(linux_path(ap.parse_args().request)).read_text())
    root = Path(linux_path(req["ai_root"]))
    out = Path(linux_path(req["out_dir"]))
    out.mkdir(parents=True, exist_ok=True)
    audio = Path(linux_path(req["audio"]))
    if not audio.is_file():
        fail("input_missing", f"audio not found: {audio}")
    wdir = root / "Whisper" / "model"
    if not (wdir / "model.safetensors").is_file():
        fail("weights_missing", f"weights not found: {wdir}/model.safetensors")
    language = req.get("language", "auto")
    emit("progress", stage="import", fraction=0.02, message="importing torch and transformers")
    try:
        import torch
        torch.set_num_threads(4)
        from transformers import pipeline
    except Exception as e:  # noqa
        fail("import_failed", f"{type(e).__name__}: {e}")
    emit("progress", stage="audio", fraction=0.05, message="decoding audio")
    try:
        samples = load_audio(audio)
    except Exception as e:  # noqa
        fail("audio_unreadable", f"cannot decode {audio.name}: {type(e).__name__}: {e}")
    duration = len(samples) / 16000.0
    cuda = torch.cuda.is_available()
    device, dtype = ("cuda:0", torch.float16) if cuda else ("cpu", torch.float32)
    emit("progress", stage="load", fraction=0.1, message=f"loading weights on {device}")
    t0 = time.time()
    try:
        pipe = pipeline("automatic-speech-recognition", model=str(wdir), torch_dtype=dtype, device=device,
                        chunk_length_s=30, return_timestamps=True)
    except Exception as e:  # noqa
        fail("model_load_failed", f"{type(e).__name__}: {e}")
    t1 = time.time()
    if cuda:
        torch.cuda.reset_peak_memory_stats()
    gen = {"task": "transcribe"}
    if language != "auto":
        gen["language"] = language
    emit("progress", stage="transcribe", fraction=0.4, message="transcribing")
    try:
        r = pipe({"raw": samples, "sampling_rate": 16000}, generate_kwargs=gen)
    except torch.cuda.OutOfMemoryError:
        fail("out_of_memory", "GPU out of memory while transcribing")
    t2 = time.time()
    segments = []
    for c in r.get("chunks") or []:
        ts = c.get("timestamp") or (None, None)
        start = float(ts[0]) if ts[0] is not None else 0.0
        end = float(ts[1]) if ts[1] is not None else duration
        segments.append({"start": start, "end": end, "text": c.get("text", "").strip()})
    detected = language
    if language == "auto":
        try:
            tok = pipe.tokenizer
            ids = pipe.model.generate(**pipe.feature_extractor(samples[:480000], sampling_rate=16000, return_tensors="pt").to(device, dtype),
                                      max_new_tokens=1, return_dict_in_generate=True).sequences[0]
            lang_toks = [tok.decode(i) for i in ids if tok.decode(i).startswith("<|") and len(tok.decode(i)) <= 8]
            detected = lang_toks[0].strip("<|>") if lang_toks else "unknown"
        except Exception:  # noqa: detection is informational only
            detected = "unknown"
    commit = ""
    rec = root / "Whisper" / "download-whisper.json"
    if rec.is_file():
        commit = json.loads(rec.read_text()).get("revision", "")
    res = {"model": "openai/whisper-large-v3-turbo", "revision": commit, "device": device,
           "audio": str(audio), "audio_sha256": sha256_file(audio), "language_requested": language,
           "text": {"text": r["text"].strip(), "language": detected, "duration_s": duration, "segments": segments},
           "seconds_load": t1 - t0, "seconds_transcribe": t2 - t1,
           "peak_vram_GB": (torch.cuda.max_memory_allocated() / 1e9) if cuda else 0.0,
           "peak_rss_GB": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1048576.0}
    (out / "result.json").write_text(json.dumps(res, indent=2))
    emit("result", **res)


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception as e:  # noqa
        traceback.print_exc(file=sys.stderr)
        fail("exception", f"{type(e).__name__}: {e}")

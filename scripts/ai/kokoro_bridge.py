"""Vernon <-> Kokoro text-to-speech bridge. Runs inside the AI-3D WSL environment.

Usage: python kokoro_bridge.py --request request.json
Reads the request, synthesises speech with the installed Kokoro-82M weights and
writes speech.wav (24 kHz mono, 16-bit) plus result.json into the request's
out_dir. stdout carries one JSON object per line: progress events, then exactly
one `result` or `error` event. Never downloads anything: it is fully offline.
"""
import argparse
import json
import os
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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--request", required=True)
    req = json.loads(Path(linux_path(ap.parse_args().request)).read_text(encoding="utf-8"))
    root = Path(linux_path(req["ai_root"]))
    weights = root / "Kokoro"
    out = Path(linux_path(req["out_dir"]))
    out.mkdir(parents=True, exist_ok=True)
    text = str(req["text"]).strip()
    voice = str(req["voice"])
    speed = float(req["speed"])
    if not text:
        fail("empty_text", "there is no text to speak")
    if not 0.5 <= speed <= 2.0:
        fail("bad_speed", "speed must be between 0.5 and 2.0")
    if not weights.is_dir():
        fail("weights_missing", f"weights folder not found: {weights}")
    # Offline before anything is imported.
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    os.environ["HF_HOME"] = str(weights)
    os.environ["OMP_NUM_THREADS"] = "4"
    emit("progress", stage="import", fraction=0.05, message="importing torch and kokoro")
    try:
        import numpy as np
        import soundfile as sf
        import torch
        from kokoro import KPipeline
    except Exception as e:  # noqa
        fail("import_failed", f"{type(e).__name__}: {e}")
    torch.set_num_threads(4)
    lang = voice[0] if voice[:1] in ("a", "b") else "a"
    emit("progress", stage="load", fraction=0.15, message="loading Kokoro-82M")
    t0 = time.time()
    try:
        pipe = KPipeline(lang_code=lang, repo_id="hexgrad/Kokoro-82M", device="cpu")
    except Exception as e:  # noqa
        fail("load_failed", f"{type(e).__name__}: {e}")
    t1 = time.time()
    emit("progress", stage="synthesise", fraction=0.4, message="speaking")
    chunks = []
    try:
        for _gs, _ps, audio in pipe(text, voice=voice, speed=speed):
            if audio is None:
                continue
            a = audio.detach().cpu().numpy() if hasattr(audio, "detach") else np.asarray(audio)
            chunks.append(a.astype(np.float32).reshape(-1))
    except Exception as e:  # noqa
        fail("synthesis_failed", f"{type(e).__name__}: {e}")
    if not chunks:
        fail("empty_result", "the model produced no audio")
    wave = np.concatenate(chunks)
    t2 = time.time()
    wav_path = out / "speech.wav"
    sf.write(str(wav_path), wave, 24000, subtype="PCM_16")
    rev = ""
    rf = weights / "model-revision.txt"
    if rf.is_file():
        rev = rf.read_text().strip()
    import resource
    res = {"model": "hexgrad/Kokoro-82M", "revision": rev, "voice": voice, "speed": speed, "text": text,
           "wav": "speech.wav", "sample_rate": 24000, "samples": int(wave.shape[0]),
           "seconds_load": t1 - t0, "seconds_generate": t2 - t1,
           "peak_rss_GB": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1048576.0, "device": "cpu"}
    (out / "result.json").write_text(json.dumps(res, indent=2), encoding="utf-8")
    emit("result", **res)


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception as e:  # noqa
        traceback.print_exc(file=sys.stderr)
        fail("exception", f"{type(e).__name__}: {e}")

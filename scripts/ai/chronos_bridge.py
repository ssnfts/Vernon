"""Vernon <-> Chronos-2 bridge. Runs inside the AI-3D WSL environment.

Usage: python chronos_bridge.py --request request.json
The request names a series.json (name -> list of numbers, already in the space to forecast)
and the horizon. The bridge forecasts each series on its own (no cross-series learning) and
writes result.json with the 0.1 / 0.5 / 0.9 quantiles. stdout carries one JSON object per
line: progress events, then exactly one `result` or `error` event.
The model is loaded from a local folder only; nothing is ever downloaded.
"""
import os

# Must be set before anything that could touch the network is imported.
os.environ["HF_HUB_OFFLINE"] = "1"
os.environ["TRANSFORMERS_OFFLINE"] = "1"
os.environ.setdefault("OMP_NUM_THREADS", "4")
os.environ.setdefault("MKL_NUM_THREADS", "4")

import argparse
import json
import resource
import sys
import time
import traceback
from pathlib import Path

LEVELS = [0.1, 0.5, 0.9]


def emit(event, **kw):
    print(json.dumps({"event": event, **kw}), flush=True)


def fail(code, message):
    emit("error", code=code, message=message)
    sys.exit(2)


def linux_path(v):
    if isinstance(v, str) and len(v) > 2 and v[1] == ":" and v[2] in "\\/":
        return "/mnt/" + v[0].lower() + "/" + v[3:].replace("\\", "/")
    return v


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--request", required=True)
    req = json.loads(Path(linux_path(ap.parse_args().request)).read_text())
    root = Path(linux_path(req["ai_root"]))
    out = Path(linux_path(req["out_dir"]))
    out.mkdir(parents=True, exist_ok=True)
    model_dir = root / "Chronos" / "model"
    if not (model_dir / "config.json").is_file():
        fail("weights_missing", f"model folder not found: {model_dir}")
    series_path = Path(linux_path(req["series"]))
    if not series_path.is_file():
        fail("input_missing", f"series file not found: {series_path}")
    series = json.loads(series_path.read_text())
    horizon = int(req["horizon"])
    if horizon < 1:
        fail("bad_horizon", "horizon must be at least 1")
    emit("progress", stage="import", fraction=0.05, message="importing torch and chronos")
    try:
        import numpy as np
        import torch
        from chronos import Chronos2Pipeline
    except Exception as e:  # noqa
        fail("import_failed", f"{type(e).__name__}: {e}")
    torch.set_num_threads(4)
    device = "cuda" if torch.cuda.is_available() else "cpu"
    emit("progress", stage="load", fraction=0.15, message=f"loading Chronos-2 on {device}")
    t0 = time.time()
    pipe = Chronos2Pipeline.from_pretrained(str(model_dir), device_map=device)
    t1 = time.time()
    max_h = int(pipe.model_prediction_length)
    if horizon > max_h:
        fail("bad_horizon", f"horizon {horizon} exceeds the model limit {max_h}")
    max_ctx = int(pipe.model_context_length)
    names = list(series.keys())
    inputs, used = [], []
    for n in names:
        v = np.asarray(series[n], dtype=np.float32)[-max_ctx:]
        used.append(int(len(v)))
        inputs.append(torch.tensor(v))
    emit("progress", stage="predict", fraction=0.4, message=f"forecasting {len(names)} series, horizon {horizon}")
    if device == "cuda":
        torch.cuda.reset_peak_memory_stats()
    t2 = time.time()
    with torch.inference_mode():
        quantiles, _mean = pipe.predict_quantiles(inputs, prediction_length=horizon, quantile_levels=LEVELS, cross_learning=False)
    t3 = time.time()
    rows = []
    for n, ctxlen, q in zip(names, used, quantiles):
        q = q.float().cpu().numpy()[0]  # (horizon, 3)
        rows.append({"name": n, "context_length": ctxlen, "q10": q[:, 0].tolist(), "q50": q[:, 1].tolist(), "q90": q[:, 2].tolist()})
    revision = None
    rev_file = root / "Chronos" / "model-revision.txt"
    if rev_file.is_file():
        revision = rev_file.read_text().strip()
    res = {"model": "amazon/chronos-2", "revision": revision, "device": device, "quantile_levels": LEVELS,
           "horizon": horizon, "model_max_horizon": max_h, "model_max_context": max_ctx, "series": rows,
           "seconds_load": t1 - t0, "seconds_infer": t3 - t2,
           "peak_vram_GB": (torch.cuda.max_memory_allocated() / 1e9) if device == "cuda" else 0.0,
           "peak_rss_GB": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1048576.0}
    (out / "result.json").write_text(json.dumps(res))
    emit("result", **res)


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception as e:  # noqa
        traceback.print_exc(file=sys.stderr)
        fail("exception", f"{type(e).__name__}: {e}")

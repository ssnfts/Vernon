"""Vernon <-> SAM 2.1 click-to-segment bridge. Runs inside the AI-3D WSL environment.

Usage: python sam2_bridge.py --request request.json
Reads the request (image, point as fractions of width/height), runs
facebook/sam2.1-hiera-large through the `transformers` implementation with one
positive point, keeps the highest-scoring mask and writes into out_dir:
  cutout.png  RGBA, same size as the input, alpha = mask (0 or 255)
  mask.u8     raw bytes, width*height, 0 or 255, row-major
  result.json
stdout carries one JSON object per line: progress events, then exactly one
`result` or `error` event. Never downloads anything: it is fully offline.
"""
import argparse
import json
import os
import sys
import time
import traceback
from pathlib import Path

MODEL_ID = "facebook/sam2.1-hiera-large"


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
    weights = root / "SAM2"
    out = Path(linux_path(req["out_dir"]))
    out.mkdir(parents=True, exist_ok=True)
    image_path = Path(linux_path(req["image"]))
    px, py = float(req["point_x"]), float(req["point_y"])
    if not image_path.is_file():
        fail("image_missing", f"image not found: {image_path}")
    if not (0.0 <= px <= 1.0 and 0.0 <= py <= 1.0):
        fail("bad_point", "point_x and point_y must be between 0 and 1")
    if not weights.is_dir():
        fail("weights_missing", f"weights folder not found: {weights}")
    # Offline before anything is imported.
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    os.environ["HF_HOME"] = str(weights)
    os.environ["OMP_NUM_THREADS"] = "4"
    emit("progress", stage="import", fraction=0.05, message="importing torch and transformers")
    try:
        import numpy as np
        import torch
        from PIL import Image
        from transformers import Sam2Model, Sam2Processor
    except Exception as e:  # noqa
        fail("import_failed", f"{type(e).__name__}: {e}")
    torch.set_num_threads(4)
    device = "cuda" if torch.cuda.is_available() else "cpu"
    try:
        img = Image.open(image_path).convert("RGB")
    except Exception as e:  # noqa
        fail("image_unreadable", f"{type(e).__name__}: {e}")
    w, h = img.size
    emit("progress", stage="load", fraction=0.15, message=f"loading SAM 2.1 on {device}")
    t0 = time.time()
    try:
        local = str(weights / "sam2.1-hiera-large")
        processor = Sam2Processor.from_pretrained(local)
        model = Sam2Model.from_pretrained(local).to(device).eval()
    except Exception as e:  # noqa
        fail("load_failed", f"{type(e).__name__}: {e}")
    t1 = time.time()
    emit("progress", stage="segment", fraction=0.6, message="segmenting")
    try:
        point = [[[[px * w, py * h]]]]
        inputs = processor(images=img, input_points=point, input_labels=[[[1]]], return_tensors="pt").to(device)
        with torch.no_grad():
            outputs = model(**inputs, multimask_output=True)
        masks = processor.post_process_masks(outputs.pred_masks.cpu(), inputs["original_sizes"])[0]
        scores = outputs.iou_scores.float().cpu().reshape(-1)
        masks = masks.reshape(-1, h, w)
        best = int(scores.argmax())
        mask = (masks[best] > 0).numpy()
        score = float(scores[best])
    except Exception as e:  # noqa
        traceback.print_exc(file=sys.stderr)
        fail("segmentation_failed", f"{type(e).__name__}: {e}")
    t2 = time.time()
    m8 = (mask.astype(np.uint8)) * 255
    (out / "mask.u8").write_bytes(m8.tobytes())
    rgba = np.dstack([np.asarray(img, dtype=np.uint8), m8])
    Image.fromarray(rgba, "RGBA").save(out / "cutout.png")
    rev = ""
    rf = weights / "model-revision.txt"
    if rf.is_file():
        rev = rf.read_text().strip()
    import resource
    res = {"model": MODEL_ID, "revision": rev, "library": "transformers Sam2Model", "device": device,
           "width": w, "height": h, "point": [px, py], "score": score, "mask_index": best,
           "cutout": "cutout.png", "mask": "mask.u8",
           "seconds_load": t1 - t0, "seconds_segment": t2 - t1,
           "peak_vram_GB": (torch.cuda.max_memory_allocated() / 1073741824.0) if device == "cuda" else 0.0,
           "peak_rss_GB": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1048576.0}
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

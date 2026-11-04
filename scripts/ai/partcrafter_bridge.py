"""Vernon <-> PartCrafter bridge. Runs inside the AI-3D WSL environment.

Usage: python partcrafter_bridge.py --request request.json
Reads the request, runs the installed PartCrafter weights and writes one OBJ per
part plus result.json into the request's out_dir. stdout carries one JSON object
per line: progress events, then exactly one `result` or `error` event.
The installed launcher (run_local.py) is not modified or imported.
"""
import argparse
import functools
import hashlib
import json
import os
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
    if isinstance(v, str) and len(v) > 2 and v[1] == ":" and v[2] in "\\/":
        return "/mnt/" + v[0].lower() + "/" + v[3:].replace("\\", "/")
    return v


def sha256_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 22), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--request", required=True)
    req = json.loads(Path(linux_path(ap.parse_args().request)).read_text())
    root = Path(linux_path(req["ai_root"]))
    base = root / "PartCrafter"
    out = Path(linux_path(req["out_dir"]))
    out.mkdir(parents=True, exist_ok=True)
    image = Path(linux_path(req["image"]))
    if not image.is_file():
        fail("input_missing", f"image not found: {image}")
    scene = req["mode"] == "scene"
    parts = int(req["parts"])
    draft = req["quality"] == "draft"
    if not 1 <= parts <= 16:
        fail("bad_parts", "parts must be 1..16")
    model = "PartCrafter-Scene" if scene else "PartCrafter"
    wdir = base / "source" / "pretrained_weights" / model
    if not wdir.is_dir():
        fail("weights_missing", f"weights folder not found: {wdir}")
    os.environ.setdefault("HF_HOME", str(root / "_cache/huggingface-linux"))
    os.environ.setdefault("TORCH_HOME", str(root / "_cache/torch"))
    os.environ.setdefault("PYTORCH_CUDA_ALLOC_CONF", "expandable_segments:True")
    sys.path.insert(0, str(base / "source"))
    emit("progress", stage="import", fraction=0.02, message="importing torch and PartCrafter")
    try:
        import torch
        from PIL import Image
        from src.pipelines.pipeline_partcrafter import PartCrafterPipeline
    except Exception as e:  # noqa
        fail("import_failed", f"{type(e).__name__}: {e}")
    if not torch.cuda.is_available():
        fail("no_cuda", "CUDA is not available inside the WSL environment")
    emit("progress", stage="load", fraction=0.05, message=f"loading {model} weights")
    t0 = time.time()
    pipe = PartCrafterPipeline.from_pretrained(str(wdir), torch_dtype=torch.float16, local_files_only=True)
    pipe.vae.decode = functools.partial(pipe.vae.decode, num_chunks=10000)
    if req.get("offload"):
        pipe.to(dtype=torch.float16)
        pipe.model_cpu_offload_seq = "image_encoder_dinov2->transformer->vae"
        pipe.enable_model_cpu_offload()
    else:
        pipe.to("cuda", torch.float16)
    steps = 10 if draft else int(req["steps"])
    tokens = 512 if draft else int(req["tokens"] or (2048 if scene else 1024))
    seed = int(req["seed"])

    # The pipeline's step callback is unusable (it references an unbound local), so a
    # heartbeat reports elapsed time instead; the fraction is an estimate, not a measurement.
    import threading
    stop_beat = threading.Event()

    def beat():
        start = time.time()
        while not stop_beat.wait(5.0):
            el = time.time() - start
            emit("progress", stage="denoise", fraction=min(0.85, 0.1 + el / 600.0), message=f"generating, {el:.0f}s elapsed")

    emit("progress", stage="denoise", fraction=0.1, message="generating")
    threading.Thread(target=beat, daemon=True).start()
    torch.cuda.reset_peak_memory_stats()
    img = Image.open(image).convert("RGB")
    t1 = time.time()
    kwargs = dict(image=[img] * parts, attention_kwargs={"num_parts": parts}, num_tokens=tokens,
                  generator=torch.Generator("cuda").manual_seed(seed), num_inference_steps=steps,
                  guidance_scale=7.0, dense_octree_depth=6 if draft else 8,
                  hierarchical_octree_depth=7 if draft else 9, use_flash_decoder=False)
    try:
        with torch.inference_mode():
            meshes = pipe(**kwargs).meshes
    except torch.cuda.OutOfMemoryError:
        stop_beat.set()
        fail("out_of_memory", "GPU out of memory; retry with offload=true, quality=draft or fewer parts")
    stop_beat.set()
    emit("progress", stage="export", fraction=0.9, message="writing meshes")
    written, skipped = [], []
    for i, m in enumerate(meshes):
        if m is None or len(m.faces) == 0:
            skipped.append(i)
            continue
        name = f"part_{i:02d}"
        m.export(out / f"{name}.obj")
        written.append({"index": i, "name": name, "file": f"{name}.obj", "vertices": int(len(m.vertices)),
                        "faces": int(len(m.faces)),
                        "bounds_min": [float(x) for x in m.bounds[0]],
                        "bounds_max": [float(x) for x in m.bounds[1]]})
    if not written:
        fail("empty_result", "model produced no non-empty parts; try another seed or image")
    manifest = {}
    cand = root / "_logs" / "download-partcrafter.json"
    if cand.is_file():
        for r in json.loads(cand.read_text()).get("repositories", []):
            if r["repo"].endswith(model):
                manifest = {"repo": r["repo"], "revision": r.get("revision"), "bytes": r.get("total_bytes")}
    res = {"model": model, "weights": manifest, "image": str(image), "image_sha256": sha256_file(image),
           "mode": req["mode"], "quality": req["quality"], "parts_requested": parts, "seed": seed, "steps": steps,
           "tokens": tokens, "offload": bool(req.get("offload")), "parts": written, "skipped_empty_parts": skipped,
           "seconds_load": t1 - t0, "seconds_generate": time.time() - t1,
           "peak_vram_GB": torch.cuda.max_memory_allocated() / 1e9,
           "peak_rss_GB": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1048576.0,
           "scale_note": "output is in the model's normalised space (arbitrary units); Vernon rescales it"}
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

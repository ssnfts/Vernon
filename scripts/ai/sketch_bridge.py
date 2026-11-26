"""Vernon <-> FLUX.2-klein-4B text-to-picture bridge. Runs inside the AI-3D WSL environment.

Usage: python sketch_bridge.py --request request.json
Reads the request (prompt, width, height, steps, seed), draws one picture with the
installed model and writes sketch.png, sketch.rgb (raw 8-bit RGB, for checks) and
result.json into the request's out_dir. stdout carries one JSON object per line:
progress events, then exactly one `result` or `error` event. Never downloads anything:
it is fully offline.
"""
import argparse
import json
import os
import sys
import time
import traceback
from pathlib import Path

MODEL_ID = "black-forest-labs/FLUX.2-klein-4B"
GUIDANCE = 1.0  # the distilled model takes no real guidance (model card: guidance_scale=1.0)


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
    req = json.loads(Path(linux_path(ap.parse_args().request)).read_text(encoding="utf-8"))
    root = Path(linux_path(req["ai_root"]))
    weights = root / "Sketch"
    model_dir = weights / "model"
    out = Path(linux_path(req["out_dir"]))
    out.mkdir(parents=True, exist_ok=True)
    prompt = str(req["prompt"]).strip()
    width, height = int(req["width"]), int(req["height"])
    steps, seed = int(req["steps"]), int(req["seed"])
    if not prompt:
        fail("empty_prompt", "there is no prompt to draw")
    if not (256 <= width <= 1024 and 256 <= height <= 1024 and width % 16 == 0 and height % 16 == 0):
        fail("bad_size", "width and height must be multiples of 16 between 256 and 1024")
    if not 1 <= steps <= 12:
        fail("bad_steps", "steps must be between 1 and 12")
    if not model_dir.is_dir():
        fail("weights_missing", f"weights folder not found: {model_dir}")
    # Offline before anything is imported.
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    os.environ["HF_HOME"] = str(weights)
    os.environ["OMP_NUM_THREADS"] = "4"
    emit("progress", stage="import", fraction=0.05, message="importing torch and diffusers")
    try:
        import torch
        import numpy as np
        from PIL import Image
        from diffusers import Flux2KleinPipeline
    except Exception as e:  # noqa
        fail("import_failed", f"{type(e).__name__}: {e}")
    torch.set_num_threads(4)
    if not torch.cuda.is_available():
        fail("no_gpu", "CUDA is not available in the model environment")
    emit("progress", stage="load", fraction=0.1, message="reading the weights")
    t0 = time.time()
    # Weights are memory-mapped. On a Windows drive seen from WSL, page faults are very
    # slow, so read the files once from start to end first (fills the page cache).
    files = sorted(model_dir.rglob("*.safetensors"))
    total_bytes = 0
    for f in files:
        total_bytes += f.stat().st_size
        with open(f, "rb", buffering=0) as fh:
            while fh.read(64 << 20):
                pass
    emit("progress", stage="load", fraction=0.4, message="loading FLUX.2-klein-4B")
    free_b, _ = torch.cuda.mem_get_info()
    # Everything on the GPU needs the weights plus room for activations and the VAE.
    offload = total_bytes * 1.1 + 2.0 * 1073741824 > free_b
    try:
        pipe = Flux2KleinPipeline.from_pretrained(str(model_dir), torch_dtype=torch.bfloat16)
        if offload:
            pipe.enable_model_cpu_offload()
        else:
            pipe.to("cuda")
    except torch.cuda.OutOfMemoryError as e:
        fail("out_of_memory", f"GPU ran out of memory: {e}")
    except Exception as e:  # noqa
        fail("load_failed", f"{type(e).__name__}: {e}")
    t1 = time.time()
    emit("progress", stage="generate", fraction=0.6, message="drawing the picture", offload=offload)
    try:
        gen = torch.Generator(device="cuda").manual_seed(seed)
        with torch.inference_mode():
            image = pipe(prompt=prompt, height=height, width=width, guidance_scale=GUIDANCE,
                         num_inference_steps=steps, generator=gen).images[0]
        image = image.convert("RGB")
    except torch.cuda.OutOfMemoryError as e:
        fail("out_of_memory", f"GPU ran out of memory: {e}")
    except Exception as e:  # noqa
        fail("generate_failed", f"{type(e).__name__}: {e}")
    t2 = time.time()
    if list(image.size) != [width, height]:
        fail("generate_failed", f"the model drew {image.size[0]}x{image.size[1]}, not {width}x{height}")
    image.save(out / "sketch.png")
    (out / "sketch.rgb").write_bytes(np.asarray(image, dtype=np.uint8).tobytes())
    rev = ""
    rf = weights / "model-revision.txt"
    if rf.is_file():
        rev = rf.read_text().strip()
    import resource
    import diffusers
    res = {"model": MODEL_ID, "revision": rev, "prompt": prompt, "width": width, "height": height, "steps": steps,
           "seed": seed, "guidance_scale": GUIDANCE, "dtype": "bfloat16", "offload": offload,
           "pipeline": "Flux2KleinPipeline", "diffusers": diffusers.__version__, "torch": torch.__version__,
           "weights_GB": total_bytes / 1073741824.0, "gpu_free_at_start_GB": free_b / 1073741824.0,
           "seconds_load": t1 - t0, "seconds_generate": t2 - t1,
           "peak_vram_GB": torch.cuda.max_memory_allocated() / 1073741824.0,
           "peak_rss_GB": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1048576.0, "device": "cuda"}
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

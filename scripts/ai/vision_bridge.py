"""Vernon <-> Qwen3-VL-4B-Instruct vision bridge. Runs inside the AI-3D WSL environment.

Usage: python vision_bridge.py --request request.json
Reads the request (image path, question, max_tokens), looks at the image with the
installed model and writes result.json into the request's out_dir. stdout carries
one JSON object per line: progress events, then exactly one `result` or `error`
event. Never downloads anything: it is fully offline.
"""
import argparse
import json
import os
import sys
import time
import traceback
from pathlib import Path

MODEL_ID = "Qwen/Qwen3-VL-4B-Instruct"
MAX_SIDE = 1024


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
    weights = root / "Vision"
    model_dir = weights / "model"
    out = Path(linux_path(req["out_dir"]))
    out.mkdir(parents=True, exist_ok=True)
    image_path = Path(linux_path(req["image"]))
    question = str(req["question"]).strip()
    max_tokens = int(req["max_tokens"])
    if not question:
        fail("empty_question", "there is no question to ask")
    if not 16 <= max_tokens <= 1024:
        fail("bad_max_tokens", "max_tokens must be between 16 and 1024")
    if not image_path.is_file():
        fail("image_missing", f"image not found: {image_path}")
    if not model_dir.is_dir():
        fail("weights_missing", f"weights folder not found: {model_dir}")
    # Offline before anything is imported.
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    os.environ["HF_HOME"] = str(weights)
    os.environ["OMP_NUM_THREADS"] = "4"
    emit("progress", stage="import", fraction=0.05, message="importing torch and transformers")
    try:
        import torch
        from PIL import Image
        from transformers import AutoModelForImageTextToText, AutoProcessor
    except Exception as e:  # noqa
        fail("import_failed", f"{type(e).__name__}: {e}")
    torch.set_num_threads(4)
    if not torch.cuda.is_available():
        fail("no_gpu", "CUDA is not available in the model environment")
    try:
        img = Image.open(image_path)
        img.load()
        img = img.convert("RGB")
    except Exception as e:  # noqa
        fail("image_unreadable", f"{type(e).__name__}: {e}")
    orig_size = list(img.size)
    scale = MAX_SIDE / max(img.size)
    if scale < 1.0:
        img = img.resize((max(1, round(img.size[0] * scale)), max(1, round(img.size[1] * scale))), Image.LANCZOS)
    emit("progress", stage="load", fraction=0.15, message="reading the weights")
    t0 = time.time()
    # Weights are memory-mapped. On a Windows drive seen from WSL, page faults are very
    # slow, so read the files once from start to end first (fills the page cache).
    for f in sorted(model_dir.glob("*.safetensors")):
        with open(f, "rb", buffering=0) as fh:
            while fh.read(64 << 20):
                pass
    emit("progress", stage="load", fraction=0.3, message="loading Qwen3-VL-4B-Instruct")
    try:
        model = AutoModelForImageTextToText.from_pretrained(str(model_dir), dtype=torch.bfloat16, device_map="cuda")
        processor = AutoProcessor.from_pretrained(str(model_dir))
    except Exception as e:  # noqa
        fail("load_failed", f"{type(e).__name__}: {e}")
    model.eval()
    t1 = time.time()
    emit("progress", stage="generate", fraction=0.5, message="looking at the image")
    try:
        messages = [{"role": "user", "content": [{"type": "image", "image": img}, {"type": "text", "text": question}]}]
        inputs = processor.apply_chat_template(
            messages, tokenize=True, add_generation_prompt=True, return_dict=True, return_tensors="pt"
        ).to(model.device)
        with torch.inference_mode():
            gen = model.generate(**inputs, max_new_tokens=max_tokens, do_sample=False)
        new_tokens = gen[:, inputs["input_ids"].shape[1]:]
        answer = processor.batch_decode(new_tokens, skip_special_tokens=True, clean_up_tokenization_spaces=False)[0].strip()
    except torch.cuda.OutOfMemoryError as e:
        fail("out_of_memory", f"GPU ran out of memory: {e}")
    except Exception as e:  # noqa
        fail("generate_failed", f"{type(e).__name__}: {e}")
    t2 = time.time()
    rev = ""
    rf = weights / "model-revision.txt"
    if rf.is_file():
        rev = rf.read_text().strip()
    import resource
    res = {"model": MODEL_ID, "revision": rev, "question": question, "answer": answer, "max_tokens": max_tokens,
           "image_size": orig_size, "encoded_size": list(img.size), "new_tokens": int(new_tokens.shape[1]),
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

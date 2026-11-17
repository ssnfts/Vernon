"""Vernon <-> SAM 3D Body bridge (body shape and pose from one photo). Runs inside the
AI-3D WSL environment.

Usage: python bodyfit_bridge.py --request request.json
Reads the request (image), treats the whole picture as one person, runs
facebook/sam-3d-body-dinov3 and writes into out_dir:
  body.obj     the body surface, metres, the model's camera frame (x right, y down, z away)
  result.json  key points (names, 3D in the same frame, 2D in pixels), camera guess, timings
stdout carries one JSON object per line: progress events, then exactly one
`result` or `error` event. Never downloads anything: it is fully offline. The
model code is imported from the pinned checkouts the install script made.
"""
import argparse
import json
import os
import sys
import time
import traceback
from pathlib import Path

MODEL_ID = "facebook/sam-3d-body-dinov3"


def emit(event, **kw):
    print(json.dumps({"event": event, **kw}), flush=True)


def fail(code, message):
    emit("error", code=code, message=message)
    sys.exit(2)


def linux_path(v):
    if isinstance(v, str) and len(v) > 2 and v[1] == ":" and v[2] in "\\/":
        return "/mnt/" + v[0].lower() + "/" + v[3:].replace("\\", "/")
    return v


def read_text(p):
    return p.read_text().strip() if p.is_file() else ""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--request", required=True)
    req = json.loads(Path(linux_path(ap.parse_args().request)).read_text(encoding="utf-8"))
    root = Path(linux_path(req["ai_root"]))
    weights = root / "BodyFit"
    model_dir = weights / "sam-3d-body-dinov3"
    out = Path(linux_path(req["out_dir"]))
    out.mkdir(parents=True, exist_ok=True)
    image_path = Path(linux_path(req["image"]))
    if not image_path.is_file():
        fail("image_missing", f"image not found: {image_path}")
    ckpt, mhr = model_dir / "model.ckpt", model_dir / "assets" / "mhr_model.pt"
    for f in (ckpt, model_dir / "model_config.yaml", mhr):
        if not f.is_file():
            fail("weights_missing", f"model file not found: {f}; run install_bodyfit.sh")
    src = Path(sys.prefix) / "src"
    code, backbone = src / "sam-3d-body", src / "dinov3"
    for d in (code, backbone):
        if not d.is_dir():
            fail("code_missing", f"model code not found: {d}; run install_bodyfit.sh")
    # Offline before anything is imported.
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    os.environ["OMP_NUM_THREADS"] = "4"
    emit("progress", stage="import", fraction=0.05, message="importing torch and the model code")
    try:
        import numpy as np
        import torch
        from PIL import Image, ImageOps

        # The backbone is built through torch.hub from a GitHub name; point it at the local checkout.
        hub_load = torch.hub.load

        def local_hub_load(repo, *a, **kw):
            if repo == "facebookresearch/dinov3":
                kw["source"] = "local"
                return hub_load(str(backbone), *a, **kw)
            raise RuntimeError(f"refusing to fetch {repo} at run time")

        torch.hub.load = local_hub_load
        sys.path.insert(0, str(code))
        from sam_3d_body import SAM3DBodyEstimator, load_sam_3d_body
        from sam_3d_body.metadata.mhr70 import mhr_names
    except Exception as e:  # noqa
        traceback.print_exc(file=sys.stderr)
        fail("import_failed", f"{type(e).__name__}: {e}")
    if not torch.cuda.is_available():
        fail("no_gpu", "this model needs an NVIDIA GPU (CUDA); none is visible")
    torch.set_num_threads(4)
    try:
        img = ImageOps.exif_transpose(Image.open(image_path)).convert("RGB")
    except Exception as e:  # noqa
        fail("image_unreadable", f"{type(e).__name__}: {e}")
    w, h = img.size
    emit("progress", stage="load", fraction=0.15, message="loading SAM 3D Body on cuda")
    t0 = time.time()
    try:
        model, cfg = load_sam_3d_body(str(ckpt), device=torch.device("cuda"), mhr_path=str(mhr))
        estimator = SAM3DBodyEstimator(sam_3d_body_model=model, model_cfg=cfg)
    except Exception as e:  # noqa
        traceback.print_exc(file=sys.stderr)
        fail("load_failed", f"{type(e).__name__}: {e}")
    t1 = time.time()
    emit("progress", stage="fit", fraction=0.7, message="fitting the body")
    try:
        people = estimator.process_one_image(np.asarray(img, dtype=np.uint8), inference_type="full")
    except Exception as e:  # noqa
        traceback.print_exc(file=sys.stderr)
        fail("fit_failed", f"{type(e).__name__}: {e}")
    t2 = time.time()
    if not people:
        fail("no_person", "the model returned no body for this picture")
    o = people[0]
    verts = np.asarray(o["pred_vertices"], dtype=np.float64).reshape(-1, 3)
    faces = np.asarray(estimator.faces, dtype=np.int64).reshape(-1, 3)
    k3 = np.asarray(o["pred_keypoints_3d"], dtype=np.float64).reshape(-1, 3)
    k2 = np.asarray(o["pred_keypoints_2d"], dtype=np.float64).reshape(-1, 2)
    if not (np.isfinite(verts).all() and np.isfinite(k3).all() and np.isfinite(k2).all()):
        fail("bad_output", "the model returned numbers that are not finite")
    if len(verts) == 0 or len(faces) == 0 or faces.min() < 0 or faces.max() >= len(verts):
        fail("bad_output", "the model returned an empty or inconsistent mesh")
    with open(out / "body.obj", "w", encoding="ascii") as f:
        f.write("# body surface, metres, camera frame: x right, y down, z away from the camera\n")
        for v in verts:
            f.write(f"v {v[0]:.6f} {v[1]:.6f} {v[2]:.6f}\n")
        for t in faces:
            f.write(f"f {t[0] + 1} {t[1] + 1} {t[2] + 1}\n")
    names = list(mhr_names)
    keypoints = [{"name": names[i] if i < len(names) else f"point_{i}", "xyz_m": [float(c) for c in k3[i]],
                  "uv_px": [float(c) for c in k2[i]]} for i in range(len(k3))]

    def flat(key):
        return [float(c) for c in np.asarray(o[key], dtype=np.float64).reshape(-1)]

    import resource
    res = {"model": MODEL_ID, "revision": read_text(weights / "model-revision.txt"),
           "code_commit": read_text(code / "COMMIT"), "backbone_commit": read_text(backbone / "COMMIT"),
           "device": "cuda", "image_size": [w, h], "mesh": "body.obj",
           "vertices": int(len(verts)), "triangles": int(len(faces)), "keypoints": keypoints,
           "focal_length_px": flat("focal_length"), "camera_translation_m": flat("pred_cam_t"),
           "shape_params": flat("shape_params"), "scale_params": flat("scale_params"),
           "seconds_load": t1 - t0, "seconds_fit": t2 - t1,
           "peak_vram_GB": torch.cuda.max_memory_allocated() / 1073741824.0,
           "peak_rss_GB": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1048576.0}
    (out / "result.json").write_text(json.dumps(res, indent=2), encoding="utf-8")
    emit("result", **{k: v for k, v in res.items() if k != "keypoints"})


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception as e:  # noqa
        traceback.print_exc(file=sys.stderr)
        fail("exception", f"{type(e).__name__}: {e}")

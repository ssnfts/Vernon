"""Vernon <-> DoMINO (DrivAerML) bridge. Runs inside the AI-3D WSL environment.

Reads a request.json, runs the installed surface and/or volume checkpoints and
writes predictions.npz + result.json. stdout: JSON-lines progress, then one
`result` or `error`. Outputs are de-normalised with the checkpoint's
min_max scaling factors but remain DIMENSIONLESS MODEL UNITS: the mapping to
SI units has not been verified, so none is applied.
Geometry arrives as an STL already in metres in the training frame.
"""
import argparse
import gc
import resource
import hashlib
import io
import json
import os
import pickle
import sys
import time
import traceback
import zipfile
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


class _Loose(pickle.Unpickler):
    """The scaling pickle references a training-side class; read it as a plain bag."""

    def find_class(self, m, n):
        try:
            return super().find_class(m, n)
        except Exception:
            return type(n, (), {})


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--request", required=True)
    req = json.loads(Path(linux_path(ap.parse_args().request)).read_text())
    root = Path(linux_path(req["ai_root"]))
    base = root / "DoMINO"
    out = Path(linux_path(req["out_dir"]))
    out.mkdir(parents=True, exist_ok=True)
    os.environ.setdefault("PYTORCH_CUDA_ALLOC_CONF", "expandable_segments:True")
    emit("progress", stage="import", fraction=0.02, message="importing torch and PhysicsNeMo")
    try:
        import numpy as np
        import torch
        import trimesh
        from omegaconf import OmegaConf
        from physicsnemo.models.domino import DoMINO
        from physicsnemo.datapipes.cae.domino_datapipe import DoMINODataPipe
    except Exception as e:  # noqa
        fail("import_failed", f"{type(e).__name__}: {e}")
    if not torch.cuda.is_available():
        fail("no_cuda", "CUDA is not available inside the WSL environment")
    mesh = trimesh.load(linux_path(req["stl"]), force="mesh", process=False)
    nf = len(mesh.faces)
    if nf < 8:
        fail("bad_geometry", "need at least eight triangles")
    modes = ["surface", "volume"] if req["mode"] == "both" else [req["mode"]]
    arrays, reports = {}, []
    arrays["face_centers_m"] = np.asarray(mesh.triangles_center, dtype=np.float32)
    arrays["face_areas_m2"] = np.asarray(mesh.area_faces, dtype=np.float32)
    for k, mode in enumerate(modes):
        ckpt = base / "weights" / f"domino_drivaerml_{mode}_checkpoint"
        if not ckpt.is_dir():
            fail("weights_missing", f"checkpoint not found: {ckpt}")
        emit("progress", stage=f"{mode}_load", fraction=0.05 + 0.45 * k, message=f"loading {mode} checkpoint")
        cfg = OmegaConf.load(ckpt / "config.yaml")
        with zipfile.ZipFile(ckpt / "DoMINO.0.501.mdlus") as z:
            saved = json.loads(z.read("args.json"))["__args__"]
            saved["model_parameters"] = OmegaConf.create(saved["model_parameters"])
            model = DoMINO(**saved)
            model.load_state_dict(torch.load(io.BytesIO(z.read("model.pt")), map_location="cpu", weights_only=True),
                                  strict=True)
        model.eval().to("cuda")
        with open(ckpt / "scaling_factors.pkl", "rb") as fh:
            sf = _Loose(fh).load()
        key = "surface_fields" if mode == "surface" else "volume_fields"
        fmin = np.asarray(sf.__dict__["min_val"][key], dtype=np.float64)
        fmax = np.asarray(sf.__dict__["max_val"][key], dtype=np.float64)

        def t(x, dt=torch.float32):
            return torch.as_tensor(np.asarray(x).copy(), dtype=dt, device="cuda")

        raw = {"stl_coordinates": t(mesh.vertices), "stl_faces": t(mesh.faces.reshape(-1), torch.int32),
               "stl_centers": t(mesh.triangles_center), "stl_areas": t(mesh.area_faces),
               "surface_mesh_centers": t(mesh.triangles_center), "surface_normals": t(mesh.face_normals),
               "surface_areas": t(mesh.area_faces),
               "global_params_values": t([[30.0], [1.205]]), "global_params_reference": t([[30.0], [1.205]])}
        if mode == "volume":
            pts = np.asarray(req["volume_points_m"], dtype=np.float32).reshape(-1, 3)
            raw["volume_mesh_centers"] = t(pts)
            npts = len(pts)
        else:
            npts = nf
        pipe = DoMINODataPipe(input_path=None, model_type=mode, phase="test",
                              grid_resolution=list(cfg.model.interp_res),
                              bounding_box_dims_surf=cfg.data.bounding_box_surface,
                              bounding_box_dims=cfg.data.bounding_box, normalize_coordinates=True, sampling=False,
                              surface_sampling_algorithm="area_weighted",
                              geom_points_sample=min(300000, len(mesh.vertices)), surface_points_sample=nf,
                              volume_points_sample=npts, num_surface_neighbors=8, gpu_preprocessing=True,
                              gpu_output=True)
        torch.cuda.reset_peak_memory_stats()
        t0 = time.time()
        emit("progress", stage=f"{mode}_infer", fraction=0.1 + 0.45 * k,
             message=f"{mode} inference on {npts} points")
        try:
            data = pipe(raw)
            with torch.inference_mode():
                vol, surf = model(data)
        except torch.cuda.OutOfMemoryError:
            fail("out_of_memory", f"GPU out of memory during {mode} inference; reduce triangles or points")
        except RuntimeError as e:
            if "CUDA" in str(e):  # large meshes surface as 'CUDA driver error: device not ready', not a clean OOM
                fail("gpu_error", f"GPU error during {mode} inference on {nf} triangles ({e}); this usually means too many "
                                  "triangles for the available GPU memory - decimate the mesh and retry")
            raise
        pred = surf if mode == "surface" else vol
        if pred is None or not torch.isfinite(pred).all():
            fail("bad_prediction", f"{mode}: missing or non-finite prediction")
        p = pred.float().cpu().numpy()[0]
        centers = data[mode + "_mesh_centers"].float().cpu().numpy().reshape(-1, 3)
        phys = (p.astype(np.float64) + 1.0) * 0.5 * (fmax - fmin) + fmin  # inverse of min_max_scaling
        arrays[f"{mode}_fields"] = phys.astype(np.float32)
        phys.astype("<f4").tofile(out / f"{mode}_fields.f32")  # raw row-major float32 for Vernon
        arrays[f"{mode}_fields_normalized"] = p
        arrays[f"{mode}_query_normalized"] = centers
        reports.append({"mode": mode, "points": int(len(p)), "seconds": time.time() - t0,
                        "peak_vram_GB": torch.cuda.max_memory_allocated() / 1e9,
                        "order_matches_faces": bool(len(p) == nf) if mode == "surface" else None,
                        "field_names": ["pressure", "shear_x", "shear_y", "shear_z"] if mode == "surface"
                        else ["Ux", "Uy", "Uz", "pressure", "nut"],
                        "scaling": "min_max_scaling inverse; dimensionless model units",
                        "training_min": [float(x) for x in fmin], "training_max": [float(x) for x in fmax]})
        del model, pipe, data, vol, surf, pred
        gc.collect()
        torch.cuda.empty_cache()
    np.savez_compressed(out / "predictions.npz", **arrays)
    man = {}
    mf = base / "weights" / "download-manifest.json"
    if mf.is_file():
        full = json.loads(mf.read_text())
        man = {"repo": full.get("repo"), "revision": full.get("revision")}
    res = {"model": "nvidia/domino_drivaerml", "checkpoint": "0.501", "runs": reports, "inlet_speed_mps": 30.0,
           "air_density": 1.205, "manifest": man,
           "checkpoint_sha256": {m: sha256_file(base / "weights" / f"domino_drivaerml_{m}_checkpoint" /
                                                "DoMINO.0.501.mdlus") for m in modes},
           "label": "AI prediction - CFD validation pending",
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

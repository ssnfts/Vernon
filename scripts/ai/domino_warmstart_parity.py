"""M1 reference parity: DoMINO volume predictions against a labelled DrivAerML slice.

Runs inside the AI environment (WSL, CUDA, TF32 off). Inputs are one DrivAerML
run's STL and one slice VTP (cell data). Writes parity-report.json and, apart
from the model, uses only the labels; it never edits the audit.

Two independent questions:

A. Source units, from the labels alone (no model):
   * pMeanTrim / CpMeanTrim over cells with |Cp| > 0.2 gives the dynamic pressure q
     used by the CFD post-processing; with U_inf from the slice's undisturbed flow,
     q / (0.5 U_inf^2) is 1 for kinematic pressure (or Pa with rho = 1) and rho for Pa.
   * The gauge: median of pMeanTrim - q * CpMeanTrim.
B. The checkpoint's normalisation, by regressing labels on model output:
   * velocity: UMeanTrim_x = V_fit * Ux_hat (no offset); compare V_fit with 30 and 38.889.
   * pressure: pMeanTrim = D_fit * p_hat + c (offset is the gauge); compare D_fit with the
     kinematic-equivalent divisor of every candidate convention at both reference speeds.
   * each fixed candidate is also scored with its offset aligned on the upstream region only,
     so a fitted constant cannot hide a wrong multiplier.

Preregistered criteria (set before the first run, recorded in the report):
   a candidate is "supported" when its fixed multiplier is within 5 % of the fitted one AND its
   relative L2 error is the lowest of all candidates by at least 1.5x over the runner-up.
   "Supported" is reference evidence for a person to accept; the script never writes "verified".

Limits: one run (run_1, membership in the checkpoint's training split unknown), one plane
(y = 0), time-averaged HRLES labels against a RANS-like surrogate trained on them.

  wsl -d AI-3D -u root --exec /opt/ai3d/DoMINO/bin/python -I \
      /mnt/e/.../scripts/ai/domino_warmstart_parity.py --ai-root /mnt/e/AI-3D \
      --stl <run>/drivaer_1.stl --slice <run>/slices/yNormal_p00000.vtp --out <dir>
"""
import argparse
import hashlib
import io
import json
import os
import sys
import time
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from domino_warmstart_scaling import PRESSURE_CONVENTIONS  # noqa: E402

CHANNELS = ["Ux", "Uy", "Uz", "pressure", "nut"]
SEEDS = [11, 12]
RHO = 1.205
SPEEDS = {"checkpoint_30": 30.0, "drivaerml_38.889": 38.889}
CHUNK = 40000
CRITERIA = {"multiplier_within": 0.05, "error_margin_over_runner_up": 1.5}


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for block in iter(lambda: fh.read(1 << 22), b""):
            h.update(block)
    return h.hexdigest()


def rel_l2(pred, ref):
    import numpy as np
    return float(np.linalg.norm(pred - ref) / np.linalg.norm(ref))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ai-root", required=True)
    ap.add_argument("--stl", required=True)
    ap.add_argument("--slice", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    os.environ.setdefault("PYTORCH_CUDA_ALLOC_CONF", "expandable_segments:True")

    import numpy as np
    import pyvista as pv
    import torch
    import trimesh
    from omegaconf import OmegaConf
    import physicsnemo
    from physicsnemo.models.domino import DoMINO
    from physicsnemo.datapipes.cae.domino_datapipe import DoMINODataPipe

    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False
    ck = Path(a.ai_root) / "DoMINO" / "weights" / "domino_drivaerml_volume_checkpoint"
    report = {"schema": "vernon.domino_warmstart_parity.v1", "kind": "reference_parity",
              "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
              "criteria": CRITERIA,
              "environment": {"torch": torch.__version__, "physicsnemo": getattr(physicsnemo, "__version__", "?"),
                              "gpu": torch.cuda.get_device_name(0), "tf32": False},
              "inputs": {"stl": a.stl, "stl_sha256": sha256(a.stl), "slice": a.slice, "slice_sha256": sha256(a.slice),
                         "checkpoint_sha256": {n: sha256(ck / n) for n in ("DoMINO.0.501.mdlus", "scaling_factors.pkl",
                                                                           "config.yaml", "global_stats.json")}}}

    # Labels.
    sl = pv.read(a.slice)
    centres = np.asarray(sl.cell_centers().points, dtype=np.float64)
    lab = {k: np.asarray(sl.cell_data[k], dtype=np.float64) for k in ("UMeanTrim", "pMeanTrim", "CpMeanTrim", "nutMeanTrim")}
    report["slice"] = {"cells": int(sl.n_cells), "bounds": [float(v) for v in sl.bounds],
                       "y_range": [float(centres[:, 1].min()), float(centres[:, 1].max())]}

    # Geometry for the model.
    car = trimesh.load(a.stl, force="mesh", process=True)
    lo, hi = car.bounds
    length_scale = float(np.max(hi - lo))
    report["geometry"] = {"faces": int(len(car.faces)), "vertices": int(len(car.vertices)),
                          "bounds_min_m": lo.tolist(), "bounds_max_m": hi.tolist(), "length_scale_m": length_scale}

    cfg = OmegaConf.load(ck / "config.yaml")
    bmin = np.array(cfg.data.bounding_box.min)
    bmax = np.array(cfg.data.bounding_box.max)
    keep = np.all((centres > bmin + 0.01) & (centres < bmax - 0.01), axis=1)
    pts = centres[keep]
    L = {k: v[keep] for k, v in lab.items()}
    report["slice"]["cells_inside_checkpoint_box"] = int(keep.sum())

    # A. Source units from the labels alone.
    up = (pts[:, 0] < lo[0] - 0.5) & (pts[:, 2] > lo[2] + 0.3) & (pts[:, 2] < hi[2] + 0.5)
    u_inf = float(np.median(L["UMeanTrim"][up, 0]))
    cp, p = L["CpMeanTrim"], L["pMeanTrim"]
    m = np.abs(cp) > 0.2
    q = float(np.median(p[m] / cp[m]))
    gauge = float(np.median(p - q * cp))
    report["source_units"] = {
        "upstream_cells": int(up.sum()), "U_inf_median_mps": u_inf,
        "U_inf_upstream_std_mps": float(L["UMeanTrim"][up, 0].std()),
        "q_from_p_over_cp": q, "q_ratio_spread_p05_p95": np.percentile(p[m] / cp[m], [5, 95]).tolist(),
        "q_over_half_U_inf_sq": q / (0.5 * u_inf ** 2),
        "q_over_half_38.889_sq": q / (0.5 * 38.889 ** 2),
        "gauge_median_p_minus_q_cp": gauge,
        "reading_rule": "q_over_half_U_inf_sq = 1 means kinematic (or Pa with rho 1); = 1.205 means Pa with rho 1.205"}

    # Model.
    with zipfile.ZipFile(ck / "DoMINO.0.501.mdlus") as z:
        saved = json.loads(z.read("args.json"))["__args__"]
        saved["model_parameters"] = OmegaConf.create(saved["model_parameters"])
        model = DoMINO(**saved)
        model.load_state_dict(torch.load(io.BytesIO(z.read("model.pt")), map_location="cpu", weights_only=True), strict=True)
    model.eval().to("cuda")
    import pickle

    class _Bag:
        def __setstate__(self, s):
            self.__dict__.update(s if isinstance(s, dict) else {"state": s})

    class _Loose(pickle.Unpickler):
        def find_class(self, module, name):
            if module.startswith("numpy") or module in ("builtins", "collections", "copyreg", "_codecs"):
                return super().find_class(module, name)
            return _Bag

    with open(ck / "scaling_factors.pkl", "rb") as fh:
        sf = _Loose(fh).load()
    fmin = np.asarray(sf.__dict__["min_val"]["volume_fields"], dtype=np.float64)
    fmax = np.asarray(sf.__dict__["max_val"]["volume_fields"], dtype=np.float64)

    def t(x, dt=torch.float32):
        return torch.as_tensor(np.asarray(x).copy(), dtype=dt, device="cuda")

    base = {"stl_coordinates": t(car.vertices), "stl_faces": t(car.faces.reshape(-1), torch.int32),
            "stl_centers": t(car.triangles_center), "stl_areas": t(car.area_faces),
            "surface_mesh_centers": t(car.triangles_center), "surface_normals": t(car.face_normals),
            "surface_areas": t(car.area_faces),
            "global_params_values": t([[30.0], [RHO]]), "global_params_reference": t([[30.0], [RHO]])}

    def predict(points, seed):
        torch.manual_seed(seed)
        outs = []
        for s in range(0, len(points), CHUNK):
            chunk = points[s:s + CHUNK]
            pipe = DoMINODataPipe(input_path=None, model_type="volume", phase="test",
                                  grid_resolution=list(cfg.model.interp_res),
                                  bounding_box_dims_surf=cfg.data.bounding_box_surface,
                                  bounding_box_dims=cfg.data.bounding_box, normalize_coordinates=True, sampling=False,
                                  surface_sampling_algorithm="area_weighted",
                                  geom_points_sample=len(car.vertices), surface_points_sample=len(car.faces),
                                  volume_points_sample=len(chunk), num_surface_neighbors=8,
                                  gpu_preprocessing=True, gpu_output=True)
            data = pipe(dict(base, volume_mesh_centers=t(chunk)))
            with torch.inference_mode():
                vol, _ = model(data)
            outs.append(vol.double().cpu().numpy()[0])
            del data, vol
            torch.cuda.empty_cache()
        y = np.concatenate(outs)
        if not np.isfinite(y).all():
            raise RuntimeError("non-finite prediction")
        return (y + 1.0) * 0.5 * (fmax - fmin) + fmin  # statistical inverse, still nondimensional

    torch.cuda.reset_peak_memory_stats()
    t0 = time.time()
    runs = [predict(pts, s) for s in SEEDS]
    f = np.mean(runs, axis=0)
    report["timing"] = {"seconds": time.time() - t0, "points": int(len(pts)), "seeds": SEEDS, "chunk": CHUNK,
                        "peak_vram_GB": torch.cuda.max_memory_allocated() / 1e9}
    report["seed_difference_abs_median"] = np.median(np.abs(runs[0] - runs[1]), axis=0).tolist()

    regions = {"all": np.ones(len(pts), bool), "upstream": up,
               "above_car": pts[:, 2] > hi[2] + 0.1,
               "wake": pts[:, 0] > hi[0],
               "near_body": (pts[:, 0] >= lo[0] - 0.5) & (pts[:, 0] <= hi[0]) & (pts[:, 2] <= hi[2] + 0.1)}

    # B1. Velocity reference.
    ux_hat, ux = f[:, 0], L["UMeanTrim"][:, 0]
    v_fit = float(np.dot(ux_hat, ux) / np.dot(ux_hat, ux_hat))
    vel = {"V_fit_mps": v_fit, "candidates": {}}
    for name, v in SPEEDS.items():
        pred = f[:, :3] * v
        vel["candidates"][name] = {"V": v, "multiplier_error": v / v_fit - 1.0,
                                   **{f"rel_l2_{r}": rel_l2(pred[k], L["UMeanTrim"][k]) for r, k in regions.items()}}
    report["velocity"] = vel

    # B2. Pressure divisor (kinematic-equivalent) and gauge.
    p_hat = f[:, 3]
    A = np.stack([p_hat, np.ones_like(p_hat)], axis=1)
    (d_fit, c_fit), *_ = np.linalg.lstsq(A, p, rcond=None)
    pres = {"D_fit_m2_s2": float(d_fit), "offset_fit_m2_s2": float(c_fit),
            "rel_l2_fitted_all": rel_l2(A @ np.array([d_fit, c_fit]), p), "candidates": {}}
    for conv, (factor, source) in PRESSURE_CONVENTIONS.items():
        for sname, v in SPEEDS.items():
            d = factor * v * v * (RHO if source == "kinematic" else 1.0)  # kinematic-equivalent divisor
            off = float(np.median(p[up] - d * p_hat[up]))  # gauge aligned upstream only
            pred = d * p_hat + off
            pres["candidates"][f"{conv}@{sname}"] = {
                "D_m2_s2": d, "multiplier_error": d / d_fit - 1.0, "upstream_offset_m2_s2": off,
                **{f"rel_l2_{r}": rel_l2(pred[k], p[k]) for r, k in regions.items()}}
    report["pressure"] = pres

    # nut (reported only; not imported in the first delivery).
    nut_hat = f[:, 4]
    n_fit = float(np.dot(nut_hat, L["nutMeanTrim"]) / np.dot(nut_hat, nut_hat))
    report["nut"] = {"multiplier_fit": n_fit,
                     "candidates": {s: {"V_times_L": v * length_scale, "multiplier_error": v * length_scale / n_fit - 1.0}
                                    for s, v in SPEEDS.items()},
                     "note": "nut stays unused; reported for completeness"}

    # Preregistered evaluation.
    def judge(cands):
        ranked = sorted(cands.items(), key=lambda kv: kv[1]["rel_l2_all"])
        best, runner = ranked[0], ranked[1]
        ok = (abs(best[1]["multiplier_error"]) <= CRITERIA["multiplier_within"]
              and runner[1]["rel_l2_all"] >= CRITERIA["error_margin_over_runner_up"] * best[1]["rel_l2_all"])
        return {"best": best[0], "runner_up": runner[0], "best_rel_l2": best[1]["rel_l2_all"],
                "runner_up_rel_l2": runner[1]["rel_l2_all"], "supported": bool(ok)}

    report["evaluation"] = {"velocity": judge(vel["candidates"]), "pressure": judge(pres["candidates"]),
                            "note": "'supported' is reference evidence for a person to accept; this script never "
                                    "writes 'verified'"}
    report["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    np.savez_compressed(out / "parity-fields.npz", points=pts, pred_seed_mean=f, ux=ux, p=p,
                        cp=L["CpMeanTrim"], nut=L["nutMeanTrim"], U=L["UMeanTrim"])
    (out / "parity-report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps({k: report[k] for k in ("slice", "source_units", "velocity", "pressure", "nut", "evaluation",
                                              "timing", "seed_difference_abs_median")}, indent=1))


if __name__ == "__main__":
    main()

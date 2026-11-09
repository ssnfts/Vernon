"""M1 audit probe for the DoMINO volume checkpoint (diagnostic only, no CFD output).

Runs inside the AI environment (WSL, CUDA) and writes probe-report.json. It never
marks a channel verified and never chooses a pressure convention.

1. Does scaling_factors.pkl (used by the bridge) equal global_stats.json?
2. Repeatability, reported as separate measurements: the same seed repeated, the
   spread across seeds, a two-chunk split at a fixed seed, and mirror pairs.
3. Consistency checks of the model's own pressure and velocity output: Bernoulli
   coefficient on upstream inviscid probes, stagnation rise and far-field velocity,
   each set beside the value every candidate convention predicts.

Limits, also written into the report:
* A surrogate's own predictions cannot establish its dimensional normalisation or
  gauge. The Bernoulli coefficient is V^2/(2D) for velocity reference V and pressure
  divisor D, so it cannot separate source units from divisor; only the pinned
  preprocessing path or independent reference parity can resolve the convention.
* Seed spread and mirror disagreement are sensitivity diagnostics, not calibrated
  uncertainty and not evidence of correctness. The DrivAer detailed underbody is
  not mirror symmetric, so geometry contributes to mirror disagreement.
* Probe points are hand-placed; no CFD field is compared here.

Usage (from Windows):
  wsl -d AI-3D -u root --exec /opt/ai3d/DoMINO/bin/python \
      /mnt/e/.../scripts/ai/domino_warmstart_probe.py \
      --ai-root /mnt/e/AI-3D --parts-dir /mnt/e/.../tum-drivaer/extracted \
      --top Notchback --out /mnt/e/.../probe-out
"""
import argparse
import hashlib
import inspect
import io
import json
import os
import pickle
import sys
import time
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from domino_warmstart_scaling import (  # noqa: E402
    PRESSURE_CONVENTIONS,
    bernoulli_coefficient,
    consistent_conventions,
    fit_bernoulli,
    inverse_minmax_row,
)

CHANNELS = ["Ux", "Uy", "Uz", "pressure", "nut"]
SEEDS = [11, 12, 13, 14, 15, 16, 17, 18]


class _Bag:
    def __init__(self, *a, **k):
        pass

    def __setstate__(self, state):
        self.__dict__.update(state if isinstance(state, dict) else {"state": state})


class _Loose(pickle.Unpickler):
    """Same restricted reading as domino_bridge.py: training classes become plain bags."""

    def find_class(self, module, name):
        if module.startswith("numpy") or module in ("builtins", "collections", "copyreg", "_codecs"):
            return super().find_class(module, name)
        return _Bag


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for block in iter(lambda: fh.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def load_car(parts_dir, top):
    import numpy as np
    import trimesh
    names = ["part_01_Body.stl", "part_02_UB_Detailed.stl", f"part_03_{top}.stl",
             "part_05_Wheels_Front.stl", "part_06_Wheels_Rear.stl", "part_07_Mirror.stl"]
    parts, hashes = [], {}
    for n in names:
        p = Path(parts_dir) / n
        hashes[n] = sha256(p)
        parts.append(trimesh.load(p, force="mesh", process=True))
    car = trimesh.util.concatenate(parts)
    car.apply_scale(1e-3)  # mm -> m; the source frame already matches the training frame
    lo, hi = car.bounds
    com_y = float(np.average(car.triangles_center[:, 1], weights=car.area_faces))
    return car, hashes, {"bounds_min_m": lo.tolist(), "bounds_max_m": hi.tolist(),
                         "faces": int(len(car.faces)), "vertices": int(len(car.vertices)),
                         "area_weighted_center_y_m": com_y}


def probe_points(car, box_min, box_max):
    import numpy as np
    lo, hi = car.bounds
    nose_x = float(lo[0])
    nose_z = float(car.vertices[np.argmin(car.vertices[:, 0]), 2])
    x0 = box_min[0] + 0.2
    # Upstream block: in front of the nose, above the ground boundary layer.
    xs = np.linspace(x0, nose_x - 0.2, 12)
    ys = np.linspace(-0.8, 0.8, 9)
    zs = np.linspace(0.0, 1.3, 9)
    up = np.array([[x, y, z] for x in xs for y in ys for z in zs])
    # Centreline approach to the nose: strongest speed change, still inviscid.
    line = np.array([[x, 0.0, nose_z] for x in np.linspace(nose_x - 1.0, nose_x - 0.02, 60)])
    # Inlet plane of the model box.
    far = np.array([[x0, y, z] for y in np.linspace(-1.8, 1.8, 7) for z in np.linspace(0.2, 2.6, 7)])
    # Mirror pairs around the car and wake (y > 0.05), skipping points inside the body box.
    rng = np.random.default_rng(7)
    m = rng.uniform([nose_x - 0.5, 0.05, -0.25], [min(box_max[0] - 0.2, hi[0] + 3.0), 1.6, 1.8], (6000, 3))
    inside = np.all((m > lo - 0.02) & (m < hi + 0.02), axis=1)
    m = m[~inside][:2000]
    mirror = np.concatenate([m, m * np.array([1.0, -1.0, 1.0])])
    sets = {"upstream": up, "nose_line": line, "far_field": far, "mirror": mirror}
    return sets, {"nose_x_m": nose_x, "nose_z_m": nose_z}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ai-root", required=True)
    ap.add_argument("--parts-dir", required=True)
    ap.add_argument("--top", default="Notchback", choices=["Notchback", "Fastback", "Estate"])
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    os.environ.setdefault("PYTORCH_CUDA_ALLOC_CONF", "expandable_segments:True")

    import numpy as np
    import torch
    from omegaconf import OmegaConf
    import physicsnemo
    from physicsnemo.models.domino import DoMINO
    from physicsnemo.datapipes.cae.domino_datapipe import DoMINODataPipe
    import physicsnemo.models.domino.solutions as solutions

    # Parity-grade numerics: no TF32 in matmuls or the geometry UNet's 3-D convolutions.
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False

    ck = Path(a.ai_root) / "DoMINO" / "weights" / "domino_drivaerml_volume_checkpoint"
    report = {"schema": "vernon.domino_warmstart_probe.v1", "diagnostic_only": True,
              "cfd_validation": "DEFERRED", "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
              "environment": {"torch": torch.__version__, "cuda": torch.version.cuda,
                              "physicsnemo": getattr(physicsnemo, "__version__", "unknown"),
                              "gpu": torch.cuda.get_device_name(0), "tf32": False},
              "hashes": {n: sha256(ck / n) for n in ("DoMINO.0.501.mdlus", "scaling_factors.pkl",
                                                     "config.yaml", "global_stats.json")}}
    sol_src = inspect.getsource(solutions)
    report["installed_solutions"] = {
        "sha256": hashlib.sha256(sol_src.encode()).hexdigest(),
        "uses_torch_rand": ("rand_like" in sol_src) or ("torch.rand" in sol_src),
        "file": solutions.__file__}

    # 1. Scaler agreement.
    with open(ck / "scaling_factors.pkl", "rb") as fh:
        sf = _Loose(fh).load()
    fmin = np.asarray(sf.__dict__["min_val"]["volume_fields"], dtype=np.float64)
    fmax = np.asarray(sf.__dict__["max_val"]["volume_fields"], dtype=np.float64)
    gs = json.loads((ck / "global_stats.json").read_text())
    g = {k: np.asarray(gs[k]["velocity"] + gs[k]["pressure_volume"] + gs[k]["turbulent_viscosity"])
         for k in ("min", "max")}
    report["scaler"] = {"pkl_min": fmin.tolist(), "pkl_max": fmax.tolist(),
                        "global_stats_min": g["min"].tolist(), "global_stats_max": g["max"].tolist(),
                        "max_abs_difference": float(max(np.max(np.abs(fmin - g["min"])),
                                                        np.max(np.abs(fmax - g["max"]))))}

    # Geometry and model.
    car, part_hashes, geo = load_car(a.parts_dir, a.top)
    report["geometry"] = {"top": a.top, "part_sha256": part_hashes, "transform": "x_m = 1e-3 * x_mm, identity frame", **geo}
    cfg = OmegaConf.load(ck / "config.yaml")
    with zipfile.ZipFile(ck / "DoMINO.0.501.mdlus") as z:
        saved = json.loads(z.read("args.json"))["__args__"]
        saved["model_parameters"] = OmegaConf.create(saved["model_parameters"])
        model = DoMINO(**saved)
        model.load_state_dict(torch.load(io.BytesIO(z.read("model.pt")), map_location="cpu", weights_only=True), strict=True)
    model.eval().to("cuda")
    report["installed_solutions"]["noise_intensity"] = getattr(model.solution_calculator_vol, "noise_intensity", None)
    report["installed_solutions"]["num_sample_points"] = getattr(model.solution_calculator_vol, "num_sample_points", None)

    def t(x, dt=torch.float32):
        return torch.as_tensor(np.asarray(x).copy(), dtype=dt, device="cuda")

    base = {"stl_coordinates": t(car.vertices), "stl_faces": t(car.faces.reshape(-1), torch.int32),
            "stl_centers": t(car.triangles_center), "stl_areas": t(car.area_faces),
            "surface_mesh_centers": t(car.triangles_center), "surface_normals": t(car.face_normals),
            "surface_areas": t(car.area_faces),
            "global_params_values": t([[30.0], [1.205]]), "global_params_reference": t([[30.0], [1.205]])}

    def predict(points, seed):
        torch.manual_seed(seed)
        pipe = DoMINODataPipe(input_path=None, model_type="volume", phase="test",
                              grid_resolution=list(cfg.model.interp_res),
                              bounding_box_dims_surf=cfg.data.bounding_box_surface,
                              bounding_box_dims=cfg.data.bounding_box, normalize_coordinates=True, sampling=False,
                              surface_sampling_algorithm="area_weighted",
                              geom_points_sample=len(car.vertices), surface_points_sample=len(car.faces),
                              volume_points_sample=len(points), num_surface_neighbors=8,
                              gpu_preprocessing=True, gpu_output=True)
        raw = dict(base, volume_mesh_centers=t(points))
        data = pipe(raw)
        with torch.inference_mode():
            vol, _ = model(data)
        y = vol.double().cpu().numpy()[0]
        if not np.isfinite(y).all():
            raise RuntimeError("non-finite prediction")
        return y

    sets, anchors = probe_points(car, list(cfg.data.bounding_box.min), list(cfg.data.bounding_box.max))
    report["probe_anchors"] = anchors
    names = list(sets)
    allp = np.concatenate([sets[n] for n in names])
    cuts = np.cumsum([0] + [len(sets[n]) for n in names])
    torch.cuda.reset_peak_memory_stats()
    t0 = time.time()

    # 2. Repeatability, separate measurements (normalised model output).
    runs = np.stack([predict(allp, s) for s in SEEDS])
    same = predict(allp, SEEDS[0])
    half = len(allp) // 2
    chunked = np.concatenate([predict(allp[:half], SEEDS[0]), predict(allp[half:], SEEDS[0])])

    def per_channel(d):
        d = np.abs(d)
        return {"max": d.max(axis=0).tolist(), "median": np.median(d, axis=0).tolist(),
                "p95": np.percentile(d, 95, axis=0).tolist()}

    spread = runs.std(axis=0)
    report["repeatability"] = {
        "units": "normalised model output (min-max scaled), per channel " + ",".join(CHANNELS),
        "points": int(len(allp)),
        "same_seed_repeat": {"seed": SEEDS[0], "bit_identical": bool(np.array_equal(same, runs[0])),
                             **per_channel(same - runs[0])},
        "seed_spread": {"seeds": SEEDS, "std_max": spread.max(axis=0).tolist(),
                        "std_median": np.median(spread, axis=0).tolist(),
                        "std_p95": np.percentile(spread, 95, axis=0).tolist(),
                        "range_max": (runs.max(axis=0) - runs.min(axis=0)).max(axis=0).tolist(),
                        "meaning": "sensitivity to the solution's random neighbour sampling; not a calibrated uncertainty"},
        "two_chunk_split": {"seed": SEEDS[0], "split_at": int(half), **per_channel(chunked - runs[0]),
                            "meaning": "with one global seed, chunking also changes the random stream; compare "
                                       "with seed_spread before attributing a difference to chunking itself"},
    }
    report["timing"] = {"probe_seconds": time.time() - t0, "points": int(len(allp)),
                        "peak_vram_GB": torch.cuda.max_memory_allocated() / 1e9}

    # Physical-ish fields: statistical inverse of the seed mean (still nondimensional).
    mean = runs.mean(axis=0)
    f = (mean + 1.0) * 0.5 * (fmax - fmin) + fmin
    for i in range(5):  # cross-check the vectorised inverse against the tested pure function
        ref = inverse_minmax_row([float(v) for v in mean[i]], fmin.tolist(), fmax.tolist())
        if max(abs(r - v) for r, v in zip(ref, f[i])) > 1e-9:
            raise RuntimeError("vectorised inverse disagrees with inverse_minmax_row")
    part = {n: f[cuts[k]:cuts[k + 1]] for k, n in enumerate(names)}

    # 3. Consistency checks of pressure and velocity (never a verdict).
    fit_pts = np.concatenate([part["upstream"], part["nose_line"]])
    q = (fit_pts[:, :3] ** 2).sum(axis=1)
    c, const, rms = fit_bernoulli([float(v) for v in fit_pts[:, 3]], [float(v) for v in q])
    ff = part["far_field"]
    p_inf = float(ff[:, 3].mean())
    u_inf_sq = float((ff[:, :3].mean(axis=0) ** 2).sum())
    rho = 1.205
    report["consistency_checks"] = {
        "status": "consistency only; does not verify or select a pressure convention",
        "bernoulli": {"coefficient": c, "constant": const, "rms": rms, "fit_points": int(len(fit_pts)),
                      "speed_sq_range": [float(q.min()), float(q.max())],
                      "model": "p_hat = const - coefficient * |U_hat|^2 over upstream block and nose line"},
        "candidate_coefficients": {
            "pressure_in_Pa_over_rho_u_squared": 0.5,
            "kinematic_pressure_over_u_squared": 0.5,
            "pressure_in_Pa_over_half_rho_u_squared": 1.0,
            "kinematic_pressure_over_rho_u_squared_rho_1.205": 0.5 / rho,
            **{name: bernoulli_coefficient(name, rho) for name in PRESSURE_CONVENTIONS},
            "consistent_within_0.04": consistent_conventions(c, 0.04, rho),
            "note": "coefficient = V^2/(2D); identical for V = 30 and V = 38.89 when D scales with V^2"},
        "far_field": {"U_hat_mean": ff[:, :3].mean(axis=0).tolist(), "U_hat_std": ff[:, :3].std(axis=0).tolist(),
                      "p_hat_mean": p_inf, "p_hat_std": float(ff[:, 3].std()),
                      "candidate_Ux_hat": {"U_ref_equals_freestream": 1.0,
                                           "DrivAerML_38.89_over_checkpoint_30": 38.89 / 30.0}},
        "stagnation": {"nose_line_p_hat_max": float(part["nose_line"][:, 3].max()),
                       "rise_over_far_field": float(part["nose_line"][:, 3].max()) - p_inf,
                       "rise_expected_from_coefficient": c * u_inf_sq,
                       "nose_line_min_speed_sq": float((part["nose_line"][:, :3] ** 2).sum(axis=1).min())},
    }

    # 4. Mirror symmetry.
    mr = part["mirror"]
    n = len(mr) // 2
    a_, b_ = mr[:n], mr[n:].copy()
    b_[:, 1] *= -1.0  # Uy is antisymmetric
    d = np.abs(a_ - b_)
    k = names.index("mirror")
    sm = spread[cuts[k]:cuts[k + 1]] * 0.5 * (fmax - fmin)
    report["mirror"] = {"pairs": int(n), "median_abs": np.median(d, axis=0).tolist(),
                        "p95_abs": np.percentile(d, 95, axis=0).tolist(), "max_abs": d.max(axis=0).tolist(),
                        "seed_spread_std_median_same_points": np.median(sm, axis=0).tolist(),
                        "units": "seed-mean fields after the statistical inverse (nondimensional), channels "
                                 + ",".join(CHANNELS) + "; Uy compared with its sign flipped",
                        "meaning": "consistency diagnostic: geometry asymmetry (detailed underbody), sampling noise "
                                   "and model asymmetry all contribute; not an error estimate"}
    report["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    np.savez_compressed(out / "probe-fields.npz", points=allp, runs_normalised=runs, cuts=cuts,
                        names=np.array(names))
    (out / "probe-report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps({k: report[k] for k in ("scaler", "repeatability", "consistency_checks", "mirror", "timing")}, indent=1))


if __name__ == "__main__":
    main()

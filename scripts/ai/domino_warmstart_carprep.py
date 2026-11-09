"""Turn a styled car .glb into one closed outer shell in the DoMINO checkpoint frame.

Steps per car:
  1. Concatenate every mesh in the scene and rotate model axes into the checkpoint frame
     (x rearward, y left-to-right as in DrivAer, z up).
  2. Find the axles: tyre contact patches when the model has wheels; wheel-arch openings when it
     has none, in which case plain cylinder wheels are added (marked synthetic in the record).
  3. Scale uniformly so the measured wheelbase equals the published one.
  4. Voxelise the surfaces, close panel gaps, fill everything not reachable from outside, and
     contour one closed surface (largest connected piece only).
  5. Translate: front axle at x = 0, bounds centred on y = 0, lowest point at z = -0.3185 m
     (the Notchback reference's tyre contact height, 1.5 mm above the checkpoint box floor).

Published dimensions in CARS are recalled, not looked up; the record says so. The output is a
geometry stand-in for DoMINO sensitivity checks, not a CFD-grade model of the named car.

Usage: python -I domino_warmstart_carprep.py OUT_DIR SRC_ROOT [car ...]
"""
import hashlib
import json
import sys
import time
from pathlib import Path

import numpy as np
import pyvista as pv
import trimesh
from scipy import ndimage

PITCH = 0.006            # voxel size, m
CLOSE_VOXELS = 8         # gap closing radius in voxels (48 mm); 3 (18 mm) let outside air reach the
                         # cavity between skin and trim, leaving 1-voxel walls that smoothing erased
SMOOTH_SIGMA = 1.0       # Gaussian smoothing of the occupancy field before contouring, voxels
GROUND_Z = -0.3185       # Notchback reference contact height
SURFACE_BOX = ([-1.5, -1.4, -0.32], [5.0, 1.4, 1.4])
VOLUME_BOX = ([-3.5, -2.25, -0.32], [8.5, 2.25, 3.0])

# Model axes for all three downloads: x width, y up, z length with the front at +z.
# Checkpoint frame: x_ck = -z_m (rearward), z_ck = y_m (up), y_ck = -x_m (keeps a proper rotation).
ROT = np.array([[0.0, 0.0, -1.0],
                [-1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0]])

CARS = {
    "m4-f82": {
        "file": "m4-f82/bmw_m4_f82.glb", "body": "notchback coupe",
        "published_recalled_m": {"length": 4.671, "wheelbase": 2.812, "width_no_mirrors": 1.870, "height": 1.383},
        "wheels": "model",
    },
    "a7-sportback-c8": {
        "file": "a7-sportback-c8/audi_a7_sportback_mk2_c8_2018-2025.glb", "body": "fastback",
        "published_recalled_m": {"length": 4.969, "wheelbase": 2.926, "width_no_mirrors": 1.908, "height": 1.422},
        # 255/40 R20: diameter 0.508 + 2 * 0.102 = 0.712 m. Track recalled as about 1.64 m.
        "wheels": {"synthetic": True, "radius": 0.356, "width": 0.255, "track": 1.64},
    },
    "m3-touring-g81": {
        "file": "m3-touring-g81/bmw_m3_competition_touring_g81_hd.glb", "body": "estate",
        "published_recalled_m": {"length": 4.801, "wheelbase": 2.857, "width_no_mirrors": 1.903, "height": 1.438},
        "wheels": "model",
    },
}


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def load_rotated(path):
    scene = trimesh.load(path, force="scene")
    parts = []
    for node in scene.graph.nodes_geometry:
        tf, gname = scene.graph[node]
        g = scene.geometry[gname]
        if isinstance(g, trimesh.Trimesh) and len(g.faces):
            m = trimesh.Trimesh(g.vertices.copy(), g.faces.copy(), process=False)
            m.apply_transform(tf)
            parts.append(m)
    mesh = trimesh.util.concatenate(parts)
    mesh.vertices = mesh.vertices @ ROT.T
    return mesh, len(parts)


def split_two(x, what):
    """Split sorted 1-D positions at the largest gap; return (front, rear) medians."""
    x = np.sort(x)
    k = int(np.argmax(np.diff(x)))
    front, rear = x[: k + 1], x[k + 1:]
    if min(len(front), len(rear)) < 10:
        raise ValueError(f"{what}: could not separate front and rear")
    return float(np.median(front)), float(np.median(rear))


def axles_from_contacts(mesh):
    zmin = mesh.bounds[0, 2]
    span = float(np.ptp(mesh.vertices[:, 0]))
    sel = mesh.vertices[:, 2] < zmin + 0.002 * span
    return split_two(mesh.vertices[sel, 0], "contact patches"), int(sel.sum())


def axles_from_arches(mesh):
    """Arch openings: x-runs with no body surface low on the outer sides."""
    pts, _ = trimesh.sample.sample_surface(mesh, 2_000_000, seed=3)
    lo, hi = mesh.bounds
    span = hi[0] - lo[0]
    half_w = 0.5 * (hi[1] - lo[1])
    band = (pts[:, 2] > lo[2] + 0.01 * span) & (pts[:, 2] < lo[2] + 0.03 * span) \
        & (np.abs(pts[:, 1] - 0.5 * (lo[1] + hi[1])) > 0.75 * half_w)
    x = pts[band, 0]
    bins = np.arange(lo[0], hi[0] + 0.01 * span, 0.01 * span)
    occ = np.histogram(x, bins)[0] > 0
    first, last = np.argmax(occ), len(occ) - 1 - np.argmax(occ[::-1])
    runs, start = [], None
    for i in range(first, last + 1):
        if not occ[i] and start is None:
            start = i
        if occ[i] and start is not None:
            runs.append((start, i))
            start = None
    runs = [r for r in runs if (r[1] - r[0]) >= 8]
    if len(runs) != 2:
        raise ValueError(f"arch search found {len(runs)} openings, expected 2")
    mids = [0.5 * (bins[a] + bins[b]) for a, b in runs]
    widths = [bins[b] - bins[a] for a, b in runs]
    # arch top: lowest outer-side surface point above each opening
    tops = []
    for (a, b) in runs:
        inx = (pts[:, 0] > bins[a]) & (pts[:, 0] < bins[b]) \
            & (np.abs(pts[:, 1] - 0.5 * (lo[1] + hi[1])) > 0.75 * half_w) & (pts[:, 2] > lo[2] + 0.03 * span)
        tops.append(float(pts[inx, 2].min()) if inx.any() else None)
    return (mids[0], mids[1]), {"opening_widths": widths, "arch_low_points_z": tops}


def add_wheels(mesh, axles_x, spec, scale_guess):
    """Cylinder wheels, axis along y, resting on z = 0 of the model (taken as ground)."""
    r, w, track = (spec[k] / scale_guess for k in ("radius", "width", "track"))
    yc = 0.5 * (mesh.bounds[0, 1] + mesh.bounds[1, 1])
    wheels = []
    for x in axles_x:
        for side in (-1.0, 1.0):
            c = trimesh.creation.cylinder(radius=r, height=w, sections=96)
            c.apply_transform(trimesh.transformations.rotation_matrix(np.pi / 2, [1, 0, 0]))
            c.apply_translation([x, yc + side * 0.5 * track, r])
            wheels.append(c)
    return trimesh.util.concatenate([mesh] + wheels), {"radius_model_units": r, "axle_z_model_units": r,
                                                       "ground_assumption": "model origin height z = 0"}


def shell(mesh):
    lo, hi = mesh.bounds
    pad = (CLOSE_VOXELS + 4) * PITCH
    origin = lo - pad
    dims = np.ceil((hi - lo + 2 * pad) / PITCH).astype(int) + 1
    n = int(min(4.0 * mesh.area / PITCH ** 2, 40_000_000))
    pts, _ = trimesh.sample.sample_surface(mesh, n, seed=5)
    pts = np.vstack([pts, mesh.vertices])
    idx = np.floor((pts - origin) / PITCH).astype(np.int64)
    occ = np.zeros(dims, dtype=bool)
    occ[idx[:, 0], idx[:, 1], idx[:, 2]] = True
    # alternate 6- and 26-neighbour steps: a rounder structuring element than either alone
    steps = [ndimage.generate_binary_structure(3, 1 if i % 2 == 0 else 3) for i in range(CLOSE_VOXELS)]
    dil = occ
    for st in steps:
        dil = ndimage.binary_dilation(dil, st)
    solid = ndimage.binary_fill_holes(dil)
    for st in steps:
        solid = ndimage.binary_erosion(solid, st, border_value=0)
    field = ndimage.gaussian_filter(solid.astype(np.float32), SMOOTH_SIGMA)
    grid = pv.ImageData(dimensions=dims, spacing=(PITCH,) * 3, origin=origin)
    grid.point_data["f"] = field.ravel(order="F")
    surf = grid.contour([0.5], scalars="f", method="flying_edges").triangulate()
    surf = surf.connectivity("largest").clean()
    faces = surf.faces.reshape(-1, 4)[:, 1:]
    out = trimesh.Trimesh(np.asarray(surf.points, dtype=np.float64), faces, process=True)
    return out, {"sample_points": n, "grid": dims.tolist(), "solid_voxels": int(solid.sum()),
                 "solid_volume_m3": float(solid.sum()) * PITCH ** 3}


def box_check(b, box):
    lo, hi = map(np.asarray, box)
    return {"inside": bool((b[0] >= lo).all() and (b[1] <= hi).all()),
            "margin_min_side_m": (b[0] - lo).round(4).tolist(), "margin_max_side_m": (hi - b[1]).round(4).tolist()}


def prep(name, src_root, out_dir):
    t0 = time.time()
    cfg = CARS[name]
    src = src_root / cfg["file"]
    mesh, n_parts = load_rotated(src)
    raw_bounds = mesh.bounds.copy()
    wb_pub = cfg["published_recalled_m"]["wheelbase"]
    rec = {"car": name, "body": cfg["body"], "source": {"file": cfg["file"], "bytes": src.stat().st_size,
                                                       "sha256": sha256(src), "parts": n_parts,
                                                       "triangles": int(len(mesh.faces))},
           "published_recalled_m": cfg["published_recalled_m"],
           "rotation_model_to_checkpoint": ROT.tolist()}
    if cfg["wheels"] == "model":
        (xf, xr), n_contact = axles_from_contacts(mesh)
        rec["axle_detection"] = {"method": "tyre contact patches", "contact_vertices": n_contact}
    else:
        (xf, xr), info = axles_from_arches(mesh)
        rec["axle_detection"] = {"method": "wheel-arch openings (model has no wheels)", **info}
        mesh, winfo = add_wheels(mesh, (xf, xr), cfg["wheels"], wb_pub / (xr - xf))
        rec["synthetic_wheels"] = {**cfg["wheels"], **winfo,
                                   "note": "plain cylinders, no rims, spokes or rotation; tyre size and track recalled"}
    s = wb_pub / (xr - xf)
    mesh.apply_scale(s)
    rec["scale"] = {"factor": s, "basis": "measured wheelbase -> published wheelbase",
                    "measured_wheelbase_model_units": xr - xf,
                    "length_after_scale_m": float(np.ptp(mesh.vertices[:, 0])),
                    "length_vs_published": float(np.ptp(mesh.vertices[:, 0])) / cfg["published_recalled_m"]["length"] - 1,
                    "height_after_scale_m": float(np.ptp(mesh.vertices[:, 2])),
                    "height_vs_published": float(np.ptp(mesh.vertices[:, 2])) / cfg["published_recalled_m"]["height"] - 1}
    out, sinfo = shell(mesh)
    shift = np.array([-xf * s, -0.5 * (out.bounds[0, 1] + out.bounds[1, 1]), GROUND_Z - out.bounds[0, 2]])
    out.apply_translation(shift)
    b = out.bounds
    stl = out_dir / f"{name}_shell_domino.stl"
    out.export(stl)
    rec["shell"] = {**sinfo, "pitch_m": PITCH, "close_radius_m": CLOSE_VOXELS * PITCH, "smooth_sigma_voxels": SMOOTH_SIGMA,
                    "file": stl.name, "sha256": sha256(stl), "triangles": int(len(out.faces)),
                    "watertight": bool(out.is_watertight), "winding_consistent": bool(out.is_winding_consistent),
                    "euler_number": int(out.euler_number), "volume_m3": float(out.volume), "area_m2": float(out.area)}
    rec["placement"] = {"translation_m": shift.round(6).tolist(), "bounds_min": b[0].round(5).tolist(),
                        "bounds_max": b[1].round(5).tolist(), "front_axle_x_m": 0.0,
                        "rear_axle_x_m": float((xr - xf) * s), "ground_gap_to_box_floor_m": float(b[0, 2] + 0.32)}
    rec["checkpoint_domain"] = {"surface_box": box_check(b, SURFACE_BOX), "volume_box": box_check(b, VOLUME_BOX)}
    rec["raw_bounds_rotated_model_units"] = raw_bounds.round(5).tolist()
    rec["seconds"] = round(time.time() - t0, 1)
    return rec


def main():
    out_dir, src_root = Path(sys.argv[1]), Path(sys.argv[2])
    names = sys.argv[3:] or list(CARS)
    out_dir.mkdir(parents=True, exist_ok=True)
    recs = []
    for name in names:
        rec = prep(name, src_root, out_dir)
        print(json.dumps(rec, indent=1), flush=True)
        recs.append(rec)
    (out_dir / "carprep-record.json").write_text(json.dumps({
        "schema": "vernon.domino_warmstart_carprep.v1", "script_sha256": sha256(Path(__file__)),
        "limitations": [
            "Sources are styled Sketchfab models, not engineering CAD; shut lines, underbody and wheels are approximate.",
            "Published dimensions are recalled, not looked up; the scale rests on the wheelbase.",
            "Voxel shell at 6 mm with 48 mm closing: openings under about 10 cm (shut lines, grilles, the "
            "gap between tyre and arch) are filled, concave corners get about 5 cm fillets, and parts "
            "thinner than about 1 cm (antenna, thin mirror stalks) can vanish.",
            "Front direction and axes were read from rendered views, not metadata.",
        ],
        "cars": recs}, indent=1) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()

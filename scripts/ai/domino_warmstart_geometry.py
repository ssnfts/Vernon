"""Numerical registration record for a DrivAer assembly in the DoMINO checkpoint frame.

Stdlib only, CPU only. Reads ASCII STL parts in millimetres, applies the one
transform under test (x_m = 1e-3 * x_mm, no rotation or shift) and records:

* per-part SHA-256, byte size, triangle count, bounds, area and area-weighted centroid;
* assembled bounds, area-weighted coordinate mean and standard deviation;
* centreline (bounds midpoint and area-weighted y) and a voxel mirror-occupancy score;
* ground contact (lowest wheel z) and front/rear axle centres from each wheel's bounds;
* the same quantities compared with the checkpoint boxes and surface coordinate statistics.

It does not prove that the assembly matches the training geometry. DrivAerML cars
are morphs of the DrivAer notchback, and the checkpoint stores only coordinate
mean/std of surface cells, which are not area-weighted. Differences are reported,
not judged.

    python scripts/ai/domino_warmstart_geometry.py --parts-dir <dir> --top Notchback \
        --weights E:/AI-3D/DoMINO/weights --out docs/research/domino-warmstart-geometry-v1.json
"""
import argparse
import hashlib
import json
import math
import time
from pathlib import Path

SCHEMA = "vernon.domino_warmstart_geometry.v1"
SCALE = 1e-3  # mm -> m
VOXEL = 0.005  # m, mirror-occupancy cell
# Literature values for the DrivAer reference car, used only as a cross-check.
DRIVAER_WHEELBASE_M = 2.786


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for block in iter(lambda: fh.read(1 << 22), b""):
            h.update(block)
    return h.hexdigest()


def triangles(path):
    verts = []
    with open(path, "r", errors="replace") as fh:
        for line in fh:
            s = line.lstrip()
            if s.startswith("vertex"):
                p = s.split()
                verts.append((float(p[1]) * SCALE, float(p[2]) * SCALE, float(p[3]) * SCALE))
                if len(verts) == 3:
                    yield verts
                    verts = []


class Acc:
    """Bounds plus area-weighted first and second moments of triangle centroids."""

    def __init__(self):
        self.lo = [math.inf] * 3
        self.hi = [-math.inf] * 3
        self.n = 0
        self.area = 0.0
        self.s1 = [0.0] * 3
        self.s2 = [0.0] * 3

    def add(self, a, b, c, area, g):
        for i in range(3):
            m = min(a[i], b[i], c[i])
            M = max(a[i], b[i], c[i])
            if m < self.lo[i]:
                self.lo[i] = m
            if M > self.hi[i]:
                self.hi[i] = M
            self.s1[i] += area * g[i]
            self.s2[i] += area * g[i] * g[i]
        self.n += 1
        self.area += area

    def merge(self, o):
        for i in range(3):
            self.lo[i] = min(self.lo[i], o.lo[i])
            self.hi[i] = max(self.hi[i], o.hi[i])
            self.s1[i] += o.s1[i]
            self.s2[i] += o.s2[i]
        self.n += o.n
        self.area += o.area

    def report(self):
        mean = [s / self.area for s in self.s1]
        std = [math.sqrt(max(0.0, s / self.area - m * m)) for s, m in zip(self.s2, mean)]
        return {"triangles": self.n, "area_m2": self.area, "min_m": self.lo, "max_m": self.hi,
                "area_weighted_mean_m": mean, "area_weighted_std_m": std}


def key(ix, iy, iz):
    return (ix * 4096 + iy) * 4096 + iz


def mirror_score(cells):
    """Fraction of occupied voxels whose y-mirror (or a 26-neighbour of it) is occupied."""
    if not cells:
        return None
    hit = 0
    for ix, iy, iz in cells:
        my = -iy - 1  # voxel [iy, iy+1) mirrors to [-iy-1, -iy)
        if any((ix + dx, my + dy, iz + dz) in cells
               for dx in (-1, 0, 1) for dy in (-1, 0, 1) for dz in (-1, 0, 1)):
            hit += 1
    return hit / len(cells)


def scan(path):
    acc = Acc()
    left, right = Acc(), Acc()  # split by centroid y, used for wheels
    cells = set()
    for a, b, c in triangles(path):
        e1 = (b[0] - a[0], b[1] - a[1], b[2] - a[2])
        e2 = (c[0] - a[0], c[1] - a[1], c[2] - a[2])
        nx = e1[1] * e2[2] - e1[2] * e2[1]
        ny = e1[2] * e2[0] - e1[0] * e2[2]
        nz = e1[0] * e2[1] - e1[1] * e2[0]
        area = 0.5 * math.sqrt(nx * nx + ny * ny + nz * nz)
        g = ((a[0] + b[0] + c[0]) / 3, (a[1] + b[1] + c[1]) / 3, (a[2] + b[2] + c[2]) / 3)
        acc.add(a, b, c, area, g)
        (left if g[1] < 0 else right).add(a, b, c, area, g)
        for p in (a, b, c, g):
            cells.add((math.floor(p[0] / VOXEL), math.floor(p[1] / VOXEL), math.floor(p[2] / VOXEL)))
    return acc, left, right, cells


def wheel_centre(side):
    r = side.report()
    lo, hi = r["min_m"], r["max_m"]
    return {"x_bounds_mid_m": (lo[0] + hi[0]) / 2, "z_bounds_mid_m": (lo[2] + hi[2]) / 2,
            "radius_from_z_extent_m": (hi[2] - lo[2]) / 2, "contact_z_m": lo[2],
            "y_range_m": [lo[1], hi[1]], "triangles": r["triangles"]}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--parts-dir", required=True, type=Path)
    ap.add_argument("--top", default="Notchback", choices=("Notchback", "Fastback", "Estate"))
    ap.add_argument("--weights", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    args = ap.parse_args()
    names = ["part_01_Body.stl", "part_02_UB_Detailed.stl", f"part_03_{args.top}.stl",
             "part_05_Wheels_Front.stl", "part_06_Wheels_Rear.stl", "part_07_Mirror.stl"]
    t0 = time.time()
    total, all_cells, parts, wheels = Acc(), set(), [], {}
    for name in names:
        path = args.parts_dir / name
        acc, left, right, cells = scan(path)
        total.merge(acc)
        all_cells |= cells
        row = {"file": name, "bytes": path.stat().st_size, "sha256": sha256(path), **acc.report(),
               "mirror_occupancy": mirror_score(cells)}
        parts.append(row)
        if "Wheels" in name:
            axle = "front" if "Front" in name else "rear"
            wheels[axle] = {"left_y_negative": wheel_centre(left), "right_y_positive": wheel_centre(right)}
        print(f"{name}: {acc.n} triangles, {time.time() - t0:.0f} s", flush=True)

    asm = total.report()
    vol_cfg = args.weights / "domino_drivaerml_volume_checkpoint" / "config.yaml"
    stats = json.loads((args.weights / "domino_drivaerml_surface_checkpoint" / "global_stats.json").read_text())
    # Values transcribed from the volume config; its SHA-256 is recorded so a change is visible.
    vol_box = {"min": [-3.5, -2.25, -0.32], "max": [8.5, 2.25, 3.00]}
    surf_box = {"min": [-1.5, -1.4, -0.32], "max": [5.0, 1.4, 1.4]}
    inside = lambda box: all(box["min"][i] <= asm["min_m"][i] and asm["max_m"][i] <= box["max"][i] for i in range(3))
    margin = lambda box: {"min_side_m": [asm["min_m"][i] - box["min"][i] for i in range(3)],
                          "max_side_m": [box["max"][i] - asm["max_m"][i] for i in range(3)]}
    fm = [(wheels["front"][s]["x_bounds_mid_m"], wheels["front"][s]["z_bounds_mid_m"]) for s in wheels["front"]]
    rm = [(wheels["rear"][s]["x_bounds_mid_m"], wheels["rear"][s]["z_bounds_mid_m"]) for s in wheels["rear"]]
    front_x = sum(p[0] for p in fm) / 2
    rear_x = sum(p[0] for p in rm) / 2
    ground = min(w["contact_z_m"] for axle in wheels.values() for w in axle.values())
    m_s, s_s = stats["mean"]["coordinates"], stats["std_dev"]["coordinates"]
    record = {
        "schema": SCHEMA,
        "transform": "x_m = 1e-3 * x_mm; no rotation, no translation",
        "parts_dir": str(args.parts_dir), "top": args.top,
        "checkpoint_files": {"volume/config.yaml": sha256(vol_cfg),
                             "surface/global_stats.json": sha256(args.weights / "domino_drivaerml_surface_checkpoint" / "global_stats.json")},
        "parts": parts,
        "assembly": {**asm, "mirror_occupancy": mirror_score(all_cells), "voxel_m": VOXEL},
        "centreline": {"bounds_mid_y_m": (asm["min_m"][1] + asm["max_m"][1]) / 2,
                       "area_weighted_y_m": asm["area_weighted_mean_m"][1]},
        "wheels": wheels,
        "axles": {"front_x_m": front_x, "rear_x_m": rear_x, "wheelbase_m": rear_x - front_x,
                  "front_z_m": sum(p[1] for p in fm) / 2, "rear_z_m": sum(p[1] for p in rm) / 2,
                  "literature_wheelbase_m": DRIVAER_WHEELBASE_M},
        "ground": {"lowest_wheel_z_m": ground, "checkpoint_box_min_z_m": vol_box["min"][2],
                   "gap_m": ground - vol_box["min"][2]},
        "checkpoint_domain": {
            "volume_box": {**vol_box, "assembly_inside": inside(vol_box), "margins": margin(vol_box)},
            "surface_box": {**surf_box, "assembly_inside": inside(surf_box), "margins": margin(surf_box)},
            "surface_coordinate_stats": {
                "checkpoint_mean_m": m_s, "checkpoint_std_m": s_s,
                "assembly_area_weighted_mean_m": asm["area_weighted_mean_m"],
                "assembly_area_weighted_std_m": asm["area_weighted_std_m"],
                "mean_difference_in_checkpoint_std": [(asm["area_weighted_mean_m"][i] - m_s[i]) / s_s[i] for i in range(3)],
                "caveat": "Checkpoint statistics are over DrivAerML surface cells (not area-weighted, refined near features, "
                          "500 morphs); the min/max coordinate fields in global_stats.json repeat mean/std and are not bounds."},
        },
        "limitations": [
            "Registration of the transform is consistent with the checkpoint boxes and statistics; it is not a match to any training car.",
            "DrivAerML geometries are morphs of the DrivAer notchback; a DrivAerML STL (run_<n>/drivaer_<n>.stl) is needed for a direct comparison.",
            "Mirror occupancy is a voxel test at the stated size; it does not measure sub-voxel asymmetry.",
            "Axle centres come from wheel-part bounds and assume each wheel part is a closed tyre plus rim.",
        ],
        "seconds": round(time.time() - t0, 1),
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(record, indent=1) + "\n", encoding="utf-8")
    print(json.dumps({k: record[k] for k in ("centreline", "axles", "ground")}, indent=1))
    print("assembly", json.dumps(record["assembly"], indent=1))


if __name__ == "__main__":
    main()

"""Post-hoc semantic checks of the run_1 reference-parity fields (CPU, NumPy only).

Reads the `parity-fields.npz` written by domino_warmstart_parity.py (seed-mean
statistical-inverse predictions and the DrivAerML run_1 y = 0 slice labels) and
prints a JSON block for docs/research/domino-warmstart-scaling-semantics-v1.json:

- each velocity component fitted on its own, so Uy and Uz are not carried by Ux;
- every permutation of the three velocity channels, to detect axis/order mistakes;
- the sign of each component;
- the pressure divisor fitted with and without a gauge, and the candidate divisors
  that the factor-of-two, extra-density-division and wrong-density mistakes give.

These checks were chosen after the parity run and are not preregistered. They
measure prediction-versus-label agreement (surrogate error included); they are not
software parity against a reference implementation. Run in the AI environment:

    python -I domino_warmstart_semantics_check.py PATH/parity-fields.npz
"""
import hashlib
import itertools
import json
import sys

import numpy as np

U_NORM = 30.0
RHO_NORM = 1.205


def rel_l2(a, b):
    return float(np.linalg.norm(a - b) / np.linalg.norm(b))


def main():
    path = sys.argv[1]
    d = np.load(path)
    f, U, p = d["pred_seed_mean"], d["U"], d["p"]
    out = {"input_npz_sha256": hashlib.sha256(open(path, "rb").read()).hexdigest(),
           "points": int(len(p)), "slice_abs_y_max_m": float(np.abs(d["points"][:, 1]).max())}

    comps = {}
    for i, name in enumerate(("Ux", "Uy", "Uz")):
        a, b = f[:, i], U[:, i]
        k = float(a @ b / (a @ a))
        comps[name] = {"fitted_multiplier_mps": k, "multiplier_error_of_30": U_NORM / k - 1.0,
                       "label_rms_mps": float(np.sqrt(np.mean(b * b))),
                       "correlation": float(np.corrcoef(a, b)[0, 1]),
                       "rel_l2_at_30": rel_l2(U_NORM * a, b), "rel_l2_sign_flipped": rel_l2(-U_NORM * a, b)}
    out["velocity_components"] = comps

    perms = {}
    for perm in itertools.permutations(range(3)):
        perms["".join("xyz"[j] for j in perm)] = rel_l2(U_NORM * f[:, list(perm)], U)
    out["velocity_channel_permutations_rel_l2"] = perms

    ph = f[:, 3]
    A = np.stack([ph, np.ones_like(ph)], axis=1)
    (dfit, cfit), *_ = np.linalg.lstsq(A, p, rcond=None)
    rng = np.random.default_rng(0)
    idx = rng.integers(0, len(p), (200000, 2))
    dp, dph = p[idx[:, 0]] - p[idx[:, 1]], ph[idx[:, 0]] - ph[idx[:, 1]]
    out["pressure_divisor"] = {
        "least_squares_with_gauge_m2_s2": float(dfit), "least_squares_gauge_m2_s2": float(cfit),
        "gauge_free_pairwise_difference_fit_m2_s2": float(dph @ dp / (dph @ dph)),
        "candidates": {}}
    cands = {"kinematic_rho_norm_u_norm_sq": RHO_NORM * U_NORM ** 2,
             "extra_density_division_u_norm_sq": U_NORM ** 2,
             "factor_two_half_u_norm_sq": 0.5 * U_NORM ** 2,
             "wrong_density_1.184_u_norm_sq": 1.184 * U_NORM ** 2,
             "training_speed_38.889_kinematic": RHO_NORM * 38.889 ** 2}
    for name, D in cands.items():
        off = float(np.median(p - D * ph))
        out["pressure_divisor"]["candidates"][name] = {
            "D_m2_s2": D, "error_vs_gauge_free_fit": D / out["pressure_divisor"]["gauge_free_pairwise_difference_fit_m2_s2"] - 1.0,
            "global_median_gauge_m2_s2": off, "rel_l2_all": rel_l2(D * ph + off, p)}
    print(json.dumps(out, indent=1))


if __name__ == "__main__":
    main()

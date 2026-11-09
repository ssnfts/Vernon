"""Semantic tests for the DoMINO warm-start unit conversion (no GPU, no NumPy).

Expected values are worked by hand in comments. Run from the checkout root:

    python -m unittest discover -s scripts/ai -p test_domino_warmstart_semantics.py -v

Optional, checked when set (the test reports a skip otherwise):
    VERNON_DOMINO_WEIGHTS   folder holding domino_drivaerml_volume_checkpoint/
    VERNON_DRIVAERML_RUN1   folder holding drivaer_1.stl and slices/yNormal_p00000.vtp
"""
import hashlib
import json
import math
import os
import unittest
from pathlib import Path

from domino_warmstart_units import (
    Reference,
    case_kinematic_pressure,
    case_velocity,
    identify_velocity_order,
    kinematic_to_pascal,
    least_squares_gauge,
    operating_point_ratio,
    reference_from_records,
    reference_kinematic_pressure,
    reference_velocity,
    volume_channel_order,
)

ROOT = Path(__file__).resolve().parents[2]
RESEARCH = ROOT / "docs" / "research"
REF = Reference(u_norm=30.0, rho_norm=1.205, u_train=38.889, p_train_ref=0.0)
OPEN = {"status": "unresolved"}
NAN = float("nan")


def load(name):
    return json.loads((RESEARCH / name).read_text(encoding="utf-8"))


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for block in iter(lambda: fh.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


class ReferenceConditionTests(unittest.TestCase):
    def test_velocity_at_reference_sample_conditions(self):
        # U_hat = 1.2963 is the training freestream: 30 * 1.2963 = 38.889 m/s, not 30.
        u = reference_velocity((38.889 / 30.0, 0.0, 0.0), REF)
        self.assertAlmostEqual(u[0], 38.889, places=9)
        self.assertEqual(reference_velocity((0.5, -0.2, 0.1), REF), (15.0, -6.0, 3.0))

    def test_pressure_is_kinematic_with_one_density_factor(self):
        # 1.205 * 30^2 * 0.2 = 216.9 m^2/s^2.
        self.assertAlmostEqual(reference_kinematic_pressure(0.2, REF), 216.9, places=9)

    def test_extra_density_division_is_detected(self):
        # The plan-6.2 Pa reading divides by rho: 30^2 * 0.2 = 180, smaller by exactly 1.205.
        p = reference_kinematic_pressure(0.2, REF)
        self.assertNotAlmostEqual(p, 180.0, places=3)
        self.assertAlmostEqual(p / 180.0, 1.205, places=12)

    def test_factor_two_is_detected(self):
        # 0.5 * rho U^2 would give 108.45: twice as small.
        self.assertAlmostEqual(reference_kinematic_pressure(0.2, REF) / 108.45, 2.0, places=12)

    def test_case_air_density_never_enters_the_kinematic_value(self):
        # Pa only for reporting: 216.9 * 1.184 = 256.8096 Pa. One multiplication, no division.
        self.assertAlmostEqual(kinematic_to_pascal(216.9, 1.184), 256.8096, places=9)
        with self.assertRaises(ValueError):
            kinematic_to_pascal(216.9, 0.0)

    def test_nonfinite_refused(self):
        with self.assertRaises(ValueError):
            reference_kinematic_pressure(NAN, REF)
        with self.assertRaises(ValueError):
            reference_velocity((0.1, NAN, 0.0), REF)
        with self.assertRaises(ValueError):
            reference_velocity((0.1, 0.0), REF)

    def test_reference_values_validated(self):
        for bad in ((0.0, 1.205, 38.889), (30.0, -1.0, 38.889), (30.0, 1.205, NAN)):
            with self.assertRaises(ValueError, msg=str(bad)):
                Reference(u_norm=bad[0], rho_norm=bad[1], u_train=bad[2], p_train_ref=0.0)


class OperatingPointTests(unittest.TestCase):
    VERIFIED = {"status": "verified"}

    def test_refused_while_gate_open(self):
        with self.assertRaises(ValueError):
            operating_point_ratio(16.0, REF, OPEN)
        with self.assertRaises(ValueError):
            case_velocity((1.0, 0.0, 0.0), REF, 16.0, OPEN)
        with self.assertRaises(ValueError):
            case_kinematic_pressure(0.2, REF, 16.0, OPEN)

    def test_explicit_hypothesis_is_allowed_for_diagnostics(self):
        s = operating_point_ratio(16.0, REF, OPEN, assume_reynolds_independence=True)
        self.assertAlmostEqual(s, 16.0 / 38.889, places=12)

    def test_training_freestream_maps_to_case_freestream(self):
        # 30 * (38.889/30) * (16/38.889) = 16 exactly: the case inlet, not 20.74 m/s.
        u = case_velocity((38.889 / 30.0, 0.0, 0.0), REF, 16.0, self.VERIFIED)
        self.assertAlmostEqual(u[0], 16.0, places=9)
        self.assertNotAlmostEqual(u[0], 16.0 * 38.889 / 30.0, places=3)

    def test_pressure_scales_with_speed_squared(self):
        # 216.9 * (16/38.889)^2 = 36.7152 m^2/s^2; the plan-6.2 formula gave 51.2 (x1.3945).
        p = case_kinematic_pressure(0.2, REF, 16.0, self.VERIFIED)
        self.assertAlmostEqual(p, 216.9 * (16.0 / 38.889) ** 2, places=9)
        self.assertAlmostEqual(51.2 / p, (38.889 / 30.0) ** 2 / 1.205, places=9)

    def test_identity_at_training_speed(self):
        self.assertAlmostEqual(case_kinematic_pressure(0.2, REF, 38.889, self.VERIFIED), 216.9, places=9)

    def test_bad_case_speed_refused(self):
        for u in (0.0, -16.0, NAN):
            with self.assertRaises(ValueError, msg=str(u)):
                operating_point_ratio(u, REF, self.VERIFIED)

    def test_unknown_gate_status_refused(self):
        with self.assertRaises(ValueError):
            operating_point_ratio(16.0, REF, {"status": "probably"})


class GaugeTests(unittest.TestCase):
    def test_gauge_is_additive_and_gradients_unchanged(self):
        pred = [10.0, 20.0, -5.0, 3.0]
        control = [x + 7.5 for x in pred]
        c = least_squares_gauge(pred, control)
        self.assertAlmostEqual(c, 7.5, places=12)
        aligned = [x + c for x in pred]
        self.assertEqual([aligned[i] - aligned[0] for i in range(4)], [pred[i] - pred[0] for i in range(4)])

    def test_gauge_does_not_absorb_a_scale_error(self):
        # A wrong multiplier leaves a residual that no additive constant removes.
        pred = [10.0, 20.0, -5.0, 3.0]
        control = [1.205 * x for x in pred]
        c = least_squares_gauge(pred, control)
        res = [control[i] - pred[i] - c for i in range(4)]
        self.assertGreater(max(abs(r) for r in res), 1.0)

    def test_bad_inputs_refused(self):
        with self.assertRaises(ValueError):
            least_squares_gauge([1.0], [1.0, 2.0])
        with self.assertRaises(ValueError):
            least_squares_gauge([], [])
        with self.assertRaises(ValueError):
            least_squares_gauge([1.0, NAN], [1.0, 2.0])


class ChannelOrderTests(unittest.TestCase):
    # Asymmetric manufactured field: every component differs, so a swap or a sign shows.
    LABEL = [(30.0 + i, 5.0 - 0.7 * i, -2.0 + 0.3 * i * i) for i in range(8)]

    def hats(self, perm, signs=(1, 1, 1)):
        return [tuple(signs[k] * row[perm[k]] / 30.0 for k in range(3)) for row in self.LABEL]

    def test_identity_found(self):
        order, margin = identify_velocity_order(self.hats((0, 1, 2)), self.LABEL, REF)
        self.assertEqual(order, ((0, 1, 2), (1, 1, 1)))
        self.assertGreater(margin, 10.0)

    def test_swapped_axes_found(self):
        order, _ = identify_velocity_order(self.hats((0, 2, 1)), self.LABEL, REF)
        self.assertEqual(order, ((0, 2, 1), (1, 1, 1)))

    def test_cyclic_order_found(self):
        # A 3-cycle is not its own inverse, so it tells channel->axis from axis->channel.
        for perm in ((1, 2, 0), (2, 0, 1)):
            order, _ = identify_velocity_order(self.hats(perm), self.LABEL, REF)
            self.assertEqual(order, (perm, (1, 1, 1)), perm)

    def test_flipped_sign_found(self):
        order, _ = identify_velocity_order(self.hats((0, 1, 2), (1, -1, 1)), self.LABEL, REF)
        self.assertEqual(order, ((0, 1, 2), (1, -1, 1)))

    def test_config_channel_order(self):
        text = ("variables:\n  surface:\n    solution:\n      pMeanTrim: scalar\n"
                "  volume:\n    solution:\n      # comment\n      UMeanTrim: vector\n"
                "      pMeanTrim: scalar\n      nutMeanTrim: scalar\n  global_parameters:\n")
        self.assertEqual(volume_channel_order(text), ["Ux", "Uy", "Uz", "pressure", "nut"])
        swapped = text.replace("      UMeanTrim: vector\n      pMeanTrim: scalar\n",
                               "      pMeanTrim: scalar\n      UMeanTrim: vector\n")
        self.assertEqual(volume_channel_order(swapped), ["pressure", "Ux", "Uy", "Uz", "nut"])
        with self.assertRaises(ValueError):
            volume_channel_order(text.replace("nutMeanTrim: scalar", "kMeanTrim: scalar"))
        with self.assertRaises(ValueError):
            volume_channel_order("variables:\n  surface:\n    solution:\n      pMeanTrim: scalar\n")


class RecordTests(unittest.TestCase):
    """The checked-in records must agree with each other and with the preregistered criteria."""

    def setUp(self):
        self.audit = load("domino-warmstart-scaling-v1.json")
        self.sem = load("domino-warmstart-scaling-semantics-v1.json")
        self.parity = load("domino-warmstart-parity-run1-20261109.json")

    def test_reference_built_from_records(self):
        ref = reference_from_records(self.audit, self.sem)
        self.assertEqual(ref, REF)

    def test_reference_refused_if_a_channel_is_not_verified(self):
        audit = json.loads(json.dumps(self.audit))
        audit["channels"][3]["status"] = "unresolved"
        with self.assertRaises(ValueError):
            reference_from_records(audit, self.sem)

    def test_reference_refused_if_constants_disagree(self):
        sem = json.loads(json.dumps(self.sem))
        sem["constants"]["u_norm_mps"] = 38.889
        with self.assertRaises(ValueError):
            reference_from_records(self.audit, sem)

    def test_parity_used_the_exact_checkpoint(self):
        model = self.audit["model"]["sha256"]
        for name, digest in self.parity["inputs"]["checkpoint_sha256"].items():
            self.assertEqual(digest, model[f"volume/{name}"], name)

    def test_parity_used_the_recorded_reference_files(self):
        files = self.audit["reference_parity_runs"][0]["reference_data"]["files"]
        self.assertEqual(self.parity["inputs"]["stl_sha256"], files["run_1/drivaer_1.stl"]["sha256"])
        self.assertEqual(self.parity["inputs"]["slice_sha256"], files["run_1/slices/yNormal_p00000.vtp"]["sha256"])

    def test_preregistered_criteria_recomputed_from_numbers(self):
        crit = self.parity["criteria"]
        for field, expected in (("velocity", "checkpoint_30"), ("pressure", "kinematic_over_rho_u_squared@checkpoint_30")):
            cands = self.parity[field]["candidates"]
            ranked = sorted(cands, key=lambda k: cands[k]["rel_l2_all"])
            best, runner = cands[ranked[0]], cands[ranked[1]]
            self.assertEqual(ranked[0], expected, field)
            self.assertLessEqual(abs(best["multiplier_error"]), crit["multiplier_within"], field)
            self.assertGreaterEqual(runner["rel_l2_all"], crit["error_margin_over_runner_up"] * best["rel_l2_all"], field)

    def test_parity_candidate_divisors_are_the_formulas(self):
        c = self.parity["pressure"]["candidates"]
        self.assertAlmostEqual(c["kinematic_over_rho_u_squared@checkpoint_30"]["D_m2_s2"], 1.205 * 900.0, places=9)
        self.assertAlmostEqual(c["rho_u_squared@checkpoint_30"]["D_m2_s2"], 900.0, places=9)
        self.assertAlmostEqual(reference_kinematic_pressure(1.0, REF), 1084.5, places=9)

    def test_post_hoc_checks_are_labelled_and_consistent(self):
        post = self.sem["post_hoc_checks"]
        self.assertFalse(post["preregistered"])
        m = post["measured"]
        self.assertEqual(m["points"], self.parity["slice"]["cells_inside_checkpoint_box"])
        comps = m["velocity_components"]
        self.assertLess(abs(comps["Ux"]["multiplier_error_of_30"]), 0.05)
        self.assertLess(abs(comps["Uz"]["multiplier_error_of_30"]), 0.05)
        for name in ("Ux", "Uy", "Uz"):
            self.assertLess(comps[name]["rel_l2_at_30"], comps[name]["rel_l2_sign_flipped"], name)
        perms = m["velocity_channel_permutations_rel_l2"]
        best = min(perms, key=perms.get)
        self.assertEqual(best, "xyz")
        self.assertGreater(min(v for k, v in perms.items() if k != "xyz") / perms["xyz"], 3.0)
        self.assertAlmostEqual(perms["xyz"], self.parity["velocity"]["candidates"]["checkpoint_30"]["rel_l2_all"], places=12)

    def test_uy_magnitude_is_not_claimed_from_this_slice(self):
        uy = self.sem["post_hoc_checks"]["measured"]["velocity_components"]["Uy"]
        self.assertGreater(abs(uy["multiplier_error_of_30"]), 0.05)  # the fit cannot pin Uy's scale
        self.assertEqual(self.sem["channel_semantic_status"]["Uy"], "sign_only_magnitude_by_shared_convention")

    def test_divisor_separates_structural_mistakes_but_not_density_digits(self):
        cands = self.sem["post_hoc_checks"]["measured"]["pressure_divisor"]["candidates"]
        self.assertLess(abs(cands["kinematic_rho_norm_u_norm_sq"]["error_vs_gauge_free_fit"]), 0.05)
        for wrong in ("extra_density_division_u_norm_sq", "factor_two_half_u_norm_sq", "training_speed_38.889_kinematic"):
            self.assertGreater(abs(cands[wrong]["error_vs_gauge_free_fit"]), 0.10, wrong)
        # 1.184 fits as well as 1.205: the density digits come from config.yaml, not from this fit.
        self.assertLess(abs(cands["wrong_density_1.184_u_norm_sq"]["error_vs_gauge_free_fit"]), 0.05)
        self.assertIn("not rho_norm 1.205 from the case air density", self.sem["evidence_classification"]["discriminating_power"])

    def test_software_parity_kept_separate_from_prediction_error(self):
        ev = self.sem["evidence_classification"]
        self.assertEqual(ev["software_parity"]["status"], "not_established")
        self.assertTrue(ev["run_1_reference_parity"].startswith("label_regression_identification"))
        for row in self.audit["channels"]:
            self.assertNotIn("software parity", (row.get("source") or "").lower(), row["name"])

    def test_operating_point_numbers(self):
        n = self.sem["conversions"]["operating_point_numbers_at_16_mps"]
        self.assertAlmostEqual(n["velocity_multiplier_mps"], 30.0 * 16.0 / 38.889, places=9)
        self.assertAlmostEqual(n["pressure_multiplier_m2_s2"], 1084.5 * (16.0 / 38.889) ** 2, places=9)
        self.assertEqual(self.audit["normalization"]["operating_point_gate"]["status"], "unresolved")

    def test_channel_order_matches_audit(self):
        self.assertEqual(self.sem["constants"]["channel_order"], [c["name"] for c in self.audit["channels"]])


class InstalledArtifactTests(unittest.TestCase):
    """Hashes and channel order of the installed checkpoint and the local reference sample."""

    def setUp(self):
        self.audit = load("domino-warmstart-scaling-v1.json")

    def test_checkpoint_files_match_audit(self):
        root = os.environ.get("VERNON_DOMINO_WEIGHTS")
        if not root:
            self.skipTest("VERNON_DOMINO_WEIGHTS not set")
        vol = Path(root) / "domino_drivaerml_volume_checkpoint"
        for name in ("DoMINO.0.501.mdlus", "scaling_factors.pkl", "config.yaml", "global_stats.json"):
            self.assertEqual(sha256(vol / name), self.audit["model"]["sha256"][f"volume/{name}"], name)
        order = volume_channel_order((vol / "config.yaml").read_text(encoding="utf-8"))
        self.assertEqual(order, [c["name"] for c in self.audit["channels"]])

    def test_reference_sample_files_match_audit(self):
        root = os.environ.get("VERNON_DRIVAERML_RUN1")
        if not root:
            self.skipTest("VERNON_DRIVAERML_RUN1 not set")
        files = self.audit["reference_parity_runs"][0]["reference_data"]["files"]
        self.assertEqual(sha256(Path(root) / "drivaer_1.stl"), files["run_1/drivaer_1.stl"]["sha256"])
        self.assertEqual(sha256(Path(root) / "slices" / "yNormal_p00000.vtp"),
                         files["run_1/slices/yNormal_p00000.vtp"]["sha256"])


if __name__ == "__main__":
    unittest.main()

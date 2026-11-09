"""No-GPU analytical tests for domino_warmstart_scaling.

Expected values are worked out by hand in the comments, never by calling the
conversion backwards. Run from the checkout root:

    python -m unittest discover -s scripts/ai -p test_domino_warmstart_scaling.py -v
"""
import json
import unittest
from pathlib import Path

from domino_warmstart_scaling import (
    KNOWN_CONVENTIONS,
    bernoulli_coefficient,
    consistent_conventions,
    fit_bernoulli,
    inverse_minmax,
    inverse_minmax_row,
    pressure_to_kinematic,
    velocity_to_si,
    verified_convention,
)

NAN = float("nan")
INF = float("inf")


class InverseMinMaxTests(unittest.TestCase):
    def test_minmax_endpoints_and_midpoint(self):
        self.assertEqual(inverse_minmax(-1.0, -2.0, 4.0), -2.0)
        self.assertEqual(inverse_minmax(0.0, -2.0, 4.0), 1.0)
        self.assertEqual(inverse_minmax(1.0, -2.0, 4.0), 4.0)

    def test_extrapolation_is_reported_not_clamped(self):
        # (2 + 1) / 2 * 6 - 2 = 7, outside [-2, 4] on purpose.
        self.assertEqual(inverse_minmax(2.0, -2.0, 4.0), 7.0)
        self.assertEqual(inverse_minmax(-3.0, -2.0, 4.0), -8.0)

    def test_constant_channel_returns_its_value(self):
        for v in (-1.0, 0.0, 0.7, 5.0):
            self.assertEqual(inverse_minmax(v, 3.5, 3.5), 3.5)

    def test_reversed_extrema_refused(self):
        with self.assertRaises(ValueError):
            inverse_minmax(0.0, 4.0, -2.0)

    def test_nonfinite_inputs_refused(self):
        for args in ((NAN, -2.0, 4.0), (INF, -2.0, 4.0), (0.0, NAN, 4.0), (0.0, -2.0, INF)):
            with self.assertRaises(ValueError, msg=str(args)):
                inverse_minmax(*args)

    def test_booleans_refused(self):
        with self.assertRaises(TypeError):
            inverse_minmax(True, -2.0, 4.0)


class RowTests(unittest.TestCase):
    MINS = [-1.0, -2.0, 0.0, -2.3, 0.0]
    MAXS = [3.0, 2.0, 1.0, 0.85, 6e-5]

    def test_row_matches_hand_values(self):
        # Channel 0: (0.5+1)/2*4-1 = 2.0. Channel 3: (0+1)/2*3.15-2.3 = -0.725.
        out = inverse_minmax_row([0.5, -1.0, 1.0, 0.0, -1.0], self.MINS, self.MAXS)
        self.assertEqual(out[0], 2.0)
        self.assertEqual(out[1], -2.0)
        self.assertEqual(out[2], 1.0)
        self.assertAlmostEqual(out[3], -0.725, places=12)
        self.assertEqual(out[4], 0.0)

    def test_permuting_channels_permutes_outputs(self):
        values = [0.5, -0.25, 0.1, 0.9, -0.6]
        base = inverse_minmax_row(values, self.MINS, self.MAXS)
        perm = [3, 0, 4, 1, 2]
        out = inverse_minmax_row([values[i] for i in perm],
                                 [self.MINS[i] for i in perm],
                                 [self.MAXS[i] for i in perm])
        self.assertEqual(out, [base[i] for i in perm])

    def test_length_mismatch_refused(self):
        with self.assertRaises(ValueError):
            inverse_minmax_row([0.0] * 4, self.MINS, self.MAXS)
        with self.assertRaises(ValueError):
            inverse_minmax_row([0.0] * 5, self.MINS[:4], self.MAXS)

    def test_empty_row_refused(self):
        with self.assertRaises(ValueError):
            inverse_minmax_row([], [], [])


class VelocityTests(unittest.TestCase):
    def test_velocity_reference(self):
        self.assertAlmostEqual(velocity_to_si(0.8, 16.0), 12.8)
        self.assertEqual(velocity_to_si(-0.5, 16.0), -8.0)

    def test_bad_reference_speed_refused(self):
        for u in (0.0, -16.0, NAN, INF):
            with self.assertRaises(ValueError, msg=str(u)):
                velocity_to_si(0.8, u)

    def test_nonfinite_value_refused(self):
        with self.assertRaises(ValueError):
            velocity_to_si(NAN, 16.0)


class PressureTests(unittest.TestCase):
    def test_pressure_kinematic_and_no_half_factor(self):
        # 0.2 * 1.184 * 16^2 = 60.6208 Pa; / 1.184 = 51.2 m^2/s^2.
        p = pressure_to_kinematic(0.2, 1.184, 16.0, 0.0, "rho_u_squared")
        self.assertAlmostEqual(p, 51.2)

    def test_pressure_reference(self):
        # (60.6208 - 11.84) / 1.184 = 41.2.
        p = pressure_to_kinematic(0.2, 1.184, 16.0, 11.84, "rho_u_squared")
        self.assertAlmostEqual(p, 41.2)

    def test_half_rho_u_squared_is_a_separate_explicit_convention(self):
        # 0.2 * 0.5 * 1.184 * 256 = 30.3104 Pa; / 1.184 = 25.6.
        p = pressure_to_kinematic(0.2, 1.184, 16.0, 0.0, "half_rho_u_squared")
        self.assertAlmostEqual(p, 25.6)

    def test_kinematic_result_does_not_depend_on_density_without_offset(self):
        a = pressure_to_kinematic(0.2, 1.184, 16.0, 0.0, "rho_u_squared")
        b = pressure_to_kinematic(0.2, 1.205, 16.0, 0.0, "rho_u_squared")
        self.assertAlmostEqual(a, b)

    def test_kinematic_source_over_rho_u_squared(self):
        # Kinematic source divided by rho U^2: p_k = 0.2 * 1.205 * 900 = 216.9 m^2/s^2.
        p = pressure_to_kinematic(0.2, 1.205, 30.0, 0.0, "kinematic_over_rho_u_squared")
        self.assertAlmostEqual(p, 216.9)
        # Reference given in Pa is converted with the same density: 216.9 - 12.05 / 1.205 = 206.9.
        p = pressure_to_kinematic(0.2, 1.205, 30.0, 12.05, "kinematic_over_rho_u_squared")
        self.assertAlmostEqual(p, 206.9)

    def test_conventions_differ_by_density_for_the_same_p_hat(self):
        # rho_u_squared gives 0.2 * 900 = 180; the kinematic reading gives 216.9 (ratio rho).
        a = pressure_to_kinematic(0.2, 1.205, 30.0, 0.0, "rho_u_squared")
        b = pressure_to_kinematic(0.2, 1.205, 30.0, 0.0, "kinematic_over_rho_u_squared")
        self.assertAlmostEqual(a, 180.0)
        self.assertAlmostEqual(b / a, 1.205)

    def test_unknown_convention_is_refused(self):
        for scale in ("guess", "", None, "RHO_U_SQUARED"):
            with self.assertRaises(ValueError, msg=repr(scale)):
                pressure_to_kinematic(0.2, 1.184, 16.0, 0.0, scale)

    def test_bad_density_or_speed_refused(self):
        for rho, u in ((0.0, 16.0), (-1.184, 16.0), (1.184, 0.0), (1.184, -16.0), (NAN, 16.0), (1.184, INF)):
            with self.assertRaises(ValueError, msg=f"{rho} {u}"):
                pressure_to_kinematic(0.2, rho, u, 0.0, "rho_u_squared")

    def test_nonfinite_value_or_reference_refused(self):
        with self.assertRaises(ValueError):
            pressure_to_kinematic(NAN, 1.184, 16.0, 0.0, "rho_u_squared")
        with self.assertRaises(ValueError):
            pressure_to_kinematic(0.2, 1.184, 16.0, INF, "rho_u_squared")


class BernoulliTests(unittest.TestCase):
    """Upstream inviscid flow: p_hat + c * |U_hat|^2 = const decides the pressure scale."""

    Q = [0.70, 0.85, 0.95, 1.00, 1.05, 1.20]

    def test_rho_u_squared_gives_half(self):
        # p/(rho U^2) + |u|^2/2 = const: p_hat = 0.3 - 0.5 q.
        p = [0.3 - 0.5 * q for q in self.Q]
        c, const, rms = fit_bernoulli(p, self.Q)
        self.assertAlmostEqual(c, 0.5, places=12)
        self.assertAlmostEqual(const, 0.3, places=12)
        self.assertLess(rms, 1e-12)
        self.assertEqual(consistent_conventions(c, 0.05, 1.205), ["rho_u_squared"])

    def test_half_rho_u_squared_gives_one(self):
        # Cp-style with gauge -0.15: p_hat = -0.15 + (1 - q) = 0.85 - q.
        p = [0.85 - q for q in self.Q]
        c, const, _ = fit_bernoulli(p, self.Q)
        self.assertAlmostEqual(c, 1.0, places=12)
        self.assertAlmostEqual(const, 0.85, places=12)
        self.assertEqual(consistent_conventions(c, 0.05, 1.205), ["half_rho_u_squared"])

    def test_noisy_fit_reports_residual(self):
        noise = [0.01, -0.01, 0.01, -0.01, 0.01, -0.01]
        p = [0.3 - 0.5 * q + e for q, e in zip(self.Q, noise)]
        c, _, rms = fit_bernoulli(p, self.Q)
        self.assertAlmostEqual(c, 0.5, delta=0.05)
        self.assertGreater(rms, 0.005)

    def test_kinematic_source_divided_by_rho_u_squared(self):
        # Kinematic p_k / (rho U^2) gives c = 0.5 / rho = 0.41494 at rho = 1.205.
        p = [0.3 - (0.5 / 1.205) * q for q in self.Q]
        c, _, _ = fit_bernoulli(p, self.Q)
        self.assertAlmostEqual(c, 0.41494, places=4)
        self.assertAlmostEqual(bernoulli_coefficient("kinematic_over_rho_u_squared", 1.205), 0.41494, places=4)
        self.assertEqual(consistent_conventions(c, 0.04, 1.205), ["kinematic_over_rho_u_squared"])

    def test_wide_tolerance_reports_every_consistent_convention(self):
        # 0.46 is within 0.05 of both 0.5 and 0.415: the check cannot decide, so it says so.
        self.assertEqual(consistent_conventions(0.46, 0.05, 1.205),
                         ["kinematic_over_rho_u_squared", "rho_u_squared"])

    def test_density_one_makes_kinematic_and_pa_readings_indistinguishable(self):
        self.assertEqual(consistent_conventions(0.5, 0.01, 1.0),
                         ["kinematic_over_rho_u_squared", "rho_u_squared"])

    def test_inconsistent_or_nonfinite_coefficient_gives_nothing(self):
        self.assertEqual(consistent_conventions(0.75, 0.05, 1.205), [])
        self.assertEqual(consistent_conventions(NAN, 0.05, 1.205), [])

    def test_bad_tolerance_or_convention_refused(self):
        for tol in (0.0, -0.1, NAN):
            with self.assertRaises(ValueError, msg=str(tol)):
                consistent_conventions(0.5, tol, 1.205)
        with self.assertRaises(ValueError):
            bernoulli_coefficient("guess", 1.205)

    def test_degenerate_fit_refused(self):
        with self.assertRaises(ValueError):
            fit_bernoulli([0.1, 0.2, 0.3], [1.0, 1.0, 1.0])  # no speed variation
        with self.assertRaises(ValueError):
            fit_bernoulli([0.1, 0.2], [0.9, 1.1])  # too few points
        with self.assertRaises(ValueError):
            fit_bernoulli([0.1, 0.2, 0.3], [0.9, 1.0])  # length mismatch
        with self.assertRaises(ValueError):
            fit_bernoulli([0.1, NAN, 0.3], [0.9, 1.0, 1.1])


class AuditGateTests(unittest.TestCase):
    def audit(self, status="verified", schema="vernon.domino_warmstart_scaling.v1"):
        return {"schema": schema,
                "channels": [{"name": "pressure", "status": status,
                              "convention": "rho_u_squared"}]}

    def test_verified_channel_returns_convention(self):
        self.assertEqual(verified_convention(self.audit(), "pressure"), "rho_u_squared")

    def test_unresolved_channel_refused(self):
        with self.assertRaises(ValueError):
            verified_convention(self.audit(status="unresolved"), "pressure")

    def test_missing_channel_refused(self):
        with self.assertRaises(ValueError):
            verified_convention(self.audit(), "Ux")

    def test_unknown_schema_refused(self):
        with self.assertRaises(ValueError):
            verified_convention(self.audit(schema="other.v9"), "pressure")

    def test_duplicate_channel_refused(self):
        a = self.audit()
        a["channels"].append(dict(a["channels"][0]))
        with self.assertRaises(ValueError):
            verified_convention(a, "pressure")


class RepositoryAuditTests(unittest.TestCase):
    """The checked-in audit must be well formed; 'verified' needs recorded evidence."""

    PATH = Path(__file__).resolve().parents[2] / "docs" / "research" / "domino-warmstart-scaling-v1.json"

    def setUp(self):
        self.audit = json.loads(self.PATH.read_text(encoding="utf-8"))

    def test_schema_and_channel_order(self):
        self.assertEqual(self.audit["schema"], "vernon.domino_warmstart_scaling.v1")
        self.assertEqual([c["name"] for c in self.audit["channels"]],
                         ["Ux", "Uy", "Uz", "pressure", "nut"])
        self.assertEqual([c["index"] for c in self.audit["channels"]], [0, 1, 2, 3, 4])

    def test_verified_rows_carry_evidence(self):
        for row in self.audit["channels"]:
            self.assertIn(row["status"], ("verified", "unresolved"), row["name"])
            if row["status"] == "verified":
                for key in ("convention", "source", "verification_artifact", "tolerance"):
                    self.assertTrue(row.get(key), f"{row['name']} verified without {key}")
                self.assertEqual(verified_convention(self.audit, row["name"]), row["convention"])
            else:
                with self.assertRaises(ValueError):
                    verified_convention(self.audit, row["name"])

    def test_nut_stays_unused_in_first_delivery(self):
        nut = [c for c in self.audit["channels"] if c["name"] == "nut"][0]
        self.assertEqual(nut["status"], "unresolved")

    def test_probe_runs_are_consistency_only_and_never_verify(self):
        # Plan 20.5: a surrogate's own predictions cannot establish its normalisation.
        probes = self.audit.get("probe_runs", [])
        artifacts = {r["artifact"] for r in probes}
        for run in probes:
            self.assertEqual(run["kind"], "consistency_only")
            self.assertTrue((self.PATH.parents[2] / run["artifact"]).is_file(), run["artifact"])
        for row in self.audit["channels"]:
            self.assertNotIn(row.get("verification_artifact"), artifacts, row["name"])

    def test_pressure_stays_unresolved_until_reference_parity(self):
        p = [c for c in self.audit["channels"] if c["name"] == "pressure"][0]
        if p["status"] == "verified":
            artifacts = {r["artifact"] for r in self.audit.get("reference_parity_runs", [])}
            self.assertIn(p["verification_artifact"], artifacts)
        else:
            self.assertEqual(p["status"], "unresolved")

    def test_parity_report_matches_recorded_reference_files(self):
        for run in self.audit.get("reference_parity_runs", []):
            self.assertEqual(run["kind"], "reference_parity")
            report = json.loads((self.PATH.parents[2] / run["artifact"]).read_text(encoding="utf-8"))
            files = run["reference_data"]["files"]
            self.assertEqual(report["inputs"]["stl_sha256"], files["run_1/drivaer_1.stl"]["sha256"])
            self.assertEqual(report["inputs"]["slice_sha256"], files["run_1/slices/yNormal_p00000.vtp"]["sha256"])

    def test_proposals_name_known_conventions(self):
        for row in self.audit["channels"]:
            prop = row.get("proposed_after_parity")
            if prop:
                self.assertIn(prop["convention"], KNOWN_CONVENTIONS, row["name"])

    def test_verified_channels_rest_on_accepted_reference_parity(self):
        artifacts = {r["artifact"] for r in self.audit.get("reference_parity_runs", [])}
        for row in self.audit["channels"]:
            if row["status"] == "verified":
                self.assertIn(row["verification_artifact"], artifacts, row["name"])
                self.assertIn("owner accepted", row.get("acceptance", ""), row["name"])
                self.assertIn("operating_point_gate", row.get("scope", ""), row["name"])
                self.assertNotIn("proposed_after_parity", row, row["name"])

    def test_operating_point_gate_stays_open(self):
        # Verifying the normalisation at 38.889 m/s says nothing about Reynolds independence at 16 m/s.
        gate = self.audit["normalization"]["operating_point_gate"]
        self.assertEqual(gate["status"], "unresolved")
        self.assertEqual(gate["blocks"], "CFD-field import")

    def test_known_weak_spots_are_recorded(self):
        rows = {c["name"]: c for c in self.audit["channels"]}
        self.assertTrue(any("sign only" in s for s in rows["Uy"].get("caveats", [])))
        self.assertTrue(any("1.52x" in s for s in rows["pressure"].get("caveats", [])))
        imp = self.audit["import_conversion"]
        self.assertEqual(imp["module"], "scripts/ai/domino_warmstart_units.py")
        self.assertIn("pressure_to_kinematic", imp["not_for_import"])

    def test_accepted_conventions(self):
        conv = {c["name"]: c for c in self.audit["channels"]}
        for name in ("Ux", "Uy", "Uz"):
            self.assertEqual(verified_convention(self.audit, name), "u_ref")
            self.assertEqual(conv[name]["reference"]["value"], 30.0)
        self.assertEqual(verified_convention(self.audit, "pressure"), "kinematic_over_rho_u_squared")
        self.assertEqual(conv["pressure"]["reference"]["value"], [30.0, 1.205])


class GeometryRecordTests(unittest.TestCase):
    """The registration record and the probe must describe the same six parts."""

    ROOT = Path(__file__).resolve().parents[2] / "docs" / "research"

    def load(self, name):
        return json.loads((self.ROOT / name).read_text(encoding="utf-8"))

    def test_record_parts_match_probe_hashes(self):
        geo = self.load("domino-warmstart-geometry-v1.json")
        probe = self.load("domino-warmstart-probe-20261109.json")
        self.assertEqual(geo["schema"], "vernon.domino_warmstart_geometry.v1")
        self.assertEqual({p["file"]: p["sha256"] for p in geo["parts"]}, probe["geometry"]["part_sha256"])

    def test_record_states_domain_checks_and_limits(self):
        geo = self.load("domino-warmstart-geometry-v1.json")
        for box in ("volume_box", "surface_box"):
            self.assertIn("assembly_inside", geo["checkpoint_domain"][box])
        self.assertTrue(geo["limitations"])
        self.assertLess(abs(geo["ground"]["gap_m"]), 0.01)


class CarPrepRecordTests(unittest.TestCase):
    """Cleaned Sketchfab shells: closed, placed like the Notchback, and honest about their sources."""

    PATH = Path(__file__).resolve().parents[2] / "docs" / "research" / "domino-warmstart-carprep-v1.json"

    def setUp(self):
        self.rec = json.loads(self.PATH.read_text(encoding="utf-8"))

    def test_three_body_types(self):
        self.assertEqual(self.rec["schema"], "vernon.domino_warmstart_carprep.v1")
        self.assertEqual({c["body"] for c in self.rec["cars"]}, {"notchback coupe", "fastback", "estate"})
        self.assertTrue(self.rec["limitations"])

    def test_shells_closed_and_placed(self):
        for car in self.rec["cars"]:
            sh, pl = car["shell"], car["placement"]
            self.assertTrue(sh["watertight"] and sh["winding_consistent"], car["car"])
            self.assertGreater(sh["volume_m3"], 5.0, car["car"])  # a hollow skin came out near 2 m^3
            self.assertAlmostEqual(pl["front_axle_x_m"], 0.0)
            self.assertAlmostEqual(pl["rear_axle_x_m"], car["published_recalled_m"]["wheelbase"], places=6)
            self.assertAlmostEqual(pl["ground_gap_to_box_floor_m"], 0.0015, places=6)
            self.assertTrue(car["checkpoint_domain"]["surface_box"]["inside"], car["car"])
            self.assertLess(abs(car["scale"]["length_vs_published"]), 0.03, car["car"])

    def test_synthetic_parts_are_labelled(self):
        for car in self.rec["cars"]:
            if car["axle_detection"]["method"].startswith("wheel-arch"):
                self.assertTrue(car["synthetic_wheels"]["synthetic"], car["car"])
            self.assertIn("published_recalled_m", car)


if __name__ == "__main__":
    unittest.main()

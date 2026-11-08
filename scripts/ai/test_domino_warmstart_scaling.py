"""No-GPU analytical tests for domino_warmstart_scaling.

Expected values are worked out by hand in the comments, never by calling the
conversion backwards. Run from the checkout root:

    python -m unittest discover -s scripts/ai -p test_domino_warmstart_scaling.py -v
"""
import json
import unittest
from pathlib import Path

from domino_warmstart_scaling import (
    classify_pressure_scale,
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
        self.assertEqual(classify_pressure_scale(c, 0.05), "rho_u_squared")

    def test_half_rho_u_squared_gives_one(self):
        # Cp-style with gauge -0.15: p_hat = -0.15 + (1 - q) = 0.85 - q.
        p = [0.85 - q for q in self.Q]
        c, const, _ = fit_bernoulli(p, self.Q)
        self.assertAlmostEqual(c, 1.0, places=12)
        self.assertAlmostEqual(const, 0.85, places=12)
        self.assertEqual(classify_pressure_scale(c, 0.05), "half_rho_u_squared")

    def test_noisy_fit_reports_residual(self):
        noise = [0.01, -0.01, 0.01, -0.01, 0.01, -0.01]
        p = [0.3 - 0.5 * q + e for q, e in zip(self.Q, noise)]
        c, _, rms = fit_bernoulli(p, self.Q)
        self.assertAlmostEqual(c, 0.5, delta=0.05)
        self.assertGreater(rms, 0.005)

    def test_ambiguous_coefficient_is_unresolved(self):
        self.assertIsNone(classify_pressure_scale(0.75, 0.05))
        self.assertIsNone(classify_pressure_scale(NAN, 0.05))

    def test_tolerance_must_separate_the_conventions(self):
        for tol in (0.0, -0.1, 0.25, 0.3, NAN):
            with self.assertRaises(ValueError, msg=str(tol)):
                classify_pressure_scale(0.5, tol)

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


if __name__ == "__main__":
    unittest.main()

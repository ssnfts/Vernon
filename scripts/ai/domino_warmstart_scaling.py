"""Pure conversions from model outputs to OpenFOAM initial fields.

No Torch, NumPy, file or model side effects: every function takes plain numbers
and returns plain numbers, so the analytical tests run anywhere. Whether a
convention may be used is decided by the audit JSON
(docs/research/domino-warmstart-scaling-v1.json) through `verified_convention`,
never by these functions.

OpenFOAM's incompressible `p` is gauge kinematic pressure, [0 2 -2 0 0 0 0].
"""
import math

AUDIT_SCHEMA = "vernon.domino_warmstart_scaling.v1"

# Pressure normalisations a channel audit may name: (divisor factor on rho * U_ref^2,
# unit of the source pressure before division). No default: an unknown or missing
# name is refused.
#   rho_u_squared                 p_hat = p_Pa / (rho U^2)
#   half_rho_u_squared            p_hat = p_Pa / (0.5 rho U^2)
#   kinematic_over_rho_u_squared  p_hat = p_kinematic / (rho U^2); arises when a kinematic
#                                 source field (OpenFOAM incompressible p, DrivAerML pMeanTrim)
#                                 is divided by a density it never carried.
PRESSURE_CONVENTIONS = {
    "rho_u_squared": (1.0, "Pa"),
    "half_rho_u_squared": (0.5, "Pa"),
    "kinematic_over_rho_u_squared": (1.0, "kinematic"),
}
PRESSURE_SCALES = {k: v[0] for k, v in PRESSURE_CONVENTIONS.items()}
KNOWN_CONVENTIONS = frozenset({"u_ref"}) | frozenset(PRESSURE_CONVENTIONS)


def _real(x, name):
    if isinstance(x, bool) or not isinstance(x, (int, float)):
        raise TypeError(f"{name} must be a real number, got {type(x).__name__}")
    x = float(x)
    if not math.isfinite(x):
        raise ValueError(f"{name} is not finite: {x}")
    return x


def _positive(x, name):
    x = _real(x, name)
    if x <= 0.0:
        raise ValueError(f"{name} must be > 0, got {x}")
    return x


def inverse_minmax(value, minimum, maximum):
    """Undo min-max scaling to [-1, 1]. Out-of-range values are extrapolated, not clamped."""
    v = _real(value, "value")
    lo = _real(minimum, "minimum")
    hi = _real(maximum, "maximum")
    if hi < lo:
        raise ValueError(f"reversed extrema: minimum {lo} > maximum {hi}")
    return (v + 1.0) * 0.5 * (hi - lo) + lo


def inverse_minmax_row(values, minima, maxima):
    """`inverse_minmax` per channel; all three sequences must have the same non-zero length."""
    if not (len(values) == len(minima) == len(maxima)):
        raise ValueError(f"channel count mismatch: {len(values)} values, "
                         f"{len(minima)} minima, {len(maxima)} maxima")
    if not values:
        raise ValueError("empty channel row")
    return [inverse_minmax(v, lo, hi) for v, lo, hi in zip(values, minima, maxima)]


def velocity_to_si(value_hat, u_ref_mps):
    """U = U_ref * U_hat, for a channel audited as U / U_ref."""
    return _real(value_hat, "value_hat") * _positive(u_ref_mps, "u_ref_mps")


def pressure_to_kinematic(value_hat, rho_kg_m3, u_ref_mps, p_reference_pa, pressure_scale):
    """Gauge kinematic pressure (m^2/s^2) from a nondimensional pressure.

    Pa source:        p_Pa = factor * rho * U_ref^2 * p_hat; result = (p_Pa - p_reference_pa) / rho.
    Kinematic source: p_k  = factor * rho * U_ref^2 * p_hat; result = p_k - p_reference_pa / rho.
    `rho_kg_m3` is the density the normalisation divided by, not necessarily the air's.
    """
    if not isinstance(pressure_scale, str) or pressure_scale not in PRESSURE_CONVENTIONS:
        raise ValueError(f"unknown pressure convention {pressure_scale!r}; "
                         f"expected one of {sorted(PRESSURE_CONVENTIONS)}")
    p_hat = _real(value_hat, "value_hat")
    rho = _positive(rho_kg_m3, "rho_kg_m3")
    u = _positive(u_ref_mps, "u_ref_mps")
    p_ref = _real(p_reference_pa, "p_reference_pa")
    factor, source = PRESSURE_CONVENTIONS[pressure_scale]
    scaled = factor * rho * u * u * p_hat
    if source == "Pa":
        return (scaled - p_ref) / rho
    return scaled - p_ref / rho


def fit_bernoulli(p_hat, speed_sq_hat):
    """Least-squares fit of p_hat = const - c * |U_hat|^2 over upstream inviscid probes.

    Returns (c, const, rms_residual). See `bernoulli_coefficient` for the value
    each convention implies; several conventions can give the same c, so the fit
    is a consistency check, never a verification. const - c * |U_inf_hat|^2 is
    the freestream gauge.
    """
    if len(p_hat) != len(speed_sq_hat):
        raise ValueError(f"length mismatch: {len(p_hat)} pressures, {len(speed_sq_hat)} speeds")
    n = len(p_hat)
    if n < 3:
        raise ValueError(f"need at least 3 probes, got {n}")
    p = [_real(x, "p_hat") for x in p_hat]
    q = [_real(x, "speed_sq_hat") for x in speed_sq_hat]
    qm = sum(q) / n
    pm = sum(p) / n
    sqq = sum((x - qm) ** 2 for x in q)
    if sqq <= 1e-12 * max(1.0, sum(x * x for x in q)):
        raise ValueError("probes have no speed variation; the fit is undetermined")
    slope = sum((x - qm) * (y - pm) for x, y in zip(q, p)) / sqq
    const = pm - slope * qm
    rms = math.sqrt(sum((y - (const + slope * x)) ** 2 for x, y in zip(q, p)) / n)
    return -slope, const, rms


def bernoulli_coefficient(pressure_scale, rho_kg_m3):
    """The c in p_hat = const - c * |U_hat|^2 implied by a convention, velocity scaled by U_ref.

    Pa source: c = 1 / (2 factor). Kinematic source: c = 1 / (2 factor rho). Independent
    of U_ref when both velocity and pressure use the same U_ref.
    """
    if not isinstance(pressure_scale, str) or pressure_scale not in PRESSURE_CONVENTIONS:
        raise ValueError(f"unknown pressure convention {pressure_scale!r}")
    rho = _positive(rho_kg_m3, "rho_kg_m3")
    factor, source = PRESSURE_CONVENTIONS[pressure_scale]
    return 0.5 / factor if source == "Pa" else 0.5 / (factor * rho)


def consistent_conventions(coefficient, tolerance, rho_kg_m3):
    """Every convention whose implied coefficient lies within `tolerance`, sorted by name.

    Consistency only: an empty or multi-name result is normal, and a single name is not a
    verification; the audit JSON decides through `verified_convention`.
    """
    tol = _positive(tolerance, "tolerance")
    if isinstance(coefficient, bool) or not isinstance(coefficient, (int, float)) \
            or not math.isfinite(coefficient):
        return []
    return sorted(name for name in PRESSURE_CONVENTIONS
                  if abs(coefficient - bernoulli_coefficient(name, rho_kg_m3)) <= tol)


def verified_convention(audit, channel):
    """The audited convention for `channel`, or ValueError unless the audit marks it verified."""
    if audit.get("schema") != AUDIT_SCHEMA:
        raise ValueError(f"unknown audit schema {audit.get('schema')!r}")
    rows = [r for r in audit.get("channels", []) if r.get("name") == channel]
    if not rows:
        raise ValueError(f"channel {channel!r} is not in the audit")
    if len(rows) > 1:
        raise ValueError(f"channel {channel!r} appears {len(rows)} times in the audit")
    row = rows[0]
    if row.get("status") != "verified":
        raise ValueError(f"channel {channel!r} is {row.get('status')!r}, not verified")
    conv = row.get("convention")
    if conv not in KNOWN_CONVENTIONS:
        raise ValueError(f"channel {channel!r} names unknown convention {conv!r}")
    return conv

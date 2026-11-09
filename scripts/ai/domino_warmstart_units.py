"""Audit-bound unit conversion for DoMINO volume outputs (pure Python, no side effects).

The three constants are different quantities and must not be swapped:

    u_norm   = 30 m/s       velocity normalisation divisor (checkpoint config)
    rho_norm = 1.205 kg/m3  pressure divisor only; the source pressure is kinematic
    u_train  = 38.889 m/s   physical freestream of the DrivAerML training labels

Equations (see docs/research/domino-warmstart-scaling-equations.md):

    U_ref_sample   = u_norm * U_hat                         m/s, at 38.889 m/s conditions
    p_kin_sample   = rho_norm * u_norm^2 * p_hat            m^2/s^2, no density division
    p_OpenFOAM     = p_kin + c                              c: additive gauge, m^2/s^2
    s              = U_case / u_train                       only with the operating-point gate
    U_case_field   = s * u_norm * U_hat
    p_case         = s^2 * rho_norm * u_norm^2 * p_hat

`p_hat` and `U_hat` are statistical-inverse values (domino_warmstart_scaling.inverse_minmax).
Nothing here reads the case air density except `kinematic_to_pascal`, for reporting.
"""
import math
from dataclasses import dataclass

SEMANTICS_SCHEMA = "vernon.domino_warmstart_scaling_semantics.v1"
AUDIT_SCHEMA = "vernon.domino_warmstart_scaling.v1"
CHANNELS = ("Ux", "Uy", "Uz", "pressure", "nut")
GATE_STATES = ("verified", "unresolved")
_VAR_CHANNELS = {("UMeanTrim", "vector"): ["Ux", "Uy", "Uz"],
                 ("pMeanTrim", "scalar"): ["pressure"],
                 ("nutMeanTrim", "scalar"): ["nut"]}


def _real(x, name):
    if isinstance(x, bool) or not isinstance(x, (int, float)):
        raise ValueError(f"{name} must be a real number, got {type(x).__name__}")
    x = float(x)
    if not math.isfinite(x):
        raise ValueError(f"{name} is not finite: {x}")
    return x


def _positive(x, name):
    x = _real(x, name)
    if x <= 0.0:
        raise ValueError(f"{name} must be > 0, got {x}")
    return x


@dataclass(frozen=True)
class Reference:
    u_norm: float
    rho_norm: float
    u_train: float
    p_train_ref: float

    def __post_init__(self):
        _positive(self.u_norm, "u_norm")
        _positive(self.rho_norm, "rho_norm")
        _positive(self.u_train, "u_train")
        _real(self.p_train_ref, "p_train_ref")


def reference_from_records(audit, semantics):
    """Build the Reference from the two checked-in records, refusing any disagreement."""
    if audit.get("schema") != AUDIT_SCHEMA or semantics.get("schema") != SEMANTICS_SCHEMA:
        raise ValueError("unknown audit or semantics schema")
    rows = {r.get("name"): r for r in audit.get("channels", [])}
    if tuple(r.get("name") for r in audit.get("channels", [])) != CHANNELS:
        raise ValueError("audit channel order is not Ux, Uy, Uz, pressure, nut")
    for name, conv in (("Ux", "u_ref"), ("Uy", "u_ref"), ("Uz", "u_ref"),
                       ("pressure", "kinematic_over_rho_u_squared")):
        row = rows[name]
        if row.get("status") != "verified" or row.get("convention") != conv:
            raise ValueError(f"channel {name} is not verified as {conv}")
    c = semantics.get("constants", {})
    ref = Reference(u_norm=c.get("u_norm_mps"), rho_norm=c.get("rho_norm_kg_m3"),
                    u_train=c.get("training_freestream_mps"), p_train_ref=c.get("training_pressure_reference_m2_s2"))
    for name in ("Ux", "Uy", "Uz"):
        if rows[name]["reference"]["value"] != ref.u_norm:
            raise ValueError(f"{name} reference {rows[name]['reference']['value']} != u_norm {ref.u_norm}")
    if rows["pressure"]["reference"]["value"] != [ref.u_norm, ref.rho_norm]:
        raise ValueError("pressure reference does not match (u_norm, rho_norm)")
    if list(c.get("channel_order", [])) != list(CHANNELS):
        raise ValueError("semantics channel order does not match the audit")
    return ref


def reference_velocity(u_hat, ref):
    """Velocity in m/s at the reference sample's conditions (training freestream u_train)."""
    if len(u_hat) != 3:
        raise ValueError(f"velocity needs 3 components, got {len(u_hat)}")
    return tuple(ref.u_norm * _real(v, f"U_hat[{i}]") for i, v in enumerate(u_hat))


def reference_kinematic_pressure(p_hat, ref):
    """Kinematic pressure (m^2/s^2) of the reference sample, relative to its p_ref; no density division."""
    return ref.rho_norm * ref.u_norm * ref.u_norm * _real(p_hat, "p_hat") + ref.p_train_ref


def kinematic_to_pascal(p_kinematic, rho_air):
    """Reporting only: one multiplication by the air density of the case being reported."""
    return _real(p_kinematic, "p_kinematic") * _positive(rho_air, "rho_air")


def operating_point_ratio(u_case, ref, gate, assume_reynolds_independence=False):
    """s = U_case / u_train. Refused while the audit's operating-point gate is open,
    unless the caller states the Reynolds-independence hypothesis explicitly (diagnostics only)."""
    status = gate.get("status") if isinstance(gate, dict) else None
    if status not in GATE_STATES:
        raise ValueError(f"unknown operating-point gate status {status!r}")
    if status != "verified" and not assume_reynolds_independence:
        raise ValueError("operating-point gate is unresolved: Reynolds independence is not established")
    return _positive(u_case, "u_case") / ref.u_train


def case_velocity(u_hat, ref, u_case, gate, assume_reynolds_independence=False):
    s = operating_point_ratio(u_case, ref, gate, assume_reynolds_independence)
    return tuple(s * v for v in reference_velocity(u_hat, ref))


def case_kinematic_pressure(p_hat, ref, u_case, gate, assume_reynolds_independence=False):
    """s^2 * p_kin_sample. The training gauge is removed first; align the case gauge with
    `least_squares_gauge` against the control's far field afterwards."""
    s = operating_point_ratio(u_case, ref, gate, assume_reynolds_independence)
    return s * s * (reference_kinematic_pressure(p_hat, ref) - ref.p_train_ref)


def least_squares_gauge(predicted, control):
    """Additive constant c minimising sum (predicted + c - control)^2, i.e. mean(control - predicted)."""
    if len(predicted) != len(control):
        raise ValueError(f"length mismatch: {len(predicted)} vs {len(control)}")
    if not predicted:
        raise ValueError("no samples")
    diffs = [_real(b, "control") - _real(a, "predicted") for a, b in zip(predicted, control)]
    return sum(diffs) / len(diffs)


def identify_velocity_order(u_hats, labels, ref):
    """Best (permutation, signs) mapping model channels to label axes, and the error ratio of
    the runner-up to the best. Consistency check on labelled data; not a verification."""
    if len(u_hats) != len(labels) or not labels:
        raise ValueError("need equal, non-zero numbers of predictions and labels")
    import itertools
    scores = []
    for perm in itertools.permutations(range(3)):
        for signs in itertools.product((1, -1), repeat=3):
            err = 0.0
            for h, lab in zip(u_hats, labels):
                for k in range(3):
                    # Channel k of the label is reproduced by sign * model channel j with perm[j] = k.
                    j = perm.index(k)
                    err += (signs[j] * ref.u_norm * _real(h[j], "U_hat") - _real(lab[k], "label")) ** 2
            scores.append((err, (perm, signs)))
    scores.sort(key=lambda t: t[0])
    best, runner = scores[0], scores[1]
    margin = math.inf if best[0] == 0.0 else runner[0] / best[0]
    return best[1], margin


def volume_channel_order(config_text):
    """Expand variables.volume.solution of a checkpoint config.yaml into channel names, in order."""
    lines = config_text.splitlines()
    try:
        start = next(i for i, l in enumerate(lines) if l.strip() == "volume:" and
                     i + 1 < len(lines) and lines[i + 1].strip() == "solution:")
    except StopIteration:
        raise ValueError("config has no variables.volume.solution block") from None
    indent = len(lines[start + 1]) - len(lines[start + 1].lstrip())
    names = []
    for l in lines[start + 2:]:
        s = l.strip()
        if not s or s.startswith("#"):
            continue
        if len(l) - len(l.lstrip()) <= indent:
            break
        var, _, kind = (t.strip() for t in s.partition(":"))
        key = (var, kind.split("#")[0].strip())
        if key not in _VAR_CHANNELS:
            raise ValueError(f"unknown volume variable {var}: {kind}")
        names += _VAR_CHANNELS[key]
    if not names:
        raise ValueError("empty volume solution block")
    return names

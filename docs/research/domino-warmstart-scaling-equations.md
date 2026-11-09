# DoMINO volume outputs: units, equations and limits

Semantic check of `domino-warmstart-scaling-v1.json` (2026-11-09). Machine-readable
record: `domino-warmstart-scaling-semantics-v1.json`. Code: `scripts/ai/domino_warmstart_units.py`,
tests `scripts/ai/test_domino_warmstart_semantics.py`. Nothing here changes a channel status.

## Three constants, three meanings

| Symbol | Value | What it is | What it is not |
|---|---|---|---|
| `u_norm` | 30 m/s | velocity divisor in the checkpoint's `global_parameters` | the speed of the training flow |
| `rho_norm` | 1.205 kg/m³ | pressure divisor | the density of the data: DrivAerML `pMeanTrim` is kinematic |
| `u_train` | 38.889 m/s | DrivAerML freestream, the physical operating point of the labels | a normalisation constant |

Channel order, from `config.yaml` `variables.volume.solution`: `UMeanTrim` (vector),
`pMeanTrim`, `nutMeanTrim` → `[Ux, Uy, Uz, pressure, nut]`.

## Equations

1. Network output → statistical inverse (per channel `c`, `scaling_factors.pkl` `volume_fields`, unclamped):

       f̂_c = (y_c + 1)/2 · (max_c − min_c) + min_c

2. Statistical inverse → velocity at the reference sample's conditions (38.889 m/s freestream):

       U_sample = u_norm · (f̂_0, f̂_1, f̂_2) = 30 · Û    [m/s]

   The training freestream is Û ≈ 38.889/30 = 1.296, not 1.

3. Statistical inverse → the reference sample's pressure quantity (kinematic, relative to DrivAerML p_ref = 0):

       p_kin,sample = rho_norm · u_norm² · f̂_3 = 1084.5 · p̂    [m²/s²]

   The source was never multiplied by a density, so **no density division follows**.

4. Pressure quantity → OpenFOAM gauge kinematic `p` ([0 2 −2 0 0 0 0]):

       p_OpenFOAM = p_kin + c

   `c` is one additive constant in m²/s², fitted by least squares to the control case's far
   field. It changes no gradient and cannot absorb a scale error. Pascals appear only for
   reporting: `p_Pa = rho_air · p_kin` (one multiplication, case density 1.184).

5. Proposed operating-point scaling to 16 m/s (hypothesis: Reynolds independence; **gated,
   unresolved**, `normalization.operating_point_gate`):

       s = U_case / u_train = 16 / 38.889 = 0.41143
       U_case = s · 30 · Û = 12.343 · Û          [m/s]
       p_case = s² · 1084.5 · p̂ = 183.58 · p̂    [m²/s²]  (+ gauge c)

   The model is not conditioned on speed (`encode_parameters: false`). Changing these
   constants rescales the output; it does not give the model 16 m/s physics.

## The earlier conversion was wrong by two factors

Plan section 6.2 proposed `p_Pa = rho·U_ref²·p̂; p_OpenFOAM = p_Pa / rho` with `U_ref` = the
case speed.

- **Extra density division.** Reading p̂ as pascals over ρU² and dividing by ρ gives
  `u_norm²·p̂ = 900·p̂`, too small by exactly 1.205.
- **Case speed in place of the divisor.** `16·Û` instead of `12.343·Û`: 29.6 % fast, the
  freestream would come out at 20.7 m/s.
- **Combined at 16 m/s:** `256·p̂` instead of `183.58·p̂`, a factor 1.3945 = (38.889/30)²/1.205.

No import path exists yet, so no field was ever written with these factors.
`domino_warmstart_scaling.pressure_to_kinematic` gives the correct 1084.5·p̂ only when called
with ρ = 1.205, U = 30 and `kinematic_over_rho_u_squared`; its signature also accepts the case
density and speed silently, and it converts a Pa gauge with the normalisation density (harmless
at p_ref = 0). `domino_warmstart_units.py` binds the constants to the audit records, takes
gauges in m²/s² only and refuses the operating-point step while the gate is open.

## What the run_1 evidence establishes

The run_1 "reference parity" is a **label-regression identification**: surrogate predictions
fitted against DrivAerML CFD labels on the y = 0 plane (147,660 cells). It identifies the
normalisation structure. Its relative L2 errors (0.10 velocity, 0.13 pressure) are **prediction
error against labels**. **Software parity** (same input tensors, a reference implementation,
rtol 1e‑5) is **not established**: no reference outputs exist locally, and the bridge and the
parity script share the same installed datapipe call.

Post-hoc checks (`domino_warmstart_semantics_check.py`, not preregistered):

| Check | Result | Reading |
|---|---|---|
| Ux fitted alone | 30.056 m/s (−0.19 %) | supports U/30 |
| Uz fitted alone | 29.807 m/s (+0.65 %), sign confirmed | supports U/30 independently of Ux |
| Uy fitted alone | 39.8 m/s, correlation 0.92 | sign only. On the symmetry plane Uy is the label's small asymmetry, so its magnitude is not tested here; it rests on the shared divisor of a vector channel |
| Channel order | identity 0.103; best swap (y↔z) 0.327 | order confirmed |
| Pressure divisor, gauge-free | 1060.5 m²/s² | 1084.5 is +2.3 %; 900 (extra division) −15 %; 450 (factor two) −58 %; 1822 (38.889 m/s) +72 % |
| Density digits | 1.184·30² is +0.5 % | the fit cannot tell 1.205 from 1.184; 1.205 comes from config.yaml |

The preregistered margin used an upstream gauge (runner-up 1.90× worse). With a global median
gauge the margin over the extra-division candidate is 1.52×, just above 1.5×.

## Still open

- Operating-point gate (16 m/s, Reynolds dependence), geometry shift (Fastback) and the CFD
  frame transform. A speed/Fastback comparison against a converged control tests them jointly.
- Software parity against a reference implementation.
- Uy magnitude on an off-plane sample.
- nut: unresolved and unused; k/omega are never derived from it.

# Spec: performance model and on-road validation (stage 8, "performance")

Clean-room spec. Sources are public pages, papers and regulations listed in section 6; wording is ours.
Reference tools are named here for documentation only; they never appear in UI strings (see `CLEAN_ROOM.md`).
No vendor binary, install folder or help file was inspected. Where a fact reached us only through a search snippet or a
third-party summary, section 6 says so, and the item is marked "verify" in the golden tests.

## 1. Scope and reference tools

Goal: take the aero numbers from the airflow stage (CdA, ClA, Cp maps), the vehicle facts (mass, power, tyres) and a track,
and emit numbers a human can sanity-check against the road: lap time and speed trace, the lap-time value of a CdA/ClA change,
and a noise-floor verdict for the cheap physical tests (coastdown, surface-pressure puck, tufts).

| Tool / method | Version basis | Used here for |
|---|---|---|
| OptimumLap (free, OptimumG) | no version number on the reachable pages; behaviour from the vendor product and track-converter pages (read 2026-10-21) plus third-party papers | point-mass quasi-steady lap simulation, track files, output channels |
| Vernon reference script (pure Python, ours) | written for this spec | the numbers in section 5 that are marked "ref script" |
| Surface-pressure puck on a differential gauge | forum posts and an SAE paper by the method's author | on-road Cp spot checks |
| Tuft flow visualisation | two Chalmers/SAE papers (abstract level) | qualitative attached/separated map |
| Coastdown regression | US federal regulation text (heavy-vehicle CdA method) and a trade article | CdA and Crr from a speed log |

Out of scope: transient or single-track (bicycle) models, load transfer, optimal racing line, tyre curves beyond a
load-sensitivity slope, ride-height/pitch/yaw aero maps (the reference tool omits them too), banking and elevation
(channels reserved, not used), multi-event scoring, tyre or brake thermal models.

Conventions: SI internally (m, s, kg, N, Pa, W); display km/h and kgf where useful. `cda_m2` and `cla_m2` are magnitudes; Vernon
`cla_m2` is **positive for downforce** (the toolchain note writes ClA = -0.5 for 0.5 m2 of downforce; importers flip the sign on a
`lift_positive` flag). Density is always passed explicitly, never assumed.

## 2. Feature catalogue (observable behaviour)

### 2.1 Lap simulator (OptimumLap)
- Model class: point mass, quasi-steady state. The vendor says it ignores load transfer and transient effects, and claims lap
  times within about 10 percent of real data, with apex speeds, end-of-straight speeds and energy use tracking logged data.
- Third-party summaries add: the car may accelerate and corner at the same time, and brake and corner at the same time (a combined
  friction limit); no yaw, no detailed tyre model. A Chalmers thesis states the tool takes total weight, powertrain
  parameters, aero coefficients and static tyre friction values, runs a predefined path, and cannot respond to ride height, pitch,
  yaw or roll changes. A 2012 forum post (snippet only) says a car is described by 10 parameters.
- Vendor-named input areas: mass, engine power/curve, gearbox characteristics, aerodynamics, tyres, suspension. Vehicles are stored in
  `.OLVeh` files with a downloadable template database. The exact parameter list, units and defaults were **not** reachable; Vernon
  chooses its own (below) and the golden tests must confirm them against a licensed copy.
- Tracks are stored in `.OLTra`. A paper that compared its own script with the tool describes a user-written text track with three
  columns per segment, the first being the type (straight, left, right coded 0, 1, 2); the other two are presumably length and
  radius (inferred, verify). Tracks can be modified, run in reverse and mirrored.
- Outputs the vendor names: lap time, apex speeds, end-of-straight speeds, energy consumption; interactive graphs; 60 output
  channels; export to a spreadsheet. A Chalmers in-house point-mass tool with the same method predicted 78.3 s against a real best of
  83.1 s on one autocross lap (the simulated car was 5.8 percent faster); the authors treat the tool as a delta engine.
- Limits to preserve: no pitch/yaw/ride-height aero, perfect driver, flat road. Do not claim absolute accuracy better than a few percent.

Vernon vehicle parameters (`vehicle.v1`; V = Vernon-chosen default, not copied from a tool):

| Param | Default | Range | Unit | Note |
|---|---|---|---|---|
| `mass_kg` | 1400 | 50-6000 | kg | includes driver and fuel |
| `rot_inertia_frac` | 0.0 | 0-0.15 | - | effective mass = m(1+e) in accel and brake |
| `power_w` | 150000 | 1e3-2e6 | W | at the wheels; used when no `engine_curve` |
| `power_density_exponent` | 0.0 | 0-1.5 | - | P = P0 (rho/rho_ref)^k; k=1 for naturally aspirated |
| `rho_ref` | 1.225 | 0.5-1.5 | kg/m3 | density at which `power_w` was measured |
| `engine_curve` | none | table | rpm, N*m | optional; with `gear_ratios`, `final_drive`, `wheel_radius_m` (0.32; 0.1-0.6), `driveline_eff` (0.92) |
| `f_drive_max_n` | 0 (off) | 0-1e5 | N | optional hard cap on drive force |
| `cda_m2` | 0.64 | 0.05-3 | m2 | |
| `cla_m2` | 0.0 | -1 to 6 | m2 | positive = downforce |
| `mu_x`, `mu_y` | 1.0, 1.0 | 0.2-3 | - | longitudinal and lateral peak friction |
| `mu_drop_per_kn` | 0.0 | 0-0.2 | 1/kN | linear friction drop with normal load (Chalmers form), floor 0.2 |
| `crr` | 0.012 | 0.002-0.05 | - | rolling resistance coefficient |
| `rr_includes_downforce` | true | bool | - | F_rr = crr*N if true, crr*m*g if false |
| `brake_max_g` | 0 (tyre-limited) | 0-5 | g | optional brake cap |
| `v_cap_ms` | 120 | 10-150 | m/s | top-speed cap for straights |
| `aero_balance_front` | 0.5 | 0-1 | - | stored for reports only in the point-mass model |

### 2.2 Track building (OptimumLap track converter)
- Inputs: CSV with a fixed accelerometer/GPS column layout (dot decimal), GPX with timestamped GPS, KML/KMZ geometry without time
  (often hand-drawn; a smoothing factor tidies the curves).
- Converter parameters: name, city, country, track type (permanent/temporary circuit, rally stage, oval, autocross, drag strip),
  closed or open, data frequency in Hz, a "g-force compensator" (values above 1.0 suggested for hairpins), approximate track
  length, analysis mode (isolate the fastest lap vs process everything), smoothing factor (default 3). The `.OLTra` internals are
  not published on the page.
- Vernon equivalents: `track.v1` stores `closed`, arc length `s[]`, signed curvature `kappa[]` (left positive), `ds[]`, and a
  segment list (type, length, radius); import from a polyline/curve, from GPS, or from a hand-entered segment list.

### 2.3 The aero/performance relations in the toolchain note (checked here)
Quantities: q = 1/2 rho v2; D = q CdA; F_down = q ClA; P_aero = 1/2 rho CdA v3; corner v = sqrt(mu g R / (1 - mu rho ClA R / 2m));
braking a = mu (g + 1/2 rho ClA v2 / m) + 1/2 rho CdA v2 / m; Cp = (p - p_inf)/q; duct heat Q = mdot cp dT. Every worked number in the note
(rho 1.127 and 1.098, q at 200 km/h, corner gains +0.5/+2.0/+4.2 percent, top speed 262.7 -> 256.3 km/h, ~1340 N at 250 km/h)
reproduces to the printed rounding; see golden tests 1-4. Two caveats found while reproducing them: the +2.0 percent figure is
+2.04 percent unrounded, and the top-speed pair assumes rolling resistance on weight only (`rr_includes_downforce = false`).

### 2.4 Coastdown (regression of a speed log)
- Observable: drive to a high speed, take the drivetrain out of the loop, log speed (and position) while the car slows; fit a
  road-load model; read CdA and Crr. Aerodynamic drag scales with speed squared, rolling resistance is nearly constant.
- Federal practice for heavy-vehicle CdA (use as a quality template, not a car standard): 10 Hz data, runs in alternating
  directions and paired, wind component along the road at or below 6 mph (about 2.7 m/s), road surface at or below 50 C,
  elevation logged if grade exceeds 0.02 percent, high-speed and low-speed windows fitted, outlier filter, yaw filter within 1.0 degree of the
  median, CdA values beyond 2 SD dropped, at least 24 points retained.
- Known noise sources: wind, driveline drag, tyre temperature, mass and rotational inertia, grade, density. Useful for large
  changes (wing on/off), not for trims.

### 2.5 Surface-pressure puck
- Observable: a small flat puck taped to a panel, a tube to a low-range differential gauge, the other side to a static reference
  (a pitot-type static port or a sealed vessel). Read the steady gauge value at a fixed speed. Total cost under US$100 as reported by the
  author; recommended gauge range about 3 in. w.c. for general use and 0-1 in. w.c. for pure aero work. A reported rear-door reading
  moved from about -91 Pa to -103 Pa after a deliberate separation-inducing device. A gauge cannot follow fast vortex shedding.
- Repeatability evidence (search summary of the same author's posts): -91 Pa on a logger vs -73 Pa on the gauge for the same panel,
  attributed partly to gusts and a hotter day, so a spot check carries a floor of tens of pascals.

### 2.6 Tufts
- Observable: yarn tufts on the body, video or still sequence, read the angle: attached flow aligns with the streamline, separated
  or recirculating flow shows reversed or fluttering tufts. Research versions photograph continuously with a DSLR, measure tuft
  orientation per image, and build streamline maps with separation lines and saddle points; on-road versions log free-stream speed
  with a probe so each image is tied to an inflow. Qualitative on a hobby budget.

## 3. Algorithms (published methods, implementer detail)

### 3.1 Air density (Tetens saturation pressure plus ideal-gas mixture)
theta in C, T = theta + 273.15 K. p_sat = 610.78 exp(17.27 theta / (theta + 237.3)) Pa (identical to the T-35.85 form). p_v = RH p_sat,
p_d = p - p_v. rho = p_d/(287.058 T) + p_v/(461.495 T). Dry shortcut rho = p/(287.05 T) (the note's choice). Method error stated as under
0.2 percent from -10 to 50 C. Inputs: theta, p (default 101325 Pa), RH 0-1.

### 3.2 Forces, normal load, tractive force
For speed v: D = 1/2 rho CdA v2; L = 1/2 rho ClA v2; N = m g + L; F_rr = crr N (or crr m g). Friction with load sensitivity:
mu(N) = max(0.2, mu0 - mu_drop*(N - N_ref)/1000), N_ref = m g (a Chalmers-style linear drop; skip when `mu_drop_per_kn` = 0).
Drive force: simple model F_eng = min(f_drive_max, eta P(rho)/v) with v floored at 1 m/s. Geared model: for each gear g,
rpm = v i_g i_f / r_w * 60/(2 pi), T from the interpolated curve, F_g = T i_g i_f eta / r_w; F_eng = max over gears within the rpm window
(an idealised shift; a shift-time penalty is a later option). Wheel power equation used by the Chalmers tool: P = T i v / r_l.

### 3.3 Combined-grip limit (friction ellipse)
Lateral demand F_y = m v2 kappa. Ratio r = F_y/(mu_y N). If r >= 1 the point is infeasible (no longitudinal capacity). Else
F_x,max = mu_x N sqrt(1 - r2) (Chalmers: a_x = a_x,max sqrt(1 - (a_y/a_y,max)2), built per speed into a g-g-v diagram). Then
a_acc = (min(F_eng, F_x,max) - D - F_rr) / (m(1+e)); a_dec = (min(F_x,max, brake cap) + D + F_rr) / (m(1+e)).

### 3.4 Corner speed limit
Steady cornering with no longitudinal demand: m v2 kappa = mu_y (m g + 1/2 rho ClA v2). Solve: v_lim = sqrt(mu_y g / (kappa - mu_y rho ClA / 2m));
if the denominator is not positive the corner has no speed limit (use `v_cap`). With R = 1/kappa this is the note's form with
k = mu rho ClA R / 2m; the speed gain over no-aero is 1/sqrt(1-k) - 1, about k/2 for small k. With load sensitivity, iterate N = m g + L(v) three times.

### 3.5 Forward-backward lap solver (quasi-steady state)
Nodes i = 0..n-1 with spacing ds_i (interval i joins node i and i+1 and carries kappa_i). Steps (the shape matches the published
forward-backward family: Chalmers thesis flowchart, Rossi 2020, the FSAE QSS papers):
1. v_lim[i] = v_lim(kappa_i) from 3.4, capped at `v_cap`.
2. Seed. Closed track: i0 = argmin v_lim; v[i0] = v_lim[i0] in both passes. Open track: v[0] = start speed (default 0 for a standing start,
   with a 0.5 m/s floor in the update), end speed free unless set.
3. Forward pass: v_f[i+1] = min(v_lim[i+1], sqrt(max(v_f[i]^2 + 2 a_acc(v_f[i], kappa_i) ds_i, 0))). For a closed track walk once round from i0.
4. Backward pass from the end (or round from i0 the other way): v_b[j] = min(v_lim[j], sqrt(v_b[j+1]^2 + 2 a_dec(v_b[j+1], kappa_j) ds_j)).
5. v[i] = min(v_f[i], v_b[i]).
6. Time: t = sum 2 ds_i/(v_i + v_{i+1}) (exact for constant acceleration per interval). The Chalmers form sum ds_i/v_i is a legal
   option (`time_rule`), about 0.002 percent apart at ds = 1 m on the golden track.
7. Channels: a_x = (v_{i+1}^2 - v_i^2)/(2 ds); a_y = v2 kappa; mode (limit/accel/brake/cruise); F_drive, F_brake, D, L, N, gear and rpm if geared,
   P_wheel = F_drive v; energy = sum max(P_wheel,0) dt / eta_drive (optional regeneration fraction on braking). Event tables: apex speed per corner
   segment (minimum of v), end-of-straight speed (maximum before the next brake zone), segment times.
Discretisation rule: ds <= R_min/20, never above 5 m; lap time drifts by under 0.1 percent between ds = 0.5 and 5 m on the golden track (section 5, test 8).

### 3.6 Sweeps and sensitivities
Single-parameter sweep: n points over [lo, hi], rerun 3.5 each time (the Chalmers tool does the same, plus grid sweeps). Sensitivity by central
difference with step h = 5 percent of the value or a user step. Report dt/d(CdA) and dt/d(ClA) as seconds per 0.01 m2, and the break-even
ratio d(ClA)/d(CdA) at constant lap time (track-specific; about 2.8 on the golden track).

### 3.7 Track from a path or log
1. Project lat/lon to a local plane (equirectangular about the centroid is enough at circuit scale; keep f64 and subtract the centroid).
2. Resample to uniform arc length ds (default 1 m) by linear or cubic interpolation; detect closure when the end is within 2 ds of the start and
   the end heading is within 20 degrees.
3. Curvature by Menger three-point: kappa = 2 sin(turn)/|chord| over a window of +/- w samples; or heading derivative kappa = d psi/ds with a
   Savitzky-Golay (order 2, window 5-15 samples); sign positive for left. A regular polygon returns exactly 1/R.
4. Smoothing `smooth_window_m` (default 15) plays the role of the converter's smoothing factor; `curvature_gain` (default 1.0) plays the role of
   the g-force compensator. From logged lateral acceleration, R = v2/a_y (Chalmers) is a fallback; it is noisy at low speed.
5. Segmentation: |kappa| < `kappa_straight` (default 0.001 1/m, R above 1 km) is straight; hysteresis 20 percent; merge pieces shorter than `min_seg_m` (default 3 m).
6. Edits: reverse (flip s, negate kappa), mirror (negate kappa), scale (kappa/scale).

### 3.8 Coastdown regression
Model (per sample): M_e dv/dt = -(F0 + F1 v + F2 (v + w_h)2) - M g sin(theta), M_e = M + M_rot. Two-term default F1 = 0, so CdA = 2 F2/rho and
Crr = F0/(M g). The federal method sets the effective mass term per tyre in contact (56.7 kg each for heavy trucks); for cars Vernon asks for `rot_mass_kg`
(default 0 with a warning, since a 3 percent mass error is a 3 percent CdA error, test 11).
1. Quality filter: Hampel on speed, window +/- 3 s, flag > 4.4478 x MAD (3 robust SD) from the local median. Reject runs with mean wind component along the road > 2.7 m/s or road temperature > 50 C unless overridden.
2. Air speed: v_air = sqrt(w2 + v2 + 2 v w cos(phi_w + phi_veh)) when wind data exist (the federal method regresses theoretical on measured air speed and corrects).
3. Estimator A (default, `integrated`): nonlinear least squares of the closed-form coastdown, with A = F0, B = F2, v(t) = sqrt(A/B) tan(atan(v0 sqrt(B/A)) - t sqrt(AB)/M_e);
   distance between speeds v1 and v2 is (M_e/2B) ln((A + B v1^2)/(A + B v2^2)). Estimator B (`accel`): slope of v over 1-second windows, regress -M_e a on [1, v2].
4. Pair opposite-direction runs and average the pair (cancels grade and steady wind to first order, test 10); drop CdA values beyond 2 SD of the set; require `min_runs` (default 8, 24 for the federal profile).
5. Uncertainty: report SE of CdA and Crr, their correlation (strongly negative, -0.76 to -0.94 in our Monte Carlo), and the minimum detectable change between two configurations
   dCdA_min = 2.8 sigma_CdA sqrt(2/N) per N pairs each (80 percent power, 5 percent two-sided). Wide speed windows decorrelate the two terms; 120 -> 60 km/h roughly doubles the spread of 120 -> 20 km/h.
6. Per-run density from 3.1 using logged T and p.

### 3.9 Pressure Cp and comparison with the airflow stage
Cp = (p - p_inf)/q_inf with p - p_inf the gauge reading against the static reference, q_inf = 1/2 rho v2, v the steady GPS or pitot speed (ground speed minus along-road wind).
Propagate sigma_Cp2 = Cp2 [(sigma_dp/dp)2 + (sigma_rho/rho)2 + (2 sigma_v/v)2]. Unit factor 1 in. w.c. = 249.09 Pa. Choose a gauge range >= 1.5 x the largest |dp|.
Prefer **differences between configurations** (same tap, same day) to absolute values: sensor offsets cancel. Comparison: sample the CFD surface Cp at each tap by projecting the tap position to the
nearest surface triangle and area-averaging over the puck radius; residual_i = Cp_cfd - Cp_meas; report mean bias, RMS, max, sign agreement count; pass when each |residual| <= max(0.05, 2 sigma_Cp)
(0.05 is the repeatability floor from the -91 vs -73 Pa example, 18 Pa = 0.042 Cp at 100 km/h; Vernon-proposed, confirm).

### 3.10 Tuft image analysis (white-box first)
Per frame: detect tuft segments (ridge or threshold plus skeleton), estimate image-plane angle theta_k, map to the surface through a user-picked planar patch homography (or camera pose), compare with the local
CFD streamline direction. Classes (Vernon-proposed placeholders, no published thresholds): attached if |dtheta| <= 30 degrees and the angle SD over the sequence <= 15 degrees; reversed if |dtheta| > 120 degrees;
separated/unsteady if SD > 30 degrees. Output a per-tuft table and a coloured map. Cite the research method (continuous capture, orientation measurement, streamline, separation line, saddle point), not any numbers.

## 4. Proposed Vernon nodes (stage `perf`, 16 nodes)

Labels are generic. `emulates` strings may name the reference tool; UI strings may not. **now** = implement with a real algorithm; **wb** = white-box stub first (ports and params correct).
Contracts (Json): `env.v1` {rho, T_c, p_pa, rh}, `vehicle.v1` (table above), `track.v1`, `lap_result.v1`, `coast_fit.v1`, `cp_table.v1`, `perf_report.v1`.

| Id | Label | Inputs | Outputs | Key params (default; range; unit) | Status |
|---|---|---|---|---|---|
| `perf.air_density` | Air Density | none (params) | `env` | `temp_c` (20; -40-60; C), `pressure_pa` (101325; 5e4-1.1e5), `rel_humidity` (0; 0-1), `dry_gas_const` (287.058) | now |
| `perf.aero_loads` | Aero Loads | `env`, `vehicle` or params | `table` (v, q, drag N, down N, P_aero W), `report` | `cda_m2`, `cla_m2`, `speeds_kmh` (list 50-300), `scale_from_rho` (off) | now |
| `perf.top_speed` | Top Speed | `env`, `vehicle` | `report` (v_max, power split) | `power_w` (150000), `crr` (0.012), `rr_includes_downforce` (false), `cda_m2` | now |
| `perf.vehicle` | Vehicle Definition | optional `aero` Json from the airflow stage (CdA, ClA) | `vehicle` | table in 2.1 | now |
| `perf.gg_envelope` | Grip Envelope | `vehicle`, `env` | `table` (v, a_y max, a_x accel/brake by a_y) | `speeds` (0-v_cap; 20 points), `ay_points` (21) | now |
| `perf.corner_limits` | Corner Speed Table | `vehicle`, `env` | `table` (R, v_lim, gain vs no-aero %) | `radii_m` (list 15-500), `compare_cla` (0) | now |
| `perf.track_from_path` | Track From Path | `curve` Geometry or GPS CSV `file` | `track` | `ds_m` (1; 0.1-20), `smooth_window_m` (15; 0-100), `curvature_gain` (1; 0.5-3), `closed` (auto), `kappa_straight` (0.001), `min_seg_m` (3) | now |
| `perf.track_from_segments` | Track From Segments | `segments` Json [type, length_m, radius_m] | `track` | `closed` (true), `ds_m` (1) | now |
| `perf.track_edit` | Track Edit | `track` | `track` | `reverse` (false), `mirror` (false), `scale` (1; 0.1-10) | now |
| `perf.lap_sim` | Lap Simulation | `vehicle`, `track`, `env` | `lap` (`lap_result.v1`), `geometry` (path coloured by speed), `report` | `time_rule` (trapezoid/sum_ds_over_v), `start_speed_ms` (0 open / auto closed), `end_speed_ms` (free), `ds_max_m` (5), `energy` (true) | now |
| `perf.lap_sweep` | Parameter Sweep | `vehicle`, `track`, `env` | `table`, `report` (sensitivities) | `param` (cda_m2/cla_m2/mass_kg/mu/power_w/rho), `lo`, `hi`, `n` (9; 2-101), `second_param` (none) | now |
| `perf.coastdown_fit` | Coastdown Fit | `log` (CSV: t, v, optional wind, grade, T, p, heading) | `fit` (`coast_fit.v1`), `report` | `mass_kg`, `rot_mass_kg` (0), `method` (integrated/accel), `speed_hi_kmh` (120), `speed_lo_kmh` (20), `min_runs` (8), `wind_max_ms` (2.7), `outlier_sd` (2) | now (core), wb (wind/yaw correction) |
| `perf.pressure_cp` | Pressure To Cp | `taps` (CSV: id, x,y,z, dp_pa, speed_ms, rho or `env`) | `cp` (`cp_table.v1`) | `gauge_range_pa` (249), `speed_sd_ms` (0.3), `dp_sd_pa` (3), `ref_mode` (static_port/sealed) | now |
| `perf.cp_compare` | Cp Versus Simulation | `cp`, `surface` Geometry with attrib `Cp` | `table`, `report` (bias, RMS, max, sign agreement) | `tap_radius_mm` (20; 1-100), `tol` (0.05; 0.005-0.5), `sigma_k` (2) | now |
| `perf.tuft_analysis` | Tuft Analysis | `images` (Image list), `patch` Json (homography) | `table`, `geometry` (coloured markers) | `attached_deg` (30), `reversed_deg` (120), `sd_attached_deg` (15), `sd_separated_deg` (30) | wb |
| `perf.validation_gate` | Validation Gate | `lap`/`sweep` report, `fit`, `cp` report | `report` (`perf_report.v1`, pass/warn/fail) | `claimed_dcda_m2`, `claimed_dcla_m2`, `fail_on_miss` (false) | now |

Gate rules: (1) warn if |claimed dCdA| < the coastdown minimum detectable change; (2) fail if CFD CdA lies outside the coastdown 95 percent interval by more than 0.05 m2;
(3) warn if Cp RMS residual > 0.1; (4) always print the lap-time value of the claimed change so the number the next decision needs is in the report.
Graph shape: `air_density -> vehicle -> lap_sim <- track_from_path`, `lap_sim -> lap_sweep -> validation_gate`, `coastdown_fit` and `pressure_to_cp -> cp_compare` feeding the same gate.

## 5. Golden test ideas (arithmetic cases were computed here and can be asserted now; "ref script" = Vernon's own solver, to be replaced by licensed-tool output)

1. Density. Dry, 40 C, 101325 Pa, R = 287.05: 1.12721 kg/m3 (-7.98 percent vs 1.22501 at 15 C). 45 C, RH 0.30, Tetens p_sat = 9582 Pa, R_d 287.058, R_v 461.495: 1.09757 (-10.40 percent). Tolerance 1e-4 relative. Note prints 1.127 and 1.098.
2. Aero loads, rho 1.11. 200 km/h (55.556 m/s): q 1712.96 Pa; CdA 0.64 gives D 1096.3 N and P_aero 60.905 kW; ClA 0.5 gives 856.48 N (87.34 kgf). At 250 km/h: q 2676.50 Pa, downforce 1338.25 N (note: ~1340); ratio exactly 1.5625.
3. Corner gain, m 1400, mu 1.0, rho 1.11, ClA 0.5: R 50 gives k 0.009911, v 22.143 -> 22.254 m/s (+0.499 percent); R 200 gives k 0.039643, 44.287 -> 45.192 (+2.043 percent); ClA 1.0 at R 200 gives k 0.079286, 46.154 (+4.217 percent).
4. Top speed, 150 kW wheel power, crr 0.012, m 1400, rho 1.11, `rr_includes_downforce = false`: CdA 0.64 -> 262.67 km/h; 0.69 -> 256.35 (-2.41 percent for +7.81 percent drag); 0.62 -> 265.39 (+2.71 km/h). Tolerance 0.05 km/h.
5. Constant-power launch, no drag, no rolling resistance: traction ends at v = P/(m mu g) = 10.9255 m/s at t = 1.1141 s, s = 6.086 m; 0 -> 100 km/h in 4.1579 s. Pure constant power: v(5 s) = 32.733 m/s, s(5 s) = 109.11 m. Open-track solver at ds = 0.5 m matches within 0.5 percent.
6. Skidpad. R 15.25 m (circle of 360 nodes), mu 1.5, no aero, no rolling resistance: v = 14.9776 m/s, lap 6.3975 s. Solver must match to 1e-6 relative.
7. Stadium lap (ref script, ds = 1 m): two 400 m straights, two 180-degree bends of R 40 m (length 1051.33 m), closed; m 1400, 150 kW at the wheels, mu 1.0, crr 0.012 on N, no brake cap, e = 0, trapezoid time. Results: base (rho 1.11, CdA 0.64, ClA 0.5)
   **35.896 s**, v_max 163.56 km/h, v_min 71.55 km/h. Variants: rho 1.225 35.922; ClA 0 35.992; ClA 1.0 35.801; CdA 0.69 35.923; CdA 0.59 35.870; CdA 0 and ClA 0 35.672; mu 1.2 33.959; mass 1500 35.213?? -> use 36.213;
   e = 0.05 36.173; brake cap 0.8 g 36.252. Tolerances: +/-0.05 s or 0.15 percent for a different implementation.
8. Discretisation. Same stadium at ds 0.5, 2, 5 m: 35.901, 35.893, 35.876 s. The solver must stay within 0.1 percent of 35.896 for ds <= 5 m.
9. Density and power together. Same stadium, rho 1.11 with P fixed at 150 kW: 35.896 s; with P scaled by 1.11/1.225 (k = 1): 36.375 s, +1.3 percent against 35.922 s at 1.225. The engine loss, not the aero density, is most of the lap-time effect.
10. Sensitivity (stadium). dt/d(CdA) = 0.53 s/m2 (0.0053 s per 0.01 m2); dt/d(ClA) = -0.19 s/m2 (0.0019 s per 0.01 m2); break-even d(ClA)/d(CdA) = 2.8. A sweep must be monotonic: time rises with CdA and mass, falls with ClA, mu, power.
11. Solver invariants (any track): v <= v_lim everywhere; |a_x| within the g-g-v envelope (1e-6); closed-track seam: v[0] = v[n] within 1e-9; lap time unchanged by rotating the start node (1e-6); reverse track on a symmetric circuit gives the same time.
12. Coastdown closed form (rho 1.11, M 1400, Crr 0.012, CdA 0.64): A = 164.752 N, B = 0.3552 N s2/m2, characteristic speed sqrt(A/B) = 21.537 m/s. 100 -> 60 km/h takes 46.243 s over 1005.77 m; 120 -> 40 km/h 95.324 s over 1944.04 m; deceleration at 100 km/h 0.3134 m/s2. A noise-free 10 Hz log 120 -> 20 km/h (136.3 s) fits to Crr 0.01200 and CdA 0.6400 (1e-4).
13. Coastdown noise (ref script Monte Carlo, 300 trials, seed 1, 1 s slope windows, speed noise sigma): 0.1 km/h -> CdA SD 0.022 m2, Crr SD 0.0004; 0.5 km/h -> 0.103, 0.0019; 1.0 km/h -> 0.207, 0.0036 (window 120 -> 20 km/h). The 120 -> 60 km/h window gives 0.036, 0.197, 0.386 and correlation -0.92 to -0.94. With sigma 0.1 km/h the minimum detectable change per ten runs per configuration is about 0.02 m2.
14. Coastdown bias. A 2 m/s head or tail wind on single runs gives CdA 0.707 / 0.573 and Crr 0.0129 / 0.0113; the opposite-direction pair mean returns CdA 0.6401 and Crr 0.0121. Mass given 3 percent low gives CdA 0.6208 (-3.0 percent). Pairing and mass checks are part of the node's report.
15. Cp. The note's rear-door pair -91 -> -103 Pa at an assumed 100 km/h and rho 1.11 (q 428.24 Pa): Cp -0.2125 -> -0.2405, delta -0.0280; 1 Pa is 0.00234 Cp; a 1 km/h speed error scales Cp by 2 percent; using rho 1.225 instead of 1.11 rescales Cp by 9.4 percent. The -91 vs -73 Pa spread is 0.042 Cp, so `tol` 0.05 passes sensor-to-sensor differences and fails a 0.10 error.
16. Cp comparison. Ten taps whose CFD Cp equals the measured values +/- 0.03: pass, RMS < 0.03. One tap with the wrong sign: sign agreement 9/10 and the gate warns regardless of RMS.
17. Track from path. A regular 360-gon of R 15.25 m sampled at 1 m: kappa = 0.065574 +/- 0.1 percent, length 95.82 m +/- 0.1 percent, closed. A straight line gives kappa = 0. Adding 0.3 m white noise to a R 50 m circle then smoothing: confirm kappa within 10 percent (target to confirm, not computed).
18. Tuft classes. Synthetic tufts at 0, 20, 90 and 180 degrees to the flow with SD 5 degrees: attached, attached, separated, reversed with the default thresholds (confirm thresholds on real footage).
19. Gate. Claimed dCdA 0.01 m2 with SE_diff 0.03 m2 gives a warning "below noise floor" and prints the lap value (0.0053 s on the stadium); claimed 0.10 m2 passes the detectability rule.
20. Reference-tool cross-check (licensed copy, user-made vehicle and track files only, run as separate steps): stadium and skidpad inputs; compare lap time within 1 percent, apex speed 1 percent, end-of-straight speed 2 percent, energy 3 percent (targets to confirm). Record: segment units, whether drag is applied at the corner limit, time integration rule, rolling-resistance basis, how Cd and frontal area are entered, shift model. Differences become documented parameters, not silent changes.

## 6. Sources

Vendor pages (docs only):
- https://optimumg.com/product/optimumlap/ (read: model class, inputs areas, `.OLVeh`/`.OLTra`, 60 channels, 10 percent claim)
- https://optimumg.com/optimumlap-converter-tool/ (read: converter inputs and parameters)
- https://optimumg.com/software/ (search snippet only)
- https://students.optimumg.com/product/optimumlap/ (search snippet only)
- https://www.optimumg.com/software/optimumlap/ and .../optimumlap-documentation/ (404 when fetched; the first is cited by the toolchain note)

Third-party descriptions of the tool (snippet only unless stated):
- https://fsae.com/forums/archive/index.php/t-3319.html (403 on fetch; the "10 parameters" and 10 percent statements come from the search summary)
- https://www.scribd.com/document/457377590/OptimumLap-Getting-Started and https://studylib.net/doc/25891610/toaz.info-optimumlap-getting-started-pr-82588aeec9ec858fd... (body not retrievable; combined friction limit and listed limitations from the summary)
- https://www.researchgate.net/publication/337750048_Development_Of_An_Algorithm_For_Prediction_Of_Racing_Vehicle_Dynamics_And_Comparison_With_A_Commercial_Software (three-column track text file, via summary)
- https://saemobilus.sae.org/papers/lap-time-simulation-formula-sae-vehicle-quasi-steady-state-model-2016-36-0164
- https://saemobilus.sae.org/papers/lap-time-simulation-tool-development-electric-formula-student-car-2019-01-0163

Lap-simulation method:
- Albertsson, Nilsson, Wigh, "Lap Time Simulation For A Formula Student Car", Chalmers bachelor thesis, 2024 (full text read): https://odr.chalmers.se/bitstreams/2c14d842-430d-4bbd-b55f-06859f59c555/download
- Rossi et al., "A Simple Mono-Dimensional Approach for Lap Time Optimisation", Applied Sciences 10(4):1498, 2020 (snippet only): https://www.mdpi.com/2076-3417/10/4/1498/htm and https://shura.shu.ac.uk/25882

Air density:
- https://en.wikipedia.org/wiki/Density_of_air (formulas, constants, Tetens, error statement)

Coastdown:
- 40 CFR 1037.528, coastdown CdA for tractors (read; equations 1037.528-16 not rendered): https://www.law.cornell.edu/cfr/text/40/1037.528 and https://www.ecfr.gov/current/title-40/chapter-I/subchapter-U/part-1037/subpart-F/section-1037.528
- 40 CFR 1066.310 (snippet only): https://www.law.cornell.edu/cfr/text/40/1066.310
- SAE J2263 is cited by the regulation; its text was not read (name only).
- https://cleanfleetreport.com/?p=33672 (basic method and noise sources)
- https://theicct.org/wp-content/uploads/2021/06/ICCT_aero_drag_briefing_20190814.pdf (search snippet only)

Surface pressure and tufts:
- https://ecomodder.com/forum/639288-post1.html (unreachable when fetched; -91 to -103 Pa figures come from the toolchain note); https://ecomodder.com/forum/624360-post1.html and https://ecomodder.com/forum/637126-post2.html (search snippets: kit, cost, -91 vs -73 Pa)
- https://autospeed.com.au/cms/A_111111/article.html (snippet: gauge range advice)
- Edgar, "On-road Aerodynamic Measurement Techniques", SAE 2022-01-1096 (snippet only): https://saemobilus-test.cld.sae.org/papers/road-aerodynamic-measurement-techniques-2022-01-1096
- https://research.chalmers.se/publication/237074 and https://saemobilus.sae.org/papers/surface-flow-visualization-a-full-scale-passenger-car-quantitative-tuft-image-processing-2016-01-1582 (abstracts)
- https://research.chalmers.se/publication/247549 (on-road tuft study, abstract)

Project documents:
- C:\Users\mabdu\REA\3D printed body kit toolchain.md (section "The performance math, at Dubai density"; all worked numbers re-derived in section 5)
- C:\Users\mabdu\REA\vernon\CLEAN_ROOM.md

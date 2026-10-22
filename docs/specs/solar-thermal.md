# Spec: solar incident radiation, sun position and panel surface-temperature check (reference: Ladybug Incident Radiation, Blender Sun Position / Helio, hand energy balance)

Status: draft v0.2, 2026-10-22 (v0.1 2026-10-21, extended and re-verified). Clean-room spec: public help pages, papers and open-source docs only (see CLEAN_ROOM.md).
Vernon stage served: 7 solar / UV dose and surface temperature. It consumes the surface meshes from stages 2-6, and emits per-face W/m2, kWh/m2, surface temperature and heat-deflection margin that the material choice and the FEA stage check.
Marker key: **(doc)** stated by a vendor/project page, **(inf)** inferred from wording, **(V)** a Vernon design choice, **(calc)** number computed by the spec author with the stated equations (re-derive before freezing).
Source tags (section 6): [F] fetched this session, [S] seen only in search results, [R] recalled from memory, verify before relying on it, [rep] value taken from the project report (`3D printed body kit toolchain.md`), not re-fetched.

## 1. Scope and reference tools

| Reference | Role in the body-kit pipeline | Version / source |
|---|---|---|
| Ladybug "LB Incident Radiation" (+ "LB Cumulative Sky Matrix") and the Python class `ladybug_radiance.study.radiation.RadiationStudy` | per-face cumulative kWh/m2 or mean W/m2 from an EPW-driven sky matrix, ray-cast occlusion, no specular | rolling docs, no version number on the pages (doc) |
| Radiance `gendaymtx` (what the sky matrix component runs, doc) | Perez all-weather sky -> per-patch radiance, Tregenza/Reinhart patches | man page + source, BSD-style licence [F] |
| EPW weather file | hourly DNI / DHI / dry bulb / wind / IR / sky cover for one location | EnergyPlus 9.6 data dictionary [F] |
| NOAA solar calculator equations, NREL SPA | sun azimuth/altitude (cheap and reference-grade) | NOAA PDF [F]; Reda and Andreas 2004 [R] |
| Blender Sun Position add-on (bundled, GPL) and Helio 1.1.1 (GPL-3+, Blender 5.0+) | place a sun lamp; relative (uncalibrated) cosine-and-shadow bake | 4.0 manual [F]; extension page [F] |
| Report hand model | peak-hour panel temperature vs heat-deflection temperature (HDT) | project report [rep] |

Out of scope here: specular / inter-reflection (the reference recommends a different Radiance-based workflow for it, doc), spectral UV, conjugate heat transfer, transient thermal mass, PV yield. Flattening of panels (Rhino Squish class) is specified in `rhino8-mesh-tools.md`, not here.
Units: model lengths mm; irradiance W/m2; energy kWh/m2 (1 kWh/m2 = 1000 Wh/m2); temperatures degC in the UI, kelvin inside solvers; angles degrees at the API, radians inside. Azimuth: degrees clockwise from true north (0 N, 90 E, 180 S). Altitude: degrees above the horizon (zenith = 90 - altitude).

## 2. Feature catalogue (observable behaviour)

### 2.1 Incident radiation component (reference: LB Incident Radiation)
Inputs (doc): a sky matrix (ground reflectance is taken from it, no separate input); geometry (Breps are subdivided to a grid of points, each mesh face is analysed directly); optional context geometry that only blocks; grid size; offset distance; irradiance switch; legend settings; CPU count; run switch.

| Parameter | Type / range | Default | Observed behaviour |
|---|---|---|---|
| grid_size | float > 0, model units | required | sub-division size for Breps; smaller = finer and slower; should be smaller than the smallest geometry or context piece (doc) |
| offset_dist | float >= 0, model units | component: 10 cm expressed in model units; library class: 0 (doc, both pages; lift direction not stated, inf: along the face normal) | test points are lifted off the surface so the surface does not block itself |
| irradiance | bool | false | false = cumulative kWh/m2 over the sky-matrix period; true = average W/m2 over the period (doc; whether night hours count in the average is not stated, inf: all hours of the period) |
| cpu_count | int | all cores minus one, min 1 | parallel ray intersection (doc) |
| by_vertex (library only) | bool | false | per vertex instead of per face; total assumes equal area per vertex (doc) |
| use_radiance_mesh (library only) | bool | false | converts context to Radiance meshes: less memory, slower set-up (doc) |

Outputs (doc): test points; results per point (kWh/m2); total over all input geometry (kWh, not area-normalised, so model units must be right); coloured mesh; legend; title; an intersection matrix with one row per point and one value 0..1 per sky patch (0 patch not visible, 1 faces the point head-on; 145 values Tregenza, 577 high-density Reinhart).
Limits (doc): no solar reflections; ground reflection is a crude emissive ground hemisphere using the sky matrix ground reflectance, and putting ground geometry into the context blocks that approximation; the CAD ray intersector is fast for simple scenes but scales poorly with many points / complex context; for complex scenes or reflections the vendor points to a separate Radiance-based tool. A forum thread reports odd Tregenza results for very low sun [S].

### 2.2 Sky matrix component (reference: LB Cumulative Sky Matrix)
Inputs (doc): north (-360..360, counter-clockwise angle from north to +Y, default 0; 90 = west); location; annual hourly direct normal and diffuse horizontal collections; hoys (hours of year, default whole year); high_density (false = Tregenza, true = Reinhart, about 4x the patches); ground_ref (0..1, default 0.2); working folder. Output: a sky matrix of per-patch radiation. Units (doc, library skymatrix page [F]): per-patch values are cumulative kWh/m2 over the weather duration, and dividing by the duration in hours gives average W/m2; ground reflectance 0..1 (default 0.2) is stored with the matrix; north is the counter-clockwise angle from North to +Y (90 = west, 270 = east); hoys 0..8760, default whole year. The page lists no `gendaymtx` flags and no sun-hour offset. Built by `gendaymtx` (doc). Implementer reading (inf): a patch value is the irradiance on a surface facing that patch head-on, so the intersection-matrix factor (0..1) is the cosine of incidence times visibility, and the Vernon mapping is `R_p = (Lsky_p*Omega_p + Bsun_p)/1000` kWh/m2 (3.4, 3.5).
`gendaymtx` facts (doc): Perez all-weather model; reads EPW or a plain 5-column weather format; column per time step, row per patch; row 0 is the ground; `-m 1` = 145 sky patches (Tregenza), `-m 4` = 2305; `-g` ground colour default 20 % grey; `-r` rotation about zenith; `-d` sun only, `-s` no sun; `-O1` total solar radiance (W/sr/m2) instead of visible; first patch sits on the horizon toward +Y and patches spiral up; the sun is spread over the four nearest patches weighted by closeness.

### 2.3 EPW weather file (EnergyPlus data dictionary)
Eight header records then hourly rows of 35 comma-separated fields.
Header (doc): LOCATION (city, state, country, source, WMO id, latitude -90..90, longitude -180..180, time zone hours from GMT -12..12, elevation m); DESIGN CONDITIONS; TYPICAL/EXTREME PERIODS; GROUND TEMPERATURES (depth, conductivity, density, specific heat, 12 monthly means); HOLIDAYS/DAYLIGHT SAVING; COMMENTS 1; COMMENTS 2; DATA PERIODS (count, records per hour, start day-of-week, start/end day).
Columns used by this spec (1-based; missing code in brackets): 1 year, 2 month, 3 day, 4 hour 1-24, 5 minute, 7 dry bulb degC [99.9], 8 dew point degC [99.9], 9 RH % [999], 10 station pressure Pa [999999], 11 extraterrestrial horizontal Wh/m2 [9999], 12 extraterrestrial direct normal Wh/m2 [9999], 13 horizontal infrared Wh/m2 [9999], 14 global horizontal Wh/m2 [9999], 15 direct normal Wh/m2 [9999], 16 diffuse horizontal Wh/m2 [9999], 21 wind direction deg [999], 22 wind speed m/s 0..40 [999], 23 total sky cover tenths [99], 24 opaque sky cover tenths [99], 33 albedo [999].
Rules (doc): if horizontal IR is missing the host program derives it from dry bulb, dew point and opaque sky cover; missing or negative direct normal / diffuse horizontal are set to 0 and counted. Dictionary lists IR in Wh/m2 though its formula yields W/m2 (doc, treat as W/m2 for hourly data).
Timing (inf): hour N covers the 60 minutes ending at N:00 local standard time; radiation fields are hourly sums, so Wh/m2 per hour equals mean W/m2. The reference's choice of instant for the sun position inside an hour is not stated by the fetched pages (the `gendaymtx` source summary suggests the record hour is used directly, plus an optional sunrise/sunset interval nudge).

### 2.4 Sun position in Blender (reference: Sun Position add-on, Helio)
Sun Position (doc, 4.0 manual): based on the NOAA/ESRL online calculator, "a certain degree of accuracy" and no number given. Normal mode drives a sun object, a collection that visualises an analemma (yearly positions at one clock time) or a diurnal path, or a Sky Texture node; HDRI mode locks a sun lamp to the brightest spot of an environment texture. Location: longitude -180..180, latitude -90..90, coordinate paste field. North offset: north is +Y by default; "show north" draws a dashed line. Time is local, stored as a decimal number, with time-zone and daylight-saving settings.
Helio 1.1.1 (doc): needs Blender 5.0+, offline, GPL-3+. Inputs: GPS (decimal or DMS) or lat/lon/altitude; date 1901-2099; local time; UTC offset auto-detected from position including DST, manual override -12..+14; north offset -360..360. Method: NOAA solar calculator; optional refraction correction (affects sunrise/sunset). Outputs: sun lamp position, sky texture settings with colour temperature from elevation, sun strength in W/m2 from Kasten's air-mass formula with altitude correction, baked keyframes over a range with step and Linear / Constant / Bezier interpolation. It has no EPW input and no kWh/m2 output (report, doc). A Cycles diffuse-direct bake therefore yields only a relative cos(theta)-and-shadow map (report).

### 2.5 Peak-hour surface temperature (reference: report hand model)
Steady balance per unit area for a panel insulated at the back: `alpha*G = h_c*(Ts - Ta) + eps*sigma*(Ts^4 - Tsky^4)`, temperatures in K. Report settings: G = 1000 W/m2, eps = 0.9, Tsky = Ta - 15 K, `h_c = 5.7 + 3.8*V` (V m/s; the simple wind-driven correlation also listed in the EnergyPlus reference, doc), sigma 5.67e-8. Abu Dhabi measured peak global irradiance 1041 W/m2 [rep]. Results are upper bounds (no conduction into the car, no back-face cooling).

| alpha | still air, Ta 45 / 50 degC | V = 1 m/s | V = 3 m/s |
|---|---|---|---|
| 0.95 (black, raw carbon fibre) | 105 / 108 | 93 / 97 | 80 / 84 |
| 0.60 (mid grey) | 83 / 86 | 75 / 79 | 66 / 70 |
| 0.30 (white) | 61 / 66 | 58 / 62 | 54 / 58 |

Conclusions in the report: the design case is parked at noon, colour is worth about 40 K, ASA is risky on black.

### 2.6 Material heat table (all [rep], vendor datasheets, test load 0.45 MPa unless noted)
| Material | HDT degC | Notes |
|---|---|---|
| PLA | 57 | below every dark/mid panel temperature |
| PETG | 69-72 | |
| ASA | 100-103 | marginal on black (105 degC computed) |
| PET-CF | 205 | water absorption 0.37 % |
| PAHT-CF | 194 (datasheet); a second vendor gives 84.8 unannealed vs 190.7 annealed | annealing caveat: value is conditional on a post-print anneal |
| PPA-CF | 227 at 0.45 MPa, 196 at 1.8 MPa | Z tensile 57 MPa vs 168 MPa in XY |
| PA6-CF | not given | absorbs 2.35 % water |
| "CF-ABS" intake elbow failure | samples Tg only 53-54 degC | HDT on the datasheet is not a guarantee for the real spool |
Report guidance: test the actual spool; UV is a few percent of global irradiance, so the radiation map is a proportional proxy for UV dose, not a spectral UV result.

## 3. Algorithms (implementer level)

### 3.1 EPW reader
1. Read 8 header lines by keyword; parse LOCATION (lat, lon, tz, elev). Read rows with a CSV splitter; require 35 fields, else error with the row number. 8760 rows (or 8784); leap-day rows are ignored when the year column varies (V).
2. Per field apply the missing-code table (2.3). DNI/DHI missing or negative -> 0 and increment a counter in the report (doc). Dry bulb / dew point / wind missing -> carry the previous valid hour and count it (V).
3. Output a column table indexed by hour of year `hoy = (day_of_year-1)*24 + (hour-1)` plus location. Mean W/m2 equals Wh/m2 for hourly records.

### 3.2 Sun position
Local standard time to solar time (NOAA equations, [F] PDF; coefficients verified against it):
```
gamma = 2*pi/N_year * (doy - 1 + (hour - 12)/24)            N_year = 365 or 366, hour = clock hour (0-23.99)
eqtime[min] = 229.18*(0.000075 + 0.001868 cos g - 0.032077 sin g - 0.014615 cos 2g - 0.040849 sin 2g)
decl[rad] = 0.006918 - 0.399912 cos g + 0.070257 sin g - 0.006758 cos 2g + 0.000907 sin 2g - 0.002697 cos 3g + 0.00148 sin 3g
time_offset[min] = eqtime + 4*lon[deg east] - 60*tz[h]       tst[min] = hr*60 + mn + sec/60 + time_offset
ha[deg] = tst/4 - 180
cos(zenith) = sin(lat) sin(decl) + cos(lat) cos(decl) cos(ha)
cos(180 - az) = -(sin(lat) cos(zenith) - sin(decl)) / (cos(lat) sin(zenith));  az = 360 - that angle when ha > 0 (afternoon)
sunrise/sunset zenith 90.833 deg: ha_s = acos(cos(90.833)/(cos lat cos decl) - tan lat tan decl)
sunrise_utc[min] = 720 - 4*(lon + ha_s) - eqtime; sunset: use -ha_s; solar noon_utc = 720 - 4*lon - eqtime
```
Algorithm tiers (V): `noaa_low` (above; NOAA states 1901-2099 for the spreadsheet form, fine for EPW years); `noaa_meeus` (Meeus-based series, [R] constants: geometric mean longitude L0 = 280.46646 + T(36000.76983 + 0.0003032 T); mean anomaly M = 357.52911 + T(35999.05029 - 0.0001537 T); eccentricity e = 0.016708634 - T(0.000042037 + 1.267e-7 T); equation of centre C from sin M, sin 2M, sin 3M with coefficients 1.914602, 0.019993, 0.000289; apparent longitude subtracts 0.00569 + 0.00478 sin(125.04 - 1934.136 T); obliquity 23 deg 26' 21.448'' - 46.815''T plus 0.00256 cos(125.04 - 1934.136 T); T = Julian centuries from J2000); `spa` (Reda and Andreas, +-0.0003 deg for years -2000..6000, [S] NREL page; table-driven heliocentric series, nutation, aberration, parallax, topocentric correction; needs delta-T). Refraction (doc, NOAA): none above 85 deg altitude; 5..85 deg: (58.1/tan h - 0.07/tan^3 h + 0.000086/tan^5 h)/3600; -0.575..5 deg: (1735 - 518.2h + 103.4h^2 - 12.79h^3 + 0.711h^4)/3600; below -0.575: -20.774/tan h/3600 (degrees). Default refraction OFF for radiation (the sun disk is not visible geometry), ON for sunrise/sunset reports.
Model frame: sun unit vector `s = (sin(a')*cos(alt), cos(a')*cos(alt), sin(alt))` with `a' = az - north_angle` where `north_angle` (deg, counter-clockwise from north to +Y, default 0, same convention as the sky-matrix reference) so +Y is north, +X is east, +Z up. The vehicle frame is a separate transform (V).
Hour instant (V): sun evaluated at the middle of the EPW hour (clock = hour - 0.5) with a parameter `sun_time_offset_h`, default -0.5; hours whose sun altitude at the mid-instant is below -0.27 deg carry zero direct.

### 3.3 Sky patches (Tregenza / Reinhart)
Tregenza 1987 [R] and Radiance `reinhart.cal` [F]. Given subdivision factor MF (1, 2, 4):
- Row height `alpha = 90 / (7*MF + 0.5)` degrees; 7*MF rows i = 0..7*MF-1 span `[i*alpha, (i+1)*alpha]`, centre altitude `(i+0.5)*alpha`.
- Azimuth patches per row `n_i = MF * T[floor(i/MF)]`, `T = [30,30,24,24,18,12,6]`; patch j centre azimuth `(j+0.5)*360/n_i`, clockwise from the +Y direction (doc: first patch toward +Y).
- One zenith cap above `7*MF*alpha` (a single patch, centre direction +Z, altitude > 90 - alpha/2).
- Total `N = 144*MF^2 + 1`: 145, 577, 2305 for MF 1, 2, 4 (docs agree: 145 / 577 Ladybug, 2305 gendaymtx `-m 4`).
- Solid angle of row i: `2*pi*(sin(hi) - sin(lo)) / n_i`; cap `2*pi*(1 - sin(7*MF*alpha))`. Sum over all patches = 2*pi.
- Direction vector of patch p: `v_p = (sin(az) cos(alt), cos(az) cos(alt), sin(alt))` in the sun frame (3.2). The ground is NOT a patch in Vernon: it is a separate uniform lower hemisphere (V, matches doc "ground hemisphere").
- Bin lookup for a direction: altitude row = floor(alt/alpha) (clamped), azimuth = round to nearest centre with wrap (doc, `reinhart.cal`). A third-party code summary [F] disagrees with its own docstrings on Reinhart counts (2314 vs 2305): trust the Radiance and Ladybug documents.

### 3.4 Sky matrix from weather (white-box first)
Per time step t with DNI_t, DHI_t, solar altitude a_t > -0.27 deg (white-box target: Perez et al. 1990 and 1993 [R]):
1. Sky clearness epsilon (from DHI, DNI, zenith with the published 1.041 factor) selects one of 8 bins; brightness Delta = DHI * air_mass / (I0 * eccentricity). Clamp both to the paper's ranges. Air mass: Kasten-Young [R].
2. Five Perez coefficients (a..e) are linear in zenith angle and Delta per bin (Table 1 of the 1993 paper; coefficients are read from the paper, not copied from any tool).
3. Relative luminance of patch p: `f(zeta_p, gamma_p) = [1 + a exp(b/cos zeta_p)] * [1 + c (exp(d gamma_p) - exp(d pi/2)) + e cos^2 gamma_p]`, zeta = patch zenith angle, gamma = angle between patch and sun; clip negatives.
4. Normalise so the horizontal sum equals DHI: `L_p = k * f_p`, `k = DHI / sum_p( f_p * Omega_p * sin(alt_p) )` (doc: `gendaymtx` scales to the horizontal diffuse value). Fallback if the sum ~ 0: uniform `L = DHI/pi`.
5. Ground radiance `L_g = rho_g * (DHI + DNI*sin(a_t)) / pi` (doc: horizontal total times reflectance over pi), rho_g default 0.2 (doc).
6. Sun: weights `w_q = 1/(1.002 - s.v_q)` over the 4 patches nearest the sun, normalised to sum 1 (doc). Patch beam power `B_q = w_q' * DNI` [W/m2 at normal incidence] where Vernon rescales `w' = w * sin(a_t) / sum(w_q sin(alt_q))` so the horizontal component is conserved exactly (V; (calc) horizontal error 0.00 %).
7. Isotropic fallback (V, `sky_model = isotropic`): `L_p = DHI/pi` for all patches, no circumsolar term; expect larger error on tilted faces under clear sky.
8. Store per-step arrays or the cumulative sums over the selected hours: `Lsky_p = sum_t L_p,t * dt` (Wh/m2/sr) and `Bsun_p = sum_t B_q,t * dt` (Wh/m2), `Lgnd = sum_t L_g,t dt`. dt = 1 h for EPW.

### 3.5 Incident radiation on a mesh
For each test point x with unit normal n (face centroid, or Brep/NURBS grid point, or vertex), lifted to `x + offset*n`:
```
E_sky   = sum_p  Lsky_p * Omega_p * max(0, n.v_p) * V_p(x)
E_sun   = sum_q  Bsun_q * max(0, n.v_q) * V_q(x)               (sun_mode = patch)
E_gnd   = Lgnd  * (lower hemisphere cosine integral) * Vg(x)  = pi*Lgnd*(1-cos(beta))/2 * Vg(x),  beta = tilt of n from +Z
E       = E_sky + E_sun + E_gnd      [Wh/m2 over the period]
```
`V(x) in {0,1}` from a ray from x along v against context + the study mesh itself (any hit before infinity blocks; use backface-ignoring any-hit BVH rays; `offset` plus a ray `tmin` equal to `offset` avoid self-hits). Ground: if the context contains a ground plane below the point, rays to the lower hemisphere hit it and `Vg = 0` (doc behaviour, no reflection is substituted).
Outputs: `kWh/m2 = E/1000`; `W/m2 = E / hours_in_period` (default all hours; option `daylight_hours`); total kWh `= sum_f E_f/1000 * A_f` with A in m2 (mm2 * 1e-6) (doc: not area-normalised). Sub-sampling option (V): `patch_subsamples = 1` casts to patch centres (reference-like); 4 jittered directions per patch reduces boundary error.
`sun_mode = exact` (V, white-box): per hour with sun above the horizon cast one ray along the exact sun vector, `E_sun = sum_t DNI_t * max(0, n.s_t) * V_t(x)` (cost = points x daylight hours; use for the peak hour and for validating patch mode).
Cost model: rays = points x (N_patches + hours if exact). 100k faces x 145 = 14.5 M rays: needs a BVH, not a brute-force loop.
Intersection matrix export (V): `M[x][p] = max(0, n.v_p) * V_p(x)` (0..1) to match the reference matrix of 2.1, so sky matrices can be re-used.

### 3.6 Sky temperature and wind
- From EPW horizontal IR: `Tsky = (IR/sigma)^0.25` (doc, EnergyPlus) with sigma 5.6697e-8 (EnergyPlus) or 5.670374e-8 (CODATA, V default).
- If IR is missing: clear-sky emissivity `eps_clear = 0.787 + 0.764 ln(Tdp_K/273)` (Clark and Allen, default in the reference), cloud correction `1 + 0.0224N - 0.0035N^2 + 0.00028N^3` (Walton 1983, N = opaque sky cover in tenths), `IR = eps*sigma*Tdb_K^4`. Alternatives listed (doc): Martin-Berdahl, Brunt, Idso.
- Report default (V): `Tsky = Ta - 15 K`. (calc) the Clark-Allen route gives depressions 13.4 K (Tdb 45, Tdp 20) to 15.9 K (Tdp 10), supporting the 15 K choice for clear skies; cloud cover N = 10 collapses it to 2.3 K.
- Wind: the vehicle case uses `V = speed relative to air`; parked uses EPW wind speed 22 at 10 m height, scaled by the power law `V_z = V_met * (delta_met/z_met)^a_met * (z/delta)^a` (doc, defaults z_met 10 m, a_met 0.14, delta_met 270 m; flat open terrain a = 0.14, delta = 270; city a = 0.33, delta = 460) with z = 0.5 m default (V).

### 3.7 Surface temperature solve
Unknown Ts (K). View factors (V, tilt beta from +Z): `Fsky = (1+cos beta)/2`, `Fg = 1 - Fsky`; ground temperature `Tg = Ta + dT_ground` (default 0, V, EnergyPlus assumes Tg = Ta, doc; a hot road is worse).
```
f(Ts) = alpha*G - hc*(Ts - Ta) - eps*sigma*( Fsky*(Ts^4 - Tsky^4) + Fg*(Ts^4 - Tg^4) ) - hb*(Ts - Tin)
```
`hb` back-face coefficient W/m2K and `Tin` interior temperature (defaults hb = 0: insulated upper bound as in the report). `hc = 5.7 + 3.8 V` default (McAdams form, doc); alternatives from the EnergyPlus reference (doc): MoWiTT `sqrt((0.84*|dT|^(1/3))^2 + (a*V^b)^2)` with a, b = 3.26, 0.89 windward / 3.55, 0.617 leeward; natural convection `1.31*|dT|^(1/3)` (vertical). f is strictly decreasing in Ts (derivative `-(hc + hb + 4 eps sigma Ts^3) < 0`), so bracket `[Ta-60, Ta+250]` K and use bisection to 1e-4 K, or Newton with bisection safeguard; with the temperature-dependent hc iterate 5-10 times. G is the per-face absorbed-relevant irradiance (W/m2) from 3.5 for the chosen design hour (`peak_hour_select`), `alpha` solar absorptance from the colour/material, `eps` default 0.9 (report).

### 3.8 Heat deflection check
`HDT_eff = hdt_annealed` if the part is annealed else `hdt_as_printed` (falls back to `hdt_datasheet`); amorphous materials also cap at `Tg_measured` when given. Verdict per face: `fail` if `Ts > HDT_eff`; `warn` if `Ts > HDT_eff - margin_warn` (default 10 K, V); else `ok`. Report: worst face id, fraction of area in each verdict, minimum margin K. The UV proxy is `UV_dose = f_UV * H_annual` with `f_UV` default 0.045 [R] (user-editable; proxy only).

## 4. Proposed Vernon nodes

Labels contain no vendor names. Status: **now** = fully specified above and small, **WB** = white-box-first (transparent reference implementation validated against the cited paper/source before optimising).

| Id | Label | Inputs | Outputs | Key params | emulates | Status |
|---|---|---|---|---|---|---|
| solar.weather_import | Weather File Import | file path | hourly table, location, import report | missing_policy (zero/carry), strict_field_count | EPW reader (EnergyPlus) | now |
| solar.design_hour | Design Hour | none | one-row weather table, location | lat, lon, tz_h, date, clock_time_h, dni_w_m2 (900), dhi_w_m2 (100), t_air_c (45), wind_m_s (0), t_dew_c (opt), cloud_tenths (0) | report design case | now |
| solar.peak_hour_select | Peak Hour Select | hourly table | hoy list, chosen row | criterion (max_ghi / max_t_air / max_ghi_where_t_air_above), t_air_min_c (35), count (1) | report "peak hour" | now |
| solar.sun_position | Sun Position | location, hoy list or datetime list | altitude, azimuth, zenith, eqtime, decl, sunrise/noon/sunset | algorithm (noaa_low/noaa_meeus/spa), refraction (off), north_angle_deg (0), sun_time_offset_h (-0.5) | NOAA calculator, Blender Sun Position, Helio | now (noaa), WB (spa) |
| solar.sun_path | Sun Path Curves | location, year | polylines (analemma, diurnal), sun vectors | mode (analemma/diurnal), clock_time_h, step_min (10), radius_mm | Sun Position collections | now |
| solar.sky_temperature | Sky Temperature | hourly table row(s) | t_sky_c, ir_w_m2, depression_k | source (epw_ir/clark_allen/fixed_depression), depression_k (15), correlation (clark_allen/martin_berdahl/brunt/idso) | EnergyPlus climate calcs | now |
| solar.sky_matrix | Sky Patch Matrix | hourly table, location, hoy list | patch matrix (Lsky, Bsun, Lgnd), patch table | density_mf (1/2/4), ground_reflectance (0.2), sky_model (perez/isotropic), north_angle_deg (0), sun_spread (4), conserve_horizontal (true) | gendaymtx via sky matrix component | WB (perez), now (isotropic) |
| solar.incident_radiation | Incident Radiation | sky matrix, study mesh, context meshes | per-face kWh/m2 and W/m2, total kWh, intersection matrix, report | grid_size_mm (50), offset_mm (1), irradiance (false), by_vertex (false), sun_mode (patch/exact), patch_subsamples (1), ground_mode (hemisphere/none), average_over (all/daylight) | LB Incident Radiation | now |
| solar.sun_exposure_map | Relative Sun Exposure | study mesh, context, sun vectors | per-face 0..1 map | weights (uniform/dni), normalise (true) | Cycles/VOP dot(N,L)+shadow bake | now |
| solar.surface_balance | Surface Temperature Balance | per-face W/m2, face normals, weather/design row | per-face Ts_c, hc, convergence report | alpha (0.95), emissivity (0.9), wind_m_s, hc_model (mcadams/mowitt), sky_model (fixed_depression/epw), ground_offset_k (0), back_h (0), t_inside_c | report energy balance | now |
| solar.balance_sweep | Balance Sweep Table | design inputs | table alpha x wind x Ta of Ts | alphas (0.95,0.6,0.3), winds (0,1,3), t_air (45,50), g (1000) | report result table | now |
| solar.material_table | Material Heat Table | none (data) or user CSV | rows: material, hdt_c, hdt_annealed_c, tg_c, water_abs_pct, source_tag | preset (report_v1) | report material table | now |
| solar.hdt_check | Heat Deflection Check | per-face Ts_c, material row(s), face area | per-face margin_k, verdict, summary | annealed (false), margin_warn_k (10), use_tg_cap (true) | report HDT comparison | now |
| solar.uv_dose_proxy | UV Dose Proxy | per-face kWh/m2 (annual) | per-face UV kWh/m2 | f_uv (0.045), label_as_proxy (true) | report UV-proxy remark | now |

Total proposed: 14 nodes. Recommended order: weather_import, design_hour, sun_position (noaa_low), material_table, surface_balance, balance_sweep, hdt_check (these five reproduce the report table end-to-end with no ray casting), then sky_matrix (isotropic), incident_radiation, sun_exposure_map, sky_temperature, peak_hour_select, uv_dose_proxy, sun_path, then WB: Perez sky matrix, SPA.

## 5. Golden test ideas (numbers are (calc) unless marked)

Tolerances are Vernon pass criteria. "(ref)" = record from a licensed/open copy of the reference tool on a user-made input before freezing.

**G1 report table** (sigma 5.670374e-8, eps 0.9, G 1000, Tsky = Ta - 15, hc = 5.7 + 3.8V, back insulated), Ts degC at Ta 45 / 50, tolerance +-0.2 K vs these, and every cell rounds to the report integers:
- alpha 0.95: still 104.7 / 108.1; 1 m/s 93.4 / 97.3; 3 m/s 79.6 / 83.9
- alpha 0.60: still 82.6 / 86.4; 1 m/s 74.8 / 79.0; 3 m/s 65.9 / 70.4
- alpha 0.30: still 61.3 / 65.5; 1 m/s 57.6 / 62.1; 3 m/s 53.7 / 58.4

**G2 sensitivities** (black, G 1000, Ta 45, still air): eps 0.8 -> 109.05, eps 0.95 -> 102.76; Tsky depression 0 / 15 / 25 K -> 110.15 / 104.73 / 101.46; back loss hb 5 with Tin 45 -> 90.58, Tin 60 -> 94.21 degC. Tilted cases (alpha 0.95, G 600, Ta 45, still): vertical (Fsky 0.5, Tg = Ta) -> 83.62; vertical with ground +20 K -> 88.38. alpha 0.6, G 900, Ta 50, V 3, tilt 20 deg -> 68.16.

**G3 HDT verdicts** (margin 0, as-printed): black, Ta 45, still (104.7): PLA fail, PETG fail, ASA fail (104.7 > 100 and > 103), PET-CF ok, PAHT-CF annealed (190.7) ok, PAHT-CF unannealed (84.8) fail, PPA-CF ok. White, Ta 45, still (61.3): PLA fail (57 < 61.3), PETG ok, ASA ok. White, 3 m/s, Ta 45 (53.7): PLA ok at margin 0 but warn at 10 K. Note: the report says PLA and PETG are below panel temperature "in every row"; the table does not reproduce that for PETG in the white column or for mid grey at 3 m/s (65.9 < 69), so the node must show computed verdicts, not that sentence.

**G4 sky temperature** (EnergyPlus worked example, doc): Tdb 20 degC, Tdp 10 degC, N 0 -> eps_sky 0.8149, IR 341.2 W/m2, Tsky 5.38 degC (depression 14.6 K); tolerance +-0.1 W/m2. Dubai-like: Tdb 45, Tdp 20, N 0 -> IR 488.8 W/m2, Tsky 31.56; with N = 5 -> eps 0.8915, Tsky 35.99.

**G5 sun position** (lat 25.2048, lon 55.2708, tz +4, no DST; `noaa_low` vs `noaa_meeus` agree within the stated spread, SPA to 0.001 deg):
- 2026-06-21 12:00 local: altitude 85.0 +-0.15 (low 85.07, Meeus 84.96); azimuth untested (within 5 deg of zenith).
- 2026-12-21 12:00: altitude 41.21 +-0.05; azimuth 174.89 +-0.1.
- 2026-06-21 07:00: altitude 18.23 +-0.15; azimuth 72.06 +-0.1.
- 2026-08-01 15:00: altitude 53.54 +-0.15; azimuth 266.77 +-0.5.
- 2026-03-20 12:00: altitude 63.51 +-0.25; azimuth 164.67 +-0.3.
- (calc re-run 2026-10-22 with `noaa_low`, 365-day year; the Meeus tier differs by up to ~0.1 deg, so the tolerances above are wide enough for both.)
- Sunrise / noon / sunset (zenith 90.833, +-2 min): 21 Jun 05:29 / 12:20 / 19:11 (day length 13.71 h); 21 Dec 07:00 / 12:17 / 17:34 (10.57 h).
- SPA reference case [R] (recall; verify with the NREL download before freezing): 2003-10-17 12:30:30 UTC-7, lat 39.742476, lon -105.1786, elevation 1830.14 m, 820 mb, 11 degC, delta-T 67 s, refraction 0.5667 deg -> zenith 50.1116, azimuth 194.3402 (eastward from north); tolerance +-0.001 deg for `spa`. Use whole minutes elsewhere: 30 s shifts hour angle by 0.125 deg.
- (ref) Blender Sun Position elevation/azimuth for the same Dubai inputs, tolerance +-0.3 deg; Helio same, +-0.3 deg.

**G6 patch geometry**: N = 145 / 577 / 2305; solid-angle sum 2*pi = 6.283185 (+-1e-9); Tregenza per-patch solid angle sr by row (0-12, 12-24, ..., 72-84): 0.043545, 0.041642, 0.047398, 0.040673, 0.042893, 0.044522, 0.045517; zenith cap 0.034420. Cosine-weighted horizontal sum with patch-centre altitudes, uniform L=1, divided by pi: 1.0055 / 1.0015 / 1.0004 (MF 1/2/4), +-1e-4; south-facing vertical wall: 0.4979 / 0.5002 / 0.5000, +-1e-4 (ideal 0.5).

**G7 uniform-sky analytic** (L = DHI/pi, no sun, rho_g 0): unoccluded horizontal point E = DHI within 1.0 % (MF1), 0.2 % (MF2), 0.1 % (MF4); vertical wall E = DHI/2 within 0.5 %. Horizontal point at the foot of an infinitely tall vertical wall (half the dome blocked, centre-ray visibility): E = DHI/2 with worst-case error over 120 wall azimuths 5.6 % (MF1), 3.3 % (MF2), 0.1 % (MF4); gate at 6 / 4 / 1 %. A plate 1 mm above the point covering the whole dome gives E = 0 exactly.

**G8 direct beam** (Dubai, DNI 800, DHI 0, ground 0): 21 Dec 12:00 (alt 41.19, az 174.84): horizontal 526.8, south wall 599.6 W/m2 (exact mode +-0.1 %; patch mode south wall 598.8 / 593.9 / 602.2 for MF 1/2/4, gate +-5 %). 21 Jun 12:00: horizontal 796.9 (conserved: patch = exact within 0.01 %). 1 Aug 15:00 (alt 53.4, az 266.4): horizontal 642.3, west wall normal -X exact 476.0, patch 494.5 / 481.8 / 471.3 (+3.9 / +1.2 / -1.0 %). 21 Jun 07:00 (alt 18.1, az 72): horizontal 248.5, east wall exact 723.2, patch 718.1 / 713.2 / 697.3 (-0.7 / -1.4 / -3.6 %).

**G9 ground and totals**: DNI = DHI = 0 with ground GHI term only, rho_g 0.2 and GHI 1000: vertical wall 100 W/m2 (+-1 %), horizontal 0. Total kWh equals sum(E_f * A_f) for a 1 m x 2 m flat quad at 50 mm grid (area 2 m2, 2000 mm sides). Synthetic one-year EPW (constant DNI 500, DHI 100 for 4380 sun hours): horizontal annual ~ sum of GHI within 1 % in patch mode.

**G10 EPW**: synthetic file with 8 header lines and 8760 rows x 35 fields round-trips; a row with DNI 9999 yields 0 and report count 1; hour 24 maps to hoy index 23 of that day; time zone and elevation parse exactly.

**G11 end-to-end** (report chain): design hour Dubai summer, DNI 900, DHI 100 -> horizontal black ASA panel: G = 900 sin(alt) + 100; with alt 85 G = 996.6 W/m2, Ts ~ 104.6 degC (Ta 45, still), verdict fail against ASA 100. Same panel white -> 61 degC, ok. Vertical wall at 12:00 on 21 Dec (DNI 800, DHI 100, rho_g 0.2): E about 599.6 + 50 + 0.2 * (526.8 + 100)/2 = ~712 W/m2 (patch mode +-5 %).

## 6. Sources (every URL consulted or cited)

Fetched this session [F]:
- https://docs.ladybug.tools/ladybug-primer/components/3_analyzegeometry/incident_radiation (inputs, outputs, limits of the incident radiation component)
- https://docs.ladybug.tools/ladybug-primer/components/2_visualizedata/cumulative_sky_matrix (sky matrix inputs, Tregenza vs Reinhart, ground_ref 0.2)
- https://www.ladybug.tools/ladybug-radiance/docs/ladybug_radiance.study.radiation.html (RadiationStudy arguments, units, exclusions; re-read 2026-10-22: radiation_values kWh/m2, irradiance_values W/m2, total_radiation(conversion_to_meters) kWh, offset_distance default 0)
- https://www.ladybug.tools/ladybug-radiance/docs/ladybug_radiance.skymatrix.html (SkyMatrix: per-patch kWh/m2, divide by hours for W/m2, ground_reflectance default 0.2, north convention, hoys, high_density; fetched 2026-10-22)
- https://www.ladybug.tools/ladybug/docs/ladybug.sunpath.html (Ladybug Sunpath docs; fetched but the page text is 590k characters and the first 100k held no algorithm detail, so nothing is taken from it; the library is copyleft, so its source is not read for this spec)
- https://www.radiance-online.org/learning/documentation/manual-pages/pdfs/gendaymtx.pdf (gendaymtx options, patch layout, Perez)
- https://raw.githubusercontent.com/LBNL-ETA/Radiance/master/src/cal/cal/reinhart.cal (patch geometry; read for the algorithm only, nothing copied)
- https://raw.githubusercontent.com/LBNL-ETA/Radiance/master/src/gen/gendaymtx.c (sky generation steps; summarised, nothing copied)
- https://voxcity.readthedocs.io/en/latest/_modules/voxcity/simulator/solar/sky.html (Tregenza band table; secondary, contradicts itself on Reinhart totals)
- https://bigladdersoftware.com/epx/docs/9-6/auxiliary-programs/energyplus-weather-file-epw-data-dictionary.html (EPW header and 35 fields)
- https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/outside-surface-heat-balance.html (outside face balance, view factors, convection correlations)
- https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/climate-calculations.html (sky temperature, sky emissivity correlations, cloud correction)
- https://gml.noaa.gov/grad/solcalc/solareqns.PDF (low-accuracy sun equations, sunrise/sunset)
- https://gml.noaa.gov/grad/solcalc/calcdetails.html (Meeus basis, refraction piecewise formula, accuracy note)
- https://extensions.blender.org/add-ons/helio/ (Helio features and requirements)
- https://docs.blender.org/manual/en/4.0/addons/lighting/sun_position.html (Sun Position modes)

Seen in search results only [S]:
- https://midcdmz.nrel.gov/spa (NREL Solar Position Algorithm, accuracy and year range)
- https://discourse.ladybug.tools/t/abnormal-sky-radiation-of-tregenza-sky-for-low-altitude-sun-positions/1778 (low-sun Tregenza anomaly)
- https://docs.ladybug.tools/ladybug-primer/components/3_analyzegeometry/real_time_incident_radiation (related component)

Papers recalled from memory [R], verify before implementing:
- Tregenza, P. R. (1987), Subdivision of the sky hemisphere for luminance measurements, Lighting Research and Technology 19(1) 13-14.
- Reinhart, C. F. and Walkenhorst, O. (2001), Validation of dynamic RADIANCE-based daylight simulation for a test office with external blinds, Energy and Buildings 33(7) 683-697.
- Perez, R. et al. (1990), Modeling daylight availability and irradiance components from direct and global irradiance, Solar Energy 44(5) 271-289; Perez, R., Seals, R., Michalsky, J. (1993), All-weather model for sky luminance distribution, Solar Energy 50(3) 235-245.
- Reda, I. and Andreas, A. (2004), Solar position algorithm for solar radiation applications, Solar Energy 76(5) 577-589 (NREL/TP-560-34302); Meeus, J. (1998), Astronomical Algorithms, 2nd ed.
- Kasten, F. and Young, A. T. (1989), Revised optical air mass tables, Applied Optics 28(22); Walton, G. N. (1983) and Clark, G. and Allen, C. (1978) via the EnergyPlus climate reference above; McAdams (1954) via the EnergyPlus convection reference above.

From the project report [rep] (not re-fetched): Abu Dhabi peak irradiance 1041 W/m2 (https://khazna.ku.ac.ae/en/publications/measurement-of-solar-energy-radiation-in-abu-dhabi-uae/); HDT figures (https://www.lesimprimantes3d.fr/dl/Bambu_Lab_Filament-guide.pdf, https://wiki.polymaker.com/polymaker-wiki-pt/produtos-polymaker/mais-sobre-nossos-produtos/documentos/fichas-tecnicas/abs-asa/polymaker-tm-asa, https://www.additive-x.com/shop/mpattachments/file/viewonline/id/916/product_id/2729); PAHT-CF annealing (https://qidi3d.com/blogs/print-lab/best-filament-car-interiors-abs-asa); CF-ABS failure (https://hackaday.com/2025/12/10/failed-3d-printed-part-brings-down-small-plane/); weathering panel temperature 70 degC (https://www.q-lab.com/weathering/weathering-test-standards/sae-j2527); flat-plate solar-load validation (https://simscale.com/docs/validation-cases/solar-radiation).

Unreachable on 2026-10-22 (DNS or HTTP 403), so the SPA reference case in G5 stays [R]: https://midcdmz.nrel.gov/spa/, https://www.nrel.gov/docs/fy08osti/34302.pdf, https://docs.nrel.gov/docs/fy08osti/34302.pdf, https://www.mikrocontroller.net/attachment/157160/NREL_SPA.pdf. Verify against the NREL report or its C test driver before freezing the `spa` golden.

Open questions: whether the reference sky-matrix patch value is exactly `L*Omega` (head-on irradiance) as assumed in 2.2, to be confirmed by recording a (ref) run; instant of the sun inside an EPW hour (the reference sunpath docs were not readable); whether the irradiance average includes night hours; Perez coefficient table (read from the paper); reference EPW for Dubai (a public typical-year file, to be chosen and licence-checked before use as a golden fixture); the exact default f_UV.

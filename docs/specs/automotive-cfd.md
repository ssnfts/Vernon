# Spec: Automotive CFD (virtual wind tunnel) - stage 5

Clean-room spec (see `CLEAN_ROOM.md`). Written from public docs, papers and open-source manuals only; wording is our own.
Reference-tool names below appear for documentation only and must never reach UI strings. `[S#]` = section 6.
Vernon drives OpenFOAM as a **separate process** (GPL-3.0); it writes input files and reads output files, never links or copies source.

## 1. Scope and reference tools

**Behaviour reproduced.** A hosted automotive wind-tunnel service and a general cloud CFD product (reference class: AirShaper, SimScale), re-built on the open stack the toolchain report recommends: OpenFOAM driven by a FreeCAD front end (CfdOF) [S1, S11].
Observable service behaviour (from the toolchain report, no private observation): upload body + wheels + optional radiators, choose speed, pick ground/wheel treatment, get Cd, Cl, CdA, ClA, surface-pressure maps and A/B comparisons; "regular" runs are ~10M cells with rotating wheels and radiators [S11].

**Reference versions.** CfdOF supports OpenFOAM Foundation 9-13 and OpenCFD 2006-2512, uses snappyHexMesh / cfMesh / Gmsh, simpleFoam, porous zones and baffles, AMI rotating regions, basic force/force-coefficient function objects, LGPL-3.0 [S1]. The dataset used as calibration ran OpenFOAM v11 [S4]. Vernon's writer takes a `flavour` enum (`foundation{9..13}`, `opencfd{2006..2512}`) because file names and keywords differ (see 3.7).

**In scope (v1).** Steady incompressible RANS (simpleFoam, k-omega SST), half-car with symmetry plane at zero yaw, moving ground, rotating wheels (wall velocity BC, optional MRF zones), porous radiator zones, force/pressure post-processing, mesh and iteration convergence gates, baseline-vs-kit deltas, density scaling, validation on DrivAer-class and Ahmed-body setups.
**Out of scope (v1).** DDES/SBES/LES, lattice-Boltzmann, yaw (needs full car), engine-bay and brake-duct flow without explicit internal geometry, thermal/CHT, sliding-mesh wheels, free-surface flow, AI surrogates (R2 only ~0.60-0.64 on varied designs, 19-25% surface-pressure error [S4, S11]).

**Hand-offs.** Upstream: watertight surface set from the surface stage (metres, x forward, z up). Downstream: CdA, ClA, axle balance, design-speed pressure load (FEA), force table at site density (performance stage).

## 2. Feature catalogue

| # | Feature | Observable behaviour | Inputs | Outputs |
|---|---|---|---|---|
| F1 | Air state | Density/viscosity from T, p, RH or altitude | T, p, RH | rho, mu, nu, q(U) |
| F2 | Reference geometry | Frontal area, length, wheelbase, axle x, wheel radius | surfaces | A_front, L, W, H, x_f, x_r, R_w |
| F3 | Half-car check | Confirms mirror symmetry before using a symmetry plane | surfaces | max mirror deviation, pass |
| F4 | Virtual tunnel | Domain box sized by blockage, inlet/outlet distances | ref geometry | domain, blockage %, BC set |
| F5 | Ground + wheels | Moving belt at U, wheels spinning at U/R, contact patch | wheel regions | BC set, omega |
| F6 | Porous radiator | Core as Darcy-Forchheimer zone from a dp(v) curve | core volume, curve | zone + d, f vectors |
| F7 | Refinement plan | Body, wheel, underbody, kit and wake boxes with levels | domain, kit bbox | level table |
| F8 | Meshing | background hex -> castellated -> snap -> prism layers | STL, plan | polyMesh, quality report |
| F9 | Solve | potentialFoam init then SIMPLE steady run, parallel | case | fields, residual + force histories |
| F10 | Coefficients | Cd, Cl, CdA, ClA, axle lift split, window mean +/- spread | histories | scalars + uncertainty |
| F11 | Surface maps | Cp per face, wall shear, y+ | fields | per-face scalar fields |
| F12 | Convergence | Iteration stationarity gate and 3-mesh GCI gate | histories, 3 runs | pass/fail, p, GCI |
| F13 | A/B delta | Kit vs baseline with identical recipe and noise floor | 2 results | dCdA, dClA, balance shift, verdict |
| F14 | Site scaling | Forces and power at chosen density/speed | CdA, ClA, rho, v | N, W, design pressure load |
| F15 | Validation | DrivAer/Ahmed template run with pass bands | benchmark geometry | error table |

### 2.1 Parameters (defaults, ranges, units, tolerances)

| Parameter | Default | Range | Unit | Basis |
|---|---|---|---|---|
| speed `U` | 30 | 10-100 | m/s | 30 m/s used by the calibration dataset [S4]; Mach 0.20 at 250 km/h so incompressible is fine |
| yaw | 0 | 0 (half) / +-10 (full car) | deg | half-car valid only at zero yaw [S11] |
| `rho_inf` | 1.225 | 0.9-1.3 | kg/m3 | ISA; Dubai 1.098-1.127 [S11] |
| `nu` | 1.5e-5 | 1.4e-5-1.9e-5 | m2/s | Sutherland-based, see 3.12 |
| inlet turbulence intensity `I` | 0.25 | 0.05-1 | % | [S5] |
| inlet viscosity ratio `nut/nu` | 10 | 1-100 | - | [S5] |
| blockage max | 1 | 0.5-2 | % | sim 0.9% in [S6]; wind tunnel 4-8% [S5, S6] |
| upstream / downstream | 3 L / 6 L | 2-5 / 5-10 | car lengths | inlet 3L, outlet 5L common [S5]; 48 m box for 4.6 m car [S6] |
| width:height of cross-section | 5:3 | 1-3 | - | 20 m x 12 m in [S6] |
| ground mode | moving | moving/stationary/slip | - | mandatory for splitters, skirts, diffusers [S11] |
| wheel mode | rotating wall | rotating/mrf/static | - | [S4, S10] |
| contact-patch sink `delta` | 2 | 0-5 | mm | practice, unsourced |
| base cell `dx0` | 0.32 | 0.1-1 | m | finest level-5 cell 10 mm; practice |
| body/wheel surface level | 4-5 (wheels 5) | 3-7 | level | level 5 used in [S4] |
| kit box level | body max + 1 | 4-8 | level | refinement boxes around part under test [S11] |
| `nCellsBetweenLevels` | 3 | 1-5 | - | [S2] |
| prism layers | 5 | 0-15 | count | 5 in [S4] |
| layer `expansionRatio` | 1.2 | 1.1-1.4 | - | practice, unsourced |
| layer `finalLayerThickness` | 0.5 | 0.3-1 | rel. to local cell | practice, unsourced |
| wall treatment | Spalding wall function | - | - | [S4]; avoid high-Re k-epsilon [S5] |
| max iterations | 3000 | 500-7000 | iter | 7000 in [S4] |
| averaging window | last 1000 | 100-2000 | iter | [S4] |
| residual target (RMS) | 1e-5 | 1e-3-1e-6 | - | [S4] |
| mesh-study refinement ratio `r` | 1.3 | 1.25-2 | linear | Celik et al. [S9]; cell count x r^3 = 2.2 |
| mesh acceptance | last two Cd within 2% | 1-2 | % | [S11]; plus GCI_fine <= 3% (Vernon rule) |
| y+ (body mean) | 30-300 warn outside | - | - | wall-function regime; Spalding tolerates 1-300 |
| min cells, Ahmed-class half body | 0.5M (high-Re), 1.5M (low-Re) | - | cells | [S5] |
| RAM rule of thumb | 1-2 GB per M cells | - | GB | report estimate [S11], not sourced |

Limits worth surfacing in the UI: steady RANS misses the Ahmed 25-degree drag crisis and underpredicts base-pressure drop, so squareback/diffuser/rear-wing-near-stall results are low confidence [S5]; downforce is less trustworthy than drag [S11]; absolute accuracy is 2-5% on drag at best (2.2-4.9% vs wind tunnel [S4, S11]) so only A/B deltas on a validated recipe are trusted.

## 3. Algorithms (published methods, implementer detail)

### 3.1 Coefficients and reference quantities
- Dynamic pressure `q = 0.5 rho U^2`. `Cd = F_x / (q A)`, `Cl = F_z / (q A)` (z up, so downforce => Cl < 0), `CdA = Cd*A`, `ClA = Cl*A` [m2]. Report CdA/ClA, never Cd alone (reference area is otherwise ambiguous) [S11].
- `A_front` = projected area on the plane normal to x of body + wheels + kit, from the **full** car. Compute by exact polygon union of triangle projections, or a raster at <= 2 mm pixel with +/-0.5% error.
- Half-car: forces integrate only the y>=0 half. Use `Aref = A_front/2` in forceCoeffs so that Cd equals the full-car value; full-car force = 2 x half force. Using the full `A` with half forces halves Cd (classic bug) - add a unit test.
- DrivAer-class reference: `L_ref = 4.6 m`, `A = 2.16 m2` [S6].

### 3.2 Domain and background mesh
1. `cross_section = A_front / blockage_max`; `width = sqrt(cross_section * aw)`, `height = cross_section / width`, aspect `aw = 5/3` [S6 ratio]. Half model: y in [0, width/2], symmetry patch at y=0.
2. x from `-up*L` to `+down*L` from the nose/tail extremes; z from 0 (ground) to `height`.
3. blockMesh: cells per axis `n = ceil(len/dx0)`, cubical cells (aspect ~1 near snapped surfaces; one edge must cut the surface) [S2]. Level-k cell = `dx0 / 2^k`.
4. Patches: `inlet` (fixed velocity), `outlet` (fixed pressure 0), `top` and `side` (symmetry/slip, or free-stream; outflow-permitting boundaries preferred to slip walls [S5]), `ground` (moving wall), `symmetry` (y=0).
5. Optional moving-ground start: ground slip upstream of `x_belt = -0.5 L` (practice, unsourced) to avoid a developing wall layer; stationary-plate experiments start the plate 2L ahead [S5].

### 3.3 Surface prep and snappyHexMesh
Order of operations in snappyHexMesh [S2]: refine at feature edges -> refine at surfaces (max level by `resolveFeatureAngle`) -> remove cells outside the `locationInMesh` region (cells kept when ~50% or more of their volume lies inside) -> refinement regions -> snap (project, relax, reduce displacement where quality fails) -> add layers (extrude, relax, retry thinner, drop layers if checks fail).
- STL written to `constant/triSurface` in ASCII with one named solid per part (`body`, `wheel_fl/fr/rl/rr`, `kit_*`, `underbody`); patch names are `<surface>_<region>`; layers are keyed by patch name, not surface region [S2].
- Wheels sunk `delta` below the ground plane so the ground trims a flat contact patch; do not leave a point contact (practice).
- `locationInMesh`: a point in the free stream, off any cell face (e.g. 0.5 dx0 jitter) [S2].
- Closed volumes can become zones (`cellZone` with inside selection) for porous cores and MRF cylinders; snappy then writes `cellZones`.
- Keywords and typical values from [S2]: `nCellsBetweenLevels 3`, `resolveFeatureAngle 30`, `maxLocalCells`/`maxGlobalCells` per memory limit, `snap{nSmoothPatch 3; tolerance 2.0; nSolveIter 30-100; nRelaxIter 5}`; layers `nSmoothSurfaceNormals 1; nSmoothNormals 3; nSmoothThickness 10; maxFaceThicknessRatio 0.5; maxThicknessToMedialRatio 0.3; minMedianAxisAngle 90; nBufferCellsNoExtrude 0; nLayerIter 50; nRelaxedIter 20`; give exactly two of `expansionRatio / finalLayerThickness / firstLayerThickness / thickness` [S2]. Unsourced typicals to verify in golden runs: `nFeatureSnapIter 10`, `featureAngle 130`, `nGrow 0`, `minThickness 0.1 (relative)`.
- Quality dictionary [S2]: `maxNonOrtho 65; maxBoundarySkewness 20; maxInternalSkewness 4; maxConcave 80; minVol 1e-13; minTetQuality off; minArea -1; minTwist 0.05; minDeterminant 0.001; minFaceWeight 0.05; minVolRatio 0.01; minTriangleTwist -1; nSmoothScale 4; errorReduction 0.75`; `relaxed{}` overrides after `nRelaxedIter`.
- Refinement design: body `(L_min L_max) = (4,5)`; wheels 5; underbody/diffuser/splitter strips 6 within 0.1 m of the surface (distance mode, nearest first); kit box +1 level around kit bbox expanded by 0.15 m; near-wake box (x from tail - 0.5 m to tail + 1.0 L, y 0..W, z 0..1.3 H) level 4; far-wake box to tail + 2 L level 3. Calibration dataset: castellated up to level 5 on car and wheels, 5 relaxation iterations, boxes adjusted per body category, 500-750k car-surface cells, ~12M cells per half model [S4].
- Layer budget: first-layer height from the y+ target with `y1 = y+ * nu / u_tau`, `u_tau = U sqrt(Cf/2)`, `Cf = 0.026 Re_L^(-1/7)` (flat-plate turbulent estimate; Schlichting). Use as a starting guess, then read the real y+ after the first run.
- Gate after meshing (`checkMesh`): 0 failed checks, max non-orthogonality < 70 (warn 65), layer coverage >= 70% of requested faces on body+wheels (warn below), cell count within [0.5, 1.2] x target, no cell with negative volume.

### 3.4 Boundary conditions
| Patch | U | p | k, omega | nut |
|---|---|---|---|---|
| inlet | fixedValue (U,0,0) | zeroGradient | fixedValue from 3.5 | calculated |
| outlet | inletOutlet (0) | fixedValue 0 | inletOutlet | calculated |
| top/side | slip or freestream | zeroGradient | zeroGradient | calculated |
| symmetry | symmetryPlane | symmetryPlane | symmetryPlane | symmetryPlane |
| ground (moving) | fixedValue (U,0,0) | zeroGradient | kqRWallFunction / omegaWallFunction | nutUSpalding |
| body, kit | noSlip | zeroGradient | same wall functions | nutUSpalding |
| wheel (rotating) | rotatingWallVelocity (origin = axle centre, axis, omega) | zeroGradient | same | nutUSpalding |
Wall functions evaluate against the wall velocity, so a moving ground and spinning wheels need no special turbulence BC.
- Wheel angular speed: `omega = U / R_dyn`. Sign: for axis (0,1,0) the tyre bottom must move toward +x, which needs `omega = -U/R` (omega x r with r = -R z gives -omega R x). Check against [S6]: 16 m/s with 51 rad/s implies R_dyn = 0.314 m.
- MRF alternative (`wheel_mode = mrf`): one cylinder cellZone per wheel, `omega` and axis as above, `nonRotatingPatches` empty. Wheel-rotation modelling changes lift and ventilation drag far more than drag, and sliding mesh is the only reliable option for lift, so mark lift from MRF/wall-velocity wheels as moderate confidence [S10].
- Expected sign of effects (directional golden tests): moving ground alone raises drag and front lift and cuts rear lift; rotating rear wheels cut drag and rear lift; both together reduce drag and rear lift with little change in front lift (saloon car, CFD and experiment agree) [S10]. A racing-car body can reverse the drag trend, so test this only on road-car geometry.

### 3.5 Turbulence model (k-omega SST) and inlet values
Menter SST (2003 form) in OpenFOAM: transport equations
`D(k)/Dt = min(P_k, 10 beta* k omega) - beta* k omega + div((nu + sigma_k nu_t) grad k)`,
`D(omega)/Dt = gamma S^2 - beta omega^2 + div((nu + sigma_w nu_t) grad omega) + 2 (1-F1) sigma_w2 (grad k . grad omega)/omega`,
`nu_t = a1 k / max(a1 omega, S F2)`. Blend every constant as `phi = F1 phi1 + (1-F1) phi2`.
`F1 = tanh(arg1^4)`, `arg1 = min(max(sqrt(k)/(beta* omega y), 500 nu/(y^2 omega)), 4 sigma_w2 k/(CD_kw+ y^2))`, `CD_kw+ = max(2 sigma_w2 grad k . grad omega / omega, 1e-10)`; `F2 = tanh(arg2^2)`, `arg2 = max(2 sqrt(k)/(beta* omega y), 500 nu/(y^2 omega))`; `y` = wall distance (`wallDist meshWave`).
Constants: `beta* 0.09; a1 0.31; kappa 0.41`; set 1: `sigma_k1 0.85, sigma_w1 0.5, beta1 0.075, gamma1 5/9`; set 2: `sigma_k2 1.0, sigma_w2 0.856, beta2 0.0828, gamma2 0.44` [S7]. (OpenFOAM stores alpha = 1/sigma.)
- Inlet: `k = 1.5 (I U)^2`, `omega = k / (nut/nu * nu)` with `I = 0.25%`, ratio 10 [S5]. U = 40, nu = 1.5e-5 gives `k = 0.015`, `omega = 100`.
- Wall: `nutUSpalding` (continuous across y+), `omegaWallFunction`, `kqRWallFunction` [S4].
- Avoid high-Re k-epsilon with wall functions for separated body flows [S5].

### 3.6 Porous radiator (Darcy-Forchheimer)
OpenFOAM's explicit porosity source adds `S = -(nu d + 0.5 |U| f) U` (kinematic form) in a cellZone, `d` and `f` being vectors in a local coordinate system [S8]. From a measured core curve `dp(v) = a v + b v^2` (v = face velocity, core thickness `t`, dynamic viscosity `mu`, density `rho`):
`d_flow = a / (mu t)` [1/m2], `f_flow = 2 b / (rho t)` [1/m].
In the two lateral directions use `d_lat = k_lat d_flow`, `f_lat = k_lat f_flow` with `k_lat = 1e3` (forces through-core flow; practice, tune and report). Example: `a = 20, b = 15, t = 0.05 m, mu = 1.8e-5, rho = 1.1` => `d = 2.222e7`, `f = 545.5`.
Set `porosity yes` in forceCoeffs so the porous drag is included [S3], or add the zone momentum loss manually; otherwise radiator drag is silently missing. Engine-bay air outside the core is not modelled unless internal passages exist in the surface set [S11]. No public source gives radiator curves: they are user input (table node), never hard-coded.

### 3.7 Case layout and run sequence
```
case/
  0/            U p k omega nut                     (+ 0.orig kept)
  constant/     transportProperties (nu) | physicalProperties (Foundation 10+)
                momentumTransport (Foundation 9+) | turbulenceProperties (OpenCFD)
                triSurface/*.stl   polyMesh/   fvModels|fvOptions   MRFProperties
  system/       controlDict  fvSchemes  fvSolution  blockMeshDict  snappyHexMeshDict
                surfaceFeaturesDict (surfaceFeatureExtractDict on old versions)
                meshQualityDict  decomposeParDict  topoSetDict / createPatchDict
```
Per-flavour file-name differences are version facts to confirm with a golden run on each supported flavour.
Run sequence: `surfaceFeatures` -> `blockMesh` -> `decomposePar` -> parallel `snappyHexMesh -overwrite` -> `reconstructParMesh` (or single-process for small cases) -> `checkMesh -allTopology -allGeometry` -> `potentialFoam` (initial field, as in [S4]) -> `decomposePar` -> `mpirun simpleFoam -parallel` -> `reconstructPar -latestTime` -> post-processing function objects (run live, written under `postProcessing/`).
Numerics (starting recipe, practice - verify): `steadyState`; `div(phi,U) bounded Gauss linearUpwind grad(U)`; `div(phi,k|omega) bounded Gauss limitedLinear 1` (upwind on first 500 iterations); `grad Gauss linear` with `cellLimited` on U; laplacian `Gauss linear corrected` (limited 0.5 if max non-orthogonality > 70); p by GAMG (`relTol 0.1`), U/k/omega smoothSolver; SIMPLE `consistent yes`, equation relaxation 0.9, `nNonOrthogonalCorrectors 0` (1 above 70). Calibration run: 7000 iterations, forces averaged over the last 1000, RMS residuals < 1e-5 [S4].

### 3.8 Force-coefficient function object
- `forceCoeffs` entries [S3]: `type forceCoeffs; libs ("libforces.so"); patches (body wheels kit...)` (never `ground`); `p p; U U; rho rhoInf; rhoInf <rho>` because the incompressible solver carries kinematic pressure; `pRef 0; porosity yes|no; CofR; liftDir (0 0 1); dragDir (1 0 0); pitchAxis (0 1 0); magUInf; lRef; Aref; binData{nBin; direction; cumulative}`. Text output has Cd, Cd(f), Cd(r), Cl, Cl(f), Cl(r), CmPitch, CmRoll, CmYaw, Cs, Cs(f), Cs(r), each with total/pressure/viscous/internal parts [S3]. Version caveat: older sources list `lRef`/`Aref` as required [S3]; always write them.
- The reader must discover columns from the `#` header line, not by index, and accept both `coefficient.dat` and `forceCoeffs.dat` file names (unsourced version difference).
- Vernon also runs the plain `forces` object (force + moment time series) and derives its own axle split, so results do not depend on how a given version defines `Cl(f)/Cl(r)`. With `CofR` at (x_c, 0, z_c), front axle x_f, rear x_r, total vertical force `Fz` and drag `Fx`, the solver's pitch moment is the y part of r x F, `My = sum (z_i - z_c) Fx_i - sum (x_i - x_c) Fz_i`, so it also holds the drag's moment. With the drag acting along a line at height `z_d` (default: mid-height of the vehicle, lowest point + height / 2; node parameter to override), the moment of the vertical forces alone is `My_v = My - (z_d - z_c) Fx`. Solve `Ff + Fr = Fz` and `Ff (x_f - x_c) + Fr (x_r - x_c) = -My_v`. (Example: Fx = 1096 N at 0.45 m above a ground-level CofR adds 493 N m; left in, it moves a 58.4 % front balance to about 80 %.) The solver's own `Cl(f)/Cl(r)` keep the drag moment about CofR. Aero balance front % = `Ff / Fz * 100` (if both are downforce; otherwise report axle forces only).
- Window statistics over the last `N_w` iterations: mean, standard deviation, linear drift (slope x N_w / mean), and the 5-95% band; report `Cd = mean +/- sd`.
- Surface Cp: `Cp = (p - p_inf)/q`; kinematic p => `Cp = p_kin / (0.5 U^2)` with `p_inf = 0` at the outlet. Write wall-shear and y+ with the `yPlus` and `wallShearStress` objects.

### 3.9 Convergence
Iteration gate (pass all): RMS initial residuals of p, U, k, omega below target; `|mean(window_2) - mean(window_1)| / mean < 0.2%` for two adjacent windows of `N_w`; drift < 0.1% per window; Cd sd < 0.5% of mean (else flag "unsteady - report spread"; steady RANS on separated rears oscillates).
Mesh gate (Celik et al., GCI) [S9]: three grids fine/medium/coarse with refinement ratio `r21, r32 >= 1.3`, same recipe, only `dx0` (and layer sizes) scaled. With `e21 = phi2 - phi1`, `e32 = phi3 - phi2`, `s = sign(e32/e21)`:
iterate `p = |ln|e32/e21| + q| / ln r21`, `q = ln((r21^p - s)/(r32^p - s))` until converged; `phi_ext = (r21^p phi1 - phi2)/(r21^p - 1)`; `e_a = |(phi1 - phi2)/phi1|`; `GCI_fine = 1.25 e_a / (r21^p - 1)`. `e32/e21 < 0` means oscillatory convergence: skip extrapolation and report the spread of the three values. Safety factor 1.25 for three grids [S9]. Acceptance (Vernon): `|Cd_fine - Cd_med| / Cd_fine < 2%` (toolchain rule [S11]) and `GCI_fine <= 3%`.

### 3.10 Protocol (the toolchain report's 7 steps mapped to graph checks) [S11]
1. Validate on DrivAer/Ahmed first (`cfd.benchmark_check`; the project cannot enable kit runs until the template is within band).
2. Half-car with symmetry plane, zero yaw only (`cfd.symmetry_check`, yaw param locked).
3. Moving ground and rotating-wall wheels default on; turning either off shows a "not representative" badge.
4. Prism layers sized for the wall treatment (`cfd.mesh_quality` y+ check).
5. Refinement boxes around the part under test (`cfd.refinement_plan`).
6. Three mesh levels, accept when last two within 1-2% of Cd (`cfd.mesh_study`).
7. Kit vs baseline on the same recipe and identical background/refinement settings, forces averaged over final iterations (`cfd.compare_ab`).

### 3.11 Validation geometries and known behaviour
- **DrivAer-class** (fastback, notchback, estate rear ends; TUM 1:2.5 model tested in a 4.8 x 2.4 x 1.8 m section, 8% blockage, moving belt up to 50 m/s, 188 pressure taps [S6]). Wind-tunnel Cd: fastback detailed underbody 0.275, estateback 0.319, notchback 0.277 [S6]. A scale-resolving LBM run gave 0.268 / 0.304 / 0.271 (2.55 / 4.70 / 2.24% off) [S6]. Steady k-omega SST in OpenFOAM v11 was 2.88% (fastback smooth), 2.18% (fastback detailed), 4.88% (notchback), 4.11% (estate) from experiment [S4]. Estate drag is higher than fastback/notchback (reported as 15-20% in a Monash DrivAer study [S10]); the tunnel numbers above give ~13%.
- **Ahmed body** (25 and 35 degree slants): half body, symmetry, ground plate starting 2L ahead, inlet >= 3L, outlet ~5L, minimum cross-section 2L x 1.35L (full body), experiment blockage ~4%, stilts contribute >= 10% of drag and must be modelled, slant pressure drag is ~55% of total at 25 degrees vs ~35% at 35 degrees, drag crisis near 25-30 degrees, nearly all RANS miss the 25-degree wake transition and underpredict base-pressure drop; k-omega SST and low-Re variants predict separation, high-Re k-epsilon does not; hybrid DDES/SBES recommended for separated flow [S5]. Mesh minimums for a half body: 0.5M cells (wall functions) or 1.5M (y+ ~ 1); double for a full body [S5].

### 3.12 Site-density scaling (Dubai)
- Moist air: `rho = p_d/(R_d T) + p_v/(R_v T)`, `R_d = 287.05, R_v = 461.5 J/(kg K)`, `p_v = RH p_sat(T)`, `p_sat = 610.94 exp(17.625 T_C/(T_C + 243.04))` Pa (Magnus form), `p_d = p - p_v`. Viscosity by Sutherland `mu = 1.458e-6 T^1.5/(T + 110.4)` Pa s.
- Coefficients at matched Reynolds number are density independent, so forces scale by `rho/rho_ref` after the run (ISA to site factor 0.906 at rho = 1.11) [S11]. Run the solver at the real nu when Re matters; Cd is weakly Re-dependent in this range.
- Relations: `D = q CdA`, `F_down = q |ClA|`, `P_aero = 0.5 rho CdA v^3`, `Cp = (p - p_inf)/q`; structural design pressure scales with v^2 (856 N at 200 km/h becomes ~1,340 N at 250 km/h) [S11].

## 4. Proposed Vernon nodes

Ports: Surf = tagged surface set, Mesh, Case = OpenFOAM case dir, Series = time-series table, Scalar, Table. `now` = deterministic, fully specified, implement immediately. `wb-first` = build a small in-house white-box reference (analytic or reduced) plus golden fixtures before wrapping the external solver.
`NodeTypeInfo.emulates` (docs-only): `automotive wind-tunnel service`, `cloud CFD service`, `CFD GUI front end`.

| id | Label | Inputs | Outputs | Key params | Phase |
|---|---|---|---|---|---|
| cfd.air_state | Air State | - | rho, mu, nu, q | T_c, p, RH, U | now |
| cfd.frontal_area | Frontal Area | Surf | A_front, silhouette | dir, method, pixel_mm | now |
| cfd.symmetry_check | Mirror Symmetry Check | Surf | max_dev_mm, pass | tol_mm=1, plane | now |
| cfd.vehicle_prep | Prepare Vehicle | Surf (body, wheels, kit) | tagged Surf, ref geometry | sink_mm=2, axle detect | now |
| cfd.tunnel_domain | Virtual Tunnel | ref geometry | domain, BC set | blockage=1%, up=3L, down=6L, aspect=5/3 | now |
| cfd.ground_wheels | Ground and Wheels | tunnel, wheels | BC set, omega | ground_mode, wheel_mode, R_dyn, belt_x | now |
| cfd.porous_core | Porous Core | core Surf, dp(v) Table | zone, d, f | thickness, flow_dir, k_lat=1e3 | now |
| cfd.refinement_plan | Refinement Plan | tunnel, Surf, kit bbox | level table, boxes | dx0, body/wheel/kit levels, wake_L | now |
| cfd.mesh_setup | Mesh Dictionaries | plan, Surf | blockMesh/snappy/features dicts | layers, quality set, flavour | now |
| cfd.mesh_build | Build Volume Mesh | dictionaries, Case | Mesh, build log | np, max_cells | wb-first |
| cfd.mesh_quality | Mesh and Wall Check | Mesh | report, pass | maxNonOrtho, layer_cov, y+ band | now |
| cfd.case_setup | Solver Case | Mesh, BCs, air | Case | model=SST, I, nut/nu, schemes, flavour | now |
| cfd.solve_steady | Run Steady Solver | Case | fields, Series | iters, np, residual target | wb-first |
| cfd.force_coeffs | Force Coefficients | Series, ref geometry | Cd, Cl, CdA, ClA, balance +/- sd | window, half_model, CofR | now |
| cfd.run_convergence | Run Convergence | Series | pass, drift, sd | window, tol=0.2% | now |
| cfd.mesh_study | Mesh Convergence | 3 x force result | p, phi_ext, GCI, pass | r, accept_pct=2 | now |
| cfd.surface_pressure | Surface Pressure Map | fields | per-face Cp, shear, y+ | p_inf, colour range | wb-first |
| cfd.compare_ab | Baseline vs Kit | 2 x force result (recipe-stamped), optional benchmark report | dCdA, dClA, balance shift, verdict; rejects mesh/ground/wheel/yaw/turbulence mismatch | k_sigma=2, mesh_noise_cd=0.005 (or the mesh-study GCI) | now |
| cfd.aero_loads | Site Aero Loads | CdA, ClA, fields | N, W, design-speed pressure load | rho, v list, load factor | now |
| cfd.benchmark_check | Benchmark Check | benchmark result | error table, pass | case id, band | wb-first |

**Total proposed nodes: 20.** Sequence: vehicle_prep -> frontal_area / symmetry_check -> tunnel_domain -> ground_wheels / porous_core -> refinement_plan -> mesh_setup -> mesh_build -> mesh_quality -> case_setup -> solve_steady -> force_coeffs + run_convergence -> mesh_study (x3) -> compare_ab -> aero_loads -> FEA / performance stages. `benchmark_check` gates the first kit run.

## 5. Golden test ideas (concrete numbers)

| # | Node | Input | Expected | Tol |
|---|---|---|---|---|
| G1 | air_state | 40 C dry, 101325 Pa | rho 1.127 kg/m3 (-8.0% vs ISA 1.225) | +/-0.001 |
| G2 | air_state | 45 C, RH 30%, 101325 Pa | rho 1.098 (-10.4%); mu 1.93e-5; nu 1.76e-5 | +/-0.002; +/-1% |
| G3 | aero_loads | rho 1.11, 200 km/h, CdA 0.64, ClA -0.5 | q 1713 Pa; D 1096 N; P_aero 60.9 kW; downforce 856 N | +/-0.2% |
| G4 | aero_loads | same, 250 km/h | q 2677 Pa; downforce ~1338 N (report rounds to 1340) | +/-0.5% |
| G5 | aero_loads | ISA run scaled to rho 1.11 | factor 0.906 | +/-0.001 |
| G6 | tunnel_domain | A = 2.16, blockage 1%, 5:3 | cross-section 216 m2; width 18.97 m (half 9.49); height 11.38 m | +/-0.01 m |
| G7 | mesh_setup | domain 46 x 9.49 x 11.38, dx0 0.32 | blockMesh `(144 30 36)`; level-5 cell 0.01 m | exact |
| G8 | case_setup | U 40, I 0.25%, ratio 10, nu 1.5e-5 | k 0.015 m2/s2; omega 100 1/s | +/-1e-6 rel |
| G9 | refinement_plan | U 30, L 4.6, nu 1.5e-5 | Re 9.2e6; Cf 0.00263; u_tau 1.088 m/s; y1(y+=1) 13.8 um; y+=30: 0.414 mm; y+=100: 1.38 mm | +/-1% |
| G10 | ground_wheels | U 16, R_dyn 0.3137 | omega magnitude 51.0 rad/s; sign negative for axis (0 1 0) | +/-0.1 |
| G11 | porous_core | a 20, b 15, t 0.05, mu 1.8e-5, rho 1.1 | d 2.222e7 1/m2; f 545.5 1/m; lateral x1000 | +/-0.1% |
| G12 | mesh_study | phi = 6.063, 5.972, 5.863; r21 1.5, r32 1.333 | p 1.53; phi_ext 6.168; e_a 1.50%; GCI_fine 2.2% (worked example recalled from the paper; the algorithm in 3.9 reproduces it) | +/-0.02 on p |
| G13 | mesh_study | Cd = 0.2825, 0.2862, 0.2925; r 1.3 | p 2.03; Cd_ext 0.2772; e_a 1.31%; GCI_fine 2.33% -> passes 2% rule, passes GCI <= 3% | +/-0.01 on p |
| G14 | mesh_study | Cd 0.2810 vs 0.2780 (fine, med) and 0.295 vs 0.282 | 1.07% pass; 4.41% fail | exact |
| G15 | mesh_study | Cd = 0.2840, 0.2810, 0.2860 | e32/e21 < 0 -> oscillatory flag, no extrapolation | exact |
| G16 | force_coeffs | Fz -856.5 N, x_f 1.35, x_r -1.35, CofR 0, My 193.7 N m | Ff -500 N, Fr -356.5 N; balance front 58.4% | +/-0.2 N |
| G17 | force_coeffs | Fz -856.5 N, My 0, symmetric axles | Ff = Fr = -428.2 N | +/-0.1 N |
| G18 | force_coeffs | half-model force 548 N at q 1713, Aref_full 2.0 | Aref half = 1.0; Cd 0.320; using full Aref gives 0.160 (must be rejected) | +/-0.001 |
| G19 | compare_ab | base CdA 0.640 +/- 0.003, kit 0.660 +/- 0.003 | dCdA +0.020 (+3.1%), "worse" at k=2 | exact |
| G20 | compare_ab | base 0.640, kit 0.660, noise +/-0.012 | "inconclusive" (|d| < 2 x combined sd) | exact |
| G21 | surface_pressure | dp -91 and -103 Pa, 100 km/h, rho 1.11 | q 428.2 Pa; Cp -0.212 and -0.241 (puck check) | +/-0.001 |
| G22 | frontal_area | sphere r 1 m; box 4.5 x 1.8 x 1.4 m | pi m2 (+/-0.5%); 2.52 m2 (exact) | as stated |
| G23 | symmetry_check | body mirrored with 0.3 mm vs 5 mm shift | pass at tol 1 mm; fail for 5 mm | exact |
| G24 | benchmark_check | DrivAer fastback detailed, estate, notchback (user-supplied geometry, U 30) | Cd within 5% of 0.275 / 0.319 / 0.277; ordering estate > notchback ~ fastback | 5% (dataset spans 2.2-4.9%) |
| G25 | benchmark_check | Ahmed 25 vs 35 degree | Cd(25) > Cd(35) experimentally; RANS should show slant pressure share 55% vs 35% +/-10 pts; assert and record the known under-prediction, do not tighten | soft |
| G26 | ground_wheels (end-to-end A/B) | road-car geometry, moving+rotating vs static | dCd < 0, rear lift lower, front lift change small (directional) | sign only |
| G27 | mesh_quality | recorded poor mesh (max non-orthogonality 78) | warn at 65, fail at 70, y+ outside 30-300 flagged | exact |
| G28 | case writers | tiny box-in-tunnel case, each flavour | files generated exactly as snapshot; `checkMesh` 0 failures; simpleFoam residual < 1e-4 after 200 iters; analytic: empty-tunnel with moving ground has uniform U | file snapshot |

Cost/size notes for fixtures: a 12M-cell half DrivAer run is ~375 CPU-hours per case (derived from 3e6 CPU-hours / 8000 designs [S4]; estimate, not reported), so keep CI fixtures to recorded outputs and a sub-100k-cell box case.

## 6. Sources (all consulted URLs)

- S1 CfdOF README (supported versions, meshers, solvers, licence): https://github.com/jaheyns/CfdOF
- S2 snappyHexMesh user guide, OpenFOAM v9 (dictionary structure, order, typical values): https://doc.cfd.direct/openfoam/user-guide-v9/snappyhexmesh
- S3 forceCoeffs function object: https://doc.openfoam.com/2212/tools/post-processing/function-objects/forces/forceCoeffs ; header listings https://api.openfoam.com/2406/forceCoeffs_8H_source.html , https://cpp.openfoam.org/v6/forceCoeffs_8H_source.html
- S4 DrivAerNet++ (OpenFOAM v11, SST, 12M/24M cells, 7000 iterations, validation errors, cost): https://arxiv.org/pdf/2406.09624 and https://arxiv.org/html/2406.09624
- S5 ERCOFTAC Ahmed-body best practice advice AC1-05: https://www.kbwiki.ercoftac.org/w/index.php/Best_Practice_Advice_AC1-05
- S6 DrivAer validation case (reference length/area, tunnel, Cd table, domain, wheel speed): https://www.simscale.com/docs/validation-cases/validation-case-drivaer-model/ ; experimental dataset record https://www.nwtf.ac.uk/dataset/drivaer-experimental-aerodynamic-dataset/
- S7 Turbulence Modeling Resource (SST page home) https://tmbwg.github.io/turbmodels/ ; original model https://ntrs.nasa.gov/citations/19930013620 ; OpenFOAM class reference https://cpp.openfoam.org/v11/classFoam_1_1kOmegaSST.html ; Menter, Kuntz, Langtry (2003) "Ten years of industrial experience with the SST turbulence model" (constants cross-checked via search summaries, https://2021.help.altair.com/2021.2/hwsolvers/acusolve/topics/acusolve/training_manual/menter_shear_stress_transport_sst_model_r.htm)
- S8 explicitPorositySource class: https://cpp.openfoam.org/v9/classFoam_1_1fv_1_1explicitPorositySource.html ; Darcy-Forchheimer wiki page https://openfoamwiki.net/index.php/DarcyForchheimer
- S9 Celik, Ghia, Roache, Freitas, Coleman, Raad (2008), J. Fluids Eng. 130(7), grid-convergence procedure; secondary descriptions https://volupe.com/support/grid-convergence-index-calculator/ and https://pypi.org/project/convergence/0.6.3 (formulas re-derived and checked against the paper's worked example, G12)
- S10 Ground/wheel simulation studies: https://web-test.research.chalmers.se/publication/196986 (saloon car moving ground + rotating wheels), https://research.chalmers.se/publication/513305 , https://saemobilus.sae.org/content/2018-01-0728 (wheel-rotation methods on DrivAer), https://saemobilus.sae.org/content/2008-01-0659 (racing-car contrast), https://flair.monash.edu/publications/pdfs/effect_of_moving_ground.pdf , https://flair.monash.edu/publications/pdfs/AvThShBu_jweia_2018.pdf (DrivAer estate vs fastback/notchback drag)
- S11 Local toolchain report: `C:\Users\mabdu\REA\3D printed body kit toolchain.md`, which cites AirShaper pricing https://airshaper.com/pricing , SimScale Community https://www.simscale.com/?p=111634 , SimScale GPU LBM https://www.digitalengineering247.com/article/simscale-releases-a-gpu-based-lattice-boltzmann-solver/cfd , Chalmers https://research.chalmers.se/publication/513305 , pressure puck https://ecomodder.com/forum/639288-post1.html , coastdown https://cleanfleetreport.com/?p=33672 , lap sim https://www.optimumg.com/software/optimumlap/
- Other pages seen in search results only (not relied on for numbers): https://arxiv.org/pdf/2603.27565 (OpenFOAM HPC challenge, steady SST, static wheels); https://doc.cfd.direct/openfoam/user-guide-v8 (user guide index).

# Spec: static structural FEA for printed body-kit parts (reference: CalculiX solver decks, static stress studies in commercial CAD/FEA, orthotropic FDM properties)

Status: draft v0.1, 2026-10-22. Clean-room spec: public manuals, papers and open-source docs only (see CLEAN_ROOM.md).
Vernon stage served: 6 structural FEA. It consumes the aero loads (`cfd.aero_loads`, `cfd.surface_pressure`), the print orientation and directional allowables (`print.directional_strength`, `split.*`), and the surface temperatures and heat table (`solar.surface_balance`, `solar.material_table`). It emits displacement, stress, reaction forces and a safety factor at the mounts that the print and performance stages check.
Marker key: **(doc)** stated by a manual/help page, **(inf)** inferred from wording, **(V)** a Vernon design choice, **(calc)** number computed by the spec author with the stated equations (re-derive before freezing), **(rep)** value from the project report `3D printed body kit toolchain.md` (not re-fetched).
Source tags (section 6): [F] fetched this session, [S] seen only in search results, [R] recalled from memory, verify before relying on it, [X] fetch attempted and failed.

## 1. Scope and reference tools

| Reference | Role in the pipeline | Version / source |
|---|---|---|
| CalculiX CrunchiX `ccx` (GPL solver, input deck = Abaqus-style keyword cards) | the solver Vernon drives as an external process (never linked, never copied: licence is GPL, Vernon is Apache-2.0) | user manual dated 2023-07 (release 2.21) [F] |
| CalculiX GraphiX `cgx` manual, chapter "Result Format" | exact layout of the `.frd` results file | same release [F] |
| FreeCAD FEM workbench (front end that writes ccx decks, orthotropic materials possible per report) | model of the user workflow: analysis container, constraints, material, mesh, solver, result filters | wiki page blocked at fetch time [X]; report says orthotropic works through ccx (rep) |
| Fusion Simulation static stress study; Ansys-class stress tools | the behaviour being reproduced: loads + constraints + material -> displacement, stress, reaction, safety factor | Autodesk help page [F]; rolling docs, no version |
| Tsai-Wu failure criterion (Tsai and Wu 1971) | strength ratio for direction-dependent allowables | Wikipedia summary [F]; paper [R] |
| fTetWild (MPL-2.0) and Gmsh | tetrahedral meshing of a closed triangle surface | repo README [F]; arXiv abstract [F]; Gmsh manual (partial) [F] |
| DMA study of printed PLA/PETG/ASA/ABS (Struz et al., Polymers 2024) | temperature dependence of stiffness for the derating tables | PMC article [F] |

Out of scope for v1: contact and bolt pre-load (ccx supports `*CONTACT PAIR`, `*PRE-TENSION SECTION`, doc), buckling (`*BUCKLE`), vibration (`*FREQUENCY`), creep, fatigue, warpage, print-process simulation, nonlinear material. Each is listed in section 4 only as a future node. Large-deflection (`NLGEOM`) is off: a printed part at SF 3 stays linear (V), and the manual itself advises running the linear model first (doc).
Unit system (V, matches the manual's own examples, doc): **mm, N, tonne, s, MPa (= N/mm2)**. Density in tonne/mm3 (the manual's steel example is 7.8E-9, doc; PLA 1.24 g/cm3 = 1.24E-9). Gravity 9806.65 mm/s2 (standard gravity, the same constant the performance stage uses). Pressure: 1 Pa = 1E-6 MPa. Temperature degC in the UI and in the deck (the deck only needs consistency with the temperature column of the material cards). The solver is unitless, so the writer converts and a unit-consistency test (G1) guards it.

## 2. Feature catalogue (observable behaviour)

### 2.1 What a static study does (reference: static stress studies)
Inputs (doc, [F] Autodesk help + manual): a solid body; a material (stiffness, density, yield/ultimate strength); constraints (fixed faces/edges/points); loads (pressure on a face, force, gravity/acceleration); a mesh. Outputs (doc): displacement (total, x, y, z); stress (von Mises, 1st and 3rd principal, normal xx/yy/zz, shear xy/yz/zx); strain (equivalent, principal, components); reactions (total and per axis force and moment at constrained entities); contact pressure/force; **safety factor, the default result**, defined as yield strength divided by von Mises stress, below 1 means likely failure (doc); the settings let the user pick yield or ultimate as the threshold [S]. Whether results are per node or per element, legend behaviour and the mesh-refinement controls are not stated on the fetched page.
Vernon adds what the report requires and the references leave to the user: direction-dependent allowables, temperature-dependent stiffness and strength, a mesh-convergence verdict, a hand-calculation cross-check, and an equilibrium check.

### 2.2 Solver deck anatomy (reference: ccx input cards, all (doc) [F] unless noted)
| Card | Fields / behaviour | Notes for Vernon |
|---|---|---|
| `*HEADING`, `*NODE[,NSET=]` | `id, x, y, z` rows | numbering from 1; ids unique |
| `*ELEMENT,TYPE=C3D10,ELSET=` | `id, n1..n10` (max 16 entries per line, continuation lines) | C3D4 is "too stiff" for structural use, C3D10 is "a good general purpose element" with 4 integration points (doc); only C3D10 is written in v1 |
| `*NSET`, `*ELSET`, `*SURFACE,NAME=,TYPE=ELEMENT` | face labels `S1..S4` per tetra: face1 = 1-2-3, face2 = 1-4-2, face3 = 2-4-3, face4 = 3-4-1 | same numbering for `*DLOAD` labels `P1..P4` |
| `*MATERIAL,NAME=` + `*ELASTIC[,TYPE=]` | ISO: `E, nu, T`. ENGINEERING CONSTANTS: line 1 `E1,E2,E3,nu12,nu13,nu23,G12,G13`, line 2 `G23,T`. ORTHO: line 1 `D1111,D1122,D2222,D1133,D2233,D3333,D1212,D1313`, line 2 `D2323,T`. Repeat the line (pair) per temperature for temperature dependence; a single temperature is valid for all temperatures | a header row per temperature is how derating is passed |
| `*DENSITY` | `rho, T` | required for `GRAV` loads and mass output |
| `*ORIENTATION,NAME=` | rectangular: line 2 = `ax,ay,az,bx,by,bz` (a on local X', b in the X'-Y' plane, right-handed); optional line 3 = `axis, angle_deg` extra rotation | one orientation per print orientation (per part) |
| `*SOLID SECTION,ELSET=,MATERIAL=,ORIENTATION=` | assigns material and local axes; the orientation only matters for non-isotropic materials | |
| `*BOUNDARY` | `node-or-set, first_dof, last_dof[, value]`; dofs 1-3 translations; homogeneous ones go before `*STEP` | |
| `*COUPLING,REF NODE=,SURFACE=,CONSTRAINT NAME=` + `*KINEMATIC` (rigid) or `*DISTRIBUTING` | face-based surface; the ref node must not be on the surface; `RF` printing does not work on a kinematic ref node, use `*SECTION PRINT` with key `SOF` | rigid washer patch at a bolt |
| `*STEP` / `*STATIC[,SOLVER=]` / `*END STEP` | solver choices SPOOLES, PARDISO, PaStiX, TAUCS, iterative (scaling or Cholesky); default = first installed; iterative good for truly 3D bodies, poor on plates/shells; SPOOLES has no out-of-core, about 1,000,000 equations in 32 GB | thin panels: prefer a direct solver |
| `*CLOAD` | `node, dof, value`; added within a step | consistent nodal forces |
| `*DLOAD` | `elset, Pn, p` (positive pressure pushes into the body, negative is suction/tension); `GRAV, magnitude, nx, ny, nz`; label `CENTRIF` | uniform pressure per face only; non-uniform needs a user subroutine, so Vernon bins faces or uses `*CLOAD` |
| `*DSLOAD` | `surface-name, P, value` (pressure on a named face surface) | alternative to per-element `*DLOAD` |
| `*TEMPERATURE` | step-level node temperatures in a purely mechanical step; they select temperature-dependent constants; thermal strain only appears if `*EXPANSION` is given (inf: omit it so no thermal stress arises) | per-node degC from `fea.temperature_map` |
| `*NODE FILE` / `*EL FILE` | keys: `U` (DISP), `RF` (FORC, sum of reaction + `*CLOAD` + `*DLOAD` at the node), `S` (STRESS, element values extrapolated to nodes and averaged), `E` (TOSTRAIN), `ME`, `ENER`, `ERR`/`ZZS` estimators; parameter `GLOBAL=NO` gives stresses in the local material system; `S` also switches on `ERR` unless `NOE` follows | writes `.frd` |
| `*NODE PRINT,NSET=` / `*EL PRINT,ELSET=` | text to `.dat`; `U`, `RF`, `S`, `EVOL`, `ELSE`, `EMAS`; parameter `TOTALS=YES/ONLY` prints sums | element `S` here is at integration points (not smeared) |

### 2.3 Parameters of the Vernon structural run
| Parameter | Type / range | Default | Behaviour |
|---|---|---|---|
| element_type | C3D10 | C3D10 | quadratic tetra; C3D4 refused unless `allow_linear_tet` (V) |
| edge_len_mm | > 0 | min(thickness/2, 8) (V) | target edge length; at least 2 elements through a wall, 3 where bending governs (V, tested by G3) |
| sf_target | 1..10 | 3.0 | report recommends SF 3-4 as engineering judgement, no standard exists for FDM exterior aero (rep) |
| criterion | von_mises / max_principal / tsai_wu / interlayer | tsai_wu | see 3.7; Fusion-like default is `von_mises` on yield (doc) |
| design_speed_kmh | 30..400 | 200 | q = 1/2 rho v^2 (rep) |
| air_density | 0.9..1.3 kg/m3 | 1.11 | Dubai worst case (rep) |
| bump_g | 0..10 | 4 | the report's 3-5 g bump rule of thumb is unsourced engineering judgement (rep); 4 is the middle (V) |
| temperature_mode | uniform / per_face_map | uniform at 23 degC | per-face needs a thermal result |
| mount_model | fixed_patch / rigid_washer / spring_pad | rigid_washer | see 3.5 |
| avg_radius_mm | 0..30 | bolt diameter | hot-spot averaging radius for SF at a mount |
| solver | spooles / pardiso / iterative_cholesky | auto | external executable path stored in app settings |
| unit_check | bool | true | G1 on every run |

### 2.4 FDM material data (rep unless marked)
Anisotropy is the dominant effect: upright (build direction Z) strength is about **55 % of flat for PLA, 46 % for PETG, 29 % for ASA** (CNC Kitchen hook tests, rep); PPA-CF tensile **Z 57 MPa vs XY 168 MPa = 0.339** (calc 0.3393). HDT at 0.45 MPa: PLA 57, PETG 69-72, ASA 100-103, PET-CF 205, PAHT-CF 194 annealed / **84.8 unannealed (190.7 annealed)**, PPA-CF 227 (196 at 1.8 MPa) degC (rep). Moisture matters (PA6-CF absorbs 2.35 %, PET-CF 0.37 %, rep). Absolute moduli and strengths are not in the report: they come from the user's datasheet or, better, coupon tests of the actual spool (rep: "test your actual spool"). Literature says printed PLA is orthotropic and the upright modulus is the lowest, interlayer anisotropy exceeding intra-layer anisotropy [S: Li, Xu, Fang 2024; a 2019 transversely-isotropic study]; numeric tables were not retrievable [X], so Vernon ships ratios only and requires absolute inputs.
Stiffness versus temperature (3-point-bend DMA, 3 Hz, 2 K/min, 100 % rectilinear fill at 45 deg alternating, 0.3 mm layers, [F] Struz 2024; units not stated, ratios are unit-free): storage modulus E' at 20 -> 40 degC: PLA 1980 -> 1820, PETG 1100 -> 883, ASA 1210 -> 976, ABS 1030 -> 875 (inf: first value at 20 degC). Glass transition (tan-delta peak / E' onset): PLA 64.3 / 59.1, PETG 87.2 / 80.7, ASA 116 / 105.7, ABS >=115 / 110.8 degC. Onset of the modulus fall: PLA ~57, PETG ~80, ASA ~50 (faster than ABS), ABS ~35 (then stable to ~110) degC. These DMA moduli sit well under datasheet tensile moduli, so use them for **retention ratios only**, never as absolute E.

## 3. Algorithms (implementer level)

### 3.1 Deck writer and unit gate
Write sections in this order: `*HEADING`, `*NODE`, `*ELEMENT`, sets, `*ORIENTATION`, `*MATERIAL`, `*SOLID SECTION`, `*SURFACE`, constraints, `*STEP` ... `*END STEP`. Minimal deck (card shapes from the manual, (doc); element and node rows abbreviated, not a runnable model):
```
*HEADING
vernon run, units mm N tonne s MPa
*NODE,NSET=NALL
1,0.,0.,0.
*ELEMENT,TYPE=C3D10,ELSET=EALL
1,n1,n2,n3,n4,n5,n6,n7,n8,n9,n10
*NSET,NSET=MOUNT1
12,13,14
*SURFACE,NAME=SAERO
38,S1
*ORIENTATION,NAME=OR1
1.,0.,0.,0.,1.,0.
*MATERIAL,NAME=PRINT
*ELASTIC,TYPE=ENGINEERING CONSTANTS
2500.,2500.,2000.,.35,.30,.30,925.926,800.,
800.,23.
*DENSITY
1.24E-9,
*SOLID SECTION,ELSET=EALL,MATERIAL=PRINT,ORIENTATION=OR1
*BOUNDARY
MOUNT1,1,3
*STEP
*STATIC
*DSLOAD
SAERO,P,0.001713
*NODE FILE
U,RF
*EL FILE
S,E,NOE
*NODE PRINT,NSET=MOUNT1,TOTALS=ONLY
RF
*EL PRINT,ELSET=EALL,TOTALS=ONLY
EVOL
*END STEP
```
C3D10 node order: corners 1-4, then mid-edge nodes 5..10 on edges 1-2, 2-3, 3-1, 1-4, 2-4, 3-4 [R, Abaqus convention; the manual's figure is an image, verify against a one-element run]. Gmsh numbers its second-order tetra differently in the last two mid-edge nodes [R]: convert and test with G4. Elements must have positive volume (det[x2-x1, x3-x1, x4-x1] > 0); swap nodes 2 and 3 (and their mid-nodes) otherwise.
Second-order from a linear tet mesh: one new node per unique edge at its midpoint (straight sided), dedupe edges by sorted node pair. Keep a map from deck ids to Vernon vertex ids for result read-back.

### 3.2 Printed material -> constants
Axes (V): 1 = first in-plane (raster) axis, 2 = in-plane transverse, **3 = layer-stacking (build) direction**. Transversely isotropic default: E1 = E2 = E_xy, E3 = E_z, nu12 = nu_xy, nu13 = nu23 = nu_xz, G12 = E_xy / (2(1+nu12)), G13 = G23 = G_xz. Minor ratios nu21 = nu12 E2/E1, nu31 = nu13 E3/E1, nu32 = nu23 E3/E2 (reciprocity). Stability gate: all E, G > 0; nu_ij^2 < E_i/E_j; Delta = 1 - nu12 nu21 - nu23 nu32 - nu31 nu13 - 2 nu21 nu32 nu13 > 0 [R]. For `TYPE=ORTHO` output, D from inverting the 6x6 compliance, or in closed form: D1111 = E1(1 - nu23 nu32)/Delta, D1122 = E1(nu21 + nu31 nu23)/Delta, D2222 = E2(1 - nu13 nu31)/Delta, D1133 = E1(nu31 + nu21 nu32)/Delta, D2233 = E2(nu32 + nu12 nu31)/Delta, D3333 = E3(1 - nu12 nu21)/Delta, D1212 = G12, D1313 = G13, D2323 = G23 (calc, matches numeric inversion in G2). The manual does not define the sign convention of nu12; assume nu12 = -eps2/eps1 under sigma1 (inf, the usual one) and prove it with G2/G5 on the first solver run. `engineering_constants` is the default card type (fewer chances to mis-order D terms).
Conservative alternative (rep: "isotropic properties knocked down to the Z values"): `analysis_mode = isotropic_z` uses E_z and the Z strength everywhere. Offer it as a one-click bound.
Part orientation: for each printed part with rotation R (model -> printer frame), material axis 1 = R^T e_x, 2 = R^T e_y, 3 = R^T e_z, written as `a = o + axis1`, `b = o + axis2` in `*ORIENTATION` (o any point, e.g. the part centroid, unit-length offsets). The build direction is global to the part and does **not** follow a curved panel's surface normal.

### 3.3 Temperature dependence (white-box, (V) with stated anchors)
Retention r(T) = E(T)/E(23 degC), applied to E, G and strength alike (strength falls at least as fast as stiffness: conservative assumption (V)). Preference order for r(T): (1) user DMA curve (CSV of T, E'), normalised at 23 degC; (2) DMA-anchored piecewise-linear: (23, 1), (40, r40 from section 2.4), (T_onset, r40 continued), (T_g_onset, 0.2), (T_g_onset + 20, 0.05) (V, placeholders until a curve is supplied; flag "uncalibrated"); (3) HDT-anchored: the HDT test stops at a fixed small flexural strain, about 0.2 % [R, ISO 75, verify], so the apparent modulus at HDT is about stress/strain = 0.45 MPa/0.002 = **225 MPa** at HDT(0.45) and 1.8/0.002 = **900 MPa** at HDT(1.8) (calc). Use (HDT_1.8, 900) and (HDT_0.45, 225) when both exist (PPA-CF: 196 and 227 degC, rep), else (HDT_0.45 - 15 K, 900) (V); interpolate log E linearly in T, clamp at 5 % of E(23). HDT is conditional (unannealed vs annealed PAHT-CF 84.8 vs 190.7 degC, rep), so the annealed flag selects the row. DMA is short-time; sustained load creeps, so add `creep_factor` (V, default 1.0, report suggests oven creep tests at 85, 95 and 105 degC, rep).
Deck: emit the `*ELASTIC` data lines at 23 degC and at 4-8 temperatures up to HDT + 20 K; give nodal `*TEMPERATURE` from `fea.temperature_map`; omit `*EXPANSION`. Temperature field: per-face Ts from the thermal stage is projected to each volume node by nearest surface point; through-thickness uniform by default, optional linear fall to `t_inside` over the wall (V). Because the stage is linear in load, a speed sweep needs no re-solve: stress scales with v^2 (SF(v) = SF(v0) (v0/v)^2) while temperature is fixed.

### 3.4 Aero and bump loads
Dynamic pressure q = 1/2 rho v^2; net force F = q * ClA; drag likewise (rep). Pressure field: p(x) = (Cp_out(x) - Cp_in) * q with Cp_in default 0 (V); `Cp` samples come from `cfd.surface_pressure`, mapped onto the FE boundary triangles by nearest point on the CFD surface, barycentric interpolation. Sign: ccx positive pressure pushes into the body, so a suction face gets a negative `Pn` (doc). **Conservation gate**: the integral of p over the mapped faces must equal the CFD resultant within 1 % per axis, else stop.
Nodal forces for C3D10: a uniform pressure on a face puts **0 on the corner nodes and p A / 3 on each mid-edge node** (doc, manual figure). For non-uniform p use consistent loads f_i = integral of N_i p dA on the 6-node triangle with a triangle rule exact to degree 3+ (a 6-point degree-4 rule is enough, [R]); or bin faces into 32-64 pressure classes written as separate `*SURFACE` + `*DSLOAD` entries (no subroutine needed). Prefer `*DLOAD/*DSLOAD` for uniform parts; use `*CLOAD` only for the consistent non-uniform case.
Bump (inertial) load: a = bump_g * 9810 mm/s2 along the chosen axis via `*DLOAD, elset, GRAV, magnitude, nx, ny, nz` (doc: "any acceleration vector"); needs `*DENSITY`. Part density = infill-weighted effective density when stiffness is also knocked down (V). Attached mass m (lamp, wing blade) is added as `*CLOAD` m*a at its attachment nodes. Combination: LC1 aero at design speed; LC2 aero at 1.25x speed (V); LC3 bump + gravity; LC4 aero + bump (V); envelope SF = min over cases. No extra load factor, the SF target already carries margin (V).

### 3.5 Mounts, constraints, singularities
Three models (V): `fixed_patch` fixes dofs 1-3 of all nodes within r_w of the bolt axis on the contact face (r_w default = 0.75 * washer radius); `rigid_washer` couples that patch to a reference node (`*COUPLING` + `*KINEMATIC 1,3`, ref node fixed by `*BOUNDARY` or loaded); `spring_pad` adds `SPRING1` elements with stiffness k = E_pad A/t (V). Fixed nodes and point loads create stress singularities: the peak value keeps rising as the mesh is refined. So SF at a mount is the minimum SF of the **volume-averaged stress inside a sphere of avg_radius_mm around the mount** (average the integration-point stresses of elements whose centroid is inside, weighted by element volume), and the raw peak is reported beside it. Flag `singular` when the peak SF changes by more than 20 % between two mesh levels while the averaged SF changes by under 5 % (V thresholds).
Reaction forces: `*NODE PRINT,NSET=mountN,TOTALS=ONLY` with `RF` gives the sum of reactions on that set (doc; it includes applied loads at those nodes, so keep loads off constrained nodes). For a kinematic reference node use `*SECTION PRINT` with `SOF` (doc). Equilibrium gate: |sum RF + sum applied| / |sum applied| < 1e-3 per axis.

### 3.6 Results reader (frd and dat)
`.frd` is fixed width, ASCII, blocks (doc [F], cgx manual chapter 11): `1C` job header, `2C` node block (`FORMAT` flag in the last column: 0 short, node id I5; 1 long, I10), rows ` -1 node x y z` with `3E12.5`; `3C` element block, per element a ` -1 id type group material` row then ` -2` node rows (15I5 short / 10I10 long); `100C` result block header (value, node count, text, analysis type, step); `-4 NAME ncomp irtype`; `-5 comp ...` rows naming components; ` -1 node v1..v6` data rows (continuation ` -2`); ` -3` closes the block; `9999` ends the file. **Slice by column widths, never split on whitespace**: a negative `E12.5` value can touch the previous one. ccx dataset names (doc): DISP, STRESS, FORC, TOSTRAIN, MESTRAIN, ENER, ERROR. Six stress components are in the order XX YY ZZ XY YZ ZX (from the cgx block layout, inf for STRESS); the `.dat` text lists integration-point stresses as `sxx syy szz sxy sxz syz` [R, verify on the first run]. The frd node values are extrapolated and averaged, so jumps at material interfaces are smeared (doc); use `.dat` integration-point stresses for the mount average.
Stress frame: results are global by default; rotate to the material frame with the part's R (preferred, no dependence on `GLOBAL=NO`).

### 3.7 Safety factors (all post-processing, per node or integration point)
- **von_mises**: SF = sigma_allow / sigma_vm with sigma_vm = sqrt(1/2((sx-sy)^2 + (sy-sz)^2 + (sz-sx)^2) + 3(txy^2 + tyz^2 + txz^2)); sigma_allow = yield or ultimate (doc: Fusion default is yield). Isotropic only; shown beside the others because it overstates SF for printed parts (G7).
- **max_principal**: SF = sigma_t / sigma_1 if sigma_1 > 0 else sigma_c / |sigma_3|, with the **direction-dependent** allowable: project the principal direction onto the stacking axis, sigma_t(n) = 1 / (sin^2(theta)/X_t^2 + cos^2(theta)/Z_t^2) (V, an ellipse interpolation between in-plane and Z strengths, theta from the stacking axis).
- **tsai_wu** (Wikipedia form, [F]): index = F_i s_i + F_ij s_i s_j with F1 = 1/Xt - 1/Xc, F11 = 1/(Xt Xc) (same for 2 and 3), F44 = 1/S23^2, F55 = 1/S13^2, F66 = 1/S12^2, F12 = -1/2 sqrt(F11 F22) as the usual estimate (extension to F13, F23 is an assumption, [F] note). Strength ratio: with B = sum F_i s_i and A = sum F_ij s_i s_j, R solves A R^2 + B R - 1 = 0, R = (-B + sqrt(B^2 + 4A)) / (2A) for A > 0; **SF = R**. Material frame: 1,2 in-plane, 3 stacking; Xt, Yt in-plane tension, Zt = z_ratio * Xt (ratios section 2.4), compressive strengths from user input (default Xc = 1.2 Xt (V), warn), shears S12 default 0.6 Xt, S13 = S23 default 0.5 Zt (V placeholders; coupon test replaces them).
- **interlayer** (delamination, V): index = (max(s33,0)/Zt)^2 + (t13/S13)^2 + (t23/S23)^2; SF = 1/sqrt(index).
Temperature: strengths are multiplied by r(T) at the point (3.3). Report per criterion: min SF over the part, over each mount neighbourhood, location, and whether `sf_target` is met.

### 3.8 Mesh and convergence
Surface -> tet: fTetWild takes a triangle surface, ideal edge length as a fraction of the bounding-box diagonal (default 0.05), envelope epsilon (default 1E-3 of the diagonal), writes `.msh` with a per-element minimum dihedral angle, MPL-2.0 [F]. 0.05 of a car-part diagonal is far too coarse for a 3 mm wall, so pass an absolute length (`-a`) tied to `edge_len_mm`. Gate on quality: report min dihedral and the conformal AMIPS energy (its filtering energy defaults to 10, the page advises not going below 8, [F]); refuse elements with min dihedral < 5 deg (V). Open or self-intersecting surfaces are the main failure; flood-fill inside/outside options exist [F].
Mesh convergence (Richardson / grid convergence index, Roache 1994 [R]): three meshes with refinement ratio r (>= 1.5) give observable f1 (fine), f2, f3 (coarse); order p = ln((f3 - f2)/(f2 - f1)) / ln r; extrapolated f_ext = f1 + (f1 - f2)/(r^p - 1); GCI_fine = 1.25 |(f2 - f1)/f1| / (r^p - 1). Use displacement at the tip, **averaged** mount stress and total strain energy (`ENER`/`ELSE`), never the raw peak. Pass when GCI < 5 % (V) and monotone convergence (the ratio (f3-f2)/(f2-f1) > 1).

### 3.9 Hand cross-check (always run, white-box)
In-plane bearing: sigma_b = F_inplane / (d t); axial pull-through shear on the head circle: tau = F_axial / (pi D_head t); shear-out: tau = F / (2 (e - d/2) t) with edge distance e to the hole centre [R, standard joint formulas]. Compare with the in-plane and Z allowables divided by SF. FE and hand SF should agree within a factor 2 for a clean joint; larger gaps raise a warning (V).

## 4. Proposed Vernon nodes

Labels contain no vendor names. Status: **now** = fully specified above and small, **WB** = white-box-first (transparent reference implementation validated against a cited source before optimising). All 18 nodes use the `fea.` stage prefix.

| Id | Label | Inputs | Outputs | Key params | emulates | Status |
|---|---|---|---|---|---|---|
| fea.material_fdm | Printed Material | preset row or CSV | material card, report | e_xy, e_z, nu_xy, nu_xz, g_xz, xt, xc, z_ratio, s12, density, infill_eff, mode (orthotropic/isotropic_z) | ccx `*ELASTIC` types; report anisotropy table | now |
| fea.layer_axes | Layer Axes | part mesh, print rotation | orientation card, material frame matrix | rotation R, origin | `*ORIENTATION` | now |
| fea.material_derate | Temperature Derating | material, DMA/HDT data | r(T) table, ccx temperature rows | source (dma_csv/dma_anchor/hdt_anchor), annealed, creep_factor (1.0), r_floor (0.05) | DMA retention, HDT test anchors | now (dma_csv, hdt_anchor), WB (dma_anchor) |
| fea.temperature_map | Temperature Field Map | per-face Ts, volume mesh | nodal degC | projection (nearest), through_thickness (uniform/linear), t_inside_c | thermal-to-structural field mapping | now |
| fea.tet_mesh | Solid Mesh | closed triangle surface | C3D10 mesh, quality report | edge_len_mm, eps_rel (1e-3), order (2), min_dihedral_deg (5) | fTetWild/Gmsh | now (external), WB (own Delaunay later) |
| fea.mesh_check | Solid Mesh Check | solid mesh | verdict, per-element quality, thickness count | min_through_wall (2), max_aspect | mesh quality panels | now |
| fea.mount_sites | Mount Sites | mesh, picked points/faces | node/face sets, patch radius | model (fixed_patch/rigid_washer/spring_pad), r_w_mm, avg_radius_mm, bolt_d_mm | constraint objects in FEM front ends | now |
| fea.load_aero | Aero Load | CFD resultant or surface Cp field, mesh | pressure per face or nodal forces, conservation report | mode (net_force/field), rho (1.11), v_kmh (200), cp_in (0), bins (48) | `*DLOAD/*DSLOAD`, pressure loads | now |
| fea.load_bump | Bump Load | mesh, attached masses | gravity/acceleration load, mass report | bump_g (4), axis, include_gravity | `*DLOAD GRAV` | now |
| fea.load_cases | Load Case Set | loads, mounts | named cases and envelope list | cases (aero, aero_1.25v, bump, aero+bump), speed_factors | step/load-case managers | now |
| fea.deck_write | Structural Deck Writer | mesh, materials, mounts, cases | deck text, id maps | unit_check, solver_options, output_keys | ccx input deck | now |
| fea.solve_external | Run Structural Solver | deck | frd/dat paths, log, status | exe_path, solver, threads, timeout_s | ccx run | now |
| fea.solve_linear | Reference Linear Solver | mesh (small), materials, loads | displacement, stress at points | tol (1e-10), max_dof (60000) | linear tet10 and tet4 static; checked against ccx 2.21 | now |
| fea.result_read | Structural Result Reader | frd, dat, maps | fields (u, s, e, rf), totals | stress_frame (global/material), use_ip_stress | frd reader | now |
| fea.safety_factor | Safety Factor Map | stress fields, material, r(T) | per-node SF per criterion, min per mount, verdict | criterion, sf_target (3), avg_radius_mm, yield_or_ultimate | static-study safety factor, Tsai-Wu | now (vm, principal, interlayer), WB (tsai_wu) |
| fea.mount_report | Mount Reaction Report | rf totals, loads | table: reaction per mount, equilibrium error, SF | tol (1e-3) | reaction output | now |
| fea.mesh_study | Structural Mesh Convergence | 3 result sets | p, f_ext, GCI, verdict | r, gci_max (5 %), metrics | grid convergence index | now |
| fea.hand_check | Bearing and Shear-out Check | mount loads, geometry, material | hand SF per mode | d, t, e, d_head, safety | joint hand calculation | now |

Total proposed: 18 nodes. Recommended order: material_fdm, layer_axes, load_aero (net_force), mount_sites, deck_write, solve_external, result_read, safety_factor (vm) and mount_report (these run a first end-to-end case), then tet_mesh and mesh_check, material_derate (hdt_anchor), temperature_map, load_bump, load_cases, hand_check, mesh_study; then WB: solve_linear, Tsai-Wu, own mesher.

## 5. Golden test ideas (numbers are (calc) unless marked)

Tolerances are Vernon pass criteria. "(ref)" = record from a licensed or open copy of the reference tool on a user-made input before freezing.

**G1 units and aero numbers** (rho 1.11, report values): v = 200 km/h = 55.556 m/s -> q = 1712.96 Pa = 1.71296E-3 MPa; ClA 0.5 m2 -> 856.5 N (report: ~856 N); v = 250 km/h -> q = 2676.50 Pa, 1338.3 N (report: ~1340 N); ratio 1.5625 = (250/200)^2 exact to 1E-12. Density 1.24 g/cm3 -> 1.24E-9 tonne/mm3. A one-element model with `GRAV` 9806.65 under its own weight must give a total weight force rho V g (`EMAS`/`EVOL`) within 1E-9 relative.

**G2 material constants**: isotropic E 3000 MPa, nu 0.35 -> lambda 2592.593, mu 1111.111, D1111 = 4814.815. Transversely isotropic case E1 = E2 = 2500, E3 = 2000, nu12 0.35, nu13 = nu23 0.30, G12 = 925.926, G13 = G23 = 800 -> nu21 0.35, nu31 = nu32 0.24, Delta 0.6831; D1111 = D2222 = 3396.282, D1122 = 1544.430, D1133 = D2233 = 1185.771, D3333 = 2569.170, D1212 = 925.926, D1313 = D2323 = 800.000 (closed form and 6x6 inversion agree to 1E-9). Deck line must read `2500.,2500.,2000.,.35,.30,.30,925.926,800.,` then `800.,23.`. A reciprocity-violating input (nu13 0.9) must be rejected by the stability gate.

**G3 cantilever** (L 100, b = h = 10 mm, E 2500, nu 0.35, tip load F 10 N, fixed at x = 0): Euler-Bernoulli tip deflection 1.6000 mm, shear correction (kappa 5/6) +0.01296 mm -> 1.6130 mm; root bending stress 6.000 MPa. C3D10 with >= 4 elements through the depth: tip deflection within 3 % of 1.613, root stress at the extreme fibre within 8 % (clamping raises it locally); the same bar with 1 element through the depth must be flagged by `mesh_check`. Reference linear solver agrees with the external solver to 0.5 % on the same mesh.

**G4 element conversion**: unit tetra with corners (0,0,0), (10,0,0), (0,10,0), (0,0,10) mm: volume 166.667 mm3 (`EVOL`), face 1 area 50.000 mm2; second-order conversion adds 6 nodes at the edge midpoints, `det` > 0; with p = 0.01 MPa on face 1 the total force is 0.5000 N, **0 N on each corner node and 0.16667 N on each of the three face mid-edge nodes** (doc distribution). A Gmsh-ordered tet10 fed through the converter must give the same volume (catches the swapped mid-edge nodes).

**G5 orientation mapping** (uniaxial bar 100 x 10 x 10 mm, stress 10 MPa from an end pressure, free lateral contraction, E1 = E2 2500, E3 2000): bar along material axis 1 -> elongation 0.4000 mm; bar rotated so the stacking axis 3 lies along the bar -> 0.5000 mm; bar along axis 2 -> 0.4000. Both within 0.5 % (the solution is uniform). Wrong `*ORIENTATION` points swap these.

**G6 temperature retention** (ratios from [F] Struz 2024 E' at 20 and 40 degC): PLA 1820/1980 = 0.919, PETG 883/1100 = 0.803, ASA 976/1210 = 0.807, ABS 875/1030 = 0.850; a DMA-curve derating fed with those two points must return those r(40) to 1E-3 and r(23) = 1. HDT anchors: apparent E = 225 MPa at 0.45 MPa and 900 MPa at 1.8 MPa; PPA-CF anchors (196 degC, 900 MPa), (227 degC, 225 MPa) interpolate to E(211.5) = sqrt(900*225) = 450 MPa (log-linear). Stress/displacement at 50 degC must be larger than at 23 degC for the same load by about 1/r (linear model).

**G7 failure criteria** (illustrative strengths XY tension 50, compression 60, Z tension 27.5 = 0.55 * 50 (PLA ratio), Z compression 60, S12 30, S13 = S23 20 MPa): pure sx = 10 -> Tsai-Wu SF 5.000; pure sz = 5 -> SF 5.500 (= 27.5/5); mixed sx 10, sz 5, txz 3 -> A 0.048512, B 0.131818, **SF 3.381**, while von Mises on 50 MPa gives 10.0995 MPa and **SF 4.951**: the isotropic check overstates safety by 46 %. The interlayer index for sz 5, txz 3: (5/27.5)^2 + (3/20)^2 = 0.0331 + 0.0225 -> SF 4.24.

**G8 mount hand check** (PPA-CF, report numbers): two mounts share 856.5 N at 200 km/h -> 428.25 N each (V); bolt d 5 mm, plate t 4 mm: in-plane bearing 428.25/(5*4) = 21.41 MPa vs allowable 168/3 = 56 MPa passes; the same load acting through the stacking axis vs 57/3 = 19.0 MPa **fails** (21.41 > 19.0). Demonstrates the report's rule to orient aero loads in-plane and size mounts against the Z value. At 250 km/h load is 1338/2 = 669 N per mount.

**G9 equilibrium and conservation**: mount `RF` totals plus applied `*DLOAD` resultant sum to < 1E-3 of the load per axis on a 3-mount test panel; mapped CFD pressure integral within 1 % of the CFD resultant (e.g. 856 N); speed sweep without re-solve: SF at 250 km/h = SF at 200 km/h * 0.64 (equals 1713/2677) within 1E-9.

**G10 convergence index** (three mesh tip deflections, r = 2): f1 1.6120, f2 1.6060, f3 1.5800 mm -> p = 2.115, f_ext 1.6138 mm, GCI_fine 0.14 %, pass; reversing to a non-monotone triplet (1.60, 1.61, 1.58) must give a "non-monotone" verdict.

**G11 frd parser**: a synthetic frd with a fused pair of values `-1.23456E+00-2.34567E+00` in one row parses to (-1.23456, -2.34567); short and long node id formats both read; a truncated file without `9999` raises "incomplete result".

**G12 end-to-end** (report chain, ClA 0.5, rho 1.11): printed wing mount in PPA-CF with SF target 3: at 200 km/h the load is 856.5 N, at 250 km/h 1338 N; the pass/fail verdict flips at the speed where the FE SF reaches 3 and must scale as 1/v^2 (SF(250) = 0.64 SF(200)). (ref) FreeCAD FEM + ccx on the same geometry and loads, tolerance +-5 % on deflection and +-10 % on averaged mount stress.

## 5a. Node map: material table and oven creep plan

- `fea.material_table`: the printed-material database shared with the print and solar stages
  (`vernon_core::materials`). Per filament family it holds the heat deflection temperature at 0.45 MPa (as printed,
  annealed where the project note gives one, and the vendor datasheet figure where that differs), the stacking
  direction (Z) strength over the in-plane strength, the in-plane allowable, density and water absorption, each with a
  source tag (`report`, `vendor datasheet`, `assumed`). Absolute moduli and strengths are placeholders until
  coupons are tested, and the node says so. Check: the Z ratios of PETG (0.46), ASA (0.29) and PLA (0.55) come out
  unchanged.
- `fea.creep_plan`: a test plan, not a prediction. For sustained-load coupons in an oven it lists three temperatures
  (`t1_c`, `t2_c`, `t3_c`, each below the heat deflection temperature), the load as `stress_fraction` of the
  room-temperature allowable on a coupon of `coupon_w_mm` by `coupon_t_mm`, the hold time `hold_h`, the reading
  times, and the margin from each test temperature to the heat deflection temperature. Measured strains entered as
  `measured_1_pct` to `measured_3_pct` are compared with `allowed_strain_pct` and a suggested creep factor in (0, 1]
  for the temperature-derating node is returned. Check: with all measured strains at zero the plan is output without a
  factor.

## 6. Sources (every URL consulted or cited)

Fetched this session [F]:
- https://www.dhondt.de/ccx_2.21.pdf (CalculiX CrunchiX user's manual: `*ELASTIC` card orders, `*DLOAD`, `*DSLOAD`, `*CLOAD`, `*BOUNDARY`, `*COUPLING`, `*KINEMATIC`, `*STATIC` solvers, `*NODE/EL FILE/PRINT` keys, tetra face numbering, C3D4 vs C3D10 advice, C3D10 pressure nodal forces; text extracted locally from the downloaded PDF)
- https://www.dhondt.de/cgx_2.21.pdf (CalculiX GraphiX manual, chapter "Result Format": frd block layout)
- https://help.autodesk.com/cloudhelp/ENU/Fusion-Simulate/files/SIM-STATIC-STRESS-RESULTS-CONCEPT.htm (static stress study result types, default safety factor definition)
- https://en.wikipedia.org/wiki/Tsai%E2%80%93Wu_failure_criterion (3D and plane-stress Tsai-Wu coefficients, F12 estimate)
- https://pmc.ncbi.nlm.nih.gov/articles/PMC11548368/ (Struz et al., DMA of FDM PLA, PETG, ASA, ABS, PA, HIPS: storage modulus, Tg, test conditions)
- https://arxiv.org/abs/1908.03581 (fTetWild: Hu et al., "Fast Tetrahedral Meshing in the Wild", ACM TOG 39(4), 2020)
- https://github.com/wildmeshing/fTetWild (licence MPL-2.0, parameters and defaults)
- https://gmsh.info/doc/texinfo/gmsh.html (Gmsh manual, first 100k characters only: element order options; node ordering section not reached)

Fetch attempted, failed [X]:
- https://wiki.freecad.org/FEM_Workbench (access-denied page)
- https://iris.poliba.it/retrieve/handle/11589/58649/329641/Pre_Print%20Materials%20and%20Design%2090%20(2016)%20453%BF458.pdf (HTTP 403; classical-lamination FDM ABS/PLA study)
- https://www.additive-x.com/shop/mpattachments/file/viewonline/id/916/product_id/2729 (HTTP 403; PPA-CF datasheet cited by the report)
- https://opus.lib.uts.edu.au/bitstream/10453/186075/2/1-s2.0-S026382312400243X-main.pdf (over the size limit; Li, Xu, Fang 2024, orthotropic FDM PLA, open access)

Seen in search results only [S]:
- https://www.autodesk.com/products/fusion-360/blog/static-stress-simulations-autodesk-fusion/ (yield/ultimate threshold option)
- https://univ-bejaia.dz/revue/baj/article/view/839 (Mehir et al., orthotropic elastic characterisation of FDM PLA, 2026)
- https://prepomax.discourse.group/t/timber-modeling-engineering-constants/1120 (use of `ENGINEERING CONSTANTS` in a ccx front end)
- https://link.springer.com/article/10.1007%2Fs40430-019-2037-8 (0-90 deg raster parts transversely isotropic)

From the project report, not re-fetched [rep]:
- https://calculix.discourse.group/t/orthotropic-strain-values-in-thermomechanical-analysis/2241 (orthotropic materials in ccx)
- https://cnckitchen.com/blog/comparing-pla-petg-amp-asa-feat-prusament (upright vs flat strength ratios)
- https://qidi3d.com/blogs/print-lab/best-filament-car-interiors-abs-asa (PAHT-CF HDT annealed vs not)
- https://hackaday.com/2025/12/10/failed-3d-printed-part-brings-down-small-plane/ (CF-ABS part failing on heat, Tg 53-54 degC)
- https://www.q-lab.com/weathering/weathering-test-standards/sae-j2527 (70 degC black-panel weathering)

Recalled from memory [R], verify before implementing:
- Tsai, S. W. and Wu, E. M. (1971), A general theory of strength for anisotropic materials, Journal of Composite Materials 5(1) 58-80.
- Hu, Y. et al. (2018), Tetrahedral meshing in the wild, ACM Transactions on Graphics 37(4) article 60.
- Roache, P. J. (1994), Perspective: a method for uniform reporting of grid refinement studies, Journal of Fluids Engineering 116(3) 405-413.
- ISO 75 (plastics, deflection temperature under load; the standard flexural strain increase of 0.2 % behind the HDT anchors).
- Bruhn, E. F., Analysis and Design of Flight Vehicle Structures (bearing, pull-through and shear-out joint formulas).
- Abaqus-style C3D10 node numbering and Gmsh second-order tetra ordering (node 5-10 edge order).

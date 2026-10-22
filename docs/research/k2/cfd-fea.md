# k2 vetting: CFD + FEA + result maps

Scope: vernon-nodes-cfd, vernon-nodes-fea, app viewport result maps. Reviewed 2026-10-22 against `crates/vernon-core` (geometry, report, attrib), `crates/vernon-graph` (node, param, data, graph), `docs/specs/automotive-cfd.md`, `docs/specs/structural-fea.md`, `CLEAN_ROOM.md`. Claims are ids k01-k97 in k2's order (k01-k36 CFD chunk, k37-k68 CalculiX chunk, k69-k97 result-map chunk). Verdicts: ACCEPT = adopt (or bug finding confirmed), ADAPT = adopt idea in Vernon's API, REJECT = do not adopt.

## 1. Summary

1. k2 never saw Vernon: its `MetaInfo`/`VertexField`/`ModifierNode`/`CoreEvent`/Python layer does not exist and is not needed. `Geometry` point/prim attribs (Float, Vec3), `Report` metrics with limits and `Data::File`/`Data::Json` already carry everything.
2. Its analytic "fallbacks" are physically wrong (cosine pressure with abs() and no suction; plate coefficient 1/16 gives 0.95 mm where the Navier series gives 0.42 mm for its own example) and also unsafe: fabricated numbers would pass Report gates. Replace with explicit estimate modes that fail the gate, or with a hard `CookError::External` when a solver is missing.
3. k2's own self-review of the OpenFOAM and CalculiX text is largely correct (wrong dictionary locations, invalid snappy keys, no solid section, no output cards, plane-stress stiffness, SZZ as von Mises, Cp shifted by -1). We confirmed it and map each fix to the existing specs (`cfd.*` 20 nodes, `fea.*` 18 nodes), whose numbers we re-derived (G9, G12, G13, G16, G2, G3 all reproduce).
4. Vernon-side hazards k2 missed: mm geometry vs k2's metre/Pa assumptions (x1000 and x1e6 errors); spec says "x forward" yet the case flows toward +x; the graph caches `CookError` results and is blind to file contents and tool versions; `cook` is synchronous with no cancel; serde_json turns non-finite floats into null.
5. Crates: none of shlex/regex/nalgebra/vtkio(default)/blake3 are needed; std `Command` plus a small hand-written frd reader, colour ramp and VTP/CSV writer cover it. Colour maps become a `Cd`-writing node plus an egui legend; the renderer needs no new buffer path. Legal claims are unverified assertions in k2; see section 3.

## 2. Claim table

| id | claim (short) | verdict | adaptation / reason | target | pri |
|---|---|---|---|---|---|
| k01 | CFD and structural nodes as placeholder-first modifiers returning mesh plus MetaInfo; UI from schema; NodeCooked event | ADAPT | Use `NodeOp` + `NodeTypeInfo` (forms already generated from `params`); placeholder = `whitebox` flag; results as attribs plus `Report`. Ids per spec (`cfd.*`, `fea.*`), never vendor names (k2's `airshaper_cfd` breaks CLEAN_ROOM). Event bus and Python example do not exist | cfd, fea | P1 |
| k02 | MetaInfo with user_blob and Vec<VertexField>; new GeometryObject variant | REJECT | Recursive enum does not compile; serde-skip drops data; `Geometry.point_attribs`/`prim_attribs` (Float, Vec3) plus `validate()` already enforce per-element lengths. Add standard names `cp`, `p_pa`, `disp` (Vec3 mm), `vm_mpa`, `sf` to the geometry.rs doc list | core | P1 |
| k03 | Cosine-law pressure p = 0.5 rho V^2 cos(theta), thirds to vertices, called Bernoulli | ADAPT | k2's critique is right (abs() erases suction; raw sum scales with valence; not Bernoulli). Keep only as `cfd.surface_pressure` mode `impact_estimate`: Cp = Cp_max * max(0, n.d_up)^2 with Cp_max = 1 (incompressible stagnation; the classical 2cos^2 form is the hypersonic limit), leeward Cp = param `cp_base` (default 0), area-weighted vertex average, then p = Cp*q. Emit metric `estimate_only` = 1 with limit 0 (fails, so `Report::passes()` is false) plus a Warning note. Test: 30 deg inclined plate gives Cp 0.25 windward, plate parallel to wind gives 0 | cfd | P2 |
| k04 | Plate deflection w = p Lx^2 Ly^2 / (16 E t^3), tent shape | REJECT | Verified: for 0.2 x 0.1 m, t 5 mm, E 2.1e11, p 1 MPa, nu 0.3 the Navier double series gives 0.421 mm (nu 0.28: 0.427) vs k2's 0.952 mm (2.26x high). Square case: coefficient 0.004062 (Timoshenko 0.00406) so 1/16 is 41% high there. No nu term, wrong shape. Do not ship an analytic structural fallback; use Navier and the spec G3 cantilever as test oracles only | fea | P2 |
| k05 | Tent factor uses max() so edges do not go to zero; NaN on flat axis; undefined vertex_normal | ACCEPT | Confirmed (max at nx=0, ny=0.5 gives 1.0; min gives 0). Lesson only: any reference plate test must assert edge displacement = 0 and guard zero-length axes | fea | P3 |
| k06 | Generic process runner with reader threads and mpsc messages | ADAPT | k2's use-after-move and Sender/Receiver mix-up are real. Build `ExternalJob` in vernon-graph (std only): spawn, drain stdout and stderr on threads to `work_dir/logs`, poll `try_wait`, kill on deadline or cancel flag. Error type `CookError::External{tool,message}`. Cwd = `ctx.work_dir`, not `temp_dir/airshaper_<id>` | graph | P1 |
| k07 | recv_timeout(1 s) loop, JSON result contract, analytic fallback on any failure, vendor CLIs | ADAPT | Loop until exit or disconnect, deadline checked each tick, child killed on timeout (k2's own bug list is right). REJECT the invented JSON contract (read real outputs: forceCoeffs file, `.frd`), the silent fallback (see k04, gate hazard H3) and the assumed vendor CLIs (unverified; SimScale is cloud; out of scope and trademark) | graph, cfd, fea | P1 |
| k08 | split external_args with shlex | REJECT | Backslash is an escape in POSIX rules so `C:\models\car.stl` is mangled (k2 is right). No shell parsing: one argument per line in a `Str` param, passed to `Command::args`. Also k2's pin is stale: shlex is 2.0.1 now | cfd, fea | P1 |
| k09 | Fingerprint by XOR of rotated float bits | REJECT | Vernon has no fingerprint method: `Graph::cook_inner` hashes type id, params JSON, bypass and upstream signatures. XOR scheme is weak and hashes only path presence. Real gap is file contents and tool version, see H4 and graph action 2 | graph | P1 |
| k10 | Apache-2.0, no native libs, optional user-installed solvers, CI uses missing path | ADAPT | Consistent with CLEAN_ROOM "External tools" table. Missing solver must give `CookError::External`, never analytic numbers. serde_json and anyhow are workspace deps; tempfile (dev-dep) is already in the lock | cfd, fea | P1 |
| k11 | tests/simulation.rs plate tests | ADAPT | Keep ideas (tilted plate, closed form, zero edge). Fix: test names vs asserts, `vernor_core` typo, in-plane wind cannot catch the sign error. Drive via `Graph`+`Registry` like the existing `caches_until_upstream_changes` test; oracles are Navier and spec G3 | cfd, fea | P2 |
| k12 | Wizard node writes a half-car case, optional run, id openfoam_cfd_wizard | ADAPT | Split per spec section 4: `cfd.tunnel_domain`, `cfd.mesh_setup`, `cfd.case_setup` (write files, pure) and `cfd.mesh_build`/`cfd.solve_steady` (run). Output `Data::File(case_dir)` under `ctx.work_dir`; `/tmp` path rejected | cfd | P1 |
| k13 | Seven-step protocol mapping | ADAPT | Use spec 3.10 (benchmark gate first, A/B last; mesh study = 3 meshes). k2 step 7 "Dubai density in constant folder" is wrong: solver is incompressible, density enters via `rhoInf` in force coefficients and post-run scaling (`cfd.aero_loads`, spec 3.12). Empty default boxes: boxes come from `cfd.refinement_plan` | cfd | P1 |
| k14 | Defaults 27.8 m/s, R 0.33, 84.2 rad/s, 10 layers, 1.20 kg/m3, 303.15 K | ADAPT | Arithmetic checks (27.8/0.33 = 84.24 rad/s). k2's density critique is right: p/(R T) = 1.164 kg/m3 at 30 C dry (1.204 at 20 C), mu 1.861e-5, nu 1.598e-5 at that density. Use spec defaults (U 30, 5 layers, nu 1.5e-5), compute rho from `cfd.air_state`, wheel R from `cfd.vehicle_prep`, drop the unsourced "Dubai-standard" label, label wheel speed "rad/s" only | cfd | P1 |
| k15 | simpleFoam needs kinematic nu, not volScalarField rho and mu | ACCEPT | Correct. nu = mu/rho (1.85e-5/1.20 = 1.54e-5 as k2 states; 1.60e-5 at 1.164). File is `transportProperties` or `physicalProperties` by flavour (spec 3.7). No thermo, no PCG entries for rho or nu | cfd | P1 |
| k16 | blockMesh 3 blocks, 10 m domain, invalid vertices, axes inconsistent | ACCEPT | Confirmed hazards. Domain from spec 3.2 (3L up, 6L down, 1% blockage, aspect 5:3, G6/G7), one side only (y from 0), Z up, ground at z = 0, vertex count validated by test | cfd | P1 |
| k17 | blockMeshDict and snappyHexMeshDict belong in system/; boundary list syntax invalid | ACCEPT | Correct (0/ holds fields). Write per-patch `type` and `faces`; `mergePatchPairs` only. Test: snapshot plus a minimal dictionary tokenizer that checks FoamFile header and balanced braces | cfd | P1 |
| k18 | movingWallVelocity with motionSolver in snappy dict | ACCEPT | Diagnosis right: static-mesh moving ground is `fixedValue` U = (U,0,0) on `ground` (spec 3.4). Frame hazard H1 applies | cfd | P1 |
| k19 | rotatingWallVelocity with U0 and vector omega, wheel axis z | ACCEPT | Keywords are origin, axis, scalar omega. Z-up car has lateral axis (0,1,0); omega = -U/R for that axis (spec 3.4, G10: 51.0 rad/s at 16 m/s, R 0.3137). Patch names from the `part` prim attribute set by `cfd.vehicle_prep`, not hard-coded | cfd | P1 |
| k20 | Prism layer keys; relSize expression always 1/N | ACCEPT | Verified: first/(first*N) = 1/N always. Use nSurfaceLayers, expansionRatio, two of final/first/thickness, patch regexes without stray space; first height from y+ (spec 3.3, G9 reproduces: Cf 0.00263, u_tau 1.088, y1 13.8 um) | cfd | P1 |
| k21 | regionBox not snappy syntax; levels hard-coded; centre/size ambiguity | ACCEPT | Use `searchableBox` geometry (min, max corners, converted mm to m) plus `refinementRegions`. Boxes computed by `cfd.refinement_plan` (kit bbox +0.15 m, wake boxes) and passed as `Data::Json`, since `ParamKind` has no list type | cfd | P1 |
| k22 | Quality limits belong in meshQualityControls; sub-dicts missing | ACCEPT | Use spec 3.3 keys (maxNonOrtho 65, maxBoundarySkewness 20, maxInternalSkewness 4, maxConcave 80...). k2's "maxSkewness 1.5" is not the right key. Add surfaceFeatures and checkMesh to the chain (spec 3.7) | cfd | P1 |
| k23 | Three-level convergence is mesh vs tolerance confusion; needs three meshes and GCI | ACCEPT | Correct. `cfd.mesh_study` per spec 3.9; re-verified: G12 gives p 1.537, phi_ext 6.168, e_a 1.50%, GCI 2.17%; G13 gives p 2.029, ext 0.27723, GCI 2.33% (spec G12 says p 1.53; inside its 0.02 tolerance). Separate case directory per level. Invented simpleFoam flags dropped; `residualControl` in SIMPLE block | cfd | P1 |
| k24 | Steady controlDict misuse; forceCoeffs needed | ACCEPT | deltaT 1, endTime = iterations. forceCoeffs on body, wheels, kit (not ground) with rhoInf, magUInf, lRef, Aref; half model Aref = A_front/2 (spec 3.1, G18) | cfd | P1 |
| k25 | fvSolution: PIMPLE ignored by simpleFoam, rho/nu/mu solvers meaningless, no residualControl | ACCEPT | SIMPLE block with `consistent yes` and `residualControl`; regex keys written correctly (e.g. `"(U\|k\|omega)"` form), per flavour | cfd | P1 |
| k26 | fvSchemes missing turbulence terms; linearUpwind advice | ACCEPT | steadyState ddt; `bounded Gauss linearUpwind grad(U)` for U, limitedLinear for k and omega (spec 3.7 recipe, marked practice) | cfd | P1 |
| k27 | No 0/ fields, no turbulence model file, no FoamFile headers, stray `?.`, empty dynamicMeshDict | ACCEPT | Write 0/U p k omega nut, momentumTransport or turbulenceProperties by flavour, FoamFile header in one helper. Do not write dynamicMeshDict (not needed for wall-velocity wheels) | cfd | P1 |
| k28 | run_now spawns blockMesh, snappyHexMesh, runCase.sh | ADAPT | Confirmed defects (inherit stdio, warnings ignored). Run chain per spec 3.7 through `ExternalJob`, logs per step in `work_dir/logs`, stop on first non-zero exit, `checkMesh` gate before solving, no bash script | cfd | P1 |
| k29 | Windows host needs WSL2 or a container; CI covers file writing only | ACCEPT | Add `runner` enum param (native / wsl / container). WSL: `wsl.exe -- <cmd>` with `wslpath` path translation. Snapshot tests run on every OS; solver jobs optional on Linux (spec G28) | cfd | P2 |
| k30 | `{:d}`, `box` keyword, Hash on f64 fail to compile | ACCEPT | Confirmed (`box` is a reserved word). Nothing to port: params live in `Params` and the graph hashes their JSON | cfd | P4 |
| k31 | cook reads 4 of 11 inputs and returns Mesh::default() | ACCEPT | Confirmed bug. Vernon pattern: read every param via `params.float/bool/str`, pass geometry through as the same `Arc` (free), never default it | cfd | P1 |
| k32 | fingerprint constant when run_now is true | REJECT | No fingerprint API. Real issue: a `run` Bool param auto-launches hours of CPU on any re-cook. Keep run out of `cook`: cook reads results if present; the app's Run button starts the job on a worker thread and then re-cooks | cfd, app | P1 |
| k33 | STL copied with read_to_string; half_car mirror missing | ADAPT | read_to_string breaks binary STL (right). Write STL from `Geometry` via vernon-io with named solids, ASCII for snappy, scaled mm to m. Half model = keep y >= 0 half (spec F3 symmetry check), not mirror a full car | io, cfd | P1 |
| k34 | Tests in target/debug, string asserts; clippy cannot detect GPL | ACCEPT | Use `tempfile`, header and tokenizer checks, snapshots (spec G28). Clippy remark is right | cfd | P2 |
| k35 | Generated OpenFOAM case is data so Apache-2.0 stays | ADAPT | Reasonable but k2 cites nothing. Our risk is copying verbatim tutorial dictionaries (GPL text): write templates from the public user guide keyword descriptions only (CLEAN_ROOM allowed sources). See section 3 for legal status | cfd | P1 |
| k36 | UI via InputSpec, custom refinement-box list | ADAPT | Auto-generated panel from `ParamSpec`; no custom widget. Box list is a graph input (k21). Fix "RPM (rad/s)" label to rad/s. Run-now checkbox rejected (k32) | cfd, app | P3 |
| k37 | CalculiX node as text-writer plus external ccx; files under src/core | ADAPT | Layout is crate-based: `crates/vernon-nodes-fea/src/{material,deck,frd,...}.rs`, `register(reg)`. Split per spec (`fea.deck_write`, `fea.solve_external`, `fea.result_read`). Pure-Rust, no linking: ACCEPT | fea | P1 |
| k38 | Outputs fe_results Json, inp_path, run_log; store_aux_output | ADAPT | Outputs: `Data::Geometry` with attribs, `Data::Report`, `Data::File` (inp, frd). `store_aux_output` is undefined, drop. Case dir = `ctx.work_dir` | fea | P1 |
| k39 | Inputs: temperature, knockdown_z 0..1, E, nu, expansion, fixed_nodes, point_loads JSON | ADAPT | Material from `fea.material_fdm` (spec 2.3). k2's zero lower bound makes stiffness singular (right): floor 0.05. Units MPa (E 1e3..1e6). Fixed nodes and loads come from `fea.mount_sites` and `fea.load_*` (point groups on `Geometry`), not UintList or JSON strings. Steel defaults are not printed-part defaults. 150 C is far above polymer Tg (PLA 64 C), a poor example | fea | P1 |
| k40 | Scale only E_z, G_xz, G_yz by k; nu unchanged | ADAPT | Plausible but incomplete: reducing E3 changes nu31 = nu13 E3/E1 and Delta must stay positive (spec 3.2 stability gate). Verified: for E 210000, nu 0.28, k 0.6 the true C33 is 144939 MPa, not k*E = 126000. Use `z_ratio` with full orthotropic constants | fea | P1 |
| k41 | No *EXPANSION so temperature has no effect | ADAPT | Finding is right for the card set, but spec 2.2 omits *EXPANSION deliberately: temperature only selects temperature-dependent constants (r(T), spec 3.3). Risk the spec leaves open: bolt-restrained panels develop real thermal stress. Add optional `thermal_strain` (default false) writing *EXPANSION with a reference temperature (verify card syntax in the manual) | fea | P3 |
| k42 | ANISOTROPIC card with E/(1-nu^2), C33 = E_z, 12 values | ACCEPT | Wrong maths confirmed: plane-stress 2.279e11 vs 3D isotropic C11 2.685e11 Pa at E 2.1e11, nu 0.28. The card needs 21 values. Use `ENGINEERING CONSTANTS` or ORTHO per spec; we re-derived spec G2 by 6x6 inversion (D1111 3396.28, D1122 1544.43, D1133 1185.77, D3333 2569.17) | fea | P1 |
| k43 | No *SOLID SECTION or element set | ACCEPT | Add `ELSET=EALL` on *ELEMENT and `*SOLID SECTION,ELSET,MATERIAL,ORIENTATION` (spec 3.1 deck order) | fea | P1 |
| k44 | G hard-coded 8.1e10 vs E/(2(1+nu)) | ACCEPT | E/(2(1+nu)) = 8.203e10 Pa for E 2.1e11, nu 0.28. Always derive G12 (spec 3.2) | fea | P1 |
| k45 | *BOUNDARY node,1,3,0; 0-based vs 1-based ids | ACCEPT | Keep a deck-id to Vernon-point map (spec 3.1); the tet mesher creates its own nodes so ids are not vertex index + 1 | fea | P1 |
| k46 | *CLOAD only for unit-axis directions, others dropped silently | ACCEPT | Bug confirmed. Aero loads go through `*DLOAD/*DSLOAD` pressure or consistent nodal forces (spec 3.4); any vector is decomposed, never dropped. Conservation gate within 1% | fea | P1 |
| k47 | C3D8 from triangles with repeated corners | ACCEPT | Invalid (zero-volume). Use closed surface to C3D10 via external fTetWild or Gmsh (`fea.tet_mesh`, spec 3.8; node order conversion test G4) | fea | P1 |
| k48 | let bindings swallowed in a comment | ACCEPT | Compile bug in k2 text only | fea | P4 |
| k49 | 6-decimal coordinates; units never declared; x1000 and /1e6 conversions | ADAPT | Real Vernon fact: geometry is mm. Spec unit system mm, N, tonne, s, MPa, so coordinates need no scaling, displacement is already mm and stress already MPa. k2's x1000 and /1e6 would corrupt results. Write `*HEADING` with the unit system and run unit gate G1 | fea | P1 |
| k50 | NLGEOM=ON static step | ACCEPT | Linear by default (spec 1: NLGEOM off). `*STATIC` field order in k2's text is right but moot | fea | P2 |
| k51 | No output cards; .frd alternative unmentioned | ACCEPT | Add `*NODE FILE U,RF`, `*EL FILE S,E,NOE`, `*NODE PRINT TOTALS=ONLY` for reactions; read `.frd` with fixed-width slicing (spec 3.6, G11) | fea | P1 |
| k52 | `ccx -i case.inp -o case`, unused Command, error type mismatch | ADAPT | Man page (calculix-ccx 2.21) lists only `-i <jobname>` and `-v`; `-o` is not documented there (UNVERIFIED), so do not pass it. Job name without extension is the usual form (UNVERIFIED from the man page text we could fetch; confirm with a one-element run). Use `CookError::External` | fea | P1 |
| k53 | Wait loop exits on first silent second | ACCEPT | Same fix as k06/k07 | graph | P1 |
| k54 | Stale case.dat reused when not re-run | ACCEPT | Per-run subfolder keyed by deck hash, delete old `.frd/.dat/.sta` before run, manifest file with deck hash that the reader checks | fea | P1 |
| k55 | Regex over whole .dat; stress rows match displacement | ACCEPT | Bug confirmed. Prefer `.frd` fixed width; if `.dat` is parsed, section-aware. No regex crate needed (not in the lock) | fea | P2 |
| k56 | HashMap enumerate gives arbitrary order | ACCEPT | Dense vector indexed through the id map; length equals point count or error; `Geometry::validate` catches attrib length drift | fea | P1 |
| k57 | von Mises taken from SZZ | ACCEPT | Use full formula (spec 3.7); component order XX YY ZZ XY YZ ZX. Test: uniaxial s gives s; pure shear t gives sqrt(3) t. vm overstates SF for printed parts (spec G7: 4.95 vs Tsai-Wu 3.38) so it is a secondary criterion | fea | P1 |
| k58 | Zero-filled vectors when no results | REJECT | Compile error is real, but fake zero fields look like a valid run (displacement 0, SF infinite) and pass gates. Missing results must be an error | fea | P1 |
| k59 | build_meta_info with VertexField, mm and MPa conversion | REJECT | No MetaInfo. Write point `disp` (Vec3), `vm_mpa`, `sf` and Report metrics (`max_disp_mm`, `max_vm_mpa`, `sf_min` with limit = sf_target, lower_is_better false). Deck is already mm and MPa (k49) | fea | P1 |
| k60 | Fingerprint with DefaultHasher, f64 not Hash | REJECT | See k09. k2's bug list (f64 Hash, missing params) is right but there is nothing to port | graph | P4 |
| k61 | Python listener and quick-start | REJECT | No Python binding or `vernon.listen` exists. Magnitude bug is real (norm of triples, not max component). Use `vernon-cli` JSON of `Report` | cli | P4 |
| k62 | Colour-map by von_mises, displacement arrows, no scale | ADAPT | Colour via colour-map node (k75, k78). Deformed shape via a Displace node (P + s*disp), `s` default = 5% of bbox diagonal / max disp, printed in the legend. Arrows deferred. Files named by k2 do not exist (app is a 15-line stub) | geo, app | P3 |
| k63 | Four fast tests incl. 2-cube node test expecting zero fields | ADAPT | Keep deck, parser and node tests; replace the zero-field expectation by "missing solver gives External error". Use spec goldens G1, G2, G11, G3 | fea | P1 |
| k64 | Linux CI cantilever with apt calculaix | ADAPT | Package is `calculix-ccx` (the Debian and Ubuntu man pages are for that package; "calculaix" is a typo). Oracle: F L^3/(3EI) = 1.600 mm and 6.000 MPa root (verified), 1.613 mm with shear; tolerance 3% per spec G3 (not 5%). Optional Linux job | fea | P3 |
| k65 | CalculiX launched not linked so GPL does not reach Vernon; NOTICE line | ADAPT | Matches CLEAN_ROOM. Policy: v1 does not bundle ccx. If an installer ever ships it, ship as a separate executable with GPL text and source offer. Status of the legal point in section 3 | docs | P3 |
| k66 | Crates regex, anyhow, serde_json, std::process | ADAPT | serde_json 1.0.154 and anyhow 1.0.104 already locked. regex is not in the lock; avoid | fea | P4 |
| k67 | Stretch: thermal gradients, multi-step, progress bar, rollers, coupling, slicer path | ADAPT | Thermal map and anisotropy are core (spec `fea.temperature_map`). Progress bar from the log tail; rollers become symmetry constraints; coupling and slicer hand-off deferred | fea | P4 |
| k68 | Five-week build order | ADAPT | Use spec section 4 order: material_fdm, layer_axes, load_aero, mount_sites, deck_write, solve_external, result_read, safety_factor, mount_report, then meshing and derating | fea | P1 |
| k69 | Named scalar fields in MetaInfo with scalar_field and field_range | REJECT | Use attribs. Keep the idea of a range helper: `scalar_range` ignoring non-finite values, returning None for empty (k2's +inf/-inf bug). f64 to f32 happens in `MeshBuffers::from_geometry` already | core | P2 |
| k70 | Cp = (p - p_ref)/q with p_ref = q | ACCEPT | Bug confirmed: stagnation p = q gives Cp 0, not 1. Correct Cp = (p - p_inf)/q; kinematic pressure: Cp = p_kin/(0.5 U^2) (spec 3.8). Spec G21 reproduces (428.2 Pa, Cp -0.2125) | cfd | P1 |
| k71 | Cp node inputs rho and U optional; kinematic caveat | ADAPT | Part of `cfd.surface_pressure`: params `rho`, `u`, `p_inf`, enum `pressure_kind` {static_pa, kinematic}. Stale optional-input bug cannot occur since params are hashed. Writes `cp` | cfd | P1 |
| k72 | Cp cook sketch with two incompatible matches | REJECT | Non-compiling and wrong types. Vernon: `inputs::geometry(inputs, 0)?` then `Arc::make_mut` and `point_attribs.set("cp", ...)` | cfd | P1 |
| k73 | SF = yield/von_mises with MPa to Pa, cap 999 | ADAPT | Formula right. Vernon stress is already MPa, so k2's x1e6 would be a bug. Non-finite values must not be stored (serde_json writes them as null): store capped `sf` (param `sf_cap`, default 100) and exclude capped values from the colour range | fea | P1 |
| k74 | Inputs yield_strength and zero_div_cap, empty output_spec | ADAPT | Allowables from the material (`fea.material_fdm`) by port, no silent 250 MPa default (k2 point accepted). Declare outputs (geometry, report). Criteria per spec 3.7 | fea | P1 |
| k75 | 256-entry Viridis LUT, abbreviated | ADAPT | Table is legal to embed (colormap data is CC0, see crate/licence notes) but generate it by script rather than paste. Add a diverging ramp centred on 0 for signed Cp and a banded ramp for SF (red below 1, amber below target). NaN gets a grey, min == max gets the mid colour. Home: `vernon-core::colormap` | core | P2 |
| k76 | Test expects min = blue-green | ACCEPT | Viridis min is (68,1,84) dark purple, max (253,231,37). Fix expectation; add NaN and min == max cases | core | P2 |
| k77 | NOTICE line, "public domain" vs "MIT" LUT, year 2025 | ADAPT | Viridis data is CC0 (secondary sources). Fix wording, year 2026. Keep `Vernon Team` NOTICE minimal | docs | P4 |
| k78 | wgpu per-vertex colour buffer at location 3 | REJECT | Renderer already reads point `Cd` (smooth path) or prim `Cd` (flat) in `MeshBuffers::from_geometry`. The colour-map node writes `Cd`; no new buffer, no device argument, length checked by `validate()` | geo, render | P2 |
| k79 | Toolbar drop-down listing scalar fields | ADAPT | Node param `attribute` (Str) with an app-side picker listing Float attribs of the displayed geometry. eframe 0.32 is pinned: use `ComboBox::from_id_salt` (rename from `from_id_source`, recalled, verify) and `selectable_label(bool, text)` | app | P3 |
| k80 | Min/max override and colour bar | ADAPT | Node params `range_mode` {auto, percentile, manual}, `min`, `max` with min < max enforced (`BadParam`). Legend drawn with `painter` mesh of coloured quads (no texture), unit label from the attribute | app, geo | P2 |
| k81 | Register via registry.register with closures | ADAPT | `reg.add(Op)` in `register()`; ids `cfd.surface_pressure`, `fea.safety_factor`, `geo.colour_map`; `Stage::Cfd`/`Stage::Fea` grouping is automatic; mark `.implemented()` | cfd, fea, geo | P1 |
| k82 | Shared fingerprint over-invalidates | REJECT | No such function; the graph hashes each node's own params. Per-node isolation already holds | graph | P4 |
| k83 | Viewport needs no cache key | ADAPT | Rebuild `MeshBuffers` when the displayed output `Arc<Geometry>` changes (`Arc::ptr_eq`) or expose the cook signature | app | P3 |
| k84 | Python get_meta by name | REJECT | No Python; fields-by-name map issue is moot with named attribs | cli | P4 |
| k85 | Solver values need mapping onto display mesh | ACCEPT | Valid gap. Add a `transfer attributes` operation (nearest triangle plus barycentric, spec 3.4) with conservation check; CFD wall values are patch face centres on the refined snappy surface, not STL triangles | geo, cfd | P2 |
| k86 | Post-process only, external solvers | ACCEPT | Matches CLEAN_ROOM external tools table | cfd, fea | P1 |
| k87 | Proposed file layout under src/ | REJECT | Paths do not exist. Colour ramp in `vernon-core`, node in `vernon-nodes-geo`, legend in `app` | core, geo, app | P2 |
| k88 | Cp test uses same p_ref as code | ACCEPT | Circular (right). Use independent values: spec G21, stagnation Cp 1 | cfd | P1 |
| k89 | SF test only the main case | ACCEPT | Add zero stress (capped, finite), MPa input, missing attribute error | fea | P1 |
| k90 | Headless GPU render test | ADAPT | `request_adapter` is an `Instance` method (right); software path is `force_fallback_adapter`, platform dependent. Test colour output on the CPU (Cd values equal LUT) and keep a GPU readback test `#[ignore]` | render | P3 |
| k91 | Cache invalidation test | ADAPT | Add a graph-level test in the style of the existing one: change `rho` recooks Cp only; change SF params leaves Cp cached | graph | P3 |
| k92 | CI deps nalgebra, once_cell | ADAPT | nalgebra 0.35 needs rustc 1.89 vs workspace `rust-version = 1.80`, and is unnecessary (glam plus a small 6x6 solver). once_cell replaced by `std::sync::OnceLock`. CI must include Windows | all | P3 |
| k93 | User colour tables from CSV | ADAPT | Path param, validate monotone values, 2 to 256 stops; defer | geo | P4 |
| k94 | Linear/log/percentile scaling | ACCEPT | Log needs positive guard. Percentile (default 2 to 98) matters here: mount singularities (spec 3.5) and the SF cap would otherwise crush the colour range | core, geo | P2 |
| k95 | Animated field playback | REJECT | v1 is steady RANS and static FEA. `CookContext.frame` exists for later; no frame cache now | graph | P4 |
| k96 | ExportMapNode to .vti or CSV | ADAPT | `.vti` is image data (right). Write ASCII `.vtp` (PolyData) or CSV by hand; vernon-io PLY already carries scalars. Do not add vtkio default features | io | P3 |
| k97 | Compute shader per-fragment interpolation | REJECT | Premature; wording inconsistent (a fragment shader interpolates). Per-vertex `Cd` suffices for surface meshes | render | P4 |

## 3. Bugs and hazards

Confirmed bugs in k2 claims (all verified above): cosine-law abs() hides suction; plate formula high by 41% (square) and 2.26x (its own 2:1 example); tent uses max(); plane-stress E/(1-nu^2) as 3D C11; C33 = E_z; ANISOTROPIC card 12 values vs 21; SZZ as von Mises; Cp = p/q - 1; recursive enum; process-runner use-after-move; silent-second loop exit; shlex on Windows paths; zero-filled fake results; 999 SF cap polluting colour range; Viridis min colour; Cp test circularity.

Hazards in the Vernon/spec context that k2 did not see:

- H1 Frame. Spec hand-off says "x forward", but CFD spec 3.2 puts the nose at minimum x and flow toward +x, with the ground and wheel tangent moving +x. A car whose nose points to +x would sit backwards. Decide once: `cfd.vehicle_prep` rotates the vehicle 180 degrees about Z into the flow frame (or the inlet moves to +x). Add a test using an asymmetric body.
- H2 Units. Geometry is mm. OpenFOAM needs metres (scale at STL write, k33); the structural deck uses mm, N, tonne, s, MPa (no scaling, k49). Any metre/Pa assumption (k2) is off by 1e3 to 1e6.
- H3 Gating. `Report::passes()` is true when nothing carries a failing limit. Estimates and fallbacks must carry a failing metric (k03), and missing solver output must error, or an invented SF would turn the pipeline green.
- H4 Graph cache. `CacheEntry` stores errors too, and only `invalidate_all` exists: a missing-solver error stays cached after the user installs the solver. The signature covers params JSON and upstream signatures only, so editing a CSV or tool upgrade with the same path does not recook. `work_dir` defaults to the temp dir and is keyed by node name.
- H5 Blocking. `cook(&self, ...)` is synchronous with no cancel flag; CFD jobs run for hours (spec: about 375 CPU-hours for a 12M-cell half car). Long runs need a worker thread and a cancel flag in `CookContext`.
- H6 Non-finite floats. serde_json writes NaN and infinity as null, which then fails to read as f64: attribs and metrics must be finite.
- H7 Thermal strain. Spec choice to omit *EXPANSION is defensible but not conservative for bolt-restrained printed panels (k41).
- H8 Spec check. Spec G12 states p 1.53; the algorithm gives 1.537 (inside its tolerance). Other spec goldens re-derived and fine: G2, G3, G9, G13, G16, G21.

Legal (general area only; not legal advice). k2 cited no case or statute for its GPL positions (k10, k35, k65).
- Separate process versus linking: the FSF FAQ treats pipes, sockets and command-line arguments as the normal mechanism between separate programs, with an "intimate data structures" caveat and the final call left to courts. Verified only through secondary quotations; the FSF page itself was not fetched, so the primary text is UNVERIFIED.
- Artifex Software v. Hancom, N.D. Cal., April 2017 (Judge Corley): motion to dismiss denied, GPL can be enforced as a contract. VERIFIED from two secondary write-ups (law-firm note, press); docket not opened. It cites Jacobsen v. Katzer (Fed. Cir.); existence confirmed in the same sources, year (2008) from memory.
- That GPL does not cover files a GPL program reads or writes (generated case files): the FSF position is recalled, not verified this session: UNVERIFIED.
- CalculiX "GPL-2.0 or later" (k65) versus the CLEAN_ROOM table's "GPL-2.0": UNVERIFIED.
- Viridis colormap data is CC0 (from a matplotlib-derived colormap file header; secondary). Practical risk for Vernon is copying GPL tutorial text, not generated numbers.

## 4. Crate vetting

`cargo info` run 2026-10-22 with rustc 1.99.1. Workspace `rust-version` is 1.80.

| crate | version | licence | pure Rust? | verdict |
|---|---|---|---|---|
| shlex | 2.0.1 | MIT OR Apache-2.0 | yes | REJECT: POSIX quoting wrong on Windows paths; k2 pinned stale 1.2 |
| tempfile | 3.27.0 | MIT OR Apache-2.0 | yes (OS bindings) | ACCEPT as dev-dependency (already in lock) |
| regex | 1.13.1 | MIT OR Apache-2.0 | yes | not needed; hand parser for frd and dat |
| serde_json | 1.0.154 (lock) | MIT OR Apache-2.0 | yes | already used; beware null for non-finite |
| anyhow | 1.0.104 (lock) | MIT OR Apache-2.0 | yes | already used; node errors stay `CookError` |
| wait-timeout | 0.2.1 | MIT/Apache-2.0 | yes (OS FFI) | REJECT: `Child::try_wait` plus deadline suffices |
| which | 8.0.6 | MIT | yes | optional, only for PATH discovery of solvers; std can do it |
| vtkio | 0.7.0-rc2 | MIT OR Apache-2.0 | default features pull lz4 and xz2 (C libraries) | REJECT with defaults; write ASCII VTP by hand |
| colorous | 1.0.16 | Apache-2.0 | yes | optional source to generate the LUT; check bundled data notices before depending on it |
| colorgrad | 0.9.0 | MIT OR Apache-2.0 | yes | REJECT: MSRV 1.85 exceeds workspace 1.80, unneeded |
| nalgebra | 0.35.0 | Apache-2.0 | yes | REJECT: MSRV 1.89, not needed (glam plus 6x6 Gauss-Jordan, verified) |
| blake3 | 1.8.7 | CC0-1.0 OR Apache-2.0 | default build compiles C/asm | REJECT: std `DefaultHasher` already used by the graph |
| once_cell | n/a | n/a | yes | use `std::sync::OnceLock` |

External programs (separate processes only): OpenFOAM (GPL-3.0), CalculiX (GPL), fTetWild (MPL-2.0, per spec), Gmsh (GPL): none linked, none bundled in v1.

## 5. Ordered actions per target crate (each testable)

**vernon-core**
1. Add standard names `cp`, `p_pa`, `disp`, `vm_mpa`, `sf` to the geometry.rs doc list and a `names` constants module. Test: constants unique, a Geometry carrying them passes `validate()`.
2. Add `colormap.rs` (viridis from a generated 256 LUT, diverging, SF-banded). Test: endpoints (68,1,84) and (253,231,37); NaN gives grey; min == max gives mid colour; out of range clamps.
3. Add `scalar_range` (finite only; optional percentile clip). Test: empty gives None; infinities ignored; 2 to 98 percentile ignores a single spike.

**vernon-graph**
1. Add `Graph::invalidate(id)` and stop caching `CookError::External`. Test: op that fails once then succeeds cooks twice.
2. Add an optional `NodeOp` hook contributing extra signature data (file content hash of Path params, tool version). Test: change file contents with the same path, node recooks.
3. Add `cancel: Arc<AtomicBool>` to `CookContext`.
4. Add `ExternalJob` (std only: spawn, drain both pipes to log files, `try_wait` polling, kill on timeout or cancel). Tests using the test binary as the child: exit 0; non-zero with captured log; silent child killed at deadline; more than 1 MiB output does not deadlock.

**vernon-io**
1. STL writer: ASCII multi-solid with scale factor. Test: mm to m round trip, triangle count, solid names.
2. ASCII VTP and CSV attribute export (P3). Test: header, counts, values.

**vernon-nodes-cfd** (order follows spec section 4)
1. `cfd.air_state`: ideal gas plus Sutherland. Tests: 303.15 K dry gives 1.164; spec G1, G2.
2. `cfd.force_coeffs` maths: Cd, Cl, half-model Aref/2, axle split, window stats. Tests: G16, G17, G18; Report limits via `Metric::with_limit`.
3. `cfd.mesh_study`: GCI with oscillation flag. Tests: G12 to G15; gates 2% and 3%.
4. `cfd.surface_pressure`: Cp from p, kinematic option. Tests: G21, stagnation Cp = 1, sign of Cl.
5. `cfd.tunnel_domain`, `cfd.refinement_plan`, `cfd.ground_wheels`: G6, G7, G9, G10; frame test (H1).
6. Dictionary writers with FoamFile header helper and `flavour` enum; `system/` placement; snapshot plus tokenizer tests (G28); no vendor text.
7. `cfd.case_setup`: 0/ fields, SIMPLE `residualControl`, steady controlDict, forceCoeffs; G8.
8. `cfd.mesh_build`/`cfd.solve_steady`: `ExternalJob` chain, runner enum, per-level case dirs, `checkMesh` gate; missing binary yields `CookError::External`.
9. `impact_estimate` mode with failing `estimate_only` metric; tilted-plate test.

**vernon-nodes-fea**
1. `fea.material_fdm`: ORTHO and ENGINEERING CONSTANTS lines with stability gate. Tests: G2 numbers; nu13 = 0.9 rejected; C33 differs from k*E.
2. `.frd` reader fixed width into a dense vector via the id map. Tests: G11 (fused numbers, short and long ids, missing 9999).
3. `fea.deck_write`: units header, *SOLID SECTION, *NODE FILE and *EL FILE, linear step; unit gate G1; snapshot.
4. `fea.solve_external`: argv `-i <job>` without extension, per-run directory keyed by deck hash, stale-result guard, version string into the signature.
5. `fea.safety_factor`: von Mises (uniaxial, pure shear), SF = allow/vm in MPa, capped finite `sf`, missing-attribute error; Tsai-Wu G7.
6. `fea.load_aero` conservation gate (1% per axis) and `fea.mount_report` equilibrium (1e-3).
7. Oracles: cantilever G3 (1.600 mm, 6.000 MPa), Navier plate (0.421 mm for the 2:1 example), optional Linux ccx job.
8. Optional `thermal_strain` flag (P3).

**vernon-nodes-geo**
1. `geo.colour_map`: attribute, ramp, `range_mode`, writes `Cd`. Test: ramp output equals LUT; min < max enforced.
2. Displace-by-attribute node with auto scale. Test: zero scale leaves points unchanged.
3. Attribute transfer (nearest triangle plus barycentric) with conservation check.

**app**
1. Legend widget (coloured-quad mesh, range, unit, NaN swatch).
2. Attribute picker from Float attribs; Run and Cancel buttons for external jobs on a worker thread, then re-cook; Recook button using `Graph::invalidate`.
3. Rebuild `MeshBuffers` when the displayed `Arc<Geometry>` changes.
4. Tests: CPU check that `from_geometry` colours equal the ramp; GPU readback `#[ignore]`.

# k2 advice vetting: general area (core, graph, io, cli, clean room, verification)

Vetted 2026-10-22 against Vernon code (`vernon-core` geometry/report/attrib, `vernon-graph` node/param/data/graph/registry, `vernon-io` lib, `vernon-cli`), `CLEAN_ROOM.md`, `README.md`, `docs/specs/*.md`. Crate facts from `cargo info` (crates.io index, 2026-10-22). Legal facts from 5 web searches. Numbers in section 3 were re-computed. Not legal advice. Anything not checked is marked UNVERIFIED.

## 1. Summary

1. k2 never saw Vernon's code: its Qt/Electron, gRPC, C-ABI OpenCascade shim, PyO3, rayon/DashMap scheduler, enum-of-geometry and C++ library advice conflicts with a pure-Rust, Apache-2.0, egui, mm / Z-up workspace and is mostly rejected; its process instincts (white-box first, tolerance-based goldens, separate-process copyleft tools, Merkle-style cache keys, honest provenance) are kept.
2. Re-computed and found wrong in k2: y+ about 1 for a 1 mm first layer (really 73-94), Ahmed Re and reference length, Catmull-Clark `n`, the 24-vertex half-edge cube, mm-to-m scale 1000, the solar 0.05 kWh/m2-h vs 3% tolerance, and the lap 0.30 s vs 0.5% tolerance; every vendor "gold" number (Cd 0.032, 182.342 MPa, 23 patches, 12,856 faces, generation 18) is invented and rejected as acceptance data.
3. Legal citations: VERIFIED by search: Lotus v. Borland (1st Cir. 1995, affirmed by an equally divided Supreme Court 1996), Altai (2d Cir. 1992), Sega v. Accolade (9th Cir. 1992), L'Oreal v eBay (C-324/09, trade-mark liability, not trade dress), SAS v WPL (C-406/10, 2012); k2 got three courts wrong, misused two cases, and Baker v. Microsoft, the patent numbers and the UAE-law points are UNVERIFIED and must not be cited; the fabricated approvers, dates and vernon-org.org addresses must never enter the repo.
4. Reading Vernon's own code surfaced hazards k2 could not: `Graph::set_param` accepts any name/type/range, `next_id` can make `add_node` overwrite a node after a hand-edited `.vrn`, node names flow into `work_dir` (path traversal), PLY header counts reach `with_capacity` unchecked, `io::write` is not atomic, `Path` params are cached by string only, cook errors (including external-tool failures) are cached, `Report::passes()` is vacuously true and ignores NaN, declared `rust-version = "1.80"` is false (eframe 0.32.3 needs 1.85), `vernon-cli` is an empty `main`, and there is no LICENSE, NOTICE, `deny.toml` or CI.
5. Verdicts: 17 ACCEPT / 82 ADAPT / 65 REJECT across 164 rows (3 rows K96, K110, K111 are derived hazards, not k2 claims); top P0 actions: LICENSE/NOTICE + `cargo deny` licence gate, forbid fabricated numbers/records in docs, replace the `ldd` idea with `cargo deny` + a DLL-list check; P1 is the graph/io hardening and the `vernon-cli cook` gate with exit codes 0/1/2.

## 2. Claim table

Verdict is exactly ACCEPT, ADAPT or REJECT. Priority: P0 before anything public, P1 next sprint, P2 soon, P3 later, "-" nothing to do. `nodes-*` means the stage crates. Conventions assumed in adaptations: mm, Z-up, outward CCW winding, `Report` gating via `Metric.limit`, point attrib `N` / `Cd` / `uv` / `dist`, prim attrib `part`, `temp_c` (Float, degC, surface temperature, to be added to the attribute list in `geometry.rs`).

| id | claim (short) | verdict | adaptation / reason | target | pri |
|---|---|---|---|---|---|
| K01 | White-box every tool first, keep placeholders swappable, capability matrix | ACCEPT | Already Vernon's model (`NodeTypeInfo.whitebox`, `implemented()`). Add a generated matrix stage x {whitebox, implemented, external_tool} from the Registry. | vernon-cli | P2 |
| K02 | Black-box contract from public docs, log undefined behaviour, pin tool version | ADAPT | Add "Reference version" and "Undefined or implementation-specific behaviour" sections to the spec template; matches current `docs/specs` headers. | docs | P2 |
| K03 | Keep a copy of the vendor docs in the repo | REJECT | `CLEAN_ROOM.md` forbids copying vendor documentation; specs paraphrase and cite the URL. | docs | P0 |
| K04 | Data-driven test vectors (nominal, boundary, error, perf); negative tests for silent clamping | ADAPT | Golden cases carry per-field tolerances; error cases assert a specific `CookError` variant. Vernon today never clamps or checks (`set_param` stores anything), so negative tests need the validation in K110. | graph, verify | P1 |
| K05 | CI fails on any output deviation greater than 0 | REJECT | Floating point: per-field absolute/relative tolerance. Exact hashes only for Vernon's own deterministic writers. | verify | P1 |
| K06 | Run the original repeatedly for determinism; keep a DIFFS.md | ADAPT | For OpenFOAM/CalculiX record run-to-run spread as the noise floor (parallel runs are not bitwise repeatable); keep `docs/specs/DEVIATIONS.md` listing deliberate differences. | verify, docs | P2 |
| K07 | Fuzz 1 to 2 hours and triage | ADAPT | Budget by iteration count, not hours. proptest byte-mutation of valid files in normal CI; cargo-fuzz (needs nightly + libFuzzer) as a Linux-only job. | io | P1 |
| K08 | Two disjoint repos, dedicated VM, separate CI/credentials, clipboard off, biweekly audits | REJECT | Disproportionate for this project; the two-team rule is already met by spec-writer vs implementer roles in docs. | docs | - |
| K09 | Pre-commit hook matching hashes of known binaries | ADAPT | Hash matching misses nearly all copying. Instead a CI script fails on banned extensions (.dll .exe .hda .otl .3dm .f3d .x_t) and on install-folder strings (`houdini21.0`, `Program Files\`) in tracked files, plus a dated provenance log `docs/research/PROVENANCE.md` (source URL, what was read, by whom or which model). | verify, docs | P1 |
| K10 | FOSSology / scancode nightly; signed IP log | ADAPT | One ScanCode run (Apache-2.0, UNVERIFIED) before each release; a "signed" log is just dated, attributed git history. | docs | P3 |
| K11 | Spec template with goal/scope/inputs/edge matrix/strings/patent line/approval/tag | ADAPT | Existing specs already cover goal, scope, parameters, tests, sources. Add: edge-case matrix, "Vernon UI strings" (ours only), "Patent status: unreviewed or reviewed(date, who)". Per-spec git tags rejected (git history suffices). | docs | P2 |
| K12 | Numeric error codes (1001, 1102, 2003) | REJECT | Invented; Vernon uses typed `CookError` variants. | graph | - |
| K13 | `specs/<Tool>/<Feature>.md` for 11 vendor tools | REJECT | Specs are organised by pipeline stage; vendor names stay inside docs. k2's "publicly documented defaults" carry no citation: treat all as unverified. | docs | - |
| K14 | Concrete acceptance numbers (Cd 0.032, 182.342 MPa, 23 patches, 12,856 faces, generation 18, 800k cells, 0.176 mm, 0.3 mm) | REJECT | No vendor run happened; invented. Use analytic or Vernon-reference-script numbers and mark a value "recorded" only after a real licensed run on our own input. | verify | P0 |
| K15 | Legal README / approvals: Alice Nguyen, Carlos Mendez, 2025-09-03, vernon-org.org addresses, "patent search performed" | REJECT | Fabricated provenance. The repo is github.com/vernon3d/vernon. Never copy these into any record. | docs | P0 |
| K16 | Prefer geometry-level comparison over exact match; byte-identical IGES; SSIM > 0.99 | ADAPT | Geometry tolerances yes (P95 of `dist`, Hausdorff, vertex distance). Byte-identical IGES is impossible (timestamps). SSIM only for Vernon's own renderer with fixed seed, otherwise skip. | verify | P2 |
| K17 | Copy functional UI concepts (quad viewports, command panel, modifier stack, time slider); own icons, art, wording, palette, name | ACCEPT | Matches `CLEAN_ROOM.md` and `ui-dcc-conventions.md` section 4.1. Generalise the trademark test that exists only in `vernon-nodes-geo/src/tests.rs` (list includes orca, prusa, blender) to every registered node, `Stage::label`, enum options, port names. | vernon-cli (tests) | P1 |
| K18 | Trade-dress perception test (over 30% name the reference product, iterate) | ADAPT | Threshold is invented, not a legal standard. Keep as an internal design review only. | docs | P3 |
| K19 | Trademark clearance for the product name, Nice classes 9 and 42, avoid vendor marks | ADAPT | Sound: "Vernon" is a common name; run a clearance search before a public release (counsel). | docs | P2 |
| K20 | Patent keyword search (USPTO/EPO/WIPO), CSV of 20 hits, CLEAR / DESIGN-AROUND / BLOCK per algorithm, FTO memo | ADAPT | Keep a one-line patent status per algorithm, default `unreviewed`, never CLEAR without a recorded search. Apache-2.0 section 3 gives a patent grant from contributors. FTO memo is a counsel decision. | docs | P3 |
| K21 | Specific patent numbers (US 10,544,317, 10 247 847, 8 941 726, 9 562 123, 6 784 437) | REJECT | UNVERIFIED (search budget spent on case law); do not cite. | docs | - |
| K22 | Lotus v. Borland, Fed. Cir. 1995, menu hierarchy functional | ADAPT | Court wrong. VERIFIED: 1st Cir., 49 F.3d 807 (1995), menu command hierarchy held an uncopyrightable method of operation; affirmed by an equally divided Supreme Court, 516 U.S. 233 (1996), so no national precedent. | docs | P2 |
| K23 | Computer Associates v. Altai, Fed. Cir. 1992, abstraction-filtration-comparison | ADAPT | Court wrong. VERIFIED: 2d Cir., 982 F.2d 693 (1992). Test description is correct. | docs | P2 |
| K24 | Sega v. Accolade, 9th Cir. 1992, intermediate copying fair use | ACCEPT | VERIFIED 977 F.2d 1510 (9th Cir. 1992). It concerns disassembly, which Vernon forbids anyway; it does not loosen our rules. | docs | P3 |
| K25 | Apple v. Samsung (2012 Fed. Cir.) for look-and-feel; Baker v. Microsoft (2019 E.D. Va.) | REJECT | Apple: the 2012 Fed. Cir. ruling (No. 2012-1105) is a preliminary-injunction decision on design/utility patents; the trade-dress ruling is the Nov 2013 permanent-injunction appeal (No. 13-1129). Baker: no search hit, UNVERIFIED, do not cite. | docs | - |
| K26 | L'Oreal v. eBay as C-210/10 limiting trade dress | REJECT | VERIFIED: C-324/09, 12 July 2011, marketplace operator liability for trade marks, not functional trade dress. | docs | - |
| K27 | UAE Federal Law No. 7 of 2002 on Copyright | ADAPT | UNVERIFIED here; local counsel if distributed in the UAE. | docs | P3 |
| K28 | Core permissive; GPL/AGPL tools as unmodified separate executables with licence + source offer; core-only vs bundle build | ACCEPT | Matches `CLEAN_ROOM.md`. In 0.x do not bundle them: detect user-installed tools (probe `--version`). If bundled later, ship their LICENSE and exact source URL. | docs, graph | P2 |
| K29 | CLA granting the core licence | ADAPT | Apache-2.0 section 5 already licenses submitted contributions; a DCO sign-off is lighter than a CLA. | docs | P3 |
| K30 | Component licence labels (Orca/Prusa GPL-3.0, Ladybug MIT, OpenSCAD LGPL, CalculiX GPL-2.0, Klipper AGPL/GPL-3.0) | ADAPT | `CLEAN_ROOM.md` already says AGPL-3.0 for Orca/Prusa. Ladybug "MIT" and OpenSCAD "LGPL" are believed wrong (UNVERIFIED, not crates). Add an SPDX id and upstream LICENSE URL column to the CLEAN_ROOM table and verify each row once. | docs | P2 |
| K31 | CGAL as a clean-room kernel inside an Apache core | REJECT | GPL-3.0-or-later / commercial C++ (not on crates.io). | - | - |
| K32 | OpenCascade LGPL-2.1 may be dynamically linked if relinkable | REJECT | Legally arguable but a C++ build on a Windows-GNU toolchain contradicts "pure Rust, separate processes"; crates `opencascade` and `opencascade-sys` are LGPL-2.1. STEP/B-rep goes via a helper process (`DataKind::File`) or a pure-Rust spike (K70). | io, docs | P2 |
| K33 | Permissive substitutes: Open3D, Embree, OSPRay, OGRE, Assimp, tinygltf, OpenVDB, RapidJSON | REJECT | All C++ (OpenVDB believed MPL-2.0, UNVERIFIED); not Rust dependencies. Use `gltf` or `serde_json` only if a need appears. | - | - |
| K34 | Three conflicting core licences (README MIT, memo Apache-2.0, plan closed core) | ACCEPT | Resolution: Apache-2.0 (workspace `Cargo.toml`, `README.md`, `CLEAN_ROOM.md`). Reject MIT and closed core. The repo root has no LICENSE or NOTICE file: add both. | docs | P0 |
| K35 | Overstated legal claims (RPC never derivative, AGPL section 13, export review) | ACCEPT | Agree. Docs say "separate processes at arm's length", not "never derivative"; counsel for edge cases. | docs | P2 |
| K36 | `ldd` on the core binary plus check_*.sh scripts, license_finder / FOSSA / SBOM | ADAPT | `ldd` cannot see static or copied code and separate executables never show. Gate with `cargo deny check licenses bans sources` (no `deny.toml` exists yet), `cargo tree -e normal`, and on Windows GNU `objdump -p` DLL list of release exe; SBOM via cargo-cyclonedx 0.5.9 at release. | verify | P0 |
| K37 | Buy commercial SDKs (RhinoCommon, Fusion API, SimScale, Polycam), load at runtime via dlopen | REJECT | Conflicts with clean-room/open design; EULAs usually forbid redistribution; "free" and "under 5k USD" unverified. libloading 0.8.9 (ISC) is not needed. | - | - |
| K38 | Monorepo with services/, plugins/, legal/, gRPC service bus, Qt/Electron UI, CMake C++ wrappers | REJECT | Vernon is one Rust workspace with an egui app; nothing here helps. | - | - |
| K39 | `cleanroom_registry.json` (spec version, test hash, patent status) | ADAPT | Derive it: registry test that every `NodeTypeInfo.spec` path exists and every node with `whitebox == false` has a spec. No new JSON file. | graph (test) | P1 |
| K40 | `.so`/`.dll` plugins through a C ABI | REJECT | Unsafe and ABI-fragile; nodes are Rust crates exposing `register(&mut Registry)`. | - | - |
| K41 | Rust memory safety avoids output divergence from the reference | REJECT | Wrong rationale; memory safety does not constrain numerics. | - | - |
| K42 | Wrap OSS tools as JSON stdin/stdout CLIs; Blender 2.9x/3.x | ADAPT | `CLEAN_ROOM.md`: Vernon writes input files, reads output files; no shim protocol. Add a plain process runner (K111). Pin tool versions in the spec, not the code (Blender 5.0+ per solar spec). | graph | P1 |
| K43 | NURBS surface: de Boor, open-uniform knots, weights default 1, degree 1 to 5 | ADAPT | References correct (Piegl and Tiller). The sample is invalid: a degree-p direction needs at least p+1 control points, in a (nu x nv) grid. Validate knots: count = n+p+1, non-decreasing, clamped ends. Degree cap is a spec choice. | nodes-surface | P2 |
| K44 | Rhino file formats (.3dm, .gh) and IGES/STEP test sequence | REJECT | Not Vernon formats; sequence not runnable as written; IGES has timestamps. | - | - |
| K45 | Rhino default tolerances (0.001, 0.01 mm, 0.5 degree, 64 spp) | REJECT | Uncited vendor defaults; Vernon documents its own defaults in `rhino8-mesh-tools.md`. | docs | - |
| K46 | Point cloud to surface (voxel decimation, outlier removal, hole fill, max deviation 0.5 mm, voxel 2 mm) | ADAPT | Keep the pipeline. Consistency rule: replacing a voxel's points by their centroid moves a point at most the cube diagonal, sqrt(3) x voxel, so voxel <= max_dev / sqrt(3) (0.289 mm at 0.5 mm); k2's own default 2 mm violates its own rule. Warn in the node. One watertight NURBS surface and identical vertex counts are unrealistic. | nodes-clean | P2 |
| K47 | Mesh to CAD (plane/cylinder/sphere/cone/torus recognition, fit tolerance 0.2 mm, 23 patches) | ADAPT | Primitive detection by RANSAC (Schnabel et al. 2007, public paper). Tests on synthetic cylinder/plane meshes recovering radius within 1e-6 mm; the 23 / 0.176 mm numbers rejected (invented); GDM and Parasolid formats out. | nodes-surface | P2 |
| K48 | Sheet-metal flat patterns, K-factor 0.2-0.5, DXF/G-code | REJECT | Out of scope (stage 7b is doubly-curved panels). Checked for the record: BA = theta (R + K T) gives 9.0478 mm per 90 degree bend (R 5, T 2, K 0.38), five bends 45.24 mm; BD = 2 (R+T) tan(theta/2) - BA = 4.952 mm. "Minimum bend radius per building code" is wrong. | - | - |
| K49 | Fusion-style parametric CAD / CAM / generative design, bracket test | REJECT | Out of scope. | - | - |
| K50 | ZEISS-style ICP alignment, GD&T, deviation map; RMS <= 0.001 mm on 200k points | ADAPT | Covered by `alignment-deviation.md`. At least 3 non-collinear points for 6-DOF is correct. A fixed RMS is noise-floor dependent: test with a synthetic known rigid transform (noise-free recovery within 1e-6 mm; with sigma noise, RMS tends to sigma). | nodes-clean | P2 |
| K51 | Cloud CFD REST API (SimScale/AirShaper), Cd rounded to 3 dp, 1.225 and 1.81e-5 | REJECT | Vernon runs OpenFOAM locally. Never round results (store full precision, round at display). 1.81e-5 / 1.225 gives nu 1.478e-5; the spec derives nu from T, p, RH. | nodes-cfd | - |
| K52 | FEA K u = F, CalculiX .inp, cantilever 182.342 MPa +/-0.5 | ADAPT | CalculiX decks per `structural-fea.md`. A fixed peak-stress value is brittle (mesh dependent). Gate on analytic beam checks (tip deflection F L^3 / (3 E I), bending stress M c / I) and mesh convergence. | nodes-fea | P2 |
| K53 | GA optimiser with seeded best generation 18; exact vendor window title | REJECT | Needs the vendor's GA and RNG; vendor brand string; "targetDrag" ambiguous (coefficient vs force). No optimiser in v1. | - | - |
| K54 | Cloud photogrammetry; test says scale factor 1000 converts mm to m | REJECT | Reversed: mm to m multiplies by 0.001; Vernon `FileUnit::Metre.to_mm() = 1000` is m to mm. Local COLMAP avoids uploading photos (k2's privacy point is right). | nodes-capture | - |
| K55 | Klipper spec, Moonraker macro test, pressure-advance numbers | REJECT | Klipper is "target only" in `CLEAN_ROOM.md`. Per the summariser the k2 text mixes up pressure advance and input shaping and uses wrong Moonraker routes (UNVERIFIED). | - | - |
| K56 | Pure-Rust geometry core plus B-rep behind a C-ABI OpenCascade shim (liboc_shim, libTKernel) | ADAPT | Core is already pure Rust (`vernon-core`). Shim rejected (C++/LGPL, MinGW build). STEP: helper process, or spike pure-Rust truck-stepio 0.3.0 / ruststep 0.4.0 (both Apache-2.0). | io, docs | P2 |
| K57 | LGPL-2.1 compliance checklist (sections 4.1/4.2, pinned OCCT 7.7.0, swap libraries) | ADAPT | Only relevant if an LGPL component ever ships. Relink duty is in section 6 (UNVERIFIED reading); a swap needs the wrapper rebuilt against an ABI-compatible build. | docs | P3 |
| K58 | Post-build `ldd` / `otool` link check on three OSes | ADAPT | See K36; Windows GNU leg uses `objdump -p`. | verify | P1 |
| K59 | gRPC `geometry.export(path)` | REJECT | tonic 0.14.6 pulls an async runtime; the CLI covers headless use. The path hazard is real: `vernon_io::write` truncates the target with `File::create` (K96). | io, cli | P1 |
| K60 | NOTICE.txt lists components; GPL tools shipped in the same zip | ADAPT | NOTICE for dependencies via cargo-about 0.9.2; do not ship GPL tools in 0.x; CalculiX GPL-2.0 vs 3.0 UNVERIFIED. | docs | P2 |
| K61 | PyO3 + maturin wheels for Python 3.9-3.12 | REJECT | Defer. pyo3 0.30.1 and maturin 1.15.0 are MIT/Apache. Python 3.9 is end of life and 3.10 ends Oct 2026 (memory): target 3.11+ when a binding is wanted. | - | P3 |
| K62 | `Geometry` trait + `GeometryObject` enum (Mesh/PointCloud/SubD/BRep), row-major 4x4 | REJECT | Vernon has one `Geometry` (point cloud = no prims, polyline = open prim), `DMat4` column-major, `Aabb` exists. | core | - |
| K63 | Separate Mesh / PointCloud / SubD / TopoShape types; mesh stores faces plus an edge list | REJECT | No type split; derive adjacency on demand (`vernon-nodes-geo` topology) and never store a redundant edge list (k2's own sync hazard). | core | - |
| K64 | Half-edge mesh; cube test expects 24 vertices | ADAPT | A half-edge mesh needs shared vertices. `make_box` has 8 points: add test V=8, E=12, F=6, V-E+F=2 (needs an `edge_count()` helper). | core | P2 |
| K65 | Quadric-error decimation (Garland-Heckbert) | ADAPT | Citation right (1997). Needs fold-over (normal flip) rejection, boundary preservation, manifold link check, target ratio or error. Tests: sphere to 10% stays closed, no flipped normal, deviation written to `dist`. | nodes-clean | P2 |
| K66 | f32 point clouds, f64 B-rep, "same precision throughout" | REJECT | Core is f64 (`DVec3`). Checked f32 spacing: 0.0078125 mm at 65.5-131 m, 0.015625 mm at 131-262 m; fine for parts, but keep f64 in core. f32 only at GPU upload and inside STL files. | core | P2 |
| K67 | Voxel downsample + statistical outlier removal with kiddo | ADAPT | kiddo 6.3.0 (MIT/Apache) needs Rust 1.89 (workspace says 1.80); start with a hash-grid kNN like `weld.rs`, or rstar 0.13. Defaults to state: k 20, std ratio 2.0 (general knowledge). Output order must be deterministic: sort voxel keys (std `HashMap` order is random). | nodes-clean | P2 |
| K68 | Catmull-Clark formulas and 5-level default | ADAPT | Edge point (V1+V2+F1+F2)/4 and vertex (F + 2R + (n-3)P)/n are right, but `n` is the vertex valence (k2 wrote average of face points). Verified on the cube [-1,1]^3: corner goes to (5/9, 5/9, 5/9), edge point (0, 0.75, 0.75), face point (1, 0, 0). Vertex/face counts per level: 26/24, 98/96, 386/384, 1538/1536, 6146/6144, Euler 2 each. Boundary/crease rules and a max-prim guard are required (5 levels = x1024). | nodes-surface | P2 |
| K69 | SubD test with exactly 4500 refined vertices and 0.02 mm to the limit surface | ADAPT | Exact target unreachable (see counts); choose the nearest level. Measure against a deeper level (k+3) as limit proxy, not nearest point on the refined mesh. | nodes-surface | P3 |
| K70 | Quick-start sample (teapot, SubD, STEP, Boolean) | REJECT | Use-after-move, wrong API, exports the wrong mesh. A Vernon quick start is `vernon-cli cook`. | vernon-cli | P3 |
| K71 | C shim API, Rust FFI RAII wrapper, `unwrap` on paths, undefined handle types | REJECT | Same as K56; listed bugs (c_char imports, NulError conversion, pointer-to-pointer mismatch) show it was never compiled. | - | - |
| K72 | `oc_shape_hash` as cache fingerprint | REJECT | Identity-based, not stable across processes (per summariser). Fingerprints must be content based (K86). | graph | - |
| K73 | Mesh-to-B-rep by sewing triangles into a solid | REJECT | OCCT specific; `BRepMesh_IncrementalMesh` is B-rep to mesh (per summariser). Surface stage fits analytic faces/NURBS, then hands off STEP. | - | - |
| K74 | Wrap OCCT extrude/loft/fillet/chamfer; future: sewing, healing, GPU SubD, Open3D ICP, Python | REJECT | No modelling kernel in scope; Open3D is C++; ICP is already specced in `alignment-deviation.md`. | - | - |
| K75 | Geometry golden tests (face count +/-1, volume 0.1%, Boolean 0.5%, library checksum) | ADAPT | Add `Geometry::signed_volume()` and closedness helpers (absent). Box 10 x 20 x 30 = 6000 mm3; uniform scale s gives s^3; flipped winding gives negative. Exact counts for deterministic algorithms. Binary-vs-tarball checksum is meaningless: hash source and built files separately. | core | P1 |
| K76 | bash build scripts for OCCT and shim, Windows via oc_shim.dll | ADAPT | Vernon is PowerShell (`scripts/dev-env.ps1`); CI on `windows-latest` with the GNU target plus `ubuntu-latest`. | verify | P1 |
| K77 | 1M-triangle Boolean under 2 s on 8 cores | REJECT | Unverified; Vernon has no Booleans. Time weld, normals and read per stage instead. | verify | P3 |
| K78 | 24-week plan with named owners, v0.9.0 vs 1.0 | REJECT | Fictional owners, internally inconsistent schedule. | - | - |
| K79 | Unit enum mm/cm/m/inch/foot/Custom; canonical mm; per-primitive unit | ADAPT | Canonical mm exists; `vernon_io::FileUnit` has mm/cm/m/in and a passing 254 mm = 10 in round trip. Optional Foot (304.8 mm); no Custom, no per-geometry unit. | io | P3 |
| K80 | Graph is a HashMap plus an edge list, frozen after build | REJECT | Vernon: `IndexMap<NodeId, NodeInstance>` with `(node, output)` wires, editable. | graph | - |
| K81 | Node trait with `cook(&mut self)`, incremental `recook`, Send+Sync, PythonCallbackNode | REJECT | Vernon `NodeOp::cook(&self)` is stateless and cacheable; no incremental hook (YAGNI); scalars travel as `Report` metrics or `Json`. | graph | - |
| K82 | Built-in node families; `add(name, **params)`; typed signatures from schema | ADAPT | CLI `info <type_id>` prints the `ParamSpec` table; `--set node.param=value` validates against it. Primitives/transform/merge already live in `vernon-nodes-geo`. | vernon-cli | P2 |
| K83 | Validation before cook: all slots wired, DFS cycle check, type check, order stored | ADAPT | Cycles are rejected at `connect`; kinds only at cook. Add `Graph::validate(&Registry)` (K108). Kahn vs DFS is moot: cook is a lazy pull. | graph | P1 |
| K84 | rayon pool + DashMap scheduler with dirty propagation | REJECT | Cook is recursive and single-threaded with signature caching, no dirty flags. dashmap's newest index entry is 7.0.0-rc2 (pre-release). Parallelise inside nodes with rayon `par_iter`; if graph-level parallelism ever comes, use per-node pending counters (k2's own unfixed bug). | graph | P3 |
| K85 | xxHash128 fingerprint, fixed seed, "identical on any machine" | ADAPT | Current signature is a Merkle chain of `DefaultHasher` over type, params JSON, bypass, upstream signatures: fine in memory, but std does not promise a stable algorithm, so anything persisted needs sha2 0.11.1 (pure Rust) or blake3 1.8.7 (`pure` feature). xxhash-rust is BSL-1.0 and also permissive. | graph, core | P2 |
| K86 | Geometry digest as vertex count + bbox is too weak | ACCEPT | Agree; Vernon chains upstream signatures instead. For goldens and any disk cache add canonical `Geometry::content_hash()` (f64 bits, prims, attribs sorted by name, groups). | core | P2 |
| K87 | Node id is graph-local; parameters outside fingerprint | ADAPT | Vernon hashes params and not ids already. Real gaps: `Path` params hash only the string (edited scan on disk is not noticed), failed cooks incl. `External` are cached, `frame` is fixed 0 and unhashed. | graph | P1 |
| K88 | On-disk cache: Postcard + manifest.json + SHA-256, VERNON_CACHE_DIR | ADAPT | Defer. postcard 1.1.3 is non-self-describing: needs a schema header; key files by signature not node id; atomic writes; use `LOCALAPPDATA`/platform dir, not /tmp. | graph | P3 |
| K89 | Cache hit returns an Arc clone; schema constant bump | ACCEPT | `Data` already holds `Arc<Geometry>`; add a `CACHE_SCHEMA` only when the disk cache exists. | graph | P3 |
| K90 | Python API (VernonNodeGraph, wrappers, exception hierarchy) | REJECT | Deferred with K61; `CookError` is already typed. | - | P3 |
| K91 | tracing logs per node (id, fingerprint, ms, hit/miss) | ADAPT | tracing 0.1.44 (MIT) behind `vernon-cli --verbose`; low priority. | cli | P3 |
| K92 | Node-graph tests (cache hit rate, partial recompute, parallel 1/8, persistence, golden OBJ) | ADAPT | Existing test covers cache hit and param change on a chain. Add a branching test (change branch A, branch B counter unchanged). Reject byte-identical OBJ (compare parsed geometry within tolerance) and the 1/8 speedup assertion. | graph | P1 |
| K93 | CI licence gate: `cargo tree --depth=1` allow-list, `cargo deny --all`, weekly cargo audit | ADAPT | Depth 1 misses transitive crates: use `cargo deny check` (v0.20.2) with a `deny.toml` allow-list (Apache-2.0, MIT, BSD-2/3-Clause, ISC, Zlib, Unicode-3.0, BSL-1.0, CC0-1.0, Unlicense; deny GPL/AGPL/LGPL). deny's advisories check covers cargo-audit. | verify | P0 |
| K94 | White-box harness that runs real tool binaries and records exit code, stdout, output size and SHA-256, runtime | ADAPT | Automate only open-source tools on our own inputs; for licensed proprietary tools capture goldens manually (`CLEAN_ROOM.md` rule 3). Wall-clock goes in the record but never in any comparison. | verify | P2 |
| K95 | Contract JSON per tool with `spec_id` = hash of the contract and `max_memory_mb` | REJECT | Circular (the id sits inside what is hashed) and memory is unenforced. The Rust `ParamSpec` list plus spec doc is the contract. | - | - |
| K96 | (see K59) non-atomic writes | ADAPT | Merged into hazards: add `write_atomic` (temp file in same directory, then rename). Row kept for the id. | io | P1 |
| K97 | Download archives, verify SHA-256, build OSS tools from pinned source | REJECT | Vernon does not fetch tools; the user installs them and Vernon probes `--version`. k2's hashes are placeholders. | - | - |
| K98 | Version drift: compare `--version`, raise VersionMismatch, fall back to built-in | ADAPT | Probe at cook and warn; fallback only where Vernon has its own implementation, else `CookError::NotImplemented`. | graph | P1 |
| K99 | Read source only of open-source tools; use public SDK signatures for proprietary ones | ADAPT | Rule 4 of `CLEAN_ROOM.md` allows open source, but implementers re-implementing an algorithm must not read the GPL/AGPL source of that same tool (contamination); take file formats from documentation. SDK signatures are unneeded. | docs | P1 |
| K100 | Tool descriptors in YAML with yamllint | REJECT | serde_yaml is 0.9.34+deprecated; tool facts live in `NodeTypeInfo.external_tool` and the spec; TOML (toml 1.1.8) if a file is ever needed. "Only documented exit codes" is a good rule and is kept. | - | - |
| K101 | Input corpus: upstream samples, proptest values, broken cases, chained outputs | ADAPT | proptest 1.12.0 as dev-dependency; malformed corpus for readers (truncated, NaN, giant counts). Upstream sample files may carry upstream licences: use only inputs we made (rule 3). | io | P1 |
| K102 | Runner timeouts with the wait_timeout crate | ADAPT | std `try_wait` loop plus `kill` is enough (wait-timeout 0.2.1 is MIT/Apache if wanted). Send stdout and stderr to files: piped output that is not drained deadlocks the child and fakes a timeout. Record `timed_out`. | graph | P1 |
| K103 | Infer contracts from sampled runs | REJECT | Circular as a correctness oracle (records bugs too); fine only for drift detection. | - | - |
| K104 | Golden sets (5 or 10 per tool), Blender writes /tmp/out.png | ADAPT | At least 3 cases per node (nominal, boundary, error); all paths through `ctx.work_dir`; Windows-safe. | verify | P2 |
| K105 | SHA-256 golden plus a tolerance field | ACCEPT | Agree the combination is contradictory: hash only Vernon's own deterministic writers; use a numeric comparator elsewhere. | verify | P1 |
| K106 | `verify_tool` golden rerun at every app start / node execution | REJECT | Slow and says nothing about user data. Provide `vernon-cli selftest` and a CI job. | vernon-cli | - |
| K107 | Validate inputs by JSON Schema and outputs by hash | ADAPT | No schema: validate against `ParamSpec`/`PortSpec`. After cook also check each output `Data::kind()` equals the declared `PortSpec.kind` (only the count is checked now) and run `Geometry::validate()` in strict mode. | graph | P1 |
| K108 | `Tool` trait with a HashMap of named inputs | REJECT | Argument order matters; `NodeOp` already takes an ordered slice. The runner takes `Vec<OsString>`. | graph | - |
| K109 | Add `spec_id` and frame to the node fingerprint | ADAPT | Add optional `NodeOp::signature_salt(&self) -> String` (default empty; external nodes return the probed tool version) and hash `ctx.frame` once animation exists. | graph | P2 |
| K110 | (hazard row) `set_param` accepts anything | ADAPT | From K04/K107: `ParamSpec::validate(&ParamValue)` for type, min/max, finite, enum membership; called by `Graph::set_param`. | graph | P1 |
| K111 | (see K42, K102) process runner | ADAPT | New `ext` module in `vernon-graph`: `run_tool` with ordered args, no shell, outputs to files, timeout kill. | graph | P1 |
| K112 | Load proprietary SDKs with dlopen | REJECT | See K37. | - | - |
| K113 | Open-source binaries separate or dynamically linked; Apache for contracts and golden data | ADAPT | Separate processes only. Golden inputs must be self-made. Apache-2.0 is incompatible with GPL-2.0-only if linked (compatible with GPL-3.0): irrelevant while never linked. | docs | P2 |
| K114 | Exclude undocumented CLI flags to limit copyright exposure | ACCEPT | Sensible scope limit (documented flags only), not a legal conclusion. | docs | P3 |
| K115 | CI matrix with `--features whitebox`, `VERNON_WHITEBOX=1`, nightly full corpus | ADAPT | No such feature exists (`whitebox` is a runtime flag). PR: fmt, clippy, test, deny. Nightly: external-tool goldens where tools are installed. | verify | P1 |
| K116 | 11 person-weeks equals 2.5 months for two people | REJECT | Arithmetic inconsistent and irrelevant. | - | - |
| K117 | Acceptance: every contract exit code observed; corrupted output gives UnexpectedOutput; 2 s golden budget | ADAPT | Corrupted-output test fine. "Every code observed" rejected (sampling). 2 s budget unmeasured. | verify | P3 |
| K118 | Output-path regex rejecting Windows drive letters | REJECT | Use `ParamKind::Path` with an extension filter; no regex. | graph | - |
| K119 | Seven ModifierNode stubs, `register_placeholders()` global, `create_full_pipeline()` | ADAPT | Stage stubs already exist; `Registry` is a value so no global race. Add an integration test that builds the stage chain from the registry, wires output 0 to input 0 where kinds match, cooks, and checks each stage's `Report`. | vernon-cli (tests) | P1 |
| K120 | Swap-in only changes `cook()` | ACCEPT | Agree it is false when output kinds change. Declare final port kinds up front and snapshot every `NodeTypeInfo` (ports, params) in a test so changes are explicit. | graph | P2 |
| K121 | Keep non-geometry results out of `GeometryObject` | ACCEPT | Already `Data::{Report, File, Json}`. | graph | - |
| K122 | Scan stub returns empty cloud; `inputs[0]` panics when unwired | ADAPT | Use `inputs::geometry` (returns `MissingInput`, no panic). Test: cook every node with all inputs `None`; result is `MissingInput` or `NotImplemented`, never a panic. | vernon-cli (tests) | P1 |
| K123 | Surface stub unit cube winding | ADAPT | k2's cube is inverted, Vernon's `make_box` is outward (bottom face normal -Z, existing test). Add outward-orientation test: `signed_volume > 0` for every primitive. | core | P1 |
| K124 | Split via OpenMesh or CGAL | REJECT | C++ / copyleft. | - | - |
| K125 | Slice / Simulate / Solar / Lap stub outputs | ADAPT | Slice gives `File` (G-code path), Simulate `Report` + `Json`, Lap a `Report` metric `lap_time_s` (unit s). Solar: three hourly point samples are not a time average; use cumulative kWh/m2 or mean W/m2 as in `solar-thermal.md`. | nodes-* | P2 |
| K126 | Immediate-mode param panel code | REJECT | Idea (UI generated from `ParamSpec`) already exists; k2's code resets state each frame and does not borrow-check. | - | - |
| K127 | Pipeline integration test (reads `default_output`, missing fixture) | ADAPT | Folded into K119; assert non-empty reports, not merely "not Nothing". | vernon-cli (tests) | P1 |
| K128 | Scaffold is "compile-ready" | REJECT | It is not; nothing adopted. | - | - |
| K129 | `--python-expr` runs arbitrary code | ACCEPT | Valid hazard. Vernon never forwards text to a shell or script: one `Command::arg` per typed argument, reject path params beginning with `-`, treat `.vrn` files as untrusted input. | graph | P1 |
| K130 | Per-feature verification matrix (reference, test type, numeric criterion) | ADAPT | Same shape as the existing G1..G28 tables. Per-format round-trip bound: PLY double exact; OBJ and XYZ shortest-round-trip text exact; STL float32, error at most half an ulp, bounded by abs(x) x 6e-8 (worst case at 5000 mm: 2.4e-4 mm; at 100 m: 0.0039 mm). | io, verify | P1 |
| K131 | OCCT used for a non-linear dynamics module; test names `BRepAPI_Import` | REJECT | OCCT is a B-rep kernel, not a solver; STEP reading is `STEPControl_Reader` (per summariser). | - | - |
| K132 | Lap validation: <= 0.30 s ("0.5%"), Pearson r >= 0.95 | ADAPT | 0.30 s at 0.5% implies a 60 s lap; fix arithmetic. Add slope and RMSE, since r ignores bias. The race-team CFD map is UNVERIFIED. | verify | P3 |
| K133 | Solar tolerance 0.05 kWh/m2-h = "3%"; HDT "NBA test rig" | ADAPT | 0.05 kWh/m2-h = 50 W/m2; 3% would need a 1.67 kW/m2 reference, above the 1041 W/m2 peak in the spec. Compare W/m2 per hour with relative tolerance on cumulative kWh/m2 and an analytic horizontal-plane case. Drop the unexplained rig. | verify | P2 |
| K134 | Flatten validated against libigl ARAP on the bunny | ADAPT | Circular. Use developable ground truth: cylinder and cone unroll (stretch 1.000 within 1e-6), plane identity. libigl is C++ (MPL-2.0, UNVERIFIED): implement from the paper only. | nodes-surface | P2 |
| K135 | loom and ThreadSanitizer for concurrency | ADAPT | Graph is single-threaded: replace with a determinism test (same `content_hash` with `RAYON_NUM_THREADS=1` and default) because parallel float reductions reorder. TSan needs nightly and is not available for the Windows target (memory). | verify | P2 |
| K136 | CI gates: fmt, clippy -D warnings, 95% test pass, tarpaulin 80%, audit, 5% perf gate | ADAPT | Require 100% pass (quarantine flaky). Coverage with cargo-llvm-cov 0.9.1 (tarpaulin is Linux-centric). Shared runners make wall-time gates flaky: assert counts (cook invocations) not seconds. | verify | P1 |
| K137 | Ready-to-paste workflows | REJECT | Write our own. Known defects: unmaintained actions-rs, cache key without `${{ }}`, merged step lines, cache misses `target/`. | verify | P2 |
| K138 | Release job gated on tags but workflow has no tag trigger | ACCEPT | Agree it never runs. Ours: `on.push.tags: ['v*']`, `permissions: contents: write`, `fetch-depth: 0` for the changelog. | verify | P2 |
| K139 | Workspace: nalgebra 0.33, rayon 1.9, `members = [". "]` | ADAPT | Vernon uses glam 0.30 f64 (`DVec3`, `DMat4`) and rayon 1 (1.12.0 available). nalgebra 0.35.0 (Apache-2.0, needs Rust 1.89) only if `vernon-nodes-clean` needs an SVD. Set `rust-version = "1.85"` (eframe 0.32.3 requires 1.85) and check it in CI. | workspace | P1 |
| K140 | Manual release packaging script (bugs) | REJECT | Write `scripts/package.ps1` (build release, zip exe + LICENSE + NOTICE, SHA256SUMS). | verify | P3 |
| K141 | Bundle the OCCT runtime; install OCCT from distro packages or a DLL zip | REJECT | No OCCT. | - | - |
| K142 | Licence position: OCCT "GPL-2.0", "MIT-compatible wrapper", runtime loading avoids GPL linking | ADAPT | OCCT is LGPL-2.1 with an exception (verify the LICENSE text). Runtime loading is not a safe harbour. Apache-2.0 vs GPL-2.0-only incompatibility only matters when linked. | docs | P2 |
| K143 | Compliance tooling: cargo deny, OSS Review Toolkit, CycloneDX, NOTICE incl. Ladybug data | ADAPT | cargo-deny 0.20.2, cargo-cyclonedx 0.5.9, cargo-about 0.9.2; skip ORT (heavy). Do not ship Ladybug or weather data (licence unverified; EPW files are user input). | verify, docs | P2 |
| K144 | Release artifacts and sizes (45/42/48 MB, Docker 140 MB) | ADAPT | Windows zip first, `SHA256SUMS`; sizes unmeasured; no Docker. | verify | P3 |
| K145 | Performance targets 12 s / 4 GB / "12-core i9-12900K" | REJECT | That CPU has 16 cores (8P + 8E); 12 s, 10 s and 9.8 s all used as the bar. No wall-time gates on shared runners. | - | - |
| K146 | Release timeline, rollback, opt-in telemetry | REJECT | No telemetry in Vernon. A clean-machine smoke test is just `vernon-cli selftest`. | vernon-cli | - |
| K147 | mdBook 0.4.37 docs site | ADAPT | mdbook 0.5.4 is MPL-2.0 and only a tool; keep Markdown in-repo first. `cargo doc --open=false` is invalid. | docs | P3 |
| K148 | Code signing, MSI, Homebrew, ghcr, static-link snippet | REJECT | Premature; snippet invalid. | - | - |
| K149 | CFD set-up: kOmegaSST, 30 m/s, TI 5%, stationary no-slip ground, domain 10L/30L/15L, 6000 iterations | ADAPT | `automotive-cfd.md` governs: moving ground and rotating wheels default on, TI 0.25%, half-car, 3L/6L. Stationary ground rejected (ground boundary layer biases lift). Top BC differs between k2's base and troubleshooting steps. | nodes-cfd | P2 |
| K150 | 15 prism layers, 1 mm first layer gives y+ about 1 (reported 0.98 / 1.04) | REJECT | Checked with u_tau = U sqrt(Cf/2), Cf = 0.026 Re^(-1/7), nu 1.5e-5: 1 mm gives y+ 73-94; y+ 1 needs 11-14 um; 0.5 mm gives 36-47. Spec G9 has 13.8 um at L 4.6 m. Reported values are impossible. Gate y+ in 30-300 for wall functions. | nodes-cfd | P1 |
| K151 | Reference lengths and Re: 0.1279 m Ahmed, 2.5 m DrivAer, Re 2.3e6 / 6e6 | REJECT | Re = U L / nu: 0.1279 m gives 2.56e5; Ahmed length 1.044 m gives 2.09e6; DrivAer 4.6 m gives 9.2e6 (spec), 2.5 m gives 5.0e6. Cd uses frontal area as Aref (Ahmed about 0.112 m2), not a length. | nodes-cfd | P2 |
| K152 | Use forceCoeffs not forces; "smoothSolver (GAMG)" mislabel | ACCEPT | Spec already requires forceCoeffs with `Aref = A/2` for half-car and header-based column discovery. | nodes-cfd | P2 |
| K153 | Ahmed Cd 0.311, Cl -0.02; DrivAer Cd 0.235 etc.; Ahmed 1990, Mohan 2001 | REJECT | Contradict the spec's cited TUM DrivAer data (0.275 / 0.319 / 0.277); unverified. Original Ahmed paper is 1984 (SAE 840300, memory); Mohan UNVERIFIED. | verify | - |
| K154 | Verdict calls Cl +10% and +6.7% pass/borderline against a 5% limit; release text says within 1% | ACCEPT | Agree it is wrong. Verdict text must come from `Report::passes()`; add test: `Metric` 0.10 against limit 0.05 gives `Some(false)`. | core, verify | P1 |
| K155 | Laplacian smoothing 3 x 0.3; single refinement for mesh independence | ADAPT | Laplacian shrinks and biases Cd. Taubin lambda/mu (lambda 0.6307, mu -0.6732 gives pass band 1/lambda + 1/mu = 0.1001) or none. Three-mesh GCI is already in the spec. snappyHexMesh is unstructured. | nodes-clean | P2 |
| K156 | CLI snippets with Unicode dashes and typos | REJECT | CI check: fenced code in docs and scripts has no U+2010-U+2015 or U+00A0. | verify | P3 |
| K157 | Cp plot via plot_trisurf; Cp visualisation, troubleshooting remedies | REJECT | Wrong plot (draws a height map over the x-y projection); remedies unvalidated. Vernon stores Cp as prim attribute `cp` (Float) and colours in `Cd`. | - | - |
| K158 | Validation deliverables and per-push CI CFD job | ADAPT | Nightly or on demand with recorded outputs; committing PDFs and logs bloats history. | verify | P3 |
| K159 | StageMetric / StageCheck / PipelineReport model | ADAPT | Use `Report` / `Metric` (String keys). Add `min`/`max` limits (`#[serde(default)]`, old reports still load), `Report::require(&[names])` (missing becomes an Error note, so fail), non-finite metric fails. `PipelineReport` = ordered (node, stage, Report) plus `overall_pass`, `elapsed_s`. | core, graph | P1 |
| K160 | `ModifierNode::metrics()` and `downstream_checks()` | ADAPT | Receiverless fn breaks dyn compatibility. Vernon pattern: producer sets `Metric.limit`; a consumer gate node (Report in, Report out; params metric, min, max) in `vernon-nodes-perf`. | graph, nodes-perf | P1 |
| K161 | Executor aggregation; headless exit codes 0 and 1 | ADAPT | Check every successor; missing metric is FAIL not panic; exit 0 pass, 1 gate failed, 2 usage or cook error. | vernon-cli | P1 |
| K162 | Solar peak check 0..1200 W/m2; remaining-material percent | ADAPT | Warn above 1200, error above 1361 (solar constant) or below 0; metric names carry units; name the percent `remaining_fraction_pct` to avoid inversion. | nodes-solar | P2 |
| K163 | egui dashboard with status colour; Export CSV / JSON | ADAPT | Keep: explicit PASS/FAIL text (not colour only, as the UI spec requires). Code defects rejected. | app | P3 |
| K164 | Dashboard integration test only checks the success path | ACCEPT | Add: out-of-range gives overall fail; missing metric gives fail not panic; branching graph checks all successors; JSON round trip. | vernon-cli (tests) | P1 |

## 3. Bugs and hazards

In k2's claims and code (checked or reported by the summariser; code blocks not reproduced):

- Maths: y+ for a 1 mm layer is 73-94, not 1 (K150); Re for Ahmed at 0.1279 m is 2.56e5 (K151); Catmull-Clark `n` is valence (K68); half-edge cube has 8 vertices (K64); mm to m is x0.001 (K54); 0.30 s is 0.5% of 60 s (K132); 0.05 kWh/m2-h is 50 W/m2 (K133); i9-12900K has 16 cores (K145).
- Process: hash-matching binaries, `ldd` as licence gate, byte-identical IGES/OBJ, "tolerance on a SHA-256", and a CI that fails on any deviation (K05, K09, K36, K105) give false comfort or false failures.
- Fabrication: invented vendor results, approvers, dates and org addresses (K14, K15); invented patent numbers and an unverifiable case (K21, K25).
- Licence: CGAL in an Apache core; OCCT called GPL-2.0; Ladybug MIT; inconsistent core licence and AGPL statements (K30-K34, K142).
- Code that cannot compile or run (shim, FFI, scheduler, egui panel, workflows, scripts) (K71, K84, K126, K128, K137, K140).
- Pipe-capture plus timeout deadlocks; `--python-expr` arbitrary code (K102, K129).

In Vernon, found while reading (each has a test in section 5):

1. `Graph::set_param` / `Params::set` accept any name, type, range, NaN or enum value; `ParamKind` min/max are never enforced. A NaN param also serialises to JSON `null` and fails to reload.
2. `Graph.next_id` is private but serialised: a hand-edited or truncated `.vrn` with a small value makes `add_node` replace an existing node.
3. `Graph::work_dir(node.name)` joins a user-editable name onto the work root: a `.vrn` with a name like `..\x` escapes it.
4. `Graph::from_json` performs no validation (unknown types, dangling wires, bad output index, wrong input count, duplicate names).
5. `check_inputs` runs only at cook; `connect` does not check `DataKind`; cook checks output count but not output kinds, and never calls `Geometry::validate()`.
6. Signatures: `DefaultHasher` is not a stable algorithm (fine in memory only); `ParamKind::Path` is hashed by string only; failed results, including `External`, are cached; `ctx.frame` is not hashed.
7. `Report::passes()` is true for an empty report, and a NaN value without a limit passes; only one-sided limits exist.
8. `vernon_io` PLY reader passes the header-declared element count to `Vec::with_capacity` with no size check (STL is bounds-checked); `io::write` truncates the target via `File::create` before writing (only 3MF is encoded first).
9. `Geometry::triangles()` fans from vertex 0: correct for convex faces, wrong (flipped or overlapping triangles) for concave n-gons; area and Newell normal are unaffected.
10. `Geometry` has no volume, closedness or edge-count helper, and no content hash.
11. `crates/vernon-io/src/lib.rs` declared modules `threemf`, `xml`, `xyz`, `zip` whose files were absent when first read; they exist now (another agent added them), but a green `cargo test -p vernon-io` has not been confirmed by this review.
12. `vernon-cli/src/main.rs` is `fn main() {}`.
13. Workspace `rust-version = "1.80"` is false (eframe 0.32.3: 1.85; kiddo 6.3.0 and nalgebra 0.35.0: 1.89); installed cargo is 1.99.1.
14. Repo root has no LICENSE, NOTICE, `deny.toml`, `.github/`; `docs/research/PROVENANCE.md` does not exist.
15. The trademark denylist test lives only in `vernon-nodes-geo`, and covers labels and help but not `Stage` labels, enum options or port names.

Legal facts checked by search (not legal advice): Lotus v. Borland, 49 F.3d 807 (1st Cir. 1995), affirmed by an equally divided Court, 516 U.S. 233 (1996); Computer Associates v. Altai, 982 F.2d 693 (2d Cir. 1992); Sega v. Accolade, 977 F.2d 1510 (9th Cir. 1992); L'Oreal v. eBay, C-324/09 (12 July 2011); Apple v. Samsung 2012 Fed. Cir. (No. 2012-1105) is a patent preliminary-injunction ruling; SAS Institute v. World Programming, C-406/10 (2 May 2012): functionality, programming language and data-file formats are not protected by copyright in the EU (suggested addition to the docs; supports black-box reimplementation but says nothing about vendor documentation or UI artwork). Google v. Oracle (US Supreme Court, 2021, fair use of API declarations) is from memory, UNVERIFIED. Baker v. Microsoft, UAE Law No. 7 of 2002, all k2 patent numbers: UNVERIFIED.

## 4. Crate vetting

`cargo info` on 2026-10-22. "Pure Rust" means no C/C++ build step in the default configuration (judged from crate description or recalled; not built here).

| crate | version | licence | pure Rust? | verdict |
|---|---|---|---|---|
| glam | 0.30.10 (latest 0.34.1) | MIT OR Apache-2.0 | yes | KEEP (in use; f64 `DVec3`, `DMat4`) |
| rayon | 1.12.0 | MIT OR Apache-2.0 | yes | KEEP; use inside nodes |
| indexmap | 2.14.2 | Apache-2.0 OR MIT | yes | KEEP |
| thiserror | 2.0.21 | MIT OR Apache-2.0 | yes | KEEP |
| serde | 1.0.229 | MIT OR Apache-2.0 | yes | KEEP |
| miniz_oxide | 0.8.9 (as pinned in vernon-io) | MIT OR Zlib OR Apache-2.0 (per Cargo.toml comment) | yes | KEEP |
| eframe / egui | 0.32.3 (latest 0.37.0) | MIT OR Apache-2.0 | yes | KEEP (in use; needs Rust 1.85) |
| nalgebra | 0.35.0 | Apache-2.0 | yes | REJECT for core; optional in nodes-clean for SVD (Rust 1.89) |
| nalgebra-glm / cgmath | 0.21.0 / 0.18.0 | Apache-2.0 | yes | REJECT (glam already) |
| kiddo | 6.3.0 | MIT OR Apache-2.0 | yes | OPTIONAL after bumping MSRV; prefer own grid |
| rstar | 0.13.0 | MIT OR Apache-2.0 | yes | OPTIONAL alternative to kiddo |
| sha2 | 0.11.1 | MIT OR Apache-2.0 | yes | ADOPT for goldens and `content_hash` |
| blake3 | 1.8.7 | CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception | default builds C/asm SIMD; `pure` feature recalled | OPTIONAL |
| xxhash-rust | 0.8.19 | BSL-1.0 | yes | OPTIONAL (permissive) |
| dashmap | 7.0.0-rc2 (pre-release) | MIT | yes | REJECT (not needed) |
| postcard | 1.1.3 | MIT OR Apache-2.0 | yes | DEFER (disk cache) |
| tracing | 0.1.44 | MIT | yes | OPTIONAL (cli) |
| proptest | 1.12.0 | MIT OR Apache-2.0 | yes | ADOPT (dev-dependency) |
| loom | 0.7.2 | MIT | yes | REJECT for now |
| wait-timeout | 0.2.1 | MIT/Apache-2.0 | yes | OPTIONAL (std loop suffices) |
| insta | 1.49.0 | Apache-2.0 | yes | OPTIONAL (dev snapshots) |
| criterion | 0.8.2 | Apache-2.0 OR MIT | yes | OPTIONAL (benches) |
| approx | 0.6.0-rc2 (pre-release only) | Apache-2.0 | yes | SKIP; own `assert_close` |
| clap | 4.6.7 | MIT OR Apache-2.0 | yes | OPTIONAL for cli |
| csv | 1.4.0 | Unlicense/MIT | yes | OPTIONAL |
| toml | 1.1.8+spec-1.1.0 | MIT OR Apache-2.0 | yes | OPTIONAL |
| serde_yaml | 0.9.34+deprecated | MIT OR Apache-2.0 | yes | REJECT (deprecated) |
| jsonschema | 0.60.0 | MIT | yes | REJECT (unneeded) |
| tonic | 0.14.6 | MIT | yes, async stack | REJECT |
| libloading | 0.8.9 (latest 0.9.0) | ISC | yes | REJECT (no plugins) |
| cxx | 1.0.205 | MIT OR Apache-2.0 | no, needs C++ | REJECT |
| pyo3 | 0.30.1 | MIT OR Apache-2.0 | links libpython | DEFER |
| opencascade, opencascade-sys | 0.3.0 | LGPL-2.1 | no, C++ | REJECT |
| meshopt | 0.7.0 | MIT OR Apache-2.0 | wraps C++ (recalled) | REJECT |
| truck-stepio | 0.3.0 | Apache-2.0 | yes (description) | SPIKE for STEP |
| truck-modeling | 0.6.0 | Apache-2.0 | yes (description) | SPIKE only |
| ruststep | 0.4.0 | Apache-2.0 | yes (Rust 1.75) | SPIKE for STEP |
| curvo | 0.3.3 | MIT | yes (nalgebra based) | SPIKE for NURBS |
| gltf / tobj / ply-rs / stl_io | 1.4.1 / 4.0.5 / 0.1.3 / 0.11.0 | MIT or MIT OR Apache-2.0 | yes | NOT NEEDED (vernon-io has own readers) |
| cgal, libigl, open3d | not on crates.io | GPL-3.0+ / MPL-2.0 / MIT (C++, recalled) | no | REJECT |
| cargo-deny (tool) | 0.20.2 | MIT OR Apache-2.0 | yes | ADOPT |
| cargo-audit (tool) | 0.22.2 | Apache-2.0 OR MIT | yes | OPTIONAL (deny covers) |
| cargo-cyclonedx (tool) | 0.5.9 | Apache-2.0 | yes | ADOPT at release |
| cargo-about (tool) | 0.9.2 | MIT OR Apache-2.0 | yes | ADOPT at release |
| cargo-llvm-cov (tool) | 0.9.1 | Apache-2.0 OR MIT | yes | ADOPT (optional) |
| cargo-tarpaulin (tool) | 0.37.5 | MIT OR Apache-2.0 | yes | SKIP (Linux-centric) |
| cargo-nextest (tool) | 0.9.149 (Rust 1.91) | Apache-2.0 OR MIT | yes | OPTIONAL |
| cargo-fuzz (tool) | 0.13.2 | MIT OR Apache-2.0 | needs nightly | Linux CI only |
| maturin / mdbook / cross (tools) | 1.15.0 / 0.5.4 (MPL-2.0) / 0.2.5 | MIT OR Apache-2.0 / MPL-2.0 / MIT OR Apache-2.0 | yes | DEFER / OPTIONAL / SKIP |

## 5. Action list per target crate (each item is a testable change)

### vernon-core
1. Add `Geometry::signed_volume()`, `edge_count()`, `is_closed()`. Tests: `make_box(10,20,30)` volume 6000 within 1e-9; uniform scale 2 gives 48000; reversed winding gives -6000; box V-E+F = 2; every primitive from `vernon-nodes-geo` has positive volume (K64, K75, K123).
2. Harden `Report`: non-finite metric makes `passes()` false; `Report::require(&[&str])` adds an Error note per missing metric; optional `min`/`max` limits with `#[serde(default)]`. Tests: NaN fails; empty report with a required name fails; 0.10 against limit 0.05 gives `Some(false)`; a JSON report from before the change still parses (K154, K159).
3. Add `Geometry::content_hash()` (sha2, canonical: f64 bits, prims, attribs sorted by name, groups). Tests: attribute insertion order does not matter; moving one point by 1e-9 changes it; `make_box(ONE)` has a fixed golden digest (K86).
4. Add the `temp_c` and `cp` attribute names to the standard-attribute doc in `geometry.rs` as constants used by nodes; test that no crate hard-codes a different spelling of `N`, `Cd`, `uv`, `dist`, `part`, `temp_c`.
5. Document or fix `triangles()` for concave faces; test an L-shaped polygon: no fan triangle normal points against the polygon normal (hazard 9).
6. Set workspace `rust-version = "1.85"` and add a CI job `cargo +1.85 check --workspace` (K139).

### vernon-graph
1. `Graph::validate(&Registry) -> Vec<GraphIssue>`; tests flag: unknown type id, dangling source, output index out of range, input count differing from the type, duplicate names, unsafe names, `next_id` not above the maximum id (K83, hazards 2-4).
2. Restrict node names to `[A-Za-z0-9_-]{1,64}` on rename and on load; test: `work_dir` for a name containing `..`, `/` or `\` stays under `work_root` or is rejected (hazard 3).
3. `ParamSpec::validate` called from `Graph::set_param`; tests: unknown name, wrong type, below min, above max, NaN, inf, enum not in options all return `BadParam` and leave the graph unchanged (K04, K110).
4. After cook, compare each output `Data::kind()` with `PortSpec.kind` and call `Geometry::validate()` in strict mode; test a node returning `Report` for a `Geometry` port gives `Failed` (K107).
5. `connect` checks `DataKind` compatibility; test Report output to Geometry input fails at connect.
6. Cache: hash `Path` params by (length, mtime) or content; do not cache `External` errors; add `NodeOp::signature_salt`. Tests: touching the input file recooks once; a failing external node reruns on the next cook; salt change invalidates (K87, K109).
7. `ext::run_tool`: ordered `Vec<OsString>` args, no shell, stdout and stderr to files in `work_dir`, timeout with kill, `timed_out` flag. Tests (self-spawn the test binary): sleeping child killed within 2 x timeout; 10 MB output does not deadlock; argument starting with `-` in a path param is rejected (K102, K111, K129).
8. `pipeline_report(graph, registry, targets) -> PipelineReport` (serde): ordered (node, stage, Report), `overall_pass`, `elapsed_s`. Tests: out-of-range metric fails; missing metric fails without panic; a branch with two successors counts both (K159, K161).
9. Registry tests: every `spec` path exists; every non-whitebox node has one; ids match `stage.verb_noun` with a prefix consistent with `Stage`; snapshot of all ports and params in `tests/snapshots/registry.json` (K39, K120).
10. Branching cache test: two branches off one source; changing a param on branch A recooks only A (K92).

### vernon-io
1. Validate declared counts in PLY (and any text format with counts) against the remaining bytes before `with_capacity`; test a header claiming 4,000,000,000 vertices returns `IoError::Parse` quickly with no abort (hazard 8).
2. `write_atomic` (temp file in the same directory, then rename) used by `write`; tests: success leaves no `.tmp`; an invalid geometry leaves an existing target byte-identical (K59, K96).
3. Reject non-finite coordinates on read with a `Parse` error; test NaN and inf in OBJ, PLY, XYZ (K101).
4. Round-trip tolerance table at car scale (coordinates near 5000 mm and 1e5 mm): PLY exact, OBJ/XYZ exact, STL within abs(x) x 6e-8, 3MF per its text precision; one test per format (K130).
5. proptest properties: OBJ and PLY round trip exactly; `weld_points` is idempotent and keeps surface area within tolerance x perimeter; unit conversion round trip mm to inch to mm within 1e-9 (K101).
6. Truncated and malformed corpus (binary STL/PLY, zip, XML): every case returns an `Err`, none panics (K07, K101).
7. Run `cargo test -p vernon-io` and keep it green now that `threemf.rs`, `xml.rs`, `xyz.rs`, `zip.rs` exist (hazard 11); add a test that every `mod` in `lib.rs` has a round-trip case.
8. STEP spike, time-boxed: write a unit box with truck-stepio, parse it with ruststep, bounding box equals the input within 1e-6 mm; otherwise document the helper-process route (K56).

### vernon-cli
1. Implement `vernon-cli cook <graph.vrn> [--node NAME] [--report out.json] [--set node.param=value] [--out file]`. Tests via `env!("CARGO_BIN_EXE_vernon-cli")`: passing graph exits 0; a failing gate exits 1 and the JSON lists the failing metric; missing file, unknown node or invalid `--set` exits 2; output is written atomically and not overwritten without `--force` (K59, K161).
2. `list [--json]` with the stage x {whitebox, implemented, external} matrix; test row total equals `Registry::len()` and ids are unique (K01).
3. `info <type_id>` prints every `ParamSpec` (name, kind, range, unit); test against two known nodes (K82).
4. `selftest`: cook a built-in box, measure, gate graph, exit 0 (K106, K146).
5. Integration tests that register every node crate: (a) trademark denylist over labels, help, param labels and help, port names, enum options and `Stage::label`, excluding `emulates`, `spec`, `external_tool`; (b) cooking every node with all inputs `None` never panics; (c) stage-chain smoke test with `Report` hand-off (K17, K119, K122, K127, K164).
6. Optional `--verbose` with tracing: one line per node with signature, cache hit/miss, milliseconds (K91).

### docs (CLEAN_ROOM.md, README.md, specs, legal)
1. Add `LICENSE` (Apache-2.0 text) and `NOTICE`; add `deny.toml` (allow-list in K93); CI test: `cargo deny check licenses bans sources` passes and fails when a GPL crate is injected into a scratch workspace (K34, K36, K93).
2. `CLEAN_ROOM.md`: add SPDX and upstream-URL columns and verify each tool once (K30); add the rule that implementers do not read the GPL/AGPL source of the tool they re-implement (K99); add the rule that numbers from model output are never acceptance data unless re-derived (K14); add a provenance-log rule (K09).
3. Create `docs/research/PROVENANCE.md` (date, source URL, what was read, by whom or which model).
4. `docs/legal/notes.md`: verified case list from section 3 with court and year, marked "not legal advice"; K21, K25 and K27 listed as unverified and not to be cited.
5. Spec template additions (K11): reference version, undefined behaviour, edge-case matrix, our UI strings, patent status default `unreviewed`.
6. README: say plainly that third-party solvers are not bundled in 0.x and that the status table is a plan (K28, K60).
7. Before 1.0: trademark clearance for the product name, Nice classes 9 and 42, by counsel (K19).

### verification approach (scripts and CI)
1. `.github/workflows/ci.yml`: windows-latest (GNU target) and ubuntu-latest running `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `cargo deny check`; separate tag-triggered release job with `contents: write` and `fetch-depth: 0` (K76, K115, K136, K138).
2. `scripts/check_clean_room.ps1`: fails on banned extensions, install-folder strings and non-ASCII dashes in fenced code; test by planting a temporary `x.dll` and expecting exit 1 (K09, K156).
3. Shared test comparator (`assert_close` with abs/rel tolerance; geometry compare via vertex distance and P95 of `dist`): no exact float equality in golden tests (K05, K16, K105).
4. Determinism tests: cook the same graph twice and with `RAYON_NUM_THREADS=1` versus default; `content_hash` identical (K135).
5. External-tool goldens (nightly, only where the tool is installed): at least three cases per node, wall-clock recorded and excluded from comparison, version string stored (K94, K104).
6. Remove or re-derive the numbers listed in K14, K132, K133 and K153 wherever they reach any spec or test.

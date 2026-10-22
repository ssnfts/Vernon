# k2 review vetting: capture-clean (vernon-nodes-capture, vernon-nodes-clean, vernon-io)

Vetted 2026-10-22 against: CLEAN_ROOM.md, vernon-core `geometry.rs`/`report.rs`, vernon-graph `node.rs`/`param.rs`/`data.rs`, `docs/specs/capture-scanning.md`, `docs/specs/alignment-deviation.md`, crate manifests (Cargo.toml files and `cargo info`). `registry.rs`, `attrib.rs`, `graph.rs` and the vernon-io sources were not read. Status of vernon-io is taken from its Cargo description and file list (obj, ply, stl, weld, XYZ/PTS, 3MF). Legal items are marked UNVERIFIED where only secondary sources were found. k2 code is not reproduced.

## 1. Summary

1. k2 reviewed an imagined Vernon (`ModifierNode`, `GeometryObject`, `store_aux_output`, `CoreEvent`, `PointCloud`/`TextFile` types, file-path ports). The real API is `NodeOp::cook(ctx, inputs, params) -> Vec<Data>` over one generic `Geometry`, plus `Report` metrics for gating. Almost none of its code ports. Its ideas and its self-found bugs are the useful part.
2. Verdicts over 68 claims: 10 ACCEPT, 28 ADAPT, 30 REJECT. The REJECTs are mainly the C++ OpenVDB shim (no C++ deps), the ZEISS/E57 `libloading` plans, hand-rolled importers vernon-io already has, and the fabricated-statistics stub.
3. Real maths corrections to k2: the percentile index is off from nearest-rank (spec 3.5); ICP rows must pair `[p x n, +n]` with `b = n.(q-p)` (or `[n x p, -n]` with `r = n.(q-Tp)`); a centroid KD-tree is not a nearest-triangle search; ray parity is the wrong sign rule (use pseudo-normals); the stop rule must be RMS change, not RMS below a tolerance; the translation of a scale-bar fit is `target_A - s R p_A`.
4. Licence claims are mostly wrong or stale: `ply` is not BSD-3 v0.6; `obj` and `stl_io` versions are wrong; libE57Format is BSL-1.0, not BSD-3; OpenVDB is not BSD-3 (MPL-2.0 before 12.0, reported Apache-2.0 from 12.0, UNVERIFIED against the LICENSE file); CloudCompare is GPL-2.0+ per CLEAN_ROOM.md, not GPL-3.0. Pure-Rust `e57` and `las`/`laz` (all permissive) replace the C++ E57 plan.
5. Vernon-side hazards found while vetting: spec says `dev`/`dev_valid`, `geometry.rs` standardises `dist`; `rust-version = 1.80` blocks nalgebra 0.35 and kiddo 6.3 (MSRV 1.89), though the installed toolchain is 1.99.1; `Geometry` has no unit field, so a `metric` detail attribute is needed before any mm-based clean node; `Geometry::triangles()` is a plain fan, wrong for concave polygons.

## 2. Claim table

Priority: P0 = blocks honest gating or violates policy, P1 = core of the stage, P2 = useful, P3 = later or optional.

| id | claim (short) | verdict | adaptation / reason | target | pri |
|---|---|---|---|---|---|
| K01 | Insert `capture_clean` step Scan -> Clean -> Surface, display name with vendor names | ADAPT | Order already encoded by `Stage::Capture/Clean/Surface`. Replace the single stub by separate `clean.*` nodes (ids `stage.verb_noun`). Vendor names in a label violate CLEAN_ROOM; use "Remove Outliers" etc. and put tool names only in `emulates`. `unwrap()` on construction must become a `Result` | clean | P0 |
| K02 | SOR, voxel downsample, radius filter, optional "ZEISS-style" smoothing | ADAPT | Three separate nodes. SOR needs `k` (Int, default 16, 3-200) and `sigma` (Float, default 2.0, 0.5-10); threshold = mu_g + sigma*s_g over the per-point mean k-NN distances (Rusu et al. 2008). Voxel size mm default 1.0, 0.01-100 (k2's 5.0 cap too small for photogrammetry). Smoothing becomes a neutral `clean.denoise_points` (feature-preserving bilateral/MLS), off by default, no vendor name in param names or labels | clean | P1 |
| K03 | voxel_size in mm but nothing records units | ADAPT | Concern valid. Convert at import (`capture.import_scan` unit param, spec 2.2 size check). Mark `Geometry` with detail attrib `metric` (Int 0/1). `clean.*` refuse `metric=0` with an Error note (spec section 4 note). Guard: warn if voxel > bbox diagonal/2, Info if removed fraction is 0 | capture + clean | P0 |
| K04 | I/O contract: path in, `_clean.ply` path out, stats JSON | ADAPT | Wires carry `Geometry`, not paths. File I/O only in `capture.import_scan` (`ParamKind::Path` with extensions xyz/pts/asc/ply/las/laz/e57/obj/stl/3mf). Outputs: `Geometry` + `Report` (`kept_points`, `removed_points`, `removed_fraction` %, `voxel_size_used` mm). Optional `Json`. `.obj` accepted only when it has no faces (then `is_point_cloud()`). Smoothing iterations is a param | capture, clean | P1 |
| K05 | Drive CloudCompare CLI for real cleaning | ADAPT | CLEAN_ROOM lists it only for optional reference checks. Native Rust is primary; CLI only as a golden cross-check behind env `VERNON_CLOUDCOMPARE`. k2's reviewer is right that `--STATISTICAL_OUTLIER_REMOVAL` is not CLI syntax: wiki/forum show `-SOR <knn> <sigma>` and `-SILENT` first. `-SS` and `-C_EXPORT_FMT` details UNVERIFIED; check `-h` of the installed version | clean tests | P3 |
| K06 | Stub spec: halves points, writes `_clean.ply`, returns stats | REJECT | Hard-coded stats (1,000,000 / 500,000) are fabricated numbers; they poison Report gating. A white-box stub (`whitebox=true`) must pass geometry through and compute counts from the real input, with an Info note "no points removed" | clean | P0 |
| K07 | Stub cook() does not compile (maps, tokens) | REJECT | Code unusable and written to a non-existent `ModifierNode`. Nothing to port | clean | P3 |
| K08 | `store_aux_output` / `peek_output` for extra results | REJECT | No side channel exists. A node returns exactly one `Data` per declared output port, in order (`Geometry`, `Report`, `File`, `Json`). Add ports instead | graph contract | P2 |
| K09 | End-to-end test Scan -> clean -> Surface with `dummy.xyz` | ADAPT | Build the cloud in memory (plane, 100k points, noise 0.035 mm, 500 injected outliers at known indices); assert exact removed/kept counts and `geometry.validate()`; then feed Surface and assert the Report. A test that checks only names/types is useless, as k2 says | clean tests | P1 |
| K10 | White-box contract: corpus, hash outputs, `contracts/clean.json` | ADAPT | Agree that byte SHA-256 of a cloud is brittle. Compare counts, bbox, Report metrics with tolerances; for CloudCompare cross-check compare removed-index sets (>= 0.99 overlap), never file hashes. Golden sizes 100/10k/100k/1M kept | clean tests | P2 |
| K11 | CI builds CloudCompare from source on every PR | REJECT | Slow, needless, GPL build in CI. Use a pinned release binary in a nightly/manual job; contracts updated only by an explicit bless flag, read-only in PR CI | CI | P3 |
| K12 | Map non-zero exit to `ToolFailed`, emit `ToolRunning` progress | ADAPT | Use `ctx.fail(msg)` (`CookError::Failed`) with stderr tail attached. `CoreEvent`/`ToolRunning` do not exist; `cook` is synchronous with no progress channel (see K31). Child pipes on reader threads: accepted | capture, graph | P1 |
| K13 | Schema-driven panel; file picker for FileExtension | ACCEPT | Already the design: `NodeTypeInfo` + `ParamSpec`; `ParamKind::Path{extensions, save}` is the picker mapping. Add a test that every `ParamKind` variant has a widget (app side) | graph/app | P3 |
| K14 | Registry with internal id separate from label | ADAPT | Right idea; Vernon already has `id` vs `label` in `NodeTypeInfo`. Use `clean.outlier_remove`, not `capture_clean`. Concurrency guard unneeded if registry is built at startup (registry.rs not reviewed) | clean | P2 |
| K15 | Licensing: Apache core, GPL tools as separate processes | ADAPT | Consistent with CLEAN_ROOM (CloudCompare GPL-2.0+, not 3.0; no OpenCascade/LGPL shim exists in the repo). FSF GPL FAQ treats exec/pipes/args as usually separate programs but says intimate data exchange can combine them (secondary sources only; UNVERIFIED primary). Keep exchange to files and argv. Needs counsel before shipping | docs | P2 |
| K16 | Load ZEISS-style library via `libloading` | REJECT | No public SDK shown; proprietary binary/plug-in is forbidden by CLEAN_ROOM; vendor name. libloading (ISC) is not the issue | none | P0 |
| K17 | Add crates obj 0.3, stl_io 0.8, ply 0.6, libloading, anyhow | REJECT | vernon-io already has hand-written OBJ/PLY/STL/3MF/XYZ with one dep (`miniz_oxide`). Versions and licences are wrong (section 4). anyhow is in the workspace but libraries use `thiserror` | vernon-io | P1 |
| K18 | `PointCloud{Vec<Point3>, HashMap<f64>}` and `TextFile` types | REJECT | `Geometry` already is the point cloud (`is_point_cloud()`), `DVec3` f64 points, typed `AttribTable`, `Cd`. f64 positions are required (f32 spacing near 4500 mm is 2^-11 = 0.00049 mm, same size as the effect measured; alignment spec section 1). Units go in the `metric` detail attrib | core | P1 |
| K19 | PLY importer code | REJECT | Importer exists. k2's bugs become tests: per-vertex rows, quads/n-gons, `uchar` counts with int/uint indices, binary big/little endian, extra properties, truncated file returns `Err` | vernon-io tests | P1 |
| K20 | OBJ importer: quads split, >4 rejected | REJECT | Keep polygons as `Prim::polygon` of any size. Hazard in Vernon: `triangles()` is a fan, wrong for concave n-gons; add an ear-clip triangulate step if needed (P3). Test relative (negative) indices, `v` without `f`, >65535 vertices | vernon-io, core | P2 |
| K21 | STL: weld vertices, ignore facet normals | ACCEPT | Matches `weld.rs`; ignore file normals, recompute `N`. Test: cube STL (12 triangles) -> 8 points; binary and ASCII agree | vernon-io | P1 |
| K22 | E57 via `libloading` + C++ library; stub returns 10k synthetic points | REJECT | libE57Format is C++ (no C ABI), as k2 notes. Use pure-Rust `e57` crate (MIT). A stub returning invented points is fabricated data and must never exist | vernon-io | P1 |
| K23 | E57 SDK is BSD-3 (RIEGL) | REJECT | k2's own doubt is right: libE57Format is Boost Software License 1.0 per its README (secondary search; LICENSE file not read). Moot with the `e57` crate | docs | P3 |
| K24 | Single Import node dispatching on extension | ADAPT | Use `capture.import_scan`: extension plus magic sniffing (`ply`, `LASF`, `ASTM-E57`, STL ASCII vs binary size check). Missing file or bad header -> `ctx.fail` with path and reason, no panic (inputs helpers already return `MissingInput`) | capture | P1 |
| K25 | `egui_file_dialog` for the picker (optional) | ACCEPT | `egui-file-dialog` 0.15.0 (MIT) or `rfd` 0.17.2 (MIT, native dialogs). App-level only, not node crates | app | P3 |
| K26 | `automatic_reconstructor --workspace_path --image_path [--dense]`, `COLMAP_BIN` | ADAPT | Bare `--dense` suspect; spec 2.5 lists `--quality`, `--data_type`, `--mesher`. Prefer staged drv nodes (feature_extractor, matcher, mapper) for progress/resume; read option names from `-h` at runtime. Env var `VERNON_COLMAP` | capture | P2 |
| K27 | `remove_dir_all(output_dir)` before each run | REJECT | Data-loss hazard. Work only inside `ctx.work_dir`; refuse a non-empty workspace unless `overwrite` Bool is set (COLMAP resumes databases anyway, spec 2.5) | capture | P0 |
| K28 | Read `sparse/0/points3D.txt`, fall back to `dense/0/fused.ply` | ADAPT | Spec 2.5: `sparse/0` holds `.bin` models; txt is usually absent. Read `points3D.bin` natively (documented little-endian layout [M, not re-fetched]) and `.txt`; or run `model_converter --output_type TXT`. Dense fallback uses the existing PLY reader. Output `metric=0` | vernon-io, capture | P2 |
| K29 | points3D parser splits on commas, `unwrap_or(0.0)` | ACCEPT | Real bug. Rows are whitespace-separated: `POINT3D_ID X Y Z R G B ERROR TRACK[]`, `#` comments. Parse with `split_whitespace`, return errors, never default to 0.0 (points would pile at the origin). Map RGB/255 to `Cd`, error to a Float point attrib | vernon-io | P1 |
| K30 | Store COLMAP log as aux output | ACCEPT | Capture stdout and stderr (two reader threads), write a log file in `ctx.work_dir`, put the tail into the error on failure and an Info note on success. k2 is right that most messages go to stderr (not re-verified) | capture | P1 |
| K31 | Stream output on a thread, progress events, cancel | ADAPT | Principle accepted (runs take hours). `cook` has no progress/cancel channel: add an optional cancel flag and progress callback to `CookContext` (vernon-graph change), keep the child handle to kill on cancel. Until then: spawn, read pipes, kill on drop | graph, capture | P2 |
| K32 | Optional inputs read by position (index 1, 2) | REJECT | Positional optional inputs shift. Options belong in `Params` (named); only real data ports are inputs (`PortSpec::optional`). `ParamKind::Path` has no directory mode, so use a `Str` or default `ctx.work_dir/colmap` | capture | P1 |
| K33 | Dense: patch_match_stereo + stereo_fusion triggered by flag | ACCEPT | Add `image_undistorter` first (spec 2.5). Needs CUDA: if absent, fail with a clear message or return the sparse result with a Warning note. Spec node `capture.mvs_dense` | capture | P2 |
| K34 | Expose SIFT / SIFT_ORB, camera_model, max_num_features | ADAPT | `SIFT_ORB` is not a type in spec 2.5 (SIFT, ALIKED_N16ROT, ALIKED_N32, LOMA_B...). Enums built from `-h`, default SIFT / SIMPLE_RADIAL. `SiftExtraction.max_num_features` (8192) is accepted; the size option drifts between `SiftExtraction.max_image_size` and `FeatureExtraction.max_image_size` | capture | P2 |
| K35 | Parallel COLMAP runs on rayon | REJECT | Oversubscribes CPU/GPU. Serialise external-tool nodes with a process-wide limit (1 per GPU) in the executor | graph | P2 |
| K36 | Separate-process architecture; YAML descriptors + white-box harness | ADAPT | Separate processes accepted. No second source of truth: derive argv from the node's `ParamSpec` names in one Rust `ToolSpec` table per tool, and test argv snapshots (spec test 20). Contracts as committed JSON are fine | capture, clean | P2 |
| K37 | Importer unit tests on tiny fixtures | ACCEPT | Assert counts and bounds, not just the variant: cube = 8 points, 6 quads / 12 triangles, bbox exact; E57 test must use a real file written by the `e57` writer | vernon-io | P1 |
| K38 | Mock COLMAP executable via env var; CI on 3 OSes | ACCEPT | Mock must write real whitespace-format text and a real `.bin`. On Windows use a small helper binary built by the test crate, not a script. Three-OS matrix is P3 | capture tests | P2 |
| K39 | Write per-format docs with licence notes | ACCEPT | Licence notes from section 4 of this file, taken from `cargo info`, not from k2. Put in `docs/specs` or `docs/formats.md` | docs | P3 |
| K40 | One `align_deviation` node; PNG/CSV/JSON via events | ADAPT | Split into the spec's nodes: `clean.pair_align`, `clean.icp_align`, `clean.mesh_distance`, `clean.cloud_distance`, `clean.deviation_stats`, `clean.deviation_histogram`, `clean.deviation_colors`, `clean.tolerance_gate`. Outputs: `Geometry` (with `dist`), transform as `Json`, `Report`. Histogram as `Json`, CSV as `File`. Declared outputs must equal returned `Data` count (k2's undeclared `histogram_csv` is exactly the bug) | clean | P1 |
| K41 | nalgebra 0.33, kiddo 2.5, plotters 0.3 | ADAPT | Versions stale: nalgebra 0.35.0 (Apache-2.0), kiddo 6.3.0 (MIT OR Apache-2.0); both MSRV 1.89 > workspace 1.80. Prefer a small in-house `linalg` (3x3 Jacobi SVD, 6x6 SPD solve) and grid/BVH index; adopt nalgebra/kiddo only after deliberately bumping `rust-version`. plotters: REJECT (image/font stack; histogram is data, drawn by the UI) | clean | P1 |
| K42 | `Point3` struct with From conversions | REJECT | Use `glam::DVec3` (Copy, already the point type). k2's derive/Copy bugs are moot | core | P3 |
| K43 | Scale-bar coarse alignment: s = D / dist(pa,pb), axis-angle onto +X | ADAPT | Scale formula correct. Rotation (axis = v x X, angle = acos) correct but degenerate when parallel/antiparallel (cross ~ 0): handle explicitly; error if dist(pa,pb) < 1e-9. Translation is `target_A - s R p_A` (k2's note right). One bar fixes scale and one direction only: spec splits it into `capture.scale_from_distance` (scale) and `clean.pair_align` (>= 3 pairs, Umeyama, collinear -> typed error) | capture, clean | P1 |
| K44 | Gauss-Newton point-to-plane ICP | ADAPT | Algorithm class right. Consistent rows: `[p x n, +n]` with `b = n.(q-p)` (spec 3.2), or `[n x p, -n]` with `r = n.(q-Tp)`; k2 mixed them. Update `R' = R_inc R`, `t' = R_inc t + dt` is right. Stop on `abs(rms_k - rms_k-1) < rms_delta` (default 1e-6 mm) or step < 1e-9, NOT "RMS < 0.01 mm" (scan noise ~0.03 mm means it never triggers). Solve by Cholesky/LDLT, report condition number; > 1e8 = sliding degeneracy, name free direction. Centre coordinates first (f64) | clean | P1 |
| K45 | Nearest mesh vertex + its face normal | ADAPT | Agree it is crude. Use closest point on triangle with interpolated `N` or the plane of the hit triangle; BVH, not 3N duplicated vertices | clean | P1 |
| K46 | Coarse transform applied twice | ACCEPT | Real bug. ICP starts from the initial pose and returns an absolute pose; apply once with `Geometry::transform`. Test: `pair_align -> icp_align` result equals the known transform to 1e-9 | clean | P0 |
| K47 | Ericson closest-point-on-triangle | ADAPT | Reference method fine (Ericson 5.1.5; spec 3.4). Return unsigned distance, closest point and region (face/edge/vertex); vertex branch is `abs(p-a)`, not `abs(p)`. Sign comes from pseudo-normals, not the face plane value | clean | P1 |
| K48 | Sign by +X ray parity (Moller-Trumbore) | REJECT | Fails on open meshes and exact edge hits; O(points x triangles). Use angle-weighted pseudo-normal (spec 3.4). Parity ray (jittered direction, u>=0, v>=0, u+v<=1) only as optional watertightness check. Sign is positive along outward normal, so "inside = negative" only holds for closed outward-oriented meshes | clean | P1 |
| K49 | Nearest triangle via KD-tree on centroids | REJECT | Nearest centroid is not nearest triangle. Use AABB BVH, nearest-first with pruning (spec 3.4). Own ~200 lines; `bvh` 0.13 (MSRV 1.90) or `parry3d-f64` 0.31.1 (heavy) are fallbacks | clean | P1 |
| K50 | Histogram: bins anchored at data min, PNG via plotters | ADAPT | Spec 3.5: B equal bins over `[lo,hi]`, symmetric range option so 0 is an edge, last bin right-closed, integer-micrometre tests. Reject bin width <= 0 as `BadParam` (a `ParamSpec` min does not by itself guarantee it); empty or all-NaN input -> Error note, no panic; NaN excluded and counted invalid. PNG: REJECT, output `Json` + optional CSV `File` in `ctx.work_dir` | clean | P1 |
| K51 | p95 of absolute deviation, JSON keys | ADAPT | Use nearest rank: sort ascending, 1-based `ceil(q N)` (0-based `ceil(qN)-1`), `total_cmp` with NaN filtered. k2's `ceil((n-1)*0.95)` differs: for N = 1000 it returns 0.951 where spec test 11 requires 0.950. Emit Report metrics `p95_abs` (mm, `with_limit(tol, true)`) and `fraction_within` plus `deviation_report.v1` Json | clean | P0 |
| K52 | Alignment e2e test + UI smoke test | ADAPT | Use `Geometry::make_box` and analytic expectations (k2's cloud at z=20 vs faces at +-0.5 is wrong). Write files only under `ctx.work_dir` (no `/tmp`). Reuse spec golden tests 1-14 | clean tests | P1 |
| K53 | "Swap in a full CloudCompare call" for ICP; "all MIT/Apache" | REJECT | GPL-2.0+ per CLEAN_ROOM, allowed only as an optional separate-process reference check. A core stage must not depend on it | clean | P0 |
| K54 | Future: faster ICP, click-pick scale points, PNG in egui, cache distance structure | ADAPT | GPU ICP: out of scope. Picking in viewport: P2, emits `pairs` Json for `clean.pair_align`. PNG: REJECT. Caching: nodes are stateless; cache lives in the graph cook cache, not in the node. "Rust port of Open3D" and "pointclouds-rust": UNVERIFIED, not searched | render/graph | P3 |
| K55 | C ABI for OpenVDB shim (verts f64, tris u32, malloc/free) | REJECT | C++ shared library = forbidden dependency and unsafe FFI. Implement natively. k2's FFI hazards (malloc(0), cross-CRT free, missing `__declspec(dllexport)`) are accurate but moot | clean | P0 |
| K56 | Runtime-loaded `libvdb_shim`, `Lazy` loader | REJECT | Same. Also compile-time `CARGO_MANIFEST_DIR` path breaks packaged builds | clean | P0 |
| K57 | CMake build with install into `src/` | REJECT | No shim | none | P3 |
| K58 | `build_vdb_shim.sh` + CI step | REJECT | No shim; the script's `-S`/`$` errors confirm it was never run | none | P3 |
| K59 | Placeholder `vdb_shim.cpp` | REJECT | Does not compile as k2 admits | none | P3 |
| K60 | Decimate by triangle stride | REJECT | Overshoots (9 tris at 0.5 -> target 4, stride 2, keeps 5) and punches holes. A stub must pass through with a Warning note; real node is QEM edge collapse (Garland-Heckbert 1997) with boundary weighting | clean | P1 |
| K61 | meshToVolume -> narrow-band -> volumeToMesh as decimation | REJECT | That is remeshing, loses sharp features, and `tools::decimate`/`narrowBand` names are UNVERIFIED. Use QEM | clean | P2 |
| K62 | Watertight wrap: mesh -> level set -> iso-surface; voxel 0.01-10, isovalue 0-1 | ADAPT | Concept accepted. Native sparse narrow-band SDF (sign by pseudo-normal) + marching cubes. `voxel_size` is the voxel edge in mm; `offset` is a signed mm shrink/grow, not 0-1. Refuse if estimated band voxels exceed a cap (4500 mm / 1 mm dense = 4.5e9). Report `open_edges` (limit 0), `volume` mm^3 | clean | P2 |
| K63 | Point cloud input: fan of triangles from point 0 | REJECT | Not a surface. Wrap accepts a mesh only; clouds go through the surface stage | clean | P2 |
| K64 | Decimate/shrink-wrap tests on a unit cube | ADAPT | Cube is a minimal mesh. Use a UV sphere (~5k triangles, r = 50 mm): decimate to 50% gives triangles within +-2% of target, closed, no flipped faces, `max dist` below a stated bound; wrap at voxel 1 mm gives V-E+F = 2, `open_edges` 0, radius error <= 1 voxel. Count assertions must not hard-code OpenVDB-like counts | clean tests | P2 |
| K65 | `to_flat`/`from_flat` helpers | REJECT | Only exist for FFI. `Prim.vertices` is already `u32`; if conversion is ever needed use `u32::try_from` | core | P3 |
| K66 | OpenVDB is BSD-3 | REJECT | Wrong. MPL-2.0 up to 11.x; one secondary source says 12.0 relicensed to Apache-2.0 (UNVERIFIED; LICENSE not read). crates.io `openvdb` 0.1.0 is a BSD-3 placeholder crate, not bindings. Moot: no OpenVDB | docs | P3 |
| K67 | Register `decimate`/`shrinkwrap` in `register_placeholders` | REJECT | No such function; ids are `stage.verb_noun`; trait is `NodeOp`. k2's own two replies use incompatible module paths | clean | P3 |
| K68 | Both replies compile-ready, tests pass | REJECT | Contradicted by k2's own notes (undefined variables, commented-out declarations, wrong crate APIs). Treat as a design sketch | all | P3 |

## 3. Bugs and hazards (k2 claims and Vernon itself)

k2 maths and logic (checked by hand):
- Percentile: nearest-rank 0-based index is `ceil(qN)-1`. N = 20: spec index 18, k2 `ceil(19*0.95)` = 19 (the maximum). N = 1000: 0.950 vs 0.951.
- ICP sign pairing (K44). With `r = n.(q-Tp)`, d r/d rot = `n x p` and d r/d t = `-n`. Mixing `n x p` with `+n` moves translation the wrong way, as k2 itself noted.
- Scale from a bar: `s = D / dist(pa,pb)` correct; translation must subtract the transformed anchor, `t = target_A - s R p_A`.
- ICP stop rule uses an absolute RMS threshold smaller than scan noise, so it never fires; use RMS change.
- k2 claims "a naive centroid KD-tree" gives a cloud-to-mesh deviation; it overstates distances for large or elongated triangles.
- Voxel size: 0.5 "units" on a metre-scale COLMAP cloud erases most detail; sparse-cloud scale is arbitrary until a scale node runs (spec: `metric=false`).
- Dense COLMAP chain without `image_undistorter` fails; `SIFT_ORB` is not a listed feature type.
- Report fabrication in the stub (K06) is worse than a compile error: it would pass downstream gates on invented numbers.
- `rmse` on a linearised residual after the update is not the convergence measure; compute RMS over real correspondences each iteration.

Vernon-side hazards:
- Naming split: `geometry.rs` standard point attribute is `dist` (signed mm); `docs/specs/alignment-deviation.md` says `dev`, `dev_valid`. Decide on `dist` plus `dist_valid` (Int) and fix the spec.
- No unit/frame field on `Geometry`; spec says stage-2 nodes should refuse `metric=false` but nothing stores the flag. Add the `metric` detail attribute convention (document in core crate docs).
- `Geometry::transform` builds `m.inverse()` for normals; a zero or negative scale (user error in a scale node) yields NaN normals. Scale nodes must reject `s <= 0`.
- `Geometry::triangles()` fan-triangulates every closed prim; wrong for concave polygons (OBJ/PLY n-gons) and would corrupt BVH distances.
- `ParamSpec` min/max are declared but this review did not confirm cooking enforces them; nodes should validate and return `CookError::BadParam` (histogram bin width, k, sigma, voxel).
- `ParamKind::Path` has no directory mode (COLMAP workspace).
- `cook` is synchronous with no cancel/progress token; hours-long external tools block a worker (K31).
- Workspace `rust-version = "1.80"` vs MSRV of kiddo 6.3 / nalgebra 0.35 (1.89) / `bvh` 0.13 (1.90). Toolchain installed is rustc 1.99.1, so a bump is feasible but must be a conscious decision.
- External-tool licence boundary (GPL tools by argv/files) is plausible but UNVERIFIED legally; keep data exchange to files, get counsel review before a release.
- COLMAP option names drift (spec section 6): never hard-code, introspect `-h`.
- Vendor names appear in k2's proposed display names and parameter names ("CloudCompare/ZEISS stub", `apply_zeiss_smoothing`); none may reach labels, help text or parameter labels.

## 4. Crate vetting

Checked with `cargo info` (version, licence, MSRV) and the downloaded manifests (build script, `cc`/`links`). "Pure Rust" = no C/C++ code compiled or linked by default.

| crate | version | licence | pure Rust? | verdict |
|---|---|---|---|---|
| obj | 0.10.2 (k2: 0.3 MIT) | Apache-2.0 | yes | REJECT: vernon-io has OBJ |
| obj-rs | 0.7.4 | Apache-2.0 OR MIT | yes | REJECT: not needed |
| stl_io | 0.11.0 (k2: 0.8) | MIT (MSRV 1.85) | yes | REJECT: own STL + weld |
| ply | 0.1.0 (k2: 0.6 BSD-3) | MIT | yes (nom) | REJECT: k2's description does not match any such crate |
| ply-rs | 0.1.3 | MIT | yes (peg; build.rs is dev-only skeptic) | REJECT: own PLY |
| ply-rs-bw | 4.0.1 | MIT (MSRV 1.87) | yes | REJECT: own PLY |
| libloading | 0.8.9 (latest 0.9.0) | ISC | yes (FFI loader) | REJECT: no runtime-loaded vendor/C++ libs |
| anyhow | 1.0.104 | MIT OR Apache-2.0 | yes | ACCEPT for app/CLI only; libraries use thiserror |
| nalgebra | 0.35.0 (k2: 0.33) | Apache-2.0 (MSRV 1.89) | yes | ADAPT: defer; needs MSRV bump; in-house small solver first |
| kiddo | 6.3.0 (k2: 2.5, API differs) | MIT OR Apache-2.0 (MSRV 1.89) | yes by default (`cc` only under opt-in `cpp_competitors`; build.rs detects cache line) | ADAPT: defer, or own grid/BVH index |
| rstar | 0.13.0 | MIT OR Apache-2.0 (MSRV 1.85) | yes | candidate for kNN/radius queries |
| kdtree | 0.8.1 | MIT OR Apache-2.0 | yes | fallback only |
| bvh | 0.13.0 | MIT (MSRV 1.90) | yes | candidate; own BVH preferred |
| parry3d-f64 | 0.31.1 | Apache-2.0 | yes | candidate; heavy, own BVH preferred |
| spade | 2.15.1 | MIT OR Apache-2.0 | yes | candidate for the triangulation local model (wb) |
| plotters | 0.3.7 | MIT | Rust, but default stack pulls image/font crates | REJECT: histogram is data |
| base64 | 0.23.1 (k2: 0.21) | MIT OR Apache-2.0 | yes | not needed (no PNG-in-JSON) |
| once_cell | 1.21.4 | MIT OR Apache-2.0 | yes | REJECT: use `std::sync::LazyLock` (stable since 1.80) |
| libc | 0.2.191 | MIT OR Apache-2.0 | yes | not needed |
| e57 | 0.11.13 | MIT | yes (roxmltree, optional crc32c) | ACCEPT as optional vernon-io feature |
| las | 0.11.1 | MIT | yes | ACCEPT as optional vernon-io feature |
| laz | 0.13.0 | Apache-2.0 | yes (optional rayon) | ACCEPT with `las` feature `laz` |
| egui-file-dialog | 0.15.0 | MIT | yes | ACCEPT, app crate only |
| rfd | 0.17.2 | MIT | build.rs; native toolkits (GTK/portal on Linux) | alternative, app crate only |
| rayon | 1.12.0 | MIT OR Apache-2.0 (MSRV 1.80) | yes | ACCEPT; already a workspace dep |
| openvdb (crates.io) | 0.1.0 | BSD-3-Clause | "placeholder" bindings | REJECT |

Non-crate licence claims: CloudCompare GPL-2.0+ (CLEAN_ROOM.md), libE57Format BSL-1.0 (README via search, LICENSE file not read), OpenVDB MPL-2.0 up to 11.x and Apache-2.0 from 12.0 per one news report (UNVERIFIED), GPL "separate program" boundary (FSF FAQ, secondary quotations only; UNVERIFIED), `-SS` and `-C_EXPORT_FMT` CloudCompare syntax (UNVERIFIED).

## 5. Ordered actions per target crate

### vernon-core (supporting, outside the three targets)
1. Document the `metric` detail attribute (Int 0/1) and unit convention in crate docs; test that `merge` keeps it zero-filled consistent and `validate()` passes.
2. Reconcile `dist`/`dist_valid` with the alignment spec (edit the spec); add a `fan triangulation is only valid for convex polygons` doc line and a test that a concave L-shaped polygon is flagged by a `triangulate` helper (P3).

### vernon-io
1. Add tests for existing readers: cube PLY (ASCII and both binary endiannesses, `uchar` count + int and uint indices, a quad face, extra properties) -> 8 points; cube OBJ with relative negative indices and n-gon; cube STL binary/ASCII -> 8 points after weld. Pass = counts and bbox exact.
2. Truncation test: truncate a small PLY/OBJ/STL/XYZ at every byte; each read returns `Err` or a valid geometry, never panics.
3. OBJ with only `v` lines returns `is_point_cloud() == true`.
4. COLMAP `points3D.txt` and `points3D.bin` readers: 3-image fixture (spec test 21) parses to exact counts; whitespace and `#` comments; malformed number returns `Err` (never 0.0); RGB mapped to `Cd` in 0..1.
5. Optional `las` (+`laz`) feature on the `las` crate: 3-point round trip within the LAS scale quantum; colour 16-bit to `Cd` 0..1; intensity as Float attrib; unit not guessed (left to the caller).
6. Optional `e57` feature on the `e57` crate: write 1000 points with its writer, read back to 1e-9; multi-scan files merge with a per-point Int `scan` attribute.
7. Format sniff function (magic bytes) used by the import node: unit tests for `ply`, `LASF`, `ASTM-E57`, STL ASCII vs binary, unknown -> `Err`.

### vernon-nodes-capture
1. `capture.import_scan` (Stage::Capture): `Path` param with the extension list, `Enum` unit (mm/cm/m/in/auto), size-check `unit_guess` per spec 2.2; sets `metric`; Report has `point_count`, `bbox_diagonal` mm. Test spec test 19 values (bbox 4480x1810x1290 -> mm; /10 -> cm; 4.48 -> m; 176.4 -> in; 450-unit part -> ambiguous warning, no rescale; missing file -> `CookError`, not a panic).
2. `capture.set_units` and `capture.scale_from_distance`: `s = L_true/|p1-p0|`; reject `|p1-p0| < 1e-9` and `s <= 0`; sets `metric=1`. Test: spec tests 7-8 (s = 0.99950025 to 1e-7; box length 4500.000 +- 0.001).
3. `capture.scale_bar_solve`: weighted fit, leave-one-out rejection, thermal correction. Test: spec tests 8, 9, 10 (rejects the 1002.0 bar; aluminium 1000.230, invar 1000.012).
4. `capture.similarity_align` (Umeyama, RANSAC option): spec test 11 (recover scale 1000, 30 deg, translation to 1e-9; outlier case; plain least squares RMS > 50 mm).
5. `capture.error_budget` and `capture.length_check` as `Report` producers with `Metric::with_limit`: spec tests 1-6, 12, 13 (E_total 0.245 / 0.310 / 1.450 / 1.900; verdict flips between the 0.5 mm and 0.02 mm scale bars).
6. COLMAP driver nodes (`capture.sfm_features_match`, `sfm_reconstruct`, `mvs_dense`): one Rust `ToolSpec` table for argv; runtime `-h` introspection; refuse non-empty workspace unless `overwrite`; work only inside `ctx.work_dir`; capture both streams; log file plus failure tail; kill child on drop. Tests: argv snapshots (spec test 20, including the old-name `-h` fake and the 1,124,250-pair warning), mock executable via a test helper binary writing real whitespace text and a `.bin`, error path on non-zero exit, `overwrite=false` leaves an existing file untouched.
7. Dense chain order test: argv log shows `image_undistorter` before `patch_match_stereo`, then `stereo_fusion`; with a mock that lacks CUDA the node returns a clear `Failed` or sparse + Warning note.
8. External-tool concurrency: two COLMAP nodes in one graph never run at once (needs the vernon-graph limit below).

### vernon-nodes-clean
1. Small `linalg` module: 3x3 Jacobi SVD, 6x6 SPD solve with condition number. Test against closed forms (spec test 1: Horn and SVD routes agree to 1e-9) and a rank-deficient case that reports the free direction.
2. Spatial index (grid hash for radius/kNN, exact): equality with brute force on 5k random points for k and radius queries; index build on 1M points completes (timing logged, not asserted).
3. `clean.outlier_remove`: params `k`, `sigma`, `mode` (remove/tag); Report `removed_points`, `kept_points`, `removed_fraction`, `threshold_mm`. Test: plane of 100k points, noise 0.035 mm, 500 outliers >= 20 mm away: >= 99% of outliers removed and <= 3% of inliers removed (one-sided 2-sigma tail is 2.3%); every attribute column and group is compacted consistently and `validate()` passes; `metric=0` input is refused.
4. `clean.voxel_downsample`: centroid per voxel, `N` renormalised, `Cd` averaged, deterministic output order. Test: 100x100 grid at 1 mm spacing, voxel 2 mm -> 2500 points; voxel below spacing -> unchanged count plus Info note; voxel above half the bbox diagonal -> Warning.
5. `clean.radius_filter`: `radius` mm, `min_neighbors`. Test on a cluster plus isolated points with exact removed set.
6. `clean.denoise_points` (feature-preserving): off-by-default, `iterations`. Test: noisy plane sigma 0.035 mm drops by >= 2x after 3 iterations, while a box edge position stays within 0.05 mm.
7. `clean.pair_align`, `clean.icp_align` per spec: correct row pairing, RMS-change stop, Cholesky + condition number, trimmed overlap, `max_pair_dist` exclusion share flagged above 50%, centred f64 coordinates. Tests: spec tests 1-7 (ellipsoid 3 deg + (2, -1, 0.5) mm recovered to 0.01 mm / 0.01 deg; flat plate reports cond > 1e8 and a free direction; double application check: chained result equals the known transform to 1e-9).
8. `clean.mesh_distance`: AABB BVH nearest-first, closest point with region code, angle-weighted pseudo-normal sign, writes point attribs `dist` and `dist_valid`. Tests: spec tests 9 (+0.3, -0.4, 0.0; flip negates) and 10 (cube edge point (60,60,0) -> 14.142136 mm positive; centre -> -50); a centroid-nearest counterexample (one long thin triangle) where k2's approach is wrong.
9. `clean.cloud_distance` (none/plane models; quadric and triangulation as white-box): spec test 8 (0.768 vs 0.300).
10. `clean.deviation_stats` and `clean.tolerance_gate`: nearest-rank percentiles, inclusive fraction, `Metric::with_limit` gating, `fail_on_miss` Warning vs Error. Tests: spec tests 11-12 (N = 1000: P50 0.500, P95 0.950, mean 0.5005, rms 0.577783, gate fails; 9,500 + 500 split passes at exactly 0.95, 9,499 fails); NaN input counted invalid, empty input -> Error note.
11. `clean.deviation_histogram` and `clean.deviation_colors`: spec tests 13-14 (counts 99,100,...,101; colour t values); bin width <= 0 -> `BadParam`; output `Json` plus optional CSV `File` under `ctx.work_dir`.
12. `clean.decimate` (QEM, `target_ratio`, boundary weight) and `clean.watertight_wrap` (sparse SDF + marching cubes, `voxel_size` mm, signed `offset` mm, band-size cap). Tests on a UV sphere r = 50 mm: decimate to 50% gives triangles within +-2% of target with the Hausdorff distance below the stated bound; wrap at 1 mm gives V-E+F = 2, `open_edges` 0, radius error <= 1 voxel, and an oversize request returns an Error note instead of allocating.
13. Honest white-box stubs: any node still stubbed passes geometry through, computes real counts, adds an Info note, and `whitebox` stays true. Test: no stub returns constants independent of input.

### vernon-graph (cross-cutting, flagged for the owner of that crate)
1. Optional cancel flag and progress callback on `CookContext`; test: a cook of a long-running mock tool stops within one second of cancel.
2. Process-wide limit for nodes with `external_tool` set (default 1); test: two mock tool nodes never overlap in time.
3. Enforce `ParamSpec` min/max (clamp or `BadParam`) before `cook`; test: histogram bin width 0 never reaches the node.
4. Add a directory mode to `ParamKind::Path` (or a new kind) for workspace folders.
5. Decide the workspace `rust-version` (1.80 vs 1.89+) before any dependency on nalgebra or kiddo.

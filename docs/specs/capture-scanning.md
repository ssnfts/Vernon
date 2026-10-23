# Spec: Stage 1 Capture - scanning, photogrammetry, scale, drift, multi-scan registration

Status: draft 1 (clean-room spec). Author: spec-writer subagent, 2026-10-21. Consumers: `crates/vernon-nodes-capture`.
Legend: **[F]** = read from a fetched page; **[S]** = only seen in a search snippet or retailer page (treat as soft); **[M]** = from background knowledge, not re-checked this session (verify before relying on it). Vendor names appear only in this file and in `NodeTypeInfo.emulates`, never in UI strings.

## 1. Scope and reference tools

In scope: getting a car-sized surface into one metric, single-frame point cloud/mesh in **millimetres**, with a stated error budget, so stage 2 (clean/align) can check it. Out of scope: deviation analysis against CAD (stage 2), cleaning/decimation, surfacing.

| Reference | Version / date seen | Role in this spec |
|---|---|---|
| COLMAP CLI | 4.3.0.dev0 (2026-10-11) **[F]** S1-S4 | open-source photogrammetry, driven as a separate process (BSD, see CLEAN_ROOM.md) |
| Open3D registration docs | 0.19.0 **[F]** S7, S8 | published parameter recipes for ICP / FPFH-RANSAC / pose graph (algorithms re-implemented, no code copied) |
| Handheld laser scanner, entry class (Creality CR-Scan Raptor) | retail listings **[S]** S19 | consumer blue-laser reference |
| Handheld laser + wide-volume tracker (Revopoint MIRACO Plus; Trackit / Trackit SR) | listings **[S]**, CNX spec table **[F]** S16-S18 | pro reference with a published "a + b mm/m" figure |
| Handheld crossed-line laser + IR (Shining 3D EinScan Rigil / Rigil Lite) | vendor page **[F]** S20 | pro reference with global markers |
| Phone photogrammetry/LiDAR app (Polycam) | help-centre snippets **[S]** S21 | mock-up capture, rescale tool |
| Close-range photogrammetry metrology (JWST CPM paper) | 2016 paper **[F]** S15 | scale-bar practice, ppm model of length uncertainty |

Exports of all of the above are neutral meshes/clouds (PLY, OBJ, STL, ASC, 3MF, glTF, FBX, LAS). Vernon never reads vendor project files.

## 2. Feature catalogue (observable behaviour)

### 2.1 Scanner classes and published figures

| Scanner | Tech | Single-frame / point accuracy | Length ("volumetric") accuracy | Point distance | Working distance / FOV | Tracking modes | Export | Source |
|---|---|---|---|---|---|---|---|---|
| Entry laser (Raptor) | 7-line blue laser + NIR structured light, 1920x1200 camera | blue mode up to 0.02 mm (stated at 100 mm); NIR ~0.1 mm; a reviewer measured 0.03-0.04 mm on a test part | **not published in anything fetched** | n/a | object size 5 mm to ~2000 mm per side | feature/texture; reviewer saw tracking loss on featureless panels | OBJ, STL, PLY | S19, S23 |
| Laser + tracker (MIRACO Plus) | standalone, photogrammetric length metrology | "up to 0.04 mm" in one listing, "0.02 mm precision / 0.04 mm accuracy" in another (sources disagree) | 0.02 mm + 0.05 mm/m (retail listings; the report cites the same) - condition under which it holds (markers / photogrammetry mode) **unconfirmed** | n/a | 20 fps, weight ~750 g | marker + feature | n/a | S18 |
| Optical-tracked laser (Trackit) | tracker 5 MP IR camera + handheld, marker-free | 0.02 mm single frame | tracker system: 0.025 mm + 0.04 mm x L(m) (CNX, vendor page); one store lists 0.02 + 0.04 | fused up to 0.05 mm | scanner 200-400 mm, capture 160x70 mm @200 to 320x215 mm @400; tracker 1.5-4 m, volume 11 m^3, recommended part 0.01-6 m | tracker follows the part with no surface markers (method not described) | PLY, OBJ, STL, ASC, 3MF, GLTF, FBX | S16, S17 |
| Crossed-line laser (Rigil) | 25+25 crossed blue lines (Lite: 17+17), IR VCSEL fast mode | Laser HD: geometric resolution up to 0.05 mm (retail) | Rigil FAQ: 0.04 + 0.06 (mm/m by convention); IR fast mode listed 0.1 + 0.3 or 0.1 + 0.4 mm/m (retailers disagree). Lite: not stated | Laser HD 0.05-10 mm, IR 0.2-10 mm | Laser HD 170-550 mm; IR 160-1500 mm | Laser HD: global markers, markers, features. IR: markers, features, texture, global markers (combinable) | STL, OBJ, PLY, 3MF, ASC, LAS | S20 |

Rules derived from the table:
- Length-accuracy figures are **maximum-permissible-error style limits for a measured length L**: `E(L) <= a + b*L`, with `a` in mm and `b` in mm/m [S]. The same form appears in ISO 10360-style acceptance tests; for area scanners the sphere-spacing / probing-error tests are in ISO 10360-10 and the VDI/VDE 2634 Part 3 guideline [S] S22. Vernon treats them as a *claim to check*, not a measured truth.
- `b` in mm/m equals parts-per-thousand-of-a-mm per metre, i.e. `b*1000` ppm: 0.05 mm/m = 50 ppm, 0.06 = 60 ppm, 0.3 = 300 ppm.
- Spec-sheet single-frame numbers are lab figures on small matte parts. Real panels (gloss clear-coat, featureless areas) are worse; project report notes no independent full-car measurement exists. Treat 3-5x as a planning haircut for first-run paint without developer spray (Vernon heuristic, tune with data).
- Over a car (L = 4.5 m) the `b*L` term dominates `a` for every pro profile (section 5 has the numbers).

### 2.2 Units and formats on import

| Format | Unit tag? | Vernon rule |
|---|---|---|
| STL (binary/ASCII) | none [M] | user unit, default mm |
| OBJ, PLY, XYZ/ASC | none (PLY header may carry comments) [M] | user unit, default mm; ASC/XYZ = whitespace columns `x y z [nx ny nz] [r g b]` |
| 3MF | `unit` attribute on the model; default millimeter [M] | honour the attribute; still run the size check |
| glTF/GLB | metres by spec [M] | convert x1000 |
| LAS | via CRS metadata [M] | read scale/offset; unit from CRS, else ask |
| Polycam exports (OBJ/DAE/FBX/GLTF/STL/USDZ meshes; LAS/PLY/PTS/XYZ/DXF clouds on paid tiers) | the help pages do not state units **[S]** | never assume; require explicit unit and run size check |

Size check (Vernon heuristic, not vendor behaviour): let `D` = longest bounding-box edge in file units. For a whole vehicle expect 3.0-6.0 m: candidates mm if D in [3000, 6000], cm if [300, 600], m if [3, 6], inch if [118, 236]. Part-scale files are ambiguous (a 400-unit bumper could be mm or cm), so the check only *warns* and reports `unit_guess`; it rescales only when the user opts in.

### 2.3 Marker and tracking behaviour in scanner software (generic)
- **Feature / geometry tracking**: the scanner aligns each frame to the growing model with ICP-like matching on geometry or texture. Fails on flat, featureless panels (tracking loss) and drifts with trajectory length.
- **Marker mode**: retro-reflective or printed dots (box contents of one scanner: 3 mm and 6 mm reflective markers) stuck on or around the part give frame-to-frame anchors independent of geometry. S20
- **Global markers / pre-scan photogrammetry**: a first pass measures all marker 3D positions in one frame; later scans snap to that frame, so error is set by the marker frame rather than accumulated drift. This is why the Porsche project reported ~0.1 mm scan accuracy and < 1 mm panel fit along the car (vendor figures, S23).
- **External tracker**: positions the scanner in a fixed frame, so error does not grow with path length but is bounded by tracker volume (11 m^3 above), and the part must stay inside it or be tracked with it.
- Developer spray (matte, ~microns thick) reduces gloss errors; not modelled in Vernon beyond a `surface_finish` field on the scan record.

### 2.4 Photogrammetry - phone app behaviour (mock-up grade)
Observable: capture modes (photo/photogrammetry and LiDAR) produce a textured mesh; export formats depend on subscription (free tier glTF only; a middle tier 12 formats; top tiers add point clouds incl. geo-referenced LAS) [S]. A **Rescale tool** lets the user pick reference points whose real distance is measurable, enter it, and the scale is baked into exports; it can be repeated [S] S21. Report finding: phone LiDAR is poor for fitment; photogrammetry is better but needs scale correction (S23). Vernon equivalent: `capture.scale_from_distance` + `capture.scale_bar_solve`.

### 2.5 Photogrammetry - COLMAP command pipeline **[F]** S1-S3
Workspace: `images/` (recursive scan, relative paths are IDs), `database.db` (SQLite, all features and matches), `sparse/0/` (`cameras.bin images.bin points3D.bin` + `rigs.bin frames.bin`), `dense/0/` (`images/ sparse/ stereo/ fused.ply meshed-*.ply`).

| Step (command) | Key options (name = default) | Notes / limits |
|---|---|---|
| feature_extractor | `ImageReader.camera_model=SIMPLE_RADIAL`; `ImageReader.single_camera=0`; `single_camera_per_folder=0`; `ImageReader.default_focal_length_factor=1.2` (tutorial says guess 1.25 x max(w,h): version drift); `FeatureExtraction.type=SIFT` (also ALIKED_N16ROT, ALIKED_N32, LOMA_B, LOMA_B128); `FeatureExtraction.max_image_size=3200`; `FeatureExtraction.use_gpu=1`; `SiftExtraction.max_num_features=8192`; `num_octaves=4`; `octave_resolution=3`; `peak_threshold=0.00667`; `edge_threshold=10`; `max_num_orientations=2`; `first_octave=-1`; `estimate_affine_shape=0`; `domain_size_pooling=0`; `upright=0` | Exits if images share a camera model but differ in size/EXIF focal. Older builds spell the size option `SiftExtraction.max_image_size`: introspect `colmap feature_extractor -h` and map names at runtime. Cancelled extraction resumes. |
| exhaustive_matcher | - | best quality, O(n^2) pairs; fine to a few hundred images |
| sequential_matcher | `overlap`, `loop_detection_period`, `loop_detection_num_images`, `loop_detection_min_index_distance` (0 = off) | ordered/video frames; needs vocab tree (auto-download) |
| vocab_tree_matcher / spatial_matcher / transitive_matcher | `VocabTreeMatching.num_images`, `SpatialMatching.max_num_neighbors` | thousands of images / GPS or prior positions / infer A-C from A-B, B-C |
| guided matching | `FeatureMatching.guided_matching` (default off) | more matches on hard surfaces |
| mapper | incremental SfM; models may be merged with `model_merger`; tips: more matching, manual initial pair, DSP-SIFT (`estimate_affine_shape=1`, `domain_size_pooling=1`) for more points | intrinsics refined except principal point by default (refine it in a final global BA) |
| global_mapper / hierarchical_mapper | rotation-averaging + global positioning / split-and-merge | alternatives for large sets |
| model_aligner | `--ref_images_path` (text: `image_name X Y Z`) or database priors, `--ref_is_gps`, `--alignment_type ecef` (ENU optional), `--alignment_max_error` (3.0 in the example, not a default) | Sim(3) incl. scale via RANSAC; >= 3 reference images |
| image_undistorter | output dir, `--output_type COLMAP` | required before MVS |
| patch_match_stereo | `PatchMatchStereo.geom_consistency` true in the example; `max_image_size`, `window_radius`, `window_step`, `num_samples`, `num_iterations`, `filter`, `filter_min_ncc`, `cache_size` | needs CUDA; geom_consistency raises GPU memory; textureless: higher resolution, bigger window, lower `filter_min_ncc` |
| stereo_fusion | `--input_type geometric`; `StereoFusion.min_num_pixels` (> 1; raise to cut noise; 1 with known poses); `max_image_size`; `cache_size` | writes `fused.ply` with normals |
| poisson_mesher / delaunay_mesher / advancing_front_mesher | - | Delaunay uses visibility voting; advancing front needs CGAL |
| mesh_simplifier | `MeshSimplification.target_face_ratio=0.1`; `max_error=0` (off); `boundary_weight=1000` | QEM |
| automatic_reconstructor | `--quality LOW/MEDIUM/HIGH/EXTREME`; `--data_type INDIVIDUAL/VIDEO/INTERNET`; `--feature`; `--mapper INCREMENTAL/HIERARCHICAL/GLOBAL`; `--mesher POISSON/DELAUNAY/ADVANCING_FRONT` | one-shot wrapper |

Camera-model guidance (FAQ): SIMPLE_RADIAL default; PINHOLE if distortion negligible; OPENCV for moderate distortion; use the simplest model that keeps reprojection error low; share intrinsics with `single_camera=1` when settings are identical. Each object should be seen in >= 3 images.
**Scale is not recovered by monocular SfM** (gauge freedom): the sparse/dense output has arbitrary scale and orientation; Vernon must scale it (section 3.1) and mark it `metric=false` until then.
Real-world cost anchor (S23): ~1,500 photos of a white car with mud/water film and tape markers took ~13 h sparse + ~37 h dense and gave a ~38 M-face mesh.

### 2.6 Scale bars and markers
- At least one known distance must appear in the images/scans; several distances give higher scale accuracy and let you detect a bad bar (S15).
- Each bar carries redundant target pairs and, where the temperature moves, a thermometer, so length is corrected for thermal expansion (S15). Bars made of poorly characterised materials were dropped because their expansion was too variable (S15).
- Coded targets are recognisable by software and give camera pose to the adjustment; plan >= 4 visible per image (S15).
- A scale bar with measured error above 50 um versus its calibration was excluded in the paper's hardware tests; scale factors computed for separate objects differed by up to 285 ppm, large against a ~100 ppm requirement (S15).
- Bar uncertainty maps straight to the car: relative error `sigma_L/L` times the span you care about.

### 2.7 Multi-scan registration (behaviour)
Observable results: each scan has an `fitness` (fraction of source points with a counterpart inside the distance gate) and `inlier_rmse` (mm); final merged cloud is voxel-resampled. Failure modes: too little overlap, symmetric/flat overlap (sliding), drift accumulating along a chain (closed by a loop edge), wrong global start. A quality report must expose all of these (section 3.6).

## 3. Algorithms (published methods, implementer detail)

### 3.1 Scale and similarity transforms
**Single/multi scale bar.** Measured lengths `m_i` (from the cloud/mesh targets), true lengths `L_i` (corrected for temperature `L_i = L0_i*(1 + alpha_i*(T_i - T_ref))`, `T_ref = 20 C`, alpha from the bar datasheet; typical handbook values: invar ~1.2e-6 /K, aluminium ~23e-6 /K **[M]**). Scale `s` minimising `sum w_i (s*m_i - L_i)^2` with `w_i = 1/sigma_i^2`:
`s = sum(w_i m_i L_i) / sum(w_i m_i^2)`; `u(s) = 1/sqrt(sum(w_i m_i^2))`. Relative scale uncertainty `u(s)/s`; effect on an end-to-end car length `Lc` is `Lc * u(s)/s`.
**Outlier rejection** (follows the paper's practice, S15): leave-one-out - compute `s_(-i)`, residual `r_i = s_(-i)*m_i - L_i`; drop bars with `|r_i| > reject_tol_mm` (default 0.05 mm for calibrated bars; use 0.5 mm for tape-measure bars); iterate until stable; need >= 1 bar left (warn < 3). Report per-bar residual in mm and ppm and the span of s (max-min over bars in ppm).
**Sim(3) from corresponded points** (Umeyama, S12): centroids `mu_x, mu_y`; `Sigma = (1/n) sum (y_i - mu_y)(x_i - mu_x)^T`; SVD `Sigma = U D V^T`; `S = diag(1,1,det(U)det(V))`; `R = U S V^T`; `c = trace(D S)/sigma_x^2` with `sigma_x^2 = (1/n) sum |x_i - mu_x|^2`; `t = mu_y - c R mu_x`. Needs n >= 3 non-collinear. For rigid-only use c = 1. Robust variant: RANSAC over 3-point subsets, inlier gate `inlier_mm`, refit on inliers (this mirrors what the photogrammetry tool's geo-register step does with its max-error gate, S3).

### 3.2 Photogrammetry back-end
Incremental SfM with robust initialisation, triangulation, bundle adjustment and re-triangulation (S5); dense depth/normal maps by PatchMatch with pixelwise view selection and geometric consistency (S6); fusion by multi-view consistency; meshing by Poisson or Delaunay graph cut. Vernon drives the binary (spawn, stream stdout, parse `cameras/images/points3D`), it does not re-implement SfM. Wrapper duties: build argv, check GPU/CUDA, detect option-name drift via `-h`, resume (databases and cancelled extraction resume), cancel safely, ETA from image count, and estimate pairs `n(n-1)/2` to warn about exhaustive matching on large sets.
**Marker triangulation** (white-box): given 2D marker observations `(image, id, u, v)` and COLMAP poses/intrinsics, per marker with >= 3 views run linear DLT triangulation, then Gauss-Newton on reprojection error; reject observations with reprojection > `max_reproj_px`; output 3D point, per-point RMS in px and in mm (via mean ray distance). The 3D points feed 3.1.

### 3.3 Length-accuracy model and verification
Prediction: `E_scanner(L) = a + b*L`. Combined car budget (Vernon): `E_total = sqrt(E_scanner^2 + E_scale^2 + (k*rms_reg)^2)` with `E_scale = L*u(s)/s`, `rms_reg` the registration inlier RMSE, `k = 2` (coverage factor); `combine=linear` sums the terms for a worst-case bound. Photogrammetry: distance uncertainty `sigma_d = ppm*1e-6*L` from a bundle adjustment ppm figure (S15, Blaha-Sandwith model cited there); e.g. 0.012 mm over 3 m = 4 ppm, so 13 m gives 0.052 mm (1 sigma) in that paper's best case.
Verification (sphere-spacing style, S22): for all marker pairs `k`, `e_k = d_meas,k - d_ref,k`; check `|e_k| <= a + b*L_k` (or a fixed gate), and regress `b_hat = sum(L_k e_k)/sum(L_k^2)` (through-origin scale error, ppm). Residual after removing `b_hat*L` isolates non-linear drift/bending. Pass only if both the max error and the post-scale residual are under gate.

### 3.4 Pairwise ICP (S13, S11)
Point-to-plane error metric (Chen-Medioni) with linearised small-angle solve (6x6 normal equations on `[r; t]`), nearest-neighbour via k-d tree, optional normal-compatibility rejection (> 45 deg), distance gate, trimming of worst `1-overlap` fraction, uniform or normal-space sampling for flat meshes (S11 shows normal-space sampling helps small features on near-planar surfaces). Convergence: relative change in fitness and RMSE < 1e-6 or `max_iter` (Open3D-style defaults: 30 iterations **[M]**). Coarse-to-fine: run at a large gate, then at the fine gate from the previous result (S7: 15x then 1.5x voxel). Outputs `T` (4x4), `fitness`, `inlier_rmse`, number of correspondences, and the 6x6 information matrix `Lambda = sum G_p^T G_p` with `G_p = [-[p]_x | I]` over final correspondences **[M]** (needed by 3.6).

### 3.5 Global (coarse) registration (S8, S14, S10)
Downsample with voxel `v`; normals with hybrid radius search radius `2v`, max 30 neighbours; 33-dim FPFH descriptors radius `5v`, max 100 neighbours (S8); RANSAC on feature matches: mutual filter on, distance gate `1.5v`, 3 points per sample, edge-length similarity checker 0.9, distance checker `1.5v`, up to 100,000 iterations with 0.999 confidence; then refine with point-to-plane ICP on the full-resolution clouds at gate `0.4v` (S8). Alternative: Fast Global Registration (gate `0.5v`, one dense objective that switches off false correspondences, S10) - faster, tolerant of noise. In the published example RANSAC reached fitness 0.67 / RMSE 0.037 (dataset units) and ICP refinement fitness 0.62 / RMSE 0.0066 (S8): refinement lowers RMSE even though fitness drops because the gate shrinks.

### 3.6 Multiway pose graph (S7, S9)
Nodes: one pose `T_i` per scan (scan 0 fixed as reference, identity). Odometry edges (certain) join consecutive scans, initialised by chaining pairwise ICP; loop edges (uncertain) join non-consecutive scans that overlap, ideally from global registration. Edge: `T_ij`, information `Lambda_ij`, `uncertain` flag. Solve with Levenberg-Marquardt over se(3). Cost with line processes `l_ij in [0,1]` on uncertain edges: `sum_ij l_ij * r_ij^T Lambda_ij r_ij + mu * sum_(uncertain)(sqrt(l_ij) - 1)^2`, closed-form `l_ij = (mu/(mu + r_ij^T Lambda_ij r_ij))^2` **[M, Black-Rangarajan duality used in S9]**. Two passes: optimise, prune edges with `l < edge_prune_threshold` (0.25 in S7), re-optimise. S7 distances: coarse 15x voxel, fine 1.5x voxel, voxel 0.02 in its dataset (i.e. scale to scanner point distance). Resample the merged cloud at the voxel size. Reports: per-edge residual (mm), pruned edges, **loop-closure gap before/after** (norm of translation part of `T_0^-1 T_n T_n0` for the closing edge).

## 4. Proposed Vernon nodes

Port types: `Geo` = mesh or point cloud (mm), `Xf` = 4x4 transform, `Pts` = id-keyed points, `Tab` = table, `Prof` = scanner profile, `Rep` = report (key/values + pass flag). Parameter units in mm unless stated. Status: **now** = implement with the real algorithm; **WB** = white-box placeholder first, algorithm later; **drv** = process driver (real once the binary is found, otherwise reports "tool missing").

| id | Label | Inputs | Outputs | Key params (default; range) | Status |
|---|---|---|---|---|---|
| capture.import_scan | Import Scan | file path | Geo, Rep | format auto; unit mm (mm/cm/m/in/auto); unit_check warn; keep_normals/colors true; columns `x y z nx ny nz r g b` | now (extends geo.file_read) |
| capture.set_units | Set Units | Geo | Geo, Rep | from_unit; factor override (>0); to mm fixed; guess_only false | now |
| capture.scale_from_distance | Scale From Distance | Geo, Pts p0 p1 | Geo, Rep | true_length 1000 (>0); uncertainty 0.5 (>=0); pivot origin/centroid (origin) | now |
| capture.scale_bar_solve | Solve Scale Bars | Pts, Tab bars(id_a,id_b,L,sigma,alpha,T) | Rep (s, ppm, per-bar residual), Geo opt. | ref_temp 20 C; reject_tol 0.05 (0.001-5); min_bars 1; weighting inv_var/equal | now |
| capture.similarity_align | Align By Points | Pts source, Pts target | Xf, Rep | allow_scale false; ransac false; inlier 1.0 (0.01-50); iterations 1000 | now |
| capture.marker_table | Marker Table | csv path | Pts | columns id,x,y,z,sigma; units mm; duplicate-id policy error | now |
| capture.marker_triangulate | Triangulate Markers | SfM model, Tab 2D obs | Pts, Rep | min_views 3 (2-50); max_reproj_px 2.0 (0.1-20); refine true | WB |
| capture.sfm_features_match | Feature Match | image dir | db path, Rep | camera_model SIMPLE_RADIAL; single_camera false; max_image_size 3200; max_features 8192; matcher exhaustive/sequential/vocab_tree/spatial; guided false; use_gpu true | drv |
| capture.sfm_reconstruct | Sparse Reconstruct | db path, image dir | SfM model, Pts cloud, Rep | mapper incremental/global/hierarchical; refine_focal true; refine_principal false; init_pair auto | drv |
| capture.mvs_dense | Dense Reconstruct | SfM model | Geo (cloud), Rep | max_image_size tool default; geom_consistency true; min_num_pixels tool default (>1); window_radius tool default | drv |
| capture.mvs_mesh | Mesh Dense Cloud | Geo cloud | Geo mesh | mesher poisson/delaunay; target_face_ratio 0.1 (0.01-1); boundary_weight 1000 | drv |
| capture.icp_pair | Pairwise Align | Geo src, Geo tgt, Xf init opt. | Xf, Rep, info matrix | voxel 5.0 (0.05-50); max_corr 1.5x voxel; metric point_to_plane; max_iter 30; overlap 0.3 (0.05-1); normal_gate 45 deg | now |
| capture.global_align | Global Align | Geo src, Geo tgt | Xf, Rep | method fpfh_ransac/fgr; voxel 5.0; normal_r 2x; fpfh_r 5x; dist 1.5x; ransac_n 3; iters 100000; conf 0.999; refine true | WB |
| capture.multiway_align | Multiway Align | Geo[ ], edge list opt. | Geo[ ] (placed), Xf[ ], Rep | voxel 5.0; coarse 15x; fine 1.5x; prune 0.25 (0-1); solver LM; reference 0; auto_loops true | WB |
| capture.merge_scans | Merge Scans | Geo[ ] | Geo | dedupe_voxel 1.0 (0.01-20); keep_normals true; keep_colors true | now |
| capture.scanner_profile | Scanner Profile | - | Prof | a 0.02 (0-10); b 0.05 mm/m (0-5); single_frame 0.04; point_distance 0.05; work_dist 200-400 | now |
| capture.error_budget | Error Budget | Prof, Rep(s), Rep(reg) opt. | Rep (E_scanner, E_scale, E_reg, E_total, verdict) | length 4500 (100-30000); scale_bar_sigma 0.01; scale_bar_len 1000; reg_rms 0.1; k 2; combine rss/linear; gate 1.0 | now |
| capture.length_check | Length Check | Pts measured, Tab reference lengths, Prof opt. | Rep (e_k, b_hat ppm, max abs, post-scale resid, pass), plot data | gate_mode profile/fixed (fixed 1.0); min_pairs 3 | now |
| capture.scan_quality | Scan Quality | Geo | Rep (density /mm^2, noise rms, hole fraction, bbox) | radius 3x point distance; min_neighbors 12; hole_gap 5.0 | now (noise/density), WB (holes) |

Notes: presets for `capture.scanner_profile` use generic names ("tracked handheld laser", "marker-assisted handheld laser", "entry handheld laser"); the numeric rows of 2.1 live here in docs only. `capture.sfm_*` and `capture.mvs_*` write caches under the project dir and are resumable. Anything produced before scaling carries `metric=false`; downstream stage-2 nodes should refuse it.

## 5. Golden tests (concrete numbers)

Tolerances: closed-form checks 1e-6 relative; stochastic checks fixed RNG seed.

**Error budget (closed form, car length L = 4.5 m)**
1. a=0.02, b=0.05: `E_scanner = 0.02 + 0.05*4.5 = 0.245 mm`; drift term alone 0.225 mm (report quotes ~0.23). Expect 0.245 +/- 0.0005.
2. a=0.04, b=0.06: 0.04 + 0.27 = 0.310 mm. IR mode a=0.1, b=0.3: 1.450 mm; b=0.4: 1.900 mm (shows why retailer disagreement matters).
3. a=0.025, b=0.04: 0.025 + 0.18 = 0.205 mm; store variant a=0.02: 0.200 mm.
4. Paper check (S15): 5 um + 5 um/m gives 0.025 mm at 4 m and 0.070 mm at 13 m (1 sigma); 0.012 mm over 3 m is 4 ppm, times 13 m is 0.052 mm.
5. Scale-bar term: tape-measure bar sigma 0.5 mm over 1000 mm = 500 ppm -> `E_scale = 2.25 mm` over 4.5 m (fails a 1.0 mm gate). Calibrated bar sigma 0.02 mm over 1000 mm = 20 ppm -> 0.090 mm (passes). Two independent such bars: 0.064 mm. The verdict must flip between cases.
6. RSS vs linear: E_scanner 0.245, E_scale 0.090, reg_rms 0.10 with k=2: RSS = sqrt(0.245^2+0.090^2+0.2^2) = 0.328 mm; linear = 0.535 mm.

**Scale solve**
7. Synthetic box 4500x1800x1300 mm scaled by 1.0005 (+500 ppm); one bar true 1000 mm measured 1000.5 -> s = 1000/1000.5 = 0.99950025 (tol 1e-7); after applying, box length 4500.000 +/- 0.001.
8. Three bars true (500, 1000, 2000) measured (500.25, 1000.50, 2001.00), equal weights: `s = 5,252,625 / 5,255,251.3125 = 0.99950025`; per-bar residuals < 1e-3 mm.
9. Add a fourth bar true 1000 measured 1002.0 with reject_tol 0.05: it is rejected, s unchanged to 1e-6, flagged in the table.
10. Thermal: 1 m aluminium bar at 30 C vs 20 C reference, alpha 23e-6: L = 1000.230 mm. Invar alpha 1.2e-6: 1000.012 mm.
11. Umeyama: 6 random non-collinear points, known transform (scale 1000, 30 deg about z, translation (120, -40, 15)); recover c, R, t to 1e-9; RMS 0. Add one gross outlier (500 mm off): with `ransac=true, inlier=1.0` recover to 1e-6 and flag 1 outlier; plain LS must give RMS > 50 mm (proves RANSAC matters).

**Length check**
12. Eight markers along a 4.5 m span with pairwise reference lengths from CAD, measured set stretched by 60 ppm: `b_hat = 60 ppm +/- 1`, max |e| = 0.27 mm at L = 4.5 m (equals 60 ppm x 4.5 m), post-scale residual < 0.001 mm; fails for a profile with b=0.05 (limit 0.245 mm) and passes with b=0.06 + a=0.04 (limit 0.31 mm).
13. Gate from the Porsche case (S23): 8 markers with random errors <= 0.1 mm and linear drift 0.5 mm over 4.5 m must pass the fixed 1.0 mm gate; drift 1.2 mm over 4.5 m must fail.

**Registration**
14. ICP: synthetic doubly curved patch 1000x600 mm (z = 50 sin(pi x/1000) cos(pi y/600)) sampled at 1 mm, noise sigma 0.03 mm, 50% overlap, initial offset 15 mm / 8 deg: recovered translation error < 0.05 mm, rotation error < 0.005 deg, fitness >= 0.45, inlier RMSE 0.02-0.06 mm.
15. Global: same patch plus two asymmetric 20 mm bosses, initial 90 deg yaw + 300 mm shift, voxel 5 mm: RANSAC fitness >= 0.3, then ICP RMSE <= 0.1 mm; FGR within 2x of that. Fixed seed; accept 1 retry.
16. Pose graph: chain of 10 scans along 4.5 m, per-edge translation noise sigma 0.02 mm/axis, plus one loop-closure edge: RMS pose error after optimisation <= 0.5 x dead-reckoned RMS (expected dead-reckoned end error ~ sqrt(9) x 0.02 = 0.06 mm/axis). A deliberately wrong loop edge (offset 50 mm) must get `l < 0.25` and be pruned; chain error then equals the no-loop case within 10%.
17. Drift injection: chain of 10 scans with systematic 0.05 mm/m scale drift: end gap 0.225 mm before loop closure, < 0.05 mm after.

**Quality and format**
18. Plane of 100k points, noise sigma 0.035 mm (middle of the reviewer's 0.03-0.04 mm): `noise_rms` in [0.033, 0.037].
19. Units: bbox (4480, 1810, 1290) -> mm candidate; same numbers /10 -> cm; (4.48, 1.81, 1.29) -> m; (176.4, 71.3, 50.8) -> inch; a 450-unit bumper -> ambiguous warning, no auto-rescale. 3MF with unit=meter and a 4.5-unit box imports as 4500 mm.
20. COLMAP argv snapshots: extractor with defaults gives `--ImageReader.camera_model SIMPLE_RADIAL --ImageReader.single_camera 0 --FeatureExtraction.max_image_size 3200 --SiftExtraction.max_num_features 8192`; with a fake `-h` listing only the old name it emits `SiftExtraction.max_image_size`. 1,500 images + exhaustive = 1,124,250 pairs -> warning recommending sequential/vocab-tree (report's 1,500-photo case); runtime ETA must not claim < 10 h sparse for that set on CPU-only.
21. Model parsing: a 3-image, 2-camera fixture of `cameras.txt/images.txt/points3D.txt` parses to the expected counts; `metric=false` flag set until a scale node runs.

## 6. Open questions / risks
- MIRACO Plus length accuracy conditions and Raptor length accuracy are not in any primary source seen; keep both as user-entered profile values.
- COLMAP option names drift between releases (4.3.0.dev0 vs older); runtime introspection is mandatory.
- Option defaults for patch_match_stereo / stereo_fusion were not on the fetched pages; read them from `-h` at runtime rather than hard-coding.
- Open3D-derived thresholds (voxel multiples) came from room-scale RGB-D data; confirm on car panels (golden tests 14-17) before freezing defaults.
- All scanner accuracy numbers are vendor or retailer claims; the only independent figure is the 0.03-0.04 mm reviewer measurement on a small part.

## 6a. Node map: stage-1 nodes and point-cloud filters under their registered ids

Section 4 used working names; the ids below are the registered ones. Everything is a paraphrase of textbook or
published methods (Sources at the end of this section).

- `capture.drift_report`: the scanner's published volumetric figure is `E = a + b L` (a in mm, b in mm per metre, L
  in metres; section 2.1). Parameters: `length` (mm), `a`, `b`, `gate` (mm) and `haircut` (a planning factor of 1 or more for difficult
  surfaces). The node evaluates `E`, separates the constant part `a` from the part that grows with length `b L`, and
  checks the planning value (`haircut` times `E`) against the gate. It states that a manufacturer figure is a claim to verify with the scale-bar and sphere-spacing
  checks of section 2, not a measurement. Check: defaults a = 0.02 mm, b = 0.05 mm/m, L = 4.5 m give 0.245 mm, of
  which 0.225 mm (92 %) grows with length.
- `capture.import_phone_scan`: reads a mock-up grade mesh or cloud from a phone or photogrammetry export (section
  2.4). `unit = unknown` keeps the numbers as they are and flags the geometry as non-metric until a scale node has run;
  nothing guesses a scale. `weld` merges coincident vertices and `keep_colors` keeps vertex colours.
- `capture.marker_merge`: markers with the same ids in a scan and in the reference marker set (or in the first
  scan) form point pairs; each scan is moved into the common frame by the closed-form rigid fit of the pairs (section
  3.1), optionally with a uniform scale (`allow_scale`) and a RANSAC pass (`ransac`, `inlier`, `iterations`). Fewer
  than `min_shared` shared markers is an error; a scan whose marker residual exceeds `max_rms` makes the report fail. The report
  lists the shared marker count and residual per scan; the merged cloud is thinned with `dedupe_voxel`.
- `clean.estimate_normals`: plane fit to the `k` nearest neighbours (normal = eigenvector of the smallest
  covariance eigenvalue; Hoppe et al.) for clouds, area-weighted face normals for meshes. `orient` sets the sign:
  `up` makes Z non-negative, `viewpoint` faces a point, `centroid_out` points away from the centroid, `none` leaves
  it arbitrary. Also writes `curv`, the surface variation `l0 / (l0 + l1 + l2)` (Pauly et al.): 0 for a plane, 1/3
  for isotropic noise. Check: a planar patch gives `curv` below 1e-9 and normals within 0.01 degree of the plane normal.
- `clean.outlier_remove`: statistical outlier removal (Rusu et al.). Each point's mean distance to its `k` nearest
  neighbours is compared with the distribution over the cloud; points above `mean + sigma * std` are outliers.
  `remove` deletes them, `flag` keeps everything and writes the point attribute `outlier`.
- `clean.radius_filter`: removes points that have fewer than the required number of other points inside a fixed
  `radius`; it takes out floating specks and stray returns.
- `clean.random_subsample`: keeps `count` points, or a `fraction` of them, chosen uniformly at random from the seeded
  generator, in their original order; the same seed gives the same subset.
- `clean.voxel_subsample`: one point per occupied grid cell of edge `voxel`; `centroid` mode averages position and
  float and vector attributes, the other modes keep an original point.
- `clean.denoise_points`: moving-least-squares style smoothing after Alexa et al. For each point the `k` nearest
  neighbours get Gaussian weights `exp(-d^2 / h^2)` where `h^2` is half the squared distance to the farthest
  neighbour; the weighted centroid and covariance define a local plane (normal = eigenvector of the smallest
  eigenvalue) and the point moves toward that plane by `strength` times its signed distance. `iterations` passes run,
  rebuilding the neighbour search each time. Corners round off slightly; remove off-surface speckle first. Stored
  normals are kept and may no longer match the moved points (an info note says so). The report gives the mean and
  maximum shift. Check: an exactly planar cloud does not move (maximum shift below 1e-9 mm) and a plane with added
  height noise has its height RMS reduced.

Sources for this section:

- Hoppe, DeRose, Duchamp, McDonald, Stuetzle, "Surface reconstruction from unorganized points", SIGGRAPH 1992.
  https://doi.org/10.1145/142920.134011
- Pauly, Gross, Kobbelt, "Efficient simplification of point-sampled surfaces", IEEE Visualization 2002.
  https://doi.org/10.1109/VISUAL.2002.1183771
- Rusu, Marton, Blodow, Dolha, Beetz, "Towards 3D point cloud based object maps for household environments",
  Robotics and Autonomous Systems 56(11), 2008. https://doi.org/10.1016/j.robot.2008.08.005
- Alexa, Behr, Cohen-Or, Fleishman, Levin, Silva, "Computing and rendering point set surfaces", IEEE TVCG 9(1), 2003.
  https://doi.org/10.1109/TVCG.2003.1175093

## 7. Sources
S1 COLMAP command-line interface: https://colmap.github.io/cli.html [fetched]
S2 COLMAP tutorial: https://colmap.github.io/tutorial.html [fetched]
S3 COLMAP FAQ (geo-registration, tuning): https://colmap.github.io/faq.html [fetched]
S4 COLMAP home (version, citations): https://colmap.github.io/ [fetched]
S5 Schoenberger and Frahm, Structure-from-Motion Revisited, CVPR 2016: https://openaccess.thecvf.com/content_cvpr_2016/html/Schonberger_Structure-From-Motion_Revisited_CVPR_2016_paper.html [search-verified]
S6 Schoenberger, Zheng, Pollefeys, Frahm, Pixelwise View Selection for Unstructured Multi-View Stereo, ECCV 2016: https://doi.org/10.1007/978-3-319-46487-9_31 [M]
S7 Open3D multiway registration: https://www.open3d.org/docs/release/tutorial/pipelines/multiway_registration.html [fetched]
S8 Open3D global registration: https://www.open3d.org/docs/release/tutorial/pipelines/global_registration.html [fetched]
S9 Choi, Zhou, Koltun, Robust Reconstruction of Indoor Scenes, CVPR 2015: https://openaccess.thecvf.com/content_cvpr_2015/html/Choi_Robust_Reconstruction_of_2015_CVPR_paper.html [search-verified]
S10 Zhou, Park, Koltun, Fast Global Registration, ECCV 2016: https://www.researchgate.net/profile/Vladlen_Koltun/publication/305983982_Fast_Global_Registration/links/57a8086908aefe6167bc8366/Fast-Global-Registration.pdf [search-verified]
S11 Rusinkiewicz and Levoy, Efficient Variants of the ICP Algorithm, 3DIM 2001: https://www.cs.princeton.edu/~smr/papers/fasticp [search-verified]
S12 Umeyama, Least-squares estimation of transformation parameters between two point patterns, TPAMI 1991: https://doi.org/10.1109/34.88573 [DOI confirmed via library header]
S13 Besl and McKay, A method for registration of 3-D shapes, TPAMI 1992: https://doi.org/10.1109/34.121791 [M]; Chen and Medioni, Object modelling by registration of multiple range images, IVC 1992: https://doi.org/10.1016/0262-8856(92)90066-C [M]
S14 Rusu, Blodow, Beetz, Fast Point Feature Histograms (FPFH), ICRA 2009: https://doi.org/10.1109/ROBOT.2009.5152473 [M]
S15 Lunt et al., Model predictions and observed performance of JWST's cryogenic position metrology system (SPIE 2016): https://ntrs.nasa.gov/api/citations/20160007994/downloads/20160007994.pdf [fetched, text extracted locally]
S16 CNX Software, Trackit specification table: https://www.cnx-software.com/2025/05/28/revopoint-trackit-an-high-end-optical-tracking-3d-scanner-with-marker-free-scanning/ [fetched]
S17 Trackit SR product page: https://www.revopoint3d.com/products/wireless-tracking-3d-scanner-trackit-sr [snippet]
S18 MIRACO Plus listings: https://jbhifi.com.au/products/revopoint-miraco-plus-3d-scanner ; https://3dprinting.com/scanners/best-3d-scanners-for-large-objects/ [snippets; second is cited by the project report]
S19 Raptor listings and review: https://www.sainsmart.com/products/creality-cr-scan-raptor-3d-scanner-for-3d-printing [snippet]; https://www.creality.com/products/creality-cr-scan-raptor [fetched, no spec text]; https://www.3dtechvalley.com/creality-cr-scan-raptor-review/ [via project report]
S20 EinScan Rigil Lite page (and Rigil FAQ): https://eu.einstar.com/products/einscan-rigil-lite [fetched]; retailer: https://www.makerpoint.nl/en-gb/3d-scanning/3d-scanners/shining3d-einscan-rigil-210000007279 [snippet]
S21 Polycam export and rescale help: https://learn.poly.cam/hc/en-us/articles/27756102599572 ; https://learn.poly.cam/hc/en-us/articles/31430181212948 [snippets; rescale page returned HTTP 403 on fetch]
S22 VDI/VDE 2634 Part 3 contents: https://m.vdi.de/fileadmin/pages/vdi_de/redakteure/richtlinien/inhaltsverzeichnisse/1456386.pdf ; ISO 10360-10 overview (secondary): https://www.at-sensors.com/measurement/geometric-product-specification-din-en-iso-10360-10/ [snippets; standard texts not read]
S23 Project report `C:\Users\mabdu\REA\3D printed body kit toolchain.md` and its cited pages: https://blenderartists.org/t/car-photogrammetry-blender-carbon-fiber-parts/1641502 ; https://www.allaboutthebuild.com/blog/2025/2/iphone-vs-professional-3d-scanner-for-3d-printed-car-parts-the-full-test ; https://www.revopoint3d.com/blogs/blog/from-a-porsche-chassis-to-a-carbon-fiber-body-kit-3d-scanning-case-study ; https://dozuki.umd.edu/Guide/Parts+Comparison/153

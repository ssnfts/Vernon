# WP8: external photogrammetry runtime contract

Status: implementation and measured runtime contract, 2026-10-30. **The 20-photo public-sample feature/sparse/CUDA-dense/Poisson/scale smoke test passed in run `wp8-real-20261030-b`; a separate sparse Delaunay node trial also passed.** Full-workspace verification remains root-owned and in progress; this is not a complete-dataset, metrology or manufacturing-quality claim. This document refines the process-driver portions of [capture-scanning.md](capture-scanning.md); it does not replace the scale or marker algorithms.

## Scope and evidence boundary

The four existing node IDs remain `capture.sfm_features_match`, `capture.sfm_reconstruct`, `capture.mvs_dense`, and `capture.mvs_mesh`. They run a separately installed program; no native library is linked. COLMAP's own code licence is BSD-3-Clause; dependency and photo licences are separate. See [the official licence page](https://colmap.github.io/license.html) and `CLEAN_ROOM.md`.

Sources used here are official public documentation, official release metadata, the existing Vernon interfaces, and observed CLI help. No proprietary install, CGAL/TetGen implementation, vendor artwork, or vendor sample content is copied into Vernon. Dataset images, binaries, caches and generated reconstructions stay outside version control. Native height-field meshing remains an explicitly labelled white-box stand-in; it is never evidence of successful external Poisson or Delaunay reconstruction.

The locally checked executable is `C:\Users\mabdu\REA\tools\colmap\4.2.1\bin\colmap.exe`. Its help reports version **4.2.1**, commit **bd1fcf6**, built **2026-09-29 with CUDA**. Approved runtime archive SHA256, rehashed locally on 2026-10-30, is `e9c5cbd84c2ea986d2e970a2473fc2d2e6b34a2cdcf5d3df2765c319a63af881`. Help alone proves only the build identity; run b additionally records successful `cudacc.cc` PatchMatch work on GPU index 0 and geometric fusion. The hardware model is not captured in this evidence record. Current web documentation identifies itself as 4.3 development documentation, so installed subcommand help takes precedence for accepted options and defaults.

## Existing node surface

| Node | Input | Output | Runtime settings retained |
|---|---|---|---|
| Feature/match | Image directory parameter | Database file, Report | Camera model, shared intrinsics, feature size/count/type, matcher, guided matching, GPU choice |
| Sparse reconstruction | Optional database file; image directory parameter | Selected model directory, cloud, Report | Mapper family, focal/principal refinement, optional initial image IDs |
| Dense reconstruction | Sparse model directory; image directory parameter | Cloud, Report | Maximum image size, geometric consistency, fusion minimum pixels, patch radius |
| External mesh | Cloud; workspace path parameter for Delaunay | Mesh, Report | Mesher, simplification ratio and boundary weight, white-box cell size |

Every node retains `run=false` by default, an executable path, timeout, and existing overwrite controls. A specified executable wins over `VERNON_PHOTOGRAMMETRY_EXE` and PATH discovery. No search enters proprietary installation directories. Files owned by the node are written only below `ctx.work_dir`; source images and connected upstream models/databases are inputs, not deletion targets.

Planning mode writes `commands.txt` and all inputs that Vernon itself can generate, including Poisson's `cloud.ply`, even without the executable. Paths to prerequisite outputs of earlier planned stages may remain prospective and must be identified as such. `run=false` starts no external command, including help discovery; cached verified help or a clearly identified compatibility template can provide the offline plan. Planning never claims that a database, reconstruction, dense cloud, or external mesh was produced. The mesh fallback Report retains the required `white-box placeholder:` prefix.

## Verified command family

The following argv templates describe separate process invocations; `<...>` tokens are paths, not literal arguments. Pass argv directly instead of constructing a shell command. This sequence follows the [official CLI documentation](https://colmap.github.io/cli.html), checked against 4.2.1 help.

```text
feature_extractor --database_path <database.db> --image_path <images>
exhaustive_matcher --database_path <database.db>
mapper --database_path <database.db> --image_path <images> --output_path <sparse-root>
model_converter --input_path <selected-model> --output_path <text-model> --output_type TXT
image_undistorter --image_path <images> --input_path <selected-model> --output_path <dense> --output_type COLMAP
patch_match_stereo --workspace_path <dense> --workspace_format COLMAP --PatchMatchStereo.geom_consistency 1
stereo_fusion --workspace_path <dense> --workspace_format COLMAP --input_type geometric --output_path <dense/fused.ply>
poisson_mesher --input_path <oriented-cloud.ply> --output_path <mesh.ply>
delaunay_mesher --input_path <dense> --input_type dense --output_path <mesh.ply>
mesh_simplifier --input_path <mesh.ply> --output_path <simplified.ply> --MeshSimplification.target_face_ratio <ratio> --MeshSimplification.boundary_weight <weight>
```

`sequential_matcher --database_path <database.db> --SequentialMatching.overlap <count>` is the supported small-sequence alternative. An ignored public-sample test starts with SIFT, exhaustive matching, and incremental mapping; optional learned features, vocabulary files and global/hierarchical variants do not become validated merely because they appear in the UI.

| Setting | 4.2.1 argument verified by help | Compatibility rule |
|---|---|---|
| Extractor image size | `FeatureExtraction.max_image_size` | Older-help candidate: `SiftExtraction.max_image_size` |
| Extractor GPU | `FeatureExtraction.use_gpu` | Older-help candidate: `SiftExtraction.use_gpu` |
| SIFT feature limit | `SiftExtraction.max_num_features` | Keep the detector-specific name |
| Matcher GPU / guided | `FeatureMatching.use_gpu`, `FeatureMatching.guided_matching` | Older-help candidates use `SiftMatching` |
| Incremental refinement | `Mapper.ba_refine_focal_length`, `Mapper.ba_refine_principal_point` | Do not forward incremental-only controls to another mapper blindly |
| Initial pair | `Mapper.init_image_id1`, `Mapper.init_image_id2` | IDs are identifiers, not image array positions |
| Undistortion size | `max_image_size` | Distinct from the PatchMatch image-size flag |
| Stereo size / radius | `PatchMatchStereo.max_image_size`, `PatchMatchStereo.window_radius` | Zero in Vernon means omit the optional override |
| Fusion support | `StereoFusion.min_num_pixels` | Zero in Vernon means omit the optional override |

Resolve options against the exact selected subcommand's help. If fetched help rejects all known candidates, report unsupported configuration before reconstruction; do not silently choose a known-absent flag. Bounded help discovery must not launch a GUI or reconstruct images. Cache help only for the same executable identity. Preserve installed defaults by omission where the node defines zero as default. For example, observed 4.2.1 extractor help gives maximum image size `-1`, whereas the current web page shows `3200`; neither is a universal default.

Geometric consistency determines the fusion source: `geom_consistency=true` uses `--input_type geometric`; false uses `--input_type photometric`. The false-mode path must retain filtering or explicitly request it when supported. Do not fuse nonexistent geometric maps after a photometric-only run. The [official FAQ](https://colmap.github.io/faq.html#speedup-dense-reconstruction) describes this quality/performance choice; installed fusion help accepts both source types.

## Capability and failure contract

On the selected Windows CUDA build, PatchMatch needs a usable CUDA device. SIFT extraction and matching can use CPU flags; undistortion and sparse reconstruction remain useful without CUDA. The presence of `nvidia-smi` on PATH is advisory and cannot prove executable support, available devices, compatible drivers, or sufficient memory. See [official GPU guidance](https://colmap.github.io/faq.html#available-functionality-without-gpu-cuda).

Run prerequisites in order and inspect actual failures. Missing CUDA must yield a Warning containing the failed command, bounded stdout/stderr tail, transcript path, and an actionable alternative. Distinguish missing device/build support from out-of-memory, invalid input, missing output, timeout, cancellation, and an unsupported flag. A GPU allocation failure is not proof that no GPU exists. Stop dependent stereo/fusion steps after failure; retain a successfully prepared undistortion workspace and useful sparse output.

The no-CUDA alternative is specifically:

```text
delaunay_mesher --input_path <valid-sparse-model-directory> --input_type sparse --output_path <sparse-mesh.ply>
```

The installed Delaunay help accepts `input_type={dense,sparse}` and defaults to dense. Sparse input includes cameras, image poses, points and visibility tracks; an arbitrary PLY is insufficient. This sparse alternative bypasses PatchMatch and fusion but has sparse-data quality and still needs successful input/output validation. Dense-mode Delaunay uses the completed dense workspace with fused points/visibility; undistortion alone does not create it. Do not automatically switch a requested mesher or treat a sparse mesh as a dense result. The node now exposes `delaunay_input_type=sparse` with the sparse directory supplied through `dense_workspace`; the separate measured trial below exercises that route. The [official surface reconstruction guidance](https://colmap.github.io/faq.html#surface-mesh-reconstruction) supports separate sparse and dense Delaunay use.

Every invoked stage records argv, executable identity, start/end or elapsed time, exit classification, output paths and full combined transcript. The Report includes bounded nonempty diagnostic lines and transcript location, including timeout/cancel paths. No nonzero exit or absent/invalid output is turned into apparent success. If the node API returns a Report with an Error note, downstream consumers must still be able to identify the failed external stage. Missing executable/run-off states are Warnings with no claim of execution.

Use a fresh result directory per acceptance run. If reuse/import is supported, label it as existing output and state its provenance separately. A skipped or failed run cannot import leftover `fused.ply` or an older mesh as its new result. Successful exit is necessary but insufficient: validate each expected artifact and imported geometry. A real requested mesh run with no faces must fail validation; never substitute height-field geometry for it.

The database handoff performs a bounded SQLite plausibility check (regular readable file, more than its 16-byte signature, correct signature). This does not certify schema, feature counts or usable matches; the subsequent real sparse reconstruction is the meaningful check. A failed requested feature/sparse run returns an absent sentinel File path below its work directory, selected from a bounded set of names, so downstream nodes cannot mistake an older on-disk database/model for its new output.

External mesh import can remove exactly zero-area fan triangles while retaining positive-area triangles. It records `degenerate_faces_removed` and a Warning when cleanup occurs, preserves applicable primitive attributes/groups while triangulating retained pieces, and fails if no usable surface remains or triangle-area arithmetic is nonfinite. This is explicit import cleanup; it does not guarantee closure, manifoldness or manufacturing quality, and it does not replace a failed run with a synthetic surface.

Graph caching fingerprints declared input directory metadata recursively within 32,768 entries and 32 directory levels. Unreadable, linked/reparse, special or over-budget trees are uncertifiable rather than assigned a successful partial fingerprint. Action-off readers recook conservatively and propagate that status downstream; an enabled action on uncertifiable inputs fails before the operation executes. External drivers, including geometry-only outputs, obey deferred-write handling during editing. Metadata identity is not content hashing or an atomic snapshot; edits preserving both size and modification time can evade it.

## Model and geometry handoff

Mapper may produce multiple numbered components. The current driver explicitly imports numbered model `sparse/0`; it does not rank or merge disconnected components and makes no largest-component guarantee. The measured trial registered every selected image in that model. A missing model, unreadable file, malformed model or zero-point reconstruction is an error for a requested real run.

Use official `model_converter` to obtain TXT camera/image metadata; do not implement new camera/image binary parsers for WP8. When both point forms exist, fresh binary `points3D.bin` takes precedence, matching the [documented format preference](https://colmap.github.io/format.html). TXT poses and binary points must belong to the same selected reconstruction; conversion success and a fresh workspace establish this provenance. The importer permits noncontiguous IDs and image filenames containing spaces, preserves paired image records (including empty observation lines), validates finite complete observation triples and point-track pairs, and propagates read failures instead of treating them as absent files. Cross-table track referential integrity and mixed-generation certification are not established by parser syntax checks alone.

Sparse points preserve RGB as `Cd` in [0,1], reprojection error in pixels, and track length. Dense/mesh PLY is read through `vernon_io`, including binary PLY, colours and normals where present. Reject invalid geometry, nonfinite coordinates and empty outputs. Poisson input needs finite nonzero point normals; no fabricated normals are introduced merely to make the command run.

Monocular reconstruction remains arbitrary-scale geometry: sparse and dense clouds are `metric=false`. Poisson meshes inherit the cloud's scale status because that cloud is their coordinate source. A real Delaunay run uses workspace/model coordinates instead, so its result stays non-metric and warns that scaling the separate connected cloud did not scale those cameras/workspace. Camera conventions and arbitrary orientation remain unchanged unless a distinct transform is applied; a successful reconstruction alone does not establish millimetres or Z-up. Existing `capture.scale_from_distance` / `capture.scale_bar_solve` provide the explicit metric handoff. Do not apply a metric transform to cloud points and then use untransformed cameras as though they shared a frame.

## Public-photo sample and opt-in tests

The real sample is **South Building**, the 128-photo UNC capture credited to Christopher Zach by the [official dataset page](https://colmap.github.io/datasets.html). Its [official linked host](https://demuc.de/colmap/datasets/) resolves to [south-building.zip, release 3.11.1](https://github.com/colmap/colmap/releases/download/3.11.1/south-building.zip). This older dataset package is independent of the selected runtime version. The measured run uses the first 20 sorted photos, `P1180141.JPG` through `P1180160.JPG`, not all 128.

Official [asset metadata](https://api.github.com/repos/colmap/colmap/releases/assets/213120796), checked 2026-10-30, gives **419,421,847 bytes** (about 400 MiB), asset ID `213120796`, and `digest=null`. No published archive checksum was found in the inspected listing, release body, or asset metadata. The authorized local archive at `scratch/wp8-data/south-building.zip` was rehashed as **`d210016bd2de20936a5f02b87fd38a76bf0440c42d045231218372cf9db9a7a1`**; this is an observed local hash, not an upstream-authenticated checksum. Gerrard Hall has fewer photos but its official archive is larger: 1,005,335,375 bytes.

The inspected pages do **not** state a separate photo licence. COLMAP's BSD code licence does not establish image redistribution rights. The saved extraction status reports no archive licence/readme entries, so an image licence grant remains **not established**. Images and derived dataset packages stay local and uncommitted. This is a publicly offered evaluation sample, not a newly asserted permissively licensed asset pack.

Integration tests are `#[ignore = "needs COLMAP"]`. Default tests need neither the executable nor network. `VERNON_COLMAP_DATA` is the explicit data opt-in: unset means no download; an existing local folder means use supplied images read-only. If a download mode is supported, it uses only the fixed official URL after explicit opt-in, downloads/extracts inside a fresh bounded temporary directory, rejects escaping archive paths, and records URL, size, local hash and image manifest. No hidden learned-model download is needed for the SIFT acceptance path.

Archive-supplied databases or reconstructed models are not new pipeline evidence. The feature/match acceptance starts from images and a fresh owned database. Any subset is a separately named trial with exact sorted filenames and image hashes; it must not be described as reconstruction of all 128 images. A generated multi-view synthetic scene is useful for deterministic regression and known geometry, but cannot satisfy the handoff's real public-dataset requirement.

### Minimum gates and observed run b

These are Vernon smoke-test gates, **not published dataset benchmark results**. Run b used preselected SIFT/exhaustive CPU extraction and matching at 800 px maximum image side and 4096 features, then incremental mapping and CUDA dense reconstruction at 800 px. The earlier 1200 px extraction suggestion was a planning value, not the selected fixture setting. Count/validity gates were not lowered after a failure.

| Stage | Minimum meaningful artifact/check | Measurement status |
|---|---|---|
| Fresh feature/match | Both stages exit zero; new nonempty database; sparse reconstruction subsequently demonstrates usable matches | Passed: 20 photos, 190 exhaustive pairs; 2 commands, both exit 0; 6.939 s |
| Sparse | At least 3 registered images and 100 finite points; nonzero 3D extent; readable valid camera/image metadata and track lengths; geometry validation passes; finite nonnegative mean reprojection error; `metric=false` | Passed: 20/20 registered, 2,703 points, mean reprojection error 0.6952701826 px; mapper/converter exit 0; 2.110 s |
| Prepared workspace | Undistorted image(s), sparse model and stereo configuration produced by this run; selected model path recorded | Passed: fresh `dense/dense` workspace from `sparse/sparse/0`; 40 depth maps and 40 normal maps retained after both stereo passes |
| CUDA stereo/fusion | Stereo and fusion exit zero; matching depth/normal maps exist; imported fused PLY has at least 1,000 finite points, valid colour/normal arrays when present, nonzero extent; `metric=false` | Passed: 233,744 fused points; undistort/PatchMatch/geometric fusion all exit 0; 117.320 s for the 3-command dense node |
| External mesh | Selected real mesher exits zero; imported PLY has at least 100 nondegenerate faces and finite vertices, valid indices and geometry validation; source route and scale flag recorded | Passed: Poisson exit 0; returned geometry has 293,974 vertices / 554,298 triangles; 104 exactly zero-area imported triangles explicitly removed; 78.406 s |
| Simplifier, when requested | Exit zero, valid nonempty mesh; face count no larger than source, ratio and final count recorded; no exact ratio promise | Not requested (`target_face_ratio=1`); unverified by this real trial |
| Scale handoff | Apply existing scale node to actual imported points; numerical factor/length and metric flag match the test's explicit reference | Passed separately on sparse cloud and Poisson mesh using declared 100 mm test references; geometry/primitive counts preserved and `metric=true` after scaling |

The measured scale fixture chose two distinct imported points with nonzero separation `d` and declared **test reference** `L=100 mm`, uncertainty `sigma=0.001 mm` (10 ppm). It checked `s=L/d`, transformed separation within `1e-8 mm`, preserved geometry/primitive counts, and `metric=true`. Sparse input separation was 29.470358680039812 arbitrary units, giving scale 3.39324000381881; the mesh's independently selected points were 9.051999472269221 units apart, giving 11.047283012592937. This checks plumbing and arithmetic on real imported geometry. It does **not** measure the building or prove physical accuracy. A physical scale-bar claim requires independently measured landmarks/bars and uncertainty, absent from the inspected sample description. The earlier 1000 mm planning reference was not used by this fixture.

A separate real sparse Delaunay node trial used the fresh sparse model, `mesher=delaunay`, `delaunay_input_type=sparse`, `target_face_ratio=1`, and a 2-minute timeout. It completed one command with exit 0 in **0.233 s** node time, producing **2,024 vertices / 4,025 triangles**, zero degenerate removals, valid geometry and non-metric output with the workspace-scale warning. Its transcript records no additional dense reconstruction commands. This verifies the sparse input route on the current host, not behavior on a physically CUDA-less host or dense-mode Delaunay. It does not substitute for the separately passed CUDA stereo/fusion gate.

No real trial here validates simplification, sequential/vocabulary/spatial matching, learned feature types, global/hierarchical mapping, photometric-only fusion or dense-mode Delaunay. The synthetic/mocked missing-CUDA tests remain distinct from actual device absence. Passing the smoke gates does not establish complete reconstruction of all 128 photos, dimensional accuracy, watertightness or suitability for body-kit manufacture.

## Measured evidence and reproducibility

Run status is saved at `scratch/continuation/runs/wp8-real-20261030-b/status.json`: completed, exit 0, one executed/passing ignored full-branch test. UTC window is 2026-10-30 08:07:57 to 08:11:31. Source fingerprint before and after is unchanged: `64b54e8147204386444277762e4f566b2888851002a7733c55506364238272e6`. `wp8-real.stdout.log` contains the passing Rust summary; `wp8-real.stderr.log` contains the stage metrics and post-assertion `WP8_REAL_COMPLETE` marker. Its elapsed node-chain measurement is **204.997 s**; the test harness reports 205.25 s. Timings are observations of this host/configuration, not universal performance limits.

Local fixture/evidence root:

```text
C:\Users\mabdu\REA\vernon\scratch\continuation\runs\wp8-real-20261030-b\temp\vernon-wp8-real-63600-1793347686143423200
```

It contains `fixture.json` (official source, exact sorted photo list, executable, fresh-work declaration), `version.log`, and each node's `commands.txt`, `colmap.log` and saved `report.json` under `features`, `sparse`, `dense` and `mesh`. Scale Reports are under `scale-sparse` / `scale-mesh`. The CPU trial has `cpu-delaunay/commands.txt` and `colmap.log`, with its node result captured in `scratch/wp8-sparse-delaunay.log`. Selected images were copied from the local authorized sample; every feature database, sparse model, dense workspace and mesh started absent. No archive-supplied database/model was used.

All main nodes used `run=true`, the explicit 4.2.1 executable and `timeout_min=20`. Extraction used `SIMPLE_RADIAL`, `single_camera=false`, CPU SIFT and CPU exhaustive matching. Mapping used incremental defaults (focal refinement enabled, principal-point refinement disabled, automatic initial pair). Dense settings were geometric consistency true, image side 800, window radius 3 and minimum fusion support 3; GPU index 0 is recorded in PatchMatch's actual log. Poisson used the fused cloud's finite point normals, ratio 1 and no simplification. Exact argv and remaining program-provided defaults are retained in the node transcripts.

Selected-photo manifest SHA256 is `ca7f410abda9b6f5b6e75f09f5b82e9e6cc249d170ba7de307f89f3c37401c53`, locally computed over UTF-8 lines `<filename> <lowercase-SHA256>`, sorted by filename, joined by LF with no final LF. The component hashes below identify the actual 20 copied inputs; these are metadata only, not redistributed photos.

```text
P1180141.JPG be3e0dff291078c936462a00c5fd9c28c96607654395fbd320769ae0b3732b3f
P1180142.JPG ac400c467eb5240d4bceea84f7291fd1798079aca75cfd76eafcce5656fa6346
P1180143.JPG 92d887729eaf159bcc3900e454e089fdfd810b6ca527eefbd9af040decb776d4
P1180144.JPG 9b160183250da6bac0c025a3635508c544c9aaa85cecc56da20dbbeef0eb9f63
P1180145.JPG 8dbe80258e34e7d676cc25c4ae6f14374640456b5197d289663e075a80dd785b
P1180146.JPG 9589a333091bacc27f71499ef0efad3c3e2a8d5eb28067d19367f3c5e368ad3e
P1180147.JPG afb5b97398083c17434264ecae7641fad27b0e2cb8f173d65b58112de81545bb
P1180148.JPG f3912b040d30bbf1e70723c3644df8d712961d5fed9f2c6fa8e57c9bf5660cb6
P1180149.JPG 5714aad939d47ff64b3c9d017811ba4ea81a280880807b6c106301875fb35daf
P1180150.JPG d8d996dfc0bbcfd602ae9cc948eaedbaf0f170698963dd23e0a374680eca1ff0
P1180151.JPG 6911d703ad3786b792053321c88d63974498eebad0a6a3ea828ba8f46c4d92fd
P1180152.JPG dfafd450716c7bfc8076602c6c31338bf6327eb77348d217345a17694d7dfede
P1180153.JPG 5350d618b91c516423f975f32d74115396e5ef9422fcea928bb6cc6d22364b30
P1180154.JPG 70d06a0820eac49ee5b1976532aa4aa2d003554bece613ea7355ead5c88cfacc
P1180155.JPG 457166a16739cf2b15f1985344298684df475434a8653f61ccc4e0ec18f50b1e
P1180156.JPG 7eb19f9cfc3b1ca342f2f2c8999c89f5bee47c6542d5f8b0fb3e6b2fc5d668a5
P1180157.JPG f14c1e1915b79312b2328475dc91ec39066f3c184ec85a6ae81e0c19a5ab481e
P1180158.JPG d9159f4194ff75c6afe8d8b05fdefffa238f7657df1a70b63cf5aec4fc9572c9
P1180159.JPG 4eed5ee2459747f86d9a15a758355c6637d896e70e48ac6f9aa510b72a77fe70
P1180160.JPG ea0bc7b0f4bbe41cd2237c0d7e7e647326ce1322dee0975ccb5f76dc074bef41
```

The archive, photos and reconstruction products remain local-only. Required remaining release checks include the active full-workspace verification; real-method variants marked unverified above are outside this measured smoke acceptance. No physical scale-bar accuracy, surface closure, manufacturing quality or performance benchmark is inferred from these counts.

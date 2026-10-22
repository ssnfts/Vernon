# Spec: split, key and print stage (slicing / printing)

Clean-room spec for the Vernon `split.*` and `print.*` nodes. Written from public
docs, papers and open-source behaviour only (see section 6). Reference tools may be
named here; they must never appear in user-visible strings (CLEAN_ROOM.md).
Confidence tags: **[doc]** read in a public source this pass, **[report]** taken from
`3D printed body kit toolchain.md`, **[recalled]** general knowledge, verify before
relying on it, **[design]** a Vernon choice, not a claim about any tool.

## 1. Scope and reference tools

Stage contract: input is a clean, closed, surfaced body panel (mesh, mm); output is a set
of printable parts plus numbers the later stages check (part count, fit margin, seam
area, strength ratio, heat margin, time, filament).

| Tool | Version basis | Role |
|---|---|---|
| OrcaSlicer (AGPL-3.0) | 2.4.x stable; 2.5.0 alpha dated 16 Oct (Python plugin API) | primary slicer, run as a separate process |
| PrusaSlicer (AGPL-3.0) | current help pages, version not stated | origin of the cut tool lineage; fallback |
| 3MF Core spec | GitHub master at fetch time | neutral job and part exchange |
| Klipper 0.13.0 [report] / Moonraker | current docs | firmware target and upload API |
| Marlin, Marlin 2, RepRapFirmware, Repetier | flavours only | G-code dialect targets |

Out of scope: warpage prediction and FEA inside the slicer (no mainstream slicer does it
[report]); support-tree algorithms; multi-material purge logic; mesh repair (clean stage).
Vernon drives the slicer through files and the command line and never links its code.

## 2. Feature catalogue (observable behaviour)

### 2.1 Cut tool (planar, dovetail, connectors)

Observed in the Prusa help page [doc] and the Orca-family forks [doc/search snippets].

| Aspect | Behaviour |
|---|---|
| Entry | select one object, press C or pick the tool; a plane widget appears |
| Planar mode | one plane; position per axis (Z entry in mm), rotation by widget arrows; two-point pick with Shift defines a plane perpendicular to the current view |
| Dovetail mode | trapezoid tail plus matching groove on the plane; extra arrow slides the tail along the plane; parameters: depth, width, flap angle, groove angle, tolerance |
| Connectors (planar only) | left-click adds, right-click removes; default shape is a round prism |
| Plug | positive on one side, matching void on the other |
| Dowel | void in both sides plus an extra loose pin object to print |
| Snap | snap-fit arm on one side, recess on the other; community notes two tuning values: "space" (flex room) and "bulge" (overshoot past the hole) |
| Connector options | style, shape, depth, size, rotation; "flip cut plane" swaps which half gets plug or snap (n/a for dowel) |
| Tolerance | shrinks the male feature on all sides like a hole-compensation value; examples 0.25 mm (forum), 0.15 mm slip-fit-and-glue, 0.10 mm press fit (third-party video) [search snippets, treat as starting points] |
| Result choices | keep upper, keep lower, or both; each kept half may keep orientation, lie cut-face down, or be flipped; "cut to parts" keeps two sectors in one object with separate settings (not in dovetail mode) |
| Multi-region planes | if the plane crosses several islands, right-click an island to cut it or leave it joined |
| Reset | returns plane to start and removes connectors |
| Hard limits [report] | two parts per cut, one dovetail per cut, no grid split |
| Not published | numeric defaults, ranges, hole-generation method, manifold requirements. Record them later by running a licensed copy on our own test meshes (section 5) |
| Shape list [recalled] | triangle, square, hexagon, circle; style prism or frustum |

Vernon closes the limits: repeated cuts give N parts, dovetails can be arrayed along a
seam, and a grid/optimised split node produces many parts in one step.

### 2.2 Headless CLI slicing and export (Orca) [doc]

Invocation: `orca-slicer <inputs...> [flags]`. Non-dash arguments are input files in
order; `--` ends flag parsing.

| Rule or flag | Meaning |
|---|---|
| Boolean flag | on when present; use `=0` / `=1` to force. A space-separated `0` does not behave as expected, so never use it |
| Value flag | `--slice 0` or `--slice=0` |
| Vector flag | comma list, index from 0, missing entries copy the first value given, repeating the flag appends |
| Key mapping | setting `layer_height` becomes `--layer-height`; roughly 43 keys (preset identity, host credentials, internal indices) are blocked and give "Invalid option" |
| `--load-settings "a.json;b.json"` | process and printer presets, semicolon separated |
| `--load-filaments f.json` | filament presets |
| `--arrange 1` | auto arrange (seen in the doc example) |
| `--slice N` | 0 = all plates, i = plate i; other values invalid |
| `--export-3mf f` | write project, default `output.3mf`; `--min-save` strips non-print data |
| `--export-stl` / `--export-stls dir` | one merged STL / one STL per object |
| `--export-settings f.json` | dump effective settings, default `output.json` |
| `--export-slicedata dir` / `--load-slicedata dir` | cache sliced result; the two cannot be combined, and load cannot combine with `--repetitions` |
| `--mtcpp` | max triangles per plate, default 1 000 000 |
| `--mstpp` | max slicing seconds per plate, default 300 |
| `--no-check`, `--normative-check` | skip / run validity checks (check default on) |
| `--info` | print counts and dimensions of the unrepaired model |
| `--pipe name` | progress to a named pipe |
| `--uptodate`, `--load-defaultfila` | migrate loaded 3MF settings; default filament for unassigned objects |
| Exit codes | 0 ok, -2 invalid parameter, -3 input not found [search snippet, verify] |
| Not found in docs | the flag that chooses the G-code output folder. Vernon must probe `--help` of the installed build at run time instead of hard-coding it |

Body-panel consequence: a fine mesh above 1 M triangles is rejected unless `--mtcpp` is
raised, and a long slice needs `--mstpp` above 300.

### 2.3 3MF core basics [doc]

- Package is a ZIP (Deflate or stored); consumers must read ZIP64 and stream. The root
  model part is found through the package start-part relationship; the recommended name
  is `/3D/3dModel.model` but real files vary in case, so resolve through `_rels/.rels`.
- Root element `model` with `unit` (default `millimeter`; also micron, centimeter, inch,
  foot, meter), `xml:lang`, namespace `http://schemas.microsoft.com/3dmanufacturing/core/2015/02`.
- Children: `metadata` (name required; well-known: Title, Designer, Description,
  Copyright, LicenseTerms, Rating, CreationDate, ModificationDate, Application),
  `resources`, `build`. Resource ids are positive integers below 2^31.
- `object`: `id`, `type` (model default, solidsupport, support, surface, other), optional
  `pid`, `pindex`, `thumbnail`, `partnumber`, `name`; holds `mesh` or `components`.
  `other` objects cannot be build items. Component targets must be defined earlier.
- `mesh`: `vertices/vertex x y z` (implicit zero-based index) and `triangles/triangle v1 v2 v3`
  (+ optional `p1 p2 p3 pid`). A solid needs at least 4 triangles, distinct indices.
- Winding counter-clockwise seen from outside, normal (B-A)x(C-A) points out; every
  shared edge is traversed in opposite directions; each edge shared by exactly two
  triangles for `model`/`solidsupport`; negative-volume meshes are not printed.
  Self-intersections must be tolerated by readers (positive fill rule).
- `build/item`: `objectid`, optional `transform`, `partnumber`. Unreferenced objects are
  never output. Transform = 12 numbers `m00 m01 m02 m10 m11 m12 m20 m21 m22 m30 m31 m32`,
  last column implied 0 0 0 1, translation in the last three; use the row-vector form
  p' = p*M (confirm in the spec figure). Reject near-singular matrices; a negative
  determinant must not flip volume sign.
- Coordinates: right-handed, +X right, +Y back, +Z up, origin front-left-bottom of the bed.
- Extensions: `requiredextensions` (reader must refuse if unsupported),
  `recommendedextensions` (warn). Thumbnails are PNG or JPEG.
- Content types [recalled, confirm in Appendix C]: model part
  `application/vnd.ms-package.3dmanufacturing-3dmodel+xml`; relationships part
  `application/vnd.openxmlformats-package.relationships+xml`; start-part relationship type
  `http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel`.
- Slicer projects add vendor parts (configs, plate data). A reader keeps unknown parts
  byte-for-byte and never depends on them. Orca can also emit a packaged `.gcode.3mf`
  (per-printer option "use 3MF instead of G-code", default off, export only) and, since
  2.4.2, a `plateindex` field so hosts print the right plate [doc].

### 2.4 G-code flavours

Orca setting `gcode_flavor` takes `marlin`, `marlin2`, `klipper`, `reprapfirmware`,
`repetier` [doc]. Related printer settings [doc]: `use_relative_e_distances` (default on,
needed for the wipe tower), `use_firmware_retraction` (G10/G11, experimental),
`disable_m73` (omit remaining-time M73), `gcode_skip_config_block`, `thumbnails`
(`WxH/PNG` list), `enable_power_loss_recovery` (Marlin 2 and one vendor only),
`bed_temperature_formula` (`by_first_filament` | `by_highest_temp`),
`build_plate_tilt_x/y` (deg, 0 = level), `pellet_modded_printer` with
`pellet_flow_coefficient`, equivalent filament diameter d = sqrt(4 / (coef * pi)).

Klipper dialect [doc]: `EXCLUDE_OBJECT_DEFINE NAME= [CENTER=x,y] [POLYGON=[[x,y],..]]`,
`EXCLUDE_OBJECT_START NAME=`, `EXCLUDE_OBJECT_END [NAME=]`; `SET_PRESSURE_ADVANCE
[EXTRUDER=] [ADVANCE=] [SMOOTH_TIME=]`; `SET_PRINT_STATS_INFO TOTAL_LAYER= CURRENT_LAYER=`;
`SET_GCODE_VARIABLE MACRO= VARIABLE= VALUE=`; standard M104/M109/M140/M190/M73/M117.
Extended commands are `NAME PARAM=value`, case-insensitive.

Expected per-flavour deltas [recalled, verify against each firmware doc before coding]:

| Item | marlin | marlin2 | klipper | reprapfirmware | repetier |
|---|---|---|---|---|---|
| Pressure advance | M900 K | M900 K | SET_PRESSURE_ADVANCE | M572 D S | none or vendor |
| Accel command | M204 S | M204 P R T | SET_VELOCITY_LIMIT ACCEL= or M204 | M204 P T | M201/M204 |
| Object cancel | none | none | EXCLUDE_OBJECT_* | none | none |
| Pre-print macro | inline | inline | macro call (name from profile) | macro file | inline |

### 2.5 Printer-host upload (Moonraker) [doc]

`POST /server/files/upload`, `multipart/form-data`. Field `file` required. Optional
`root` (`gcodes` default, or `config`), `path` (sub-folder created if missing), `checksum`
(client SHA-256 hex; mismatch gives HTTP 422), `print` (default false; only for `gcodes`).
Success is HTTP 201 with `Location` header and JSON `item{path,root,modified,size,permissions}`,
`print_started`, `print_queued`, `action:"create_file"`. `GET /server/files/list?root=`
returns File Info entries. `GET /server/files/metadata?filename=` returns
`estimated_time` (s), `filament_total` (mm), `filament_weight_total` (g), `slicer`,
`slicer_version`, `thumbnails` when the slicer wrote them. Default port 7125 and the
`X-Api-Key` header are [recalled]. Orca 2.4 added native Moonraker hosts [doc].

### 2.6 Orca 2.5 Python plugin API (alpha) [doc]

Plugins are one `.py` or a `.whl`; a Python runtime ships with the app. Event hooks: project
open/save/close, slice and G-code export start/finish, object and plate changes, preset
select/save, printer discovery/connect, upload and print job start/finish. Capabilities:
script actions, slicing-pipeline steps (edit sliced layers, post-process G-code), pages and
dockable panels, resource texture packs. Named calls: `orca.host.app_info()`,
`orca.host.preset.bundles.reload_local(bundle_id)`. Permission prompts guard file access
outside the data folder, network, process launch and threads; the notes say plainly it is not
a sandbox. The notes say the API may change, and pipeline plugins are experimental. Decision:
Vernon does not build on it now; it only emits a thin example script later (node 20).

### 2.7 Orientation and anisotropy numbers [report + doc]

| Material | Upright / flat hook failure load | HDT at 0.45 MPa |
|---|---|---|
| PLA | 40 kg / 73 kg = 0.55 | 57 C |
| PETG | 25 / 55 = 0.46 | 69-72 C |
| ASA | 17 / 57 = 0.29-0.30 | 100-103 C |
| PET-CF | not given | 205 C |
| PAHT-CF | not given | 194 C; 84.8 C unannealed vs 190.7 C annealed |
| PPA-CF | Z 57 MPa vs XY 168 MPa = 0.34 | 227 C (196 C at 1.8 MPa) |

The hook tests used one printer with stock profiles; layer height, walls and infill were not
stated, so the ratios are a rough prior, not design allowables. Rule from the report: put
aero loads in-plane and size mounts against the Z allowable. An enclosed ASA run at about 30 C
ambient held 20 kg upright, 20% more than open air.

## 3. Algorithms

**A1 Plane cut of a triangle mesh** (standard clipping; Sutherland-Hodgman is the polygon
ancestor).
1. Unit normal n, offset d. Signed distance s_i = n.v_i - d. Snap |s_i| < eps to 0 with
   eps = 1e-6 * bbox diagonal [design].
2. Triangle with all s >= 0 goes up, all s <= 0 goes down, mixed is split. New vertices are
   created once per edge through a hash on the ordered vertex pair, so both neighbours share them
   and the result stays manifold. One lone vertex on a side gives 1 triangle plus 1 quad (2 triangles)
   on the other side; a vertex exactly on the plane gives 2 triangles.
3. Every split triangle contributes one on-plane segment. Chain segments through shared
   vertex ids into closed loops; an open chain means the input was not closed, so raise `E_OPEN_LOOP`.
4. Project loops into a 2D frame on the plane. Nesting depth by point-in-polygon: even depth =
   outer, odd = hole. Orient outers CCW, holes CW about n.
5. Fill each outer plus its holes: bridge holes to the outer loop and ear-clip (Eberly), or use a
   constrained Delaunay triangulation if available. Upper cap faces -n, lower cap faces +n.
6. Check: volume(up)+volume(down) = volume(input) within 1e-9 relative; cap area equals sum of
   signed loop areas; both outputs are closed and 2-manifold.

**A2 Joints by constructive geometry.** Build male and female features analytically
(prisms, frusta, trapezoid prisms) in the plane frame, then subtract/union with the cut halves
using a robust mesh boolean (in-house exact-predicate boolean or an external tool; unproven, so
this is white-box-first for snap and for curved seams).
- Plug pair: male prism of radius r - t and depth h on half A; void of radius r and depth
  h + gap_z on half B (gap_z default 0.3 mm [design]). Tolerance t shrinks the male on every
  side [matches the community meaning].
- Dowel: void radius r, depth h in both halves; separate pin of length 2h - gap, radius r - t.
- Dovetail: with neck width w, depth d, flap angle a measured from the cut-plane normal, the free
  end width is w + 2 d tan(a) [design definition]; groove is the same trapezoid grown by t.
  Arraying k tails along a seam is allowed, which lifts the one-dovetail limit.
- Place connectors on the cap polygon: sample points at least r + wall_min inside the loop
  (distance-transform on the cap), reject points whose connector prism leaves the part (check
  by intersecting with both halves); spacing parameter drives count.

**A3 Grid and optimised split** (Chopper; Luo, Baran, Rusinkiewicz, Matusik, SIGGRAPH Asia
2012). Published goals: pieces fit the build volume, few pieces, inconspicuous seams, sound
structure, with automatic or user-guided search and custom connectors at interfaces. Details
beyond the abstract were not fetched; the following is a Vernon design in that spirit:
- Grid mode: along each axis i use k_i = ceil(extent_i / usable_i) slabs where
  usable_i = bed_i - 2*margin; cut positions evenly spaced, then optionally snapped to the
  nearest low-curvature or body-line-crossing position within +-slack. Lower bound on parts
  = product of k_i (rotation of the whole body may reduce it; try 24 axis-aligned rotations).
- Search mode: recursive binary partition (plane per node); cost = a*part_count +
  b*seam_area + c*visible_seam_length + d*fragility + e*unused_volume; greedy beam search with
  width W (default 8) [design]; a part is final once it fits the volume.

**A4 Fit check.** For each part test every orientation in a candidate set (6 axis-aligned,
plus face-normal clusters, plus 64 Fibonacci-sphere directions); fit margin = min over axes of
(usable_i - extent_i); report the best orientation per part.

**A5 Orientation score.** For a candidate up-vector u and layer height h:
- height H(u); layer count = ceil(H/h).
- overhang area A_o = sum of facet areas with n.u < -cos(theta_o), theta_o default 45 deg
  (support threshold, [design]).
- stair-step cusp per facet = h * |n.u| (rectangular deposition profile, normal distance of a
  step corner to the true plane); weight by area and visibility. The published
  anti-aliasing method (Song, Ray, Sokolov, Lefebvre, arXiv 1609.03032) bends paths within half
  a layer thickness to reduce this on shallow slopes; Orca 2.4 ships a Z anti-aliasing feature
  [report]. Vernon only estimates where it would matter (cusp > 0.3 h) and leaves the
  correction to the slicer.
- load cost: for load direction f with angle phi to u, effective allowable ratio r follows
  1/r^2 = sin^2(phi) + cos^2(phi)/k^2, with k the Z/XY ratio from section 2.7 [design
  interpolation, replace after tests]. Utilisation = F / (A * r * sigma_xy / SF).
- total = w_s*A_o + w_c*cusp_sum + w_a*(1/r_min) + w_h*H; defaults 1.0, 0.5, 2.0, 0.2 [design].
  Classic reference for build-orientation cost trade-offs: Alexander, Allen, Dutta 1998.

**A6 Heat gate** (report, solved by bisection). Steady balance for a panel insulated at the
back: alpha*G = h_c (T_s - T_a) + eps*sigma*(T_s^4 - T_sky^4); h_c = 5.7 + 3.8 V; eps = 0.9;
T_sky = T_a - 15 K; G = 1000 W/m2. Margin = HDT(material, annealed?) - T_s - safety, safety
default 10 K [design]. Our bisection reproduces the report table to 0.5 K.

**A7 3MF write.** Merge duplicate vertices (weld tol 1e-6 mm), orient CCW outward, emit
f32-safe decimals (up to 9 significant digits), one `object` per part, one `item` per
placement, `unit="millimeter"`, metadata Title/Application/CreationDate (ISO 8601). Deflate ZIP,
plus `[Content_Types].xml` and `_rels/.rels`. Reader: resolve start part, apply `unit` to mm, apply
item transforms in row-vector form, reject singular matrices.

**A8 External slice run.** Write the 3MF and the three JSON presets to a temp dir; probe the
binary (`--help`) for the output flag; run with `--slice 0 --export-3mf`, raising `--mtcpp` to
1.2 x triangle count and `--mstpp` to a time budget; read exit code and parse produced G-code.
G-code summary: count layers by layer-change comments or Z moves, sum E (honouring relative or
absolute E), read time/filament comments when present, detect flavour from command signatures.

**A9 Upload.** Compute SHA-256 of the file, `POST` multipart with `checksum`, optional `path`,
`print=false` by default; success only on 201 and matching `item.size`; then optionally
`GET /server/files/metadata` and compare `estimated_time` with the local estimate (warn above
25% difference [design]). `print=true` needs explicit user confirmation in the UI.

## 4. Proposed Vernon nodes

Labels are generic. "Now" = implement from this spec; "WB" = white-box-first (prototype and
validate against golden outputs before exposing).

| # | id | Label | Inputs | Outputs | Key params (default; range; unit) | Plan |
|---|---|---|---|---|---|---|
| 1 | split.plane_cut | Planar Cut | mesh, plane or 2 points + view dir | upper, lower, cap_area_mm2, loops | eps 1e-6*diag; keep both/upper/lower | Now |
| 2 | split.cut_region_select | Cut Region Select | cut result | per-island joined flags | mode cut or join | WB |
| 3 | split.grid_partition | Grid Split | mesh, build volume | parts[], part_count, plane_list | margin 2; 0-20; mm. slack 15; 0-50; mm | Now |
| 4 | split.smart_partition | Smart Partition | mesh, build volume, guide curves | parts[], cost_breakdown | beam width 8; 1-64. weights a-e | WB |
| 5 | split.plug_joint | Plug Joint | cut halves, cap | halves with plug/void | r 4; 1.5-15; mm. depth 6; 2-20; mm. tol 0.15; 0.05-0.4; mm. spacing 40; 10-200; mm | Now |
| 6 | split.dowel_joint | Dowel Joint | cut halves, cap | halves with voids, pin meshes | r 4; depth 8; tol 0.15 | Now |
| 7 | split.snap_joint | Snap Joint | cut halves, cap | halves, snap geometry | arm length 8 mm; flex room 0.6 mm; bulge 0.4 mm | WB |
| 8 | split.dovetail_joint | Dovetail Joint | cut halves, cap | halves with tails and grooves | depth 6; width 12; flap angle 15; 5-30; deg. tol 0.15; count 1; 1-20 | Now |
| 9 | split.build_fit | Build Volume Fit | parts, bed size, orientations | fit_margin_mm, best_orientation, fail_list | bed x y z; usable margin 2 mm | Now |
| 10 | print.orient_score | Orientation Score | part, layer height, loads, material | ranked orientations, H, A_o, cusp, 1/r | h 0.2; 0.05-0.6; mm. theta_o 45; 20-70; deg | Now |
| 11 | print.directional_strength | Directional Strength Check | load vectors, area, material | z_allowable_ratio, utilisation | SF 2; 1-5. k override | Now |
| 12 | print.heat_margin | Heat Margin Gate | surface temp, material, annealed flag | heat_margin_k, pass | safety 10; 0-30; K | Now |
| 13 | print.stair_step | Stair-Step Estimate | part, up vector, h | cusp_map, area_over_threshold | threshold 0.3 h | WB |
| 14 | print.package_write | Package Writer | parts, transforms, unit | 3MF file, validation report | unit mm; weld 1e-6 mm | Now |
| 15 | print.package_read | Package Reader | 3MF file | meshes, transforms, metadata | unit conversion on | Now |
| 16 | print.profile_build | Profile Builder | flavour, printer, filament, process | three preset JSON files | flavour (5 values); relative E on; pellet coef | WB |
| 17 | print.slice_external | External Slice Runner | package, presets, binary path | G-code path, exit code, log | plate 0; mtcpp auto; mstpp 600; s | Now |
| 18 | print.gcode_summary | G-code Summary | G-code | layers, time_s, filament_mm, filament_g, flavour guess | relative-E auto | Now |
| 19 | print.flavour_check | Flavour Conformance | G-code, flavour | violations list | strict off | WB |
| 20 | print.remote_upload | Remote Upload | G-code, host URL | item, print_started, metadata diff | root gcodes; path; print false | Now |
| 21 | print.script_bridge | Script Bridge | node graph subset | example plugin script | none (deferred until API stable) | WB |

Contract numbers for downstream checks: `fit_margin_mm >= 0`, `seam_area_mm2` (sum of cap
areas), `part_count`, `z_allowable_ratio`, `heat_margin_k >= 0`, `time_s`, `filament_g`.

## 5. Golden test ideas (numbers)

Inputs are meshes we build ourselves; reference-tool outputs are recorded from a licensed copy.

1. Cube 100 mm cut at z = 40: upper volume 600 000 mm3, lower 400 000 mm3, cap area 10 000 mm2,
   both closed manifold, total volume error < 1e-9 relative.
2. Sphere r = 50 mm (tessellated, 128 x 64) cut at z = 20: cap radius sqrt(2500-400) = 45.826 mm,
   cap area 6 597 mm2, cap volume pi*30^2*(150-30)/3 = 113 097 mm3; mesh result within 0.5% of these.
3. Plane through existing vertices (cube spanning z 0..100 cut at z = 100, the top face): returns one
   part and an empty other, no zero-area triangles.
4. Open mesh (cube with one face deleted): `E_OPEN_LOOP`.
5. Two-island input (two 20 mm cubes 50 mm apart) cut across both: cap area 800 mm2 in two
   loops; "join island" leaves one island uncut.
6. Grid split of a 600 x 300 x 100 mm slab on a 256 mm bed with margin 2 (usable 252): k = (3, 2, 1),
   6 parts, planes x = 200, 400 and y = 150, largest part 200 x 150 x 100, fit margin 52 mm. On an
   800 x 800 x 1000 mm bed (large-format class from the report): 1 part.
7. Dovetail: neck 12, depth 6, flap 15 deg gives free width 12 + 2*6*tan(15 deg) = 15.215 mm; tail
   shrunk by t = 0.15 vs groove: clearance 0.15 mm per side measured on the output mesh.
8. Plug pair: r = 4, depth 6, t = 0.15 gives male radius 3.85 mm, void radius 4.0 mm; male volume
   within 0.2% of pi*3.85^2*6 for a high facet count.
9. Orientation: strip 100 x 20 x 3 mm, h = 0.2: flat gives 15 layers, upright (100 mm up) gives
   500 layers; with an in-plane load, ASA ratio r(upright) = 0.30 vs 1.0 flat.
10. Strength: PPA-CF, F = 500 N, SF = 2, sigma_xy 168 MPa, sigma_z 57 MPa: required area in-plane
    500/84 = 5.95 mm2, along Z 500/28.5 = 17.54 mm2 (ratio 2.95).
11. Heat gate (T_a 45 C, still air): black alpha 0.95 gives T_s = 104.7 C; ASA 100 C fails (-4.7 K),
    PET-CF 205 C passes (+100.3 K); white alpha 0.30 gives 61.3 C, PETG 69 C passes (+7.7 K, fails
    with a 10 K safety); mid grey 0.60 gives 82.6 C, PLA 57 C fails (-25.6 K). With wind 3 m/s, T_a 50 C,
    black: 83.9 C. PAHT-CF unannealed (84.8 C) vs annealed (190.7 C) flips the black-panel verdict.
12. 3MF round trip: 10 mm cube, 8 vertices, 12 triangles, signed volume 1000 mm3; unit `inch` with a
    1.0 cube reads back as 25.4 mm edge, 16 387.064 mm3; item transform translating +50 mm in X moves
    bbox min x to 50; a mirror (det -1) must not produce negative volume; `.rels` points to the model part.
13. Pellet coefficient round trip: d = 1.75 mm gives coef = 4/(pi*1.75^2) = 0.4157; inverse recovers d.
14. CLI run: `model.3mf --load-settings "process.json;printer.json" --load-filaments fil.json --slice 0
    --export-3mf out.3mf` exits 0 and writes the files; a missing input exits with the not-found code;
    a model of 1.2 M triangles fails without `--mtcpp` and passes with `--mtcpp 1500000`.
15. Flavour conformance: klipper G-code for two objects contains one `EXCLUDE_OBJECT_DEFINE` per
    object before the first layer and matched START/END pairs; marlin output contains none.
16. Upload against a mock server: wrong checksum gives 422 and no `item`; correct upload gives 201
    with `item.size` equal to the local size and `print_started=false`.
17. Layer counts for a 100 mm tall part: h 0.2 gives 500, h 0.12 gives 834 (ceil). Stair-step flagging
    with threshold 0.3 h (cusp = h*|n.u|): a facet sloped 10 deg from horizontal has cusp 0.985 h and is
    flagged; one sloped 80 deg has 0.174 h and is not; the cut-off is slope 72.5 deg (cos = 0.3). The
    threshold is a Vernon tuning knob (node 13).

## 6. Sources

Read in this pass:
- https://help.prusa3d.com/article/cut-tool_1779 (cut tool modes, connectors, results)
- https://github.com/3MFConsortium/spec_core/blob/master/3MF%20Core%20Specification.md (3MF core)
- https://www.orcaslicer.com/wiki/releases/release_2_4_0 (ZAA, Moonraker host, 3MF-instead-of-G-code)
- https://github.com/OrcaSlicer/OrcaSlicer/releases (2.5.0 alpha, plugin API, 2.4.2 plateindex)
- https://www.orcaslicer.com/wiki/printer_settings/basic%20information/printer_basic_information_advanced (flavours, pellet mode)
- https://www.orcaslicer.com/wiki/cli/cli_mode and https://www.orcaslicer.com/wiki/cli/cli_actions (CLI)
- https://moonraker.readthedocs.io/en/latest/external_api/file_manager/ (upload, list, metadata)
- https://www.klipper3d.org/G-Codes.html (extended commands, EXCLUDE_OBJECT)
- https://cnckitchen.com/blog/comparing-pla-petg-amp-asa-feat-prusament (hook test, upright vs flat)
- https://pixl.cs.princeton.edu/pubs/Luo_2012_CPM/index.php (Chopper abstract and citation)

Seen only as search-result snippets (treat as unverified):
- https://arxiv.org/abs/1609.03032 and https://arxiv.org/pdf/1609.03032 (anti-aliasing for fused filament deposition)
- https://forum.bambulab.com/t/cut-menu-in-studio/152029 (tolerance meaning, 0.25 mm example)
- https://printpal.io/resources/how-to-connect-3d-prints-inside-your-slicer (0.15 / 0.10 mm clearance advice)
- https://wiki.anycubic.com/en/software-and-app/new-page-anycubic-slicer-beta(orca-version)/cut (dovetail parameter names; page body did not load)
- https://deepwiki.com/SoftFever/OrcaSlicer/10.2-cli-mode-and-headless-operation and https://printago.io/blog/orca-slicer-cli-reference (exit codes, flag list for 2.3.2)

Cited through the report, not re-read:
- https://forum.bambulab.com/t/bambu-studio-cut-feature/72041 (two-part, single-dovetail, no-grid limits)
- https://all3dp.com/4/massive-orcaslicer-update-lands-with-z-anti-aliasing-stronger-gyroid-infill-and-a-cloud-of-its-own/
- https://github.com/Klipper3d/klipper/blob/master/docs/Releases.md (Klipper 0.13.0)
- https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/outside-surface-heat-balance.html (convection relation)
- https://www.lesimprimantes3d.fr/dl/Bambu_Lab_Filament-guide.pdf ; https://wiki.polymaker.com/polymaker-wiki-pt/produtos-polymaker/mais-sobre-nossos-produtos/documentos/fichas-tecnicas/abs-asa/polymaker-tm-asa ; https://www.additive-x.com/shop/mpattachments/file/viewonline/id/916/product_id/2729 (HDT values)
- https://qidi3d.com/blogs/print-lab/best-filament-car-interiors-abs-asa (PAHT-CF annealing)
- https://www.theautopian.com/meet-the-guy-3d-printing-a-widebody-kit-for-his-mazda-miata/ (mould/plug production pattern)

Algorithm references (not fetched; standard texts):
- https://www.geometrictools.com/Documentation/TriangulationByEarClipping.pdf (Eberly, ear clipping with holes)
- https://doi.org/10.1145/360767.360802 (Sutherland and Hodgman, reentrant polygon clipping, 1974)
- https://doi.org/10.1016/S0010-4485(97)00083-3 (Alexander, Allen, Dutta, part orientation and build cost, 1998)

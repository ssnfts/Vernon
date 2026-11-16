# Roadmap: partial nodes and external routes

Status of the nodes that are honestly flagged as white-box (final ports and parameters, placeholder algorithm; the
catalogue in `NODES.md` counts them) or that depend on a program outside Vernon. These are not labelling bugs: each
node says so in its cook warning. The order is the order of value to the validation gates, because the gates are what
stop a wrong number reaching the next stage.

## Implemented as library modules, not yet wired into their nodes (2026-11-04)

Merged batches added tested algorithms for most items below. Each one compiles
and passes its own unit tests, but no node calls it yet, so the node keeps its
placeholder behaviour and white-box warning until it is wired in and its
report metrics are checked end to end:

| Item | Module |
|---|---|
| 1 slicer dialects | `vernon-nodes-print/src/dialects.rs` (**wired 2026-11-14** into `print.flavour_check`: metric `dialect_table_findings`) |
| 2 tuft detection from images | `vernon-nodes-perf/src/tuft_detect.rs` (**wired 2026-11-14** into `perf.tuft_analysis` for PGM/PPM photos; PNG not read) |
| 3 preset key schema | `vernon-nodes-print/src/profile_schema.rs` (**wired 2026-11-15** into `print.profile_build`: metrics `preset_unknown_keys`, `preset_mistyped_keys`; keys checked against the 621-key config block of OrcaSlicer 2.4.2; the four extension keys `build_plate_tilt_x/y`, `pellet_modded_printer`, `pellet_flow_coefficient` are not known to that version and are reported as a warning) |
| 4 script bridge | `vernon-nodes-print/src/script_bridge.rs`, `script_template.rs` (**wired 2026-11-15** into `print.script_bridge`, now implemented: it writes a post-processing script that inserts G-code lines at chosen layers; tested by running the generated script on a small G-code file and comparing with the Rust rules; the slicer itself has not run it yet) |
| 5 snap-arm geometry | `vernon-nodes-print/src/snap_geometry.rs` (**wired 2026-11-14** into `print.snap_joint`: stiffness, assembly, insertion and retention forces) |
| 6a islands | `vernon-nodes-print/src/island_select.rs` (**wired 2026-11-14** into `print.cut_region_select`: selectors `largest_island` and `island_area_above`; the node could only pick an island by a known index, which is wrong whenever debris precedes the body; metrics `selected_islands`, `selected_area_mm2`) |
| 6b cut projection | `vernon-nodes-print/src/polyline_project.rs` (**evaluated 2026-11-14, not adopted**: it cuts only along a closed loop lying on the surface. Box, closed ring: same 2 parts and volume as `print.polyline_cut` (error 2e-16 vs 0), 300 mm seam, 0.13 ms vs 1.1 ms. An open drawn line fails with NotSeparating, and a guide off the surface of the 3 mm car panel fails with FarFromMesh (61 mm); the node cuts the panel in 22 ms with volume error 2e-14 and reports cap area) |
| 6c split search | `vernon-nodes-print/src/partition_search.rs` (**evaluated 2026-11-14, not adopted**: it only chooses among drawn candidate seams. 700x300x200 box, 350 mm bed: 3 parts and 2000 mm seam, same as `print.smart_partition`, 0.2 ms vs 8 ms with two hand-made candidates. 1200x600x300 box: the node finds 8 parts (10200 mm seam, 46 mm margin, 59 ms); with a ring grid as candidates the module finds no plan or fails on a partial-ring candidate) |
| 7 stretch-aware constraints, ply shear | `vernon-nodes-solar/src/flatten_constraints.rs`, `ply_shear.rs` |
| strict CFD convergence | `vernon-nodes-cfd/src/convergence.rs` (compare with `cfd.run_convergence` before adopting) |
| mesh simplification | `vernon-nodes-clean/src/qem_simplify.rs` (**evaluated 2026-11-14, not adopted**: `clean.decimate` already runs the same quadric collapse with boundary and crease penalties and the link condition; on a 9024-face sphere at 10% and 30% both give the same max error (0.37 vs 0.35 mm, 0.106 vs 0.111 mm), mean error, volume and triangle quality, and `qem_simplify` is 5 to 6 times slower (81 vs 15 ms). Its only extra is boundary vertices that never move; kept as a library module) |
| surface reconstruction | `vernon-nodes-capture/src/ball_pivot.rs` (**wired 2026-11-14** as mesher `ball_pivot` of `capture.mvs_mesh`: native, no program; metrics `faces`, `points_used`, `points_unused`, `boundary_edges`, `nonmanifold_edges`, `holes`; Poisson and Delaunay still drive the external program) |

## First: items that feed a validation gate

1. **Slicer dialect tables** (`print.flavour_check`). Today it checks object-cancel markers, pressure advance and a few
   firmware commands. Needed: a full per-dialect table (command set, arc moves, pressure-advance syntax, object
   labelling, thumbnail comments) for the Klipper, Marlin 2 and RepRapFirmware dialects, each entry with the
   documentation URL it came from, so a G-code file is rejected before it reaches the printer.
2. **Tuft detection from photographs** (`perf.tuft_analysis`). The node classifies a CSV or JSON of tuft positions and
   measured angles as attached, separated, reversed or unsteady and maps the classes onto a surface. Needed: find the tufts in a
   still or video frame (thin line segments against the panel) and measure their angle to the free-stream direction,
   so the CSV no longer has to be made by hand. Until then a photo input only gets the white-box warning.

## Second: items that make a white-box node usable

3. `print.profile_build` writes preset keys from memory, not from a checked key list of the slicer. Needed:
   validate the keys against the documented schema of the slicer version.
4. `print.script_bridge` writes an inert example, so the plug-in route for connector placement is not usable. Needed:
   the documented scripting interface of the slicer and a round-trip test.
5. `print.snap_joint` falls back to a plain plug. Needed: the snap-arm geometry (arm length, flex room, bulge,
   engagement angle) with a fit test.
6. `print.polyline_cut`, `print.smart_partition`, `print.cut_region_select`: the cut-line projection, the search over
   split plans and the per-island choice are placeholders.
7. `flatten.stretch_aware` runs the as-rigid-as-possible map. Still open: separate rigid and floppy material weighting,
   hard stretch-only and compress-only constraints (today the opposite deformation is weighted 10000 times) and a
   fold-free line search. The ply mode (`fibre_dir`) reports the warp angle and weave shear.
8. `capture.sfm_*` and `capture.mvs_*` need an external COLMAP run for any real result; Vernon writes the commands and
   reads the outputs.

## Not planned in this repository

- ~~Transient detached-eddy cases.~~ Superseded 2026-11-11: `cfd.case_setup` writes a DDES case (`solver_mode =
  transient_ddes`, hybrid central/upwind convection, fixed or adjustable step, averaged fields) and
  `vernon-nodes-cfd/src/transient.rs` holds the time-resolved acceptance (wash-out, stop rule, per-step rule,
  half-step check). The run is driven by the test harness `real_openfoam_fast_tier_run`; a graph node that runs it
  and a runner profile are still open (`docs/research/wp9-fast-tier-plan.md`).
- A spectral UV model. `solar.uv_dose_proxy` scales total radiation and checks the material against published
  accelerated-test figures; there is no published factor from test hours to outdoor years.

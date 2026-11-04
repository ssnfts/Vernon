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
| 1 slicer dialects | `vernon-nodes-print/src/dialects.rs` |
| 2 tuft detection from images | `vernon-nodes-perf/src/tuft_detect.rs` |
| 3 preset key schema | `vernon-nodes-print/src/profile_schema.rs` |
| 4 script bridge | `vernon-nodes-print/src/script_bridge.rs`, `script_template.rs` |
| 5 snap-arm geometry | `vernon-nodes-print/src/snap_geometry.rs` |
| 6 cut projection, split search, islands | `vernon-nodes-print/src/polyline_project.rs`, `partition_search.rs`, `island_select.rs` |
| 7 stretch-aware constraints, ply shear | `vernon-nodes-solar/src/flatten_constraints.rs`, `ply_shear.rs` |
| strict CFD convergence | `vernon-nodes-cfd/src/convergence.rs` (compare with `cfd.run_convergence` before adopting) |
| mesh simplification, reconstruction | `vernon-nodes-clean/src/qem_simplify.rs`, `vernon-nodes-capture/src/ball_pivot.rs` |

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

- Transient detached-eddy cases. `cfd.case_setup` writes steady k-omega SST and flags separated-flow features so the
  force report warns; a DDES or SBES case writer is a larger piece of work.
- A spectral UV model. `solar.uv_dose_proxy` scales total radiation and checks the material against published
  accelerated-test figures; there is no published factor from test hours to outdoor years.

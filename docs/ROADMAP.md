# Roadmap: partial nodes and external routes

Status of the nodes that are honestly flagged as white-box (final ports and parameters, placeholder algorithm; the
catalogue in `NODES.md` counts them) or that depend on a program outside Vernon. These are not labelling bugs: each
node says so in its cook warning. The order is the order of value to the validation gates, because the gates are what
stop a wrong number reaching the next stage.

## First: items that feed a validation gate

1. **Slicer dialect tables** (`print.flavour_check`). Today it checks object-cancel markers, pressure advance and a few
   firmware commands. Needed: a full per-dialect table (command set, arc moves, pressure-advance syntax, object
   labelling, thumbnail comments) for the Klipper, Marlin 2 and RepRapFirmware dialects, each entry with the
   documentation URL it came from, so a G-code file is rejected before it reaches the printer.
2. **Tuft detection** (`perf.tuft_analysis`). Tufts are the cheap physical check of the airflow stage, but the node only
   reads an angle series or shows a synthetic example. Needed: find tufts in a still or video frame (thin line
   segments against the panel), measure their angle to the free-stream direction, and report attached, fluttering or
   reversed per tuft.

## Second: items that make a white-box node usable

3. `print.profile_build` writes preset keys from memory, not from a checked key list of the slicer. Needed:
   validate the keys against the documented schema of the slicer version.
4. `print.script_bridge` writes an inert example, so the plug-in route for connector placement is not usable. Needed:
   the documented scripting interface of the slicer and a round-trip test.
5. `print.snap_joint` falls back to a plain plug. Needed: the snap-arm geometry (arm length, flex room, bulge,
   engagement angle) with a fit test.
6. `print.polyline_cut`, `print.smart_partition`, `print.cut_region_select`: the cut-line projection, the search over
   split plans and the per-island choice are placeholders.
7. `flatten.stretch_aware`: rigid and floppy material weighting, hard stretch-only and compress-only constraints and
   a fold-free line search. The ply mode (`fibre_dir`) already reports the warp angle and weave shear.
8. `capture.sfm_*` and `capture.mvs_*` need an external COLMAP run for any real result; Vernon writes the commands and
   reads the outputs.

## Not planned in this repository

- Transient detached-eddy cases. `cfd.case_setup` writes steady k-omega SST and flags separated-flow features so the
  force report warns; a DDES or SBES case writer is a larger piece of work.
- A spectral UV model. `solar.uv_dose_proxy` scales total radiation and checks the material against published
  accelerated-test figures; there is no published factor from test hours to outdoor years.

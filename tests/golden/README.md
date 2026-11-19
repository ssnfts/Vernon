# Golden tests

Each case compares Vernon's output with the output of a reference tool, run on
an input that Vernon wrote itself. Allowed by CLEAN_ROOM.md, "Allowed sources"
item 3: the tools are only run, never inspected. Their install folders and
binaries are not opened. Recorded outputs are committed here; the tests in
`crates/vernon-cli/tests/golden.rs` read them, so no tool is needed to run them.

## Recording a case again

1. Write the input with Vernon: replace `__GOLDEN_DIR__` in the case's
   `make_input.vrn` with the absolute case folder and run
   `cargo run -p vernon-cli -- cook <that file>`.
2. Run the reference tool as below and save its output in the case folder.
3. Run `cargo test -p vernon-cli --test golden`.

## Cases

| Case | Tool | What is compared | Tolerance | Result when recorded |
|---|---|---|---|---|
| `blender/subd_catmull_clark` | Blender 5.2.1 LTS | `surface.subd_refine`, 2 levels: refined vertices, and vertices on the limit surface; face counts | 1e-4 mm per vertex (Blender stores 32-bit floats) | worst 8.8e-6 mm refined, 1.3e-5 mm limit |
| `cloudcompare/c2m_c2c` | CloudCompare 2.13.2 | `clean.mesh_distance` signed cloud-to-mesh, and `clean.cloud_distance` nearest-neighbour cloud-to-cloud, per point (5,551 points) | 1e-4 mm per point (32-bit export) | worst 5.8e-6 mm C2M, 2.0e-5 mm C2C |
| `orcaslicer/box_3mf` | OrcaSlicer 2.4.2 | layer count and filament grams of the slice | 2% | **layer/mass comparison not recorded**; later CLI smoke succeeded (see below); reference mass unknown |

### Blender

```
blender -b --factory-startup --python scripts/golden/blender_subd.py -- <case>/input.obj <case> 2
```

The OBJ is imported without axis conversion. Creases are off and boundary
corners preserved; the cage is closed, so boundary rules do not take part.

### CloudCompare

From the case folder:

```
CloudCompare -SILENT -AUTO_SAVE OFF -C_EXPORT_FMT ASC -PREC 9 -ADD_HEADER -O compared.xyz -O reference.obj -C2M_DIST -SAVE_CLOUDS FILE c2m.asc
CloudCompare -SILENT -AUTO_SAVE OFF -C_EXPORT_FMT ASC -PREC 9 -ADD_HEADER -O compared.xyz -O reference.xyz -C2C_DIST -SAVE_CLOUDS FILE "c2c.asc reference_out.asc"
```

Delete `reference_out.asc` afterwards; only `c2c.asc` is used.

#### Offline statistics coverage

`signed_cloud_to_mesh_statistics_match_cloudcompare` uses the existing `c2m.asc`
scalar export and the real `clean.mesh_distance` node. It keeps the **1e-4 mm**
per-point comparison and **1e-3 mm** row-position check, and requires nonempty,
finite arrays with one distance per input point.

- **Array coverage:** independently reduce each signed distance array to mean
  and sample standard deviation (**N-1**), following
  `docs/specs/alignment-deviation.md` section 3.5. Compare both at **1e-3 mm**
  absolute error, the section 5 case 15 bound. A population-standard-deviation
  (**N**) comparison at the same bound is supplementary, not the spec estimator.
- **Report coverage:** read actual output port 1 metrics `mean_dev` and `std_dev`,
  require finite values in mm, and compare to independent reductions of the
  reference scalar export at **1e-3 mm**. `distance.rs::deviation_report` calls
  `stats::moments(..., None)`, which uses sample std (N-1). Require
  `valid_points == input count` and `invalid_points == 0`, in points, so a filtered
  report cannot pass as complete-fixture coverage.
- **Reference limit:** these are statistics reconstructed from recorded reference
  scalars, not a recording of the reference tool's displayed statistics. Its UI
  estimator convention is not established here. ICP final RMS within **5%**
  (section 5 case 15) remains unrecorded by this fixture.

Three analytic helper tests distinguish signed from absolute means, N from N-1,
constant/singleton behavior, and rejection of empty/nonfinite/overflowing data.
The four existing geometry/distance tests retain their tolerances. The added
statistics test and helper tests were staged on 2026-11-18 and subsequently
passed in run `wp15-wp16-20261119-b` on 2026-11-19 (all eight golden tests passed). The table's recorded errors remain historical, not new measurements.

### OrcaSlicer (open)

`part.3mf` is written by `print.package_write`. OrcaSlicer 2.4.2 slices it from the
command line without any export from its interface, provided the printer, process and
filament files each name an installed preset in `inherits`. A hand-written set with an
empty `inherits` stops with "process not compatible with printer" (exit 127): the slicer
decides compatibility from the preset each file derives from. The bench case
`B.slicer.flavour_check` builds a working set at run time from the generic presets the
install ships (`resources/profiles/Custom`: a generic Klipper printer and its standard
process; `resources/profiles/OrcaFilamentLibrary`: a generic PLA), flattens each
`inherits` chain into one full file, then sets
`inherits` to the installed preset, `compatible_printers` of the process to the printer,
and the three `*_settings_id` names. The files go to the bench scratch folder only; nothing
of the slicer's profiles is copied into this repository. Command:

```
orca-slicer --slice 0 --outputdir <dir> --load-settings "machine.json;process.json" --load-filaments filament.json part.3mf
```

Result on this machine (2026-11-15): 100 layers, G-code with the slicer's own config block
(621 keys). The case checks that G-code against `print.flavour_check` and that every key of
`profile_schema::SCHEMA` is in that config block.

The `orcaslicer/box_3mf` row in the table above stays "not recorded": layer count and
filament grams have not been compared to Vernon's own estimate yet.

#### Independent layer/mass golden contract — blocked

This case remains **blocked pending an independent measurement**, not PASS.
Reference grams are unknown. The CLI smoke and config-key check do not satisfy
the two **2%** comparison gates. No slicer comparison or production parser change
is included in the statistics patch.

Before implementing the case, retain the owner's raw G-code, input 3MF and hashes,
tool version/build, recording date/operator, exact owner-exported settings,
filament diameter (mm), density (g/cm3), orientation and layer heights, and
independently recorded tool-displayed layer count and grams for that same job.
Define whether the recorded total includes supports, skirt/brim, purge/prime and
start/end extrusion. Begin with one material/extruder, non-volumetric extrusion,
fixed layer height, and no unaccounted macros, flow scaling or firmware retraction;
otherwise document and implement those semantics before using the comparison.

`crates/vernon-nodes-print/src/gcode.rs` prefers `comment_g` over computed mass
and can fall back to `comment_mm` when net extrusion is nonpositive. Reading either
reference comment as the estimate is circular. A future comparison must preserve
the original file, exclude both mass and length summary comments from the estimate
in memory, and derive consumed filament length from extrusion commands with M82,
M83 and G92 semantics. Retraction/unretraction must cancel rather than counting
every positive move twice; terminal retraction and purge inclusion must match the
reference's accounting. Require finite positive independently derived length;
unsupported commands or unknown accounting leave the case blocked, not silently
zero or a fallback reference value. Compute grams as
`length_mm * (pi / 4) * diameter_mm^2 * density_g_cm3 / 1000` using recorded settings.
Use an independently recorded slicer total as the expected value, never a guessed
mass, geometry volume, or a value calibrated from the same test result.

For layers, distinguish layer markers from independent geometry/height estimation;
Z hops and extrusion before the first print layer are not extra printed layers.
Record first-layer height and adaptive-layer settings explicitly. For positive
reference layers and grams, independently require
`abs(actual - reference) <= 0.02 * abs(reference)` for each quantity. Do not widen
either bound or average the two errors. Report extraction agreement separately
from independently estimated consumption; neither validates physically printed
mass. Capture provenance and resolve any unsupported accounting before adding the
offline golden test.

If a machine has no OrcaSlicer install with a `resources/profiles` folder (a portable or
Linux build keeps it elsewhere), the case is skipped with the reason. The owner then
exports a set once: in the slicer choose a printer, a process and a filament, open each
preset's menu, "Export preset" (or File > Export > Export preset bundle), save the three
JSON files into one folder named `machine.json`, `process.json` and `filament.json`, and
set the environment variable `VERNON_SLICER_PROFILES` to that folder before running the
bench. The case then uses those files instead of building a set.

## Not compared

Features without a reference tool on this machine are not claimed as
validated. Tools present but not yet scripted: Houdini, 3ds Max, Plasticity,
Marvelous Designer.

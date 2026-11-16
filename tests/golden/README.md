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
| `orcaslicer/box_3mf` | OrcaSlicer 2.4.2 | layer count and filament grams of the slice | 2% | **not recorded**: the command-line slice exits with a generic error (see below) |

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

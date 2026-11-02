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

`part.3mf` is written by `print.package_write`. Slicing it with
`orca-slicer --slice 0 --outputdir <dir> part.3mf` stops with "found error,
exit" and code 127, with and without `--load-settings` / `--load-filaments`
files, and no log explains why. The next step is a complete printer, process
and filament profile set exported from the slicer's own interface, then the
same command with those files.

## Not compared

Features without a reference tool on this machine are not claimed as
validated. Tools present but not yet scripted: Houdini, 3ds Max, Plasticity,
Marvelous Designer.

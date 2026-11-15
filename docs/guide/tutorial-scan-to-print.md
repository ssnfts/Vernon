# Tutorial: from a scan to a print package

**Scene:** `examples/bodykit_pipeline.vrn`, the first part of the chain only.
**Time:** about 30 minutes, most of it reading.
**You will learn:** how a scan is aligned to the design, how the deviation is
checked against a limit, how a surface is fitted and thickened, and how the
part is cut to fit a print bed and packed into a 3MF file.

Read [the user guide](README.md) sections 5 and 6 first if the words "node",
"report" and "gate" are new to you.

## What the example does, in plain words

The scene starts with a car panel: a curved sheet 900 mm long and 600 mm wide,
built by the `geo.car_panel` node. The "scan" is a copy of that panel that has
been given a little noise and moved away from its place. Those two steps are
fake on purpose, so you know the right answer. The steps after them are the
real ones:

1. `icp_align` moves the scan back onto the panel (ICP is an iterative
   best-fit).
2. `scan_deviation` measures how far each scan point is from the panel.
3. `scan_gate` checks that the 95th percentile of that distance is under
   0.2 mm.
4. `fit_patch` fits a smooth surface to the aligned scan, and
   `surface_deviation` checks that surface against the scan.
5. `thicken` gives the surface a 3 mm wall.
6. `grid_split` cuts the wall into pieces, `bed_fit` checks each piece against
   a 300 mm printer bed, and `package_3mf` writes one 3MF file.

The example uses synthetic data only. Its numbers show the arithmetic of each
step on a known shape. They do not show how accurate a real scanner is. See
[what is not validated](../VALIDATION.md#what-is-not-validated) before you
quote a number from a real scan.

## What you need

- Vernon installed (guide section 2), or a development checkout of this
  repository with a Rust build.
- The example scene. It is in the package folder `examples\`, or in the
  repository at `examples/bodykit_pipeline.vrn`.

Commands below use the repository form. With the installed package, replace
`cargo run -p vernon-cli --` with `bin\vernon-cli.exe`.

## Step 1: open the scene and cook it

**In the app:** start Vernon, click **Open the demo (body-kit pipeline)**. The
scene runs by itself.

**In a terminal:**

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn
```

You should see the last line:

```text
40 nodes cooked, 0 failed, 36 reports
gates: 0 failing, 5 expected
```

The five expected gates are for steps this tutorial does not use (no wheels,
a placeholder mesher, and the airflow solver). The tutorial does not depend on
them. Exit code 0 means the whole scene cooked.

The outputs go to `examples\bodykit_pipeline.vrn.work\`, one folder per node.
The command prints `--work DIR` if you want another folder.

## Step 2: check the alignment

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node icp_align
```

You should see:

```text
icp_align (clean.icp_align)
  report: ICP Align [PASS]
    rms_all_before = 2.1438 mm
    rms_all_after = 0.0509 mm
    iterations = 7
    pairs_used = 925 pairs
    converged = 1   (limit >= 1: ok)
```

- `rms_all_before` is the average distance between the scan and the panel
  before alignment. It is 2.1438 mm, because the scan was moved on purpose.
- `rms_all_after` is the same after alignment: 0.0509 mm.
- `converged = 1` means the iteration stopped on its own, not at the
  iteration cap.

In the app, the same node is `icp_align` in the Scene list. Click it and its
**Modify** tab shows the method (`point_to_plane`) and the maximum of 60
iterations. You do not need to change them.

## Step 3: read the deviation gate

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node scan_gate
```

You should see:

```text
scan_gate (clean.tolerance_gate)
  report: Tolerance Gate [PASS]
    tolerance = 0.2 mm
    target_share = 95 %
    p_target_abs = 0.0994 mm  (limit <= 0.2: ok)
    share_within_at_least = 100 %
    pass = 1   (limit >= 1: ok)
    max_abs_dev = 0.1716 mm
```

Reading it:

- `p_target_abs` is the 95th percentile of the distance from each scan point
  to the panel, 0.0994 mm. Ninety-five out of every hundred points are at most
  that far away.
- `max_abs_dev` is the single worst point, 0.1716 mm. The gate does not use
  this number, but it tells you the worst case.
- `pass = 1` is the gate result.

**Make the gate fail, on purpose.** This shows what a failing gate looks like.
Do not edit the file in `examples\`. Copy it first:

1. Copy `examples\bodykit_pipeline.vrn` to a new folder, such as
   `Documents\vernon-practice\`.
2. Open the copy in a text editor. Find the block whose `"name"` is
   `"scan_gate"`. Change its `"tolerance"` from `0.2` to `0.05`. Save.
   (In the app you can do the same thing: select `scan_gate`, change
   **tolerance** in **Modify**. Save the scene under another name first.)
3. Cook the copy's gate:

```powershell
cargo run -p vernon-cli -- cook "Documents\vernon-practice\bodykit_pipeline.vrn" --node scan_gate
```

You should see:

```text
  report: Tolerance Gate [FAIL]
    p_target_abs = 0.0994 mm  (limit <= 0.05: FAIL)
    share_within_at_least = 66 %
    pass = 0   (limit >= 1: FAIL)
gates: 2 failing, 0 expected
```

The exit code is 3. The panel's real scatter did not change. Only the limit
did. A tighter limit than the data supports makes the gate fail, and the
report shows by how much (margin -0.0494 mm). Put the tolerance back to 0.2
before you go on, or use the original file.

## Step 4: check the surface

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node fit_patch
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node surface_deviation
```

`fit_patch` should report `[PASS]` with:

```text
fit_max_mm = 0.1493 mm  (limit <= 0.2: ok)
fit_p95_mm = 0.0811 mm  (limit <= 0.2: ok)
control_points_u = 24, control_points_v = 19
```

`surface_deviation` should report `[PASS]` with:

```text
dev_p95_mm = 0.1655 mm  (limit <= 0.5: ok)
dev_max_mm = 0.4507 mm
pct_within_tol = 100 %
```

The first is how far the fitted surface is from the aligned scan. The second is
how far the scan is from the fitted surface. The limit for the second is 0.5 mm,
looser than the scan gate, because a fit is smoother than a scan.

## Step 5: thicken, split and check the print bed

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node thicken
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node grid_split
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node bed_fit
```

`thicken` should report a 3 mm wall, `thickness_mm = 3 mm`, with a volume of
`1.6488e6 mm³`.

`grid_split` should report:

```text
part_count = 12
plane_count = 5
seam_area_mm2 = 10939.5428 mm²
fit_margin_mm = 70.2288 mm  (limit >= 0: ok)
largest part 225.8 x 202.6 x 44.7 mm
```

`bed_fit` should report:

```text
part_count = 12
fit_margin_mm = 65.8907 mm  (limit >= 0: ok)
fail_count = 0   (limit <= 0: ok)
usable volume 296 x 296 x 296 mm
```

`fit_margin_mm` is the smallest of the per-part margins (`part.0.fit_margin_mm`
to `part.11.fit_margin_mm` in the full report). A negative value would mean a
part does not fit. Here the smallest margin is 65.9 mm, and no part fails. The bed used is `bed_300` with the
ASA material preset.

`seam_area_mm2` is the total area of the cut faces between the pieces.

## Step 6: write the 3MF package

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn --node package_3mf
```

You should see:

```text
package_3mf (print.package_write)
  file: .../n14_package_3mf/parts.3mf
  report: Package writer [PASS]
    part_count = 12 count
    triangle_count = 29238 count
    volume_mm3 = 1.6488e6 mm³
    open_edges = 0 count  (limit <= 0: ok)
    non_manifold_edges = 0 count  (limit <= 0: ok)
    file_bytes = 591476 bytes
    roundtrip_volume_error_rel = 7.0608e-15   (limit <= 0.000001: ok)
```

The file is `parts.3mf` in the node's folder, `n14_package_3mf`, inside the
`.vrn.work` folder. (The console line shows the path with a stray `\n` in it.
The file is in the folder named `n14_package_3mf`, as the listing shows.)

The checks that matter:

- `open_edges = 0` and `non_manifold_edges = 0`: the mesh has no holes and no
  edge shared by more than two triangles.
- `roundtrip_volume_error_rel`: the file was read back and its volume compared
  with the original. The difference is at the level of floating-point rounding.

The volume of the package, `1.6488e6 mm³`, matches the thickened wall. The cut
does not add or remove material.

## Step 7: the whole chain, once

```powershell
cargo run -p vernon-cli -- cook examples/bodykit_pipeline.vrn
```

The gate summary should match what you saw in step 1. Every gate in this
tutorial passes, and the other expected gates are not from this chain.

## What this tutorial does not show

- **Real scans.** The scan here is synthetic: the panel, plus noise of 0.05 mm
  and a known move. Scans from a real scanner come with their own errors. The
  scan-quality node reports them (noise, point spacing, boundary fraction), but
  this example does not test that.
- **Printing.** The 3MF file is checked for geometry, not for the printer. The
  slicer step needs a slicer program and its profile set. The example does not
  run one. See the `print.slice_external` node in `docs/NODES.md` and
  [guide section 9](README.md#9-programs-vernon-runs-and-the-trust-prompt).
- **Joints.** The example makes the cuts, but it does not add the dovetail,
  dowel or snap joints. Those are separate nodes you can add to the graph.
- **Material strength.** The bed fit uses the size of the parts. It does not
  show whether a printed part is strong enough. For that, see the
  [mount strength tutorial](tutorial-mount-strength.md), with its own limits.

## Check yourself

- Why is the scan gate limit (0.2 mm) on a percentile, not on the worst point?
- Why does the tighter tolerance fail when the data did not change?
- Which number tells you the parts fit the bed, and what would a failing value
  look like?

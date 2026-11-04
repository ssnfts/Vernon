# Golden slots for licensed commercial tools (Tier C)

Vernon does not have, and must not borrow, the commercial tools it re-implements. If **you own a
licence**, `CLEAN_ROOM.md` (allowed source 3) lets you run your own copy on inputs you made
yourself and record the numbers it gives. Put them here and `bench/run.ps1` compares Vernon's
output with them. With no `golden.json` a slot reports `SKIPPED(no golden)`.

Never copy anything out of a vendor install folder, never open or decompile its binaries, and
never commit vendor sample files, help text or screenshots. Record **numbers you read from the
tool's own user interface or its own exports of your inputs**.

## Slots

One folder per tool: `bench/golden/<slug>/`. Slugs the harness looks for: `rhino`, `fusion`,
`quicksurface`, `geomagic-designx`, `zeiss-inspect`, `airshaper`, `simscale`, `ansys`,
`optimumlap`, `ladybug-tools` (list in `crates/vernon-bench/src/tier_c.rs`).

## How to record

1. Make an input with Vernon (a node graph, or any mesh you authored) and save it as OBJ/STL/PLY
   into the slot folder, for example `bench/golden/rhino/input_sphere.obj`.
2. Open it in your licensed copy and run the operation named in the case. Write down the numbers
   it reports (deviation statistics, triangle count, area, lap time, drag area...), with the
   tool's version.
3. Write `golden.json` (below). Choose each tolerance from how the number is displayed (rounding
   in the UI) and from known algorithm differences. A tolerance of zero is refused.
4. Run `bench\run.ps1 -NoPerf`. A mismatch is a FAIL with both numbers in `REPORT.md`; do not
   widen a tolerance just to make it pass without writing the reason in `notes`.

## `golden.json` schema

```json
{
  "tool": "Rhino",
  "tool_version": "8.x (your exact build)",
  "recorded_on": "2026-11-04",
  "recorded_by": "your name",
  "cases": [
    {
      "name": "quad remesh deviation",
      "node": "surface.quad_remesh",
      "inputs": ["input_sphere.obj"],
      "params": { "target_quads": 2000 },
      "expected": [
        { "metric": "dev_p95_mm", "value": 0.31, "unit": "mm", "tol_abs": 0.1 },
        { "metric": "quads", "value": 2000, "tol_rel": 0.05 }
      ],
      "notes": "how the number was read in the tool, any setting that differs from Vernon's"
    }
  ]
}
```

- `node`: a Vernon node id (`vernon-cli list`). `inputs` are files in the slot folder, in port order.
- `params`: node parameter overrides (booleans, integers, floats or strings).
- `expected[].metric`: a metric name from that node's report (`vernon-cli info <node>`, or run once
  and read `REPORT.md`). `value` is the number your tool gave. `tol_abs` and `tol_rel` combine as
  `|vernon - value| <= tol_abs + tol_rel * |value|`.

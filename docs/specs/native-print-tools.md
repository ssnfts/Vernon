# Spec: native split and print tools (WP5)

Implementation contract checked against the local source on 2026-10-28.
This records the current WP5 behavior, parameters, measured fixtures and limits
for the nodes below. It supplements [slicing-printing.md](slicing-printing.md)
and replaces that document's older white-box descriptions for these implemented
nodes. `print.script_bridge` remains white-box. This document makes no claim
about final workspace test totals or independent slicer/printer validation.

All geometry lengths are millimetres, Z is up, and output indices are zero
based. Native algorithms use independently written Rust. External slicers
remain separate processes under [the clean-room policy](../../CLEAN_ROOM.md).
No proprietary installation or GPL algorithm implementation was consulted for
these native methods.

## 1. Ports and implementation map

| Node | Inputs, in order | Outputs, in order | Implementation |
|---|---|---|---|
| `print.plane_cut` | mesh Geometry; optional planes JSON | parts Geometry; plane_list JSON; Report | [nodes_cut.rs](../../crates/vernon-nodes-print/src/nodes_cut.rs), existing shared cutter |
| `print.polyline_cut` | mesh Geometry; guide Geometry | parts Geometry; plane_list JSON; Report | [nodes_cut.rs](../../crates/vernon-nodes-print/src/nodes_cut.rs), [curtain.rs](../../crates/vernon-nodes-print/src/curtain.rs) |
| `print.cut_region_select` | cut Geometry | Geometry; Report | [nodes_cut.rs](../../crates/vernon-nodes-print/src/nodes_cut.rs) |
| `print.smart_partition` | mesh Geometry; optional guide Geometry | parts Geometry; Report | [nodes_split.rs](../../crates/vernon-nodes-print/src/nodes_split.rs), [partition.rs](../../crates/vernon-nodes-print/src/partition.rs) |
| `print.snap_joint` | parts Geometry | parts Geometry; Report | [nodes_joints.rs](../../crates/vernon-nodes-print/src/nodes_joints.rs), [snap.rs](../../crates/vernon-nodes-print/src/snap.rs), shared joint engine |
| `print.flavour_check` | G-code File | violations JSON; Report | [nodes_pack.rs](../../crates/vernon-nodes-print/src/nodes_pack.rs), [gcode_check.rs](../../crates/vernon-nodes-print/src/gcode_check.rs), object-marker checker in [gcode.rs](../../crates/vernon-nodes-print/src/gcode.rs) |
| `print.profile_build` | none | profiles JSON; Report | [nodes_pack.rs](../../crates/vernon-nodes-print/src/nodes_pack.rs), [profiles.rs](../../crates/vernon-nodes-print/src/profiles.rs), preset maps in [slicer.rs](../../crates/vernon-nodes-print/src/slicer.rs) |
| `print.script_bridge` | optional graph JSON, currently unused | script File; Report | white-box writer in [nodes_pack.rs](../../crates/vernon-nodes-print/src/nodes_pack.rs) |

The shared print-node metadata still points to `docs/specs/slicing-printing.md`;
the implementation details and WP5 measurements are recorded here.

## 2. Projected body-line cuts and component selection

### Cutting contract

Body Line Cut uses an ordered, open, piecewise-linear guide extruded along
`view_dir`. With normalized view direction `d`, the first-to-last endpoint
vector `delta` projected perpendicular to `d` defines `u`; `v = d cross u`.
The implementation forms the projection with two cross products, avoiding a
parallel-subtraction residual becoming a false in-plane axis. Projected
endpoint distance must exceed
`max(1e-10 mm, 64*f64::EPSILON*max(length(delta),1 mm))`. The constructed
directions must be finite, unit length and mutually orthogonal within `1e-10`
checks. Parallel or numerically unresolved endpoint projections error before
plane construction. Separately, every guide vertex must advance strictly in
projected `u` by more than `1e-10` mm. The guide therefore defines one height
`v = f(u)`. Its first and last segments extend beyond the endpoints across the
body's extent. Guide depth along the view direction has no effect on the
projected curtain.

The guide may be one open primitive, using its vertex order, or points-only
geometry, using point order. Closed primitives, multiple open primitives,
coincident projected endpoints, folds, projected duplicates, nonfinite values
and a zero view direction fail explicitly. There must be 2 to 128 guide points.
This is a monotone curtain cutter, not an arbitrary self-overlapping curve
Boolean. The old best-fit plane calculation supplies `fit_rms_mm` and
`fit_max_mm` diagnostics only; it does not substitute a plane for a bent guide.

The native method refines triangles at internal `u` knots, welds matching strip
boundaries, shears by `-v*f(u)`, reuses shared-edge plane clipping/capping,
refines new caps at those knots, then applies the inverse shear. Each strip is
affine with determinant one. Refining the caps before inversion prevents a
triangle from spanning a bend incorrectly. The two finished sides are kept
separate, and connected output components receive dense primitive `part` ids.
Each guide segment receives its own planar `cap` region; `plane_list` entries
contain matching `cap`, `origin` and normalized `normal` fields. Both cut
adapters reserve fresh cap ids above the maximum in the **complete input**
before selection, preserving seams on untouched components and preventing new
cuts from colliding with those ids. Exhausting the integer range rejects.

Source float/vector attributes interpolate at generated vertices; `N` vectors
normalize, and integer/string attributes select the nearer endpoint. New cap
interior knot points interpolate along constant-`u` view-direction sections.
This is tested with the analytic linear field `x + 2*y + 3*z`. Arbitrary field
accuracy on newly created cap interiors is not independently certified.

| Parameter | Default | Declared range or choices | Meaning |
|---|---|---|---|
| `view_dir` | `(0,1,0)` | finite nonzero vector | Extrusion/projection direction for Body Line Cut |
| `keep` | `both` | `both`, `upper`, `lower` | Retained side in the curtain's sheared plane frame |
| `cap` | `true` | boolean | Close new cuts; false leaves open cut surfaces |
| `eps` | `1e-6` | `1e-9` to `1e-3`, times bounding diagonal | Body Line Cut fixes the physical snap distance from the selected input before shearing |

The curtain cutter rejects selected-body or guide coordinates above `1e12` mm
in magnitude and input or refinement counts above one million
primitives/triangles. With `D`
the selected input's pre-shear bounding diagonal (floored at `1e-9` mm), it
fixes absolute clip tolerance `max(eps*D, 1e-12 mm)`, refinement tolerance
`max(D*1e-13, 1e-12 mm)`, weld tolerance `max(D*1e-12, 1e-10 mm)`, and
cap-boundary sampling tolerance `max(D*1e-11, 1e-9 mm)`. Shearing never enlarges
these tolerances from an inflated transformed bounding box. Even while points
are sheared, welding compares their inverse-shear world positions.

A conditioning guard estimates world-coordinate roundoff using
`64*f64::EPSILON*(1+maximum_absolute_guide_slope)*max(2*coordinate_scale+height,1)`.
Coordinate scale includes the body and guide origin; height includes evaluated
guide height at body points and guide knots. A nonfinite estimate or one above
the physical clip tolerance errors for a nonempty selection, recommending a
less steep guide or separate cuts. There is no fixed slope threshold
independent of model scale. An empty selection performs no shear or clipping,
so it skips this conditioning rejection while still validating the guide,
snap tolerance and complete-input cap-id capacity.
The cutter also rejects nonfinite output and, for capped closed manifold
inputs, any loss of closed directed-edge incidence. With both sides retained,
it rejects nonfinite or greater-than-0.005 relative per-part volume difference
using recentered volume sums. These checks do not certify self-intersection
freedom or perform arbitrary mesh repair. Closed, consistently wound shells
are the acceptance input; capping cannot repair unrelated source holes.

Planar Cut retains `origin=(0,0,0)` mm and `normal=(0,0,1)` plus the common
`keep`, `cap` and `eps` parameters. Its optional JSON accepts an array of
`{origin:[x,y,z], normal:[x,y,z]}` entries, also accepting `point` for `origin`
and a containing `{planes:[...]}` object. Invalid entries warn; no usable
plane is an error. At most 62 planes are supported in one Planar Cut cook.
It shares the selection behavior below.

### Selection contract

Cut Region Select emits the primitive group `cut_region`. Its rules select
faces, but both cut nodes use those faces as eligibility seeds for **whole
edge-connected components** within the input parts. Any touched component is
cut in full. Components with no selected face pass through intact. A missing
group permits all components; an existing empty group permits none. This does
not implement partial cutting inside one connected solid.

| Parameter | Default | Meaning |
|---|---|---|
| `selector` | `all` | `all`, `bbox`, `normal_cone`, or `group` |
| `bounds_min`, `bounds_max` | `(-1e9,-1e9,-1e9)`, `(1e9,1e9,1e9)` mm | Inclusive face-centroid box; finite ordered bounds required |
| `normal_axis` | `(0,0,1)` | Cone axis; finite and nonzero |
| `normal_angle` | `45` degrees, range `0..180` | Cone half-angle tested against area-normal direction |
| `source_group` | empty string | Existing primitive group for `group`; missing group is an error |
| `island` | `-1`, range `-1..1000` | Optional edge-connected component index; `-1` means all |
| `mode` | `cut` | `cut` selects matching faces; `join` excludes every component touched by a matching face and selects the remaining components |

`join` does not invert individual face bits inside a touched component.
The primitive integer attribute `joined` is a selection diagnostic. Reports
contain `island_count` and `selected_prims`; downstream cutting expands the
selected faces to component eligibility.

### Metrics and measured acceptance

Both cutters report `part_count`, `plane_count`, `cap_area_mm2`,
`seam_area_mm2` and `cap_loops`. Body Line Cut adds `guide_segments`,
`fit_rms_mm`, `fit_max_mm` and `open_edges`; its open-edge gate is zero when
capping is on. With both sides retained and a closed input, its
`volume_error_rel` gate is at most `0.005`. The existing Planar Cut reports
`open_edges` when capping is on, and its closed-input, capped, both-side
volume gate remains `1e-9`. Seam/cap area counts a shared cut once when both
sides are retained.

The tracked [cut regression tests](../../crates/vernon-nodes-print/src/nodes_cut.rs)
`wp5_bent_guide_cuts_the_actual_curtain`,
`wp5_curtain_refinement_preserves_linear_attributes_and_volume`,
`wp5_diagonal_car_panel_shell_is_closed_and_preserves_volume`, and
`wp5_curtain_caps_are_planar_regions_with_matching_metadata` cover these
fixtures and their recorded measurements:

| Fixture | Observed result |
|---|---|
| 100 mm cube, bent guide `(-60,-15,0), (0,20,0), (60,-15,0)`, view Z | Two closed manifold parts; cap points follow `y = 20 - 35*abs(x)/60`; volumes `445833.3333333333` and `554166.6666666667` mm3 |
| Actual default `geo.car_panel`, thickness 3 mm, diagonal bent guide, view Z | Two closed, consistently wound manifold parts; input volume `1636075.523190558` mm3, output `1636075.523190528` mm3; relative difference about `1.8e-14` |
| Two connected guide segments | Four planar cap regions across the two sides, each with matching plane metadata |

Regression coverage also includes selectors, empty selection, whole-component
join exclusion, invalid/folded/closed guides, and linear attribute preservation.
`wp5_steep_curtain_never_returns_damaged_closed_solids` exercises conditioning
with extremely narrow, steep guides; it requires a valid closed result within
the volume tolerance or an explicit conditioning error.
`wp5_selected_cuts_preserve_untouched_seam_ids_and_joint_pairs` checks both cut
nodes with an existing untouched seam and confirms the new metadata and two
subsequent snap-joint pairs retain distinct cap ids. Chained planar cuts and
complete-input cap-id overflow also have dedicated regressions.

## 3. Bounded smart partition search

The method follows the recursive BSP beam-search framework of Luo et al.'s
[Chopper paper and author project page](https://pixl.cs.princeton.edu/pubs/Luo_2012_CPM/index.php).
Vernon uses its own finite candidate family, fit routine and cost proxies;
neither exact Chopper parity nor a globally minimum part count is claimed.

For each state, split the unfinished part with the largest estimated subvolume
count. Candidate normals comprise world axes, six diagonal directions and
distinct original face normals, capped at 16 directions. Candidate offsets
include the midpoint, usable-volume-derived positions and up to the first 16
optional guide points projected onto each direction. Guide primitives are not
followed as continuous body lines. Stable insertion and cost ordering make
repeated cooks deterministic.

Each candidate uses actual capped geometry, verifies closed/manifold directed
edge incidence and positive recentered volume, and calls orientation-aware
bed fitting with six build axes and XY spin search. Scaling is disabled.
Complete fitting states remain eligible; if the evaluation budget expires,
the best complete plan found can be returned with a warning. No complete
fitting plan produces an explicit error. Output keeps assembly coordinates
and dense primitive `part` ids; use the existing bed-fit node to place parts
on a print bed.

| Parameter | Default | Declared range or choices |
|---|---|---|
| `printer` | `bed_300` | `bed_300`, `bed_350`, `bed_350x320x325`, `bed_800x800x1000`, `custom` |
| `bed` | `(256,256,256)` mm | Custom build volume when `printer=custom` |
| `margin` | `2` mm | `0..20`, removed from every side |
| `beam_width` | `8` | `1..64` |
| `w_parts` | `1` | `0..100` |
| `w_seam` | `0.001` | `0..100` |
| `w_visible` | `0.01` | `0..100` |
| `w_fragile` | `1` | `0..100` |
| `w_unused` | `1e-6` | `0..100` |

The five weighted costs are respectively estimated part count during search
(actual count at a goal), accumulated new seam area, weighted seam perimeter,
small-interface count, and unused usable-bed volume. Visibility uses seam
midpoint distance to guide samples and adjacent source-face dihedral angles.
It is a crease/distance proxy, not surface visibility or curvature fitting.
The connector proxy counts interfaces with hydraulic radius
`2 * cap_area / seam_perimeter < 2 mm`; it does not prove connector placement
or mechanical feasibility. Each reported cost already includes its weight.

Hard bounds are 64 output parts, 4096 attempted plane cuts, 20,000 input
faces, 200,000 input points, input-face-count times beam width at most 160,000,
and 80,000 faces in a candidate state. Coordinates and bed dimensions are
bounded by `1e12` mm; runtime weights must be finite and within `0..1e6`.
Margin must leave positive usable dimensions. Open/nonmanifold or inconsistent
winding, nonpositive volume, exhausted cap identifiers and excessive numeric
or search size fail. This is bounded sampling, so failure to find a plan does
not establish that no printable partition exists.

Reports contain `parts`/`part_count`, `plane_count`, `seam_area_mm2`,
`seam_length_mm`, `min_fit_margin_mm`/`fit_margin_mm` (gate at least zero),
`cost_parts`, `cost_seam`, `cost_visible`, `cost_fragile`, `cost_unused`,
`cost_total`, `search_evaluations`, `search_evaluation_limit` (4096),
`search_part_limit` (64), `search_limited` and `guide_samples`.
`search_limited` specifically records hitting the evaluation budget.

The 1200 x 600 x 300 mm solid-box acceptance fixture on a 350 mm cube bed
with 2 mm margin reports **8 parts, 10,200 mm seam perimeter, 46 mm minimum
fit margin and 557 evaluations**. Every part is independently fit-checked
without scaling, valid, closed and edge-manifold; the test checks relative
volume difference below 0.005. These are the recorded current fixture values,
not an optimality proof. The tracked regression
[`wp5_smart_partition_search_fits_large_box`](../../crates/vernon-nodes-print/src/nodes_split.rs)
performs these checks and prints the measurements. The
[partition helper tests](../../crates/vernon-nodes-print/src/partition.rs)
cover deterministic output, already-fitting input, guide influence, limits
and connector-proxy behavior.

## 4. Cantilever snap joints

Snap Joint builds a rectangular beam, return shoulder, catch and lead-in nose
on one side of a paired planar cap, with a mating recess including clearance
and deflection room. The beam width is `2*radius`; the legacy `radius` id now
means half-width. The return shoulder is 75 degrees. `arm_length` is the
root-to-catch distance; the nose extends farther by
`bulge / tan(ramp_angle)`. The shared joint engine analytically rebuilds cap
holes and lofts the section rings. This is not a general mesh Boolean.

| Parameter | Default | Declared range or choices |
|---|---|---|
| `radius` | `4` mm | `1.5..15`; beam half-width |
| `arm_length` | `8` mm | `3..30` |
| `thickness` | `1.2` mm | `0.4..5` |
| `bulge` | `0.4` mm | `0.1..2`; catch undercut and assembly deflection |
| `flex_room` | `0.6` mm | `0.1..3`; must be at least `bulge` |
| `ramp_angle` | `35` degrees | `5..80` |
| `material` | `asa` | `pla`, `petg`, `asa`, `pet_cf`, `paht_cf`, `ppa_cf` |
| `spacing` | `40` mm | `10..200` |
| `wall_min` | `1.5` mm | `0.5..10` |
| `tolerance` | `0.15` mm per side | `0.05..0.4` |
| `max_count` | `0` | `0..200`; zero fits as many as spacing permits |
| `male_side` | `lower` | Lower or upper part id carries the hook |
| `sides` | `64` | Retained common circle-facet parameter `8..256`; the hook section is rectangular |

Input parts must be valid, finite, closed and edge-manifold; each detected cap
region must be planar within `1e-6` mm. Split a bent seam into separate planar
cap regions first. The feature dimensions must be finite and positive, within
the supported feature bounds; the return shoulder must fit inside the arm.
Placement uses the shared sampled wall, overlap and depth checks. No cap pair
is an error; cap pairs with no fitted joint produce a failing `joint_count`
gate rather than a successful no-op.

The uniform rectangular cantilever screening formula is

```text
strain_pct = 100 * 1.5 * thickness * bulge / arm_length^2
```

It follows the uniform-beam relationship in Covestro's primary
[Snap-Fit Joints for Plastic guide](https://solutions.covestro.com/-/media/covestro/solution-center/brands/downloads/imported/1557218421.pdf),
Table 1. The guide's molded-grade, short-term values are not reused as
printed-material certification. The new shared
[material table](../../crates/vernon-core/src/materials.rs) labels every
initial FDM allowance **`Source::Assumed`**:

| Material id | Assumed allowable strain, percent |
|---|---:|
| `pla` | 1.0 |
| `petg` | 2.0 |
| `asa` | 2.0 |
| `pet_cf` | 0.6 |
| `paht_cf` | 1.0 |
| `ppa_cf` | 0.8 |

These are screening assumptions, not measured FDM snap limits. Every cook
warns that print orientation, temperature, conditioning and repeated assembly
need printed-coupon validation. The root is sharp; root stress concentration,
nonlinear contact, fatigue, cyclic life and print-process effects are not
solved. A passing strain gate is not mechanical certification.

Reports retain `joint_count`, `pair_count`, `skipped_count`, `open_edges`
(zero gate) and `volume_change_mm3`, adding `strain_pct` (at most
`allowable_pct`), `allowable_pct`, `undercut_mm`, `arm_thickness_mm`,
`arm_length_mm` and `lead_angle_deg`. `joint_count` gates at least one.
Unknown materials reject. Defaults give strain **1.125%**; the regression
with length 4 mm, thickness 2 mm and undercut 0.8 mm gives **15%** and fails.
Doubling length quarters predicted strain.

The tracked
[`wp5_snap_has_hook_geometry_and_an_analytic_strain_gate`](../../crates/vernon-nodes-print/src/nodes_joints.rs)
and [`hook_sections_have_a_real_catch_and_flex_clearance`](../../crates/vernon-nodes-print/src/snap.rs)
regressions check 1.2 mm root thickness, 1.6 mm catch thickness, the 35-degree
ramp and closed manifold parts. Other adapter tests reject insufficient flex
room, nonplanar caps and open hosts; the shared material-table test checks
source labels and coverage. These are geometric and analytic tests, not
physical coupon tests.

## 5. Static G-code conformance and physical screening

### Node parameters and output

| Parameter | Default | Declared range or choices |
|---|---|---|
| `flavour` | `klipper` | `marlin`, `marlin2`, `klipper`, `reprapfirmware`, `repetier` |
| `strict` | `false` | Promote warning diagnostics to errors |
| `bed` | `(300,300,300)` mm | Finite positive Cartesian axis limits from zero |
| `max_nozzle_c` | `320` degrees C | `0..600` |
| `max_bed_c` | `130` degrees C | `0..250` |
| `max_chamber_c` | `90` degrees C | `0..150` |
| `extrusion_mode` | `auto` | `auto`, `relative`, `absolute` |

The violations JSON is an array of `{line, message, severity}` records.
Physical/parser errors fail even with `strict=false`; unknown or unsupported
commands normally warn. `strict=true` promotes all warnings. The Report has
`violation_count`, `error_count` (zero gate), `warning_count` and
`object_define_count`, with at most the first 50 diagnostics copied as notes.
It does not execute the job or inspect runtime firmware configuration.

### Parsed and physically modeled subset

The bounded checker accepts UTF-8 text, case-insensitive compact words such
as `G1X10Y20`, optional `N` line numbers, semicolon/parenthesis comments and
checksum suffixes. It strips checksum text without verifying the checksum.
Integer numeric G/M/T command aliases normalize leading zeros (`G01` is
`G1`). Numeric words use fixed decimal syntax; malformed, nonfinite or duplicate
words are errors. `M117`, `M118` and underscore-named extended commands retain
text instead of parsing it as ordinary numeric words. Nonfinite arithmetic
after unit conversion in motion, G92 offsets or arc geometry also errors and
invalidates coordinate certainty.

The new checker's line reader rejects lines above 64 KiB. Its ordinary-command
path stops after more than 10,000 diagnostics, but malformed-word and invalid
UTF-8 continuation paths do not pass that guard. The separate object-marker
and definition-count passes use `reader.lines()` without the same line or
diagnostic bounds and retain object names in memory. There is no whole-job
memory or diagnostic-count guarantee.

| Commands or state | Implemented physical behavior |
|---|---|
| Initial state | Position and G92 offset zero, mm units, absolute XYZ, absolute E, known Cartesian state; this is an assumption about the job's starting state |
| `G20`, `G21` | Inch/mm conversion; a dialect may still warn for an unsupported command name |
| `G90`, `G91` | Absolute/relative XYZ; for `marlin`/`marlin2` also change E mode to absolute/relative |
| `M82`, `M83` | Set absolute/relative E mode |
| `G92` | Track XYZ work offsets against the current physical position; reject nonfinite converted offsets |
| Full `G28` with no parameters | Set position and offset to zero; does not restore certainty already lost to unsupported state changes |
| Partial `G28` | Warn and invalidate coordinate certainty |
| `G0`, `G1` | Update XYZ; reject nonfinite converted coordinates and error outside `[0,bed]` with `1e-6` mm tolerance; supplied feed must be positive |
| `G2`, `G3` | Update endpoint and check XY I/J centre-offset arcs, including cardinal extrema along the swept arc; matching start/end means a full circle; reject nonfinite converted arc geometry |
| Other arc forms | Missing I or J, radius-form arcs or other-plane arcs cannot receive a passing arc-bounds check |
| Extrusion moves | If requested, require the active E mode to match `extrusion_mode`; no extrusion-volume or flow/dynamics simulation |
| `M104`, `M109` | Every supplied S/R target must be within `0..max_nozzle_c` |
| `M140`, `M190` | Every supplied S/R target must be within `0..max_bed_c` |
| `M141`, `M191` | Every supplied S/R target must be within `0..max_chamber_c`, independently of dialect membership |
| RepRapFirmware `G10`, `M568` | Supplied S/R tool targets checked against the nozzle limit |
| `SET_HEATER_TEMPERATURE` | Parse `HEATER=extruder` or `heater_bed` and finite TARGET against the respective limit; custom heaters warn as outside this set |
| `T0` | Retain single-tool state; other `T0..T255` commands warn and invalidate coordinate certainty |

`G18`, `G19`, `G53` through `G59`, `SET_GCODE_OFFSET`,
`RESTORE_GCODE_STATE`, `M206`, and `G10` containing XYZ/L configuration
invalidate coordinate certainty. So do `M98`, `M99`, `M501`, `M502`, `G29`
through `G32`, `RESTART`, `FIRMWARE_RESTART`, `PAUSE` and `RESUME`. Unknown
commands also invalidate it unless they are numeric M codes. Thus an unknown
named macro such as `MOVE_HEAD` loses certainty despite its M prefix. Each
later modeled move then errors because its bounds cannot be verified; a later
`G28` does not silently repair that uncertainty. Unknown numeric M codes warn
but do not generically invalidate state.
Most allow-listed nonmotion commands have no simulated parameters or effects.
The check therefore remains a declared static subset rather than complete
machine-state, macro, heater or firmware conformance certification.

### Current command-name allowlist

This table transcribes `gcode_check.rs`, not a promise that every firmware
build enables every feature or that all listed commands are physically
simulated. The base set applies to all flavours except the explicit Klipper
removals; each flavour then adds its row. The public
[Klipper command reference](https://www.klipper3d.org/G-Codes.html) is the
existing primary source for its standard and extended command distinction.

| Set | Allowed command names |
|---|---|
| Base | `G0 G1 G2 G3 G4 G10 G11 G17 G20 G21 G28 G90 G91 G92 M0 M1 M18 M73 M82 M83 M84 M104 M105 M106 M107 M109 M110 M112 M114 M115 M117 M118 M140 M190 M201 M203 M204 M220 M221 M400` |
| Remove from base for `klipper` | `G17 G20 M0 M1 M110 M201 M203` |
| Add for `marlin` and `marlin2` | `G29 G30 M17 M42 M75 M76 M77 M92 M108 M141 M155 M191 M205 M206 M207 M208 M209 M211 M300 M301 M302 M303 M304 M355 M410 M420 M500 M501 M502 M503 M600 M900 M999` |
| Additional `marlin2` | `M486 M593` |
| Add for `klipper` | `BED_MESH_CALIBRATE BED_MESH_CLEAR BED_MESH_PROFILE SET_PRESSURE_ADVANCE SET_VELOCITY_LIMIT SET_GCODE_VARIABLE EXCLUDE_OBJECT_DEFINE EXCLUDE_OBJECT_START EXCLUDE_OBJECT_END EXCLUDE_OBJECT SET_PRINT_STATS_INFO TURN_OFF_HEATERS TEMPERATURE_WAIT SET_HEATER_TEMPERATURE SET_FAN_SPEED SET_GCODE_OFFSET SAVE_GCODE_STATE RESTORE_GCODE_STATE PAUSE RESUME CLEAR_PAUSE CANCEL_PRINT RESTART FIRMWARE_RESTART STATUS HELP GET_POSITION` |
| Add for `reprapfirmware` | `G29 G30 G31 G32 M42 M98 M99 M116 M141 M191 M201.1 M207 M208 M350 M400 M500 M501 M502 M503 M563 M564 M566 M568 M572 M573 M574 M577 M581 M584 M591 M593 M600 M669 M671 M906 M913 M950 M997 M999` |
| Add for `repetier` | `G29 G30 G31 G32 M42 M80 M81 M92 M111 M119 M205 M206 M207 M232 M300 M302 M303 M320 M321 M322 M500 M501 M502 M600 M908` |
| All flavours | Integer tool names `T0..T255`; only T0 stays inside the physical single-tool model |

Object-marker checks additionally require Klipper object names to be defined
once before starts, prohibit nested starts, match ends, and flag an unclosed
object. Klipper-specific commands in other dialects and `M900` in Klipper or
RepRapFirmware warn. The separate marker and definition-count passes strip
optional `N`/`n` integer prefixes, including `N1EXCLUDE_OBJECT_DEFINE`, and
trailing semicolon comments/checksum suffixes before tokenizing. The counter
matches the exact command name. Numbered/checksummed markers therefore receive
the same name/state checks as plain markers; checksum integrity remains a
transport responsibility. Regressions cover leading-zero motion,
named-macro/tool uncertainty, numbered markers, mode coupling, G92 offsets,
swept arcs, coordinate-frame changes, converted-number overflow, heater targets,
unknown commands and physical out-of-bed moves. These contracts are exercised
by the tracked tests in [gcode_check.rs](../../crates/vernon-nodes-print/src/gcode_check.rs),
[gcode.rs](../../crates/vernon-nodes-print/src/gcode.rs), and the
[Flavour Conformance adapter](../../crates/vernon-nodes-print/src/nodes_pack.rs).

## 6. Preset writer: JSON and INI subset

Profile Builder writes exactly three documents in its cook work directory:
`process.json`, `printer.json`, `filament.json`, or the corresponding `.ini`
names. The output JSON contains their paths as `process`, `printer`,
`filament`, `load_settings:[process,printer]`, `load_filaments:[filament]`,
plus `flavour`, `bed_mm` and `format`. There is no profile input or external
import step. All three documents are constructed and parsed before writes,
but the three writes are not a transactional multi-file replacement.

| Parameter | Default | Declared range or choices |
|---|---|---|
| `format` | `json` | `json`, `ini` |
| `flavour` | `klipper` | The five checked dialect ids |
| `printer`, `bed` | `bed_300`, `(256,256,256)` mm | Same preset ids as Smart Partition; custom uses `bed` |
| `material` | `asa` | The six shared material ids |
| `nozzle` | `0.4` mm | `0.2..1.2` |
| `layer_height` | `0.2` mm | `0.05..0.6`, also at most 80% of nozzle diameter |
| `walls` | `3` | `1..10` |
| `infill` | `15` percent | `0..100` |
| `relative_e` | `true` | Boolean |
| `firmware_retraction` | `false` | Boolean |
| `disable_remaining_time` | `false` | Boolean; JSON writes `disable_m73` |
| `pellet`, `pellet_coef` | `false`, `0.4157` | Coefficient `0.05..5`; equivalent diameter `sqrt(4/(coef*pi))` |
| `bed_temp_formula` | `by_highest_temp` | Also `by_first_filament` |
| `chamber_c` | `0` degrees C | `0..90`; zero disables control |
| `pressure_advance` | `0` | `0..2`; zero disables it |
| `flow_ratio` | `1` | `0.8..1.2` |
| `max_volumetric_speed` | `0` mm3/s | `0..60`; zero means no limit |
| `z_anti_alias` | `false` | True errors because no verified target preset key is implemented |

Printer sizes are 300 cubed, 350 cubed, 350 x 320 x 325, and
800 x 800 x 1000 mm. Their chamber maxima are respectively 0, 60, 65 and
0 degrees C; asking for more errors. `custom` does not establish a machine's
physical chamber capability. A material-preferring-warmer-chamber warning is
separate from the preset's maximum. The print temperature rows in
[common.rs](../../crates/vernon-nodes-print/src/common.rs) are PLA 215/60/0,
PETG 240/80/0, ASA 250/100/45, PET-CF 280/80/0, PAHT-CF 290/90/60 and PPA-CF
300/100/65 degrees C (nozzle/bed/preferred chamber). These presets are starting
values, not a calibrated print-process validation.

### JSON structure

Each document is an object with `type`, `name`, `from:"User"`, and empty
`inherits`. Printer type is `machine`; the other types are `process` and
`filament`. Numeric/boolean settings are serialized as strings; filament and
nozzle settings shown as lists below are single-element string arrays.

| Document | Emitted fields beyond common metadata |
|---|---|
| Process | `layer_height`, `initial_layer_print_height` (1.25 times layer height), `wall_loops`, `sparse_infill_density` (percent string), `top_shell_layers="5"`, `bottom_shell_layers="4"` |
| Printer | `gcode_flavor`, `printable_area` (four `"XxY"` strings), `printable_height`, `nozzle_diameter` (list), `use_relative_e_distances`, `use_firmware_retraction`, `disable_m73`, `bed_temperature_formula`, `build_plate_tilt_x="0"`, `build_plate_tilt_y="0"`, `pellet_modded_printer`, `pellet_flow_coefficient`, `support_chamber_temp_control` |
| Filament, all as lists | `filament_type`, `nozzle_temperature`, `hot_plate_temp`, `filament_density`, `filament_diameter`, `activate_chamber_temp_control`, `chamber_temperature`, `enable_pressure_advance`, `pressure_advance`, `filament_flow_ratio`, `filament_max_volumetric_speed` |

The validator reparses each JSON document, checks its role/name and selected
required types, dimensions, finite numbers and dialect ids, and compares it
for exact equality with the intended value. This is validation of Vernon's
emitted subset. It is not a complete external slicer schema validator.
The field basis is the existing public-source inventory in
[slicing-printing.md](slicing-printing.md#6-sources), including the
[printer settings reference](https://www.orcaslicer.com/wiki/printer_settings/basic%20information/printer_basic_information_advanced).
Some retained template spellings may differ across slicer versions. No
successful external import, inherited-profile resolution, or cross-version
compatibility is claimed.

### INI structure and explicit omissions

INI documents are flat, sorted `key = value` lines without sections. The
parser accepts blank/comment lines, requires nonempty lowercase/digit/underscore
keys and values, rejects duplicates, checks required role keys, and compares
the parsed map with the intended map.

| Document | Emitted keys |
|---|---|
| Process | `layer_height`, `first_layer_height` (same as layer height), `perimeters`, `fill_density` (percent string), `fill_pattern=grid` |
| Printer | `gcode_flavor`, `bed_shape` (four comma-separated `XxY` points), `max_print_height`, `nozzle_diameter`, `use_relative_e_distances`, `use_firmware_retraction` |
| Filament | `temperature`, `first_layer_temperature`, `bed_temperature`, `first_layer_bed_temperature`, `filament_diameter=1.75`, `filament_density`, `extrusion_multiplier`, `filament_max_volumetric_speed` |

The INI dialect spelling maps `marlin` to `reprap`; other dialect ids pass
through. Pellet mode, a positive chamber setpoint, or positive pressure advance
explicitly reject in INI mode. `disable_remaining_time` and `bed_temp_formula`
are not emitted in this subset. The default JSON external-slicer runner
rejects connected INI presets instead of passing them as JSON. INI is a
documented export subset for a separately compatible command-line workflow;
its import into PrusaSlicer or another external program has not been tested.

Reports contain `preset_count=3`, `nozzle_temp_c`, `bed_temp_c`,
`chamber_temp_c`, `material_chamber_c`, `round_trip_validated=1`, and
`equivalent_filament_diameter_mm` in pellet mode. The round-trip metric proves
the local serialization checks, not external acceptance. Regressions cover
both formats, parser/type rejection, unsupported extensions, reserved surface
modulation rejection and chamber/geometry limits.

## 7. Deferred scripting bridge

Script Bridge's help records the **2.5.0-alpha Python plugin API**, checked
against public documentation on **2026-10-28**. The release-source basis is the
existing [official release inventory](https://github.com/OrcaSlicer/OrcaSlicer/releases).
That date/version is a checked interface basis, not a claim that hooks work.
The node still lacks `.implemented()` and emits the required
`white-box placeholder:` warning.

Its optional graph JSON is ignored. `name` defaults to `vernon_hook`; the file
stem retains only ASCII alphanumeric characters/underscore, with that default
if empty. It writes an inert `.py` example under the cook work directory with
three symbolic hook names and a no-op event function. `hook_count=3` counts
example names, not registered hooks. Vernon does not execute or install it;
no slicing-plugin integration has been validated.

## 8. Connected File cache invalidation

The graph's consumer signature now includes every connected `Data::File`'s
native path, size and modification time, in addition to upstream signature
and output index. The application's `read_files_stamp` also includes successful
cached File outputs by node id, output index, path and metadata, along with
existing read-path parameters. See [graph.rs](../../crates/vernon-graph/src/graph.rs).

This catches external replacement, deletion and recreation even when a producer
keeps returning the same path and remains cached. Unchanged metadata retains
the cached reader. It is **size/mtime metadata tracking**, not content hashing
or recursive directory watching: same-size changes with unchanged/resolution-
indistinguishable modification time can be missed. Files hidden in arbitrary
JSON path strings are not automatically connected File values.

The tracked regression
[`connected_files_join_the_signature_and_poll_stamp`](../../crates/vernon-graph/src/graph.rs)
checks that unchanged reads stay cached; an external different-length rewrite
changes the poll stamp and reader output without recooking the producer;
deletion invalidates success, and recreation invalidates a cached read error.

## 9. Source and evidence basis

The implementation and adapter files linked above are the authority for the
current contract. Primary public method sources already recorded in the WP5
designs and existing source inventory are:

- Luo, Baran, Rusinkiewicz and Matusik, *Chopper: Partitioning Models into 3D-Printable Parts* (2012): [author project page](https://pixl.cs.princeton.edu/pubs/Luo_2012_CPM/index.php) and [paper](https://gfx.cs.princeton.edu/pubs/Luo_2012_CPM/chopper.pdf). Vernon's sampled BSP adaptation has explicit bounds and different proxies.
- Covestro, *Snap-Fit Joints for Plastic*: [primary design guide](https://solutions.covestro.com/-/media/covestro/solution-center/brands/downloads/imported/1557218421.pdf). The beam relationship informs the formula; no molded-grade value is represented as an FDM measurement.
- Prusa Research, [cut tool documentation](https://help.prusa3d.com/article/cut-tool_1779), for public connector and cut behavior; no vendor geometry implementation copied.
- Klipper, [G-code reference](https://www.klipper3d.org/G-Codes.html), for command distinctions; acceptance remains limited to the current explicit list and static model.
- OrcaSlicer, [printer settings](https://www.orcaslicer.com/wiki/printer_settings/basic%20information/printer_basic_information_advanced) and [release inventory](https://github.com/OrcaSlicer/OrcaSlicer/releases), for the retained preset/interface basis. An external schema/import check remains absent.

Transient local design and verification logs are ignored. The measured fixture
values and limitations above are retained in this tracked document; their
regression tests are in the linked tracked Rust sources.

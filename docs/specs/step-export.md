# STEP export: implemented contract and validation limits

WP4 implementation contract, checked against the native writer on 2026-10-28.
This document supersedes the STEP proposals in
[reverse-engineering surfacing](reverse-engineering-surfacing.md).

## Node interface

`surface.write_step` is a native Rust writer for ISO 10303-21 (Part 21)
files. It needs no CAD library or external program during an ordinary cook.
Lengths are millimetres. Patch sets declaring another unit are rejected.

| Port index | Input | Kind | Requirement |
|---|---|---|---|
| 0 | `patches` | Json (`nurbs_patch_set`) | Optional; nonempty when connected |
| 1 | `mesh` | Geometry | Optional; closed polygon primitives |

At least one input must contain exportable faces. Both inputs may be connected;
they produce separate components in one product, without sewing mesh faces to
patch faces. Outputs remain File at index 0 and Report at index 1.

| Parameter | Default | Contract |
|---|---|---|
| `schema` | `ap214` | `ap214` or `ap242` |
| `name` | `vernon_surface` | Nonempty part name; also determines the sanitized `.stp` filename |
| `save_to` | Empty | Optional additional copy within `ctx.work_dir`; relative paths resolve there |
| `max_faces` | 500,000 | UI range 100–50,000,000; bounds actual emitted mesh faces plus patch faces |

The node validates the generated document before writing. The main output and
optional copy resolve within the node work directory; parent traversal and
outside paths are rejected. Each file is replaced atomically after its contents
have been written to a sibling temporary file. An unsuccessful optional copy
can still leave a successfully written main output; the two replacements are
not one filesystem transaction.

## Document and topology

Headers select `AUTOMOTIVE_DESIGN` for AP214 or
`AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF` for AP242. Product,
product-definition, representation-context and shape-definition links accompany
the geometry. The unit context contains millimetres, radians and steradians,
with a declared length uncertainty of `1e-6` mm. The timestamp is fixed to keep
the content deterministic. Apostrophes are escaped; unsupported Unicode,
control characters and backslashes in names become underscores. Real numbers
use a decimal mantissa and an uppercase `E` exponent.

Each edge-connected component has its own shell and appropriately typed shape
representation, linked to the same product-definition shape:

| Geometry | Shell and item | Representation |
|---|---|---|
| Closed faceted component with positive volume | `CLOSED_SHELL`, `FACETED_BREP` | `FACETED_BREP_SHAPE_REPRESENTATION` |
| Closed patch component with nonzero sampled volume | `CLOSED_SHELL`, `MANIFOLD_SOLID_BREP` | `ADVANCED_BREP_SHAPE_REPRESENTATION` |
| Open component | `OPEN_SHELL`, `SHELL_BASED_SURFACE_MODEL` | `MANIFOLD_SURFACE_SHAPE_REPRESENTATION` |

Disconnected components never share one purportedly connected closed shell.
A mixture of open shells and solids retains their separate representation
types.

### Faceted input

The writer welds mesh vertices using `max(1e-8, 1e-13 * maximum absolute
coordinate)` mm, then orients the welded faces consistently. Every planar
polygon becomes a `FACE_SURFACE` on a `PLANE`, bounded by a `POLY_LOOP` of
shared Cartesian points. Plane-distance tolerance is `1e-8` mm. A nonplanar
polygon becomes the explicit triangle fan used by `Geometry::triangles`, and
those triangles count against the face limit. This is not a general polygon
repair or intersection algorithm.

Invalid indices, open/empty polygon primitives, degeneracies, nonmanifold edge
incidence and inconsistent/nonorientable topology are rejected. A closed mesh
component must have positive signed volume for solid export; otherwise it is
exported as an open surface model.

### Patch input

Clamped B-spline surfaces preserve their control grids, degrees, knots and
rational weights. Nonrational surfaces use `B_SPLINE_SURFACE_WITH_KNOTS`;
rational surfaces use the corresponding complex entity with
`RATIONAL_B_SPLINE_SURFACE`. Each `ADVANCED_FACE` has four complete parameter
boundary curves, expressed as `EDGE_CURVE` and `ORIENTED_EDGE` entries in an
`EDGE_LOOP`. Rational boundary curves retain their weights too.

Corners within `1e-9` mm reuse a `VERTEX_POINT`. A whole boundary edge is shared
only when its endpoints agree and its degree, raw knot sequence and raw
weight sequence match exactly, with control points within `1e-9` mm. Reversed
curve direction is considered. Omitted weights are equivalent to explicit
unit weights. Different parameter ranges, globally rescaled weights, knot
refinements and partial overlaps can leave geometrically equivalent boundaries
unshared. This conservative predicate avoids claiming a join from a small
dimensionless weight difference that causes a large physical gap.

Face orientation propagates over shared edges. Actual edge incidence, rather
than the input `closed`/`solid` flags, determines closure. Closed patch
components also require absolute sampled signed volume above `1e-12` mm³.
The volume estimate uses an 8-by-8 subdivision per patch and one local origin
per component, recentering control points before evaluation. A negative volume
reverses the component's face senses. Sampling is not a proof against folded
or self-intersecting surfaces.

Unmatched or partial seams remain open. Collapsed boundaries with endpoint
distance below `1e-10` mm are unsupported. There is no analytic CAD trimming,
arbitrary intersection, partial-edge splitting, periodic seam construction or
self-intersection certification.

## Reports and bounds

| Metric | Meaning |
|---|---|
| `faces`, `patches` | Total emitted faces and number of patch faces |
| `entities`, `file_bytes` | Number of Part 21 data entities and document bytes |
| `solid_shells`, `open_shells` | Exported component counts |
| `closed_shell` | 1 only when at least one solid exists and no open shell exists |
| `shared_patch_edges` | Patch edges used by two faces |
| `validated_entities`, `validated_references` | Counts returned by the independent structural parser |
| `bbox_x_mm`, `bbox_y_mm`, `bbox_z_mm` | Extents of emitted geometry/control points; conservative control bounds for curved patches |

The parser entity count must match the writer summary. An informational note
explains untrimmed patch boundaries and the limits of structural validation.

Writer limits are 256 patches, 256 control points per surface axis (65,536 per
patch), degree at most 16, finite coordinate magnitude at most `1e50`, finite
knot magnitude at most `1e100`, positive weights in `[1e-100, 1e100]`, and each
parameter-domain span at least `1e-100`. Knot order, multiplicities, grid sizes
and weights are validated; oversized integer metadata returns an error rather
than overflowing. The face limit includes triangulated faces and both inputs.

The node's independent parser also caps documents at 128 MiB, data entities at
1,000,000 and nesting depth at 64. Those caps can reject an output even when
its face count is within `max_faces`.

## Validation evidence and external check

`step_validate.rs` parses the Part 21 subset independently of the writer. It
checks required header names, simple and complex entities, strings/comments,
numeric syntax, unique positive entity ids, resolved references, selected
entity-kind links, spline knot/grid/weight structure, edge-loop endpoint
identity, shell connectivity and incidence, and permitted representation items.
Its tests include hand-authored malformed documents rather than only output
from the writer.

This is a bounded structural subset validator, not a complete ISO/EXPRESS
validator. Unrecognized metadata receives syntax/reference checking only.
Header values, every EXPRESS rule, geometric coincidence, face folding and
self-intersection are not certified by it.

Native regressions cover planar cube facets, disconnected boxes, mixed mesh
and rational patches, adjacent/reversed shared patch edges, a six-patch solid,
nonplanar triangulation, unsupported partial seams, invalid/resource cases,
scientific notation and escaped names. They also cover translated closed patch
solids and large-scale rational curves that must remain separate.

An ignored test, marked `needs FreeCAD`, writes four fixtures and invokes an
existing command-line executable as a separate process. It checks AP214/AP242
boxes, two shared planar patches and a rational quarter-cylinder. Imported
shapes must be non-null and valid, with expected solid counts, bounding-box
dimensions within `1e-4` mm, and applicable surface areas within `1e-3` mm².
The process must exit successfully within 60 seconds and print its completion
marker. The test passes a Python script file to the executable.

From a configured repository PowerShell, with an existing executable available:

```powershell
. .\scripts\dev-env.ps1
$env:FREECADCMD = 'C:\path\to\FreeCADCmd.exe'
cargo test -p vernon-nodes-surface freecad_import -- --ignored --nocapture
```

Without `FREECADCMD`, the test tries `FreeCADCmd` on PATH. It writes its script,
fixtures, configuration and log beneath the test work directory. It does not
install or download a runtime. FreeCAD remains a separate optional validation
process, listed in [the clean-room external-tool table](../../CLEAN_ROOM.md).

**Independent CAD import has not been run for this checkpoint.** Targeted
discovery found no existing FreeCAD command-line executable or usable
FreeCAD/OpenCascade Python module in the checked locations. Passing the native
parser tests must not be presented as a successful CAD round trip.

## Public sources

The implementation uses published format descriptions, not proprietary
installation files or copied vendor code. Entity/representation choices were
checked against the public STEP Tools EXPRESS documentation:

- [Faceted representation constraints](https://downloads.steptools.com/docs/stp_aim/html/t_faceted_brep_shape_representation.html).
- [FACE_SURFACE](https://www.steptools.com/docs/stp_aim/html/t_face_surface.html).
- [FACETED_BREP](https://www.steptools.com/docs/stp_aim/html/t_faceted_brep.html).
- [FreeCAD project licence](https://github.com/FreeCAD/FreeCAD/blob/main/LICENSE), for the optional external test program.

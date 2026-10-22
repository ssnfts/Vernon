# Spec: geometry basics (general-purpose geometry nodes, `geo.*`)

Clean-room spec. These are the plain building blocks every DCC tool has: primitives, transform, merge, mirror,
normals, reverse, triangulate, grouping, deleting, attribute creation, measuring, welding, linear subdivision and
file read/write. Behaviour comes from common knowledge of mesh processing (textbook formulas, public format
documents); no vendor source or binary was inspected. Reference tools are named for documentation only and never
appear in UI strings (see `CLEAN_ROOM.md`): 3ds Max (Box / Plane / Sphere / Tube, Mirror, Normal, Edit Poly),
Houdini (Box, Grid, Sphere, Tube, Merge, Transform, Normal, Reverse, Divide, Fuse, Group, Delete, Attribute Create,
Measure), generic mesh-statistics panels.

Reference version: not tied to a tool release; the behaviours below are stable across the common versions.

## 1. Conventions

- Lengths in millimetres, right-handed, Z up, f64 internally. Files carry a unit (mm, cm, m, in) that the file nodes
  convert to and from millimetres.
- Faces wind counter-clockwise seen from outside; the outward normal is `(p1-p0) x (p2-p0)` for a convex face.
  A closed surface with outward winding has positive enclosed volume.
- Standard attributes: point `N` (vector, unit length), `uv` (vector, x/y used), prim `area` (float, mm^2), prim
  `N` (face normal). Groups are named lists of point or prim indices (`outer`, `inner`, `edge` on the car panel).
- Every node is pure: it never changes its input, only the geometry it returns. A node with a missing required input
  returns a cook error, never a panic.

## 2. Primitives

All primitives have `center` (vec3, mm, default 0,0,0), emit point `N` and `uv`, and are closed with outward winding
unless noted.

| Node | Parameters (default, range) | Result |
|---|---|---|
| `geo.box` | `size` vec3 (100,100,100); `divisions` int (1, 1..256) | Quad box, 6 sides, `divisions` quads per edge, outward winding. |
| `geo.grid` | `size_u` (1000), `size_v` (1000) mm; `cols` (10, 1..4096); `rows` (10, 1..4096); `plane` xy / yz / zx | Open quad grid, normal along +Z for xy, +X for yz, +Y for zx. Area = size_u * size_v. |
| `geo.sphere` | `radius` (50); `rows` (16, 2..1024); `cols` (32, 3..2048) | UV sphere, pole triangles plus quads. Area and volume converge to `4 pi r^2` and `4/3 pi r^3` from below as the counts grow. |
| `geo.tube` | `radius_bottom` (25); `radius_top` (25); `height` (100); `rows` (1); `cols` (24); `caps` (true); `axis` x / y / z (z) | Truncated cone or cylinder along the axis, base at the centre minus half height. With `caps` it is closed. |
| `geo.car_panel` | `length` (900); `width` (600); `crown` (40); `flare` (60) mm; `cols` (36); `rows` (24); `thickness` (0) mm | Doubly curved fender-like test surface, see below. |

### 2.1 Car panel (synthetic test surface)

A stand-in for a scanned fender so every stage can be exercised without real scan data. Plan extent is `length` along X
and `width` along Y, centred on the origin. With `s = 2x / length` in [-1, 1] and `t = y / width + 1/2` in [0, 1]:

```
z(x, y) = crown * (1 - s^2) * (1 - (2t - 1)^2)  +  flare * smoothstep(1/2, 1, t) * (1 - s^2)^2
```

Golden values: `z = crown` at the centre, `z = flare` at the middle of the +Y edge, `z = 0` along the -Y edge and both X ends.
`thickness = 0` gives one open surface with outward (+Z) normals. `thickness > 0` offsets a copy by `thickness` along
-Z, closes the rim with side quads and flips the inner copy, giving a closed solid whose volume is within a small
tolerance of `area * thickness`; prim groups `outer`, `inner`, `edge` identify the three regions.

## 3. File read / write

- `geo.file_read`: `path`, `unit` (mm), `weld` (true), `weld_tolerance` (1e-6 mm). Outputs geometry and a Report with
  `point_count`, `prim_count`, `size_x/y/z`. Supported formats come from the `vernon-io` crate (OBJ, STL, PLY, 3MF, XYZ).
  A missing or unknown file is a cook error.
- `geo.file_write`: `path` (relative paths go to the work folder; the extension picks the format), `unit` (mm),
  `ascii` (false). Passes the geometry through and outputs the file path.
- Round trip: write then read gives the same counts and bounding box to within the unit rounding of the format.

## 4. Transform, merge, mirror

- `geo.transform`: `translate`, `rotate` (degrees), `rotate_order` (xyz and the other five orders), `scale`,
  `uniform_scale`, `pivot`, `fix_winding` (true). Order of application: scale, then rotate, both about `pivot`, then
  translate. `N` is transformed by the inverse transpose of the linear part and renormalised. A negative scale count
  that is odd mirrors the geometry; with `fix_winding` the faces are reversed so the volume stays positive.
- `geo.merge`: inputs `a`, `b`, optional `c`. Concatenates points and prims, offsets indices, keeps attributes by name
  (missing values default to zero) and keeps groups (same-name groups are unioned).
- `geo.mirror`: `axis` x / y / z (y), `origin` (0 mm), `keep_original` (true), `weld_seam` (true), `tolerance`
  (0.01 mm). Reflects across the plane `axis = origin`, reverses the reflected faces so they stay outward (they join prim group
  `mirrored`), and with `weld_seam` the copy shares the original points within `tolerance` of the plane, snapped onto it. Mirroring a half panel across its centreline gives
  `2 * points - seam points` points and a continuous surface.

## 5. Normals, reverse, triangulate, fuse, subdivide

- `geo.normals`: `class` points / prims / both (both), `weighting` angle / area / equal (angle). Face normal is the
  normalised Newell vector. Point normal is the weighted sum of adjacent face normals: angle weights use the interior
  angle at that corner, area weights use the face area, equal uses 1.
- `geo.reverse`: `group` (empty = all), `flip_normals` (true). Reverses vertex order of the selected faces and negates
  `N` of the affected elements. Enclosed volume changes sign when applied to a whole closed surface.
- `geo.triangulate`: `method` ear_clip / fan (ear_clip), `group`. Splits faces with more than three corners; ear clipping
  handles concave planar faces (convex quads split along the shorter diagonal) and preserves orientation and total area; prim attributes and group membership carry to
  every child triangle.
- `geo.fuse`: `tolerance` (0.001 mm), `snap` average / first (average), `remove_degenerate` (true). Merges points
  within the tolerance (grid-hash clustering), rewrites prim vertices, drops duplicate consecutive vertices and faces that
  collapse below three corners (below two for lines). Clusters are single-linkage; with `snap` first the merged point keeps its lowest-index member's position.
- `geo.subdivide`: `iterations` (1, 0..8), `tris_to_quads` (false). Linear (non-smoothing) split: edge midpoints and a
  centroid, shape and area unchanged, point attributes interpolated, groups extended to new points whose parents are
  all members. A triangle becomes four triangles (or three quads), any other face one quad per corner around its centroid, a polyline gains midpoints.

## 6. Grouping, deleting, attributes

- `geo.group`: `name` (group1), `element` prims / points (prims), `method` bbox / normal (bbox), `bbox_center`,
  `bbox_size`, `direction` (0,0,1), `spread` (30 degrees), `combine` replace / union / intersect / subtract (replace).
  A point is in a box group when inside the box (inclusive); a face is in when its centroid is inside. A normal group
  holds elements whose normal is within `spread` degrees of `direction`.
- `geo.delete`: `mode` group / bbox (group), `element` prims / points (prims), `group`, `bbox_center`, `bbox_size`,
  `invert` (false), `remove_unused` (true). Deleting points deletes every prim that uses them. `remove_unused` also
  drops points no prim references and renumbers the survivors.
- `geo.attrib_create`: `name`, `class` point / prim / detail (point), `type` float / int / vector / string (float),
  `value`, `string_value`, `group` (restrict to a group; unselected elements keep their previous value, or zero when the column is new). Invalid names are a cook error.

## 7. Measure and the report it emits

`geo.measure` has options `prim_area` (store per-face area, true) and `require_watertight` (false). It passes the
geometry through and emits a Report titled "Measure":

| Metric | Unit | Definition |
|---|---|---|
| `point_count`, `prim_count`, `triangle_count` | count | Triangle count after fan triangulation of all faces. |
| `surface_area` / `surface_area_m2` | mm^2 / m^2 | Sum of triangle areas. |
| `volume` | mm^3 | Signed volume `sum(p0 . (p1 x p2)) / 6` over outward triangles; meaningful only when closed. |
| `size_x`, `size_y`, `size_z`, `bbox_diagonal` | mm | Axis-aligned bounding box extent and its diagonal. |
| `centroid_x/y/z` | mm | Area-weighted surface centroid (point average if there is no area). |
| `open_edges`, `non_manifold_edges` | count | Edges used by exactly one / more than two faces. With `require_watertight` both carry a limit of 0 (lower is better). |

Notes: an open surface gets an Info note ("volume is not meaningful"), or an Error note when `require_watertight` is set;
a closed surface with negative volume gets a Warning that the faces wind inward. These numbers are what the surface,
split/print and CFD stages gate on (watertight, size, volume, area).

## 8. Undefined or implementation-specific behaviour

- Winding of an input with mixed orientation is not repaired by any node here; `geo.measure` volume is then not meaningful.
- Non-planar polygons are triangulated in the best-fit plane of the face (Newell normal); results on strongly warped
  faces are not specified.
- Point-attribute interpolation in `geo.subdivide` is linear for float and vector columns (a vector column named `N` is
  renormalised); integer and string columns take the heaviest source.

## 9. Test vectors

- Box 100 mm: area 60000 mm^2, volume 1e6 mm^3, 12 * divisions^2 triangles, 0 open edges.
- Grid 1000 x 1000: area 1e6 mm^2, `open_edges` equals the perimeter edge count.
- Sphere r = 50, 64 x 128: area within 0.1 % of 31415.9 mm^2, volume within 0.2 % of 523598.8 mm^3.
- Car panel defaults: height at the centre 40 mm, at the +Y edge mid-length 60 mm, along the -Y edge 0 mm.
- Mirror of a half panel with a seam: point count `2 * n - seam`, bounding box doubled across the mirror axis.
- Scale (-1, 1, 1) with `fix_winding` keeps volume positive and equal to the original.

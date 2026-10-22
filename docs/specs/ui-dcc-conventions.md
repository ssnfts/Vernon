# Spec: DCC-style UI layout and interaction conventions

Clean-room spec for the Vernon application shell (viewports, command panel, explorers, node editor,
timeline, hotkeys, theme) and the headless `geometry.*` / `performance.*` support nodes that compute
what the shell draws. Layout and behaviour only; no artwork, strings or code from any tool. Written
from public help pages and published papers (section 6). Reference tools may be named here; they must
never appear in user-visible strings (CLEAN_ROOM.md). Confidence tags: **[doc]** read in a public
page this pass, **[snip]** seen only in a search-result snippet (verify), **[report]** from
`3D printed body kit toolchain.md`, **[recalled]** general knowledge or the task brief, verify before
relying on it, **[design]** a Vernon choice, not a claim about any tool.

## 1. Scope and reference tools

| Reference | Version basis | Used for |
|---|---|---|
| 3ds Max help (window, status bar, create panel, scene explorer, schematic view, nav, grid) | 2015 to 2017 pages for layout, 2026 pages for shortcuts and stack controls | layout and behaviour model |
| Same help, ViewCube, time configuration, type-in | search snippets across 2012 to 2027 | secondary; tagged [snip] |
| Shoemake arcball (1992), Moller-Trumbore (1997), layered graph drawing (1993), WCAG 2.1 contrast | published | algorithms in section 3 |

Out of scope: rendering features, animation curves and rigging, MAXScript, material editors
(Slate is reduced to "same node editor, different node set"), plug-in SDKs, workspace file formats.
Stage contract: the shell wraps the pipeline capture -> clean -> surface -> split/print -> CFD ->
FEA -> solar/flatten -> performance. Each stage emits numbers; the shell shows them as gates (4.1).

## 2. Feature catalogue (observable behaviour)

### 2.1 Window regions (top to bottom, left to right)

| Region | Behaviour | Tag |
|---|---|---|
| Application button + quick-access bar | file commands, undo, redo, workspace switch | doc |
| Menu bar | directly under the title bar; each title names the command group | doc |
| Main toolbar | quick access to common tools and dialogs | doc |
| Ribbon | toolbar docks horizontally or vertically, or floats vertically; contextual tools | doc |
| Scene Explorer | modeless; table of objects; docked at the left of the viewports in a workspace | doc |
| Viewport layout tabs | one-click switch between saved viewport arrangements; saved with the scene | doc |
| Viewports | main work area; default quad Top, Front, Left, Perspective | doc/recalled |
| Command panel | six tabbed panels, one visible at a time; default right edge; detachable | doc |
| Mini listener | single-line view of the script listener, left end of the status bar | doc |
| Status line + prompt line | status shows count and type of selection; prompt guides the active command | doc |
| Selection lock, isolate toggle | lock prevents accidental reselection; isolate hides all but selection (Alt+Q) | doc |
| Coordinate display | cursor position or transform values; typing applies a transform | doc |
| Time controls | between status bar and navigation buttons; playback buttons, time config | doc |
| Navigation buttons | right end of the status bar | doc |

Dark theme: dark chrome is the default in the reference tool [recalled]. Menu titles seen in recent
versions: Edit, Tools, Group, Views, Create, Modifiers, Animation, Graph Editors, Rendering,
Customize, Scripting, Help [recalled].

### 2.2 Viewports

| Aspect | Behaviour |
|---|---|
| Default arrangement | four viewports: Top, Front, Left (orthographic) and Perspective [recalled] |
| Label bar | clickable labels in the upper-left corner; up to five menus depending on driver: General, Point-of-View, Shading, Per-View Preference, 2D Pan Zoom (last two only in the modern driver or mode) [doc] |
| General menu | overall display options, activation, access to viewport configuration [doc] |
| Point-of-View menu | choose the view (perspective, orthographic, cameras, lights, standard six) and toggle integrated elements such as ViewCube and grid [doc/recalled] |
| Shading menu | shaded, wireframe, edged faces and similar display modes [doc/recalled] |
| Active viewport | exactly one; drawn with a coloured border (yellow in the reference) [recalled]; right-click activates without losing selection [recalled] |
| Maximise toggle | Alt+W switches the active viewport between its tile and the full area [doc]; with a layout, holding the OS key while maximised switches viewports [snip] |
| Quad menu | right-click in the active viewport, away from labels; options depend on the selection; Space locks the quad menu set [doc] |
| Rotating a head-on ortho view | converts it to a user-orthographic view and the label changes [doc] |
| Redraw / disable | ` redraws all; Ctrl+Shift+D disables a viewport [doc] |

### 2.3 Navigation

| Action | Input | Notes |
|---|---|---|
| Pan | middle mouse drag with any tool active; or Pan button | parallel to the view plane [doc] |
| Zoom | wheel, or Zoom button then drag | active viewport only; Zoom All affects every non-camera viewport [doc/snip for wheel] |
| Orbit | Alt + middle drag, or Ctrl+R then drag | uses the current orbit mode even if the button is off [doc]; a circle guide appears, dragging outside it rolls the view [snip] |
| Orbit modes | Orbit (view centre), Orbit Selected (selection stays on screen), Orbit Sub-Object | Selected falls back to plain Orbit with empty selection [doc] |
| Zoom extents | Z = selected; Ctrl+Alt+Z = all; Shift+Ctrl+Z = all viewports | [doc] |
| Zoom region, FOV | Ctrl+W region; FOV button (perspective only) | wide FOV acts like a wide lens [doc] |
| Walk-through | Up arrow, then W A S D, E up, C down, Z decelerate | perspective and camera views only [doc] |
| Camera views | dolly, truck, roll, FOV, orbit/pan camera | camera viewports only [doc] |
| Modal buttons end | Esc, right-click in viewport, or choosing another tool | [doc] |
| Home view | Alt+Home; Home icon at the ViewCube; "set current view as home" in its menu | [doc/snip] |
| Virtual viewport | NumPad 2/4/6/8 pan, NumPad +/- zoom, NumPad / toggle | [doc] |

### 2.4 ViewCube

26 clickable regions: 6 faces, 12 edges, 8 corners [snip]. A face click gives an orthogonal view, an
edge or corner click an angled one; drag to orbit; snap-to-nearest-preset option; roll arrows in face
views; compass for cardinal headings; right-click menu; Home icon at upper left [snip]. Size has four
steps Large, Normal, Small, Tiny (index 0 to 3); idle opacity default 0.5; can show in every viewport
or only the active one [snip, from the scripting interface page]. SteeringWheels are a separate tracking
menu and are out of scope [doc].

### 2.5 Home grid

Grid and Snap Settings, Home Grid tab [doc]:

| Setting | Behaviour | Default |
|---|---|---|
| Grid spacing | size of the smallest square, in current units | not stated in page; sample uses 10 [doc example] |
| Major lines every Nth | heavier line every N squares; minimum 2 | not stated [doc] |
| Perspective grid extent | half-length as a multiple of spacing; spacing 10 and extent 7 give 140 x 140 units | not stated [doc] |
| Inhibit subdivision below spacing | zooming in stops at the spacing; off lets squares keep subdividing | on [doc] |
| Inhibit perspective grid resize | grid keeps a fixed size while zooming | on [doc] |
| Dynamic update | which viewports redraw while a value changes | Active Viewport [doc] |

G toggles grid visibility. The status bar shows the size of one grid square [doc].

### 2.6 Command panel

| Tab | Contents | Tag |
|---|---|---|
| Create | row of seven category buttons (Geometry default); subcategory drop-down; Object Type rollout of labelled buttons plus an AutoGrid checkbox; Name and Color rollout (editable auto name, swatch sets viewport colour); Creation Method; Keyboard Entry; Parameters | doc |
| Create categories | Geometry, Shapes, Lights, Cameras, Helpers, Space Warps, Systems | recalled |
| Modify | name and colour at top, Modifier List drop-down (type to jump), the stack, tool buttons, then the selected entry's rollouts | doc |
| Hierarchy | pivot, inverse kinematics and link info sub-pages | recalled |
| Motion | parameters and trajectories sub-pages | recalled |
| Display | hide by category, hide and freeze, display properties | recalled |
| Utilities | a single rollout that lists and launches utilities | doc |

Rollouts can be dragged to reorder and their open or closed state persists across sessions [doc].
Creating: choose a type, click or drag in a viewport (one click, a drag, or a sequence by type),
then edit parameters at once or later [doc]. Press right-click to end a sequence [recalled].

### 2.7 Modifier stack

| Control or display | Behaviour |
|---|---|
| Order | base object entry at the bottom; object-space modifiers above it in application order; world-space bindings at the top [doc] |
| Add | list inserts above the current entry; world-space items go to the top [doc] |
| Light bulb | white = active, grey = off; click toggles; viewport and render visibility can be set separately [doc] |
| Plus/minus | expands sub-controls (gizmo, centre) [doc] |
| Pin Stack | locks the panel to the current object so selection changes do not move it [doc] |
| Show End Result | on: object shown with the whole stack; off: shown up to the highlighted entry [doc] |
| Make Unique | detaches an instanced modifier or pipeline for this object [doc] |
| Remove Modifier | deletes the entry, cancelling its effect [doc] |
| Configure Modifier Sets | shows or customises up to 32 shortcut buttons [doc] |
| Preserve Stack Position | re-selecting an object returns to its last stack entry; persists [doc] |
| Name styling | bold object = instance; bold modifier = in an instanced pipeline; italic = shared across pipelines [doc] |
| Recently used | cache of recent modifiers, default length 20 [doc] |
| Use Pivot Points | first list item with multiple objects; choose per-object pivots or shared centre; fixed at apply time [doc] |

### 2.8 Time slider, track bar, time configuration

Time slider shows the current frame and drags to any frame in the active segment; right-click opens a
key-creation dialog [doc]. Track bar: frame-numbered strip to move, copy, delete keys, also for several
selected objects; a mini curve editor button replaces slider and track bar until closed [doc]. Time
configuration opens by right-clicking a time button: frame-rate presets NTSC (30), Film (24), PAL (25)
and Custom; playback real-time (frames skipped to keep rate), loop, speed default 1x; active segment
default 0 to 100 [snip]. Playback buttons: start, previous key, play, next key, end, key-mode toggle,
current-frame field [recalled]. Time tags label points on the timeline [doc].

### 2.9 Status bar, coordinates

Coordinate display accepts X, Y, Z. Absolute/Offset toggle: off (default) treats entries as absolute,
on applies them as offsets and the offset fields return to 0 after each operation [snip]. Type-in
dialog opens with F12 [snip]. Undo is split: Ctrl+Z / Ctrl+Y act on the scene; Shift+Z / Shift+Y act on
viewport operations [doc]. Adaptive degradation toggle (O) trades quality for speed [doc].

### 2.10 Scene Explorer

Modeless table, one row per object, one column per property; default columns are name and frozen
[doc]. Sort By Layer (drag to edit layers) and Sort By Hierarchy (drag to edit parents) [doc]. A bulb
column toggles visibility; hiding a layer hides children [doc]. Search plus advanced filter with Boolean
conditions [doc]. Selection syncs both ways with the viewports [doc]. Several explorers at once
(layer, container, material and others); local ones save with the scene, global ones work in all scenes
[doc]. Context menu: isolate, freeze, unfreeze (explorer-only because frozen items cannot be picked in
viewports) [snip].

### 2.11 Node editors (Schematic and Slate)

Schematic view: scene graph with one named box per entity, lines for relationships (hierarchy,
modifiers, materials, controllers, constraints, wired parameters) [doc]. Box state: solid end =
arranged, open end = free, red border = animated, bold = instanced [doc]. Selected in the editor = white
fill; selected in a viewport = white border [doc]. Display floater hides node categories; list view
gives sortable text lists to edit wiring and make instances unique [doc]. Layout: auto-arrange or free
placement, new nodes avoid overlap, optional background grid [doc]. Each view is a named window saved
with the file [doc]. The material editor uses the same node-and-wire idea and opens with M [doc].

### 2.12 Default shortcuts used by Vernon (from the 2026 table)

| Keys | Command | Keys | Command |
|---|---|---|---|
| Q | smart select | W | select and move |
| E | select and rotate | R | smart scale |
| A / S | angle snap / snaps toggle | Alt+S | cycle snap type |
| F5 F6 F7 | gizmo X, Y, Z constraint | F8 | cycle plane constraint |
| - and = | gizmo size down, up | Insert | affect-pivot-only |
| Ctrl+A / D / I | select all / none / invert | Ctrl+Shift+N | selection lock |
| Alt+Q | isolate selection | Alt+H / I / U | hide selected / unselected / unhide all |
| P T F L | perspective, top, front, left | U C | ortho user, camera |
| Alt+W | maximise toggle | Alt+Home | home view |
| F3 | wireframe / shaded toggle | F4 | edged faces toggle |
| F2 | shade selected faces | G | grid toggle |
| Shift+F | safe frames | Shift+G, H, L, C, S, W, P | hide geometry, helpers, lights, cameras, shapes, space warps, particles |
| 7 | statistics | Alt+X | see-through |
| Z | zoom extents selected | Ctrl+Alt+Z | zoom extents |
| Shift+Ctrl+Z | zoom extents all | Alt+Z | zoom mode |
| Ctrl+W | zoom region | I | pan viewport |
| Ctrl+R | orbit | [ and ] | zoom in, out |
| Ctrl+Z / Ctrl+Y | scene undo, redo | Shift+Z / Shift+Y | viewport undo, redo |
| X | global search | Space | lock quad set |
| Ctrl+Shift+A | select similar | O | adaptive degradation |

No bottom-view key appears in the table [doc]; Vernon assigns B [design].

## 3. Algorithms (published methods, implementer detail)

**3.1 Camera fit (zoom extents)** [design on elementary geometry]. c = (min+max)/2, r = |max-min|/2
(conservative bounding sphere; a tighter sphere uses Ritter, "An Efficient Bounding Sphere", Graphics
Gems, 1990, https://github.com/erich666/GraphicsGems [recalled]). Effective half-angle
t = min(tan(fov_v/2), aspect * tan(fov_v/2)); distance d = margin * r / sin(atan(t)). Orthographic:
half-height = margin * r / max(1, 1/aspect). Near = max(1e-3 d, d - 1.01 r), far = d + 1.01 r.
Empty selection falls back to scene bounds; empty scene falls back to a 1000 mm cube [design].

**3.2 Orbit** (Shoemake, "ARCBALL: A User Interface for Specifying Three-Dimensional Orientation Using
a Mouse", Graphics Interface 1992, pp. 151-156, https://graphicsinterface.org/wp-content/uploads/gi1992-18.pdf).
Radius R = min(W,H)/2 px about the viewport centre. Map a pixel to the unit sphere:
p = (x/R, y/R, sqrt(1-x^2-y^2)) when x^2+y^2 <= 1, else normalise (x,y) and set z = 0. For a drag from
p0 to p1 the quaternion q = (p0 . p1, p0 x p1) rotates by TWICE the angle between the points. Apply
q in camera space about the pivot: pos' = pivot + q(pos - pivot), up' = q(up). Starting outside the
circle gives a roll about the view axis: angle = atan2 change of the pointer about the centre [snip
behaviour, design maths]. Pivot per mode: view centre, selection centre, sub-selection centre.
Optional turntable mode (yaw about world up, pitch clamped to +-89.9 deg) [design].

**3.3 Pan.** world-per-pixel w = 2 d tan(fov_v/2) / H for perspective at pivot depth d; w =
2 * half_height / H for orthographic. Drag (dx, dy) moves camera and target by -w (dx * right - dy * up)
[design].

**3.4 Wheel zoom to cursor.** Scale factor s = 1.1 ^ (-notches) [design]. Take anchor point a (hit from
3.5 or the focal-plane point under the cursor). Perspective: cam' = a + (cam - a) * s. Orthographic:
half_height' = s * half_height and centre' = a + (centre - a) * s, so the world point under the cursor
does not move. Clamp d to [1e-3, 1e6] * scene_diag [design].

**3.5 Picking.** Ray from the pixel through the inverse projection; broadphase by axis-aligned box slab
test (Kay and Kajiya, 1986) over a BVH, narrowphase Moller and Trumbore, "Fast, Minimum Storage
Ray-Triangle Intersection", J. Graphics Tools 2(1):21-28, 1997,
https://en-cg-web.coecis.cornell.edu/pubs/1997/MT97.html : with e1 = v1-v0, e2 = v2-v0, P = D x e2,
det = e1 . P; reject |det| < 1e-12; u = (T . P)/det with T = O - v0; v = (D . Q)/det with Q = T x e1;
hit if u,v >= 0, u+v <= 1, t = (e2 . Q)/det > 0. Edge and vertex picks use a 4 px screen tolerance
[design]. Gizmo handles keep constant screen size: scale = 2 d tan(fov_v/2) * px / H [design].

**3.6 ViewCube region.** Cube of half-width 1 at the origin, rotated by the inverse camera rotation.
Ray-cube hit h; the face axis has |h_i| = 1. For each other axis i with |h_i| > 1 - b (band b = 0.3
[design]) set s_i = sign(h_i), else 0; s on the face axis is sign(h_i). Non-zero count 1/2/3 = face/
edge/corner; 3^3 - 1 = 26 regions. View direction = normalise(s) (camera sits along s). Transition: slerp
of orientation over 0.3 s, ease in-out [design]. Snap option: when dragging, round the orientation to the
nearest of the 26 directions if within 10 deg [design]. Roll arrows add +-90 deg about the view axis.

**3.7 Stack evaluation.** The stack is a linear view of a chain in the node graph. Evaluate in
topological order (Kahn, "Topological sorting of large networks", CACM 5(11):558-562, 1962,
https://doi.org/10.1145/368996.369025). Cache per node key = hash(node id, params, input keys).
Editing a node dirties itself and every downstream node. Bypassed node (bulb off): output = input if
the port types match, else the bulb is disabled. Show End Result off: the viewport shows the output of
the selected entry. Cycle -> `E_CYCLE`. Remove node: reconnect predecessor to successor if types are
compatible, else leave the successor unconnected and flag it [design].

**3.8 Layered graph layout** (Gansner, Koutsofios, North, Vo, "A Technique for Drawing Directed Graphs",
IEEE TSE 19(3):214-230, 1993; overview in the dot guide, https://web.eecs.utk.edu/~bvanderz/cs461/dotguide.pdf).
Four passes: rank by network simplex, order within ranks to cut crossings (weighted median plus
transpose), coordinates, edges. Now: longest-path ranking, barycentre ordering with 24 sweeps, rank
gap 120 px, node gap 40 px [design]. White-box-first: network simplex and spline routing.

**3.9 Adaptive grid** [design from the documented options]. Let p = spacing / w pixels per square.
If adaptive: while p < 8: spacing *= 10; while p > 80 and spacing > base (inhibit on) or spacing > base/1000
(inhibit off): spacing /= 10. Lines at k * spacing for |k * spacing| <= extent * spacing; every Nth line
(N >= 2, default 10) is major. Fade alpha linearly to 0 over the outer 20% of the extent.

**3.10 Study timeline.** t = frame / fps; frame = round(t * fps); value(frame) = v0 + (v1 - v0) *
(frame - a) / (b - a) for active segment [a, b]. Real-time playback advances floor(elapsed * fps * speed)
frames, clamping or looping at b [snip behaviour, design maths].

**3.11 Numeric transform.** Absolute: p' = entry. Offset: p' = p + entry, then entry := 0. Per-axis
fields; units from scene units (mm default) [snip/design].

**3.12 Theme contrast** (WCAG 2.1, https://www.w3.org/TR/WCAG21/ ; secondary explainer
https://webaim.org/articles/contrast/). Channel c in [0,1]: lin = c/12.92 if c <= 0.03928 else
((c+0.055)/1.055)^2.4; L = 0.2126 R + 0.7152 G + 0.0722 B; ratio = (L1+0.05)/(L2+0.05) with L1 >= L2.
Rules: text >= 4.5, large text >= 3, icons and borders against adjacent colour >= 3. Never encode a gate
state by colour alone: add a glyph and a text label.

## 4. Vernon adaptation and proposed nodes

### 4.1 Mapping (own names, own icons)

| Reference idea | Vernon form |
|---|---|
| Quad Top/Front/Left/Perspective | same four views on vehicle axes (x forward, y left, z up, right-handed [design]); label menus read "View", "Camera", "Shading" |
| Create panel categories | one category per pipeline stage: Capture, Clean, Surface, Split and Print, Airflow, Structure, Solar, Flatten, Performance, plus Geometry; Object Type rollout lists that stage's nodes |
| Name and Color | part name and a display colour per part |
| Object (stack base) | the source node of a part (scan import, imported surface, primitive) |
| Modifier stack | the single-input chain of nodes applied to a part, bottom to top; multi-input nodes show their extra operands as sub-entries; branching is shown only in the node editor |
| Bulb | node bypass (greyed when port types differ) |
| Show End Result | "show downstream result" |
| Make Unique | detach shared parameter set; badge icons instead of bold or italic |
| Pin Stack | pin the panel to a part |
| Hierarchy tab | part tree after split (parent part, child parts, joint datums, pivots) |
| Motion tab | "Study" tab: sweep parameters (speed, yaw, air density, load case) |
| Display tab | result overlays per stage (deviation, Cp, stress, temperature, stair-step) and hide by category |
| Utilities tab | units, validators, external-tool paths, report export |
| Time slider and track bar | study slider: frame = one sweep sample; track bar shows which samples are solved, stale or failed |
| Scene Explorer | Pipeline Explorer: rows per part then per node; columns Name, Stage, Gate, Locked, Visible, Last run |
| Schematic and Slate | one node editor (M) for the same graph; the stack is a projection of it |
| Mini listener | one-line command and log strip |
| Status prompt | prompt shows the next required input or the failing gate |
| Home grid | scale-aware reference grid in mm; default spacing 10 mm, major every 10, extent 7 [design] |
| Dark theme | tokens: surface #262626, text #E6E6E6, accent #F2C14E, pass #57C26B, warn #F2C14E, fail #E5534B; light variant optional [design] |

Gate dashboard: a docked panel with one row per stage and one cell per gate (value, unit, limit,
margin, state). State = worst of its gates; a stale upstream edit greys downstream cells until re-run.
Clicking a cell selects the producing node. Example gates from the report: density ratio vs standard
air (1.127 / 1.225 = 0.920, i.e. -8.0% at 40 C dry; 1.098 / 1.225 = 0.896, -10.4% at 45 C and 30% RH)
[report]; heat margin from the slicing spec. Icons are drawn in-house (24 px grid, 1.5 px stroke); no
vendor name, logo or glyph appears in labels, help or menus.

### 4.2 Proposed nodes

Support nodes are headless: pure functions of their inputs so the shell logic is unit-testable without a
window. "Now" = implement from this spec; "WB" = white-box-first.

| # | id | Label | Inputs | Outputs | Key params (default; range; unit) | Plan |
|---|---|---|---|---|---|---|
| 1 | geometry.viewport_layout | Viewport Layout | window rect, layout name, maximised id | tile rects, active id | layout quad; splitter 0.5; 0.1-0.9; ratio. gutter 2; 0-8; px | Now |
| 2 | geometry.frame_bounds | Frame Bounds | mesh or selection, camera | aabb, sphere_r_mm, fit_distance_mm, near, far | fov 45; 10-120; deg. margin 1.05; 1-1.5. mode sphere or box | Now |
| 3 | geometry.reference_grid | Reference Grid | plane, camera, unit | line list, major list, readout | spacing 10; 0.001-1e6; mm. major_every 10; 2-100. extent 7; 1-100. adaptive on | Now |
| 4 | geometry.view_orient | View Orientation | click ray, camera | region id 0-25, view dir, up | band 0.3; 0.1-0.45. snap deg 10; 0-20. slerp 0.3; 0-1; s | Now |
| 5 | geometry.view_navigate | View Navigate | camera, drag or wheel, mode | camera | mode orbit, selected, sub, pan, zoom. wheel 1.1; 1.02-1.5. clamp 1e-3 to 1e6 x diag | Now |
| 6 | geometry.pick_ray | Pick Ray | pixel, camera, meshes | hit part, face id, t, u, v | edge tol 4; 1-12; px | Now |
| 7 | geometry.numeric_transform | Numeric Transform | part, x y z, mode | transform, readout | mode absolute or offset; unit mm | Now |
| 8 | geometry.scene_stats | Scene Statistics | meshes | vertices, triangles, aabb, volume_mm3 | selected only off | Now |
| 9 | geometry.stack_view | Stack View | graph, part | ordered entries, flags (bypass, shared, end result id) | show_end_result on; preserve position on; mru 20; 1-50 | Now |
| 10 | geometry.graph_layout | Graph Layout | nodes, edges | positions, crossing count | rank gap 120; node gap 40; sweeps 24; px | Now (simple), WB (network simplex) |
| 11 | geometry.hotkey_map | Hotkey Map | profile, user overrides | binding table, conflict list | profile default; chords on | Now |
| 12 | geometry.theme_check | Theme Contrast Check | token pairs | ratios, violations | text min 4.5; non-text min 3.0 | Now |
| 13 | performance.study_sweep | Study Sweep | parameter, v0, v1, segment | samples frame -> value, state per frame | v0 0; v1 300; unit km/h. segment 0-150; fps 30; 1-120 | Now |
| 14 | performance.gate_summary | Gate Summary | gate records (stage, id, value, unit, limit, margin, state) | rows, worst state, pass and fail counts, stale list | stale policy grey; warn band 0 | Now |

14 nodes proposed.

## 5. Golden test ideas (numbers)

Inputs are built by us; the reference tool is only consulted for behaviour, not data.

1. Zoom extents: box [-50, 50]^3 mm, fov_v 45 deg. r = 86.603 mm. Aspect 16:9: d = 226.303 mm (vertical
   governs; fov_h 72.734 deg); with margin 1.05, d = 237.619 mm. Aspect 9:16: fov_h 26.231 deg,
   d = 381.648 mm (400.731 with margin 1.05).
2. Pan scale: that camera at 1080 px height: w = 2 * 226.303 * tan(22.5 deg) / 1080 = 0.17359 mm/px;
   a 100 px drag moves the camera 17.359 mm.
3. Arcball: drag from the centre to 0.5 R right: p1 = (0.5, 0, 0.866025), rotation 60.000 deg about the
   view y axis (twice the 30 deg between points). Drag to the rim (1, 0, 0): 180 deg. Drag from the
   start and back to the start: identity within 1e-9.
4. Wheel zoom: factor 1.1, anchor at 640 px right of centre: the anchor's screen pixel changes by
   < 0.5 px after the zoom, perspective and orthographic.
5. Ray pick: ray (0,0,10) toward -z against triangle (0,0,0), (1,0,0), (0,1,0) at (0.25, 0.25):
   t = 10, u = 0.25, v = 0.25; at (0.8, 0.8) no hit.
6. ViewCube: 26 regions (6 + 12 + 8). Angle face-to-face 90 deg, face-to-edge 45 deg, face-to-corner
   54.7356 deg. Hit h = (0.2, 0.1, 1) is face +Z (Top view, up +Y); (0.8, 0, 1) is edge (+X, +Z); (0.8, 0.8, 1)
   is corner (+X, +Y, +Z). Idle opacity default 0.5.
7. Grid: spacing 10, major 10, extent 7: 15 lines per axis, 1 major (the axis through the origin).
   Spacing 100, major 10, extent 10: 21 lines per axis, 3 major (-1000, 0, 1000). Adaptive: 10 mm at
   5 mm/px is 2 px, so spacing becomes 100 mm (20 px); at 0.5 mm/px it stays 10 mm (20 px).
8. Stack: box 100 mm, planar cut at z = 40 keeping the upper part. Bulb on: end result volume
   600 000 mm3 (matches the slicing spec test 1); bulb off: 1 000 000 mm3; Show End Result off with the
   box entry selected: 1 000 000 mm3 even with the cut on. Editing the cut re-evaluates only the cut and
   later nodes (cache hits on the box = 1).
9. Time: 30 fps, segment 0-100: duration 3.333 s; frame 45 = 1.5 s. 24 fps: frame 36 = 1.5 s. 25 fps:
   100 frames = 4.0 s. Defaults segment 0-100, speed 1x, NTSC 30.
10. Study speed sweep: v 0-300 km/h over frames 0-150 (2 km/h per frame), rho 1.11 kg/m3, CdA 0.64 m2,
    ClA -0.5 m2 [report]: frame 100 = 200 km/h: q = 1 712.96 Pa (report 1 713), D = 1 096.3 N,
    P = 60.9 kW, downforce 856.5 N. Frame 125 = 250 km/h: q = 2 676.5 Pa, downforce 1 338.3 N (report ~1 340).
11. Numeric transform: object at (10, 20, 30): offset (5, 0, 0) gives (15, 20, 30) and the offset field
    returns to 0; absolute 5 on X gives (5, 20, 30).
12. Undo split: move (scene), orbit (viewport): Ctrl+Z reverts the move and leaves the camera;
    Shift+Z reverts the orbit and leaves the object.
13. Hotkeys: the 38 bindings in 2.12 resolve with zero conflicts; W, E, R, Q map to move, rotate, scale,
    select; Alt+W twice returns tile rects identical to the start.
14. Layout: 1360 x 760 px area with gutter 2: four tiles tile exactly (sum of areas = area minus gutters);
    maximise then restore gives identical rects.
15. Graph layout: chain of 9 stages gives ranks 0-8 and 0 crossings; diamond A->B, A->C, B->D, C->D gives
    ranks 0, 1, 1, 2 and 0 crossings; complete bipartite K2,2 between two ranks gives exactly 1 crossing.
16. Scene stats: cube 100 mm: 8 vertices, 12 triangles, volume 1e6 mm3. UV sphere, 128 segments x 64
    rings: 8 066 vertices, 16 128 triangles, Euler V - E + F = 2.
17. Theme (WCAG): #E6E6E6 on #262626 = 12.13; #B8B8B8 = 7.63; accent #F2C14E = 9.02; pass #57C26B = 6.73;
    fail #E5534B = 4.09 (allowed as non-text only, flagged as text); disabled #8A8A8A = 4.38 (flagged
    below 4.5, allowed only for disabled).
18. Gate summary: heat margin records from the slicing spec: ASA 100 C vs T_s 104.7 C = -4.7 K fail;
    PET-CF 205 C = +100.3 K pass; summary worst = fail, counts 1 pass 1 fail. Density ratio rows:
    0.920 and 0.896 pass the "within 12% of standard" demo gate.

## 6. Sources

Read in this pass (Autodesk help pages, paraphrased, no text copied):
- https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-F8326C68-F2F9-47F7-AC1D-BA41D7825C7C.htm (window regions)
- https://help.autodesk.com/cloudhelp/2015/ENU/3DSMax/files/GUID-212A9477-E69E-4174-BB6F-1B7FD97A4281.htm (interface overview, M for material editor)
- https://help.autodesk.com/cloudhelp/2026/ENU/3DSMax-Basics/files/GUID-A73E1B09-7BFE-4A22-8153-1D3D2237B8E9.htm (default shortcuts)
- https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-D65DE5FD-F859-4F66-9E14-F9A5C1016411.htm (zoom, pan, orbit)
- https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-010B3CF4-0D7E-4886-8C65-BF332313D423.htm (navigation controls)
- https://help.autodesk.com/cloudhelp/2026/ENU/3DSMax-Basics/files/GUID-80209C3A-C2E4-4541-8738-D1E5ECE16E9C.htm (modifier stack controls)
- https://help.autodesk.com/cloudhelp/2016/ENU/3DSMax/files/GUID-55F70CB0-0590-4F1E-9879-EF070AF11CE6.htm (home grid tab)
- https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-BE30312F-5765-4BE1-9890-8CD5CAA4EFE5.htm (viewport label menus)
- https://help.autodesk.com/cloudhelp/2016/ENU/3DSMax/files/GUID-A62CEC88-5390-4CAF-97BB-C7D07EBC6F65.htm (status bar controls)
- https://help.autodesk.com/cloudhelp/2016/ENU/3DSMax/files/GUID-86F256C7-EC0B-4731-A2FB-B6FF85991929.htm (create panel)
- https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-2569461E-C859-4D54-BAFF-C8BD078B53AC.htm (scene explorer)
- https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-842CD36E-9E73-4733-9BAA-C7876013CA72.htm (schematic view)
- https://graphicsinterface.org/wp-content/uploads/gi1992-18.pdf (arcball; confirmed by search, abstract only)
- https://en-cg-web.coecis.cornell.edu/pubs/1997/MT97.html (Moller-Trumbore citation page)
- https://web.eecs.utk.edu/~bvanderz/cs461/dotguide.pdf (dot guide, cites the 1993 layout paper)

Seen only as search-result snippets (treat as unverified):
- https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-9473CAF3-AF73-4127-A98C-58ACEF01ACAC.htm (time configuration: presets, 0-100 segment, speed 1x)
- https://help.autodesk.com/cloudhelp/2015/ENU/3DSMax/files/GUID-B9779BBF-5B04-44EF-95EC-465DD7267707.htm (transform type-in, F12, offset mode)
- https://download.autodesk.com/us/3DSmax/2012help/files/GUID-F8D9EF38-A2BB-4502-95A7-4897A7F30F2-38.htm (ViewCube)
- https://help.autodesk.com/cloudhelp/2019/ENU/3DSMax-MAXScript/files/GUID-E7C0F9E4-7FE6-4796-8CF1-52FC69A5CC67.htm (ViewCube size and opacity properties)
- https://help.autodesk.com/cloudhelp/2027/ENU/3DSMax-Tutorial/files/GUID-F9386A18-B2DE-4971-ADD2-6D22416E6F3B.htm (navigation circle, Home icon)
- https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-5B06CB73-DEA7-465E-9C6F-B8E7D454A7FB.htm (walkthrough keys)
- https://help.autodesk.com/cloudhelp/2024/ENU/3DSMax-Basics/files/GUID-6ED10DFF-77FC-42D1-884D-23EDF7C131FC.htm (utilities panel)
- https://help.autodesk.com/cloudhelp/2026/ENU/3DSMax-Customizing/files/GUID-E625E116-6070-4A02-9A31-672BA6A0A954.htm (rollout reorder and persistence)
- https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-A0647085-9080-4741-96F9-C711E9C1CA1E.htm (scene explorer menus)
- https://webaim.org/articles/contrast/ (contrast explainer); https://www.w3.org/TR/WCAG21/ (normative, not opened)
- https://doi.org/10.1145/368996.369025 (Kahn 1962) and https://github.com/erich666/GraphicsGems (Ritter 1990): cited from memory, not opened

Internal: CLEAN_ROOM.md; `3D printed body kit toolchain.md` (rho, q, CdA numbers); docs/specs/slicing-printing.md (cut and heat-gate numbers).

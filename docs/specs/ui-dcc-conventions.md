# Spec: DCC-style UI layout and interaction conventions

Clean-room spec for the Vernon application shell (viewports, command panel, explorers, node editor, timeline, hotkeys, theme) and the headless `geometry.*` / `performance.*`
support nodes that compute what the shell draws. Layout and behaviour only; no artwork, strings or code from any tool. Written from public help pages and published papers
(section 6). Reference tools may be named here and in `NodeTypeInfo.emulates`; they must never appear in user-visible strings (CLEAN_ROOM.md). Confidence tags: **[doc]** read
on a public page, **[snip]** seen only in a search-result summary (verify), **[report]** from `3D printed body kit toolchain.md`, **[recalled]** general knowledge or the task
brief, verify before relying on it, **[design]** a Vernon choice, not a claim about any tool.

## 1. Scope and reference tools

| Reference | Version basis | Used for |
|---|---|---|
| 3ds Max help: window, status bar, create panel, command panel, scene explorer, schematic view, navigation, grid, viewport controls, time slider, time configuration, type-in, walk-through | 2015 to 2026 help pages | layout and behaviour model |
| Same help: ViewCube overview (2025) and its scripting properties page (2019) | read | ViewCube behaviour |
| Same help: track bar, utilities panel, rollout order (read 2024 to 2026) | read | track bar and Utilities behaviour |
| Same help: transform gizmo detail, home icon, scene explorer menus | search summaries 2017 to 2027 | secondary, tagged [snip] |
| Shoemake arcball (1992), Moller-Trumbore (1997), layered graph drawing (1993), Kahn (1962), Ritter (1990), WCAG 2.1 | published | algorithms in section 3 |

Out of scope: rendering features, animation curves and rigging, MAXScript, material editors (Slate is reduced to "same node editor, different node set"), plug-in SDKs,
workspace file formats. Stage contract: the shell wraps the pipeline scan -> clean -> surface -> split/print -> CFD -> FEA -> solar/flatten -> performance. Each stage emits
numbers; the shell shows them as gates (4.1).

## 2. Feature catalogue (observable behaviour)

### 2.1 Window regions (top to bottom, left to right)

| Region | Behaviour | Tag |
|---|---|---|
| Application button, quick-access bar, menu bar, main toolbar | file commands, undo, redo, workspace switch; the menu bar sits under the title bar and each title names a command group; the toolbar gives quick access to common tools and dialogs | doc |
| Ribbon | docks horizontally or vertically, or floats vertically; contextual tools | doc |
| Scene Explorer | modeless table of objects; docked at the left of the viewports in a workspace | doc |
| Viewport layout tabs | one-click switch between saved arrangements; saved with the scene | doc |
| Viewports | main work area; default quad Top, Front, Left, Perspective | recalled |
| Command panel | six tabs, one visible at a time; default right edge; dock on other edges or float; right-click an empty menu-bar area to toggle | doc |
| Mini listener, status line, prompt line | single-line view of the script listener at the left end of the status bar; status shows count and type of selection; prompt guides the active command | doc |
| Selection lock, isolate, coordinate display | lock prevents accidental reselection; isolate hides all but selection (Alt+Q); coordinates show cursor position or transform values and typing applies a transform (2.9) | doc |
| Time controls, navigation buttons | time controls sit between the status bar and the navigation buttons (playback, time config); navigation buttons at the right end, maximise-viewport button at the lower right | doc |

Dark chrome is the default look in recent versions [recalled]. Menu titles seen in recent versions: Edit, Tools, Group, Views, Create, Modifiers, Animation, Graph Editors,
Rendering, Customize, Scripting, Help [recalled].

### 2.2 Viewports

| Aspect | Behaviour |
|---|---|
| Default arrangement | four viewports: Top, Front, Left (orthographic), Perspective. The help page describes a default quad layout without naming it; a saved start-scene file in the defaults folder overrides it [doc/recalled] |
| Label bar | clickable labels at upper left; up to five menus depending on driver: General, Point-of-View, Shading, Per-View Preference, 2D Pan Zoom (last two only in the modern mode) [doc] |
| General, Point-of-View, Shading menus | General: display options, activation, access to viewport configuration. Point-of-View: choose the view (perspective, orthographic, cameras, lights, six standard views) and toggle ViewCube and grid. Shading: shaded, wireframe, edged faces and similar modes [doc/recalled] |
| Active viewport | exactly one, marked by a highlighted border; the border turns red while auto-key or set-key is on. Commands apply there [doc] |
| Activation | right-click activates without changing the selection; left-click activates and also selects what was clicked or clears the selection on empty space [doc] |
| Maximise toggle | Alt+W swaps the active tile with the full area; the lower-right button does the same. With one viewport maximised, holding the OS key opens a switcher listing the others [doc] |
| Quad menu | right-click in the active viewport away from labels; options depend on the selection; Space locks the quad set [doc] |
| Misc | rotating a head-on ortho view converts it to a user-orthographic view and the label changes; the backtick key redraws all; Ctrl+Shift+D disables a viewport; layouts are saved with the scene [doc] |

### 2.3 Navigation

| Action | Input | Notes |
|---|---|---|
| Pan | middle drag with any tool active, or Pan button | parallel to the view plane [doc] |
| Zoom | wheel, or Zoom button then drag | active viewport only; Zoom All affects every non-camera viewport [doc/snip for wheel] |
| Orbit | Alt + middle drag, or Ctrl+R then drag | uses the current orbit mode even if its button is off [doc]; a guide circle appears, dragging outside it rolls the view [snip] |
| Orbit modes | Orbit (view centre), Orbit Selected (selection stays on screen), Orbit Sub-Object | Selected falls back to plain Orbit with empty selection [doc] |
| Zoom extents | Z = selected; Ctrl+Alt+Z = all; Shift+Ctrl+Z = all viewports | [doc] |
| Zoom region, FOV | Ctrl+W region; FOV button (perspective only) | wide FOV acts like a wide lens [doc] |
| Walk-through | Up arrow or button starts; W/S or Up/Down move; A/D or Left/Right strafe; E/C or Shift+Up/Down rise and sink; Q and Z toggle faster and slower; [ and ] shrink and grow the step; Alt+[ resets it; Space locks vertical look; Shift+Space levels | perspective and camera views only; exit by right-click, another viewport, a view-type change or another tool; step size saved with the file [doc] |
| Camera views, ending modal tools | dolly, truck, roll, FOV, orbit and pan camera (camera viewports only); Esc, right-click in a viewport, or choosing another tool ends a modal navigation button | doc |
| Home view | Alt+Home; Home icon beside the ViewCube; "set current view as home" in its menu | [doc/snip] |
| Virtual viewport | NumPad 2/4/6/8 pan, NumPad +/- zoom, NumPad / toggle | [doc] |

### 2.4 ViewCube

26 clickable regions: 6 faces, 12 edges, 8 corners [doc]. A face click gives an orthogonal view; an edge click gives an angled view built from two sides, a corner click one
built from three sides [doc]. In a face view, four triangles at the cube sides jump to the neighbouring face, and roll arrows turn the view 90 degrees per click, left
counter-clockwise, right clockwise [doc]. Dragging orbits to a custom angle [doc]. The cube's idea of "front" and "up" comes from a stored front view; the menu can set the
current view as front and reset it [doc]. Scripting properties [doc]: visibility; all-viewports vs active-only; size integer 0 Large, 1 Normal, 2 Small, 3 Tiny; inactive
opacity 0.0 to 1.0, default 0.5 (the only default the page states); drag snapping to the closest standard view; fit to view after a change; animated transition; keep scene up;
compass under the cube with a north angle in degrees.

The Home icon at upper left and the right-click menu are [snip]. A separate tracking-wheel menu exists and is out of scope [doc].

### 2.5 Home grid

Grid and Snap Settings, Home Grid tab [doc]:

| Setting | Behaviour | Default |
|---|---|---|
| Grid spacing | size of the smallest square, in current units | not stated; a worked sample uses 10 |
| Major lines every Nth | heavier line every N squares; minimum 2 | not stated |
| Perspective grid extent | half-length as a multiple of spacing; spacing 10 and extent 7 give 140 x 140 units | not stated |
| Inhibit subdivision below spacing | zooming in stops at the spacing; off lets squares keep subdividing | on |
| Inhibit perspective grid resize | grid keeps a fixed size while zooming | on |
| Dynamic update | which viewports redraw while a value changes | Active Viewport |

G toggles grid visibility. The status bar shows the size of one grid square [doc].

### 2.6 Command panel

| Tab | Contents | Tag |
|---|---|---|
| Create | row of seven category buttons (Geometry default); subcategory drop-down; Object Type rollout of labelled buttons plus an AutoGrid checkbox; Name and Color rollout (editable auto name; swatch sets viewport colour); Creation Method; Keyboard Entry; Parameters | doc |
| Create categories | Geometry, Shapes, Lights, Cameras, Helpers, Space Warps, Systems | recalled |
| Modify | name and colour fields at the top, Modifier List drop-down (type to jump), the stack, tool buttons, then the selected entry's rollouts | doc |
| Hierarchy | parent-child links, joints and inverse kinematics | doc |
| Motion | animation controllers and trajectories of the selected object | doc |
| Display | hide and unhide, freeze, display options | doc |
| Utilities | one rollout that lists and launches small tools: a "more" dialog adds tools missing from the current button set, a "sets" list switches button sets, and a configure dialog builds custom sets of up to 32 buttons; a running tool keeps its button lit, clicking it again or another button ends it, and some tools add rollouts below or open their own dialog | doc |

Name and colour appear at the top of every tab except Create, where they sit in a rollout [doc]. Rollouts can be dragged to reorder (the order is kept in a small text file in the
application's ui folder), and each rollout's open or closed state persists across sessions [doc]. Creating: choose a type, click or drag in a viewport (one click, a drag, or a sequence by type), then edit parameters at once or later [doc].
Right-click ends a sequence [recalled].

### 2.7 Modifier stack

| Control or display | Behaviour |
|---|---|
| Order, add | base object entry at the bottom; object-space modifiers above it in application order; world-space bindings at the top. The list inserts above the current entry; world-space items go to the top [doc] |
| Light bulb | white = active, grey = off; click toggles; viewport and render visibility can be set separately [doc] |
| Plus/minus | expands sub-controls (gizmo, centre) [doc] |
| Pin Stack | locks the panel to the current object so selection changes do not move it [doc] |
| Show End Result | on: object shown with the whole stack; off: shown up to the highlighted entry [doc] |
| Make Unique, Remove | Make Unique detaches an instanced modifier or pipeline for this object; Remove deletes the entry, cancelling its effect [doc] |
| Configure Modifier Sets | shows or customises up to 32 shortcut buttons [doc] |
| Preserve Stack Position | re-selecting an object returns to its last stack entry; persists [doc] |
| Name styling | bold object = instance; bold modifier = in an instanced pipeline; italic = shared across pipelines [doc] |
| Recently used | cache of recent modifiers, default length 20 [doc] |
| Use Pivot Points | first list item with multiple objects: per-object pivots or shared centre; fixed at apply time [doc] |

### 2.8 Time slider, track bar, time configuration

Time slider [doc]: sits in the status area, floatable or dockable; shows current frame over segment length (the sample reads 25/100); dragging scrubs and plays the animation
under it; end arrows step one frame, or jump key to key when key mode is on; right-click opens a key-creation dialog; default range 0 to 100. Track bar [doc]:
strip directly under the time slider and above the status bar, floatable and dockable; shows the keys of the selected objects against frame numbers and lets you move, copy and
delete keys or edit their properties; position, rotation and scale keys are drawn red, green, blue, other keys grey, selected transform keys white, and the current-frame
marker is a blue bar; a button at its left end opens a mini curve editor that swaps slider and track bar for a curve view until its close button is pressed. Time configuration [doc]:

| Setting | Behaviour | Default |
|---|---|---|
| Frame rate | NTSC 30, Film 24, PAL 25, Custom fps spinner | not stated |
| Time display | Frames, SMPTE, Frames:Ticks, Minutes:Seconds:Ticks. Frame 35 at 30 fps reads 35, 0:1:5, 35:0, 0:1:800 | Frames |
| Real time, active viewport only, loop | real time on skips frames to hold the rate (off plays every frame); active-viewport-only limits playback to the active viewport; loop repeats vs plays once | not stated |
| Speed | five choices from 1x down to fractions such as 1/2x; viewport playback only | 1x |
| Direction | forward, reverse, ping-pong; only when real time is off | forward |
| Start, end | active segment; may be negative | 0 to 100 |
| Length, frame count, re-scale | length = end - start; frame count = length + 1; re-scale stretches or shrinks the animation to a new length | 100, 101 |
| Key steps | key mode honours all track-bar keys, or only chosen transforms of selected objects | selected only on |

Playback buttons: go to start, previous frame or key, play (flyout: selected objects only), next frame or key, go to end, key-mode toggle, current-frame field [doc/snip]. Time
tags label points [doc].

### 2.9 Status bar, coordinates, undo

Status bar has X, Y, Z fields with a toggle to their left: off (default) = Absolute, on = Offset; offset fields return to 0 after each operation; fields turn grey with nothing
selected [doc]. Both use the active reference coordinate system and the labels change to show it [doc]. F12 opens the type-in dialog with the same fields; Enter applies;
dragging the object updates the fields [doc]. Undo is split: Ctrl+Z / Ctrl+Y act on the scene; Shift+Z / Shift+Y act on viewport operations [doc]. Adaptive degradation (O)
trades quality for speed during motion [doc].

### 2.10 Scene Explorer

Modeless table, one row per object, one column per property; default columns are name and frozen [doc]. Sort By Layer (drag to edit layers) and Sort By Hierarchy (drag to edit
parents) [doc]. A bulb column toggles visibility; hiding a layer hides children [doc]. Search plus advanced filter with Boolean conditions [doc]. Selection syncs both ways
with the viewports [doc]. Several explorers at once (layer, container, material, others); local ones save with the scene, global ones work in all scenes [doc]. Context menu:
isolate, freeze, unfreeze (explorer-only because frozen items cannot be picked) [snip].

### 2.11 Node editors (Schematic and Slate)

Schematic view: scene graph, one named box per entity, lines for relationships (hierarchy, modifiers, materials, controllers, constraints, wired parameters) [doc]. Box state:
solid end = arranged, open end = free, red border = animated, bold = instanced [doc]. Selected in the editor = white fill; selected in a viewport = white border [doc]. A
display floater hides node categories; list view gives sortable text lists to edit wiring and make instances unique [doc]. Layout: auto-arrange or free placement, new nodes
avoid overlap, optional background grid [doc]. Each view is a named window saved with the file [doc]. The material editor uses the same node-and-wire idea and opens with M
[doc].

### 2.12 Default shortcuts used by Vernon (from the 2026 table)

- Tools: Q smart select, W move, E rotate, R smart scale, A angle snap, S snaps toggle, Alt+S cycle snap type, Insert affect-pivot-only,
F5 F6 F7 gizmo axis X Y Z, F8 cycle plane constraint, - and = gizmo size down and up.
- Selection: Ctrl+A all, Ctrl+D none, Ctrl+I invert, Ctrl+Shift+A select similar, Ctrl+Shift+N selection lock, Alt+Q isolate,
Alt+H hide selected, Alt+I hide unselected, Alt+U unhide all, X global search, Space lock quad set.
- Views: P perspective, T top, F front, L left, U ortho user, C camera, Alt+W maximise toggle, Alt+Home home view, F3 wireframe/shaded,
F4 edged faces, F2 shade selected faces, G grid, Shift+F safe frames, 7 statistics, Alt+X see-through, O adaptive degradation,
Shift+G H L C S W P hide geometry, helpers, lights, cameras, shapes, space warps, particles.
- Zoom and pan: Z extents selected, Ctrl+Alt+Z extents, Shift+Ctrl+Z extents all viewports, Alt+Z zoom mode, Ctrl+W zoom region,
I pan, Ctrl+R orbit, [ and ] zoom in and out.
- Undo: Ctrl+Z and Ctrl+Y scene undo and redo, Shift+Z and Shift+Y viewport undo and redo.

Vernon status [design]: 39 of the 61 chords are bound (`BINDINGS` in `app/src/hotkeys.rs`; `[` zooms in, `]` out, Alt+Home is the home view, Ctrl+R switches the orbit
style, Ctrl+Shift+A selects objects of the same node type, 7 opens the statistics utility, X focuses the explorer's find-by-name box, Shift+G hides or shows objects that output
geometry). Vernon adds Ctrl+X, Ctrl+C and Ctrl+V for sub-graph cut, copy and paste. `keymap.json` in the user config directory overrides any bound chord
(`"action_name": "Ctrl+Shift+K"`, `null` to unbind); bad entries warn in the log and leave the default in place, and a reserved chord is refused. The other 22 name features
Vernon does not have (snap-type cycling, affect-pivot mode, gizmo axis and plane constraints and size, quad-set lock, camera view, shade-selected-faces, safe frames,
see-through, adaptive degradation, zoom and pan modes, the Shift+H/L/C/S/W/P hide categories). They stay in the `RESERVED` list of the same file, resolve to no action and cannot be given to another command;
a test checks the whole table, bound and reserved, for conflicts.

No bottom-view key appears in the table [doc]; Vernon assigns B [design]. Walk-through keys (2.3) are a modal context and may reuse letters. Vernon default mouse map: MMB pan,
Alt+MMB orbit, wheel zoom to cursor, RMB quad menu [design on 2.3].

## 3. Algorithms (published methods, implementer detail)

**3.1 Camera fit (zoom extents)** [design on elementary geometry]. c = (min+max)/2, r = |max-min|/2 (conservative bounding sphere; a tighter sphere uses Ritter, "An Efficient
Bounding Sphere", Graphics Gems, Academic Press 1990, file BoundSphere.c at https://github.com/erich666/GraphicsGems). Effective tangent t = min(tan(fov_v/2), aspect *
tan(fov_v/2)); distance d = margin * r / sin(atan(t)). Orthographic: half-height = margin * r / max(1, 1/aspect). Near = max(1e-3 d, d - 1.01 r), far = d + 1.01 r. Empty
selection falls back to scene bounds; empty scene to a 1000 mm cube [design].

**3.2 Orbit** (Shoemake, "ARCBALL: A User Interface for Specifying Three-Dimensional Orientation Using a Mouse", Graphics Interface 1992, pp. 151-156,
https://graphicsinterface.org/wp-content/uploads/gi1992-18.pdf). Radius R = min(W,H)/2 px about the viewport centre. Map a pixel to the unit sphere: p = (x/R, y/R,
sqrt(1-x^2-y^2)) when x^2+y^2 <= 1, else normalise (x,y) and set z = 0. For a drag from p0 to p1 the quaternion q = (p0 . p1, p0 x p1) rotates by TWICE the angle between the
points. Apply q in camera space about the pivot: pos' = pivot + q(pos - pivot), up' = q(up). Starting outside the circle gives a roll about the view axis: angle = change of
atan2 of the pointer about the centre [snip behaviour, design maths]. Pivot per mode: view centre, selection centre, sub-selection centre. Optional turntable mode (yaw about
world up, pitch clamped to +-89.9 deg) [design].

**3.3 Pan.** world-per-pixel w = 2 d tan(fov_v/2) / H for perspective at pivot depth d; w = 2 * half_height / H for orthographic. Drag (dx, dy) moves camera and target by -w
(dx * right - dy * up) [design].

**3.4 Wheel zoom to cursor.** s = 1.1 ^ (-notches) [design]. Take anchor a (hit from 3.5 or the focal-plane point under the cursor). Perspective: cam' = a + (cam - a) * s.
Orthographic: half_height' = s * half_height and centre' = a + (centre - a) * s, so the world point under the cursor does not move. Clamp d to [1e-3, 1e6] * scene_diag
[design].

**3.5 Picking.** Ray from the pixel through the inverse projection; broadphase by slab test (Kay and Kajiya, "Ray Tracing Complex Scenes", SIGGRAPH 1986) over a BVH;
narrowphase Moller and Trumbore, "Fast, Minimum Storage Ray-Triangle Intersection", J. Graphics Tools 2(1):21-28, 1997,
https://en-cg-web.coecis.cornell.edu/pubs/1997/MT97.html , doi 10.1080/10867651.1997.10487468 : e1 = v1-v0, e2 = v2-v0, P = D x e2, det = e1 . P; reject |det| < 1e-12; T = O -
v0; u = (T . P)/det; Q = T x e1; v = (D . Q)/det; hit if u,v >= 0, u+v <= 1, t = (e2 . Q)/det > 0. Edge and vertex picks use a 4 px screen tolerance [design]. Gizmo handles
keep constant screen size: scale = 2 d tan(fov_v/2) * px / H [design].

**3.6 ViewCube region.** Cube of half-width 1 at the origin, rotated by the inverse camera rotation. Ray-cube hit h; the face axis has |h_i| = 1. For each other axis i with
|h_i| > 1 - b (band b = 0.3 [design]) set s_i = sign(h_i), else 0; the face axis gets sign(h_i). Non-zero count 1/2/3 = face/edge/corner; 3^3 - 1 = 26 regions [doc count].
View direction = normalise(s) (camera sits along s). Transition: slerp over 0.3 s, ease in-out, skipped when animated-transition is off [design]. Drag snapping: round to the
nearest of the 26 directions if within 10 deg [design]. Roll arrows add +-90 deg about the view axis [doc]. Fit-to-view reruns 3.1 after the move.

**3.7 Stack evaluation.** The stack is a linear view of a chain in the node graph. Evaluate in topological order (Kahn, "Topological sorting of large networks", CACM
5(11):558-562, 1962, https://doi.org/10.1145/368996.369025). Cache per node key = hash(node id, params, input keys). Editing a node dirties itself and every downstream node.
Bypassed node (bulb off): output = input if port types match, else the bulb is disabled. Show End Result off: the viewport shows the output of the selected entry. Cycle ->
`E_CYCLE`. Remove node: reconnect predecessor to successor if types are compatible, else leave the successor unconnected and flag it [design].

**3.8 Layered graph layout** (Gansner, Koutsofios, North, Vo, "A Technique for Drawing Directed Graphs", IEEE TSE 19(3):214-230, 1993; overview in the dot guide,
https://web.eecs.utk.edu/~bvanderz/cs461/dotguide.pdf). Four passes: rank by network simplex, order within ranks to cut crossings (weighted median plus transpose),
coordinates, edges. Now: longest-path ranking, barycentre ordering with 24 sweeps, rank gap 120 px, node gap 40 px [design]. White-box-first: network simplex and spline
routing.

**3.9 Adaptive grid** [design from the documented options]. p = spacing / w pixels per square. If adaptive: while p < 8: spacing *= 10; while p > 80 and (spacing > base when
inhibit is on, or spacing > base/1000 when off): spacing /= 10. Lines at k * spacing for |k| <= extent; every Nth line (N >= 2, default 10) is major. Fade alpha linearly to 0
over the outer 20% of the extent.

**3.10 Study timeline and time display.** t = frame / fps; frame = round(t * fps); value(frame) = v0 + (v1 - v0) * (frame - a) / (b - a) for segment [a, b]; frame count = (b -
a) + 1 [doc]. Real-time playback advances floor(elapsed * fps * speed) frames, clamping, looping or ping-ponging at b. Display formats with integer fps and 4800 ticks per
second (derived from the documented example 0:1:800 at frame 35, 30 fps: 800 ticks = 1/6 s): s = floor(f / fps); r = f - s * fps; frames = f; timecode = floor(s/60) : s mod 60
: r; frames_ticks = f : 0; min_sec_ticks = floor(s/60) : s mod 60 : r * 4800 / fps [doc-derived, design].

**3.11 Numeric transform.** Absolute: p' = entry. Offset: p' = p + entry, then entry := 0. Per-axis fields; fields disabled with an empty selection; units from scene units (mm
default) [doc/design].

**3.12 Theme contrast** (WCAG 2.1, https://www.w3.org/TR/WCAG21/ ; explainer https://webaim.org/articles/contrast/). Channel c in [0,1]: lin = c/12.92 if c <= 0.03928 else
((c+0.055)/1.055)^2.4; L = 0.2126 R + 0.7152 G + 0.0722 B; ratio = (L1+0.05)/(L2+0.05) with L1 >= L2 [recalled for the luminance weights; ratio form and thresholds read].
Rules read on the page: normal text >= 4.5, large text >= 3, interface components and states >= 3 against the adjacent colour. Never encode a gate state by colour alone: add a
glyph and a text label.

## 4. Vernon adaptation and proposed nodes

### 4.1 Mapping (own names, own icons)

| Reference idea | Vernon form |
|---|---|
| Quad Top/Front/Left/Perspective | same four views on vehicle axes (x forward, y left, z up, right-handed [design]); label menus read "View", "Camera", "Shading" |
| Create panel categories | one category per pipeline stage: Capture, Clean, Surface, Split and Print, Airflow, Structure, Solar, Flatten, Performance, plus Geometry; the Object Type rollout lists that stage's nodes |
| Object (stack base), Name and Color | the source node of a part (scan import, imported surface, primitive); a part name and a display colour per part |
| Modifier stack | the single-input chain of nodes applied to a part, bottom to top; multi-input nodes show extra operands as sub-entries; branching is shown only in the node editor |
| Bulb, Show End Result | node bypass (greyed when port types differ); "show downstream result" |
| Make Unique, Pin Stack | detach shared parameter set (badge icons instead of bold or italic); pin the panel to a part |
| Hierarchy tab | part tree after split (parent part, child parts, joint datums, pivots) |
| Motion tab | "Study" tab: sweep parameters (speed, yaw, air density, load case) |
| Display tab | result overlays per stage (deviation, Cp, stress, temperature, stair-step) and hide by category |
| Utilities tab | units, validators, external-tool paths, report export |
| Time slider and track bar | study slider: frame = one sweep sample; track bar shows which samples are solved, stale or failed |
| Scene Explorer | Pipeline Explorer: rows per part then per node; columns Name, Stage, Gate, Locked, Visible, Last run |
| Schematic and Slate | one node editor (M) for the same graph; the stack is a projection of it |
| Mini listener, status prompt | one-line command and log strip; the prompt shows the next required input or the failing gate |
| Home grid | scale-aware reference grid in mm; default spacing 10 mm, major every 10, extent 7 [design] |
| Dark theme | tokens: surface #262626, text #E6E6E6, accent #F2C14E, pass #57C26B, warn #F2C14E, fail #E5534B; light variant optional [design]. Active viewport border = accent |

Gate dashboard: a docked panel with one row per stage and one cell per gate (value, unit, limit, margin, state). State = worst of its gates; a stale upstream edit greys
downstream cells until re-run. Clicking a cell selects the producing node. Example gates from the report: density ratio vs standard air (1.127 / 1.225 = 0.920, -8.0% at 40 C
dry; 1.098 / 1.225 = 0.896, -10.4% at 45 C and 30% RH) [report]; heat margin from the slicing spec. Icons are drawn in-house (24 px grid, 1.5 px stroke); no vendor name, logo
or glyph appears in labels, help or menus.

Status (implemented): Scene Explorer rows carry visibility, lock (locked objects cannot be picked or transformed in the viewports), colour, name, gate, stage and
last run (time since the object last produced a new result), sortable by name, stage or last run; rows are per object (a part split is one object), not per part.
Utilities: Units and Checks (scene units, nodes with errors, failing gates, unknown node types), External Tools (one program location per external program, applied to
every node that runs it) and a CSV export of every report metric (Reports page). Hotkeys: Alt+Q isolate (again to end), Alt+I hide unselected, Ctrl+Shift+N selection
lock, Ctrl+W zoom region, F12 transform type-in. Hierarchy tab: part tree with parent/child links, seam datums, pivots and joint-feature counts, read from the attributes in
`slicing-printing.md` section 5b. Not yet: editing parent links by hand, and a datum per joint feature (datums are per seam).

### 4.2 Proposed nodes

Support nodes are headless: pure functions of their inputs, so shell logic is unit-testable without a window. "Now" = implement from this spec; "WB" = white-box-first.

| # | id | Label | Inputs | Outputs | Key params (default; range; unit) | Plan |
|---|---|---|---|---|---|---|
| 1 | geo.viewport_layout | Viewport Layout | window rect, layout name, maximised id | tile rects, active id | layout quad; splitter 0.5; 0.1-0.9; ratio. gutter 2; 0-8; px | Now |
| 2 | geo.frame_bounds | Frame Bounds | mesh or selection, camera | aabb, sphere_r_mm, fit_distance_mm, near, far | fov 45; 10-120; deg. margin 1.05; 1-1.5. mode sphere or box | Now |
| 3 | geo.reference_grid | Reference Grid | plane, camera, unit | line list, major list, readout | spacing 10; 0.001-1e6; mm. major_every 10; 2-100. extent 7; 1-100. adaptive on | Now |
| 4 | geo.view_orient | View Orientation | click ray, camera | region id 0-25, view dir, up | band 0.3; 0.1-0.45. snap deg 10; 0-20. slerp 0.3; 0-1; s. size 1; 0-3 | Now |
| 5 | geo.view_navigate | View Navigate | camera, drag or wheel, mode | camera | mode orbit, selected, sub, pan, zoom. wheel 1.1; 1.02-1.5. clamp 1e-3 to 1e6 x diag | Now |
| 6 | geo.pick_ray | Pick Ray | pixel, camera, meshes | hit part, face id, t, u, v | edge tol 4; 1-12; px | Now |
| 7 | geo.numeric_transform | Numeric Transform | part, x y z, mode | transform, readout | mode absolute or offset; unit mm | Now |
| 8 | geo.scene_stats | Scene Statistics | meshes | vertices, triangles, aabb, volume_mm3 | selected only off | Now |
| 9 | geo.stack_view | Stack View | graph, part | ordered entries, flags (bypass, shared, end result id) | show_end_result on; preserve position on; mru 20; 1-50 | Now |
| 10 | geo.graph_layout | Graph Layout | nodes, edges | positions, crossing count | rank gap 120; node gap 40; sweeps 24; px | Now (simple), WB (network simplex) |
| 11 | geo.hotkey_map | Hotkey Map | profile, user overrides | binding table, conflict list | profile default; chords on | Now |
| 12 | geo.theme_check | Theme Contrast Check | token pairs | ratios, violations | text min 4.5; non-text min 3.0 | Now |
| 13 | perf.study_sweep | Study Sweep | parameter, v0, v1, segment | samples frame -> value, state per frame, display string | v0 0; v1 300; unit km/h. segment 0-150; fps 30; 1-120. display frames, timecode, frames_ticks, min_sec_ticks | Now |
| 14 | perf.gate_summary | Gate Summary | gate records (stage, id, value, unit, limit, margin, state) | rows, worst state, pass and fail counts, stale list | stale policy grey; warn band 0 | Now |

14 nodes proposed. Ids use the registry's stage prefixes (`geo.`, `perf.`).

Status (implemented): the logic of #1-#12 is UI-state logic and lives as headless, unit-tested app modules rather than graph nodes, because it works on window, camera and selection state that no graph wire carries: #1 `app/src/viewport/layout.rs`, #2/#5/#6 `crates/vernon-render/src/camera.rs`, #3 `app/src/viewport/grid.rs`, #4 `crates/vernon-render/src/navcube.rs` (26 regions, roll arrows and neighbour-face triangles in face views, drag snapping within 10 deg, four sizes, 0.5 inactive opacity, compass with north angle, stored front view, 0.3 s transition), #7 `app/src/typein.rs` (status-bar Absolute/Offset X/Y/Z fields and the F12 dialog, on top of the Move/Rotate/Scale tools), #8 the Utilities statistics page, #9 the Modify stack (Pin Stack, Make Unique, Preserve Stack Position saved with the scene, a recently used list of 20 and type-to-find in the modifier list; logic in `app/src/state.rs` and `app/src/scene_ops.rs`), #10 the node editor's layered layout, #11 `app/src/hotkeys.rs`, #12 `app/src/theme.rs`. #13 is the Motion tab's study sweep (`app/src/study.rs`): it maps the time-slider frame to a node parameter, passes the frame to every cook (`Graph::frame` -> `CookContext::frame`) and keeps a solved/failed/stale state per sample for the track bar. #14 is a registered node, `perf.gate_summary`, built on the gate rules in `vernon_graph::gate` that the app's Gate Dashboard and `vernon-cli cook` share.

## 5. Golden test ideas (numbers)

Inputs are built by us; the reference tool is only consulted for behaviour, not data. Tolerances: 1e-6 relative on geometry maths, 0.5 px on screen positions, exact on
integers and strings.

1. Zoom extents: box [-50, 50]^3 mm, fov_v 45 deg. r = 86.603 mm. Aspect 16:9: d = 226.303 mm (vertical governs; fov_h 72.734 deg); with margin 1.05, d = 237.619 mm. Aspect
    9:16: fov_h 26.231 deg, d = 381.648 mm (400.731 with margin 1.05).
2. Pan scale: that camera at 1080 px height: w = 2 * 226.303 * tan(22.5 deg) / 1080 = 0.17359 mm/px; a 100 px drag moves the camera 17.359 mm.
3. Arcball: drag from the centre to 0.5 R right: p1 = (0.5, 0, 0.866025), rotation 60.000 deg about the view y axis (twice the 30 deg between points). Drag to the rim (1, 0,
    0): 180 deg. Drag out and back to the start: identity within 1e-9.
4. Wheel zoom: factor 1.1, anchor 640 px right of centre: the anchor's screen pixel changes by < 0.5 px after the zoom, perspective and orthographic.
5. Ray pick: ray (0,0,10) toward -z against triangle (0,0,0), (1,0,0), (0,1,0) at (0.25, 0.25): t = 10, u = 0.25, v = 0.25; at (0.8, 0.8) no hit.
6. ViewCube: 26 regions (6 + 12 + 8). Angle face-to-face 90 deg, face-to-edge 45 deg, face-to-corner 54.7356 deg. Hit h = (0.2, 0.1, 1) is face +Z (Top view, up +Y); (0.8, 0,
    1) is edge (+X, +Z); (0.8, 0.8, 1) is corner (+X, +Y, +Z). Roll arrow in a face view: 90 deg per click, four clicks = identity. Size enum 0 to 3 maps to four distinct
    pixel sizes, strictly decreasing. Default inactive opacity 0.5.
7. Grid: spacing 10, major 10, extent 7: 15 lines per axis, 1 major (the axis through the origin). Spacing 100, major 10, extent 10: 21 lines per axis, 3 major (-1000, 0,
    1000). Adaptive: 10 mm at 5 mm/px is 2 px, so spacing becomes 100 mm (20 px); at 0.5 mm/px it stays 10 mm (20 px).
8. Stack: box 100 mm spanning z 0 to 100, planar cut at z = 40 keeping the upper part. Bulb on: end result volume 600,000 mm3 (matches the slicing spec test 1);
    bulb off: 1,000,000 mm3; Show End Result off with the box entry selected: 1,000,000 mm3 even with the cut on. Editing the cut re-evaluates only the cut and later
    nodes (cache hits on the box = 1).
9. Time: 30 fps, segment 0-100: duration 3.333 s, frame count 101; frame 45 = 1.5 s. 24 fps: frame 36 = 1.5 s. 25 fps: 100 frames = 4.0 s. Defaults segment 0-100, speed 1x,
    rate 30. Display of frame 35 at 30 fps: frames "35", timecode "0:1:5", frames_ticks "35:0", min_sec_ticks "0:1:800". Slider text at frame 25 of a 0-100 segment: "25/100".
    Ping-pong over segment 0-10 at frames 0..25 visits 0..10 then 9..0 then 1..5.
10. Study speed sweep: v 0-300 km/h over frames 0-150 (2 km/h per frame), rho 1.11 kg/m3, CdA 0.64 m2, ClA -0.5 m2 [report]: frame 100 = 200 km/h: q = 1712.96 Pa (report 1713), D = 1096.3 N, P = 60.9 kW, downforce 856.5 N. Frame 125 = 250 km/h: q = 2 676.5 Pa, downforce 1338.3 N.
11. Numeric transform: object at (10, 20, 30): offset (5, 0, 0) gives (15, 20, 30) and the offset field returns to 0; absolute 5 on X gives (5, 20, 30); with nothing selected
    the node returns a disabled readout.
12. Undo split: move (scene), orbit (viewport): Ctrl+Z reverts the move and leaves the camera; Shift+Z reverts the orbit and leaves the object.
13. Hotkeys: every chord in 2.12 (61 by our count) plus B is bound or reserved (see the status note in 2.12) with zero conflicts per context; W, E, R, Q map to move, rotate, scale, select; Alt+W twice returns tile
    rects identical to the start.
14. Layout: 1360 x 760 px area with gutter 2: four tiles tile exactly (sum of areas = area minus gutters); maximise then restore gives identical rects; right-click activation
    leaves the selection set unchanged, left-click on empty space clears it.
15. Graph layout: chain of 9 stages gives ranks 0-8 and 0 crossings; diamond A->B, A->C, B->D, C->D gives ranks 0, 1, 1, 2 and 0 crossings; K2,2 between two ranks gives
    exactly 1 crossing.
16. Scene stats: cube 100 mm: 8 vertices, 12 triangles, volume 1e6 mm3. UV sphere, 128 segments x 64 rings: 8066 vertices, 16128 triangles, Euler V - E + F = 2.
17. Theme (WCAG ratios on surface #262626, computed with the formula in 3.12): text #E6E6E6 = 12.13; #B8B8B8 = 7.63; accent #F2C14E = 9.02; pass #57C26B = 6.73; fail #E5534B =
    4.09 (allowed as non-text only, flagged as text); disabled #8A8A8A = 4.38 (flagged below 4.5, allowed only for disabled).
18. Gate summary: heat margin records from the slicing spec: ASA 100 C vs T_s 104.7 C = -4.7 K fail; PET-CF 205 C = +100.3 K pass; summary worst = fail, counts 1 pass 1 fail.
    Density ratio rows 0.920 and 0.896 pass the "within 12% of standard" demo gate.

## 6. Sources

Read in full or in part (Autodesk help pages, paraphrased, no text copied):
- https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-F8326C68-F2F9-47F7-AC1D-BA41D7825C7C.htm (window regions); https://help.autodesk.com/cloudhelp/2015/ENU/3DSMax/files/GUID-212A9477-E69E-4174-BB6F-1B7FD97A4281.htm (interface overview, M for material editor)
- https://help.autodesk.com/cloudhelp/2026/ENU/3DSMax-Basics/files/GUID-A73E1B09-7BFE-4A22-8153-1D3D2237B8E9.htm (default shortcuts); https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-D65DE5FD-F859-4F66-9E14-F9A5C1016411.htm (zoom, pan, orbit)
- https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-010B3CF4-0D7E-4886-8C65-BF332313D423.htm (navigation controls); https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-5B06CB73-DEA7-465E-9C6F-B8E7D454A7FB.htm (walk-through)
- https://help.autodesk.com/cloudhelp/2026/ENU/3DSMax-Basics/files/GUID-80209C3A-C2E4-4541-8738-D1E5ECE16E9C.htm (modifier stack controls); https://help.autodesk.com/cloudhelp/2016/ENU/3DSMax/files/GUID-55F70CB0-0590-4F1E-9879-EF070AF11CE6.htm (home grid tab)
- https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-BE30312F-5765-4BE1-9890-8CD5CAA4EFE5.htm (viewport label menus); https://help.autodesk.com/cloudhelp/2015/ENU/3DSMax/files/GUID-B39C0590-058C-4E59-B03D-AEC52DE830AB.htm (viewport controls: active border, activation, maximise)
- https://help.autodesk.com/cloudhelp/2016/ENU/3DSMax/files/GUID-A62CEC88-5390-4CAF-97BB-C7D07EBC6F65.htm (status bar controls); https://help.autodesk.com/cloudhelp/2016/ENU/3DSMax/files/GUID-86F256C7-EC0B-4731-A2FB-B6FF85991929.htm (create panel)
- https://help.autodesk.com/cloudhelp/2025/ENU/3DSMax-Basics/files/GUID-E3CB809D-94ED-4C30-892B-1D12B8721EA5.htm (command panel tabs); https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-2569461E-C859-4D54-BAFF-C8BD078B53AC.htm (scene explorer)
- https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-842CD36E-9E73-4733-9BAA-C7876013CA72.htm (schematic view); https://help.autodesk.com/cloudhelp/2025/ENU/3DSMax-Basics/files/GUID-5E421428-3284-4D92-9E37-F55D88BB9B10.htm (ViewCube overview)
- https://help.autodesk.com/cloudhelp/2019/ENU/3DSMax-MAXScript/files/GUID-E7C0F9E4-7FE6-4796-8CF1-52FC69A5CC67.htm (ViewCube properties); https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-9473CAF3-AF73-4127-A98C-58ACEF01ACAC.htm (time configuration)
- https://help.autodesk.com/cloudhelp/2023/ENU/3DSMax-Basics/files/GUID-A612A8B5-5F68-4B8F-9A4C-4E60A746E1CB.htm (time slider); https://help.autodesk.com/cloudhelp/2015/ENU/3DSMax/files/GUID-B9779BBF-5B04-44EF-95EC-465DD7267707.htm (transform type-in, offset mode, F12)
- https://www.w3.org/TR/WCAG21/ (contrast thresholds 1.4.3 and 1.4.11 read; luminance definition not reached); https://github.com/erich666/GraphicsGems/tree/master/gems (listing shows BoundSphere.c; algorithm not opened)
- https://graphicsinterface.org/wp-content/uploads/gi1992-18.pdf (arcball; abstract level); https://en-cg-web.coecis.cornell.edu/pubs/1997/MT97.html (Moller-Trumbore citation page)
- https://web.eecs.utk.edu/~bvanderz/cs461/dotguide.pdf (dot guide, cites the 1993 layout paper)

Read in a later pass (upgraded from summary to doc):
- https://help.autodesk.com/cloudhelp/2024/ENU/3DSMax-Basics/files/GUID-A55E4702-263E-4768-9964-5866698784DA.htm (track bar, key colours, mini curve editor); https://help.autodesk.com/cloudhelp/2024/ENU/3DSMax-Basics/files/GUID-6ED10DFF-77FC-42D1-884D-23EDF7C131FC.htm (utilities panel); https://help.autodesk.com/cloudhelp/2026/ENU/3DSMax-Customizing/files/GUID-E625E116-6070-4A02-9A31-672BA6A0A954.htm (rollout reorder and persistence)

Seen only as search-result summaries (treat as unverified):
- https://help.autodesk.com/cloudhelp/2027/ENU/3DSMax-Tutorial/files/GUID-F9386A18-B2DE-4971-ADD2-6D22416E6F3B.htm (navigation circle, Home icon); https://help.autodesk.com/cloudhelp/2017/ENU/3DSMax/files/GUID-A0647085-9080-4741-96F9-C711E9C1CA1E.htm (scene explorer menus)
- https://webaim.org/articles/contrast/ (contrast explainer); https://doi.org/10.1145/368996.369025 (Kahn 1962, cited from memory, publisher blocks scripted access)

Internal: CLEAN_ROOM.md; `3D printed body kit toolchain.md` (rho, q, CdA numbers, verified in lines 70-72); docs/specs/slicing-printing.md (cut and heat-gate numbers, line
332).

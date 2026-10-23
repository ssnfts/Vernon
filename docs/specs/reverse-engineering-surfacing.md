# Spec: reverse-engineering surfacing (stage 3, mesh to CAD surface)

Clean-room spec. Written from public vendor help/marketing pages, retailer feature tables, and published papers only (see section 6). Paraphrased; no vendor text, icons or binaries consulted. Tool names appear here for documentation and in `NodeTypeInfo.emulates` only, never in UI strings.

Provenance tags: **[P]** public statement by a vendor or reseller; **[C]** community/anecdotal; **[A]** published algorithm; **[I]** inference or Vernon proposal (needs golden-test confirmation against a licensed copy before it is called "matching").

## 1. Scope and reference tools

Stage 3 takes the aligned, watertight reference mesh from stage 2 (mm) and produces (a) editable surface geometry (analytic faces plus NURBS patches), (b) a STEP hand-off for CFD/FEA/print stages, and (c) a deviation report that stage 4+ gates on. Out of scope here: scan alignment and hole filling (stage 2), unroll/flatten (stage 7b; QUICKSURFACE "roll/unroll" is cross-referenced only), full parametric feature history.

| Tool | Version / tier observed | Role in this spec |
|---|---|---|
| QUICKSURFACE | 2026 release; Lite (subscription only), Pro (perpetual or 1-yr subscription from 1 Jan 2026), SOLIDWORKS add-in; Parasolid-kernel based [P] | Freeform + auto + hybrid surfacing, primitives, live deviation, shell/thicken |
| Revo Design | Revopoint's bundled edition built on the same engine [P] | Edition gating only. Vendor pages disagree whether it equals Lite or a trimmed Pro, so treat as "Lite plus hybrid" [P, conflicting] |
| Geomagic Design X | 2022 release sheet; tiers Go / Plus / Pro (Pro quote-only) [P] | Regions, Auto Surface, Exact Surfacing, mesh sketch, accuracy analyser |
| Fusion (Design, Mesh tab + Sketch) | Current help pages; Organic needs the Design extension [P] | Convert Mesh (faceted/prismatic/organic), mesh section sketch, Fit Curves to Mesh Section |
| Plasticity | 2026.1, Studio tier only [P] | PolySplines: mesh to G2 NURBS |

**Edition gating is not copied.** Vernon exposes the union of features; editions are a vendor pricing choice. The sources even conflict on gating (a March 2026 reseller video says both QUICKSURFACE editions have freeform and auto-surfacing, while the toolchain report says Lite is manual-only with no live deviation). Record the conflict, build the union, and gate nothing.

## 2. Feature catalogue (observable behaviour)

Defaults marked "[I]" are Vernon proposals; the vendors publish almost no numeric defaults.

### 2.1 QUICKSURFACE-class engine (Lite / Pro / Revo Design)
| Feature | Observable behaviour | Inputs | Outputs | Parameters and limits |
|---|---|---|---|---|
| Mesh handling | 64-bit app; built-in polygon reduction for big meshes [P] | STL/OBJ/PLY mesh | Mesh | ~100 M triangles stated; SOLIDWORKS add-in 80-100 M on strong hardware [P] |
| Mesh alignment | Both editions align a scan to a reference/coordinate frame [P] | Mesh, optional reference | Transform | Algorithm unnamed |
| Freeform, manual | User lays curves/patches that snap to the mesh; surface follows mesh within a live colour deviation map [P] | Mesh, user curves | NURBS patches | Tolerance control is Pro [P]; no numbers public |
| Auto surfacing | One-click patch layout and fit; Pro uses "a more advanced algorithm" for a closer fit [P] | Mesh | Patch network | Detail/tolerance sliders implied [I] |
| Hybrid modelling | Mix prismatic (analytic) and freeform surfaces in one body; strongest in Pro [P] | Regions + patches | Solid/shell | none public |
| Primitive extraction | Planes, cylinders, holes pulled from mesh; Pro has automatic mesh segmentation and faster extraction [P] | Mesh | Analytic faces | none public |
| Sketch assist, roll/unroll | Pro: auto-assisted sketch on mesh; flatten complex shapes to remodel [P] | Mesh | Sketch / flattened mesh | none public |
| Edit ops | Loft, sweep, shell, thicken, draft, move face, fillet/chamfer, patterns incl. linear/circular/mirror, trim, fill surface, booleans. Reseller table marks fill, trim, booleans and patterns as Pro [P] | Surfaces/solids | Surfaces/solids | thickness mm, offset mm |
| Live deviation | Real-time signed-distance colour map between surface and mesh while editing [P] | Surface + mesh | Colour map, statistics | range typically symmetric +-tol [I] |
| Exchange | STEP and IGES out; STEP/IGES import is Pro; Parasolid export per toolchain report (price page) [P] | Solid/surface | File | n/a |

### 2.2 Geomagic Design X-class (regions then surfaces)
| Feature | Observable behaviour | Parameters (public) |
|---|---|---|
| Auto Segment | Splits the mesh into regions typed planar, cylindrical, other analytic, or freeform. A reseller table puts automatic regions in the top tier and manual region tools in all tiers [P] | **Sensitivity**: a practitioner recommends about 50-60 to avoid fragmenting, accepting some under-segmentation; scale presumably 0-100 [C, I] |
| Resegment | Re-runs segmentation on chosen regions with higher sensitivity, e.g. recovering a plane that was labelled freeform [C] | per-region sensitivity |
| Split | Draw a line from one region edge to another to cut a region in two [C] | polyline on mesh |
| CAD Tessellation Mesh | 2022 option: a different segmentation path for meshes that came from tessellated CAD (clean facets, exact planes/cylinders) [P] | boolean |
| Mesh Fit / primitives | Create planes, cylinders etc. by picking regions; sketches can start from a planar region [C] | region ids |
| Auto Surface | Builds a patch network over freeform regions and fits NURBS patches; "Selective Surfacing" (choose which regions) is Plus/Pro; the network can be kept and reused for manual patches (2022) [P] | patch network, detail level (name from memory of UI, unverified) |
| Exact Surfacing | Pro-only, user-controlled curve network and patch layout for tighter fit; Loft gained connector definitions and tangency form options in 2022 [P] | curve network, connectors, tangency |
| Mesh sketch | 2D mesh sketch in all tiers, 3D in Plus/Pro; sketch entities are fitted to the section outline of the mesh; "replace target mesh" can re-point a sketch at an updated scan [P] | plane, entity fit tolerance |
| Accuracy Analyzer | Deviation colour map linked live to modelling operations; basic deviation in all tiers, detailed analyser in Plus/Pro; reference mesh comparison added in 2022 [P] | tolerance range, colour bands |
| Pricing context | Go $1,900/yr (basic Auto Surface), Plus $4,190/yr (selective surfacing), Pro quote [toolchain report] | n/a |

### 2.3 Fusion mesh workflow
| Feature | Observable behaviour | Parameters |
|---|---|---|
| Convert Mesh: Faceted | One output face per mesh face [P] | none |
| Convert Mesh: Prismatic | Groups of mesh faces that form prismatic features collapse to single faces; relies on mesh face groups, so it suits meshes made from CAD; helper commands to generate/combine face groups [P] | face-group ids |
| Convert Mesh: Organic | Needs the design extension; produces a T-spline body that cannot be tweaked mid-conversion; failure workaround is Base Feature instead of Parametric [P] | **By Accuracy**: Low / Medium / High / Precise (speed falls with accuracy). **By Face Number**: exact face count. **Preprocess Holes**: smooth open hole edges first |
| Output rule | Watertight mesh gives a solid; anything else gives a surface body (stitch/patch afterwards) [P] | Operation: Parametric (editable timeline feature) or Base Feature |
| Mesh section sketch | A plane cuts the mesh; the sketch stores the cut outline (polylines) as reference [P; plane/offset options not retrievable, [I] plane + offset] | plane, offset mm |
| Fit Curves to Mesh Section | Open types line/arc/spline, closed types closed-spline/circle/ellipse. Open: choose **Path Direction** (Short or Long path) and two points on the section; optional **Match Tangent** at each end to adjacent geometry. Closed: pick the section line [P] | **Spline Fit Tolerance** (lower = tighter fit, more control points); **Max Curve Deviation** (cap on distance from the section); adds no constraints other than tangent, so user adds constraints afterwards [P] |
| Form / T-splines | Subdivision-style cage modelling; Organic conversion lands here [P] | n/a (modelling environment) |

### 2.4 Plasticity PolySplines
Converts a polygon mesh into editable NURBS with G2 continuity, producing fewer, cleaner patches than a Catmull-Clark conversion [P]. Output reported as single-span and multi-span surfaces; a forum user states degrees 3 and 6 [C]. Dense quad meshes convert in roughly 5-10 minutes in one user's tests [C]. No public parameter list [P]. A forum claim says the technology was licensed from a third party and relates to University of Florida SurfLab research [C]. Studio tier ($299 perpetual per toolchain report).

## 3. Algorithms from published methods

Pipeline model used by Vernon: `mesh -> regions -> {analytic primitives | freeform patches} -> boundary curves -> patch network -> NURBS -> sew -> deviation -> STEP`. Each step has a published method below. Units mm, angles degrees, all fits in double precision.

### 3.1 Curvature and region segmentation
1. **Vertex normals and curvature** [A: normal-cycle / per-vertex fundamental-form estimators; background only]. Use area-weighted normals; principal curvatures kappa1, kappa2 per vertex from a one-ring least-squares fit. Smooth with 2-3 passes of one-ring averaging before thresholding.
2. **Variational shape approximation** [A: Cohen-Steiner, Alliez, Desbrun 2004]. Proxy = (point, unit normal). Error of triangle T against proxy i is `E = area(T) * |n_T - n_i|^2` (the L2,1 metric). Lloyd loop: (a) seed k triangles; (b) priority-queue flood: always pop the (triangle, region) pair of lowest cost and label it; (c) update each proxy normal as the area-weighted mean normal of its region, renormalised; (d) re-seed each region at its minimum-cost triangle; repeat until relative energy change < 1e-3 or 30 iterations. Use it to over-segment to k0 = 4 * expected regions, then merge (3.1.4). Cheap, deterministic given the seeds.
3. **Primitive detection** [A: Schnabel, Wahl, Klein 2007, efficient RANSAC]. Candidate shapes plane, sphere, cylinder, cone, torus from minimal samples of oriented points drawn inside one octree cell (plane 3 points, sphere 2, cylinder 2, cone 3, torus 3). A candidate scores by points within distance epsilon whose normal differs by under alpha, in one connected component. Stop when the chance of a larger missed shape falls under p_t. Defaults to start from (taken from open-source implementation docs, verify when implementing): p_t = 0.01, min support 1 % of points, epsilon = 1 % of bounding-box diagonal for raw scans (use 0.05 mm on CAD-quality meshes), cos(alpha) = 0.9 (about 26 degrees), cluster epsilon 1 % of diagonal.
4. **Region merge and sensitivity map** [I]. Merge adjacent VSA regions when their best-fit shared primitive keeps RMS residual under tau and the normal jump along the shared boundary is under theta. Proposed sensitivity s in 0..100 maps linearly: theta from 35 degrees at s = 0 to 3 degrees at s = 100, tau = 4 * (scan noise sigma) at s = 0 down to 1 * sigma at s = 100. s = 55 gives theta = 17.4 degrees. A "CAD tessellation" flag instead segments by exact coplanarity (angle < 0.5 degrees between adjacent triangle normals) and exact co-cylindricity because the facets lie exactly on the surfaces.
5. **Region classification**: fit all primitive types by least squares and pick the lowest-RMS type that is within tau and has at least 3x lower residual than freeform; otherwise `freeform`. Plane: PCA smallest eigenvector. Sphere: algebraic fit then Gauss-Newton. Cylinder: axis = smallest-eigenvalue direction of the normal covariance, then circle fit (Taubin or Pratt) on the points projected to the axis plane. Cone/torus: Gauss-Newton on signed point-to-surface distance, initialised from RANSAC. A **hole** is a cylinder region whose normals point at the axis (concave) and whose radius is under a user cap (proposed 30 mm).

### 3.2 Boundaries and the patch network
Region adjacency graph from triangle edges whose two labels differ; chain edge segments between junction vertices (3+ labels) into boundary polylines. For two analytic regions replace the polyline with the exact intersection of the two primitives (plane-plane line, plane-cylinder circle/ellipse/lines) then trim to the polyline extent; otherwise fit a cubic B-spline by least squares with centripetal parameterisation and knot placement by averaging of the parameters (NURBS Book method) until max error < tol (proposed 0.5 x global tolerance). Snap junction positions so every shared boundary is one curve used by both faces (watertight network).

### 3.3 Freeform patch fitting
1. **Parameterise** the patch's triangles to the unit square. Four-sided patch: boundary-constrained mapping (chord-length on the boundary, Tutte or mean-value weights inside) [A: Floater mean-value coordinates, background]. Free-boundary regions: least-squares conformal maps [A: Levy et al. 2002], pin two boundary vertices, then rescale to the square.
2. **Least-squares tensor B-spline**: minimise `E(P) = sum_i w_i |S(u_i,v_i) - Q_i|^2 + lambda * (|S_uu|^2 + 2|S_uv|^2 + |S_vv|^2)` integrated over the patch (thin-plate fairness). With collocation matrix N (sparse, (p+1)^2 nonzeros per row) the solution solves `(N^T W N + lambda F) P = N^T W Q`; conjugate gradient with Jacobi preconditioner, or sparse Cholesky. Degree 3, clamped uniform knots, start 6x6 control points.
3. **Parameter correction** [A]: replace each (u_i, v_i) by the foot point of Q_i on the current S via Newton on `(S-Q).S_u = 0, (S-Q).S_v = 0`, re-solve; 5-10 passes. Convergence is faster with squared-distance minimisation error terms [A: Wang, Pottmann, Liu 2006 give the curve form; surface form is analogous].
4. **Adaptive refinement** [A: hierarchical fitting after Eck and Hoppe 1996 / Krishnamurthy and Levoy 1996]: where residual exceeds tol, insert knots in the offending span (Boehm insertion keeps the shape), refit; stop at tol or max 48x48 control points.
5. **Continuity**: independent fits meet with small gaps and kinks, a known weakness of patch-wise fitting [A: Eck and Hoppe note G1 is hard to enforce while fitting]. Vernon pass: (a) share boundary control rows exactly; (b) G1 by solving the fit with an equality constraint that the first interior row stays coplanar with the shared boundary tangents (Lagrange multipliers); (c) report max G1 angle across seams. True G2 from section 3.5.

### 3.4 Quad-dominant remesh and SubD to NURBS
1. **Quad remesh** [A: Jakob et al. 2015 (Instant Meshes); Huang et al. 2018 (QuadriFlow)]: smooth a 4-direction orientation field, then a 4-position field, extract the quad mesh. QuadriFlow trades speed for roughly four times fewer singularities and takes a manifold triangle mesh plus a target face count. Closed genus-0 all-quad meshes satisfy `sum over vertices (4 - valence) = 8` (use as a checkable invariant; 0 for genus 1).
2. **Quad cage to patches**: every quad whose four corners are valence 4 is exactly a uniform bicubic B-spline patch of the Catmull-Clark limit surface (control net = the 4x4 one-ring). For quads with an extraordinary vertex use bicubic approximation of the limit surface [A: Loop and Schaefer 2008 approximate Catmull-Clark, one geometry patch per quad, G0 with small tangent gaps]. Option `fit_cage`: iterate cage vertex positions so the limit surface minimises the squared distance to the mesh (same SDM loop as 3.3.3).
3. **T-splines** [A: Sederberg et al. 2003]: needed only to represent Fusion's organic (T-spline) output natively; Vernon exports a refined bicubic patch set instead.

### 3.5 G2 mesh to NURBS (PolySplines-class) [white-box first]
Public research for the same problem: bi-3 and bi-6 G2 multi-sided constructions with refinement and polyhedral splines over quad nets with star configurations and T-junctions (Karciauskas, Peters, Lo and others, SurfLab, University of Florida). Implementer outline [A, I]: (1) input quad-dominant mesh; (2) classify each vertex regular (valence 4, no joint) or extraordinary; (3) around regular regions emit bi-3 B-spline patches directly; (4) cap each extraordinary vertex with a ring of bi-3/bi-6 patches from the published constructions so that adjacent patches meet with matching curvature (G2); (5) verify by sampling the principal curvature jump across each seam (target < 1 % relative). Needs a careful read of the 2020 and 2024 SurfLab papers before coding; node is stubbed until then.

### 3.6 Mesh section sketch and curve fitting
1. **Section**: for plane (n, d) compute signed distance of every vertex; classify with epsilon 1e-9 x bbox diagonal so vertices on the plane are treated as +side (consistent rule avoids double hits). For each triangle with mixed signs, intersect two edges, producing a segment; key segments by shared mesh-edge ids and chain them into open or closed polylines (hash map, O(T)). Closed when the chain returns to its start edge.
2. **Split into spans**: corner detection by turning angle > 25 degrees (proposed) after Douglas-Peucker with tol/4; then classify each span.
3. **Line** by total least squares; **circle/arc** by Kåsa or Taubin algebraic fit then geometric refinement; **ellipse** by direct least squares; accept the simplest type whose max deviation <= Max Curve Deviation.
4. **Spline** with Spline Fit Tolerance: least-squares B-spline approximation with error bound, adding knots at the worst-residual parameter until the max error <= tolerance (NURBS Book curve approximation). Monotone behaviour required: smaller tolerance never yields fewer control points.
5. **Path direction**: for a closed section and two picked points the two arcs of the loop are candidates; Short = smaller arc length, Long = larger.
6. **Match tangent**: add endpoint tangent equality as a constraint in the LSQ system (rows with large weight, or elimination).

### 3.7 Faceted and prismatic conversion
Faceted: weld vertices (tolerance 1e-6 mm), one planar face per polygon, shared edges, orient consistently; closed 2-manifold gives a solid (Euler: V - E + F = 2 for genus 0), otherwise a shell. Prismatic: use the region/face-group id as the grouping key; for each group with a fitted analytic surface within tolerance, emit one analytic face bounded by the boundary loops (including inner loops for holes); groups without a good fit stay faceted. Needs a B-rep with analytic surfaces and trimming; STEP can carry them (advanced faces), but a kernel must do the loop classification.

### 3.8 Deviation (live) and statistics
Published method: sampled symmetric surface-to-surface distance, report mean/max/RMS and a colour map [A: Cignoni, Rocchini, Scopigno 1998 (Metro)]. Vernon: tessellate the surface adaptively (chord error <= tol/10), build a BVH over triangles, compute exact closest point on triangle for every mesh vertex plus area-weighted random samples (>= 200 000), sign by the surface normal at the foot point (positive = mesh outside surface). "Live" = tile the surface in patch-sized dirty cells and recompute only cells whose control points changed. Statistics: mean, RMS, std, max |d|, p50/p95/p99 of |d|, and percent within +-tol bands; histogram 64 bins over +-2 x tol. Report always includes the sample count.

### 3.9 Mirror, shell/thicken, sew
- **Mirror**: reflect control points across the plane (reverse parameter order to keep outward normals). Seam rule: for patches whose boundary lies on the plane, project that boundary control row onto the plane and symmetrise the adjacent row (P1 := midpoint of P1 and its reflection) so tangent is plane-perpendicular and the seam is G1.
- **Thicken/shell**: offset surfaces are not NURBS in general, so sample S + d n on a (3 x control-grid) lattice and refit within tol (3.3), then add side walls by ruled/loft between old and new boundary curves; watch for local offset self-intersection when d exceeds the minimum radius of curvature. Reject and warn when d > 0.8 x min radius.
- **Sew**: merge boundary edges closer than sew tolerance (proposed 0.05 mm; scan RMS-scale) by matching endpoints then comparing mid-curve samples; closed result = solid flag.

## 4. Proposed Vernon nodes

All ids `surface.<snake>`; stage `Stage::Surface`; spec = `docs/specs/reverse-engineering-surfacing.md`. Port kinds follow `DataKind`. Because `Geometry` has no NURBS yet, patch sets travel as `Json` (`nurbs_patch_set` schema: patches with degree, knots, control grid, boundary ids, plus analytic faces) alongside a tessellated `Geometry` preview that carries a `patch_id` primitive attribute. "Now" = implement fully; "WB" = white-box stub first (correct ports/params, placeholder algorithm).

| id | Generic label | Emulates (docs only) | Inputs | Outputs | Key params (default, range, unit) | Status |
|---|---|---|---|---|---|---|
| surface.segment_regions | Segment Regions | Design X Auto Segment; QUICKSURFACE auto segmentation | Geometry (mesh) | Geometry (`region_id`, `region_type` attribs), Report | sensitivity 55 [0,100]; cad_tessellation false; min_region_area_pct 0.05 [0,10]; types {plane,sphere,cylinder,cone,torus,freeform} | Now (VSA + merge); RANSAC mode later |
| surface.refine_regions | Refine Regions | Design X Resegment/Split/Merge | Geometry w/ regions | Geometry w/ regions | op {resegment,merge,split_by_polyline}; group; sensitivity 80 [0,100] | Now (resegment, merge); split WB |
| surface.fit_primitives | Fit Primitives | primitive extraction in RE tools | Geometry w/ regions | Json (analytic faces), Report | tol_mm 0.05 [0.001,5]; max_hole_radius_mm 30 [0.1,500]; extend_to_neighbors true | Now |
| surface.section_curves | Mesh Section Curves | Fusion mesh section sketch; Design X mesh sketch | Geometry (mesh) | Geometry (polylines, `section_id`) | mode {plane,planes_along_axis}; origin; normal; count 1 [1,500]; spacing_mm 25 [0.1,1000] | Now |
| surface.fit_section_curves | Fit Section Curves | Fusion Fit Curves to Mesh Section | Geometry (polylines) | Json (lines/arcs/splines), Geometry preview, Report | curve_type {auto,line,arc,spline,closed_spline,circle,ellipse}; path {short,long}; spline_fit_tol_mm 0.05 [0.001,5]; max_deviation_mm 0.1 [0.001,10]; match_tangent_start/end false | Now (all but ellipse); ellipse WB |
| surface.remesh_quad | Quad Remesh | Instant-meshes class remeshers; Rhino QuadRemesh-style | Geometry (mesh) | Geometry (quad-dominant) | target_faces 2000 [100,200000]; crease_angle_deg 40 [0,180]; symmetry {none,x,y,z}; curvature_adaptive 0.5 [0,1] | WB first, then drive an open-source remesher as a separate process or port from the paper (check licence) |
| surface.subd_to_nurbs | Cage to Surface Patches | SubD-to-NURBS conversion; Form/T-spline output | Geometry (quad cage) | Json (patch set), Geometry preview | extraordinary {approx_bicubic,gregory,g2_polyspline}; refine_levels 0 [0,3]; fit_cage false; max_deviation_mm 0.5 | Now (regular + approx_bicubic); g2 WB |
| surface.fit_patch | Fit Freeform Patch | freeform patch fit (QUICKSURFACE/Design X) | Geometry w/ regions or patch outline | Json (patch), Report | degree 3 [2,5]; init_cp 6 [4,32]; max_cp 48 [8,128]; smoothness 1e-3 [0,1]; tol_mm 0.1 [0.001,5]; param {boundary,conformal} | Now |
| surface.curve_network_patch | Curve Network Patches | Design X Exact Surfacing; manual freeform | Json (curves), Geometry (mesh) | Json (patch set), Report | tangency {g0,g1}; grid_density 8 [4,32]; tol_mm 0.1 | WB first |
| surface.auto_surface | Auto Surface | QUICKSURFACE auto surfacing; Design X Auto Surface | Geometry (mesh) | Json (patch set), Geometry preview, Report | detail {coarse,medium,fine} or target_patches 0=auto; tol_mm 0.3 [0.01,5]; continuity {g0,g1}; selective_regions group | WB first, composes segment+boundaries+fit_patch |
| surface.mesh_to_g2_surface | G2 Mesh to Surface | Plasticity PolySplines | Geometry (quad-dominant mesh) | Json (patch set), Report | continuity {g1,g2}; max_degree 6 [3,6]; max_deviation_mm 0.3 | WB first (research-grade) |
| surface.convert_faceted | Faceted Solid | Fusion Convert Mesh Faceted | Geometry (mesh) | File (STEP) or Json brep, Report | weld_tol_mm 1e-6; quads_as_faces true | Now |
| surface.convert_prismatic | Prismatic Solid | Fusion Convert Mesh Prismatic | Geometry w/ regions, Json (analytic faces) | Json (brep), Report | tol_mm 0.05; keep_unfit_as_facets true | WB first (needs trimmed analytic faces) |
| surface.sew | Sew Surfaces | stitch/knit in CAD | Json (patch set) | Json (sewn), Report | tol_mm 0.05 [0.001,1]; make_solid auto | Now |
| surface.mirror_surface | Mirror Surface | pattern/mirror in RE tools | Json (patch set) | Json (patch set) | plane {x,y,z,custom}; origin; merge_seam true; symmetrize_seam true | Now |
| surface.thicken_shell | Thicken Shell | shell/thicken | Json (patch set) | Json (solid), Report | thickness_mm 3 [0.01,100]; side {outward,inward,both}; refit_tol_mm 0.05 | WB first |
| surface.deviation_check | Surface Deviation | live deviation / accuracy analyser | Geometry (mesh), Json (patch set) or Geometry (surface tessellation) | Geometry (signed `dev_mm` point attrib), Report | tol_mm 0.5 [0.001,50]; samples 200000 [1000,5e7]; sign {outside_positive}; p_gate 95 [50,100] | Now |
| surface.write_step | Write STEP | STEP export of surfaces/solids | Json (patch set/brep) | File (.stp) | schema {ap214,ap242}; units mm; trimmed false | Now untrimmed; trimmed WB |

**Proposed count: 18 nodes.** Report keys emitted for the next stage to check: `dev_mean_mm`, `dev_rms_mm`, `dev_max_mm`, `dev_p95_mm`, `dev_p99_mm`, `pct_within_tol`, `patch_count`, `region_count`, `g1_max_deg`, `watertight` (0/1). `surface.deviation_check` attaches `Metric::with_limit(0.5, lower_is_better)` to `dev_p95_mm`, matching the toolchain report's "95 % within +-0.5 mm" phrasing.

## 5. Golden test ideas (concrete numbers)

Reference-tool comparisons need a licensed copy and inputs made by us; the cases below are analytic and need no vendor tool. Numbers marked (r) come from the toolchain report.

1. **Cylinder regions**: r = 25 mm, h = 100 mm, 112 segments (sagitta 0.0098 mm), caps fan-triangulated. `segment_regions` at s = 55 must give exactly 3 regions (2 plane, 1 cylinder). Fitted radius 25 +- 0.01 mm, axis error < 0.05 degrees.
2. **Filleted box**: 40 mm cube, 5 mm edge fillets, 8 spherical corners (r = 5): expect 6 plane + 12 cylinder + 8 sphere = 26 regions at s = 55; with `cad_tessellation` true the count is the same and radii are exact to 1e-6 mm.
3. **Through-hole**: 64-segment hole r = 5 mm (sagitta 0.0060 mm) in a plate: one `hole` detected, radius 5.00 +- 0.01, axis normal to plate within 0.1 degrees.
4. **Icosphere**: R = 50 mm, 4 subdivisions (2 562 vertices, 5 120 triangles, edge ~3.3 mm, sagitta ~0.03 mm): sphere fit radius 50 +- 0.001 mm on vertices.
5. **Section + fit**: plane z = 0 through that sphere gives one closed loop of radius 50 +- 0.03; `fit_section_curves` closed circle residual RMS < 0.01 mm; with curve_type spline, control-point counts satisfy n(tol 0.01) >= n(0.05) >= n(0.2) and each max deviation <= its tol. Arc test: 90-degree arc, r = 30 mm, Gaussian noise sigma 0.02 mm: recovered r = 30 +- 0.02, max deviation <= 0.1.
6. **Freeform patch**: 100 x 100 mm sheet `z = 5 sin(pi x / 50)` sampled 100 x 100 (19 602 triangles), 8 x 8 cubic control grid: max deviation <= 0.05 mm, p95 <= 0.02 mm. Add noise sigma = 0.05 mm: fit RMS must land 0.04-0.06 (fit smooths rather than chases noise) and p95 <= 0.12.
7. **Deviation statistics**: plane z = 0 versus mesh offset +0.3 mm (outside = positive): mean = +0.300 +- 1e-6, p95 = 0.300, `pct_within` is 100 % at tol 0.5 and 0 % at tol 0.25. Sign flips when the mesh is moved inside.
8. **Drift budget (r)**: a linear scan drift of 0.05 mm/m over a 4.5 m body is 0.225 mm at the far end (report: ~0.23): mean |d| = 0.1125, max = 0.225 +- 0.002; with scanner noise 0.1 mm and surface tolerance 0.3 mm the root-sum-square budget is sqrt(0.1^2 + 0.225^2 + 0.3^2) = 0.39 mm, inside the 0.5 mm / 95 % gate, so `p_gate` passes. Same data with tolerance forced to 0.2 mm must produce a failing report note.
9. **Regular SubD**: 10 x 10 quad grid (all interior vertices valence 4): `subd_to_nurbs` reproduces the Catmull-Clark limit surface exactly, deviation < 1e-9 mm. Cube cage (8 vertices of valence 3) with `approx_bicubic`: deviation to the true limit surface <= 1 % of cage edge [I, unvalidated bound].
10. **Quad remesh invariant**: sphere R = 50 mm to target 1 000 faces: face count within +-10 %, >= 90 % quads, `sum(4 - valence)` over vertices = 8.
11. **Faceted/prismatic**: cube from 12 triangles gives 12 faces and `watertight = 1`; remove one triangle gives a shell, `watertight = 0`. 40 mm cube with one through hole r = 5 (face groups: 6 sides + 1 cylinder) prismatic gives 7 faces.
12. **Mirror/sew**: half panel mirrored across x = 0: sew gap after merge <= 1e-6 mm, G1 angle across the seam <= 0.5 degrees, watertight if the half was closed against the plane.
13. **Thicken**: full sphere R = 50 mm thickened 3 mm inward: inner radius 47 +- 0.05; volume 4/3 pi (50^3 - 47^3) = 88 706 mm^3 within 0.5 %.
14. **Fusion parity (when a licence is available)**: same 20 mm-spaced section of a known scan fitted with Spline Fit Tolerance 0.05 and 0.5 mm; compare control-point counts (expect same order, not equality) and max deviation within each tolerance.
15. **STEP round trip**: `write_step` of a 2-patch surface re-read by an open-source STEP reader (separate process) gives 2 faces, bounding box equal to input within 1e-4 mm.

### 5.1 Known gaps and open questions (resolve before claiming parity)
- No vendor publishes numeric defaults for segmentation sensitivity, auto-surface detail, or sew tolerance. The values in sections 3-4 are Vernon proposals; record the licensed tool's behaviour on tests 1-6 and retune the sensitivity map (3.1.4).
- The Fusion "create mesh section sketch" parameters (plane choice, offset, spacing) were not retrievable from public help; `surface.section_curves` params are inferred.
- Whether Revo Design equals Lite or a trimmed Pro, and whether Lite has live deviation, is contradicted across pages. Not needed for Vernon (no edition gating) but do not cite either claim as fact.
- PolySplines internals are unpublished; degree 3/6 and the licensing relation to the SurfLab work are forum claims. Build against the published SurfLab constructions, not against assumed behaviour.
- Plasticity, Design X and QUICKSURFACE are Windows-first or closed; any parity tests must use inputs we create and outputs we record ourselves (CLEAN_ROOM.md, process step 3).
- Geometry has no NURBS type yet. Decide between a new `Geometry` surface section and the proposed `Json` patch-set schema before the first fit node is merged.

## 5a. Node map: loft through sections

`surface.loft_sections` skins a surface through a stack of section curves (the loft of section 2). Sections are ordered
by section id, matched in start point and direction, refitted to a common knot vector (`control_points` per section)
and interpolated across the stack with a B-spline of degree `degree_v` (Piegl and Tiller, "The NURBS Book", skinning
chapter). `loop_id` selects which curve loop of the input is lofted. The patches are output as JSON and a preview mesh
tessellated to `preview_tol_mm`. The report gives the maximum and RMS distance from the surface to every input curve
against `tol_mm` (the fidelity check of section 5). Check: sections that are circles of equal radius at equal spacing
give a cylinder whose distance to every circle is below `tol_mm`.

## 6. Sources

Vendor/product pages (fetched or search-confirmed):
- QUICKSURFACE product overview (editions, limits, kernel, formats): https://www.quicksurface.com/
- QUICKSURFACE Lite vs Pro comparison video page (reseller, March 2026): https://video.trimech.com/quicksurface-software-comparison
- Revopoint QUICKSURFACE page (feature table, reached via search): https://revopoint3d.com/QuickSurface
- Revopoint Revo Design pages (relationship to QUICKSURFACE, search snippets): https://www.revopoint3d.com/pages/revodesign and https://forum.revopoint3d.com/t/curious-about-the-difference-between-revo-design-and-quicksurface-pro/39562
- Geomagic Design X 2022 release sheet (PDF): https://www.cadmicro.com/wp-content/uploads/Geomagic_Design_X_2022.pdf
- Geomagic Design X tier table (reseller): https://www.matterhackers.com/store/l/geomagic-design-x-software/sk/M3A6W011
- Geomagic Design X region workflow article: https://www.cati.com/?p=126341 (companion: https://www.cati.com/?p=125991)
- Fusion Convert Mesh: https://help.autodesk.com/cloudhelp/ENU/Fusion-Mesh/files/MESH-CONVERT-TO-SOLID.htm
- Fusion Fit Curves to Mesh Sections: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-SKETCH-CREATE-FIT-CURVE-MESH-SECTION.htm and https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-FIT-CURVE-TO-MESH-SECTION.htm (the "create mesh section sketch" page itself was not retrievable)
- Fusion API MeshConvertMethodTypes (listed in search; page returned 404): https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_MeshConvertMethodTypes.htm
- Plasticity 2026.1 coverage: https://digitalproduction.com/2026/04/21/plasticity-2026-1/ , https://cgchannel.com/2026/04/plasticity-2026-1-is-out/ , https://80.lv/articles/plasticity-2026-1-adds-new-commands-improves-old-ones , product page https://plasticity.xyz/product
- Community claims on PolySplines degree/licensing (anecdotal): https://forum.onshape.com/discussion/comment/127758 and https://discourse.mcneel.com/t/what-is-tonurbss-limit/219422?page=2
- Project toolchain report (prices, 0.23 mm drift, 95 %/+-0.5 mm phrasing): local file `3D printed body kit toolchain.md`

Papers and open-source docs (algorithms):
- Schnabel, Wahl, Klein, Efficient RANSAC for Point-Cloud Shape Detection, CGF 26(2), 2007: https://cg.cs.uni-bonn.de/backend/v1/files/publications/schnabel-2007-efficient.pdf (record https://diglib.eg.org/handle/10.2312/CGF.v26i2pp214-226 ); parameter defaults cross-checked in CGAL docs https://doc.cgal.org/latest/Shape_detection/ (read for parameter names only; no code used)
- Cohen-Steiner, Alliez, Desbrun, Variational Shape Approximation, SIGGRAPH 2004: https://people.engr.tamu.edu/schaefer/teaching/689_Fall2006/p905-cohen-steiner.pdf and https://dumas.ccsd.cnrs.fr/INRIA/inria-00070632v1
- Levy, Petitjean, Ray, Maillot, Least Squares Conformal Maps, SIGGRAPH 2002: https://people.engr.tamu.edu/schaefer/teaching/689_Fall2006/p362-levy.pdf
- Wang, Pottmann, Liu, Fitting B-spline Curves to Point Clouds by Curvature-Based Squared Distance Minimization, ACM TOG 25(2), 2006: https://www.geometrie.tuwien.ac.at/geom/ig/papers/tr143.pdf
- Eck and Hoppe, Automatic Reconstruction of B-Spline Surfaces of Arbitrary Topological Type, SIGGRAPH 1996: https://hhoppe.com/bspline.pdf
- Krishnamurthy and Levoy, Fitting Smooth Surfaces to Dense Polygon Meshes, SIGGRAPH 1996: https://www.graphics.stanford.edu/papers/surfacefitting/
- Jakob, Tarini, Panozzo, Sorkine-Hornung, Instant Field-Aligned Meshes, 2015: https://mitsuba-renderer.org/~wenzel/papers/instant-meshes-sga15.pdf
- Huang, Zhou, Niessner, Shewchuk, Guibas, QuadriFlow, CGF 37(5), 2018: https://stanford.edu/~jingweih/papers/quadriflow/
- Loop and Schaefer, Approximating Catmull-Clark Subdivision Surfaces with Bicubic Patches, 2008 (MSR-TR-2007-44): https://people.engr.tamu.edu/schaefer/research/acc.pdf
- Sederberg, Zheng, Bakenov, Nasri, T-splines and T-NURCCs, ACM TOG 22(3), 2003: https://archive.ymsc.tsinghua.edu.cn/pacm_download/53/505-jZ-SIGGRAPH03.pdf
- Cignoni, Rocchini, Scopigno, Metro: Measuring Error on Simplified Surfaces, CGF 17(2), 1998: https://diglib.eg.org/xmlui/handle/10.2312/8468 (tool page https://vcg.isti.cnr.it/vcglib/metro.html)
- UF SurfLab polyhedral/G2 spline papers (Karciauskas, Peters et al.): https://www.cise.ufl.edu/research/SurfLab/papers/20g2Ref.pdf , https://www.cise.ufl.edu/research/SurfLab/papers/24casc2.pdf , https://www.cise.ufl.edu/research/SurfLab/papers/09guiSpline_cagd.pdf
- Background, citation only, not fetched: Piegl and Tiller, The NURBS Book (Springer, 2nd ed., 1997), curve/surface approximation and knot placement chapters; Floater, Mean value coordinates (CAGD 2003); Taubin and Pratt algebraic circle fits.

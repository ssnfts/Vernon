# Spec: mesh wrap, quad remesh, subdivision surfaces, flattening (reference: Rhino 8 mesh tools)

Status: draft v0.1, 2026-10-21. Clean-room spec: public help pages, papers and open-source docs only (see CLEAN_ROOM.md).
Vernon stages served: 2 clean (watertight shell), 3 surface (mesh -> CAD surface), 7b flatten (pattern / wrap artwork).
Marker key used below: **(doc)** stated by the vendor help page, **(inf)** inferred from doc wording, **(V)** a Vernon design choice, not observed behaviour.
Source status tags (section 6): [F] fetched this session, [S] seen only in search results, [R] recalled from memory, verify before relying on it.

## 1. Scope and reference tools

| Reference command (Rhino 8) | Role in the body-kit pipeline | Help page (docs only) |
|---|---|---|
| ShrinkWrap | scan fragments / point clouds / broken meshes -> one closed shell, optional offset | commands/shrinkwrap.htm [F] |
| QuadRemesh | dense scan mesh -> clean quad mesh (or SubD) for surfacing, CFD, FEA | commands/quadremesh.htm [F] |
| ToSubD / SubD | quad cage <-> smooth subdivision surface, creases, corners, interpolation | commands/tosubd.htm [F] |
| ToNURBS (SubD input) | subdivision surface -> trim-free NURBS polysurface for STEP export | commands/tonurbs.htm [F] |
| Squish / SquishBack | flatten doubly curved panel, report stretch/compression, map markup back | commands/squish.htm [F], wiki labs/advancedflattening [F] |

All URLs are under `https://docs.mcneel.com/rhino/8/help/en-us/` except the wiki. The Rhino 9 ToNURBS page adds an OutputLayer option [S]; ignore it (layers are not a Vernon concept).
Out of scope: Rhino UI behaviour (preview, hide/delete inputs, mesh-wire display; in a node graph these are viewer settings, not node parameters), UnrollSrf/Smash (developable unroll, separate spec), mesh repair beyond hole filling.
Units: Vernon meshes are in millimetres; every length parameter below is mm unless stated. The vendor pages state no units for edge length (doc), so the unit is the document unit.

## 2. Feature catalogue (observable behaviour)

### 2.1 ShrinkWrap (watertight wrap)
Inputs (doc): any mix of NURBS surfaces, SubDs, meshes, point clouds, points. Output (doc): ONE closed mesh, or the command fails. Typical uses (doc): printing, merging objects into one solid, scan-fragment assembly, repair, offset shells.

| Parameter | Type / range | Default | Observed behaviour |
|---|---|---|---|
| Target edge length | float > 0, model units | automatic (rule not given) | approximate output edge length; real edges drift more as Polygon Optimization rises (doc) |
| Offset | float, signed | 0 (inf) | positive inflates and adds faces, negative shrinks and removes faces; little effect on edge length (doc) |
| Smoothing iterations | int >= 0, no upper limit | 0 (inf) | 0 = off; more iterations remove faces and deflate the mesh further (doc). Implies plain Laplacian smoothing, which shrinks (inf) |
| Polygon optimization | 0-100 % | 0 (inf) | adaptive vertex collapse; faces along hard edges are preserved (doc) |
| Inflate vertices and points | bool | on for points/clouds, off otherwise (doc) | builds the shell from each vertex/point; NURBS and SubD use their render-mesh vertices; overlapping inflated bits merge; negative offset pulls back toward the vertices (doc) |
| Fill holes in input | bool | not stated | closes input holes before wrapping; may also close intentional micro-gaps; no effect for points or when Inflate is on (doc) |
| Compute vertex colours | bool | not stated | copies mesh vertex colours / cloud display colours to the output (doc) |

No numeric limits are published. Failure modes: no closed mesh produced (doc); offset more negative than the local half-thickness collapses a thin wall (inf).

### 2.2 QuadRemesh
Inputs (doc): surfaces, meshes, SubDs; optional guide curves (must lie on the input), optional symmetry plane. Output (doc): quad mesh, or a SubD when Convert to SubD is on.

| Parameter | Type / range | Default | Observed behaviour |
|---|---|---|---|
| Sizing mode | exclusive: Target Edge Length / Target Quad Count | Quad Count (doc) | edge length is scale-dependent (count rises with object size); quad count is scale-independent. Both are approximate goals (doc) |
| Target edge length | float > 0 | off | fails if too large for the object (doc) |
| Target quad count | int > 0 | on | approximate face goal (doc) |
| Adaptive size | int 0-100 | not stated | low = fewer, more uniform quads; high = smaller quads in high curvature, more detail, more irregular topology; above ~30 control over total count weakens (doc) |
| Adaptive quad count | bool | checked (doc) | exact semantics not documented; treat as "let adaptivity change the total count" (inf) |
| Use surface edges | Off / Smart / Strict | not stated | polysurfaces and extrusions only: Off ignores sub-face borders, Smart keeps meaningful ones (recommended), Strict keeps all (doc) |
| Symmetry axis | X, Y, Z checkboxes, multi | none | mirrors across the bounding-box mid-plane; only valid for truly symmetric inputs (doc) |
| Guide curves + influence | None / Approximate / Edge Ring / Edge Loop | not stated | Approximate nudges flow weakly; Ring makes crossing edges perpendicular to the guide; Loop places an edge loop along the guide (strongest). Curves not on the surface have no effect (doc) |
| Detect hard edges | bool | on | adds hard-edge loops where adjacent faces meet above a 30 degree break angle (doc) |
| Convert to SubD | bool | off | emits a smoothed SubD instead of a mesh (= ToSubD) (doc) |
| SubD creases / corners | bool each | on / on | carry hard edges as creases and corner vertices as corners; need Convert to SubD (doc) |
| Interpolate SubD | bool | off | on: mesh vertices lie on the SubD (RE use); off: vertices become control points so the SubD is slightly smaller/larger (doc) |
| Preview readout | display | - | average edge length and face count (doc); Vernon exposes these as node outputs |

Not documented: quad/triangle mix of the result, quality metrics, maximum input size, exact mapping of Adaptive Size to size ratios. A forum report (Rhino 7 era) says SquishBack does not work on QuadRemesh output [S]; treat as a known-risk note, not a requirement.

### 2.3 ToSubD / SubD basics
Inputs (doc): mesh, extrusion, surface, sub-surface; trims are lost; polysurface sub-surfaces convert individually; non-manifold mesh edges and their faces are removed.

| Option | Values | Behaviour (doc) |
|---|---|---|
| UseMesh | ControlPoints / Location | mesh vertices become control points (surface does not pass through them) / become interpolated surface points |
| MeshCreases | bool | unwelded mesh edges become creased edges |
| MeshCorners | bool | for quad faces, boundary corner vertices become creased vs smooth |
| UseSurface | ControlPoints / Location | NURBS control points become SubD control points (use for Class-A joins) / SubD best-fits the surface, extra faces may appear |
| SurfaceCorners | bool | NURBS corners crease or stay smooth |
| DeleteInput | bool | graph nodes do not delete inputs; drop (V) |
| SubDivide (separate command) | iterations, whole object or selected faces | Catmull-Clark refinement |

### 2.4 ToNURBS (SubD -> polysurface)
| Option | Values | Behaviour |
|---|---|---|
| Faces (SubDOptions) | Packed / Unpacked | Packed merges each rectangular quad-face grid into one larger NURBS surface; Unpacked = one NURBS surface per quad face (doc) |
| ExtraordinaryVertex | G0 / G1 / G1x / G1xx / G2 | continuity of the polysurface at extraordinary vertices (valence != 4). The SubD is G2 there; a polysurface can only approximate it (doc). G0 = closest shape; G1 = tangent continuous, slightly larger deviation; G1x = G1 aiming at edge-normal deviation <= 1 degree; G1xx (new in 8) adds double knots on isocurves near edges and more control points for zebra-stripe smoothness; G2 = full G2 but can add bulges or dents (doc) |
| ShowFacePacks / ForceUpdateFacePacks | display / action | colour or recompute the quad grids after creases or deleted faces (doc); Vernon recomputes packs on every cook |
| Mesh input: TrimTriangularFaces | bool | triangles become trimmed (Yes) or untrimmed (No) NURBS patches (doc) |

Away from extraordinary vertices the polysurface stays G2 in smooth regions (doc). Defaults and numeric tolerances are not published.

### 2.5 Squish (stretch-aware flatten) and SquishBack
Input (doc): a doubly curved mesh or NURBS surface; trimmed NURBS are squished through a dense mesh. Outputs (doc): 2-D pattern, deformation markers, report. Not an unroll/unfold/peel; poor for developable shapes (use unroll); simplified physics (no thickness, no real compressibility); no check for overlap in the pattern (wiki). Not for extreme forming such as stamping cans (wiki).

| Parameter | Values | Default | Behaviour |
|---|---|---|---|
| SplitSeams | bool | not stated | cuts a mesh at a seam so closed shapes (cylinders) can flatten. Wiki: Yes for single-surface mesh, No for polysurface |
| PreserveBoundary (older name StiffBoundary) | bool | not stated | avoid distorting boundary lengths |
| Deformation | Free, StretchMostly, StretchOnly, CompressMostly, CompressOnly, CustomA/B/C | not stated | bias between stretch and compression; "...Only" forbids the opposite entirely |
| CustomSetup | BndStretch, BndCompress, InteriorStretch, InteriorCompress; each any positive number | 1 each | higher value = less of that deformation. Examples (doc), order BndStretch, BndCompress, InteriorStretch, InteriorCompress: [1,1,1,100] is described as severely limiting interior expansion (the page's wording does not match the slot that is raised, so the exact slot semantics need an (ref) test); [10,10,1,1] preserves boundary lengths |
| Material | Rigid / Floppy | not stated | Rigid minimises forming stresses (stiff sheet); Floppy minimises geometric distortion (pliable fabric) |
| Outside | Up / Down | not stated | orientation of the 3-D outside in the pattern |
| DecorateEdgeLengths / DecorateAreas | bool | not stated | visualise the face-diagonal edge-length constraints and area-constrained triangles (doc: newer feature) |

Report (doc + wiki): area change; compression and stretch each as an average % and a maximum %, plus the share of surface affected; red points = compression, green points = stretch; up to ten text dots at the worst spots, each number = % length change. Wiki example: area unchanged; compression avg 0.04 %, max 0.11 %, over 44 % of the surface; stretch avg 0.07 %, max 0.28 %, over 35 %. The shares sum below 100 %, so a dead-band exists; its size is not published (inf).
Method statement (doc): minimises changes in facet area and edge length between 3-D and 2-D meshes, combined through constraints that stop the 2-D mesh folding over itself or pinning points; the options choose the weighting.
SquishBack (doc, zh-tw mirror + wiki [S/F]): select the pattern, then curves/points drawn on it; they are returned to the 3-D shape. Forum note (Rhino 7) [S]: curves extending outside the pattern boundary fail.

## 3. Algorithms from published methods (implementer level)

### 3.1 Watertight wrap
Rhino's internal method is undocumented; Vernon uses two public approaches.
**A. Distance-field wrap (clean.watertight_wrap, implement-now).**
1. Sizes: voxel h = target_edge_length / 1.0 (dual contouring yields about one vertex per surface voxel). Grid over bbox padded by |offset| + 3h. Sparse narrow band (tile of 8^3 voxels) to keep a 4.5 m car at h = 5 mm tractable (900 x 360 x 300 cells dense = 97 M; sparse band is a few million).
2. Unsigned distance d(x) to the input: triangles (point-triangle distance via BVH), points (nearest point), NURBS/SubD via their render mesh (doc says Rhino does the same for Inflate).
3. Seed "outside": flood fill from a corner over cells with d(x) > max(offset, 0) + h/2. Inside = everything not reached (this seals gaps narrower than ~2*(offset + h/2), the documented "small gaps close" behaviour). For solid inputs with the Fill Holes option, first close boundary loops (3.1C), then use generalized winding number (Jacobson 2013; fast version Barill 2018) as inside test instead of flood fill when the input is a closed oriented mesh.
4. Surface = iso-level `offset` of signed field s(x) = (inside ? -1 : +1) * d(x) - offset. Extract with dual contouring (Ju 2002) or manifold dual contouring; marching cubes (Lorensen & Cline 1987) is the fallback and cannot keep sharp corners.
5. Isotropic remesh to the exact target edge length (Botsch & Kobbelt 2004): repeat 5-10 times: split edges longer than 4/3 L, collapse edges shorter than 4/5 L, flip edges toward valence 6, tangential relaxation, reproject to the iso-surface. L = target_edge_length.
6. Smoothing iterations: uniform Laplacian x_i += 0.5 * (mean(neighbours) - x_i) per iteration (shrinks, matches the documented "deflate"). Option `shrink_compensate` uses Taubin lambda|mu (lambda 0.5, mu -0.53, Taubin 1995).
7. Polygon optimization p in [0,100]: quadric error metric collapse (Garland & Heckbert 1997) with a penalty plane on edges whose dihedral angle > 30 degrees so hard edges survive; stop when faces <= F0 * (1 - 0.9 p/100) (V mapping; Rhino's mapping is unknown) or when the next collapse error exceeds (0.25 L)^2.
8. Points / clouds with Inflate: d(x) is the distance to the nearest point, offset must be > 0 (V; clamp to h if <= 0). Ball union results; balls closer than 2*offset merge.
9. Output check: closed and 2-manifold. The vendor text promises a single closed mesh or failure (doc); Vernon returns every component and a `components` count with a warning when it is > 1 (V), because disjoint inflated points are a legitimate result.
**B. Conservative alpha wrap (clean.alpha_wrap, white-box-first).** 3D alpha wrapping (Portaneri et al., SIGGRAPH 2022; CGAL package docs [F]). Parameters alpha (smallest gap that can be crossed; also caps output facet circumradius) and offset (distance of output vertices from the input). Recommended starting values: alpha = bbox_diag/20, offset = alpha/30 (docs example). Steps: put the 8 corners of a loose box in a 3D Delaunay triangulation; flood from outside through "gates" (alpha-traversable facets, circumradius > alpha) held in a priority queue ordered by decreasing circumradius; when about to move from an outside cell to an inside cell, either (a) insert the first crossing of the dual Voronoi edge with the offset surface as a Steiner point, or (b) if the cell intersects the input, insert the projection of its circumcenter on the offset surface; otherwise mark the cell outside and push new gates; at the end output the facets between inside and outside cells. Oracle needs only point-to-input distance, projection, and tetrahedron-input intersection, so triangle soup and point clouds both work. Guarantees: terminates, output is 2-manifold, oriented, self-intersection-free, strictly encloses the input. Caveat: alpha smaller than a hole can give a two-sided result. Needs robust Delaunay predicates (use `robust`/exact predicates crate; do not hand-roll).
**C. Hole fill (clean.fill_boundary_holes, implement-now).** Detect boundary loops (half-edge with no twin); optionally skip loops longer than `max_hole_edges`. Triangulate by minimum-weight dynamic programming over the loop (Liepa 2003 / Barequet-Sharir style), then refine to the local edge length and fair with a thin-plate or Laplacian solve with boundary fixed.

### 3.2 Quad remesh
Reference algorithms (open, paper-level):
- **Instant Field-Aligned Meshes** (Jakob, Tarini, Panozzo, Sorkine-Hornung, SIGGRAPH Asia 2015) [F page, paper details R]. Pipeline: (1) orientation field: per vertex a tangent direction o_i under 4-RoSy symmetry (rotations by 90 degrees about the normal are equivalent). Smoothing operator: o_i <- normalize(project(sum_j w_ij * R(o_j, k_ij))) where k_ij in {0..3} is the quarter-turn that best aligns o_j to o_i; w_ij from edge/area weights. Run Gauss-Seidel sweeps coarse-to-fine on a hierarchy built by repeated pairwise vertex merging (greedy matching on normal agreement weighted by area), roughly halving vertices per level. (2) position field: each vertex stores p_i; lattice spacing rho = sqrt(A / N_target); neighbour update p_i <- round_lattice(weighted mean of the neighbour positions after choosing the lattice translation that fits p_i), same hierarchical solve. (3) extraction: link neighbouring lattice points into edges when their rounded positions agree, delete duplicate/dangling edges, collect 4-cycles as quads; holes become triangles/pentagons (quad-dominant), optional extrinsic smoothing, optional snapping to creases above a crease-angle threshold. Cost is linear in input size; page reports < 1 s for hundreds of thousands of faces and 9 min 18 s for 372 M triangles.
- **QuadriFlow** (Huang, Zhou, Niessner, Shewchuk, Guibas, SGP 2018) [S]: reuses the Instant Meshes objective but adds integer constraints on the edge offsets of the position field, enforced by a global minimum-cost network flow plus local SAT problems; reports about 4x fewer singularities than Instant Meshes on 17,791 ShapeNet models, under 10 s per model. Output is a pure manifold quad mesh. Open-source (hjwdzh/quadriflow); the default solver is a Boykov max-flow, `-mcf` selects network simplex [S].
- **Mixed-integer quadrangulation** (Bommes, Zimmer, Kobbelt, SIGGRAPH 2009) and **QEx** (Ebke et al., SIGGRAPH Asia 2013) [R]: integer-grid map and robust quad extraction; use as the theoretical frame for how singularities arise (index +-1/4 of the 4-RoSy field).
Vernon mapping of Rhino controls onto these methods (V):

| Rhino control | Implementation |
|---|---|
| Target quad count N | rho = sqrt(A / N) (A = input surface area); for a closed genus-g all-quad mesh V = F + 2 - 2g |
| Target edge length L | rho = L; N_est = A / L^2 |
| Adaptive size a (0-100) | per-vertex size rho_i = rho * clamp(1 / (1 + (a/100) * c * kappa_i * rho), 0.25, 1), kappa_i = max principal curvature (smoothed), c = 1 (tunable); then smooth rho with a Laplacian so adjacent ratio <= 2; Instant Meshes has no sizing field, so this needs a scale-aware lattice (integer grid with locally varying rho, as in MIQ) |
| Hard edges | edges with dihedral angle > 30 degrees become orientation constraints (o_i parallel to the edge) and position constraints (lattice point snapped onto the edge line) |
| Guide curves | Approximate: soft orientation constraint, weight 0.2 within 2 rho of the curve; Edge Ring: orientation perpendicular to the curve tangent, weight 1; Edge Loop: orientation parallel plus position constraint pinning one lattice axis to the curve (hard) |
| Symmetry | remesh one half with constraints on the mirror plane, mirror and weld (preferred), or symmetrize both fields |
| Use surface edges Smart/Strict | treat B-rep face borders as hard edges (Strict) or only those whose dihedral > 30 degrees or whose length > 4 rho (Smart) |
| Convert to SubD | see 3.3; Interpolate SubD solves for a cage whose limit surface passes through the quad vertices |

Quality metrics to compute and expose (clean-room, standard): quad fraction; valence histogram; sum over vertices (4 - valence) = 8 for a closed all-quad genus-0 mesh; minimum scaled Jacobian (target > 0.3 on > 99 % of quads); edge-length ratio; one-sided deviation to the input.

### 3.3 Subdivision surfaces
- **Catmull-Clark** (Catmull & Clark 1978 [R]). Per refinement: face point = mean of face vertices; edge point = mean of the 2 endpoints and the 2 adjacent face points (boundary: edge midpoint); vertex point of valence n = (F + 2R + (n-3)P) / n, F = mean of adjacent face points, R = mean of adjacent edge midpoints, P = old position. Non-quad faces become quads after the first step.
- **Limit position** (Halstead, Kass, DeRose 1993 [R]; Stam 1998 exact evaluation [R]). Interior vertex of valence n: p_inf = (n^2 v + 4 sum e_i + sum f_i) / (n (n + 5)), e_i = edge-neighbour control points, f_i = opposite corners of the incident quads. For n = 4 this is the 16/4/1 over 36 stencil.
- **Interpolate (Location) mode.** Build the sparse matrix L of limit stencils, solve L c = v for the cage c (CG or sparse Cholesky; symmetric positive definite for closed regular-ish cages); output the cage c with the input topology (Halstead et al.).
- **Creases / corners** (DeRose, Kass, Truong 1998 [R]): crease edge rule: new edge point = midpoint; crease vertex with two crease edges = (6 v + e_a + e_b) / 8 refinement and cubic B-spline limit curve; corner vertex (>= 3 creases, or flagged) = fixed. Vernon uses infinite sharpness only (Rhino documents creases as on/off); semi-sharp is a later option (V).
- **Boundary**: open cage boundary follows the crease/boundary rule, so a flat m x n quad grid has cubic B-spline boundary curves.

### 3.4 SubD -> NURBS
- A Catmull-Clark patch whose four corners and the whole 1-ring are regular (valence 4, interior) equals a uniform bicubic B-spline: control points are the 4 x 4 cage neighbourhood. A rectangular grid of m x n regular quads (Packed) is one bicubic B-spline surface with (m+3) x (n+3) control points (open boundary with the clamped boundary rule has the same count); Unpacked gives m*n surfaces of 4 x 4 control points.
- Around extraordinary vertices (EV) the limit surface is not polynomial. Approaches: (a) sample the limit surface by Stam's eigen-decomposition and fit bicubic Bezier patches with least squares, with the edge curves fixed and shared (G0), cross-boundary derivatives made equal (G1), then optionally curvature matching (G2); (b) approximate Catmull-Clark patches (ACC) by Loop & Schaefer 2008 (one bicubic per face with G1 across edges, sufficient for G1/G1x) [R]; (c) Peters' patching of Catmull-Clark meshes (SIGGRAPH 2000) [R] as a high-continuity alternative.
- Quantities to report per conversion: surface count, control-point count, max/RMS distance to the SubD limit (sampled at 16 x 16 per face), max normal angle across shared edges (G1 check), and number of EV patches. Rhino target for G1x: edge-normal deviation <= 1 degree (doc).
- G1xx analogue (V): insert a double knot at 0.1 and 0.9 on each face isocurve neighbouring an EV and add control points; compare zebra-stripe via normal variation along the isocurve.

### 3.5 Stretch-aware flatten
Based on published surface-flattening ideas (not Rhino code): ARAP parameterization (Liu, Zhang, Xu, Gotsman, Gortler, SGP 2008 [S]) and ARAP flattening notes (Igarashi & Igarashi 2009 [S]); LSCM initialisation (Levy et al. 2002 [R]); locally injective mappings (Rabinovich et al. 2017, SLIM [R]); energy-based triangle flattening (Wang, Smith, Yuen, 2002 [R]).
1. SplitSeams: if the mesh is not a topological disk, cut along a shortest path (or the user's seam edges) between boundaries/cone points until chi = 1 (cylinder: 1 cut; sphere: 1 path then fan).
2. Initial map: LSCM or Tutte + ARAP local-global iterations; scale so total area equals the 3-D area (V).
3. Energy to minimise over 2-D positions u: E = sum_edges k_e(r) * (|u_i - u_j| / L_ij - 1)^2 + sum_t k_a(s) * (A2_t / A3_t - 1)^2, r = |u_i - u_j|/L_ij, s = A2_t/A3_t. Add edge-length constraints on the diagonals of adjacent triangle pairs (what the vendor "face diagonal" decoration shows, inf) so shear costs energy. Asymmetry via k: k_e = w_stretch if r > 1 else w_compress, and per-class factors multiply boundary vs interior edges: BndStretch, BndCompress, InteriorStretch, InteriorCompress (each divides that deformation; V: weight = 1 * factor). Deformation presets: Free = (1,1,1,1); StretchMostly = compress weights x 10; CompressMostly = stretch weights x 10; StretchOnly / CompressOnly = hard inequality r >= 1 / r <= 1 enforced by active-set projection or weight x 1e4 (V).
4. Material: Rigid = area term weight 1, edge term weight 0.3 (minimise membrane stress); Floppy = edge term weight 1, area term 0.1 and extra shear penalty (minimise geometric distortion, cloth). The vendor gives no numbers (V).
5. Fold-free guard: line search keeps every triangle's signed area > 1e-6 * mean area (Smith & Schaefer 2015-style barrier or SLIM bounds); report if no fold-free solution exists.
6. Solver: Gauss-Newton or L-BFGS on sparse variables; stop when relative energy change < 1e-6 or 200 iterations (V).
7. Strain per triangle: J = 2x2 Jacobian from the 3-D triangle's local frame to the 2-D triangle; singular values s1 >= s2. stretch % = max(s1 - 1, 0) * 100, compression % = max(1 - s2, 0) * 100, area change % = (s1*s2 - 1) * 100, shear = s1/s2 - 1. Pattern totals: area change, area-weighted mean and max of stretch and compression, share of area with strain above a dead-band eps = 0.01 % (V; the vendor dead-band is unpublished).
8. Markers: up to 10 text dots at the triangles with largest |strain|, value = percent length change, sign shown; point-cloud markers red = compression, green = stretch at triangle centroids.
9. Warnings always emitted: ignores thickness, drape and compressibility; not for strains above 10-20 % (V threshold; vendor says only "not for extreme deformation"); not valid for developable surfaces (zero strain expected; use a developable unroll).

### 3.6 Map back
For every point or sampled curve vertex in the pattern: locate the containing 2-D triangle (BVH or uniform grid), compute barycentric coordinates (b0,b1,b2), return b0*P0 + b1*P1 + b2*P2 in 3-D, optionally project onto the original NURBS. Sample curves adaptively so the 2-D chord error <= tol = 0.01 mm. Pieces outside the pattern are clipped at the boundary and reported (V; vendor behaviour on outside curves is an error). Exact for mesh vertices by construction.

## 4. Proposed Vernon nodes

Labels contain no vendor names. Status: **now** = fully specified above and small, **WB** = white-box-first (write a transparent reference implementation / test harness against the cited paper before optimising or wrapping an external tool).

| Id | Label | Inputs | Outputs | Key params | emulates | Status |
|---|---|---|---|---|---|---|
| clean.watertight_wrap | Watertight Wrap | geometry list (mesh, points, cloud) | closed mesh, wrap report | target_edge_length (mm, auto), offset (mm), smooth_iterations, polygon_optimization (0-100), inflate_points, fill_input_holes, transfer_colors | Rhino 8 ShrinkWrap | now |
| clean.alpha_wrap | Conservative Wrap (alpha) | mesh / triangle soup / cloud | closed manifold mesh | alpha (mm), offset (mm) | CGAL 3D alpha wrap (paper) | WB |
| clean.fill_boundary_holes | Fill Holes | mesh | mesh, hole report | max_hole_edges, fair (bool), target_edge | ShrinkWrap fill-holes option | now |
| clean.smooth_mesh | Smooth Mesh | mesh | mesh | iterations, method (laplace/taubin), lock_boundary, lock_hard_edges | ShrinkWrap smoothing | now |
| clean.decimate_adaptive | Adaptive Polygon Reduction | mesh | mesh | reduction (0-100), crease_angle_deg (30), max_error (mm) | ShrinkWrap polygon optimization | now |
| clean.isotropic_remesh | Uniform Triangle Remesh | mesh | mesh | target_edge_length (mm), iterations, keep_features | internal to ShrinkWrap | now |
| surface.quad_remesh | Quad Remesh | mesh, guide curves (opt), symmetry plane (opt) | quad mesh, remesh report | size_mode (count/edge), target_quads, target_edge (mm), adaptive (0-100), detect_hard_edges, hard_edge_angle_deg (30), symmetry_axes, guide_influence (none/approx/ring/loop), use_surface_edges (off/smart/strict) | Rhino 8 QuadRemesh | WB |
| surface.quad_mesh_report | Quad Mesh Report | mesh, reference mesh (opt) | metrics table | deviation_percentiles (50,95,99.9) | QuadRemesh preview readout | now |
| surface.mesh_to_subd | Mesh to Subdivision Surface | quad/poly mesh | subdivision surface | interpolate (bool), creases (bool), corners (bool), crease_angle_deg | Rhino 8 ToSubD, QuadRemesh Convert-to-SubD | now |
| surface.subd_refine | Subdivision Refine | subdivision surface | mesh (level k) or limit points | levels (0-6), output (cage/level/limit) | Rhino SubDivide | now |
| surface.subd_to_nurbs | Subdivision to NURBS | subdivision surface | NURBS patch set, conversion report | faces (packed/unpacked), ev_continuity (g0/g1/g1x/g1xx/g2) | Rhino 8 ToNURBS | WB |
| surface.subd_deviation | Subdivision vs NURBS Deviation | subdivision surface, NURBS set | deviation percentiles, G1 angle | samples_per_face (16), percentiles | ToNURBS quality check | now |
| flatten.seam_cut | Cut Flattening Seams | mesh | disk-topology mesh | seam_edges (opt), auto | Squish SplitSeams | now |
| flatten.stretch_aware | Stretch-Aware Flatten | mesh, seam (opt) | 2-D pattern mesh, strain field | deformation (free/stretch_mostly/stretch_only/compress_mostly/compress_only/custom_a-c), custom (bnd_stretch, bnd_compress, int_stretch, int_compress; default 1), material (rigid/floppy), preserve_boundary, outside (up/down) | Rhino 8 Squish | WB |
| flatten.strain_report | Flatten Strain Report | 3-D mesh, 2-D mesh (any flattener) | area change, stretch/compression stats, markers | deadband_percent (0.01), max_markers (10) | Squish report | now |
| flatten.map_back | Map Pattern Back to 3D | pattern mesh + source mesh/surface, curves/points on pattern | curves/points on 3-D | tolerance (mm, 0.01), project_to_surface | Rhino 8 SquishBack | now |

Total proposed: 16 nodes. Recommended order: clean.fill_boundary_holes, clean.isotropic_remesh, clean.watertight_wrap, surface.mesh_to_subd, surface.subd_refine, flatten.strain_report, flatten.map_back, then the WB nodes. Candidate external reference runs (separate process, licence check first): Instant Meshes and QuadriFlow executables for the baseline of surface.quad_remesh (both published as open source on GitHub; licences recalled, not verified [R]).

## 5. Golden test ideas (numbers are analytic unless marked (ref))

Tolerances are Vernon pass criteria; "(ref)" means compare to a recorded run of a licensed copy on a user-made input. h = voxel size.

**Watertight wrap**
- W1 cube a = 100 mm as triangle soup with one face deleted, target edge 5 mm, offset 0, fill_input_holes on: output closed, 2-manifold, Euler characteristic 2, volume 1,000,000 mm^3 +-3 %, one-sided Hausdorff to the cube <= 5 mm. With fill off: volume < 15 % of solid (thin two-sided shell) and a warning.
- W2 same cube, offset +2 mm, edge 2 mm: volume = a^3 + 6a^2 d + 3 pi a d^2 + (4/3) pi d^3 = 1,123,803 mm^3 +-1 %; area = 6a^2 + 6 pi a d + 4 pi d^2 = 63,820 mm^2 +-2 %. Offset -2 mm: cube side 96, volume 884,736 mm^3 +-1.5 %.
- W3 point cloud of the 8 cube corners (+-50 mm), inflate on, edge 1 mm: offset 10 -> `components` = 8, total volume 8 * (4/3) pi 10^3 = 33,510 mm^3 +-3 %, total area 8 * 4 pi 10^2 = 10,053 mm^2 +-3 %; offset 60 (> half the 100 mm spacing) -> `components` = 1 (merged).
- W4 alpha wrap of W1 cube: alpha = diag/20 = 8.66 mm, offset = alpha/30 = 0.289 mm: manifold, zero self-intersections, every input vertex inside, output vertex distance to input = 0.289 mm +-10 %.
- W5 smoothing: sphere r = 50 mm, edge 3 mm, 10 Laplacian iterations: volume decreases monotonically; with Taubin volume change < 1 %.
- W6 polygon optimization 50 %: face count <= 55 % of baseline, all 12 cube edges still present as edge chains (dihedral 90 > 30 degrees), Hausdorff to unreduced <= 0.25 * L.
- W7 edge-length sag budget for the 0.5 mm-per-stage handoff: sag = L^2 / (8R); with R_min = 500 mm and sag <= 0.1 mm, L <= sqrt(8 * 500 * 0.1) = 20 mm. A 1 m^2 panel gives about 92,400 triangles at L = 5 mm (area per equilateral triangle 0.433 L^2) and about 2,500 quads at L = 20 mm.

**Quad remesh**
- Q1 sphere r = 50 mm (A = 31,416 mm^2), N = 1000: expected L = sqrt(A/N) = 5.61 mm; accept F in [850, 1150], quads >= 98 %, sum(4 - valence) = 8 for the all-quad part, median edge length within 5.61 +-15 %, all vertices within 0.05 mm of the sphere.
- Q2 cube a = 100, N = 600, hard edges on (90 degrees > 30): exact regular result expected: 6 faces x 10 x 10 grid, F = 600, V = 602, E = 1200 (Euler = 2), 8 valence-3 vertices; accept F in [540, 660] and >= 99 % of the 12 cube-edge length covered by mesh edges within 0.1 L. With detect_hard_edges off, sharp edges may round: report only.
- Q3 symmetry X on a symmetric input: for every output vertex v the mirrored point has a vertex within 1e-6 * diag; no vertices closer than 0.1 L to the plane that are not on it.
- Q4 guide curve: equator circle (r = 50 mm) projected on the Q1 sphere; Edge Loop -> >= 95 % of the curve length within 0.25 L of a mesh edge chain; Approximate -> mean angle between edges and curve tangent smaller than with None.
- Q5 performance: 200 k-face input remeshed to N = 5000 in < 10 s (V; the published reference points are < 1 s for hundreds of thousands of faces and < 10 s per ShapeNet model).
- Q6 deviation handoff (stage check): P95 deviation to input <= 0.05 L and P99.9 <= 0.25 L; with the toolchain report's example budget (95 % within +-0.5 mm) the pass line is P95 <= 0.5 mm.

**Subdivision**
- S1 cube cage vertices (+-1,+-1,+-1): one Catmull-Clark step moves a corner to (5/9, 5/9, 5/9); face points (0,0,+-1) etc., edge points (0.75, 0.75, 0); limit position of a corner (n = 3): (9v + 4*sum e + sum f)/24 = (0.5, 0.5, 0.5) to 1e-9.
- S2 interpolate on: input cube vertices (+-1) -> solved cage is (+-2, +-2, +-2); limit at cage corners returns (+-1).
- S3 all 12 edges creased: corner limits equal the cage corners exactly; limit at each edge midpoint lies on the straight edge within 1e-9; faces bulge outward.
- S4 corners flag: a flat 4 x 3 quad grid with open boundary -> corners stay fixed when SubD Corners is on, rounded when off (difference at the grid corner > 0).

**Subdivision to NURBS**
- N1 curved regular grid, 4 x 3 quads: Packed -> 1 surface, degree 3 x 3, 7 x 6 = 42 control points; Unpacked -> 12 surfaces with 16 control points each; max distance to the limit <= 1e-9 * bbox_diag in the regular region.
- N2 cube cage with each face subdivided 4 x 4 (valence-3 EV at the 8 corners, regular elsewhere): Unpacked -> 96 surfaces; with G0 gap at shared edges <= 1e-6 mm; G1 normal jump <= 0.1 degree; G1x normal jump <= 1 degree (documented target); deviation to limit <= 0.1 % of bbox_diag (V).
- N3 G2 option on the same cage: report curvature jump at EV, flag bulges where the deviation exceeds the G0 deviation by > 2x.

**Flatten and map back**
- F1 cylinder R = 100 mm, height 200 mm, 128 segments, SplitSeams on: pattern is a rectangle of about 628.3 x 200 mm (perimeter 2 pi R = 628.32 mm; the 128-gon is about 0.01 % short, so compare against the mesh's own 3-D lengths); area change < 0.05 %, max stretch and compression < 0.1 %.
- F2 spherical cap R = 1000 mm, half-angle 30 degrees (area 841,787 mm^2, boundary 3,141.59 mm). Isoperimetric bounds hold for any correct flatten: (a) area preserved -> boundary mean length change >= +3.53 % (circle of equal area has perimeter 3,252.4 mm); (b) PreserveBoundary on with boundary length kept within 0.1 % -> area change <= -6.5 % (a circle of 3,141.59 mm perimeter encloses 785,398 mm^2, i.e. -6.70 % exactly); (c) StretchOnly -> compression share 0 % and area change >= 0; (d) CompressOnly -> stretch share 0 % and area change <= 0; (e) CustomSetup [10,10,1,1] gives smaller boundary strain than [1,1,1,1].
- F3 report synthetic: apply a uniform 3-D -> 2-D scale of 1.003 to any mesh: stretch avg = max = 0.30 %, share 100 %, area change +0.6009 % (1.003^2), compression 0 %, every text dot reads 0.3. Uniform scale 0.9995: compression 0.05 %, area -0.1 %.
- F4 report format replay of the wiki example (area unchanged, compression 0.04 % avg / 0.11 % max over 44 %, stretch 0.07 % avg / 0.28 % max over 35 %): build a synthetic strain field with those statistics and check the report prints them to 2 decimals and emits <= 10 markers. (ref) on a real panel is needed for the Squish-vs-Vernon comparison; accept Vernon max strain within 2x of the reference tool for the same Deformation/Material.
- F5 map back: for all 3-D vertices, map_back(pattern vertex) = original vertex within 1e-9 * diag; a 2-D polyline crossing 3 triangles lands on 3-D triangle edges at the barycentric crossings; a curve leaving the pattern is clipped with an outside-length report > 0.
- F6 seam cut: closed cylinder (chi = 0) -> one seam -> disk (chi = 1) with perimeter = 2 * seam length + the original boundary; sphere -> chi = 1 after one path.
- F7 guard: a developable cone patch flattens with max strain < 0.1 %; flag "developable, use unroll" if the Gaussian-curvature integral (angle defect) over interior vertices is < 1e-3 rad.

## 6. Sources (every URL consulted or cited)

Fetched this session [F]:
- https://docs.mcneel.com/rhino/8/help/en-us/commands/shrinkwrap.htm
- https://docs.mcneel.com/rhino/8/help/en-us/commands/quadremesh.htm
- https://docs.mcneel.com/rhino/8/help/en-us/commands/squish.htm
- https://docs.mcneel.com/rhino/8/help/en-us/commands/tosubd.htm
- https://docs.mcneel.com/rhino/8/help/en-us/commands/tonurbs.htm
- https://wiki.mcneel.com/labs/advancedflattening
- https://doc.cgal.org/latest/Alpha_wrap_3/index.html
- https://igl.ethz.ch/projects/instant-meshes/ (landing page; the paper PDF URL tried returned 404, so equations above are recalled [R])
- /rhino/8/help/en-us/commands/squishback.htm returned only a redirect shell; SquishBack facts come from the search-listed zh-tw squish page and the wiki.

Seen in search results only [S]:
- https://docs.mcneel.com/rhino/mac/help/zh-tw/commands/squish.htm (SquishBack one-line definition)
- https://discourse.mcneel.com/t/squish-and-squishback/36629 and https://discourse.mcneel.com/t/issues-with-squishinfo-and-squishback/132859 (SquishBack limits, Rhino 7 era, unverified)
- https://docs.mcneel.com/rhino/9/help/en-us/commands/tonurbs.htm (OutputLayer, Rhino 9)
- https://stanford.edu/~jingweih/papers/quadriflow/ , https://diglib.eg.org/handle/10.1111/cgf13498 (QuadriFlow, SGP 2018)
- https://github.com/hjwdzh/quadriflow (code; solver options reported via a README listing)
- https://www.cs.jhu.edu/~misha/ReadingSeminar/Papers/Liu08.pdf (ARAP parameterization, Liu et al. 2008)
- https://www-ui.is.s.u-tokyo.ac.jp/~takeo/papers/takeo_jgt09_arapFlattening.pdf (Igarashi & Igarashi 2009)

Recalled, not retrieved, verify before relying [R] (no URL given where I am unsure of the address):
- Portaneri et al., "3D Alpha Wrapping", ACM TOG 41(4) 2022 (SIGGRAPH), https://doi.org/10.1145/3528223.3530152
- Jakob et al., "Instant Field-Aligned Meshes", SIGGRAPH Asia 2015; code https://github.com/wjakob/instant-meshes
- Bommes, Zimmer, Kobbelt, "Mixed-integer quadrangulation", SIGGRAPH 2009; Ebke et al., "QEx", SIGGRAPH Asia 2013
- Catmull & Clark 1978, https://doi.org/10.1016/0010-4485(78)90110-0
- Halstead, Kass, DeRose, "Efficient, fair interpolation using Catmull-Clark surfaces", SIGGRAPH 1993
- Stam, "Exact evaluation of Catmull-Clark subdivision surfaces at arbitrary parameter values", SIGGRAPH 1998
- DeRose, Kass, Truong, "Subdivision surfaces in character animation", SIGGRAPH 1998
- Loop & Schaefer, "Approximating Catmull-Clark subdivision surfaces with bicubic patches", ACM TOG 27(1) 2008
- Peters, "Patching Catmull-Clark meshes", SIGGRAPH 2000
- Levy et al., "Least squares conformal maps", SIGGRAPH 2002; Rabinovich et al., "Scalable locally injective mappings", ACM TOG 2017; Smith & Schaefer, "Bijective parameterization with free boundaries", SIGGRAPH 2015; Wang, Smith, Yuen, "Surface flattening based on energy model", Computer-Aided Design 2002
- Lorensen & Cline 1987; Ju et al., "Dual contouring of Hermite data", SIGGRAPH 2002; Garland & Heckbert, "Surface simplification using quadric error metrics", SIGGRAPH 1997; Taubin, "A signal processing approach to fair surface design", SIGGRAPH 1995; Botsch & Kobbelt, "A remeshing approach to multiresolution modeling", SGP 2004; Liepa, "Filling holes in meshes", SGP 2003; Jacobson et al. 2013 (generalized winding number); Barill et al. 2018 (fast winding numbers)
- Project files: C:\Users\mabdu\REA\3D printed body kit toolchain.md (report numbers: 95 % within +-0.5 mm deviation percentile, scanner noise 0.03-0.04 mm, drift 0.05 mm/m, Squish stretch/compression reporting)

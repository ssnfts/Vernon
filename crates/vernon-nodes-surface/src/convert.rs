//! Conversion and editing nodes: faceted / prismatic solids, sewing,
//! mirroring and thickening.

use glam::DVec3;
use serde_json::json;
use vernon_core::{AttribArray, Geometry, Metric, Prim, Report, Severity};
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::curvefit::fit_with_knots;
use crate::nurbs::{clamped_uniform_knots, NurbsCurve, NurbsSurface, PatchSet};
use crate::patchops::{find_seams, Seam};
use crate::segment::{labels_from_geo, seg_mesh};
use crate::topology::{boundary_loops, geometry_volume, orient, stats, weld, Welded};
use crate::util::{check_finite, json_err, json_input, max_abs_coord, node_info, positive, triangulate_ring, SPEC_RE};

fn welded_tolerance(ctx: &CookContext, params: &Params, geo: &Geometry) -> CookResult<f64> {
    let tol = params.float("weld_tol_mm")?;
    if !(tol.is_finite() && tol >= 0.0) {
        return Err(ctx.fail("parameter 'weld_tol_mm' must not be negative"));
    }
    // meshes read from STL carry f32 coordinates: never weld below about one f32 step
    Ok(tol.max(1.2e-7 * max_abs_coord(&geo.points)))
}

/// `surface.convert_faceted`
pub struct ConvertFacetedNode;

impl NodeOp for ConvertFacetedNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.convert_faceted", "Faceted Solid", SPEC_RE)
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("shell", DataKind::Geometry))
            .output(PortSpec::new("brep", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("weld_tol_mm", "Weld tolerance", 1e-6, 0.0, 10.0).unit("mm").help("Vertices closer than this are merged. A floor of one single-precision step of the coordinates is always applied."))
            .param(ParamSpec::bool("quads_as_faces", "Keep polygons", true).help("Keep quads and larger polygons as single faces instead of splitting them into triangles."))
            .param(ParamSpec::int("max_faces", "Face limit", 2_000_000, 100, 100_000_000).help("Refuse meshes with more faces than this; reduce the mesh first."))
            .help("Turns a mesh into a faceted shell: welds vertices, makes the winding consistent, points a closed shell outwards and counts open and non-manifold edges. A closed manifold shell is reported as a solid, anything else stays a shell.")
            .emulates("Fusion Convert Mesh (Faceted)")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let mesh = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &mesh)?;
        let tol = welded_tolerance(ctx, params, &mesh)?;
        let max_faces = params.float("max_faces")? as usize;
        let n_in = mesh.prims.iter().filter(|p| p.closed && p.vertices.len() >= 3).count();
        if n_in > max_faces {
            return Err(ctx.fail(format!("the mesh has {n_in} faces, above the limit of {max_faces}; decimate it first")));
        }
        let mut w = weld(&mesh, tol);
        if w.faces.is_empty() {
            return Err(ctx.fail("the mesh has no usable faces"));
        }
        let fixed = orient(&mut w);
        let st = stats(&w);
        let faces: Vec<Vec<u32>> = if params.bool("quads_as_faces")? {
            w.faces.clone()
        } else {
            w.faces.iter().flat_map(|f| triangulate_ring(&w.points, f).into_iter().map(|t| t.to_vec())).collect()
        };
        let shell = Geometry::from_polygons(w.points.clone(), faces.clone());
        let solid = st.closed && st.volume > 0.0;
        let (b0, b1) = (mesh.bounds(), shell.bounds());
        let bbox_delta = if b0.is_empty() { 0.0 } else { (b0.min - b1.min).abs().max_element().max((b0.max - b1.max).abs().max_element()) };
        let orientation_ok = fixed.propagated_flips == 0 && fixed.flipped_components == 0;
        let mut report = Report::new("Faceted solid");
        report.metric("faces", Metric::new(faces.len() as f64, "count"));
        report.metric("vertices", Metric::new(st.vertices as f64, "count"));
        report.metric("edges", Metric::new(st.edges as f64, "count"));
        report.metric("watertight", Metric::new(f64::from(st.closed), ""));
        report.metric("boundary_edges", Metric::new(st.boundary_edges as f64, "count"));
        report.metric("nonmanifold_edges", Metric::new(st.nonmanifold_edges as f64, "count"));
        report.metric("euler_char", Metric::new(st.euler_char as f64, ""));
        report.metric("components", Metric::new(st.components as f64, "count"));
        report.metric("volume_mm3", Metric::new(if st.closed { st.volume } else { 0.0 }, "mm3"));
        report.metric("orientation_ok", Metric::new(f64::from(orientation_ok), "").with_limit(1.0, false));
        report.metric("bbox_delta_mm", Metric::new(bbox_delta, "mm"));
        if !orientation_ok {
            report.note(Severity::Warning, format!("winding was repaired: {} faces turned, {} closed shells reversed", fixed.propagated_flips, fixed.flipped_components));
        }
        if st.closed && st.volume <= 0.0 {
            report.note(Severity::Error, "the closed shell has zero or negative volume");
        }
        if !st.closed {
            report.note(Severity::Info, "the shell is open or non-manifold; it stays a shell, not a solid");
        }
        let brep = json!({"schema": "faceted_brep", "version": 1, "units": "mm", "faces": faces.len(), "vertices": st.vertices, "edges": st.edges, "watertight": st.closed, "solid": solid});
        Ok(vec![Data::geometry(shell), Data::json(brep), Data::report(report)])
    }
}

/// `surface.convert_prismatic`
pub struct ConvertPrismaticNode;

impl NodeOp for ConvertPrismaticNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.convert_prismatic", "Prismatic Solid", SPEC_RE)
            .input(PortSpec::new("regions", DataKind::Geometry))
            .output(PortSpec::new("brep", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("shell", DataKind::Geometry))
            .param(ParamSpec::float("tol_mm", "Tolerance", 0.05, 0.001, 5.0).unit("mm").help("Largest accepted distance between a region and its analytic surface."))
            .param(ParamSpec::bool("keep_unfit_as_facets", "Keep unfitted regions faceted", true).help("Regions without a good analytic fit stay as one face per triangle."))
            .help("Recognizes planes, spheres, cylinders and cones using deterministic sample consensus and geometric refitting. Optional region labels restrict supports. Emits analytic parameters, measured vertex and centroid residuals, source boundary polylines and a labelled tessellated shell. Unfitted supports optionally stay faceted; analytic CAD trimming is not performed.")
            .emulates("Fusion Convert Mesh (Prismatic)")
            .implemented()
    }

    fn cook(
        &self,
        ctx: &CookContext,
        inputs: &[Option<Data>],
        params: &Params,
    ) -> CookResult<Vec<Data>> {
        let geo = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &geo)?;
        let tol = positive(ctx, "tol_mm", params.float("tol_mm")?)?;
        if geo
            .prims
            .iter()
            .any(|p| p.vertices.iter().any(|&v| v as usize >= geo.points.len()))
        {
            return Err(ctx.fail("mesh face refers to a missing vertex"));
        }
        geo.validate().map_err(|e|ctx.fail(format!("invalid mesh: {e}")))?;
        if geo.prim_attribs.get("region_id").is_some() && geo.prim_attribs.ints("region_id").is_err() {
            return Err(ctx.fail("region_id must be an integer primitive attribute"));
        }
        let sm = seg_mesh(&geo);
        if sm.mesh.tris.is_empty() {
            return Err(ctx.fail("the mesh has no usable nondegenerate faces"));
        }
        let labels = labels_from_geo(&geo, &sm);
        let groups = crate::prismatic::recognize(&sm.mesh, labels.as_deref(), tol).map_err(|e|ctx.fail(e))?;
        if groups.is_empty() {
            return Err(ctx.fail("the mesh has no usable nondegenerate faces"));
        }
        let keep = params.bool("keep_unfit_as_facets")?;
        let (mut faces, mut analytic, mut faceted_tris) = (Vec::new(), 0usize, 0usize);
        let mut shell = Geometry::new();
        shell.points = sm.mesh.points.clone();
        let mut parts = Vec::new();
        let mut max_dev: f64 = 0.0;
        for reg in &groups {
            let ring_faces: Vec<Vec<u32>> = reg
                .tris
                .iter()
                .map(|&t| sm.mesh.tris[t as usize].to_vec())
                .collect();
            if let Some(shape) = &reg.shape {
                analytic += 1;
                max_dev = max_dev.max(reg.max_dev);
                let mut loops = boundary_loops(&ring_faces);
                let perim = |l: &Vec<u32>| {
                    (0..l.len())
                        .map(|i| {
                            (sm.mesh.points[l[i] as usize]
                                - sm.mesh.points[l[(i + 1) % l.len()] as usize])
                                .length()
                        })
                        .sum::<f64>()
                };
                loops.sort_by(|a, b| perim(b).total_cmp(&perim(a)));
                let pts = |l: &Vec<u32>| {
                    l.iter()
                        .map(|&v| sm.mesh.points[v as usize].to_array())
                        .collect::<Vec<_>>()
                };
                let mut e = shape.json();
                e["region_id"] = json!(reg.id);
                e["source_region_id"] = json!(labels.as_ref().map(|l|l[reg.tris[0]as usize]));
                e["rms_mm"] = json!(reg.rms);
                e["max_deviation_mm"] = json!(reg.max_dev);
                e["support_triangles"] = json!(reg.tris.len());
                let interior_max = reg
                    .tris
                    .iter()
                    .map(|&t| {
                        let t = sm.mesh.tris[t as usize];
                        shape.distance(
                            (sm.mesh.points[t[0] as usize]
                                + sm.mesh.points[t[1] as usize]
                                + sm.mesh.points[t[2] as usize])
                                / 3.,
                        )
                    })
                    .fold(0., f64::max);
                e["sampled_centroid_max_mm"] = json!(interior_max);
                e["outer_loop"] = json!(loops.first().map(pts));
                e["inner_loops"] = json!(loops.iter().skip(1).map(pts).collect::<Vec<_>>());
                e["boundary_loops"] = json!(loops.iter().map(pts).collect::<Vec<_>>());
                faces.push(e);
            } else if keep {
                faceted_tris += reg.tris.len();
                for t in &ring_faces {
                    faces.push(json!({"region_id":reg.id,"type":"facet","outer_loop":t.iter().map(|&v|sm.mesh.points[v as usize].to_array()).collect::<Vec<_>>()}));
                }
            }
            if reg.shape.is_some() || keep {
                for t in ring_faces {
                    shell.prims.push(Prim::polygon(t));
                    parts.push(reg.id);
                }
            }
        }
        shell.prim_attribs.set("part", AttribArray::Int(parts));
        let mut w = weld(&shell, 1e-6f64.max(1.2e-7 * max_abs_coord(&shell.points)));
        let repair = orient(&mut w);
        let st = stats(&w);
        for (p, f) in shell.prims.iter_mut().zip(&w.faces) {
            p.vertices = f.clone();
        }
        shell.points = w.points;
        let mut report = Report::new("Prismatic solid");
        report.metric("faces", Metric::new(faces.len() as f64, "count"));
        report.metric("analytic_faces", Metric::new(analytic as f64, "count"));
        report.metric(
            "faceted_triangles",
            Metric::new(faceted_tris as f64, "count"),
        );
        report.metric("regions", Metric::new(groups.len() as f64, "count"));
        report.metric(
            "fit_max_mm",
            Metric::new(max_dev, "mm").with_limit(tol, true),
        );
        report.metric(
            "analytic_coverage",
            Metric::new(
                (shell.prims.len() - faceted_tris) as f64 / sm.mesh.tris.len() as f64,
                "",
            ),
        );
        report.metric("watertight", Metric::new(f64::from(st.closed), ""));
        report.metric(
            "boundary_edges",
            Metric::new(st.boundary_edges as f64, "count"),
        );
        report.metric(
            "nonmanifold_edges",
            Metric::new(st.nonmanifold_edges as f64, "count"),
        );
        report.metric(
            "volume_mm3",
            Metric::new(if st.closed { st.volume } else { 0. }, "mm3"),
        );
        report.note(Severity::Info,"Analytic fits describe source tessellation vertices; polygon interiors are approximations. Boundary loops retain source polylines; their perimeter ordering is not a CAD trim classification. The shell remains a tessellated surface, without analytic intersection or CAD trimming.");
        if !st.closed {
            report.note(Severity::Warning,"The retained tessellation is open or nonmanifold and cannot be certified as a solid.");
        }
        if repair.propagated_flips + repair.flipped_components > 0 {
            report.note(Severity::Info, "Source tessellation winding was repaired.");
        }
        let doc = json!({"schema":"prismatic_brep","version":1,"units":"mm","faces":faces,"watertight":st.closed,"solid":st.closed&&st.volume>0.,"representation":"tessellated_shell_with_analytic_fits"});
        Ok(vec![
            Data::json(doc),
            Data::report(report),
            Data::geometry(shell),
        ])
    }
}

// ---------------------------------------------------------------------------
// patch helpers
// ---------------------------------------------------------------------------

/// Reverse the u direction (flips the surface normal).
pub fn reversed_u(s: &NurbsSurface) -> NurbsSurface {
    let (a, b) = s.domain_u();
    let mut cps = Vec::with_capacity(s.control_points.len());
    let mut w = s.weights.as_ref().map(|_| Vec::with_capacity(s.control_points.len()));
    for i in 0..s.nu {
        for j in 0..s.nv {
            cps.push(s.cp(s.nu - 1 - i, j));
            if let (Some(wv), Some(src)) = (w.as_mut(), s.weights.as_ref()) {
                wv.push(src[(s.nu - 1 - i) * s.nv + j]);
            }
        }
    }
    NurbsSurface { degree_u: s.degree_u, degree_v: s.degree_v, knots_u: s.knots_u.iter().rev().map(|k| a + b - k).collect(), knots_v: s.knots_v.clone(), nu: s.nu, nv: s.nv, control_points: cps, weights: w }
}

fn boundary_indices(s: &NurbsSurface, side: usize) -> Vec<usize> {
    match side % 4 {
        0 => (0..s.nu).map(|i| i * s.nv).collect(),
        1 => (0..s.nv).map(|j| (s.nu - 1) * s.nv + j).collect(),
        2 => (0..s.nu).map(|i| i * s.nv + s.nv - 1).collect(),
        _ => (0..s.nv).collect(),
    }
}

/// Indices of the row next to the boundary of `side`.
fn inner_indices(s: &NurbsSurface, side: usize) -> Vec<usize> {
    match side % 4 {
        0 => (0..s.nu).map(|i| i * s.nv + 1).collect(),
        1 => (0..s.nv).map(|j| (s.nu - 2) * s.nv + j).collect(),
        2 => (0..s.nu).map(|i| i * s.nv + s.nv - 2).collect(),
        _ => (0..s.nv).map(|j| s.nv + j).collect(),
    }
}

fn snap_pair(ps: &mut PatchSet, seam: &Seam) {
    let (ia, ib) = (boundary_indices(&ps.patches[seam.a].surface, seam.side_a), boundary_indices(&ps.patches[seam.b].surface, seam.side_b));
    // only borders with the same structure can be averaged control point by control point
    let (ca, cb) = (ps.patches[seam.a].surface.boundary_curve(seam.side_a), ps.patches[seam.b].surface.boundary_curve(seam.side_b));
    if ia.len() != ib.len() || ca.degree != cb.degree {
        return;
    }
    let pa: Vec<DVec3> = ia.iter().map(|&i| ps.patches[seam.a].surface.control_points[i]).collect();
    let pb: Vec<DVec3> = ib.iter().map(|&i| ps.patches[seam.b].surface.control_points[i]).collect();
    let n = pa.len();
    let fwd: f64 = (0..n).map(|k| (pa[k] - pb[k]).length_squared()).sum();
    let bwd: f64 = (0..n).map(|k| (pa[k] - pb[n - 1 - k]).length_squared()).sum();
    for k in 0..n {
        let other = if bwd < fwd { pb[n - 1 - k] } else { pb[k] };
        let avg = (pa[k] + other) * 0.5;
        ps.patches[seam.a].surface.control_points[ia[k]] = avg;
        let kb = if bwd < fwd { n - 1 - k } else { k };
        ps.patches[seam.b].surface.control_points[ib[kb]] = avg;
    }
}

/// `surface.sew`
pub struct SewNode;

impl NodeOp for SewNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.sew", "Sew Surfaces", SPEC_RE)
            .input(PortSpec::new("patches", DataKind::Json))
            .output(PortSpec::new("sewn", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("tol_mm", "Tolerance", 0.05, 0.001, 1.0).unit("mm").help("Patch borders closer than this are treated as one seam."))
            .param(ParamSpec::bool("snap", "Snap borders", true).help("Average the border control points of matching seams so the borders coincide."))
            .param(ParamSpec::choice("make_solid", "Make solid", "auto", &["auto", "yes", "no"]).help("Auto marks the result as a solid when every border is sewn and the orientation is consistent."))
            .help("Finds patch borders that coincide within the tolerance, optionally snaps them together, and reports the remaining gap, the largest normal angle across seams, borders left open and orientation conflicts. A fully sewn, consistently oriented set is flagged as closed.")
            .emulates("CAD sew / stitch / knit")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let pj = json_input(inputs, 0).ok_or_else(|| ctx.fail("connect a patch set"))?;
        let mut ps = PatchSet::from_json(&pj).map_err(|e| json_err(ctx, "patch set input", e))?;
        let tol = positive(ctx, "tol_mm", params.float("tol_mm")?)?;
        let before = find_seams(&ps, tol, 8);
        if params.bool("snap")? {
            for _ in 0..3 {
                let rep = find_seams(&ps, tol, 8);
                for s in &rep.seams {
                    snap_pair(&mut ps, s);
                }
            }
        }
        let after = find_seams(&ps, tol, 16);
        let conflicts = after.seams.iter().filter(|s| s.opposed).count();
        let watertight = after.is_closed();
        ps.closed = watertight;
        ps.solid = match params.str("make_solid")? {
            "yes" => true,
            "no" => false,
            _ => watertight && conflicts == 0,
        };
        let mut report = Report::new("Sew surfaces");
        report.metric("patch_count", Metric::new(ps.patches.len() as f64, "count"));
        report.metric("seam_count", Metric::new(after.seams.len() as f64, "count"));
        report.metric("free_edges", Metric::new(after.free_edges.len() as f64, "count"));
        report.metric("gap_before_mm", Metric::new(before.max_gap(), "mm"));
        report.metric("gap_max_mm", Metric::new(after.max_gap(), "mm").with_limit(tol, true));
        report.metric("g1_max_deg", Metric::new(after.max_g1_deg(), "deg"));
        report.metric("orientation_conflicts", Metric::new(conflicts as f64, "count"));
        report.metric("watertight", Metric::new(f64::from(watertight), ""));
        if params.str("make_solid")? == "yes" && !watertight {
            report.note(Severity::Warning, "marked as solid although borders remain open");
        }
        if conflicts > 0 {
            report.note(Severity::Warning, format!("{conflicts} seams join surfaces that face opposite ways"));
        }
        Ok(vec![Data::json(ps.to_json()), Data::report(report)])
    }
}

// ---------------------------------------------------------------------------
// mirror
// ---------------------------------------------------------------------------

fn mirror_plane(params: &Params) -> CookResult<(DVec3, DVec3)> {
    let origin = params.vec3("origin")?;
    let n = match params.str("plane")? {
        "x" => DVec3::X,
        "y" => DVec3::Y,
        "z" => DVec3::Z,
        _ => params.vec3("normal")?,
    };
    Ok((origin, n))
}

/// Make the patch meet its mirror image smoothly: boundary rows lying on the
/// plane are projected onto it and the next row loses its in-plane component
/// perpendicular to the border.
fn symmetrize_patch(s: &mut NurbsSurface, origin: DVec3, n: DVec3, tol: f64) -> usize {
    let mut fixed = 0;
    for side in 0..4 {
        let bi = boundary_indices(s, side);
        if !bi.iter().all(|&i| (s.control_points[i] - origin).dot(n).abs() <= tol) {
            continue;
        }
        let ii = inner_indices(s, side);
        let k = bi.len();
        let base: Vec<DVec3> = bi.iter().map(|&i| s.control_points[i] - n * (s.control_points[i] - origin).dot(n)).collect();
        for q in 0..k {
            s.control_points[bi[q]] = base[q];
        }
        for q in 0..k {
            let t = (base[(q + 1).min(k - 1)] - base[q.saturating_sub(1)]).normalize_or_zero();
            let d = s.control_points[ii[q]] - base[q];
            s.control_points[ii[q]] = base[q] + n * d.dot(n) + t * d.dot(t);
        }
        fixed += 1;
    }
    fixed
}

/// `surface.mirror_surface`
pub struct MirrorSurfaceNode;

impl NodeOp for MirrorSurfaceNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.mirror_surface", "Mirror Surface", SPEC_RE)
            .input(PortSpec::optional("patches", DataKind::Json))
            .input(PortSpec::optional("mesh", DataKind::Geometry))
            .output(PortSpec::new("patches", DataKind::Json))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("plane", "Plane", "x", &["x", "y", "z", "custom"]).help("Mirror plane through the origin: normal to an axis, or a custom normal."))
            .param(ParamSpec::vec3("origin", "Origin", DVec3::ZERO).unit("mm").help("A point on the mirror plane."))
            .param(ParamSpec::vec3("normal", "Normal", DVec3::X).help("Plane normal for the custom plane."))
            .param(ParamSpec::bool("keep_original", "Keep original", true).help("Output the original together with its mirror image."))
            .param(ParamSpec::bool("merge_seam", "Merge seam", true).help("Weld mesh vertices on the plane."))
            .param(ParamSpec::bool("symmetrize_seam", "Symmetrize seam", true).help("Make patches that touch the plane meet their mirror image tangent-continuously."))
            .param(ParamSpec::float("tol_mm", "Tolerance", 1e-4, 1e-9, 1.0).unit("mm").help("Distance within which a border counts as lying on the plane."))
            .help("Mirrors patches and/or a mesh across a plane. Orientation is kept so normals still point outwards. Patch borders lying on the plane can be made tangent-continuous with their mirror image; mesh vertices on the plane are welded.")
            .emulates("mirror / pattern in reverse-engineering tools")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let pj = json_input(inputs, 0);
        let mesh = vernon_graph::node::inputs::opt_geometry(inputs, 1);
        if pj.is_none() && mesh.is_none() {
            return Err(ctx.fail("connect a patch set or a mesh"));
        }
        let (origin, n) = mirror_plane(params)?;
        if n.length_squared() < 1e-24 {
            return Err(ctx.fail("the mirror plane normal must not be zero"));
        }
        let n = n.normalize();
        let tol = positive(ctx, "tol_mm", params.float("tol_mm")?)?;
        let keep = params.bool("keep_original")?;
        let sym = params.bool("symmetrize_seam")?;
        let mut report = Report::new("Mirror");
        let mut out_ps = PatchSet::new(vec![]);
        if let Some(j) = pj {
            let src = PatchSet::from_json(&j).map_err(|e| json_err(ctx, "patch set input", e))?;
            let mut originals: Vec<NurbsSurface> = src.patches.iter().map(|p| p.surface.clone()).collect();
            let mut touched = 0;
            if sym {
                for s in &mut originals {
                    touched += symmetrize_patch(s, origin, n, tol);
                }
            }
            let mirrored: Vec<NurbsSurface> = originals.iter().map(|s| s.mirrored(origin, n)).collect();
            let mut all = if keep { originals } else { Vec::new() };
            all.extend(mirrored);
            out_ps = PatchSet::new(all);
            let seams = find_seams(&out_ps, tol.max(1e-6), 8);
            report.metric("patch_count", Metric::new(out_ps.patches.len() as f64, "count"));
            report.metric("seam_borders_on_plane", Metric::new(touched as f64, "count"));
            if keep {
                report.metric("seam_gap_max_mm", Metric::new(seams.max_gap(), "mm"));
                report.metric("g1_max_deg", Metric::new(seams.max_g1_deg(), "deg"));
                report.metric("free_edges", Metric::new(seams.free_edges.len() as f64, "count"));
            }
        }
        let mut out_mesh = Geometry::new();
        if let Some(m) = mesh {
            check_finite(ctx, &m)?;
            let mut mir = (*m).clone();
            for p in &mut mir.points {
                *p -= n * (2.0 * (*p - origin).dot(n));
            }
            for pr in &mut mir.prims {
                if pr.closed {
                    pr.vertices.reverse();
                }
            }
            if let Some(AttribArray::Vec3(ns)) = mir.point_attribs.get_mut("N") {
                for v in ns.iter_mut() {
                    *v -= n * (2.0 * v.dot(n));
                }
            }
            if let Some(AttribArray::Vec3(ns)) = mir.prim_attribs.get_mut("N") {
                for v in ns.iter_mut() {
                    *v -= n * (2.0 * v.dot(n));
                }
            }
            let mut combined = if keep { (*m).clone() } else { Geometry::new() };
            combined.merge(&mir);
            if params.bool("merge_seam")? {
                let w: Welded = weld(&combined, tol.max(1e-6));
                out_mesh = Geometry::from_polygons(w.points, w.faces);
            } else {
                out_mesh = combined;
            }
            report.metric("mesh_faces", Metric::new(out_mesh.prims.len() as f64, "count"));
            report.metric("mesh_vertices", Metric::new(out_mesh.points.len() as f64, "count"));
        }
        Ok(vec![Data::json(out_ps.to_json()), Data::geometry(out_mesh), Data::report(report)])
    }
}

// ---------------------------------------------------------------------------
// thicken
// ---------------------------------------------------------------------------

/// Thicken a mesh shell. Returns the solid and (volume, min radius estimate).
pub fn thicken_mesh(w: &Welded, d_out: f64, d_in: f64) -> (Geometry, f64) {
    let nv = w.points.len();
    let mut tmp = Geometry::from_polygons(w.points.clone(), w.faces.clone());
    tmp.compute_normals();
    let normals = tmp.point_attribs.vec3s("N").map(<[DVec3]>::to_vec).unwrap_or_else(|_| vec![DVec3::ZERO; nv]);
    // smallest radius of curvature from normal change along edges
    let mut kmax: f64 = 0.0;
    for f in &w.faces {
        for i in 0..f.len() {
            let (a, b) = (f[i] as usize, f[(i + 1) % f.len()] as usize);
            let len = (w.points[a] - w.points[b]).length();
            if len > 1e-12 {
                kmax = kmax.max((normals[a] - normals[b]).length() / len);
            }
        }
    }
    let rmin = if kmax > 1e-12 { 1.0 / kmax } else { f64::INFINITY };
    let mut points = Vec::with_capacity(2 * nv);
    for i in 0..nv {
        points.push(w.points[i] + normals[i] * d_out);
    }
    for i in 0..nv {
        points.push(w.points[i] - normals[i] * d_in);
    }
    let mut g = Geometry::from_polygons(points, w.faces.clone());
    for f in &w.faces {
        let mut r: Vec<u32> = f.iter().map(|&v| v + nv as u32).collect();
        r.reverse();
        g.prims.push(Prim::polygon(r));
    }
    for lp in boundary_loops(&w.faces) {
        let k = lp.len();
        for i in 0..k {
            let (a, b) = (lp[i], lp[(i + 1) % k]);
            g.prims.push(Prim::polygon(vec![a, a + nv as u32, b + nv as u32, b]));
        }
    }
    g.point_attribs.resize_all(g.points.len());
    g.prim_attribs.resize_all(g.prims.len());
    (g, rmin)
}

/// Ruled strip between two boundary curves.
fn ruled_wall(a: &NurbsCurve, b: &NurbsCurve, ncp: usize) -> Option<NurbsSurface> {
    let m = (8 * ncp).max(64);
    let pa = crate::patchnodes::resample(a, m);
    let pb = crate::patchnodes::resample(b, m);
    let ku = clamped_uniform_knots(ncp, 3);
    let params: Vec<f64> = (0..=m).map(|i| i as f64 / m as f64).collect();
    let ca = fit_with_knots(&pa, &params, 3, &ku, None, None)?;
    let cb = fit_with_knots(&pb, &params, 3, &ku, None, None)?;
    let mut cps = Vec::with_capacity(ncp * 2);
    for i in 0..ncp {
        cps.push(ca.control_points[i]);
        cps.push(cb.control_points[i]);
    }
    Some(NurbsSurface::new(3, 1, ku, vec![0.0, 0.0, 1.0, 1.0], ncp, 2, cps))
}

/// `surface.thicken_shell`
pub struct ThickenShellNode;

impl NodeOp for ThickenShellNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.thicken_shell", "Thicken Shell", SPEC_RE)
            .input(PortSpec::optional("shell", DataKind::Geometry))
            .input(PortSpec::optional("patches", DataKind::Json))
            .output(PortSpec::new("solid", DataKind::Geometry))
            .output(PortSpec::new("patches", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("thickness_mm", "Thickness", 3.0, 0.01, 100.0).unit("mm").help("Wall thickness."))
            .param(ParamSpec::choice("side", "Side", "outward", &["outward", "inward", "both"]).help("Which way the wall grows from the surface; both splits it evenly."))
            .param(ParamSpec::float("skin_mm", "Skin thickness", 0.0, 0.0, 50.0).unit("mm").help("Plug mode: thickness of the light composite skin laid over a printed core. The whole wall moves this far inside the input surface so the finished, skinned part lands on the design surface. 0 = off."))
            .param(ParamSpec::float("sanding_mm", "Sanding allowance", 0.0, 0.0, 20.0).unit("mm").help("Plug mode: extra material left under the skin for filler and sanding. Added to the skin thickness when the wall moves inside the input surface."))
            .param(ParamSpec::float("refit_tol_mm", "Refit tolerance", 0.05, 0.001, 5.0).unit("mm").help("Tolerance when offset patches are fitted again."))
            .param(ParamSpec::float("radius_ratio", "Curvature safety", 0.8, 0.1, 1.0).help("Warn when the thickness exceeds this share of the tightest radius of curvature."))
            .param(ParamSpec::float("weld_tol_mm", "Weld tolerance", 1e-6, 0.0, 10.0).unit("mm").help("Vertices closer than this are merged before thickening a mesh."))
            .help("Gives a surface a wall thickness. A mesh shell is offset along its vertex normals and closed with a wall along open borders; a closed mesh becomes a hollow solid. Patches are offset by sampling and fitting again, with ruled walls on free borders. The report holds the volume and the tightest radius of curvature.")
            .emulates("shell / thicken in reverse-engineering tools")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let shell = vernon_graph::node::inputs::opt_geometry(inputs, 0);
        let pj = json_input(inputs, 1);
        if shell.is_none() && pj.is_none() {
            return Err(ctx.fail("connect a mesh shell or a patch set"));
        }
        let t = positive(ctx, "thickness_mm", params.float("thickness_mm")?)?;
        let (d_out, d_in) = match params.str("side")? {
            "inward" => (0.0, t),
            "both" => (t / 2.0, t / 2.0),
            _ => (t, 0.0),
        };
        // plug mode: a printed core under a skin lies inside the design surface by skin + sanding
        let (skin, sanding) = (params.float("skin_mm")?, params.float("sanding_mm")?);
        if !(skin.is_finite() && sanding.is_finite() && skin >= 0.0 && sanding >= 0.0) {
            return Err(ctx.fail("skin thickness and sanding allowance must be zero or positive"));
        }
        let plug = skin + sanding;
        let (d_out, d_in) = (d_out - plug, d_in + plug);
        let ratio = params.float("radius_ratio")?;
        let mut report = Report::new("Thicken shell");
        report.metric("thickness_mm", Metric::new(t, "mm"));
        if plug > 0.0 {
            report.metric("plug_offset_mm", Metric::new(plug, "mm"));
            report.note(
                Severity::Info,
                format!("plug mode: the wall lies {plug:.2} mm ({skin:.2} mm skin + {sanding:.2} mm sanding) inside the input surface; the skin carries the load and the printed core only shapes it"),
            );
        }
        let mut solid = Geometry::new();
        let mut out_ps = PatchSet::new(vec![]);
        if let Some(g) = shell {
            check_finite(ctx, &g)?;
            let mut w = weld(&g, welded_tolerance(ctx, params, &g)?);
            if w.faces.is_empty() {
                return Err(ctx.fail("the shell has no usable faces"));
            }
            orient(&mut w);
            let (sol, rmin) = thicken_mesh(&w, d_out, d_in);
            report.metric("volume_mm3", Metric::new(geometry_volume(&sol), "mm3"));
            report.metric("min_radius_mm", Metric::new(if rmin.is_finite() { rmin } else { 0.0 }, "mm"));
            report.metric("shell_area_mm2", Metric::new(g.surface_area(), "mm2"));
            if t > ratio * rmin {
                report.note(Severity::Warning, format!("the thickness {t} mm exceeds {ratio} of the tightest radius of curvature ({rmin:.2} mm); the offset may fold over or self-intersect"));
            }
            solid = sol;
        }
        if let Some(j) = pj {
            let src = PatchSet::from_json(&j).map_err(|e| json_err(ctx, "patch set input", e))?;
            let tol = positive(ctx, "refit_tol_mm", params.float("refit_tol_mm")?)?;
            let free: std::collections::HashSet<(usize, usize)> = find_seams(&src, 1e-3f64.max(tol), 8).free_edges.into_iter().collect();
            let mut surfaces = Vec::new();
            let mut worst_refit: f64 = 0.0;
            let mut rmin = f64::INFINITY;
            for p in &src.patches {
                let s = &p.surface;
                let (ua, ub) = s.domain_u();
                let (va, vb) = s.domain_v();
                let su = (3 * s.nu).max(12);
                let sv = (3 * s.nv).max(12);
                let mut offsets = [Vec::new(), Vec::new()];
                for (k, d) in [d_out, -d_in].iter().enumerate() {
                    let mut samples = Vec::with_capacity(su * sv);
                    for i in 0..su {
                        for j in 0..sv {
                            let (fu, fv) = (i as f64 / (su - 1) as f64, j as f64 / (sv - 1) as f64);
                            let (u, v) = (ua + (ub - ua) * fu, va + (vb - va) * fv);
                            let n = s.normal(u, v);
                            samples.push(crate::fit::FitSample { u: fu, v: fv, q: s.point(u, v) + n * *d });
                        }
                    }
                    offsets[k] = samples;
                }
                let (nu_cp, nv_cp) = (s.nu.max(4), s.nv.max(4));
                let mut fits = Vec::new();
                for samples in offsets.iter_mut() {
                    let f = crate::fit::fit_adaptive_surface(samples, s.degree_u.max(s.degree_v).clamp(2, 5), nu_cp.min(nv_cp), nu_cp.max(nv_cp) * 2, nu_cp as f64 / nv_cp as f64, tol, 1e-6, 2).ok_or_else(|| ctx.fail("an offset patch could not be fitted"))?;
                    worst_refit = worst_refit.max(f.max_dev);
                    fits.push(f.surface);
                }
                // estimate the smallest curvature radius from normal change over the grid
                for i in 0..su - 1 {
                    for j in 0..sv - 1 {
                        let (fu, fv) = (i as f64 / (su - 1) as f64, j as f64 / (sv - 1) as f64);
                        let (u, v) = (ua + (ub - ua) * fu, va + (vb - va) * fv);
                        let (u2, v2) = (ua + (ub - ua) * (i + 1) as f64 / (su - 1) as f64, va + (vb - va) * (j + 1) as f64 / (sv - 1) as f64);
                        let n0 = s.normal(u, v);
                        for (nn, q) in [(s.normal(u2, v), s.point(u2, v)), (s.normal(u, v2), s.point(u, v2))] {
                            let len = (q - s.point(u, v)).length();
                            if len > 1e-9 {
                                let k = (nn - n0).length() / len;
                                if k > 1e-12 {
                                    rmin = rmin.min(1.0 / k);
                                }
                            }
                        }
                    }
                }
                let top = fits.remove(0);
                let bottom = reversed_u(&fits[0]);
                for side in 0..4 {
                    if free.contains(&(p.id, side)) {
                        let wall = ruled_wall(&top.boundary_curve(side), &fits[0].boundary_curve(side), s.nu.max(s.nv).max(6));
                        if let Some(w) = wall {
                            surfaces.push(w);
                        }
                    }
                }
                surfaces.push(top);
                surfaces.push(bottom);
            }
            let mut ps = PatchSet::new(surfaces);
            ps.solid = false;
            report.metric("patch_count", Metric::new(ps.patches.len() as f64, "count"));
            report.metric("refit_dev_max_mm", Metric::new(worst_refit, "mm").with_limit(tol, true));
            report.metric("patch_min_radius_mm", Metric::new(if rmin.is_finite() { rmin } else { 0.0 }, "mm"));
            if t > ratio * rmin {
                report.note(Severity::Warning, format!("the thickness {t} mm exceeds {ratio} of the tightest radius of curvature ({rmin:.2} mm); the offset may self-intersect"));
            }
            if solid.points.is_empty() {
                solid = ps.tessellate(tol.max(0.05));
            }
            out_ps = ps;
        }
        Ok(vec![Data::geometry(solid), Data::json(out_ps.to_json()), Data::report(report)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{b, cook, f, geo_of, grid, report_of, s, some, sphere, tube, triangulated};
    use std::f64::consts::PI;

    fn face_patch(origin: DVec3, u: DVec3, v: DVec3, size: f64) -> NurbsSurface {
        let mut pts = Vec::new();
        for a in 0..4 {
            for c in 0..4 {
                pts.push(origin + u * (size * a as f64 / 3.0) + v * (size * c as f64 / 3.0));
            }
        }
        NurbsSurface::clamped_uniform(3, 3, 4, 4, pts)
    }

    fn cube_patches(flip_last: bool) -> PatchSet {
        let h = 10.0;
        let (x, y, z) = (DVec3::X, DVec3::Y, DVec3::Z);
        let mut v = vec![
            face_patch(DVec3::ZERO, y, x, h),
            face_patch(DVec3::new(0.0, 0.0, h), x, y, h),
            face_patch(DVec3::ZERO, x, z, h),
            face_patch(DVec3::new(0.0, h, 0.0), z, x, h),
            face_patch(DVec3::ZERO, z, y, h),
            face_patch(DVec3::new(h, 0.0, 0.0), y, z, h),
        ];
        if flip_last {
            let last = v.pop().unwrap();
            v.push(reversed_u(&last));
        }
        PatchSet::new(v)
    }

    #[test]
    fn faceted_cube_golden_numbers() {
        // spec case 11 and the closed-box checks
        let bx = Geometry::make_box(DVec3::new(10.0, 20.0, 30.0));
        let tri = triangulated(&bx);
        let out = cook("surface.convert_faceted", &[some(&tri)], &[("quads_as_faces", b(false))]).unwrap();
        let r = report_of(&out[2]);
        assert_eq!(r.metrics["faces"].value, 12.0);
        assert_eq!(r.metrics["watertight"].value, 1.0);
        assert!((r.metrics["volume_mm3"].value - 6000.0).abs() < 1e-9);
        assert_eq!(r.metrics["orientation_ok"].value, 1.0);
        assert!(r.passes());
        let quads = cook("surface.convert_faceted", &[some(&bx)], &[]).unwrap();
        assert_eq!(report_of(&quads[2]).metrics["faces"].value, 6.0);
        // reversed winding is repaired and reported
        let mut rev = bx.clone();
        for p in &mut rev.prims {
            p.vertices.reverse();
        }
        let fixed = cook("surface.convert_faceted", &[some(&rev)], &[]).unwrap();
        let fr = report_of(&fixed[2]);
        assert!((fr.metrics["volume_mm3"].value - 6000.0).abs() < 1e-9);
        assert_eq!(fr.metrics["orientation_ok"].value, 0.0);
        assert!(!fr.passes());
        // a missing face leaves a shell with four open edges
        let mut open = tri.clone();
        open.prims.pop();
        let o = cook("surface.convert_faceted", &[some(&open)], &[("quads_as_faces", b(false))]).unwrap();
        let or = report_of(&o[2]);
        assert_eq!(or.metrics["watertight"].value, 0.0);
        assert_eq!(or.metrics["boundary_edges"].value, 3.0);
        assert!(cook("surface.convert_faceted", &[some(&bx)], &[("max_faces", f(100.0))]).is_ok());
    }

    #[test]
    fn faceted_box_without_a_quad_has_four_boundary_edges() {
        let bx = Geometry::make_box(DVec3::new(10.0, 20.0, 30.0));
        let mut open = bx.clone();
        open.prims.pop();
        let o = cook("surface.convert_faceted", &[some(&open)], &[]).unwrap();
        let or = report_of(&o[2]);
        assert_eq!(or.metrics["boundary_edges"].value, 4.0);
        assert_eq!(or.metrics["watertight"].value, 0.0);
        assert_eq!(geo_of(&o[0]).prims.len(), 5);
    }

    #[test]
    fn prismatic_lists_one_face_per_fitted_region() {
        // a hollow tube: two cylinder faces; a flat ring: one plane face with a hole loop
        let t = tube(25.0, 5.0, 10.0, 64);
        let seg = cook("surface.segment_regions", &[some(&t)], &[]).unwrap();
        let out = cook("surface.convert_prismatic", &[Some(seg[0].clone())], &[]).unwrap();
        let r = report_of(&out[1]);
        assert_eq!(r.metrics["faces"].value, 2.0);
        assert_eq!(r.metrics["analytic_faces"].value, 2.0);
        assert!(!r.notes.iter().any(|(_, t)| t.starts_with("white-box placeholder")));
        assert_eq!(out.len(), 3);
        let mut ring = Geometry::new();
        for rr in [5.0, 20.0] {
            for k in 0..64 {
                let a = std::f64::consts::TAU * k as f64 / 64.0;
                ring.points.push(DVec3::new(rr * a.cos(), rr * a.sin(), 0.0));
            }
        }
        for k in 0..64u32 {
            ring.prims.push(Prim::polygon(vec![k, (k + 1) % 64, 64 + (k + 1) % 64, 64 + k]));
        }
        let seg = cook("surface.segment_regions", &[some(&ring)], &[]).unwrap();
        let out = cook("surface.convert_prismatic", &[Some(seg[0].clone())], &[]).unwrap();
        let doc = out[0].as_json().unwrap();
        let faces = doc["faces"].as_array().unwrap();
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0]["type"], "plane");
        assert_eq!(faces[0]["inner_loops"].as_array().unwrap().len(), 1);
        assert_eq!(faces[0]["outer_loop"].as_array().unwrap().len(), 64);
        assert!(cook("surface.convert_prismatic", &[some(&t)], &[]).is_ok());
    }

    #[test]
    fn sewing_a_closed_box_is_watertight_and_oriented() {
        let ps = cube_patches(false);
        let out = cook("surface.sew", &[Some(Data::json(ps.to_json()))], &[]).unwrap();
        let r = report_of(&out[1]);
        assert_eq!(r.metrics["watertight"].value, 1.0, "{:?}", r.metrics);
        assert_eq!(r.metrics["orientation_conflicts"].value, 0.0);
        assert_eq!(r.metrics["seam_count"].value, 12.0);
        assert!(PatchSet::from_json(out[0].as_json().unwrap()).unwrap().solid);
        let bad = cube_patches(true);
        let o2 = cook("surface.sew", &[Some(Data::json(bad.to_json()))], &[]).unwrap();
        let r2 = report_of(&o2[1]);
        assert!(r2.metrics["orientation_conflicts"].value >= 1.0);
        assert!(!PatchSet::from_json(o2[0].as_json().unwrap()).unwrap().solid);
    }

    #[test]
    fn sewing_snaps_a_small_gap() {
        let a = face_patch(DVec3::ZERO, DVec3::X, DVec3::Y, 10.0);
        let c = face_patch(DVec3::new(10.02, 0.0, 0.0), DVec3::X, DVec3::Y, 10.0);
        let ps = PatchSet::new(vec![a, c]);
        let out = cook("surface.sew", &[Some(Data::json(ps.to_json()))], &[("tol_mm", f(0.05))]).unwrap();
        let r = report_of(&out[1]);
        assert!(r.metrics["gap_before_mm"].value > 0.009);
        assert!(r.metrics["gap_max_mm"].value < 1e-9, "{:?}", r.metrics["gap_max_mm"]);
        assert_eq!(r.metrics["seam_count"].value, 1.0);
        assert_eq!(r.metrics["watertight"].value, 0.0);
    }

    #[test]
    fn mirror_makes_a_tangent_continuous_seam() {
        // a sloped half panel touching the plane x = 0
        let mut pts = Vec::new();
        for a in 0..4 {
            for c in 0..4 {
                let (x, y) = (10.0 * a as f64 / 3.0, 10.0 * c as f64 / 3.0);
                pts.push(DVec3::new(x, y, 5.0 - 0.8 * x - 0.02 * y * x));
            }
        }
        let half = NurbsSurface::clamped_uniform(3, 3, 4, 4, pts);
        let ps = PatchSet::new(vec![half]);
        let input = Some(Data::json(ps.to_json()));
        let sym = cook("surface.mirror_surface", &[input.clone(), None], &[("plane", s("x"))]).unwrap();
        let r = report_of(&sym[2]);
        assert_eq!(r.metrics["patch_count"].value, 2.0);
        assert!(r.metrics["seam_gap_max_mm"].value <= 1e-6);
        assert!(r.metrics["g1_max_deg"].value <= 0.5, "{}", r.metrics["g1_max_deg"].value);
        let raw = cook("surface.mirror_surface", &[input, None], &[("plane", s("x")), ("symmetrize_seam", b(false))]).unwrap();
        assert!(report_of(&raw[2]).metrics["g1_max_deg"].value > 5.0);
    }

    #[test]
    fn mirror_welds_mesh_vertices_on_the_plane() {
        let half = grid(4, 4, 40.0, 40.0, |_, _| 0.0);
        let out = cook("surface.mirror_surface", &[None, some(&half)], &[("plane", s("x"))]).unwrap();
        let m = geo_of(&out[1]);
        assert_eq!(m.prims.len(), 32);
        assert_eq!(m.points.len(), 25 + 20);
        let mirrored_only = cook("surface.mirror_surface", &[None, some(&half)], &[("plane", s("x")), ("keep_original", b(false))]).unwrap();
        assert!(geo_of(&mirrored_only[1]).points.iter().all(|p| p.x <= 1e-9));
        assert!(cook("surface.mirror_surface", &[None, None], &[]).is_err());
    }

    #[test]
    fn thickening_a_sphere_inward_matches_the_shell_volume() {
        // spec case 13
        let sph = sphere(50.0, 96, 48);
        let out = cook("surface.thicken_shell", &[some(&sph), None], &[("thickness_mm", f(3.0)), ("side", s("inward"))]).unwrap();
        let g = geo_of(&out[0]);
        let rmin = g.points.iter().map(|p| p.length()).fold(f64::INFINITY, f64::min);
        assert!((rmin - 47.0).abs() < 0.05, "inner radius {rmin}");
        let r = report_of(&out[2]);
        let expect = 4.0 / 3.0 * PI * (50f64.powi(3) - 47f64.powi(3));
        let vol = r.metrics["volume_mm3"].value;
        assert!((vol - expect).abs() / expect < 0.005, "volume {vol} vs {expect}");
        assert!(r.notes.is_empty());
        // too thick for the curvature warns
        let thick = cook("surface.thicken_shell", &[some(&sph), None], &[("thickness_mm", f(45.0)), ("side", s("inward"))]).unwrap();
        assert!(!report_of(&thick[2]).notes.is_empty());
    }

    #[test]
    fn plug_mode_moves_the_wall_inside_by_skin_plus_sanding() {
        let sph = sphere(50.0, 96, 48);
        let radii = |out: &[Data]| {
            let g = geo_of(&out[0]);
            let lo = g.points.iter().map(|p| p.length()).fold(f64::INFINITY, f64::min);
            let hi = g.points.iter().map(|p| p.length()).fold(0.0, f64::max);
            (lo, hi)
        };
        let plain = cook("surface.thicken_shell", &[some(&sph), None], &[("thickness_mm", f(3.0)), ("side", s("inward"))]).unwrap();
        let (lo, hi) = radii(&plain);
        assert!((lo - 47.0).abs() < 0.05 && (hi - 50.0).abs() < 0.05, "{lo} {hi}");
        assert!(!report_of(&plain[2]).metrics.contains_key("plug_offset_mm"));
        // a 1 mm skin and 0.5 mm of sanding allowance: the 3 mm wall sits 1.5 mm deeper
        let plug = cook("surface.thicken_shell", &[some(&sph), None], &[("thickness_mm", f(3.0)), ("side", s("inward")), ("skin_mm", f(1.0)), ("sanding_mm", f(0.5))]).unwrap();
        let (lo, hi) = radii(&plug);
        assert!((lo - 45.5).abs() < 0.05 && (hi - 48.5).abs() < 0.05, "{lo} {hi}");
        let r = report_of(&plug[2]);
        assert!((r.metrics["plug_offset_mm"].value - 1.5).abs() < 1e-12);
        // the wall keeps its thickness, so the volume follows the shifted radii
        let expect = 4.0 / 3.0 * PI * (48.5f64.powi(3) - 45.5f64.powi(3));
        assert!((r.metrics["volume_mm3"].value - expect).abs() / expect < 0.005);
        // outward walls move too: outer face at design + 3 - 1.5
        let out = cook("surface.thicken_shell", &[some(&sph), None], &[("thickness_mm", f(3.0)), ("skin_mm", f(1.0)), ("sanding_mm", f(0.5))]).unwrap();
        let (lo, hi) = radii(&out);
        assert!((lo - 48.5).abs() < 0.05 && (hi - 51.5).abs() < 0.05, "{lo} {hi}");
    }

    #[test]
    fn thickening_an_open_shell_closes_it_with_walls() {
        let sheet = grid(8, 8, 40.0, 40.0, |_, _| 0.0);
        let out = cook("surface.thicken_shell", &[some(&sheet), None], &[("thickness_mm", f(2.0)), ("side", s("both"))]).unwrap();
        let g = geo_of(&out[0]);
        assert_eq!(g.prims.len(), 64 + 64 + 32);
        let vol = crate::topology::geometry_volume(&g);
        assert!((vol - 40.0 * 40.0 * 2.0).abs() < 1e-6, "{vol}");
        let z: Vec<f64> = g.points.iter().map(|p| p.z).collect();
        assert!(z.iter().cloned().fold(f64::NEG_INFINITY, f64::max) > 0.99 && z.iter().cloned().fold(f64::INFINITY, f64::min) < -0.99);
    }

    #[test]
    fn thickening_a_patch_adds_two_sheets_and_four_walls() {
        let p = face_patch(DVec3::ZERO, DVec3::X, DVec3::Y, 30.0);
        let ps = PatchSet::new(vec![p]);
        let out = cook("surface.thicken_shell", &[None, Some(Data::json(ps.to_json()))], &[("thickness_mm", f(3.0))]).unwrap();
        let res = PatchSet::from_json(out[1].as_json().unwrap()).unwrap();
        assert_eq!(res.patches.len(), 6);
        let r = report_of(&out[2]);
        assert!(r.metrics["refit_dev_max_mm"].value < 1e-6);
        let tops: Vec<f64> = res.patches.iter().filter(|p| p.surface.degree_v == 3).map(|p| p.surface.point(0.5, 0.5).z).collect();
        assert!(tops.iter().any(|z| (z - 3.0).abs() < 1e-6) && tops.iter().any(|z| z.abs() < 1e-6));
    }
}


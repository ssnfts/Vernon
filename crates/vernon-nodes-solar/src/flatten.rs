//! Stage 7b nodes: seam cut, conformal / stretch-aware flatten, strain
//! report, UV stretch overlay and map-back.

use std::collections::BTreeSet;

use glam::{DVec2, DVec3};
use vernon_core::{AttribArray, Geometry, Metric, Prim, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{
    CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params,
    PortSpec, Stage,
};

use crate::lscm::{align_uv, arap, compact, lscm, plane_fit, ArapOpts, Compact};
use crate::strain::{angle_defect_sum, compute_strain, StrainOpts, StrainResult};
use crate::topo::{analyze, auto_cut, cut_along, seam_from_polylines, TopoInfo};
use crate::util::*;

fn mesh_in() -> PortSpec {
    PortSpec::new("mesh", DataKind::Geometry)
}

fn seam_param() -> ParamSpec {
    ParamSpec::float("seam_tolerance_mm", "Seam tolerance", 0.1, 0.0, 1000.0)
        .unit("mm")
        .help("How far a seam polyline vertex may be from the mesh.")
}

/// Cut a mesh into a disk using optional user seams, then automatic seams.
fn prepare_disk(
    ctx: &CookContext,
    g: &Geometry,
    seam: Option<&Geometry>,
    auto: bool,
    tol: f64,
) -> CookResult<(Geometry, Vec<Vec<DVec3>>, TopoInfo)> {
    let mut cur = g.clone();
    let mut paths = Vec::new();
    if let Some(s) = seam {
        let edges = seam_from_polylines(&cur, s, tol).map_err(|e| ctx.fail(e))?;
        if !edges.is_empty() {
            for pr in s.prims.iter().filter(|p| p.vertices.len() >= 2) {
                paths.push(pr.vertices.iter().map(|&i| s.points[i as usize]).collect());
            }
            cur = cut_along(&cur, &edges);
        }
    }
    let mut topo = analyze(&cur);
    if !topo.is_disk() {
        if !auto {
            return Err(ctx.fail(format!(
                "the mesh is not a disk ({} boundary loops, Euler characteristic {}); connect a seam or enable automatic seams",
                topo.loops.len(),
                topo.euler
            )));
        }
        let (c2, p2) = auto_cut(&cur).map_err(|e| ctx.fail(e))?;
        cur = c2;
        paths.extend(p2);
        topo = analyze(&cur);
    }
    Ok((cur, paths, topo))
}

fn polylines_geometry(paths: &[Vec<DVec3>]) -> Geometry {
    let mut g = Geometry::new();
    for p in paths {
        let ids: Vec<u32> = p.iter().map(|&x| g.add_point(x)).collect();
        if ids.len() >= 2 {
            g.add_prim(Prim::polyline(ids));
        }
    }
    g
}

fn to_orig(c: &Compact, uvc: &[DVec2], n: usize) -> Vec<DVec2> {
    let mut uv = vec![DVec2::ZERO; n];
    for (ci, &o) in c.used.iter().enumerate() {
        uv[o as usize] = uvc[ci];
    }
    uv
}

fn set_uv(g: &mut Geometry, uv: &[DVec2], scale: f64) {
    g.point_attribs.set(
        "uv",
        AttribArray::Vec3(
            uv.iter()
                .map(|u| DVec3::new(u.x * scale, u.y * scale, 0.0))
                .collect(),
        ),
    );
}

fn pattern_of(g: &Geometry, uv: &[DVec2]) -> Geometry {
    let mut p = g.clone();
    p.points = uv.iter().map(|u| DVec3::new(u.x, u.y, 0.0)).collect();
    p
}

/// Write per-face strain attributes and a diverging colour ramp.
fn write_strain(out: &mut Geometry, g: &Geometry, res: &StrainResult, range_pct: f64, mode: &str) {
    let val: Vec<f64> = match mode {
        "stretch" => res.prim_stretch.clone(),
        "compress" => res.prim_compress.iter().map(|c| -c).collect(),
        "area" => res.prim_area_change.clone(),
        _ => res.prim_strain.clone(),
    };
    let range = range_pct.max(1e-9);
    out.prim_attribs
        .set("stretch_pct", AttribArray::Float(res.prim_stretch.clone()));
    out.prim_attribs.set(
        "compress_pct",
        AttribArray::Float(res.prim_compress.clone()),
    );
    out.prim_attribs.set(
        "area_change_pct",
        AttribArray::Float(res.prim_area_change.clone()),
    );
    out.prim_attribs
        .set("strain_pct", AttribArray::Float(res.prim_strain.clone()));
    out.prim_attribs.set(
        "Cd",
        AttribArray::Vec3(val.iter().map(|v| ramp_div(v / range)).collect()),
    );
    let pv = prim_to_point(g, &val);
    out.point_attribs.set(
        "Cd",
        AttribArray::Vec3(pv.iter().map(|v| ramp_div(v / range)).collect()),
    );
    out.point_attribs.set(
        "stretch_pct",
        AttribArray::Float(prim_to_point(g, &res.prim_stretch)),
    );
    out.point_attribs.set(
        "compress_pct",
        AttribArray::Float(prim_to_point(g, &res.prim_compress)),
    );
    out.point_attribs.set(
        "strain_pct",
        AttribArray::Float(prim_to_point(g, &res.prim_strain)),
    );
}

fn strain_metrics(r: &mut Report, res: &StrainResult, limit_pct: f64) {
    let s = &res.stats;
    r.metric("area_change_pct", Metric::new(s.area_change_pct, "%"));
    r.metric("stretch_avg_pct", Metric::new(s.stretch_avg_pct, "%"));
    r.metric("stretch_max_pct", Metric::new(s.stretch_max_pct, "%"));
    r.metric("stretch_share_pct", Metric::new(s.stretch_share_pct, "%"));
    r.metric("compress_avg_pct", Metric::new(s.compress_avg_pct, "%"));
    r.metric("compress_max_pct", Metric::new(s.compress_max_pct, "%"));
    r.metric("compress_share_pct", Metric::new(s.compress_share_pct, "%"));
    r.metric(
        "max_length_change_pct",
        Metric::new(s.max_length_change_pct, "%").with_limit(limit_pct, true),
    );
    r.metric(
        "flipped_tris",
        Metric::new(s.flipped as f64, "count").with_limit(0.0, true),
    );
    r.metric("degenerate_tris", Metric::new(s.degenerate as f64, "count"));
    if s.flipped > 0 {
        r.note(
            Severity::Error,
            format!(
                "{} pattern triangles are folded over; the pattern overlaps itself",
                s.flipped
            ),
        );
    }
    if s.max_length_change_pct > 10.0 {
        r.note(
            Severity::Warning,
            "length changes above about 10 percent: a flat pattern is a poor model of this panel",
        );
    }
    r.note(
        Severity::Info,
        "strain model ignores sheet thickness, drape and compressibility",
    );
}

fn marker_geometry(res: &StrainResult) -> Geometry {
    let mut g = Geometry::new();
    let mut val = Vec::new();
    let mut cd = Vec::new();
    for (p, v) in &res.markers {
        g.add_point(*p);
        val.push(*v);
        cd.push(if *v < 0.0 {
            DVec3::new(0.9, 0.1, 0.1)
        } else {
            DVec3::new(0.1, 0.7, 0.2)
        });
    }
    g.point_attribs.set("strain_pct", AttribArray::Float(val));
    g.point_attribs.set("Cd", AttribArray::Vec3(cd));
    g
}

// ---------------------------------------------------------------------------
// flatten.seam_cut
// ---------------------------------------------------------------------------

pub struct SeamCutNode;

impl NodeOp for SeamCutNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("flatten.seam_cut", "Cut Flattening Seams", Stage::Flatten)
            .input(mesh_in())
            .input(PortSpec::optional("seam", DataKind::Geometry))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("seam", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::bool("auto", "Automatic seams", true).help("After the user seams, add shortest-path cuts until the mesh is a single disk."))
            .param(seam_param())
            .help("Cuts a mesh along seam polylines and/or automatic shortest paths so it becomes a topological disk that can be flattened. Cut points are duplicated; the original index is kept in the src_point attribute.")
            .emulates("Seam splitting before surface flattening")
            .spec(FLATTEN_SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        if face_count(&g) == 0 {
            return Err(ctx.fail("the mesh has no faces"));
        }
        let seam = inputs::opt_geometry(ins, 1);
        let before = analyze(&g);
        let auto = p.bool("auto")?;
        let (cut, paths, after) = if seam.is_none() && (!auto || before.is_disk()) {
            ((*g).clone(), Vec::new(), before.clone())
        } else {
            prepare_disk(
                ctx,
                &g,
                seam.as_deref(),
                auto,
                p.float("seam_tolerance_mm")?,
            )?
        };
        let mut r = Report::new("Seam cut");
        r.metric(
            "boundary_loops_before",
            Metric::new(before.loops.len() as f64, "count"),
        );
        r.metric("euler_before", Metric::new(before.euler as f64, ""));
        r.metric(
            "boundary_loops",
            Metric::new(after.loops.len() as f64, "count"),
        );
        r.metric("euler_char", Metric::new(after.euler as f64, ""));
        r.metric("seam_paths", Metric::new(paths.len() as f64, "count"));
        r.metric(
            "duplicated_points",
            Metric::new(
                after.vertices.saturating_sub(before.vertices) as f64,
                "count",
            ),
        );
        r.metric(
            "is_disk",
            Metric::new(if after.is_disk() { 1.0 } else { 0.0 }, "").with_limit(1.0, false),
        );
        if after.is_disk() && paths.is_empty() {
            r.note(
                Severity::Info,
                "the mesh is already a disk; nothing was cut",
            );
        }
        if !after.is_disk() {
            r.note(
                Severity::Warning,
                "the result is not a single disk; add seam polylines or enable automatic seams",
            );
        }
        Ok(vec![
            Data::geometry(cut),
            Data::geometry(polylines_geometry(&paths)),
            Data::report(r),
        ])
    }
}

// ---------------------------------------------------------------------------
// flatten.conformal_map
// ---------------------------------------------------------------------------

pub struct ConformalMapNode;

impl NodeOp for ConformalMapNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("flatten.conformal_map", "Conformal Flatten", Stage::Flatten)
            .input(mesh_in())
            .input(PortSpec::optional("seam", DataKind::Geometry))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("pattern", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("method", "Method", "conformal", &["conformal", "plane_fit"]).help("Least-squares conformal map, or orthogonal projection onto the best-fit plane (nearly flat panels)."))
            .param(ParamSpec::bool("auto_seam", "Automatic seams", true).help("Cut the mesh into a disk first when it is not one."))
            .param(seam_param())
            .param(ParamSpec::bool("align_to_axes", "Align to axes", true).help("Rotate the pattern so its long axis lies along X and move it to the origin."))
            .param(ParamSpec::choice("outside", "Outside", "up", &["up", "down"]).help("Which face of the sheet shows in the pattern; down mirrors it."))
            .param(ParamSpec::bool("normalize_uv", "Normalise UV", false).help("Write the uv attribute scaled to a unit square. The pattern geometry stays in millimetres."))
            .param(ParamSpec::float("deadband_percent", "Strain dead-band", 0.01, 0.0, 10.0).unit("%"))
            .param(ParamSpec::float("max_strain_pct", "Strain limit", 5.0, 0.0, 100.0).unit("%").help("Pass limit on the worst length change in the report."))
            .param(ParamSpec::float("solver_tolerance", "Solver tolerance", 1.0e-10, 1.0e-14, 1.0e-3).help("Relative residual of the conjugate-gradient solve."))
            .param(ParamSpec::int("max_iterations", "Max solver iterations", 20000, 10, 1_000_000))
            .help("Flattens a panel to 2-D. The conformal method keeps angles and rescales to the 3-D area. Writes the uv attribute in millimetres, the flat pattern, and a report of the worst length change.")
            .emulates("Least-squares conformal map (Levy et al. 2002) as initial flattening of a stretch-aware squish")
            .spec(FLATTEN_SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        if face_count(&g) == 0 {
            return Err(ctx.fail("the mesh has no faces"));
        }
        let seam = inputs::opt_geometry(ins, 1);
        let method = p.str("method")?.to_string();
        let mut r = Report::new("Flatten");
        let (geo, paths, topo) = match method.as_str() {
            "conformal" => prepare_disk(
                ctx,
                &g,
                seam.as_deref(),
                p.bool("auto_seam")?,
                p.float("seam_tolerance_mm")?,
            )?,
            "plane_fit" => ((*g).clone(), Vec::new(), analyze(&g)),
            other => return Err(CookError::BadParam(format!("method = {other}"))),
        };
        let comp = compact(&geo);
        let mirror = p.str("outside")? == "down";
        let mut uvc = match method.as_str() {
            "conformal" => {
                let bl: Vec<u32> = topo.loops[0]
                    .iter()
                    .map(|&v| comp.map[v as usize])
                    .collect();
                let (uv, it, rel) = lscm(
                    &comp,
                    &bl,
                    p.float("solver_tolerance")?,
                    p.int("max_iterations")?.max(10) as usize,
                )
                .map_err(|e| ctx.fail(e))?;
                r.metric("solver_iterations", Metric::new(it as f64, "count"));
                r.metric("solver_residual", Metric::new(rel, ""));
                if rel > 1e-6 {
                    r.note(
                        Severity::Warning,
                        "the solver stopped before converging; raise the iteration limit",
                    );
                }
                uv
            }
            _ => plane_fit(&comp)
                .ok_or_else(|| ctx.fail("the points are collinear or too few for a plane fit"))?,
        };
        if p.bool("align_to_axes")? {
            align_uv(&mut uvc, mirror);
        } else if mirror {
            for u in uvc.iter_mut() {
                u.y = -u.y;
            }
        }
        let uv = to_orig(&comp, &uvc, geo.points.len());
        let res = compute_strain(
            &geo,
            &uv,
            &StrainOpts {
                deadband_pct: p.float("deadband_percent")?,
                max_markers: 10,
            },
        );
        strain_metrics(&mut r, &res, p.float("max_strain_pct")?);
        r.metric(
            "boundary_loops",
            Metric::new(topo.loops.len() as f64, "count"),
        );
        r.metric("euler_char", Metric::new(topo.euler as f64, ""));
        r.metric("seam_paths", Metric::new(paths.len() as f64, "count"));
        let defect = angle_defect_sum(&geo, &topo);
        r.metric("angle_defect_rad", Metric::new(defect, "rad"));
        if defect < 1e-3 {
            r.note(
                Severity::Info,
                "the surface is developable: a plain unroll gives zero strain",
            );
        }
        let (mut u_min, mut u_max) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
        for u in &uvc {
            u_min = u_min.min(*u);
            u_max = u_max.max(*u);
        }
        let size = u_max - u_min;
        r.metric("pattern_width_mm", Metric::new(size.x, "mm"));
        r.metric("pattern_height_mm", Metric::new(size.y, "mm"));
        let scale = if p.bool("normalize_uv")? {
            1.0 / size.x.max(size.y).max(1e-12)
        } else {
            1.0
        };
        let mut out3 = geo.clone();
        set_uv(&mut out3, &uv, scale);
        write_strain(&mut out3, &geo, &res, p.float("max_strain_pct")?, "signed");
        let mut pat = pattern_of(&geo, &uv);
        set_uv(&mut pat, &uv, scale);
        write_strain(&mut pat, &geo, &res, p.float("max_strain_pct")?, "signed");
        Ok(vec![
            Data::geometry(out3),
            Data::geometry(pat),
            Data::report(r),
        ])
    }
}

// ---------------------------------------------------------------------------
// flatten.stretch_aware
// ---------------------------------------------------------------------------

pub struct StretchAwareNode;

impl NodeOp for StretchAwareNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("flatten.stretch_aware", "Stretch-Aware Flatten", Stage::Flatten)
            .input(mesh_in())
            .input(PortSpec::optional("seam", DataKind::Geometry))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("pattern", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(
                ParamSpec::choice(
                    "deformation",
                    "Deformation",
                    "free",
                    &[
                        "free",
                        "stretch_mostly",
                        "stretch_only",
                        "compress_mostly",
                        "compress_only",
                        "custom_a",
                        "custom_b",
                        "custom_c",
                    ],
                )
                .help("Bias between stretching and compressing the sheet."),
            )
            .param(ParamSpec::float("bnd_stretch", "Boundary stretch factor", 1.0, 0.01, 1000.0).help("Higher means less stretch of boundary edges."))
            .param(ParamSpec::float("bnd_compress", "Boundary compress factor", 1.0, 0.01, 1000.0).help("Higher means less compression of boundary edges."))
            .param(ParamSpec::float("int_stretch", "Interior stretch factor", 1.0, 0.01, 1000.0).help("Higher means less stretch of interior edges."))
            .param(ParamSpec::float("int_compress", "Interior compress factor", 1.0, 0.01, 1000.0).help("Higher means less compression of interior edges."))
            .param(ParamSpec::choice("material", "Material", "rigid", &["rigid", "floppy"]).help("Stiff sheet or pliable fabric."))
            .param(ParamSpec::bool("preserve_boundary", "Preserve boundary", false).help("Weight boundary edges heavily so their lengths change little."))
            .param(ParamSpec::choice("outside", "Outside", "up", &["up", "down"]))
            .param(ParamSpec::bool("auto_seam", "Automatic seams", true))
            .param(seam_param())
            .param(ParamSpec::int("max_iterations", "Max iterations", 200, 1, 5000))
            .param(ParamSpec::float("tolerance", "Energy tolerance", 1.0e-6, 1.0e-12, 1.0e-1).help("Stop when the relative energy change falls below this."))
            .param(ParamSpec::float("max_strain_pct", "Strain limit", 10.0, 0.0, 100.0).unit("%"))
            .param(ParamSpec::float("deadband_percent", "Strain dead-band", 0.01, 0.0, 10.0).unit("%"))
            .help("Flattens a doubly curved panel by minimising edge-length change from a conformal start, with optional bias towards stretching or compressing. Reports stretch and compression statistics and the worst length change.")
            .emulates("Stretch-aware surface flattening (squish) from published as-rigid-as-possible methods")
            .spec(FLATTEN_SPEC)
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        if face_count(&g) == 0 {
            return Err(ctx.fail("the mesh has no faces"));
        }
        let seam = inputs::opt_geometry(ins, 1);
        let (geo, paths, topo) = prepare_disk(
            ctx,
            &g,
            seam.as_deref(),
            p.bool("auto_seam")?,
            p.float("seam_tolerance_mm")?,
        )?;
        let comp = compact(&geo);
        let bl: Vec<u32> = topo.loops[0]
            .iter()
            .map(|&v| comp.map[v as usize])
            .collect();
        let (uv0, _, rel) = lscm(&comp, &bl, 1e-10, 20000).map_err(|e| ctx.fail(e))?;
        let mut o = ArapOpts {
            max_iter: p.int("max_iterations")?.max(1) as usize,
            tol: p.float("tolerance")?,
            preserve_boundary: p.bool("preserve_boundary")?,
            bnd_stretch: p.float("bnd_stretch")?,
            bnd_compress: p.float("bnd_compress")?,
            int_stretch: p.float("int_stretch")?,
            int_compress: p.float("int_compress")?,
            ..Default::default()
        };
        match p.str("deformation")? {
            "free" | "custom_a" | "custom_b" | "custom_c" => {}
            "stretch_mostly" => o.base_compress = 10.0,
            "compress_mostly" => o.base_stretch = 10.0,
            "stretch_only" => o.base_compress = 1.0e4,
            "compress_only" => o.base_stretch = 1.0e4,
            other => return Err(CookError::BadParam(format!("deformation = {other}"))),
        }
        let out = arap(&comp, &uv0, &o);
        let mut uvc = out.uv;
        align_uv(&mut uvc, p.str("outside")? == "down");
        let uv = to_orig(&comp, &uvc, geo.points.len());
        let sopts = StrainOpts {
            deadband_pct: p.float("deadband_percent")?,
            max_markers: 10,
        };
        let res = compute_strain(&geo, &uv, &sopts);
        let init = compute_strain(&geo, &to_orig(&comp, &uv0, geo.points.len()), &sopts);
        let mut r = Report::new("Stretch-aware flatten");
        strain_metrics(&mut r, &res, p.float("max_strain_pct")?);
        r.metric(
            "arap_iterations",
            Metric::new(out.iterations as f64, "count"),
        );
        r.metric("arap_energy", Metric::new(out.energy, ""));
        r.metric(
            "initial_max_length_change_pct",
            Metric::new(init.stats.max_length_change_pct, "%"),
        );
        r.metric("conformal_residual", Metric::new(rel, ""));
        r.metric("seam_paths", Metric::new(paths.len() as f64, "count"));
        placeholder(
            &mut r,
            "rigid/floppy material weighting, hard stretch-only and compress-only constraints and the fold-free line search; \
             the edge-length energy with reweighted stretch/compress factors is used instead",
        );
        let mut out3 = geo.clone();
        set_uv(&mut out3, &uv, 1.0);
        write_strain(&mut out3, &geo, &res, p.float("max_strain_pct")?, "signed");
        let mut pat = pattern_of(&geo, &uv);
        set_uv(&mut pat, &uv, 1.0);
        write_strain(&mut pat, &geo, &res, p.float("max_strain_pct")?, "signed");
        Ok(vec![
            Data::geometry(out3),
            Data::geometry(pat),
            Data::report(r),
        ])
    }
}

// ---------------------------------------------------------------------------
// flatten.strain_report and flatten.uv_stretch_overlay
// ---------------------------------------------------------------------------

/// The pattern coordinates: the second input's XY, else the `uv` attribute.
fn pattern_uv(ctx: &CookContext, g: &Geometry, pat: Option<&Geometry>) -> CookResult<Vec<DVec2>> {
    match pat {
        Some(pg) => {
            if pg.points.len() != g.points.len() {
                return Err(ctx.fail("the pattern and the 3-D mesh must have the same points"));
            }
            Ok(pg.points.iter().map(|p| DVec2::new(p.x, p.y)).collect())
        }
        None => {
            let uv = g
                .point_attribs
                .vec3s("uv")
                .map_err(|_| ctx.fail("no pattern connected and the mesh has no uv attribute"))?;
            Ok(uv.iter().map(|p| DVec2::new(p.x, p.y)).collect())
        }
    }
}

pub struct StrainReportNode;

impl NodeOp for StrainReportNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("flatten.strain_report", "Flatten Strain Report", Stage::Flatten)
            .input(mesh_in())
            .input(PortSpec::optional("pattern", DataKind::Geometry))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("markers", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("deadband_percent", "Dead-band", 0.01, 0.0, 10.0).unit("%").help("Length changes below this do not count as stretch or compression."))
            .param(ParamSpec::int("max_markers", "Markers", 10, 0, 100).help("Number of worst-strain markers."))
            .param(ParamSpec::float("limit_pct", "Limit", 5.0, 0.0, 100.0).unit("%").help("Pass limit on the worst length change."))
            .param(ParamSpec::float("range_pct", "Colour range", 5.0, 0.01, 100.0).unit("%"))
            .help("Compares a flat pattern with its 3-D surface: area change, stretch and compression (average, worst, share of area), folded triangles and markers at the worst places.")
            .emulates("Surface flattening deformation report (stretch, compression, markers)")
            .spec(FLATTEN_SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        if face_count(&g) == 0 {
            return Err(ctx.fail("the mesh has no faces"));
        }
        let pat = inputs::opt_geometry(ins, 1);
        let uv = pattern_uv(ctx, &g, pat.as_deref())?;
        let res = compute_strain(
            &g,
            &uv,
            &StrainOpts {
                deadband_pct: p.float("deadband_percent")?,
                max_markers: p.int("max_markers")?.max(0) as usize,
            },
        );
        let mut r = Report::new("Flatten strain");
        strain_metrics(&mut r, &res, p.float("limit_pct")?);
        let topo = analyze(&g);
        let defect = angle_defect_sum(&g, &topo);
        r.metric("angle_defect_rad", Metric::new(defect, "rad"));
        if defect < 1e-3 {
            r.note(
                Severity::Info,
                "the surface is developable: a plain unroll gives zero strain",
            );
        }
        let mut out = (*g).clone();
        write_strain(&mut out, &g, &res, p.float("range_pct")?, "signed");
        Ok(vec![
            Data::geometry(out),
            Data::geometry(marker_geometry(&res)),
            Data::report(r),
        ])
    }
}

pub struct UvOverlayNode;

impl NodeOp for UvOverlayNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("flatten.uv_stretch_overlay", "UV Stretch Overlay", Stage::Flatten)
            .input(mesh_in())
            .input(PortSpec::optional("pattern", DataKind::Geometry))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("pattern", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("mode", "Show", "signed", &["signed", "stretch", "compress", "area"]).help("Signed worst length change, stretch only, compression only, or area change."))
            .param(ParamSpec::float("range_pct", "Colour range", 5.0, 0.01, 100.0).unit("%").help("Strain that maps to full colour."))
            .param(ParamSpec::float("deadband_percent", "Dead-band", 0.01, 0.0, 10.0).unit("%"))
            .help("Colours a 3-D mesh and its flat pattern by the stretch of its UV map: red where the sheet is stretched, blue where it is compressed, white where lengths are kept.")
            .emulates("UV stretch visualisation (checker and stretch overlays)")
            .spec(FLATTEN_SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        if face_count(&g) == 0 {
            return Err(ctx.fail("the mesh has no faces"));
        }
        let pat = inputs::opt_geometry(ins, 1);
        let uv = pattern_uv(ctx, &g, pat.as_deref())?;
        let res = compute_strain(
            &g,
            &uv,
            &StrainOpts {
                deadband_pct: p.float("deadband_percent")?,
                max_markers: 0,
            },
        );
        let mode = p.str("mode")?.to_string();
        if !["signed", "stretch", "compress", "area"].contains(&mode.as_str()) {
            return Err(CookError::BadParam(format!("mode = {mode}")));
        }
        let mut out = (*g).clone();
        write_strain(&mut out, &g, &res, p.float("range_pct")?, &mode);
        let mut flat = pattern_of(&g, &uv);
        write_strain(&mut flat, &g, &res, p.float("range_pct")?, &mode);
        let mut r = Report::new("UV stretch");
        strain_metrics(&mut r, &res, p.float("range_pct")?);
        Ok(vec![
            Data::geometry(out),
            Data::geometry(flat),
            Data::report(r),
        ])
    }
}

// ---------------------------------------------------------------------------
// flatten.map_back
// ---------------------------------------------------------------------------

struct Locator {
    tri2: Vec<[DVec2; 3]>,
    tri3: Vec<[DVec3; 3]>,
    origin: DVec2,
    cell: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

impl Locator {
    fn build(pat: &Geometry, src: &Geometry) -> Locator {
        let mut tri2 = Vec::new();
        let mut tri3 = Vec::new();
        for (_, t) in pat.triangles() {
            tri2.push(
                [0, 1, 2]
                    .map(|k| DVec2::new(pat.points[t[k] as usize].x, pat.points[t[k] as usize].y)),
            );
            tri3.push([0, 1, 2].map(|k| src.points[t[k] as usize]));
        }
        let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
        for t in &tri2 {
            for v in t {
                lo = lo.min(*v);
                hi = hi.max(*v);
            }
        }
        let size = (hi - lo).max(DVec2::splat(1e-9));
        let n = tri2.len().max(1) as f64;
        let mut cell = (size.x * size.y / n).sqrt() * 2.0;
        if !cell.is_finite() || cell <= 0.0 {
            cell = size.x.max(size.y);
        }
        cell = cell.max(size.x.max(size.y) / 512.0);
        let nx = ((size.x / cell).ceil() as usize + 1).max(1);
        let ny = ((size.y / cell).ceil() as usize + 1).max(1);
        let mut cells = vec![Vec::new(); nx * ny];
        for (ti, t) in tri2.iter().enumerate() {
            let tlo = t[0].min(t[1]).min(t[2]);
            let thi = t[0].max(t[1]).max(t[2]);
            let (i0, j0) = (
                ((tlo.x - lo.x) / cell) as usize,
                ((tlo.y - lo.y) / cell) as usize,
            );
            let (i1, j1) = (
                (((thi.x - lo.x) / cell) as usize).min(nx - 1),
                (((thi.y - lo.y) / cell) as usize).min(ny - 1),
            );
            for j in j0..=j1 {
                for i in i0..=i1 {
                    cells[j * nx + i].push(ti as u32);
                }
            }
        }
        Locator {
            tri2,
            tri3,
            origin: lo,
            cell,
            nx,
            ny,
            cells,
        }
    }

    fn cell_of(&self, p: DVec2) -> Option<usize> {
        let (fx, fy) = (
            (p.x - self.origin.x) / self.cell,
            (p.y - self.origin.y) / self.cell,
        );
        if fx < -1e-9 || fy < -1e-9 {
            return None;
        }
        let (i, j) = (fx.max(0.0) as usize, fy.max(0.0) as usize);
        if i >= self.nx || j >= self.ny {
            return None;
        }
        Some(j * self.nx + i)
    }

    fn bary(t: &[DVec2; 3], p: DVec2) -> Option<[f64; 3]> {
        let d = (t[1] - t[0]).perp_dot(t[2] - t[0]);
        if d.abs() < 1e-18 {
            return None;
        }
        let b1 = (p - t[0]).perp_dot(t[2] - t[0]) / d;
        let b2 = (t[1] - t[0]).perp_dot(p - t[0]) / d;
        Some([1.0 - b1 - b2, b1, b2])
    }

    /// Containing triangle and barycentric coordinates.
    fn locate(&self, p: DVec2, tol: f64) -> Option<(usize, [f64; 3])> {
        let c = self.cell_of(p)?;
        for &ti in &self.cells[c] {
            let t = &self.tri2[ti as usize];
            let Some(b) = Self::bary(t, p) else { continue };
            let size = (t[1] - t[0])
                .length()
                .max((t[2] - t[0]).length())
                .max(1e-12);
            let eps = 1e-9 + tol / size;
            if b.iter().all(|&x| x >= -eps) {
                return Some((ti as usize, b));
            }
        }
        None
    }

    fn lift(&self, ti: usize, b: [f64; 3]) -> DVec3 {
        let t = &self.tri3[ti];
        t[0] * b[0] + t[1] * b[1] + t[2] * b[2]
    }

    /// Triangles whose grid cells are touched by the segment's bounding box.
    fn candidates(&self, a: DVec2, b: DVec2) -> Vec<u32> {
        let lo = a.min(b);
        let hi = a.max(b);
        let clamp = |v: f64, n: usize| -> usize { (v.max(0.0) as usize).min(n - 1) };
        let (i0, j0) = (
            clamp((lo.x - self.origin.x) / self.cell, self.nx),
            clamp((lo.y - self.origin.y) / self.cell, self.ny),
        );
        let (i1, j1) = (
            clamp((hi.x - self.origin.x) / self.cell, self.nx),
            clamp((hi.y - self.origin.y) / self.cell, self.ny),
        );
        let mut set = BTreeSet::new();
        for j in j0..=j1 {
            for i in i0..=i1 {
                set.extend(self.cells[j * self.nx + i].iter().copied());
            }
        }
        set.into_iter().collect()
    }
}

fn seg_param(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> Option<f64> {
    // Parameter t on a-b where it meets the segment c-d.
    let r = b - a;
    let s = d - c;
    let den = r.perp_dot(s);
    if den.abs() < 1e-18 {
        return None;
    }
    let t = (c - a).perp_dot(s) / den;
    let u = (c - a).perp_dot(r) / den;
    if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
        Some(t)
    } else {
        None
    }
}

pub struct MapBackNode;

impl NodeOp for MapBackNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("flatten.map_back", "Map Pattern Back to 3D", Stage::Flatten)
            .input(PortSpec::new("pattern", DataKind::Geometry))
            .input(PortSpec::new("source", DataKind::Geometry))
            .input(PortSpec::new("curves", DataKind::Geometry))
            .output(PortSpec::new("mapped", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("tolerance_mm", "Tolerance", 0.01, 0.0, 10.0).unit("mm").help("Points this close outside the pattern outline still count as inside."))
            .param(ParamSpec::bool("clip", "Clip outside pieces", true).help("Drop the parts of curves that leave the pattern. Off keeps only curves that stay inside."))
            .help("Carries points and curves drawn on the flat pattern back onto the 3-D surface through the triangle-by-triangle correspondence. Curves are split where they cross triangle edges, so the result lies exactly on the surface.")
            .emulates("Returning markup from a flattened pattern to the 3-D shape (squish back)")
            .spec(FLATTEN_SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let pat = inputs::geometry(ins, 0)?;
        let src = inputs::geometry(ins, 1)?;
        let cur = inputs::geometry(ins, 2)?;
        if pat.points.len() != src.points.len() || pat.prims.len() != src.prims.len() {
            return Err(
                ctx.fail("the pattern and the 3-D source must have the same points and faces")
            );
        }
        if face_count(&pat) == 0 {
            return Err(ctx.fail("the pattern has no faces"));
        }
        let tol = p.float("tolerance_mm")?;
        let clip = p.bool("clip")?;
        let loc = Locator::build(&pat, &src);
        let mut out = Geometry::new();
        let mut r = Report::new("Map back");
        let (mut mapped_pts, mut outside_pts) = (0usize, 0usize);
        let (mut mapped_len, mut outside_len) = (0.0f64, 0.0f64);
        let mut curves = 0usize;
        let xy = |i: u32| DVec2::new(cur.points[i as usize].x, cur.points[i as usize].y);
        let in_prim = {
            let mut v = vec![false; cur.points.len()];
            for pr in &cur.prims {
                for &i in &pr.vertices {
                    v[i as usize] = true;
                }
            }
            v
        };
        for (i, used) in in_prim.iter().enumerate() {
            if *used {
                continue;
            }
            match loc.locate(DVec2::new(cur.points[i].x, cur.points[i].y), tol) {
                Some((ti, b)) => {
                    out.add_point(loc.lift(ti, b));
                    mapped_pts += 1;
                }
                None => outside_pts += 1,
            }
        }
        for pr in cur.prims.iter().filter(|pr| pr.vertices.len() >= 2) {
            let mut pieces: Vec<Vec<DVec3>> = Vec::new();
            let mut open: Vec<DVec3> = Vec::new();
            let mut curve_out = 0.0f64;
            let mut seg_list: Vec<(DVec2, DVec2)> = pr
                .vertices
                .windows(2)
                .map(|w| (xy(w[0]), xy(w[1])))
                .collect();
            if pr.closed && pr.vertices.len() > 2 {
                seg_list.push((xy(pr.vertices[pr.vertices.len() - 1]), xy(pr.vertices[0])));
            }
            for (a, b) in seg_list {
                let mut ts = vec![0.0, 1.0];
                for ti in loc.candidates(a, b) {
                    let t = &loc.tri2[ti as usize];
                    for k in 0..3 {
                        if let Some(x) = seg_param(a, b, t[k], t[(k + 1) % 3]) {
                            ts.push(x);
                        }
                    }
                }
                ts.sort_by(|x, y| x.total_cmp(y));
                ts.dedup_by(|x, y| (*x - *y).abs() < 1e-12);
                for w in ts.windows(2) {
                    let (pa, pb) = (a.lerp(b, w[0]), a.lerp(b, w[1]));
                    let mid = a.lerp(b, 0.5 * (w[0] + w[1]));
                    match loc.locate(mid, tol) {
                        Some((ti, _)) => {
                            let t = &loc.tri2[ti];
                            let (Some(ba), Some(bb)) = (Locator::bary(t, pa), Locator::bary(t, pb))
                            else {
                                continue;
                            };
                            let (qa, qb) = (loc.lift(ti, ba), loc.lift(ti, bb));
                            if open.last().is_none_or_far(qa) {
                                if open.len() >= 2 {
                                    pieces.push(std::mem::take(&mut open));
                                } else {
                                    open.clear();
                                }
                                open.push(qa);
                            }
                            open.push(qb);
                            mapped_len += qa.distance(qb);
                        }
                        None => {
                            outside_len += pa.distance(pb);
                            curve_out += pa.distance(pb);
                            if open.len() >= 2 {
                                pieces.push(std::mem::take(&mut open));
                            } else {
                                open.clear();
                            }
                        }
                    }
                }
            }
            if open.len() >= 2 {
                pieces.push(open);
            }
            if !clip && curve_out > 0.0 {
                pieces.clear();
            }
            for pc in pieces {
                let ids: Vec<u32> = pc.iter().map(|&x| out.add_point(x)).collect();
                out.add_prim(Prim::polyline(ids));
                curves += 1;
            }
        }
        r.metric("mapped_points", Metric::new(mapped_pts as f64, "count"));
        r.metric("outside_points", Metric::new(outside_pts as f64, "count"));
        r.metric("mapped_curves", Metric::new(curves as f64, "count"));
        r.metric("mapped_length_mm", Metric::new(mapped_len, "mm"));
        r.metric("outside_length_mm", Metric::new(outside_len, "mm"));
        if outside_len > 0.0 || outside_pts > 0 {
            r.note(
                Severity::Warning,
                "some points or curve pieces lie outside the pattern outline and were clipped",
            );
        }
        Ok(vec![Data::geometry(out), Data::report(r)])
    }
}

/// `Option<&DVec3>` helper: true when there is no previous point or it is not
/// (nearly) the same as `q`.
trait FarFrom {
    fn is_none_or_far(&self, q: DVec3) -> bool;
}

impl FarFrom for Option<&DVec3> {
    fn is_none_or_far(&self, q: DVec3) -> bool {
        match self {
            None => true,
            Some(p) => p.distance(q) > 1e-9,
        }
    }
}

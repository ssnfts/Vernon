//! Mesh-side nodes: solid mesh from a closed surface, mesh check, mount sites.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Duration;

use glam::DVec3;
use vernon_core::{AttribArray, Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::mesh::{
    boundary_faces, boundary_node_set, corner_points, element_size, gmsh_tet10_to_solver, mesh_volume, orient_positive, solid_from_tets, tet4_to_tet10, tet_quality, tri_surface, voxel_tets, BFace,
};
use crate::msh4::{read_header, read_msh2_binary, read_msh4};
use crate::solver::{find_executable, run_with_timeout, tail, RunStatus};
use crate::spatial::ray_triangle;
use crate::types::{finite, to_data, MountSet, MountSite};
use crate::util::{fea_info, solid_mesh, write_work_file};

/// ASCII STL text of a triangle list.
pub fn stl_ascii(name: &str, tris: &[[DVec3; 3]]) -> String {
    let mut s = String::with_capacity(tris.len() * 200);
    let _ = writeln!(s, "solid {name}");
    for t in tris {
        let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
        let _ = writeln!(s, "facet normal {:e} {:e} {:e}\nouter loop", n.x, n.y, n.z);
        for p in t {
            let _ = writeln!(s, "vertex {:e} {:e} {:e}", p.x, p.y, p.z);
        }
        s.push_str("endloop\nendfacet\n");
    }
    let _ = writeln!(s, "endsolid {name}");
    s
}

/// Corner-node tetrahedra of a Gmsh ASCII v2 `.msh` text (element types 4 and 11).
pub fn read_msh2(text: &str) -> Result<(Vec<DVec3>, Vec<[u32; 4]>), String> {
    let mut lines = text.lines().map(str::trim);
    let mut nodes: HashMap<u64, DVec3> = HashMap::new();
    let mut raw: Vec<[u64; 4]> = Vec::new();
    while let Some(l) = lines.next() {
        match l {
            "$Nodes" => {
                let n: usize = lines.next().and_then(|x| x.parse().ok()).ok_or("bad node count")?;
                for _ in 0..n {
                    let t: Vec<&str> = lines.next().ok_or("truncated node list")?.split_whitespace().collect();
                    if t.len() < 4 {
                        return Err("bad node row".into());
                    }
                    let f = |i: usize| t[i].parse::<f64>().map_err(|e| format!("bad coordinate: {e}"));
                    nodes.insert(t[0].parse::<u64>().map_err(|e| e.to_string())?, DVec3::new(f(1)?, f(2)?, f(3)?));
                }
            }
            "$Elements" => {
                let n: usize = lines.next().and_then(|x| x.parse().ok()).ok_or("bad element count")?;
                for _ in 0..n {
                    let t: Vec<u64> = lines.next().ok_or("truncated element list")?.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                    if t.len() < 3 {
                        continue;
                    }
                    let ntags = t[2] as usize;
                    let conn = &t[3 + ntags.min(t.len() - 3)..];
                    match t[1] {
                        4 if conn.len() == 4 => raw.push([conn[0], conn[1], conn[2], conn[3]]),
                        11 if conn.len() == 10 => {
                            let ids: [u32; 10] = std::array::from_fn(|i| conn[i] as u32);
                            let s = gmsh_tet10_to_solver(&ids);
                            raw.push([s[0] as u64, s[1] as u64, s[2] as u64, s[3] as u64]);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if raw.is_empty() {
        return Err("the mesh file has no tetrahedra".into());
    }
    let mut index: HashMap<u64, u32> = HashMap::new();
    let mut points = Vec::new();
    let mut tets = Vec::with_capacity(raw.len());
    for t in raw {
        let mut e = [0u32; 4];
        for (k, id) in t.iter().enumerate() {
            let p = *nodes.get(id).ok_or_else(|| format!("element references missing node {id}"))?;
            e[k] = *index.entry(*id).or_insert_with(|| {
                points.push(p);
                (points.len() - 1) as u32
            });
        }
        tets.push(e);
    }
    Ok((points, tets))
}

/// Corner-node tetrahedra of a mesher output file: Gmsh 4.1 (ASCII or binary) or ASCII 2.x.
pub fn read_mesher_output(bytes: &[u8]) -> Result<(Vec<DVec3>, Vec<[u32; 4]>), String> {
    let h = read_header(bytes)?;
    if h.version.starts_with("2.") && !h.binary {
        return read_msh2(std::str::from_utf8(bytes).map_err(|_| "non-text data in an ASCII mesh file")?);
    }
    if h.version.starts_with("2.") {
        return read_msh2_binary(bytes);
    }
    read_msh4(bytes)
}

fn surface_volume(tris: &[[DVec3; 3]]) -> f64 {
    tris.iter().map(|t| t[0].dot(t[1].cross(t[2])) / 6.0).sum::<f64>().abs()
}

// --------------------------------------------------------------- tet mesh

pub struct TetMesh;

impl NodeOp for TetMesh {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.tet_mesh", "Solid Mesh")
            .input(PortSpec::new("surface", DataKind::Geometry))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("surface_file", DataKind::File))
            .param(ParamSpec::float("edge_len_mm", "Edge length", 8.0, 0.2, 500.0).unit("mm").help("Target element edge length; use at least two elements through a wall"))
            .param(ParamSpec::float("eps_rel", "Envelope", 1e-3, 1e-6, 0.1).help("Envelope tolerance as a fraction of the bounding-box diagonal (external mesher)"))
            .param(ParamSpec::int("order", "Element order", 2, 1, 2).help("2 adds mid-edge nodes (quadratic tetrahedra)"))
            .param(ParamSpec::float("min_dihedral_deg", "Minimum dihedral", 5.0, 0.0, 60.0).unit("deg").help("Quality gate on the smallest dihedral angle"))
            .param(ParamSpec::int("max_cells", "Maximum cells", 400000, 1000, 5000000).help("Cap on the voxel cell count of the built-in mesher"))
            .param(ParamSpec::bool("run", "Run external mesher", false).help("Run the executable below once on the written surface file and read its mesh; switches itself off again").action())
            .param(ParamSpec::path("exe_path", "Mesher executable", &["exe", ""], false).help("Empty searches the program path"))
            .param(ParamSpec::float("timeout_s", "Timeout", 600.0, 1.0, 86400.0).unit("s").help("Longest the mesher may run before it is stopped, in seconds."))
            .help("Tetrahedral mesh of a closed triangle surface. With run on, an external open-source mesher builds a mesh that follows the surface within the envelope tolerance. Otherwise, or when the mesher is not found, a built-in stand-in fills the surface with stair-stepped voxel tetrahedra; that result is flagged and fails its gate.")
            .emulates("conformal tetrahedral meshing of a closed triangle surface")
            .implemented()
            .external("fTetWild")
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let surf = inputs::geometry(inputs, 0)?;
        let ts = tri_surface(&surf);
        if ts.tris.is_empty() {
            return Err(ctx.fail("the surface has no triangles"));
        }
        if ts.open_edges > 0 {
            return Err(ctx.fail(format!("the surface is not closed: {} open or non-manifold edges; repair it before meshing", ts.open_edges)));
        }
        let h = params.float("edge_len_mm")?;
        let stl = write_work_file(ctx, "solid_surface.stl", &stl_ascii("surface", &ts.tris))?;
        let sv = surface_volume(&ts.tris);
        let mut r = Report::new("Solid mesh");
        let mut linear: Option<Geometry> = None;
        let mut source = "built-in voxel mesher";
        if params.bool("run")? {
            let diag = surf.bounds().size().length().max(1e-9);
            match find_executable(params.str("exe_path")?, &["FloatTetwild_bin", "fTetWild", "FloatTetwild"]) {
                None => {
                    r.note(Severity::Warning, "external mesher not found: the built-in voxel mesher was used instead");
                }
                Some(exe) => {
                    let out = ctx.work_dir.join("solid_mesh.msh");
                    let _ = std::fs::remove_file(&out);
                    // edge length is passed relative to the bounding-box diagonal
                    let args: Vec<String> = [
                        "-i".to_string(),
                        stl.to_string_lossy().into_owned(),
                        "-o".into(),
                        out.to_string_lossy().into_owned(),
                        "-l".into(),
                        format!("{}", h / diag),
                        "-e".into(),
                        format!("{}", params.float("eps_rel")?),
                    ]
                    .to_vec();
                    let log = ctx.work_dir.join("solid_mesh.log");
                    let st = run_with_timeout(&exe, &args, &ctx.work_dir, &log, Duration::from_secs_f64(params.float("timeout_s")?), &[])?;
                    match st {
                        RunStatus::Exited(0) if out.is_file() => {
                            let bytes = std::fs::read(&out)?;
                            let (p, t) = read_mesher_output(&bytes).map_err(|e| ctx.fail(format!("cannot read the mesher output: {e}")))?;
                            let mut g = solid_from_tets(p, &t);
                            orient_positive(&mut g);
                            linear = Some(g);
                            source = "external mesher";
                            r.metric("placeholder", Metric::new(0.0, "flag").with_limit(0.0, true));
                        }
                        RunStatus::TimedOut => {
                            return Err(vernon_graph::CookError::External { tool: "fTetWild".into(), message: "timed out".into() });
                        }
                        _ => {
                            return Err(vernon_graph::CookError::External { tool: "fTetWild".into(), message: format!("failed:\n{}", tail(&log, 15)) });
                        }
                    }
                }
            }
        }
        let linear = match linear {
            Some(g) => g,
            None => {
                let (p, t) = voxel_tets(&ts.tris, h, params.int("max_cells")? as usize).map_err(|e| ctx.fail(e))?;
                r.note(Severity::Warning, "white-box placeholder: stair-stepped voxel tetrahedra stand in for a conformal mesh; the real mesher follows the surface within the envelope tolerance");
                r.metric("placeholder", Metric::new(1.0, "flag").with_limit(0.0, true));
                solid_from_tets(p, &t)
            }
        };
        let mesh = if params.int("order")? == 2 { tet4_to_tet10(&linear).map_err(|e| ctx.fail(e))? } else { linear };
        let min_dih = (0..mesh.prims.len()).map(|e| tet_quality(corner_points(&mesh, e)).min_dihedral_deg).fold(f64::INFINITY, f64::min);
        let vol = mesh_volume(&mesh);
        r.metric("nodes", Metric::new(mesh.points.len() as f64, ""));
        r.metric("elements", Metric::new(mesh.prims.len() as f64, ""));
        r.metric("mesh_volume_mm3", Metric::new(vol, "mm3"));
        r.metric("surface_volume_mm3", Metric::new(sv, "mm3"));
        r.metric("volume_error_pct", Metric::new(100.0 * (vol - sv).abs() / sv.max(1e-12), "%"));
        r.metric("min_dihedral_deg", Metric::new(finite(min_dih), "deg").with_limit(params.float("min_dihedral_deg")?, false));
        r.note(Severity::Info, format!("mesh source: {source}"));
        if ts.degenerate > 0 {
            r.note(Severity::Warning, format!("{} degenerate triangles were ignored", ts.degenerate));
        }
        Ok(vec![Data::geometry(mesh), Data::report(r), Data::File(stl)])
    }
}

// -------------------------------------------------------------- mesh check

/// Smallest ratio of local wall thickness to shortest element edge over a sample
/// of boundary faces, with the thickness at that sample.
pub fn through_wall_ratio(mesh: &Geometry, faces: &[BFace], samples: usize) -> Option<(f64, f64)> {
    let tris: Vec<[DVec3; 3]> = faces.iter().map(|f| f.nodes.map(|i| mesh.points[i as usize])).collect();
    let stride = (faces.len() / samples.max(1)).max(1);
    let mut best: Option<(f64, f64)> = None;
    for f in faces.iter().step_by(stride) {
        let d = -f.normal;
        let hit = tris.iter().filter_map(|t| ray_triangle(f.centroid, d, t)).fold(f64::INFINITY, f64::min);
        if !hit.is_finite() {
            continue;
        }
        let p = corner_points(mesh, f.elem);
        let edge = [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)].iter().map(|&(a, b)| (p[a] - p[b]).length()).fold(f64::INFINITY, f64::min);
        let ratio = hit / edge.max(1e-12);
        if best.is_none_or(|b| ratio < b.0) {
            best = Some((ratio, hit));
        }
    }
    best
}

pub struct MeshCheck;

impl NodeOp for MeshCheck {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.mesh_check", "Solid Mesh Check")
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::float("min_through_wall", "Elements through wall", 2.0, 1.0, 10.0).help("Fewest elements allowed across the local wall thickness (use 3 where bending governs)"))
            .param(ParamSpec::float("min_dihedral_deg", "Minimum dihedral", 5.0, 0.0, 60.0).unit("deg").help("Smallest dihedral angle between faces of a tetrahedron that passes the check, in degrees. Slivers below it are counted."))
            .param(ParamSpec::float("max_aspect", "Maximum aspect", 10.0, 1.5, 100.0).help("Longest edge over the smallest altitude; a regular tetrahedron is 1.22"))
            .param(ParamSpec::int("samples", "Thickness samples", 300, 10, 5000).help("Boundary faces probed for the wall thickness"))
            .help("Quality check of a solid mesh: negative volumes, smallest dihedral angle, aspect ratio and the number of elements through the wall thickness. Writes per-element quality as primitive attributes.")
            .emulates("mesh quality panels of finite-element pre-processors")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let mesh = solid_mesh(ctx, inputs, 0)?;
        let n = mesh.prims.len();
        let q: Vec<_> = (0..n).map(|e| tet_quality(corner_points(&mesh, e))).collect();
        let neg = q.iter().filter(|x| x.volume <= 0.0).count();
        let min_dih = q.iter().map(|x| x.min_dihedral_deg).fold(f64::INFINITY, f64::min);
        let max_asp = q.iter().map(|x| x.aspect).fold(0.0, f64::max);
        let mean_edge = q.iter().map(|x| x.mean_edge).sum::<f64>() / n.max(1) as f64;
        let faces = boundary_faces(&mesh);
        let tw = through_wall_ratio(&mesh, &faces, params.int("samples")? as usize);
        let mut out = (*mesh).clone();
        out.prim_attribs.set("min_dihedral", AttribArray::Float(q.iter().map(|x| finite(x.min_dihedral_deg)).collect()));
        out.prim_attribs.set("aspect", AttribArray::Float(q.iter().map(|x| finite(x.aspect)).collect()));
        let mut r = Report::new("Solid mesh check");
        r.metric("elements", Metric::new(n as f64, ""));
        r.metric("negative_volume_elements", Metric::new(neg as f64, "").with_limit(0.0, true));
        r.metric("min_dihedral_deg", Metric::new(finite(min_dih), "deg").with_limit(params.float("min_dihedral_deg")?, false));
        r.metric("max_aspect", Metric::new(finite(max_asp), "").with_limit(params.float("max_aspect")?, true));
        r.metric("mean_edge_mm", Metric::new(mean_edge, "mm"));
        match tw {
            Some((ratio, thick)) => {
                r.metric("through_wall_elements", Metric::new(ratio, "").with_limit(params.float("min_through_wall")? - 1e-6, false));
                r.metric("thinnest_wall_mm", Metric::new(thick, "mm"));
                if ratio < params.float("min_through_wall")? - 1e-6 {
                    r.note(Severity::Error, format!("only {ratio:.2} elements across a {thick:.2} mm wall: refine the mesh (bending in a thin wall locks with too few elements)"));
                }
            }
            None => {
                r.note(Severity::Warning, "wall thickness could not be probed (no inward ray hit the opposite wall)");
            }
        }
        Ok(vec![Data::geometry(out), Data::report(r)])
    }
}

// -------------------------------------------------------------- mount sites

/// Build one mount patch around the point `p` (see the node help).
#[allow(clippy::too_many_arguments)]
pub fn build_site(mesh: &Geometry, cand: &[u32], faces: &[BFace], name: &str, p: DVec3, axis_hint: DVec3, r_w: f64, slab: f64, model: &str) -> Result<MountSite, String> {
    let c_idx = cand
        .iter()
        .copied()
        .min_by(|&a, &b| (mesh.points[a as usize] - p).length_squared().total_cmp(&(mesh.points[b as usize] - p).length_squared()))
        .ok_or("the mesh has no boundary nodes")?;
    let c = mesh.points[c_idx as usize];
    let normal = faces
        .iter()
        .min_by(|a, b| (a.centroid - c).length_squared().total_cmp(&(b.centroid - c).length_squared()))
        .map(|f| f.normal)
        .unwrap_or(DVec3::Z);
    let mut a = axis_hint.normalize_or_zero();
    if a == DVec3::ZERO {
        a = normal;
    }
    if a.dot(normal) < 0.0 {
        a = -a;
    }
    let mut nodes: Vec<u32> = cand
        .iter()
        .copied()
        .filter(|&i| {
            let d = mesh.points[i as usize] - c;
            let ax = d.dot(a);
            (d - a * ax).length() <= r_w && ax.abs() <= slab
        })
        .collect();
    nodes.sort_unstable();
    if nodes.len() < 3 {
        return Err(format!("mount '{name}': only {} surface nodes within {r_w} mm of the bolt axis; increase the patch radius or refine the mesh", nodes.len()));
    }
    Ok(MountSite {
        name: name.into(),
        center: c.into(),
        axis: a.into(),
        nodes,
        model: model.into(),
        r_w_mm: r_w,
        avg_radius_mm: 0.0,
        bolt_d_mm: 0.0,
        pad_e_mpa: 0.0,
        pad_t_mm: 0.0,
    })
}

pub struct MountSites;

impl NodeOp for MountSites {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.mount_sites", "Mount Sites")
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .input(PortSpec::optional("sites", DataKind::Geometry))
            .output(PortSpec::new("mounts", DataKind::Json))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("model", "Mount model", "rigid_washer", &["fixed_patch", "rigid_washer", "spring_pad"]).help("Fixed surface nodes, a rigid washer coupled to a reference node, or springs to ground"))
            .param(ParamSpec::float("r_w_mm", "Patch radius", 4.5, 0.5, 50.0).unit("mm").help("Radius of the contact patch around the bolt axis (about 0.75 of the washer radius)"))
            .param(ParamSpec::float("avg_radius_mm", "Averaging radius", 5.0, 0.0, 30.0).unit("mm").help("Hot-spot averaging radius for the safety factor at the mount; bolt diameter by default"))
            .param(ParamSpec::float("bolt_d_mm", "Bolt diameter", 5.0, 1.0, 30.0).unit("mm").help("Bolt diameter, in millimetres. It sets the stress-averaging size at the mount."))
            .param(ParamSpec::float("slab_mm", "Patch depth", 0.0, 0.0, 20.0).unit("mm").help("How far from the contact plane a node may lie; 0 uses 0.35 x the patch radius"))
            .param(ParamSpec::vec3("axis", "Bolt axis", DVec3::Z).help("Direction of the bolt axis; the sign is turned to point out of the part"))
            .param(ParamSpec::int("count", "Number of sites", 1, 0, 3).help("Sites taken from the three positions below when no site cloud is connected"))
            .param(ParamSpec::vec3("site_a", "Site A", DVec3::ZERO).unit("mm").help("Position of mount A in millimetres. Used when no site cloud is connected and the number of sites is at least 1."))
            .param(ParamSpec::vec3("site_b", "Site B", DVec3::ZERO).unit("mm").help("Position of mount B in millimetres. Used when no site cloud is connected and the number of sites is at least 2."))
            .param(ParamSpec::vec3("site_c", "Site C", DVec3::ZERO).unit("mm").help("Position of mount C in millimetres. Used when no site cloud is connected and the number of sites is 3."))
            .param(ParamSpec::float("pad_e_mpa", "Pad modulus", 5.0, 0.01, 1000.0).unit("MPa").help("Spring-pad model only"))
            .param(ParamSpec::float("pad_t_mm", "Pad thickness", 2.0, 0.1, 50.0).unit("mm").help("Spring-pad model only"))
            .help("Picks the bolt locations on a solid mesh and builds the node patch that the solver constrains. Constrained nodes create stress singularities, so the safety factor at a mount is taken from stress averaged over a sphere, with the raw peak reported beside it.")
            .emulates("fixed-support and bolt constraint objects of finite-element front ends")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let mesh = solid_mesh(ctx, inputs, 0)?;
        let faces = boundary_faces(&mesh);
        let cand = boundary_node_set(&mesh);
        let r_w = params.float("r_w_mm")?;
        let slab = match params.float("slab_mm")? {
            s if s > 0.0 => s,
            _ => 0.35 * r_w,
        };
        let pts: Vec<DVec3> = match inputs::opt_geometry(inputs, 1) {
            Some(g) if !g.points.is_empty() => g.points.clone(),
            _ => ["site_a", "site_b", "site_c"].iter().take(params.int("count")?.clamp(0, 3) as usize).map(|k| params.vec3(k)).collect::<Result<_, _>>()?,
        };
        if pts.is_empty() {
            return Err(ctx.fail("no mount sites: connect a point cloud or set the number of sites"));
        }
        let (model, axis) = (params.str("model")?, params.vec3("axis")?);
        let mut set = MountSet::default();
        let mut out = (*mesh).clone();
        let mut r = Report::new("Mount sites");
        for (k, p) in pts.iter().enumerate() {
            let name = format!("mount{}", k + 1);
            let mut s = build_site(&mesh, &cand, &faces, &name, *p, axis, r_w, slab, model).map_err(|e| ctx.fail(e))?;
            s.bolt_d_mm = params.float("bolt_d_mm")?;
            s.avg_radius_mm = match params.float("avg_radius_mm")? {
                v if v > 0.0 => v,
                _ => s.bolt_d_mm,
            };
            s.pad_e_mpa = params.float("pad_e_mpa")?;
            s.pad_t_mm = params.float("pad_t_mm")?;
            r.metric(format!("{name}_nodes"), Metric::new(s.nodes.len() as f64, ""));
            out.point_groups.insert(name, s.nodes.clone());
            set.sites.push(s);
        }
        r.metric("mount_count", Metric::new(set.sites.len() as f64, ""));
        r.metric("min_patch_nodes", Metric::new(set.sites.iter().map(|s| s.nodes.len()).min().unwrap_or(0) as f64, "").with_limit(3.0, false));
        r.note(Severity::Info, "fixed nodes are stress singularities: judge a mount by the averaged safety factor, not the raw peak");
        let _ = element_size(&out);
        Ok(vec![to_data(&set), Data::geometry(out), Data::report(r)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::box_tets;
    use vernon_graph::ParamValue;

    fn ctx() -> CookContext {
        let d = std::env::temp_dir().join(format!("vernon_fea_mesh_{}", std::process::id()));
        CookContext { node_name: "t".into(), work_dir: d, frame: 0.0 }
    }

    fn defaults(op: &dyn NodeOp) -> Params {
        Params::from_specs(&op.info().params)
    }

    #[test]
    fn msh2_reader_handles_tet4_and_converts_tet10_order() {
        let msh = "$MeshFormat\n2.2 0 8\n$EndMeshFormat\n$Nodes\n5\n1 0 0 0\n2 1 0 0\n3 0 1 0\n4 0 0 1\n9 1 1 1\n$EndNodes\n$Elements\n2\n1 4 2 0 1 1 2 3 4\n2 4 2 0 1 2 3 4 9\n$EndElements\n";
        let (p, t) = read_msh2(msh).unwrap();
        assert_eq!(p.len(), 5);
        assert_eq!(t.len(), 2);
        assert_eq!(t[1], [1, 2, 3, 4]);
        assert!(read_msh2("$Nodes\n0\n$EndNodes\n").is_err());
    }

    #[test]
    fn tet_mesh_fills_a_closed_box_and_refuses_an_open_one() {
        let c = ctx();
        let surf = Geometry::make_box(DVec3::new(40.0, 20.0, 10.0));
        let mut p = defaults(&TetMesh);
        p.set("edge_len_mm", ParamValue::Float(5.0));
        let out = TetMesh.cook(&c, &[Some(Data::geometry(surf.clone()))], &p).unwrap();
        let mesh = out[0].as_geometry().unwrap();
        assert_eq!(mesh.prims[0].vertices.len(), 10);
        let rep = out[1].as_report().unwrap();
        assert!((rep.metrics["mesh_volume_mm3"].value - 8000.0).abs() < 1e-6);
        assert!(rep.metrics["volume_error_pct"].value < 1e-6);
        assert!(!rep.passes(), "the white-box mesh must not pass a gate silently");
        assert!(out[2].as_file().unwrap().is_file());
        let mut open = surf;
        open.prims.pop();
        assert!(TetMesh.cook(&c, &[Some(Data::geometry(open))], &p).is_err());
        let mut lin = p.clone();
        lin.set("order", ParamValue::Int(1));
        let o = TetMesh.cook(&c, &[Some(Data::geometry(Geometry::make_box(DVec3::splat(10.0))))], &lin).unwrap();
        assert_eq!(o[0].as_geometry().unwrap().prims[0].vertices.len(), 4);
    }

    /// Stand-in mesher script: records its arguments, then copies `payload` to the `-o` path.
    fn fake_mesher(name: &str, payload: &[u8]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("vernon_fea_fake_{name}_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("payload.msh"), payload).unwrap();
        #[cfg(windows)]
        let exe = {
            let exe = d.join("mesher.bat");
            std::fs::write(&exe, "@echo off\r\necho %* > \"%~dp0args.txt\"\r\ncopy /y \"%~dp0payload.msh\" %4 >nul\r\n").unwrap();
            exe
        };
        #[cfg(not(windows))]
        let exe = {
            use std::os::unix::fs::PermissionsExt;
            let exe = d.join("mesher.sh");
            std::fs::write(&exe, "#!/bin/sh\necho \"$@\" > \"$(dirname \"$0\")/args.txt\"\ncp \"$(dirname \"$0\")/payload.msh\" \"$4\"\n").unwrap();
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
            exe
        };
        exe
    }

    fn run_params(exe: &str) -> Params {
        let mut p = defaults(&TetMesh);
        p.set("run", ParamValue::Bool(true));
        p.set("exe_path", ParamValue::Str(exe.into()));
        p.set("order", ParamValue::Int(1));
        p.set("edge_len_mm", ParamValue::Float(5.0));
        p
    }

    fn work(name: &str) -> CookContext {
        let d = std::env::temp_dir().join(format!("vernon_fea_work_{name}_{}", std::process::id()));
        CookContext { node_name: "t".into(), work_dir: d, frame: 0.0 }
    }

    #[test]
    fn a_mesher_run_clears_the_placeholder_and_passes_the_contract_arguments() {
        let exe = fake_mesher("ok", crate::msh4::tests::ASCII.as_bytes());
        let surf = Geometry::make_box(DVec3::splat(10.0));
        let out = TetMesh.cook(&work("ok"), &[Some(Data::geometry(surf.clone()))], &run_params(exe.to_str().unwrap())).unwrap();
        let rep = out[1].as_report().unwrap();
        assert_eq!(rep.metrics["placeholder"].value, 0.0);
        assert!(!rep.notes.iter().any(|(_, n)| n.starts_with("white-box placeholder:")));
        let mesh = out[0].as_geometry().unwrap();
        assert_eq!((mesh.points.len(), mesh.prims.len()), (5, 2));
        assert!((0..2).all(|e| crate::mesh::element_volume(mesh, e) > 0.0));
        let args = std::fs::read_to_string(exe.with_file_name("args.txt")).unwrap();
        let rel = 5.0 / surf.bounds().size().length();
        let t: Vec<&str> = args.split_whitespace().collect();
        assert_eq!(t[0], "-i");
        assert!(t[1].contains("solid_surface.stl"), "{args}");
        assert_eq!(t[2], "-o");
        assert!(t[3].contains("solid_mesh.msh"), "{args}");
        assert_eq!(t[4], "-l");
        assert!((t[5].parse::<f64>().unwrap() - rel).abs() < 1e-12, "{args}");
        assert_eq!(t[6], "-e");
        assert_eq!(t[7].parse::<f64>().unwrap(), 1e-3);
    }

    #[test]
    fn a_missing_mesher_falls_back_and_keeps_the_placeholder() {
        let surf = Geometry::make_box(DVec3::splat(10.0));
        let out = TetMesh.cook(&work("none"), &[Some(Data::geometry(surf))], &run_params("definitely/not/here.exe")).unwrap();
        let rep = out[1].as_report().unwrap();
        assert_eq!(rep.metrics["placeholder"].value, 1.0);
        assert!(!rep.passes());
        assert!(rep.notes.iter().any(|(s, n)| *s == Severity::Warning && n.starts_with("white-box placeholder:")));
    }

    #[test]
    fn bad_mesher_output_and_a_failing_mesher_are_errors_not_fallbacks() {
        let surf = Geometry::make_box(DVec3::splat(10.0));
        let bad = fake_mesher("bad", b"$MeshFormat\n4.1 0 8\n$EndMeshFormat\n$Nodes\n1 1");
        let e = TetMesh.cook(&work("bad"), &[Some(Data::geometry(surf.clone()))], &run_params(bad.to_str().unwrap())).unwrap_err();
        assert!(e.to_string().contains("cannot read the mesher output"), "{e}");
        // the test binary refuses the mesher's arguments and exits non-zero
        let failing = std::env::current_exe().unwrap();
        let e = TetMesh.cook(&work("fail"), &[Some(Data::geometry(surf))], &run_params(failing.to_str().unwrap())).unwrap_err();
        assert!(matches!(e, vernon_graph::CookError::External { .. }), "{e}");
    }

    #[test]
    fn mesher_output_dispatches_on_the_format_version() {
        assert_eq!(read_mesher_output(crate::msh4::tests::ASCII.as_bytes()).unwrap().1.len(), 2);
        let v2 = "$MeshFormat\n2.2 0 8\n$EndMeshFormat\n$Nodes\n4\n1 0 0 0\n2 1 0 0\n3 0 1 0\n4 0 0 1\n$EndNodes\n$Elements\n1\n1 4 2 0 1 1 2 3 4\n$EndElements\n";
        assert_eq!(read_mesher_output(v2.as_bytes()).unwrap().1.len(), 1);
        // binary format 2 is what the real mesher writes; a file with no data is still refused
        assert!(read_mesher_output(b"$MeshFormat\n2.2 1 8\n$EndMeshFormat\n").is_err());
    }

    #[test]
    #[ignore = "needs fTetWild"]
    fn real_mesher_fills_a_box() {
        let surf = Geometry::make_box(DVec3::new(40.0, 20.0, 10.0));
        let out = TetMesh.cook(&work("real"), &[Some(Data::geometry(surf))], &run_params("")).unwrap();
        let rep = out[1].as_report().unwrap();
        assert_eq!(rep.metrics["placeholder"].value, 0.0, "fTetWild must be on the program path: {:?}", rep.notes);
        assert!(rep.metrics["volume_error_pct"].value < 1.0, "{:?}", rep.metrics);
        assert!(rep.metrics["min_dihedral_deg"].value > 0.0, "{:?}", rep.metrics);
    }

    /// A closed prism over `outline` (counter-clockwise, star-shaped about its
    /// first point), from z = 0 to `height`.
    fn prism(outline: &[(f64, f64)], height: f64) -> Geometry {
        let n = outline.len() as u32;
        let points = [0.0, height].iter().flat_map(|&z| outline.iter().map(move |&(x, y)| DVec3::new(x, y, z))).collect();
        let mut faces = Vec::new();
        for i in 1..n - 1 {
            faces.push(vec![0, i + 1, i]);
            faces.push(vec![n, n + i, n + i + 1]);
        }
        for i in 0..n {
            let j = (i + 1) % n;
            faces.push(vec![i, j, n + j]);
            faces.push(vec![i, n + j, n + i]);
        }
        Geometry::from_polygons(points, faces)
    }

    #[test]
    #[ignore = "needs fTetWild"]
    fn real_mesher_builds_quadratic_elements_that_follow_a_non_convex_shape() {
        let surf = prism(&[(20.0, 20.0), (20.0, 40.0), (0.0, 40.0), (0.0, 0.0), (40.0, 0.0), (40.0, 20.0)], 10.0);
        let mut p = run_params("");
        p.set("order", ParamValue::Int(2));
        p.set("edge_len_mm", ParamValue::Float(4.0));
        let out = TetMesh.cook(&work("real-l"), &[Some(Data::geometry(surf))], &p).unwrap();
        let rep = out[1].as_report().unwrap();
        assert_eq!(rep.metrics["placeholder"].value, 0.0, "{:?}", rep.notes);
        assert!((rep.metrics["surface_volume_mm3"].value - 12000.0).abs() < 1e-6);
        assert!(rep.metrics["volume_error_pct"].value < 1.0, "{:?}", rep.metrics);
        assert!(rep.metrics["min_dihedral_deg"].value > 0.0, "{:?}", rep.metrics);
        let mesh = out[0].as_geometry().unwrap();
        assert!(mesh.prims.iter().all(|e| e.vertices.len() == 10));
        // Nothing fills the notch, and the mesh reaches the far corners.
        for e in 0..mesh.prims.len() {
            let c = corner_points(mesh, e).iter().copied().sum::<DVec3>() / 4.0;
            assert!(!(c.x > 20.5 && c.y > 20.5), "element in the notch at {c}");
        }
        let b = mesh.bounds();
        assert!((b.size() - DVec3::new(40.0, 40.0, 10.0)).length() < 0.5, "{:?}", b.size());
    }

    #[test]
    #[ignore = "needs fTetWild"]
    fn real_mesher_fills_a_curved_shape() {
        let outline: Vec<(f64, f64)> = (0..48).map(|i| (f64::from(i) * std::f64::consts::TAU / 48.0).sin_cos()).map(|(s, c)| (15.0 * c, 15.0 * s)).collect();
        let out = TetMesh.cook(&work("real-round"), &[Some(Data::geometry(prism(&outline, 12.0)))], &run_params("")).unwrap();
        let rep = out[1].as_report().unwrap();
        assert_eq!(rep.metrics["placeholder"].value, 0.0, "{:?}", rep.notes);
        assert!(rep.metrics["volume_error_pct"].value < 1.0, "{:?}", rep.metrics);
        assert!(rep.metrics["min_dihedral_deg"].value > 0.0, "{:?}", rep.metrics);
    }

    #[test]
    fn g3_mesh_check_flags_one_element_through_the_depth() {
        let c = ctx();
        let one = box_tets(DVec3::new(100.0, 10.0, 10.0), [10, 1, 1]);
        let four = box_tets(DVec3::new(100.0, 10.0, 10.0), [40, 4, 4]);
        let p = defaults(&MeshCheck);
        let r1 = MeshCheck.cook(&c, &[Some(Data::geometry(tet4_to_tet10(&one).unwrap()))], &p).unwrap();
        let r4 = MeshCheck.cook(&c, &[Some(Data::geometry(tet4_to_tet10(&four).unwrap()))], &p).unwrap();
        let a = r1[1].as_report().unwrap();
        let b = r4[1].as_report().unwrap();
        assert!(!a.passes(), "1 element through the depth must be flagged");
        assert!((a.metrics["through_wall_elements"].value - 1.0).abs() < 1e-6, "{:?}", a.metrics["through_wall_elements"]);
        assert!(b.passes(), "{:?}", b.metrics);
        assert!((b.metrics["through_wall_elements"].value - 4.0).abs() < 1e-6);
        assert!(r4[0].as_geometry().unwrap().prim_attribs.contains("min_dihedral"));
    }

    #[test]
    fn mount_patch_on_a_flat_face() {
        let c = ctx();
        let mesh = tet4_to_tet10(&box_tets(DVec3::new(40.0, 20.0, 6.0), [16, 8, 2])).unwrap();
        let mut p = defaults(&MountSites);
        p.set("site_a", ParamValue::Vec3(DVec3::new(-10.0, 0.0, 3.0)));
        let out = MountSites.cook(&c, &[Some(Data::geometry(mesh))], &p).unwrap();
        let set: MountSet = serde_json::from_value((**out[0].as_json().unwrap()).clone()).unwrap();
        let s = &set.sites[0];
        assert!(s.nodes.len() >= 8, "{}", s.nodes.len());
        // axis points out of the top face
        assert!((s.axis[2] - 1.0).abs() < 1e-9, "{:?}", s.axis);
        assert!(s.nodes.iter().all(|&i| (out[1].as_geometry().unwrap().points[i as usize].z - 3.0).abs() < 1e-9));
        assert_eq!(out[1].as_geometry().unwrap().point_groups["mount1"], s.nodes);
        assert_eq!(s.avg_radius_mm, 5.0);
        // a patch smaller than the node spacing is refused
        p.set("r_w_mm", ParamValue::Float(0.5));
        assert!(MountSites.cook(&c, &[Some(Data::geometry(tet4_to_tet10(&box_tets(DVec3::splat(20.0), [2, 2, 2])).unwrap()))], &p).is_err());
    }
}

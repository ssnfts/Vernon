//! Catmull-Clark subdivision surfaces: refinement with infinite-sharpness
//! creases, corners and the boundary rule; limit positions; interpolating
//! cage solve; and the `surface.mesh_to_subd` / `surface.subd_refine` nodes.
//!
//! A subdivision cage travels between nodes as a `Geometry`: closed prims are
//! the cage faces, open two-vertex prims are crease edges, and the point
//! attribute `corner` (Int, 1 = pinned) marks corner vertices.

use std::collections::{BTreeMap, HashMap, HashSet};

use glam::DVec3;
use vernon_core::{AttribArray, Geometry, Metric, Prim, Report, Severity};
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::linalg::cg;
use crate::topology::{edge_key, orient, weld};
use crate::util::{check_finite, max_abs_coord, node_info, positive, SPEC_MESH};

/// Linear-combination arithmetic so one refinement routine serves both
/// positions and symbolic (sparse) stencils.
pub trait Lin: Clone {
    fn zero() -> Self;
    fn add_scaled(&mut self, other: &Self, s: f64);
}

impl Lin for DVec3 {
    fn zero() -> Self {
        DVec3::ZERO
    }
    fn add_scaled(&mut self, other: &Self, s: f64) {
        *self += *other * s;
    }
}

/// A sparse linear combination of cage points.
#[derive(Debug, Clone, Default)]
pub struct Sparse(pub BTreeMap<u32, f64>);

impl Lin for Sparse {
    fn zero() -> Self {
        Sparse::default()
    }
    fn add_scaled(&mut self, other: &Self, s: f64) {
        for (&k, &w) in &other.0 {
            *self.0.entry(k).or_insert(0.0) += w * s;
        }
    }
}

/// A subdivision cage.
#[derive(Debug, Clone, Default)]
pub struct Subd {
    pub points: Vec<DVec3>,
    pub faces: Vec<Vec<u32>>,
    pub creases: HashSet<(u32, u32)>,
    pub corners: HashSet<u32>,
}

/// Edge / vertex adjacency of a cage (ids assigned in first-encounter order).
#[derive(Debug, Clone, Default)]
pub struct Adj {
    pub edges: Vec<(u32, u32)>,
    pub edge_id: HashMap<(u32, u32), u32>,
    pub edge_faces: Vec<Vec<u32>>,
    pub vert_faces: Vec<Vec<u32>>,
    pub vert_edges: Vec<Vec<u32>>,
}

/// How a vertex is treated by the subdivision rules.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VKind {
    /// Fixed (pinned, or three or more sharp edges, or no faces).
    Fixed,
    /// Two sharp edges (crease or open boundary), with their far endpoints.
    Crease(u32, u32),
    /// Smooth vertex with `n` incident edges.
    Smooth(usize),
}

impl Subd {
    /// Build from geometry: welds, orients consistently, reads crease prims
    /// and the `corner` attribute. Returns the cage and the number of faces
    /// dropped as degenerate.
    pub fn from_geometry(geo: &Geometry, weld_tol: f64) -> Subd {
        let mut w = weld(geo, weld_tol);
        orient(&mut w);
        let mut s = Subd { points: w.points.clone(), faces: w.faces.clone(), ..Default::default() };
        for p in geo.prims.iter().filter(|p| !p.closed && p.vertices.len() == 2) {
            let (a, b) = (w.map[p.vertices[0] as usize], w.map[p.vertices[1] as usize]);
            if a != b {
                s.creases.insert(edge_key(a, b).0);
            }
        }
        if let Ok(c) = geo.point_attribs.ints("corner") {
            for (i, &f) in c.iter().enumerate() {
                if f != 0 {
                    s.corners.insert(w.map[i]);
                }
            }
        }
        s
    }

    pub fn to_geometry(&self) -> Geometry {
        let mut g = Geometry::from_polygons(self.points.clone(), self.faces.clone());
        let mut cr: Vec<(u32, u32)> = self.creases.iter().copied().collect();
        cr.sort_unstable();
        let mut names = Vec::new();
        for (a, b) in cr {
            names.push(g.prims.len() as u32);
            g.prims.push(Prim::polyline(vec![a, b]));
        }
        if !names.is_empty() {
            g.prim_groups.insert("creases".into(), names);
        }
        if !self.corners.is_empty() {
            let mut c = vec![0i64; self.points.len()];
            for &v in &self.corners {
                c[v as usize] = 1;
            }
            g.point_attribs.set("corner", AttribArray::Int(c));
        }
        g.point_attribs.resize_all(g.points.len());
        g.prim_attribs.resize_all(g.prims.len());
        g
    }

    pub fn adjacency(&self) -> Adj {
        let mut a = Adj { vert_faces: vec![Vec::new(); self.points.len()], vert_edges: vec![Vec::new(); self.points.len()], ..Default::default() };
        for (fi, f) in self.faces.iter().enumerate() {
            for i in 0..f.len() {
                let (u, v) = (f[i], f[(i + 1) % f.len()]);
                a.vert_faces[u as usize].push(fi as u32);
                let key = edge_key(u, v).0;
                let id = *a.edge_id.entry(key).or_insert_with(|| {
                    a.edges.push(key);
                    a.edge_faces.push(Vec::new());
                    (a.edges.len() - 1) as u32
                });
                a.edge_faces[id as usize].push(fi as u32);
            }
        }
        for (id, &(u, v)) in a.edges.iter().enumerate() {
            a.vert_edges[u as usize].push(id as u32);
            a.vert_edges[v as usize].push(id as u32);
        }
        a
    }

    pub fn is_sharp(&self, adj: &Adj, e: usize) -> bool {
        adj.edge_faces[e].len() != 2 || self.creases.contains(&adj.edges[e])
    }

    pub fn vertex_kind(&self, adj: &Adj, v: u32) -> VKind {
        if self.corners.contains(&v) || adj.vert_faces[v as usize].is_empty() {
            return VKind::Fixed;
        }
        let mut sharp = Vec::new();
        for &e in &adj.vert_edges[v as usize] {
            if self.is_sharp(adj, e as usize) {
                let (a, b) = adj.edges[e as usize];
                sharp.push(if a == v { b } else { a });
            }
        }
        match sharp.len() {
            0 | 1 => VKind::Smooth(adj.vert_edges[v as usize].len()),
            2 => VKind::Crease(sharp[0], sharp[1]),
            _ => VKind::Fixed,
        }
    }

    pub fn all_quads(&self) -> bool {
        self.faces.iter().all(|f| f.len() == 4)
    }

    /// Number of faces after one refinement step.
    pub fn refined_face_count(&self) -> usize {
        self.faces.iter().map(Vec::len).sum()
    }

    /// One Catmull-Clark step over arbitrary linear point data.
    pub fn refine_with<P: Lin>(&self, adj: &Adj, pts: &[P]) -> (Subd, Vec<P>) {
        let (nv, ne, nf) = (self.points.len(), adj.edges.len(), self.faces.len());
        let combo = |ids: &[u32], w: f64| {
            let mut z = P::zero();
            for &i in ids {
                z.add_scaled(&pts[i as usize], w);
            }
            z
        };
        let fpts: Vec<P> = self.faces.iter().map(|f| combo(f, 1.0 / f.len() as f64)).collect();
        let mut epts: Vec<P> = Vec::with_capacity(ne);
        for (e, &(a, b)) in adj.edges.iter().enumerate() {
            let mut p = P::zero();
            if self.is_sharp(adj, e) {
                p.add_scaled(&pts[a as usize], 0.5);
                p.add_scaled(&pts[b as usize], 0.5);
            } else {
                p.add_scaled(&pts[a as usize], 0.25);
                p.add_scaled(&pts[b as usize], 0.25);
                for &f in &adj.edge_faces[e] {
                    p.add_scaled(&fpts[f as usize], 0.25);
                }
            }
            epts.push(p);
        }
        let mut out: Vec<P> = Vec::with_capacity(nv + ne + nf);
        for v in 0..nv as u32 {
            let p0 = &pts[v as usize];
            let mut q = P::zero();
            match self.vertex_kind(adj, v) {
                VKind::Fixed => q.add_scaled(p0, 1.0),
                VKind::Crease(a, b) => {
                    q.add_scaled(p0, 0.75);
                    q.add_scaled(&pts[a as usize], 0.125);
                    q.add_scaled(&pts[b as usize], 0.125);
                }
                VKind::Smooth(n) => {
                    let nf_v = adj.vert_faces[v as usize].len().max(1);
                    let nn = n as f64;
                    for &f in &adj.vert_faces[v as usize] {
                        q.add_scaled(&fpts[f as usize], 1.0 / (nf_v as f64 * nn));
                    }
                    for &e in &adj.vert_edges[v as usize] {
                        let (a, b) = adj.edges[e as usize];
                        let w = 2.0 / (nn * nn) * 0.5;
                        q.add_scaled(&pts[a as usize], w);
                        q.add_scaled(&pts[b as usize], w);
                    }
                    q.add_scaled(p0, (nn - 3.0) / nn);
                }
            }
            out.push(q);
        }
        out.extend(epts);
        out.extend(fpts);
        let ep = |a: u32, b: u32| (nv as u32) + adj.edge_id[&edge_key(a, b).0];
        let mut faces = Vec::with_capacity(self.refined_face_count());
        for (fi, f) in self.faces.iter().enumerate() {
            let k = f.len();
            let fp = (nv + ne + fi) as u32;
            for i in 0..k {
                let (prev, cur, next) = (f[(i + k - 1) % k], f[i], f[(i + 1) % k]);
                faces.push(vec![cur, ep(cur, next), fp, ep(prev, cur)]);
            }
        }
        let mut creases = HashSet::with_capacity(self.creases.len() * 2);
        for &(a, b) in &self.creases {
            if let Some(&e) = adj.edge_id.get(&(a, b)) {
                let m = nv as u32 + e;
                creases.insert(edge_key(a, m).0);
                creases.insert(edge_key(m, b).0);
            }
        }
        let positions = vec![DVec3::ZERO; out.len()];
        (Subd { points: positions, faces, creases, corners: self.corners.clone() }, out)
    }

    /// One refinement step on the cage positions.
    pub fn refine(&self) -> Subd {
        let adj = self.adjacency();
        let (mut s, pts) = self.refine_with(&adj, &self.points);
        s.points = pts;
        s
    }

    pub fn refine_levels(&self, levels: usize) -> Subd {
        let mut s = self.clone();
        for _ in 0..levels {
            s = s.refine();
        }
        s
    }

    /// Limit positions (or limit stencils) of every vertex; the cage must be
    /// all quads. Returns `None` otherwise.
    pub fn limit_with<P: Lin>(&self, adj: &Adj, pts: &[P]) -> Option<Vec<P>> {
        if !self.all_quads() {
            return None;
        }
        let mut out = Vec::with_capacity(pts.len());
        for v in 0..self.points.len() as u32 {
            let p0 = &pts[v as usize];
            let mut q = P::zero();
            match self.vertex_kind(adj, v) {
                VKind::Fixed => q.add_scaled(p0, 1.0),
                VKind::Crease(a, b) => {
                    q.add_scaled(p0, 4.0 / 6.0);
                    q.add_scaled(&pts[a as usize], 1.0 / 6.0);
                    q.add_scaled(&pts[b as usize], 1.0 / 6.0);
                }
                VKind::Smooth(n) => {
                    let faces = &adj.vert_faces[v as usize];
                    if faces.len() != n || n < 3 {
                        q.add_scaled(p0, 1.0);
                    } else {
                        let nn = n as f64;
                        let den = nn * (nn + 5.0);
                        q.add_scaled(p0, nn * nn / den);
                        for &e in &adj.vert_edges[v as usize] {
                            let (a, b) = adj.edges[e as usize];
                            q.add_scaled(&pts[(if a == v { b } else { a }) as usize], 4.0 / den);
                        }
                        for &f in faces {
                            let ring = &self.faces[f as usize];
                            let pos = ring.iter().position(|&x| x == v).expect("vertex in its face");
                            q.add_scaled(&pts[ring[(pos + 2) % 4] as usize], 1.0 / den);
                        }
                    }
                }
            }
            out.push(q);
        }
        Some(out)
    }

    /// Limit positions of the cage vertices (refines once when not all quads).
    pub fn limit_positions(&self) -> Vec<DVec3> {
        if self.all_quads() {
            let adj = self.adjacency();
            self.limit_with(&adj, &self.points).expect("all quads")
        } else {
            let r = self.refine();
            let adj = r.adjacency();
            r.limit_with(&adj, &r.points).expect("all quads").into_iter().take(self.points.len()).collect()
        }
    }

    /// Extraordinary vertices: interior vertices (a closed ring of faces) with valence != 4.
    pub fn extraordinary_count(&self, adj: &Adj) -> usize {
        (0..self.points.len()).filter(|&v| {
            let (nf, ne) = (adj.vert_faces[v].len(), adj.vert_edges[v].len());
            nf == ne && nf != 4 && nf > 0
        }).count()
    }
}

/// Solve for the cage whose limit surface passes through `self.points`
/// (Halstead et al.): minimise `|A c - v|^2` by conjugate gradients on the
/// normal equations, where `A` maps cage points to limit positions of the
/// cage vertices. Returns the cage points and the final residual in mm.
pub fn interpolate_cage(target: &Subd) -> Option<(Vec<DVec3>, f64)> {
    let n = target.points.len();
    let unit: Vec<Sparse> = (0..n as u32).map(|i| Sparse(BTreeMap::from([(i, 1.0)]))).collect();
    let adj = target.adjacency();
    let (refined, rp) = target.refine_with(&adj, &unit);
    let radj = refined.adjacency();
    let limit = refined.limit_with(&radj, &rp)?;
    let rows: Vec<Vec<(u32, f64)>> = limit.into_iter().take(n).map(|s| s.0.into_iter().filter(|e| e.1 != 0.0).collect()).collect();
    let apply_a = |x: &[f64], y: &mut [f64]| {
        for (i, r) in rows.iter().enumerate() {
            y[i] = r.iter().map(|&(j, w)| w * x[j as usize]).sum();
        }
    };
    let apply_at = |y: &[f64], x: &mut [f64]| {
        for v in x.iter_mut() {
            *v = 0.0;
        }
        for (i, r) in rows.iter().enumerate() {
            for &(j, w) in r {
                x[j as usize] += w * y[i];
            }
        }
    };
    let apply_ata = |x: &[f64], y: &mut [f64]| {
        let mut t = vec![0.0; n];
        apply_a(x, &mut t);
        apply_at(&t, y);
    };
    let mut diag = vec![1e-12; n];
    for r in &rows {
        for &(j, w) in r {
            diag[j as usize] += w * w;
        }
    }
    let mut cols: Vec<Vec<f64>> = Vec::with_capacity(3);
    for c in 0..3 {
        let v: Vec<f64> = target.points.iter().map(|p| p[c]).collect();
        let mut b = vec![0.0; n];
        apply_at(&v, &mut b);
        let mut x = v.clone();
        cg(&apply_ata, &diag, &b, &mut x, 1e-14, 400);
        cols.push(x);
    }
    let cage: Vec<DVec3> = (0..n).map(|i| DVec3::new(cols[0][i], cols[1][i], cols[2][i])).collect();
    let mut resid: f64 = 0.0;
    for (i, r) in rows.iter().enumerate() {
        let mut lp = DVec3::ZERO;
        for &(j, w) in r {
            lp += cage[j as usize] * w;
        }
        resid = resid.max((lp - target.points[i]).length());
    }
    Some((cage, resid))
}

fn dihedral_deg(sub: &Subd, adj: &Adj, e: usize) -> f64 {
    let fs = &adj.edge_faces[e];
    if fs.len() != 2 {
        return 0.0;
    }
    let normal = |f: usize| {
        let v = &sub.faces[f];
        let mut n = DVec3::ZERO;
        for i in 0..v.len() {
            let (a, b) = (sub.points[v[i] as usize], sub.points[v[(i + 1) % v.len()] as usize]);
            n += DVec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
        }
        n.normalize_or_zero()
    };
    normal(fs[0] as usize).dot(normal(fs[1] as usize)).clamp(-1.0, 1.0).acos().to_degrees()
}

/// `surface.mesh_to_subd`
pub struct MeshToSubdNode;

impl NodeOp for MeshToSubdNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.mesh_to_subd", "Mesh to Subdivision Surface", SPEC_MESH)
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("cage", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::bool("interpolate", "Interpolate", false).help("Solve for a cage whose limit surface passes through the mesh vertices instead of using them as control points."))
            .param(ParamSpec::bool("creases", "Creases", true).help("Mark edges sharper than the crease angle as creases."))
            .param(ParamSpec::bool("corners", "Corners", true).help("Pin mesh corner vertices and vertices where three or more creases meet."))
            .param(ParamSpec::float("crease_angle_deg", "Crease angle", 30.0, 0.0, 180.0).unit("deg").help("Edges whose adjacent faces turn by more than this become creases."))
            .param(ParamSpec::float("weld_tol_mm", "Weld tolerance", 1e-6, 0.0, 10.0).unit("mm").help("Vertices closer than this are merged before building the cage."))
            .help("Turns a quad or polygon mesh into a subdivision cage: welds and orients the faces, marks creases and corners, and optionally solves for an interpolating cage. Crease edges travel as two-point lines and corners as the point attribute 'corner'.")
            .emulates("Rhino ToSubD")
            .emulates("QuadRemesh convert-to-SubD option")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let mesh = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &mesh)?;
        let tol = params.float("weld_tol_mm")?.max(1.2e-7 * max_abs_coord(&mesh.points));
        let mut sub = Subd::from_geometry(&mesh, tol);
        if sub.faces.is_empty() {
            return Err(ctx.fail("the mesh has no usable faces"));
        }
        let adj = sub.adjacency();
        let mut report = Report::new("Mesh to subdivision surface");
        let nonmanifold = adj.edge_faces.iter().filter(|f| f.len() > 2).count();
        let angle = params.float("crease_angle_deg")?;
        if params.bool("creases")? {
            for e in 0..adj.edges.len() {
                if dihedral_deg(&sub, &adj, e) > angle {
                    sub.creases.insert(adj.edges[e]);
                }
            }
        }
        if params.bool("corners")? {
            let adj2 = sub.adjacency();
            for v in 0..sub.points.len() as u32 {
                let sharp = adj2.vert_edges[v as usize].iter().filter(|&&e| sub.is_sharp(&adj2, e as usize)).count();
                if adj2.vert_faces[v as usize].len() == 1 || sharp >= 3 {
                    sub.corners.insert(v);
                }
            }
        }
        let mut resid = 0.0;
        if params.bool("interpolate")? {
            if sub.points.len() > 60_000 {
                report.note(Severity::Warning, "interpolation skipped: the cage has more than 60000 vertices");
            } else if let Some((cage, r)) = interpolate_cage(&sub) {
                sub.points = cage;
                resid = r;
            }
        }
        let adj = sub.adjacency();
        let quads = sub.faces.iter().filter(|f| f.len() == 4).count();
        report.metric("cage_vertices", Metric::new(sub.points.len() as f64, "count"));
        report.metric("cage_faces", Metric::new(sub.faces.len() as f64, "count"));
        report.metric("quad_fraction", Metric::new(quads as f64 / sub.faces.len() as f64, ""));
        report.metric("crease_edges", Metric::new(sub.creases.len() as f64, "count"));
        report.metric("corner_vertices", Metric::new(sub.corners.len() as f64, "count"));
        report.metric("extraordinary_vertices", Metric::new(sub.extraordinary_count(&adj) as f64, "count"));
        report.metric("boundary_edges", Metric::new(adj.edge_faces.iter().filter(|f| f.len() == 1).count() as f64, "count"));
        report.metric("interpolation_residual_mm", Metric::new(resid, "mm"));
        if nonmanifold > 0 {
            report.note(Severity::Warning, format!("{nonmanifold} non-manifold edges were kept as creases"));
            for (e, f) in adj.edge_faces.iter().enumerate() {
                if f.len() > 2 {
                    sub.creases.insert(adj.edges[e]);
                }
            }
        }
        Ok(vec![Data::geometry(sub.to_geometry()), Data::report(report)])
    }
}

/// `surface.subd_refine`
pub struct SubdRefineNode;

impl NodeOp for SubdRefineNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.subd_refine", "Subdivision Refine", SPEC_MESH)
            .input(PortSpec::new("cage", DataKind::Geometry))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::int("levels", "Levels", 2, 0, 6).help("Refinement steps; every step multiplies the quad count by four."))
            .param(ParamSpec::choice("output", "Output", "level", &["level", "cage", "limit"]).help("The refined mesh, the unchanged cage, or the refined mesh moved onto the limit surface."))
            .param(ParamSpec::int("max_faces", "Face limit", 2_000_000, 1000, 100_000_000).help("Refuse to refine when the result would have more faces than this."))
            .param(ParamSpec::float("weld_tol_mm", "Weld tolerance", 1e-6, 0.0, 10.0).unit("mm").help("Vertices closer than this are merged first."))
            .help("Applies Catmull-Clark subdivision to a cage: creases and boundaries stay sharp, pinned corners stay fixed. Polygon faces become quads after the first step.")
            .emulates("Rhino SubDivide")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let cage = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &cage)?;
        let levels = params.int("levels")?.clamp(0, 6) as usize;
        let max_faces = positive(ctx, "max_faces", params.float("max_faces")?)? as usize;
        let tol = params.float("weld_tol_mm")?.max(1.2e-7 * max_abs_coord(&cage.points));
        let sub = Subd::from_geometry(&cage, tol);
        if sub.faces.is_empty() {
            return Err(ctx.fail("the cage has no usable faces"));
        }
        let mode = params.str("output")?.to_string();
        let mut predicted = sub.faces.len();
        if levels >= 1 {
            predicted = sub.refined_face_count();
            for _ in 1..levels {
                predicted = predicted.saturating_mul(4);
            }
        }
        if mode != "cage" && predicted > max_faces {
            return Err(ctx.fail(format!("{levels} levels would create {predicted} faces, above the limit of {max_faces}; lower the level count or raise the limit")));
        }
        let mut report = Report::new("Subdivision refine");
        let out_sub = if mode == "cage" { sub.clone() } else { sub.refine_levels(levels) };
        let mut geo = out_sub.to_geometry();
        if mode == "limit" {
            let lim = out_sub.limit_positions();
            geo.points[..lim.len()].copy_from_slice(&lim);
        }
        let adj = out_sub.adjacency();
        report.metric("levels", Metric::new(if mode == "cage" { 0.0 } else { levels as f64 }, "count"));
        report.metric("faces", Metric::new(out_sub.faces.len() as f64, "count"));
        report.metric("vertices", Metric::new(out_sub.points.len() as f64, "count"));
        report.metric("edges", Metric::new(adj.edges.len() as f64, "count"));
        report.metric("boundary_edges", Metric::new(adj.edge_faces.iter().filter(|f| f.len() == 1).count() as f64, "count"));
        report.metric("euler_char", Metric::new(out_sub.points.len() as f64 - adj.edges.len() as f64 + out_sub.faces.len() as f64, ""));
        Ok(vec![Data::geometry(geo), Data::report(report)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{b, cook, f, geo_of, grid, i, report_of, s, some};

    fn cube(h: f64) -> Subd {
        Subd::from_geometry(&Geometry::make_box(DVec3::splat(2.0 * h)), 1e-9)
    }

    #[test]
    fn unit_square_one_step_matches_hand_values() {
        // spec S24: 9 vertices, 4 quads; boundary corners (6P + a + b) / 8
        let g = Geometry::from_polygons(vec![DVec3::ZERO, DVec3::X, DVec3::new(1.0, 1.0, 0.0), DVec3::Y], vec![vec![0, 1, 2, 3]]);
        let r = Subd::from_geometry(&g, 1e-9).refine();
        assert_eq!((r.points.len(), r.faces.len()), (9, 4));
        let has = |p: DVec3| r.points.iter().any(|q| (*q - p).length() < 1e-12);
        assert!(has(DVec3::new(0.125, 0.125, 0.0)));
        assert!(has(DVec3::new(0.5, 0.0, 0.0)) && has(DVec3::new(1.0, 0.5, 0.0)) && has(DVec3::new(0.5, 1.0, 0.0)) && has(DVec3::new(0.0, 0.5, 0.0)));
        assert!(has(DVec3::new(0.5, 0.5, 0.0)));
        let adj = r.adjacency();
        assert_eq!(r.points.len() as i64 - adj.edges.len() as i64 + r.faces.len() as i64, 1);
    }

    #[test]
    fn cube_corner_after_one_step_and_in_the_limit() {
        // spec S1 golden values on the cube [-1, 1]^3
        let c = cube(1.0);
        let r = c.refine();
        let corner = c.points.iter().position(|p| (*p - DVec3::ONE).length() < 1e-12).unwrap();
        assert!((r.points[corner] - DVec3::splat(5.0 / 9.0)).length() < 1e-12);
        let lim = c.limit_positions();
        assert!((lim[corner] - DVec3::splat(0.5)).length() < 1e-9);
        // limit of a deep refinement agrees with the stencil
        let deep = c.refine_levels(6);
        let corner_deep = deep.points[corner];
        assert!((corner_deep - DVec3::splat(0.5)).length() < 5e-3);
        let adj = r.adjacency();
        assert!(adj.edge_faces.iter().all(|f| f.len() == 2));
        assert_eq!(r.points.len() as i64 - adj.edges.len() as i64 + r.faces.len() as i64, 2);
    }

    #[test]
    fn constant_and_translated_meshes_are_invariant() {
        let c = cube(1.0);
        let mut k = c.clone();
        for p in &mut k.points {
            *p = DVec3::new(3.0, -2.0, 7.0);
        }
        let r = k.refine_levels(2);
        assert!(r.points.iter().all(|p| (*p - DVec3::new(3.0, -2.0, 7.0)).length() < 1e-12), "scale bug");
        let t = DVec3::new(10.0, 20.0, -30.0);
        let mut shifted = c.clone();
        for p in &mut shifted.points {
            *p += t;
        }
        let (a, bb) = (c.refine_levels(2), shifted.refine_levels(2));
        for (p, q) in a.points.iter().zip(&bb.points) {
            assert!((*p + t - *q).length() < 1e-12);
        }
    }

    #[test]
    fn interpolating_cube_cage_is_double_size() {
        // spec S2
        let c = cube(1.0);
        let (cage, resid) = interpolate_cage(&c).unwrap();
        assert!(resid < 1e-9, "resid {resid}");
        for (p, q) in cage.iter().zip(&c.points) {
            assert!((*p - *q * 2.0).length() < 1e-8, "{p:?} vs {q:?}");
        }
        let mut solved = c.clone();
        solved.points = cage;
        for (l, q) in solved.limit_positions().iter().zip(&c.points) {
            assert!((*l - *q).length() < 1e-8);
        }
    }

    #[test]
    fn fully_creased_cube_keeps_its_corners_and_straight_edges() {
        // spec S3
        let mut c = cube(1.0);
        let adj = c.adjacency();
        c.creases = adj.edges.iter().copied().collect();
        let lim = c.limit_positions();
        for (l, p) in lim.iter().zip(&c.points) {
            assert!((*l - *p).length() < 1e-12);
        }
        let r = c.refine_levels(3);
        // every refined crease segment stays on an original cube edge
        for &(a, bb) in &r.creases {
            let (p, q) = (r.points[a as usize], r.points[bb as usize]);
            let on_edge = |x: DVec3| x.abs().cmpeq(DVec3::ONE).bitmask().count_ones() >= 2;
            assert!(on_edge(p) && on_edge(q));
        }
        // faces bulge outwards relative to the cube faces? they stay inside the cube
        assert!(r.points.iter().all(|p| p.abs().max_element() <= 1.0 + 1e-12));
    }

    #[test]
    fn corner_flag_pins_grid_corners() {
        // spec S4: 4 x 3 flat grid, open boundary
        let mut sub = Subd::from_geometry(&grid(4, 3, 4.0, 3.0, |_, _| 0.0), 1e-9);
        let corner = sub.points.iter().position(|p| p.length() < 1e-12).unwrap();
        let rounded = sub.refine_levels(2).points[corner];
        assert!(rounded.length() > 1e-3, "unpinned corner should round off");
        sub.corners.insert(corner as u32);
        let pinned = sub.refine_levels(2).points[corner];
        assert!(pinned.length() < 1e-12);
    }

    #[test]
    fn node_chain_matches_direct_refinement_and_checks_the_cap() {
        let cube_geo = Geometry::make_box(DVec3::splat(2.0));
        let out = cook("surface.mesh_to_subd", &[some(&cube_geo)], &[]).unwrap();
        let cage = geo_of(&out[0]);
        let rep = report_of(&out[1]);
        assert_eq!(rep.metrics["cage_faces"].value, 6.0);
        assert_eq!(rep.metrics["extraordinary_vertices"].value, 8.0);
        assert_eq!(rep.metrics["crease_edges"].value, 12.0, "90 degree edges exceed the 30 degree crease angle");
        let out = cook("surface.subd_refine", &[some(&cage)], &[("levels", i(2))]).unwrap();
        let rep = report_of(&out[1]);
        assert_eq!(rep.metrics["faces"].value, 96.0);
        assert_eq!(rep.metrics["euler_char"].value, 2.0);
        // 1000-quad cage refused above the cap
        let big = grid(40, 25, 40.0, 25.0, |_, _| 0.0);
        let err = cook("surface.subd_refine", &[some(&big)], &[("levels", i(5)), ("max_faces", f(100_000.0))]);
        assert!(err.is_err());
        let ok = cook("surface.subd_refine", &[some(&big)], &[("levels", i(1)), ("output", s("limit"))]);
        assert!(ok.is_ok());
        // interpolate through the node
        let out = cook("surface.mesh_to_subd", &[some(&cube_geo)], &[("interpolate", b(true)), ("creases", b(false))]).unwrap();
        let cage = geo_of(&out[0]);
        assert!(cage.points.iter().all(|p| (p.abs().max_element() - 2.0).abs() < 1e-6));
    }
}

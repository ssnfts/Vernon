//! Topology and shading operators: fuse, normals, reverse, triangulate,
//! linear subdivide.

use std::collections::HashMap;

use vernon_core::{AttribArray, DVec2, DVec3, Geometry, Prim};
use vernon_graph::node::inputs;
use vernon_graph::{
    CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec,
    Stage,
};

use crate::util::{
    derived_prim_data, int_clamped, interpolate_point_groups, interpolate_points,
    interpolate_table, non_negative, prim_selection, remap_group, reverse_winding, Recipe, SPEC,
};

fn geo_in() -> PortSpec {
    PortSpec::new("geometry", DataKind::Geometry)
}

fn geo_out() -> PortSpec {
    PortSpec::new("geometry", DataKind::Geometry)
}

// ----------------------------------------------------------------- fuse ---

fn find(parent: &mut [u32], mut i: u32) -> u32 {
    while parent[i as usize] != i {
        let p = parent[i as usize];
        parent[i as usize] = parent[p as usize];
        i = p;
    }
    i
}

/// Merge points closer than `tol` (single-linkage clusters). The merged point
/// sits at the cluster average when `average`, else at its lowest-index
/// member. Consecutive repeated vertices are removed from primitives; when
/// `remove_degenerate`, primitives left with too few vertices are deleted.
/// Returns the new geometry and the number of points removed.
pub fn fuse_points(geo: &Geometry, tol: f64, average: bool, remove_degenerate: bool) -> (Geometry, usize) {
    let n = geo.points.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    let tol = if tol.is_finite() { tol.max(0.0) } else { 0.0 };
    if tol > 0.0 {
        let cell = |p: DVec3| -> (i64, i64, i64) {
            let c = (p / tol).floor();
            (c.x as i64, c.y as i64, c.z as i64)
        };
        let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
        let tol2 = tol * tol;
        for i in 0..n {
            let p = geo.points[i];
            let (cx, cy, cz) = cell(p);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if let Some(list) = grid.get(&(cx + dx, cy + dy, cz + dz)) {
                            for &j in list {
                                if geo.points[j as usize].distance_squared(p) <= tol2 {
                                    let (ri, rj) = (find(&mut parent, i as u32), find(&mut parent, j));
                                    if ri != rj {
                                        let (lo, hi) = (ri.min(rj), ri.max(rj));
                                        parent[hi as usize] = lo;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            grid.entry((cx, cy, cz)).or_default().push(i as u32);
        }
    } else {
        let mut seen: HashMap<[u64; 3], u32> = HashMap::new();
        for (i, p) in geo.points.iter().enumerate() {
            let key = [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
            parent[i] = *seen.entry(key).or_insert(i as u32);
        }
    }

    // Representatives in index order.
    let mut map: Vec<Option<u32>> = vec![None; n];
    let mut members: Vec<Vec<usize>> = Vec::new();
    for i in 0..n {
        let r = find(&mut parent, i as u32) as usize;
        let new = match map[r] {
            Some(k) => k,
            None => {
                let k = members.len() as u32;
                map[r] = Some(k);
                members.push(Vec::new());
                k
            }
        };
        map[i] = Some(new);
        members[new as usize].push(i);
    }
    let recipes: Vec<Recipe> = members
        .iter()
        .map(|m| {
            if average {
                let w = 1.0 / m.len() as f64;
                m.iter().map(|&i| (i, w)).collect()
            } else {
                vec![(m[0], 1.0)]
            }
        })
        .collect();

    let mut prims = Vec::with_capacity(geo.prims.len());
    let mut keep = Vec::with_capacity(geo.prims.len());
    for p in &geo.prims {
        let mut v: Vec<u32> = Vec::with_capacity(p.vertices.len());
        for &old in &p.vertices {
            let nv = map[old as usize].expect("mapped");
            if v.last() != Some(&nv) {
                v.push(nv);
            }
        }
        if p.closed && v.len() > 1 && v.first() == v.last() {
            v.pop();
        }
        let ok = if p.closed { v.len() >= 3 } else { v.len() >= 2 };
        keep.push(ok || !remove_degenerate);
        prims.push(Prim { vertices: v, closed: p.closed });
    }
    let src: Vec<usize> = (0..prims.len()).filter(|&i| keep[i]).collect();
    let (prim_attribs, prim_groups) = derived_prim_data(geo, &src);
    let point_groups = geo
        .point_groups
        .iter()
        .map(|(k, v)| (k.clone(), remap_group(v, &map)))
        .collect();
    let out = Geometry {
        points: interpolate_points(&geo.points, &recipes),
        prims: src.iter().map(|&i| prims[i].clone()).collect(),
        point_attribs: interpolate_table(&geo.point_attribs, &recipes),
        prim_attribs,
        detail_attribs: geo.detail_attribs.clone(),
        point_groups,
        prim_groups,
    };
    let removed = n - members.len();
    (out, removed)
}

pub struct FuseNode;

impl NodeOp for FuseNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.fuse", "Fuse", Stage::Geometry)
            .input(geo_in())
            .output(geo_out())
            .param(
                ParamSpec::float("tolerance", "Distance", 0.001, 0.0, 1000.0)
                    .unit("mm")
                    .help("Points closer than this are merged into one; 0 merges exact duplicates only."),
            )
            .param(
                ParamSpec::choice("snap", "Merged position", "average", &["average", "first"])
                    .help("Place each merged point at the average of its cluster or at its first member."),
            )
            .param(
                ParamSpec::bool("remove_degenerate", "Remove degenerate", true)
                    .help("Delete faces and lines that collapse to too few vertices."),
            )
            .help("Welds points that lie within a distance of each other and cleans up collapsed faces.")
            .emulates("Weld / consolidate points as in 3ds Max Weld or the Houdini Fuse SOP")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, _: &CookContext, ins: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        let (out, _) = fuse_points(
            &g,
            non_negative(params, "tolerance")?,
            params.str("snap")? == "average",
            params.bool("remove_degenerate")?,
        );
        Ok(vec![Data::geometry(out)])
    }
}

// -------------------------------------------------------------- normals ---

/// Normal weighting scheme for point normals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weighting {
    Area,
    Angle,
    Equal,
}

/// Face normals (Newell) and weighted point normals.
pub fn weighted_normals(geo: &Geometry, weighting: Weighting) -> (Vec<DVec3>, Vec<DVec3>) {
    let mut pn = vec![DVec3::ZERO; geo.points.len()];
    let mut fnorm = vec![DVec3::ZERO; geo.prims.len()];
    for (pi, prim) in geo.prims.iter().enumerate() {
        let v = &prim.vertices;
        if !prim.closed || v.len() < 3 {
            continue;
        }
        let n = geo.prim_normal(pi);
        fnorm[pi] = n;
        let area = geo.prim_area(pi);
        for k in 0..v.len() {
            let w = match weighting {
                Weighting::Area => area,
                Weighting::Equal => 1.0,
                Weighting::Angle => {
                    let c = geo.points[v[k] as usize];
                    let a = geo.points[v[(k + v.len() - 1) % v.len()] as usize] - c;
                    let b = geo.points[v[(k + 1) % v.len()] as usize] - c;
                    if a.length_squared() > 0.0 && b.length_squared() > 0.0 {
                        a.angle_between(b)
                    } else {
                        0.0
                    }
                }
            };
            pn[v[k] as usize] += n * w;
        }
    }
    for n in &mut pn {
        *n = n.normalize_or_zero();
    }
    (pn, fnorm)
}

pub struct NormalsNode;

impl NodeOp for NormalsNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.normals", "Normals", Stage::Geometry)
            .input(geo_in())
            .output(geo_out())
            .param(
                ParamSpec::choice("class", "Add to", "both", &["points", "prims", "both"])
                    .help("Write point normals, face normals or both into the N attribute."),
            )
            .param(
                ParamSpec::choice("weighting", "Weighting", "angle", &["angle", "area", "equal"])
                    .help("How neighbouring faces contribute to a point normal."),
            )
            .help("Computes face normals and weighted point normals into the N attribute.")
            .emulates("Normal computation as in the Houdini Normal SOP and 3ds Max Edit Normals / Smooth")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        if !g.prims.iter().any(|p| p.closed && p.vertices.len() >= 3) {
            return Err(ctx.fail(
                "input has no faces; point-cloud normals need a neighbourhood-based estimator",
            ));
        }
        let weighting = match params.str("weighting")? {
            "area" => Weighting::Area,
            "equal" => Weighting::Equal,
            _ => Weighting::Angle,
        };
        let class = params.str("class")?;
        let (pn, fnorm) = weighted_normals(&g, weighting);
        let mut out = (*g).clone();
        if class != "prims" {
            out.point_attribs.set("N", AttribArray::Vec3(pn));
        }
        if class != "points" {
            out.prim_attribs.set("N", AttribArray::Vec3(fnorm));
        }
        Ok(vec![Data::geometry(out)])
    }
}

// -------------------------------------------------------------- reverse ---

pub struct ReverseNode;

impl NodeOp for ReverseNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.reverse", "Reverse", Stage::Geometry)
            .input(geo_in())
            .output(geo_out())
            .param(
                ParamSpec::string("group", "Group", "")
                    .help("Primitive group to flip; empty flips every face."),
            )
            .param(
                ParamSpec::bool("flip_normals", "Flip N", true)
                    .help("Also negate existing N attributes of the affected elements."),
            )
            .help("Reverses face winding so normals point the other way.")
            .emulates("Flip normals / reverse winding as in 3ds Max Normal modifier (Flip) or the Houdini Reverse SOP")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        let sel = prim_selection(&g, params.str("group")?).map_err(|e| ctx.fail(e))?;
        let mut out = (*g).clone();
        reverse_winding(&mut out, sel.as_deref());
        if params.bool("flip_normals")? {
            let selected = |i: usize| sel.as_ref().is_none_or(|s| s[i]);
            if let Some(AttribArray::Vec3(ns)) = out.prim_attribs.get_mut("N") {
                for (i, n) in ns.iter_mut().enumerate() {
                    if selected(i) {
                        *n = -*n;
                    }
                }
            }
            let mut flip_pt = vec![sel.is_none(); out.points.len()];
            if let Some(s) = &sel {
                for (i, p) in out.prims.iter().enumerate() {
                    if s[i] && p.closed {
                        for &v in &p.vertices {
                            flip_pt[v as usize] = true;
                        }
                    }
                }
            }
            if let Some(AttribArray::Vec3(ns)) = out.point_attribs.get_mut("N") {
                for (i, n) in ns.iter_mut().enumerate() {
                    if flip_pt.get(i).copied().unwrap_or(false) {
                        *n = -*n;
                    }
                }
            }
        }
        Ok(vec![Data::geometry(out)])
    }
}

// ---------------------------------------------------------- triangulate ---

fn cross2(a: DVec2, b: DVec2) -> f64 {
    a.x * b.y - a.y * b.x
}

fn point_in_tri(p: DVec2, a: DVec2, b: DVec2, c: DVec2) -> bool {
    let d1 = cross2(b - a, p - a);
    let d2 = cross2(c - b, p - b);
    let d3 = cross2(a - c, p - c);
    d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0
}

/// Triangulate one polygon given its 3D corner positions, returning corner
/// index triples that keep the polygon's winding. Ear clipping on the
/// polygon projected onto its Newell plane; convex quads are split along the
/// shorter diagonal. Falls back to a fan if no ear can be found.
pub fn triangulate_polygon(pts: &[DVec3]) -> Vec<[usize; 3]> {
    let n = pts.len();
    if n < 3 {
        return vec![];
    }
    if n == 3 {
        return vec![[0, 1, 2]];
    }
    let mut normal = DVec3::ZERO;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        normal += DVec3::new(
            (a.y - b.y) * (a.z + b.z),
            (a.z - b.z) * (a.x + b.x),
            (a.x - b.x) * (a.y + b.y),
        );
    }
    let normal = normal.normalize_or_zero();
    if normal == DVec3::ZERO {
        return (1..n - 1).map(|i| [0, i, i + 1]).collect();
    }
    let u = normal.any_orthonormal_vector();
    let v = normal.cross(u);
    let p2: Vec<DVec2> = pts.iter().map(|p| DVec2::new(p.dot(u), p.dot(v))).collect();
    let scale = pts
        .iter()
        .map(|p| (*p - pts[0]).length())
        .fold(0.0_f64, f64::max)
        .max(1e-300);
    let eps = 1e-12 * scale * scale;

    if n == 4 {
        let convex = (0..4).all(|i| {
            let a = p2[(i + 3) % 4];
            let b = p2[i];
            let c = p2[(i + 1) % 4];
            cross2(b - a, c - b) > eps
        });
        if convex {
            return if pts[0].distance_squared(pts[2]) <= pts[1].distance_squared(pts[3]) {
                vec![[0, 1, 2], [0, 2, 3]]
            } else {
                vec![[1, 2, 3], [1, 3, 0]]
            };
        }
    }

    let mut idx: Vec<usize> = (0..n).collect();
    let mut tris = Vec::with_capacity(n - 2);
    while idx.len() > 3 {
        let m = idx.len();
        let mut found = None;
        for k in 0..m {
            let (ia, ib, ic) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            let (a, b, c) = (p2[ia], p2[ib], p2[ic]);
            if cross2(b - a, c - b) <= eps {
                continue;
            }
            let blocked = idx
                .iter()
                .any(|&j| j != ia && j != ib && j != ic && point_in_tri(p2[j], a, b, c));
            if !blocked {
                found = Some(k);
                break;
            }
        }
        match found {
            Some(k) => {
                tris.push([idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]]);
                idx.remove(k);
            }
            None => break,
        }
    }
    if idx.len() >= 3 {
        for i in 1..idx.len() - 1 {
            tris.push([idx[0], idx[i], idx[i + 1]]);
        }
    }
    tris
}

/// Triangulate selected closed primitives (`sel = None` means all).
pub fn triangulate(geo: &Geometry, sel: Option<&[bool]>, fan: bool) -> Geometry {
    let mut prims = Vec::new();
    let mut src = Vec::new();
    for (pi, p) in geo.prims.iter().enumerate() {
        let selected = sel.is_none_or(|s| s[pi]);
        if !selected || !p.closed || p.vertices.len() <= 3 {
            prims.push(p.clone());
            src.push(pi);
            continue;
        }
        let v = &p.vertices;
        let tris: Vec<[usize; 3]> = if fan {
            (1..v.len() - 1).map(|i| [0, i, i + 1]).collect()
        } else {
            let pts: Vec<DVec3> = v.iter().map(|&i| geo.points[i as usize]).collect();
            triangulate_polygon(&pts)
        };
        for t in tris {
            prims.push(Prim::polygon(vec![v[t[0]], v[t[1]], v[t[2]]]));
            src.push(pi);
        }
    }
    let (prim_attribs, prim_groups) = derived_prim_data(geo, &src);
    Geometry {
        prims,
        prim_attribs,
        prim_groups,
        ..geo.clone()
    }
}

pub struct TriangulateNode;

impl NodeOp for TriangulateNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.triangulate", "Triangulate", Stage::Geometry)
            .input(geo_in())
            .output(geo_out())
            .param(
                ParamSpec::choice("method", "Method", "ear_clip", &["ear_clip", "fan"])
                    .help("Ear clipping handles concave faces; fan is fastest for convex ones."),
            )
            .param(
                ParamSpec::string("group", "Group", "")
                    .help("Primitive group to triangulate; empty means every face."),
            )
            .help("Splits faces with more than three corners into triangles, keeping their attributes and groups.")
            .emulates("Triangulate as in 3ds Max Turn to Mesh / Houdini Divide SOP (triangulate)")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        let sel = prim_selection(&g, params.str("group")?).map_err(|e| ctx.fail(e))?;
        let out = triangulate(&g, sel.as_deref(), params.str("method")? == "fan");
        Ok(vec![Data::geometry(out)])
    }
}

// ------------------------------------------------------------ subdivide ---

/// One level of linear subdivision. Positions are not smoothed, so the
/// surface shape and area are unchanged. Triangles split into four
/// triangles (or three quads when `tris_to_quads`), other faces into one
/// quad per corner around their centroid, polylines at segment midpoints.
pub fn subdivide_linear(geo: &Geometry, tris_to_quads: bool) -> Geometry {
    let mut recipes: Vec<Recipe> = (0..geo.points.len()).map(|i| vec![(i, 1.0)]).collect();
    let mut mids: HashMap<(u32, u32), u32> = HashMap::new();
    let mut mid = |a: u32, b: u32, recipes: &mut Vec<Recipe>| -> u32 {
        if a == b {
            return a;
        }
        *mids.entry((a.min(b), a.max(b))).or_insert_with(|| {
            recipes.push(vec![(a as usize, 0.5), (b as usize, 0.5)]);
            (recipes.len() - 1) as u32
        })
    };
    let mut prims = Vec::new();
    let mut src = Vec::new();
    for (pi, p) in geo.prims.iter().enumerate() {
        let v = &p.vertices;
        let n = v.len();
        if !p.closed {
            if n < 2 {
                prims.push(p.clone());
            } else {
                let mut nv = vec![v[0]];
                for k in 1..n {
                    nv.push(mid(v[k - 1], v[k], &mut recipes));
                    nv.push(v[k]);
                }
                prims.push(Prim::polyline(nv));
            }
            src.push(pi);
            continue;
        }
        if n < 3 {
            prims.push(p.clone());
            src.push(pi);
            continue;
        }
        let m: Vec<u32> = (0..n).map(|k| mid(v[k], v[(k + 1) % n], &mut recipes)).collect();
        if n == 3 && !tris_to_quads {
            let (a, b, c) = (v[0], v[1], v[2]);
            let (m01, m12, m20) = (m[0], m[1], m[2]);
            for t in [[a, m01, m20], [m01, b, m12], [m20, m12, c], [m01, m12, m20]] {
                prims.push(Prim::polygon(t.to_vec()));
                src.push(pi);
            }
        } else {
            let w = 1.0 / n as f64;
            recipes.push(v.iter().map(|&i| (i as usize, w)).collect());
            let c = (recipes.len() - 1) as u32;
            for k in 0..n {
                prims.push(Prim::polygon(vec![v[k], m[k], c, m[(k + n - 1) % n]]));
                src.push(pi);
            }
        }
    }
    let (prim_attribs, prim_groups) = derived_prim_data(geo, &src);
    Geometry {
        points: interpolate_points(&geo.points, &recipes),
        prims,
        point_attribs: interpolate_table(&geo.point_attribs, &recipes),
        prim_attribs,
        detail_attribs: geo.detail_attribs.clone(),
        point_groups: interpolate_point_groups(&geo.point_groups, &recipes),
        prim_groups,
    }
}

pub struct SubdivideNode;

impl NodeOp for SubdivideNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.subdivide", "Subdivide", Stage::Geometry)
            .input(geo_in())
            .output(geo_out())
            .param(
                ParamSpec::int("iterations", "Iterations", 1, 0, 8)
                    .help("How many times to split every face."),
            )
            .param(
                ParamSpec::bool("tris_to_quads", "Triangles to quads", false)
                    .help("Split triangles into three quads instead of four triangles."),
            )
            .help("Splits faces at edge midpoints and centroids without moving the surface, interpolating point attributes.")
            .emulates("Linear (non-smoothing) subdivision as in 3ds Max Tessellate (edge/face, tension 0) and Houdini Subdivide SOP in linear mode")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, _: &CookContext, ins: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        let iters = int_clamped(params, "iterations", 0, 8)?;
        let tq = params.bool("tris_to_quads")?;
        let mut out = (*g).clone();
        for _ in 0..iters {
            out = subdivide_linear(&out, tq);
        }
        Ok(vec![Data::geometry(out)])
    }
}

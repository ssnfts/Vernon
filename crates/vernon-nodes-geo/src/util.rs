//! Shared helpers: element filtering with attribute/group bookkeeping,
//! attribute interpolation and small vector utilities.

use std::collections::{BTreeMap, HashMap, HashSet};

use vernon_core::{AttribArray, AttribTable, DVec3, Geometry, Prim};
use vernon_graph::{CookResult, Params};

/// Spec every node in this crate follows.
pub const SPEC: &str = "docs/specs/geometry-basics.md";

/// Remap a sorted index group through `map` (old index -> new index), dropping
/// removed elements and duplicates.
pub fn remap_group(idx: &[u32], map: &[Option<u32>]) -> Vec<u32> {
    let mut out: Vec<u32> = idx
        .iter()
        .filter_map(|&i| map.get(i as usize).copied().flatten())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Remap a group where each old element became zero or more new elements.
pub fn expand_group(idx: &[u32], children: &[Vec<u32>]) -> Vec<u32> {
    let mut out: Vec<u32> = idx
        .iter()
        .filter_map(|&i| children.get(i as usize))
        .flatten()
        .copied()
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn index_map(keep: &[bool]) -> (Vec<usize>, Vec<Option<u32>>) {
    let mut idx = Vec::new();
    let mut map = vec![None; keep.len()];
    for (i, &k) in keep.iter().enumerate() {
        if k {
            map[i] = Some(idx.len() as u32);
            idx.push(i);
        }
    }
    (idx, map)
}

/// Keep only the primitives where `keep[i]` is true. Points are untouched.
pub fn filter_prims(geo: &Geometry, keep: &[bool]) -> Geometry {
    let (idx, map) = index_map(keep);
    Geometry {
        points: geo.points.clone(),
        prims: idx.iter().map(|&i| geo.prims[i].clone()).collect(),
        point_attribs: geo.point_attribs.clone(),
        prim_attribs: geo.prim_attribs.gather_all(&idx),
        detail_attribs: geo.detail_attribs.clone(),
        point_groups: geo.point_groups.clone(),
        prim_groups: geo
            .prim_groups
            .iter()
            .map(|(k, v)| (k.clone(), remap_group(v, &map)))
            .collect(),
    }
}

/// Keep only the points where `keep[i]` is true. Primitives that reference a
/// removed point are removed too.
pub fn filter_points(geo: &Geometry, keep: &[bool]) -> Geometry {
    let keep_prim: Vec<bool> = geo
        .prims
        .iter()
        .map(|p| p.vertices.iter().all(|&v| keep[v as usize]))
        .collect();
    let g = filter_prims(geo, &keep_prim);
    compact_points(&g, keep)
}

/// Remove points not in `keep`. The caller guarantees no primitive references
/// a removed point.
pub fn compact_points(geo: &Geometry, keep: &[bool]) -> Geometry {
    let (idx, map) = index_map(keep);
    Geometry {
        points: idx.iter().map(|&i| geo.points[i]).collect(),
        prims: geo
            .prims
            .iter()
            .map(|p| Prim {
                vertices: p
                    .vertices
                    .iter()
                    .map(|&v| map[v as usize].expect("prim references a removed point"))
                    .collect(),
                closed: p.closed,
            })
            .collect(),
        point_attribs: geo.point_attribs.gather_all(&idx),
        prim_attribs: geo.prim_attribs.clone(),
        detail_attribs: geo.detail_attribs.clone(),
        point_groups: geo
            .point_groups
            .iter()
            .map(|(k, v)| (k.clone(), remap_group(v, &map)))
            .collect(),
        prim_groups: geo.prim_groups.clone(),
    }
}

/// Which points are referenced by at least one primitive.
pub fn used_points(geo: &Geometry) -> Vec<bool> {
    let mut used = vec![false; geo.points.len()];
    for p in &geo.prims {
        for &v in &p.vertices {
            if let Some(u) = used.get_mut(v as usize) {
                *u = true;
            }
        }
    }
    used
}

/// A new element defined as a weighted blend of source elements.
pub type Recipe = Vec<(usize, f64)>;

fn dominant(r: &Recipe) -> usize {
    let mut best = r[0];
    for &c in &r[1..] {
        if c.1 > best.1 {
            best = c;
        }
    }
    best.0
}

/// Build a table whose elements blend the source elements per `recipes`.
/// Float and vector columns are interpolated (a vector column named `N` is
/// re-normalised); integer and string columns take the heaviest source.
pub fn interpolate_table(src: &AttribTable, recipes: &[Recipe]) -> AttribTable {
    let mut out = AttribTable::default();
    for (name, col) in src.iter() {
        let arr = match col {
            AttribArray::Float(v) => AttribArray::Float(
                recipes
                    .iter()
                    .map(|r| r.iter().map(|&(i, w)| v[i] * w).sum())
                    .collect(),
            ),
            AttribArray::Vec3(v) => AttribArray::Vec3(
                recipes
                    .iter()
                    .map(|r| {
                        let s: DVec3 = r.iter().map(|&(i, w)| v[i] * w).sum();
                        if name == "N" {
                            s.normalize_or_zero()
                        } else {
                            s
                        }
                    })
                    .collect(),
            ),
            AttribArray::Int(v) => AttribArray::Int(recipes.iter().map(|r| v[dominant(r)]).collect()),
            AttribArray::Str(v) => {
                AttribArray::Str(recipes.iter().map(|r| v[dominant(r)].clone()).collect())
            }
        };
        out.set(name, arr);
    }
    out
}

/// Blend positions per `recipes`.
pub fn interpolate_points(points: &[DVec3], recipes: &[Recipe]) -> Vec<DVec3> {
    recipes
        .iter()
        .map(|r| r.iter().map(|&(i, w)| points[i] * w).sum())
        .collect()
}

/// Point groups for interpolated points: a new point belongs to a group when
/// every source point of its recipe does.
pub fn interpolate_point_groups(
    groups: &BTreeMap<String, Vec<u32>>,
    recipes: &[Recipe],
) -> BTreeMap<String, Vec<u32>> {
    groups
        .iter()
        .map(|(name, idx)| {
            let set: HashSet<u32> = idx.iter().copied().collect();
            let members = recipes
                .iter()
                .enumerate()
                .filter(|(_, r)| r.iter().all(|&(i, _)| set.contains(&(i as u32))))
                .map(|(n, _)| n as u32)
                .collect();
            (name.clone(), members)
        })
        .collect()
}

/// Primitive attributes and groups after primitives were replaced:
/// `src_prim[k]` is the input primitive the new primitive `k` came from.
pub fn derived_prim_data(
    geo: &Geometry,
    src_prim: &[usize],
) -> (AttribTable, BTreeMap<String, Vec<u32>>) {
    let attribs = geo.prim_attribs.gather_all(src_prim);
    let mut children = vec![Vec::new(); geo.prims.len()];
    for (new, &old) in src_prim.iter().enumerate() {
        children[old].push(new as u32);
    }
    let groups = geo
        .prim_groups
        .iter()
        .map(|(k, v)| (k.clone(), expand_group(v, &children)))
        .collect();
    (attribs, groups)
}

/// Centroid (vertex average) of a primitive.
pub fn prim_centroid(geo: &Geometry, prim: usize) -> DVec3 {
    let v = &geo.prims[prim].vertices;
    if v.is_empty() {
        return DVec3::ZERO;
    }
    v.iter().map(|&i| geo.points[i as usize]).sum::<DVec3>() / v.len() as f64
}

/// Reverse the vertex order of closed primitives selected by `sel` (all when
/// `None`).
pub fn reverse_winding(geo: &mut Geometry, sel: Option<&[bool]>) {
    for (i, p) in geo.prims.iter_mut().enumerate() {
        if p.closed && sel.map_or(true, |s| s[i]) {
            p.vertices.reverse();
        }
    }
}

/// Undirected edge use counts over closed primitives.
pub fn edge_counts(geo: &Geometry) -> HashMap<(u32, u32), u32> {
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    for p in &geo.prims {
        if !p.closed || p.vertices.len() < 2 {
            continue;
        }
        let v = &p.vertices;
        for i in 0..v.len() {
            let a = v[i];
            let b = v[(i + 1) % v.len()];
            if a == b {
                continue;
            }
            *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    edges
}

/// Axis index for an `x`/`y`/`z` choice string.
pub fn axis_index(s: &str) -> usize {
    match s {
        "x" => 0,
        "y" => 1,
        _ => 2,
    }
}

/// Inside test for an axis-aligned box given by centre and size.
pub fn in_box(p: DVec3, center: DVec3, size: DVec3) -> bool {
    let d = (p - center).abs();
    let h = size.abs() * 0.5;
    d.x <= h.x && d.y <= h.y && d.z <= h.z
}

/// Parse a selection: indices of a named group, or `None` when the name is
/// empty (meaning "everything").
pub fn prim_selection(geo: &Geometry, group: &str) -> Result<Option<Vec<bool>>, String> {
    let group = group.trim();
    if group.is_empty() {
        return Ok(None);
    }
    let idx = geo
        .prim_groups
        .get(group)
        .ok_or_else(|| format!("no primitive group named '{group}'"))?;
    let mut sel = vec![false; geo.prims.len()];
    for &i in idx {
        if let Some(s) = sel.get_mut(i as usize) {
            *s = true;
        }
    }
    Ok(Some(sel))
}

/// Read a parameter as a non-negative finite float.
pub fn non_negative(params: &Params, name: &str) -> CookResult<f64> {
    let v = params.float(name)?;
    Ok(if v.is_finite() { v.max(0.0) } else { 0.0 })
}

/// Read an int parameter clamped to `[lo, hi]`.
pub fn int_clamped(params: &Params, name: &str, lo: i64, hi: i64) -> CookResult<usize> {
    Ok(params.int(name)?.clamp(lo, hi) as usize)
}

/// Point normals: the `N` point attribute if present and valid, otherwise
/// area-weighted normals from the faces.
pub fn point_normals(geo: &Geometry) -> Vec<DVec3> {
    if let Ok(n) = geo.point_attribs.vec3s("N") {
        if n.len() == geo.points.len() {
            return n.to_vec();
        }
    }
    let mut g = geo.clone();
    g.compute_normals();
    g.point_attribs
        .vec3s("N")
        .map(|n| n.to_vec())
        .unwrap_or_else(|_| vec![DVec3::ZERO; geo.points.len()])
}

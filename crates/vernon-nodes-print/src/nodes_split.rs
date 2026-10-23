//! Partition nodes: grid split to fit a bed, smart partition (placeholder
//! search), connectivity / attribute splitting, part extraction and labels.

use std::collections::{BTreeMap, HashMap};

use glam::DVec3;
use serde_json::json;
use vernon_core::{Aabb, AttribArray, Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::common::{info, placeholder, printer_bed, v3_json, PRINTER_IDS};
use crate::cut::{cross_section_area, cut_mesh, Plane};
use crate::mesh::{self, PART};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridMode {
    ByBed,
    ByCount,
    BySpacing,
}

/// Cut positions per axis plus the resulting slab counts.
#[derive(Debug, Clone, PartialEq)]
pub struct GridPlan {
    pub planes: [Vec<f64>; 3],
    pub counts: [usize; 3],
    pub usable: DVec3,
}

/// Plan the axis-aligned cut planes (spec A3, grid mode).
/// Most cut planes along one axis. A plan that reaches it comes from a body
/// or bed in the wrong units (or a non-finite point) and is refused.
pub const MAX_GRID_PLANES: usize = 256;

pub fn grid_plan(
    b: &Aabb,
    mode: GridMode,
    bed: DVec3,
    margin: f64,
    counts: [i64; 3],
    spacing: DVec3,
    offset: DVec3,
) -> GridPlan {
    let ext = b.size();
    let usable = bed - DVec3::splat(2.0 * margin);
    let diag = ext.length().max(1e-9);
    let mut planes: [Vec<f64>; 3] = [vec![], vec![], vec![]];
    for i in 0..3 {
        let (lo, hi, e) = (b.min[i], b.max[i], ext[i]);
        match mode {
            GridMode::ByBed | GridMode::ByCount => {
                let k = if mode == GridMode::ByBed {
                    let k = (e / usable[i] - 1e-9).ceil();
                    if k.is_finite() { (k as i64).max(1) } else { 1 }
                } else {
                    counts[i].max(1)
                }
                .min(MAX_GRID_PLANES as i64 + 1);
                planes[i] = (1..k).map(|j| lo + e * j as f64 / k as f64).collect();
            }
            GridMode::BySpacing => {
                let s = spacing[i];
                if s > 0.0 {
                    let tol = 1e-6 * diag;
                    let mut j = 0;
                    while planes[i].len() <= MAX_GRID_PLANES && j <= 64 * MAX_GRID_PLANES {
                        let pos = lo + offset[i] + s * j as f64;
                        if pos >= hi - tol {
                            break;
                        }
                        if pos > lo + tol {
                            planes[i].push(pos);
                        }
                        j += 1;
                    }
                }
            }
        }
    }
    let counts = [planes[0].len() + 1, planes[1].len() + 1, planes[2].len() + 1];
    GridPlan { planes, counts, usable }
}

struct Piece {
    geo: Geometry,
    cell: [usize; 3],
    origin_part: i64,
}

pub struct GridResult {
    pub geo: Geometry,
    pub plan_json: serde_json::Value,
    pub plane_count: usize,
    pub seam_area: f64,
    pub interfaces: usize,
    pub fit_margin: f64,
    pub largest: DVec3,
    pub part_count: usize,
}

/// Cut `geo` along the planned grid. `snap` moves each plane within +-`slack`
/// (bounded by the bed slack) to the position of least cross-section.
pub fn grid_split(
    ctx: &CookContext,
    geo: &Geometry,
    plan: &GridPlan,
    snap: bool,
    slack: f64,
) -> CookResult<GridResult> {
    if !geo.points.iter().all(|p| p.is_finite()) {
        return Err(ctx.fail("the body has non-finite point positions"));
    }
    if plan.planes.iter().any(|p| p.len() >= MAX_GRID_PLANES) {
        return Err(ctx.fail(format!(
            "the grid needs {MAX_GRID_PLANES} or more cuts along one axis; check the units of the body, bed and spacing"
        )));
    }
    let mut planes: [Vec<f64>; 3] = plan.planes.clone();
    if snap && slack > 0.0 {
        let ext = geo.bounds().size();
        for axis in 0..3 {
            let k = plan.counts[axis] as f64;
            let width = ext[axis] / k;
            let dmax = slack.min(((plan.usable[axis] - width) / 2.0).max(0.0));
            if dmax <= 0.0 {
                continue;
            }
            for pos in planes[axis].iter_mut() {
                let mut best = (f64::INFINITY, 0.0);
                let steps = (dmax / 0.5).floor() as i64;
                let mut cands: Vec<f64> = (-steps..=steps).map(|s| s as f64 * 0.5).collect();
                cands.sort_by(|a, b| a.abs().total_cmp(&b.abs()));
                for d in cands {
                    let mut n = DVec3::ZERO;
                    n[axis] = 1.0;
                    let mut o = DVec3::ZERO;
                    o[axis] = *pos + d;
                    let a = cross_section_area(geo, &Plane { origin: o, normal: n });
                    if a < best.0 * (1.0 - 1e-9) {
                        best = (a, d);
                    }
                }
                *pos += best.1;
            }
        }
    }
    let mut pieces: Vec<Piece> = mesh::unique_parts(geo)
        .into_iter()
        .map(|p| Piece { geo: mesh::extract_part(geo, p), cell: [0; 3], origin_part: p })
        .collect();
    let mut cap_id = 0i64;
    let mut seam_area = 0.0;
    let mut plane_list = Vec::new();
    for axis in 0..3 {
        for (j, &pos) in planes[axis].iter().enumerate() {
            cap_id += 1;
            let mut n = DVec3::ZERO;
            n[axis] = 1.0;
            let mut o = DVec3::ZERO;
            o[axis] = pos;
            let plane = Plane { origin: o, normal: n };
            let axis_name = ["x", "y", "z"][axis];
            plane_list.push(json!({"cap": cap_id, "axis": axis_name, "position_mm": pos,
                "origin": v3_json(o), "normal": v3_json(n)}));
            let mut next = Vec::with_capacity(pieces.len() * 2);
            for piece in pieces {
                if piece.cell[axis] != j {
                    next.push(piece);
                    continue;
                }
                let r = cut_mesh(&piece.geo, &plane, 1e-6, true, cap_id).map_err(|e| ctx.fail(e.to_string()))?;
                seam_area += r.cap_area;
                let mut up_cell = piece.cell;
                up_cell[axis] += 1;
                if !r.lower.prims.is_empty() {
                    next.push(Piece { geo: r.lower, cell: piece.cell, origin_part: piece.origin_part });
                }
                if !r.upper.prims.is_empty() {
                    next.push(Piece { geo: r.upper, cell: up_cell, origin_part: piece.origin_part });
                }
            }
            pieces = next;
        }
    }
    let k = plan.counts;
    let lin = |c: [usize; 3]| (c[0] + k[0] * (c[1] + k[1] * c[2])) as i64;
    pieces.sort_by_key(|p| (p.origin_part, lin(p.cell)));
    let mut fit_margin = f64::INFINITY;
    let mut largest = DVec3::ZERO;
    let mut cells: HashMap<(i64, [usize; 3]), ()> = HashMap::new();
    let mut out = Geometry::new();
    for (i, piece) in pieces.iter_mut().enumerate() {
        let size = piece.geo.bounds().size();
        fit_margin = fit_margin.min((plan.usable - size).min_element());
        if size.length_squared() > largest.length_squared() {
            largest = size;
        }
        cells.insert((piece.origin_part, piece.cell), ());
        let n = piece.geo.prims.len();
        mesh::set_int_prim_attr(&mut piece.geo, PART, vec![i as i64; n]);
        mesh::set_int_prim_attr(&mut piece.geo, "cell", vec![lin(piece.cell); n]);
        out.merge(&piece.geo);
    }
    let mut interfaces = 0;
    for &(op, c) in cells.keys() {
        for axis in 0..3 {
            let mut c2 = c;
            c2[axis] += 1;
            if cells.contains_key(&(op, c2)) {
                interfaces += 1;
            }
        }
    }
    Ok(GridResult {
        geo: out,
        plan_json: serde_json::Value::Array(plane_list),
        plane_count: cap_id as usize,
        seam_area,
        interfaces,
        fit_margin: if fit_margin.is_finite() { fit_margin } else { 0.0 },
        largest,
        part_count: pieces.len(),
    })
}

pub(crate) fn grid_params(info: NodeTypeInfo) -> NodeTypeInfo {
    info.param(ParamSpec::choice("printer", "Printer preset", "k1_max", &PRINTER_IDS).help("Build volume preset; custom uses the bed size below."))
        .param(ParamSpec::vec3("bed", "Custom bed size", DVec3::new(256.0, 256.0, 256.0)).unit("mm").help("Used when the preset is custom."))
        .param(ParamSpec::float("margin", "Bed margin", 2.0, 0.0, 20.0).unit("mm").help("Clearance kept free on every side of the bed."))
}

pub(crate) fn read_bed(p: &Params) -> CookResult<DVec3> {
    Ok(printer_bed(p.str("printer")?, p.vec3("bed")?))
}

fn grid_report(title: &str, r: &GridResult) -> Report {
    let mut rep = Report::new(title);
    rep.metric("part_count", Metric::new(r.part_count as f64, "count"));
    rep.metric("plane_count", Metric::new(r.plane_count as f64, "count"));
    rep.metric("interface_count", Metric::new(r.interfaces as f64, "count"));
    rep.metric("seam_area_mm2", Metric::new(r.seam_area, "mm2"));
    rep.metric("fit_margin_mm", Metric::new(r.fit_margin, "mm").with_limit(0.0, false));
    rep.note(
        Severity::Info,
        format!("largest part {:.1} x {:.1} x {:.1} mm", r.largest.x, r.largest.y, r.largest.z),
    );
    rep
}

pub struct GridPartition;

impl NodeOp for GridPartition {
    fn info(&self) -> NodeTypeInfo {
        grid_params(
            info("print.grid_partition", "Grid Split", "Cut a body into a grid of capped parts that each fit the printer bed.")
                .emulates("multi-part grid split of body panels to fit a bed")
                .implemented()
                .input(PortSpec::new("mesh", DataKind::Geometry))
                .param(ParamSpec::choice("mode", "Mode", "by_bed", &["by_bed", "by_count", "by_spacing"]).help("How the cut positions are chosen."))
                .param(ParamSpec::int("count_x", "Parts in X", 2, 1, 50).help("Used by the count mode."))
                .param(ParamSpec::int("count_y", "Parts in Y", 2, 1, 50).help("Used by the count mode."))
                .param(ParamSpec::int("count_z", "Parts in Z", 1, 1, 50).help("Used by the count mode."))
                .param(ParamSpec::vec3("spacing", "Spacing", DVec3::ZERO).unit("mm").help("Distance between cuts per axis; 0 turns the axis off."))
                .param(ParamSpec::vec3("origin_offset", "Origin offset", DVec3::ZERO).unit("mm").help("Shift of the first cut from the body minimum."))
                .param(ParamSpec::bool("snap", "Snap to small sections", false).help("Move each cut towards the thinnest nearby cross-section."))
                .param(ParamSpec::float("slack", "Snap slack", 15.0, 0.0, 50.0).unit("mm").help("Largest move of a cut when snapping."))
                .output(PortSpec::new("parts", DataKind::Geometry))
                .output(PortSpec::new("plane_list", DataKind::Json))
                .output(PortSpec::new("report", DataKind::Report)),
        )
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let bed = read_bed(p)?;
        let margin = p.float("margin")?;
        if (bed - DVec3::splat(2.0 * margin)).min_element() <= 0.0 {
            return Err(ctx.fail("the margin leaves no usable bed volume"));
        }
        let mode = match p.str("mode")? {
            "by_count" => GridMode::ByCount,
            "by_spacing" => GridMode::BySpacing,
            _ => GridMode::ByBed,
        };
        let counts = [p.int("count_x")?, p.int("count_y")?, p.int("count_z")?];
        let plan = grid_plan(&geo.bounds(), mode, bed, margin, counts, p.vec3("spacing")?, p.vec3("origin_offset")?);
        let r = grid_split(ctx, &geo, &plan, p.bool("snap")?, p.float("slack")?)?;
        let rep = grid_report("Grid split", &r);
        Ok(vec![Data::geometry(r.geo), Data::json(r.plan_json), Data::report(rep)])
    }
}

pub struct SmartPartition;

impl NodeOp for SmartPartition {
    fn info(&self) -> NodeTypeInfo {
        grid_params(
            info("print.smart_partition", "Smart Partition", "Search for a split with few parts, small seams and hidden seam lines.")
                .emulates("optimised partitioning of large models for printing")
                .input(PortSpec::new("mesh", DataKind::Geometry))
                .input(PortSpec::optional("guide", DataKind::Geometry))
                .param(ParamSpec::int("beam_width", "Beam width", 8, 1, 64).help("Number of candidate splits kept at each step."))
                .param(ParamSpec::float("w_parts", "Weight: part count", 1.0, 0.0, 100.0).help("Cost per part."))
                .param(ParamSpec::float("w_seam", "Weight: seam area", 0.001, 0.0, 100.0).help("Cost per mm2 of seam."))
                .param(ParamSpec::float("w_visible", "Weight: visible seam", 0.01, 0.0, 100.0).help("Cost per mm of seam on show surfaces."))
                .param(ParamSpec::float("w_fragile", "Weight: fragility", 1.0, 0.0, 100.0).help("Cost per fragile part."))
                .param(ParamSpec::float("w_unused", "Weight: unused volume", 1e-6, 0.0, 100.0).help("Cost per mm3 of unused bed volume."))
                .output(PortSpec::new("parts", DataKind::Geometry))
                .output(PortSpec::new("report", DataKind::Report)),
        )
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let bed = read_bed(p)?;
        let margin = p.float("margin")?;
        if (bed - DVec3::splat(2.0 * margin)).min_element() <= 0.0 {
            return Err(ctx.fail("the margin leaves no usable bed volume"));
        }
        let plan = grid_plan(&geo.bounds(), GridMode::ByBed, bed, margin, [1; 3], DVec3::ZERO, DVec3::ZERO);
        let r = grid_split(ctx, &geo, &plan, false, 0.0)?;
        let mut rep = grid_report("Smart partition", &r);
        let used: f64 = (0..r.part_count as i64)
            .map(|i| mesh::signed_volume(&mesh::extract_part(&r.geo, i)).abs())
            .sum();
        let unused = (r.part_count as f64 * plan.usable.x * plan.usable.y * plan.usable.z - used).max(0.0);
        let cost_parts = p.float("w_parts")? * r.part_count as f64;
        let cost_seam = p.float("w_seam")? * r.seam_area;
        let cost_unused = p.float("w_unused")? * unused;
        rep.metric("cost_parts", Metric::new(cost_parts, "cost"));
        rep.metric("cost_seam", Metric::new(cost_seam, "cost"));
        rep.metric("cost_unused", Metric::new(cost_unused, "cost"));
        rep.metric("cost_total", Metric::new(cost_parts + cost_seam + cost_unused, "cost"));
        placeholder(&mut rep, "the even grid stands in for the search; the real algorithm runs a beam search over recursive plane cuts scored by the weighted cost");
        Ok(vec![Data::geometry(r.geo), Data::report(rep)])
    }
}

// ------------------------------------------------------------ connectivity

/// Map each point to a representative within `tol` (identity when `tol <= 0`).
pub fn weld_map(points: &[DVec3], tol: f64) -> Vec<u32> {
    if tol <= 0.0 {
        return (0..points.len() as u32).collect();
    }
    let key = |p: DVec3| ((p.x / tol).floor() as i64, (p.y / tol).floor() as i64, (p.z / tol).floor() as i64);
    // Saturating neighbour keys: a huge coordinate maps to i64::MAX.
    let near = |k: i64, d: i64| k.saturating_add(d);
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    let mut map = Vec::with_capacity(points.len());
    for (i, &p) in points.iter().enumerate() {
        let (kx, ky, kz) = key(p);
        let mut rep = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(c) = grid.get(&(near(kx, dx), near(ky, dy), near(kz, dz))) {
                        for &r in c {
                            if (points[r as usize] - p).length() <= tol {
                                rep = Some(r);
                                break 'search;
                            }
                        }
                    }
                }
            }
        }
        match rep {
            Some(r) => map.push(r),
            None => {
                grid.entry((kx, ky, kz)).or_default().push(i as u32);
                map.push(i as u32);
            }
        }
    }
    map
}

struct Dsu(Vec<usize>);

impl Dsu {
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.0[hi] = lo;
        }
    }
}

/// Connected components of the closed primitives; ids by smallest prim index.
pub fn connectivity_parts(geo: &Geometry, by_point: bool, weld_tol: f64) -> Vec<i64> {
    let rep = weld_map(&geo.points, weld_tol);
    let n = geo.prims.len();
    let mut dsu = Dsu((0..n).collect());
    let mut first_edge: HashMap<(u32, u32), usize> = HashMap::new();
    let mut first_point: HashMap<u32, usize> = HashMap::new();
    for (pi, pr) in geo.prims.iter().enumerate() {
        if !pr.closed {
            continue;
        }
        let m = pr.vertices.len();
        for i in 0..m {
            let a = rep[pr.vertices[i] as usize];
            if by_point {
                match first_point.get(&a) {
                    Some(&q) => dsu.union(q, pi),
                    None => {
                        first_point.insert(a, pi);
                    }
                }
            } else {
                let b = rep[pr.vertices[(i + 1) % m] as usize];
                if a == b {
                    continue;
                }
                match first_edge.get(&(a.min(b), a.max(b))) {
                    Some(&q) => dsu.union(q, pi),
                    None => {
                        first_edge.insert((a.min(b), a.max(b)), pi);
                    }
                }
            }
        }
    }
    let mut ids: BTreeMap<usize, i64> = BTreeMap::new();
    let mut out = vec![0i64; n];
    for (pi, pr) in geo.prims.iter().enumerate() {
        if !pr.closed {
            continue;
        }
        let r = dsu.find(pi);
        let next = ids.len() as i64;
        out[pi] = *ids.entry(r).or_insert(next);
    }
    out
}

pub struct Connectivity;

impl NodeOp for Connectivity {
    fn info(&self) -> NodeTypeInfo {
        info("print.connectivity", "Split by Connectivity", "Give every connected piece of a mesh its own part id.")
            .emulates("separate loose parts of a mesh")
            .implemented()
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .param(ParamSpec::choice("link", "Link", "edge", &["edge", "point"]).help("Faces are joined by a shared edge, or by a shared point."))
            .param(ParamSpec::float("weld_tol", "Weld tolerance", 1e-6, 0.0, 10.0).unit("mm").help("Points closer than this count as one; 0 disables."))
            .output(PortSpec::new("parts", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, _ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let ids = connectivity_parts(&geo, p.str("link")? == "point", p.float("weld_tol")?);
        let mut g: Geometry = (*geo).clone();
        let count = ids.iter().copied().max().map_or(0, |m| m + 1);
        mesh::set_int_prim_attr(&mut g, PART, ids);
        let mut r = Report::new("Split by connectivity");
        r.metric("part_count", Metric::new(count as f64, "count"));
        Ok(vec![Data::geometry(g), Data::report(r)])
    }
}

pub struct ByAttribute;

impl NodeOp for ByAttribute {
    fn info(&self) -> NodeTypeInfo {
        info("print.by_attribute", "Split by Attribute", "Number parts by the value of an integer attribute.")
            .emulates("split a mesh by a key attribute")
            .implemented()
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .param(ParamSpec::string("attrib", "Attribute", "key").help("Name of an integer attribute."))
            .param(ParamSpec::choice("class", "Class", "prim", &["prim", "point"]).help("Where the attribute lives."))
            .param(ParamSpec::choice("point_rule", "Point rule", "majority", &["majority", "all_equal", "first"]).help("How a face takes a key from its points."))
            .output(PortSpec::new("parts", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let name = p.str("attrib")?;
        let as_int = |a: &AttribArray| -> Option<Vec<i64>> {
            match a {
                AttribArray::Int(v) => Some(v.clone()),
                AttribArray::Float(v) => Some(v.iter().map(|x| x.round() as i64).collect()),
                _ => None,
            }
        };
        let mut mixed = 0usize;
        let keys: Vec<i64> = if p.str("class")? == "point" {
            let col = geo
                .point_attribs
                .get(name)
                .and_then(as_int)
                .ok_or_else(|| ctx.fail(format!("point attribute '{name}' is missing or not numeric")))?;
            let rule = p.str("point_rule")?;
            geo.prims
                .iter()
                .map(|pr| {
                    let ks: Vec<i64> = pr.vertices.iter().map(|&v| col[v as usize]).collect();
                    let all_eq = ks.windows(2).all(|w| w[0] == w[1]);
                    if !all_eq {
                        mixed += 1;
                    }
                    match rule {
                        "majority" => {
                            let mut c: BTreeMap<i64, usize> = BTreeMap::new();
                            for k in &ks {
                                *c.entry(*k).or_insert(0) += 1;
                            }
                            c.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map_or(0, |x| x.0)
                        }
                        _ => ks.first().copied().unwrap_or(0),
                    }
                })
                .collect()
        } else {
            geo.prim_attribs
                .get(name)
                .and_then(as_int)
                .ok_or_else(|| ctx.fail(format!("primitive attribute '{name}' is missing or not numeric")))?
        };
        let uniq: Vec<i64> = {
            let mut u = keys.clone();
            u.sort_unstable();
            u.dedup();
            u
        };
        let ids: Vec<i64> = keys.iter().map(|k| uniq.binary_search(k).unwrap_or(0) as i64).collect();
        let mut g: Geometry = (*geo).clone();
        mesh::set_int_prim_attr(&mut g, PART, ids);
        let mut r = Report::new("Split by attribute");
        r.metric("part_count", Metric::new(uniq.len() as f64, "count"));
        r.metric("mixed_key_prims", Metric::new(mixed as f64, "count"));
        Ok(vec![Data::geometry(g), Data::report(r)])
    }
}

pub struct PartExtract;

impl NodeOp for PartExtract {
    fn info(&self) -> NodeTypeInfo {
        info("print.part_extract", "Extract Part", "Take one part out of a multi-part mesh.")
            .emulates("select one separated part")
            .implemented()
            .input(PortSpec::new("parts", DataKind::Geometry))
            .param(ParamSpec::int("part", "Part", 0, 0, 100_000).help("Part id to extract."))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let part = p.int("part")?;
        let g = mesh::extract_part(&geo, part);
        if g.prims.is_empty() {
            return Err(ctx.fail(format!("part {part} does not exist")));
        }
        let mut r = Report::new("Extract part");
        r.metric("points", Metric::new(g.point_count() as f64, "count"));
        r.metric("area_mm2", Metric::new(g.surface_area(), "mm2"));
        r.metric("volume_mm3", Metric::new(mesh::signed_volume(&g), "mm3"));
        Ok(vec![Data::geometry(g), Data::report(r)])
    }
}

pub struct PartLabel;

impl NodeOp for PartLabel {
    fn info(&self) -> NodeTypeInfo {
        info("print.part_label", "Label Parts", "Give every part a printable label such as P01, kept in the label attribute.")
            .emulates("part labelling for assembly of split bodies")
            .implemented()
            .input(PortSpec::new("parts", DataKind::Geometry))
            .param(ParamSpec::string("prefix", "Prefix", "P").help("Text in front of the number."))
            .param(ParamSpec::int("digits", "Digits", 2, 1, 6).help("Zero padded width of the number."))
            .param(ParamSpec::int("start", "First number", 1, 0, 10_000).help("Number given to the first part."))
            .output(PortSpec::new("parts", DataKind::Geometry))
            .output(PortSpec::new("labels", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
    }

    fn cook(&self, _ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let ids = mesh::part_ids(&geo);
        let parts = mesh::unique_parts(&geo);
        let prefix = p.str("prefix")?.to_string();
        let digits = p.int("digits")?.clamp(1, 6) as usize;
        let start = p.int("start")?;
        let label_of = |part: i64| {
            let rank = parts.binary_search(&part).unwrap_or(0) as i64;
            format!("{prefix}{:0width$}", start + rank, width = digits)
        };
        let mut g: Geometry = (*geo).clone();
        g.prim_attribs.set("label", AttribArray::Str(ids.iter().map(|&i| label_of(i)).collect()));
        let list: Vec<serde_json::Value> = parts
            .iter()
            .map(|&pt| {
                let n = ids.iter().filter(|&&i| i == pt).count();
                json!({"part": pt, "label": label_of(pt), "prims": n})
            })
            .collect();
        let mut r = Report::new("Label parts");
        r.metric("part_count", Metric::new(parts.len() as f64, "count"));
        Ok(vec![Data::geometry(g), Data::json(serde_json::Value::Array(list)), Data::report(r)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testutil::cook;
    use crate::mesh::testutil::tri_box;
    use vernon_graph::ParamValue;

    fn slab() -> Geometry {
        tri_box(DVec3::ZERO, DVec3::new(600.0, 300.0, 100.0))
    }

    #[test]
    fn grid_golden_six_parts_on_small_bed() {
        let out = cook(
            &GridPartition,
            "gp",
            &[Some(Data::geometry(slab()))],
            &[("printer", ParamValue::Str("custom".into())), ("bed", ParamValue::Vec3(DVec3::splat(256.0)))],
        )
        .unwrap();
        let r = out[2].as_report().unwrap();
        assert_eq!(r.metrics["part_count"].value, 6.0);
        assert_eq!(r.metrics["plane_count"].value, 3.0);
        assert_eq!(r.metrics["interface_count"].value, 7.0);
        assert!((r.metrics["fit_margin_mm"].value - 52.0).abs() < 1e-6);
        assert!(r.passes());
        let planes = out[1].as_json().unwrap().as_array().unwrap().clone();
        let mut xs: Vec<f64> = planes
            .iter()
            .filter(|p| p["axis"] == "x")
            .map(|p| p["position_mm"].as_f64().unwrap())
            .collect();
        xs.sort_by(|a, b| a.total_cmp(b));
        assert_eq!(xs, vec![200.0, 400.0]);
        let g = out[0].as_geometry().unwrap();
        let total: f64 = (0..6).map(|i| mesh::signed_volume(&mesh::extract_part(g, i))).sum();
        assert!((total - 600.0 * 300.0 * 100.0).abs() < 1e-3);
        for i in 0..6 {
            assert_eq!(mesh::edge_stats(&mesh::extract_part(g, i)).open, 0);
        }
    }

    #[test]
    fn bad_coordinates_and_params_never_panic() {
        use crate::nodes_fit::{BuildFit, OrientScoreNode};
        let mut nan = slab();
        nan.points[0] = DVec3::new(f64::NAN, 0.0, 0.0);
        let mut huge = slab();
        huge.points[0] = DVec3::splat(1e300);
        for g in [nan, huge] {
            let ins = [Some(Data::geometry(g))];
            // Errors are fine; panics (NaN sorts, key overflow, giant grids) are not.
            let _ = cook(&BuildFit, "fit", &ins, &[]);
            let _ = cook(&OrientScoreNode, "os", &ins, &[]);
            let _ = cook(&Connectivity, "conn", &ins, &[]);
            let _ = cook(&GridPartition, "gp", &ins, &[]);
        }
        let out = cook(&PartLabel, "pl", &[Some(Data::geometry(slab()))], &[("digits", ParamValue::Int(i64::MAX))]);
        assert!(out.is_ok());
        let huge_plan = grid_plan(
            &Aabb { min: DVec3::ZERO, max: DVec3::splat(1e300) },
            GridMode::ByBed,
            DVec3::splat(300.0),
            0.0,
            [1; 3],
            DVec3::ZERO,
            DVec3::ZERO,
        );
        assert!(huge_plan.planes.iter().all(|p| p.len() <= MAX_GRID_PLANES));
    }

    #[test]
    fn grid_large_bed_gives_one_part() {
        let out = cook(&GridPartition, "gp", &[Some(Data::geometry(slab()))], &[("printer", ParamValue::Str("giga_800".into()))]).unwrap();
        assert_eq!(out[2].as_report().unwrap().metrics["part_count"].value, 1.0);
    }

    #[test]
    fn grid_count_mode_counts_and_never_cuts_at_max_edge() {
        let b = slab().bounds();
        let plan = grid_plan(&b, GridMode::ByCount, DVec3::splat(1e6), 0.0, [3, 2, 1], DVec3::ZERO, DVec3::ZERO);
        assert_eq!(plan.counts, [3, 2, 1]);
        let plan = grid_plan(&b, GridMode::BySpacing, DVec3::splat(1e6), 0.0, [1; 3], DVec3::new(300.0, 0.0, 0.0), DVec3::ZERO);
        assert_eq!(plan.planes[0], vec![300.0]); // 0 and 600 are body edges
        let counts = (plan.counts[0] * plan.counts[1] * plan.counts[2]) as i64;
        assert_eq!(counts, 2);
    }

    #[test]
    fn grid_count_dowel_interface_formula() {
        // interfaces = (kx-1)ky kz + kx(ky-1)kz + kx ky(kz-1)
        let k = (5i64, 3i64, 2i64);
        assert_eq!((k.0 - 1) * k.1 * k.2 + k.0 * (k.1 - 1) * k.2 + k.0 * k.1 * (k.2 - 1), 59);
    }

    #[test]
    fn connectivity_edge_vs_point_and_weld() {
        let mut g = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        g.merge(&tri_box(DVec3::new(30.0, 0.0, 0.0), DVec3::splat(10.0)));
        let ids = connectivity_parts(&g, false, 1e-6);
        assert_eq!(ids.iter().max(), Some(&1));
        // touching at one corner: edge mode 2 parts, point mode 1
        let mut c = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        c.merge(&tri_box(DVec3::splat(10.0), DVec3::splat(10.0)));
        assert_eq!(connectivity_parts(&c, false, 1e-6).iter().max(), Some(&1));
        assert_eq!(connectivity_parts(&c, true, 1e-6).iter().max(), Some(&0));
        // unwelded soup: weld tolerance 0 splits per triangle, welding rejoins
        let soup = {
            let mut s = Geometry::new();
            let b = tri_box(DVec3::ZERO, DVec3::splat(10.0));
            for (_, t) in b.triangles() {
                let ids: Vec<u32> = t.iter().map(|&v| s.add_point(b.points[v as usize])).collect();
                s.add_prim(vernon_core::Prim::polygon(ids));
            }
            s
        };
        assert_eq!(connectivity_parts(&soup, false, 0.0).iter().max(), Some(&11));
        assert_eq!(connectivity_parts(&soup, false, 1e-6).iter().max(), Some(&0));
    }

    #[test]
    fn by_attribute_majority_conserves_prims() {
        let mut g = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        let key: Vec<i64> = g.points.iter().map(|p| i64::from(p.x > 5.0) * 7).collect();
        g.point_attribs.set("key", AttribArray::Int(key));
        let n = g.prims.len();
        let out = cook(&ByAttribute, "ba", &[Some(Data::geometry(g))], &[("class", ParamValue::Str("point".into()))]).unwrap();
        let r = out[1].as_report().unwrap();
        assert_eq!(r.metrics["part_count"].value, 2.0);
        assert!(r.metrics["mixed_key_prims"].value > 0.0);
        assert_eq!(out[0].as_geometry().unwrap().prims.len(), n);
        let missing = cook(&ByAttribute, "ba", &[Some(Data::geometry(tri_box(DVec3::ZERO, DVec3::ONE)))], &[("attrib", ParamValue::Str("nope".into()))]);
        assert!(missing.is_err());
    }

    #[test]
    fn extract_and_label() {
        let mut g = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        g.merge(&tri_box(DVec3::new(30.0, 0.0, 0.0), DVec3::splat(10.0)));
        let ids = connectivity_parts(&g, false, 1e-6);
        mesh::set_int_prim_attr(&mut g, PART, ids);
        let out = cook(&PartExtract, "pe", &[Some(Data::geometry(g.clone()))], &[("part", ParamValue::Int(1))]).unwrap();
        let e = out[0].as_geometry().unwrap();
        assert_eq!(e.point_count(), 8);
        assert!((e.bounds().min.x - 30.0).abs() < 1e-9);
        let lab = cook(&PartLabel, "pl", &[Some(Data::geometry(g))], &[]).unwrap();
        let labels = lab[1].as_json().unwrap();
        assert_eq!(labels[0]["label"], "P01");
        assert_eq!(labels[1]["label"], "P02");
    }
}

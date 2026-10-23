//! Geometry-side nodes: frontal area, mirror-symmetry check, vehicle preparation.

use std::collections::HashMap;

use rayon::prelude::*;
use vernon_core::{AttribArray, DMat4, DVec3, Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::types::RefGeom;
use crate::util::FnNode;

pub(crate) fn axis_index(name: &str) -> usize {
    match name {
        "x" => 0,
        "y" => 1,
        _ => 2,
    }
}

fn comp(p: DVec3, i: usize) -> f64 {
    match i {
        0 => p.x,
        1 => p.y,
        _ => p.z,
    }
}

fn set_comp(p: &mut DVec3, i: usize, v: f64) {
    match i {
        0 => p.x = v,
        1 => p.y = v,
        _ => p.z = v,
    }
}

/// Row-wise union of triangle projections along `axis`.
pub(crate) struct Silhouette {
    pub axes: (usize, usize),
    pub v0: f64,
    pub dv: f64,
    pub rows: Vec<Vec<(f64, f64)>>,
}

impl Silhouette {
    pub fn area(&self) -> f64 {
        self.rows
            .iter()
            .map(|r| r.iter().map(|(a, b)| b - a).sum::<f64>())
            .sum::<f64>()
            * self.dv
    }
}

/// Scanline union: exact along the row direction, midpoint sampling across
/// rows. Returns `None` for geometry without triangles or extent.
pub(crate) fn silhouette(g: &Geometry, axis: usize, n_rows: usize) -> Option<Silhouette> {
    let (iu, iv) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let b = g.bounds();
    if b.is_empty() || n_rows == 0 {
        return None;
    }
    let v0 = comp(b.min, iv);
    let ext = comp(b.max, iv) - v0;
    if ext <= 0.0 {
        return None;
    }
    let dv = ext / n_rows as f64;
    let mut rows: Vec<Vec<(f64, f64)>> = vec![Vec::new(); n_rows];
    let tris = g.triangles();
    if tris.is_empty() {
        return None;
    }
    for (_, t) in tris {
        let p: Vec<(f64, f64)> = t
            .iter()
            .map(|&i| {
                let q = g.points[i as usize];
                (comp(q, iu), comp(q, iv))
            })
            .collect();
        let vmin = p.iter().map(|q| q.1).fold(f64::INFINITY, f64::min);
        let vmax = p.iter().map(|q| q.1).fold(f64::NEG_INFINITY, f64::max);
        let k0 = (((vmin - v0) / dv - 0.5).ceil().max(0.0)) as usize;
        let k1f = ((vmax - v0) / dv - 0.5).floor();
        if k1f < 0.0 {
            continue;
        }
        let k1 = (k1f as usize).min(n_rows - 1);
        for (k, row) in rows.iter_mut().enumerate().take(k1 + 1).skip(k0) {
            let vk = v0 + (k as f64 + 0.5) * dv;
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for e in 0..3 {
                let (a, c) = (p[e], p[(e + 1) % 3]);
                if (a.1 <= vk && c.1 >= vk) || (c.1 <= vk && a.1 >= vk) {
                    if (c.1 - a.1).abs() < 1e-12 {
                        lo = lo.min(a.0).min(c.0);
                        hi = hi.max(a.0).max(c.0);
                    } else {
                        let s = (vk - a.1) / (c.1 - a.1);
                        let u = a.0 + s * (c.0 - a.0);
                        lo = lo.min(u);
                        hi = hi.max(u);
                    }
                }
            }
            if hi >= lo {
                row.push((lo, hi));
            }
        }
    }
    for row in &mut rows {
        row.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f64, f64)> = Vec::with_capacity(row.len().min(8));
        for &(lo, hi) in row.iter() {
            match merged.last_mut() {
                Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
                _ => merged.push((lo, hi)),
            }
        }
        *row = merged;
    }
    Some(Silhouette {
        axes: (iu, iv),
        v0,
        dv,
        rows,
    })
}

/// Projected area (mm2) along `axis` with the given row height (mm).
pub(crate) fn projected_area_mm2(g: &Geometry, axis: usize, pixel_mm: f64) -> f64 {
    let b = g.bounds();
    if b.is_empty() {
        return 0.0;
    }
    let iv = if axis == 2 { 1 } else { 2 };
    let ext = comp(b.max, iv) - comp(b.min, iv);
    let n = ((ext / pixel_mm.max(1e-3)).ceil() as usize).clamp(1, 20_000);
    silhouette(g, axis, n).map_or(0.0, |s| s.area())
}

// ---------------------------------------------------------------- frontal area

fn frontal_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.frontal_area", "Frontal Area", Stage::Cfd)
        .input(PortSpec::new("surfaces", DataKind::Geometry))
        .output(PortSpec::new("report", DataKind::Report))
        .output(PortSpec::new("silhouette", DataKind::Geometry))
        .param(ParamSpec::choice("dir", "Projection direction", "x", &["x", "y", "z"]).help("Axis the area is projected along; x is the flow direction"))
        .param(ParamSpec::float("pixel_mm", "Row height", 2.0, 0.5, 20.0).unit("mm").help("Scan-line spacing across the silhouette"))
        .help("Projected area of the vehicle surfaces on the plane normal to the flow, by exact scan-line union of the triangle projections. Reference area for the force coefficients.")
        .emulates("automotive wind-tunnel service: frontal reference area")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

fn frontal_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(ins, 0)?;
    let axis = axis_index(p.str("dir")?);
    let pixel = p.float("pixel_mm")?;
    let area_mm2 = projected_area_mm2(&g, axis, pixel);
    if area_mm2 <= 0.0 {
        return Err(ctx.fail("input has no triangles to project"));
    }
    let b = g.bounds();
    let (iu, iv) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let bbox_area = comp(b.size(), iu) * comp(b.size(), iv);
    let mut r = Report::new("Frontal area");
    r.metric("a_front_m2", Metric::new(area_mm2 * 1e-6, "m2"))
        .metric("bbox_fill_ratio", Metric::new(area_mm2 / bbox_area.max(1e-12), "-"));
    let mut sil = Geometry::new();
    if let Some(s) = silhouette(&g, axis, 120) {
        let plane = comp(b.min, axis);
        let mk = |u: f64, v: f64| {
            let mut q = DVec3::ZERO;
            set_comp(&mut q, axis, plane);
            set_comp(&mut q, s.axes.0, u);
            set_comp(&mut q, s.axes.1, v);
            q
        };
        for (k, row) in s.rows.iter().enumerate() {
            let (va, vb) = (s.v0 + k as f64 * s.dv, s.v0 + (k as f64 + 1.0) * s.dv);
            for &(lo, hi) in row {
                let base = sil.points.len() as u32;
                for q in [mk(lo, va), mk(hi, va), mk(hi, vb), mk(lo, vb)] {
                    sil.points.push(q);
                }
                sil.prims.push(vernon_core::Prim::polygon(vec![base, base + 1, base + 2, base + 3]));
            }
        }
    }
    Ok(vec![Data::report(r), Data::geometry(sil)])
}

// ----------------------------------------------------------- symmetry check

struct TriGrid {
    origin: DVec3,
    cell: f64,
    map: HashMap<(i32, i32, i32), Vec<u32>>,
    tris: Vec<[DVec3; 3]>,
}

impl TriGrid {
    fn new(g: &Geometry, cell: f64) -> Self {
        let b = g.bounds();
        let cell = cell.max(b.size().length() / 200.0).max(1e-6);
        let mut me = Self {
            origin: b.min,
            cell,
            map: HashMap::new(),
            tris: Vec::new(),
        };
        for (_, t) in g.triangles() {
            let tri = [g.points[t[0] as usize], g.points[t[1] as usize], g.points[t[2] as usize]];
            let lo = tri[0].min(tri[1]).min(tri[2]);
            let hi = tri[0].max(tri[1]).max(tri[2]);
            let (a, c) = (me.key(lo), me.key(hi));
            let id = me.tris.len() as u32;
            for i in a.0..=c.0 {
                for j in a.1..=c.1 {
                    for k in a.2..=c.2 {
                        me.map.entry((i, j, k)).or_default().push(id);
                    }
                }
            }
            me.tris.push(tri);
        }
        me
    }

    fn key(&self, p: DVec3) -> (i32, i32, i32) {
        let q = (p - self.origin) / self.cell;
        (q.x.floor() as i32, q.y.floor() as i32, q.z.floor() as i32)
    }

    /// Distance to the nearest triangle among the neighbouring cells, capped at `cap`
    /// (valid when `cap <= cell`).
    fn nearest(&self, p: DVec3, cap: f64) -> f64 {
        let k = self.key(p);
        let mut best = cap;
        for i in -1..=1 {
            for j in -1..=1 {
                for l in -1..=1 {
                    if let Some(ids) = self.map.get(&(k.0 + i, k.1 + j, k.2 + l)) {
                        for &id in ids {
                            let t = &self.tris[id as usize];
                            let d = (closest_on_triangle(p, t[0], t[1], t[2]) - p).length();
                            if d < best {
                                best = d;
                            }
                        }
                    }
                }
            }
        }
        best
    }
}

/// Closest point on triangle `abc` to `p` (Ericson, Real-Time Collision Detection).
pub(crate) fn closest_on_triangle(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> DVec3 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return b + (c - b) * w;
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

fn symmetry_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.symmetry_check", "Mirror Symmetry Check", Stage::Cfd)
        .input(PortSpec::new("surfaces", DataKind::Geometry))
        .output(PortSpec::new("report", DataKind::Report))
        .output(PortSpec::new("deviation", DataKind::Geometry))
        .param(ParamSpec::choice("plane", "Mirror plane normal", "y", &["y", "x", "z"]).help("Axis normal to the symmetry plane; y for a half-car at zero yaw"))
        .param(ParamSpec::float("offset_mm", "Plane position", 0.0, -10_000.0, 10_000.0).unit("mm"))
        .param(ParamSpec::float("tol_mm", "Tolerance", 1.0, 0.01, 50.0).unit("mm"))
        .param(ParamSpec::float("search_mm", "Search radius", 50.0, 5.0, 500.0).unit("mm").help("Deviations beyond this are reported as this value"))
        .help("Mirrors the vertices about the symmetry plane and measures the distance to the surface. Confirms the car is symmetric before a half-model run uses a symmetry plane.")
        .emulates("CFD pre-processing: half-model symmetry validation")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

fn symmetry_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(ins, 0)?;
    if g.triangles().is_empty() {
        return Err(ctx.fail("needs a triangulated surface"));
    }
    let axis = axis_index(p.str("plane")?);
    let off = p.float("offset_mm")?;
    let tol = p.float("tol_mm")?;
    let cap = p.float("search_mm")?;
    let grid = TriGrid::new(&g, cap);
    let dev: Vec<f64> = g
        .points
        .par_iter()
        .map(|&q| {
            let mut m = q;
            set_comp(&mut m, axis, 2.0 * off - comp(q, axis));
            grid.nearest(m, cap)
        })
        .collect();
    let max = dev.iter().copied().fold(0.0, f64::max);
    let mean = dev.iter().sum::<f64>() / dev.len().max(1) as f64;
    let over = dev.iter().filter(|&&d| d > tol).count();
    let mut r = Report::new("Mirror symmetry");
    r.metric("max_dev_mm", Metric::new(max, "mm").with_limit(tol, true))
        .metric("mean_dev_mm", Metric::new(mean, "mm"))
        .metric("points_over_tol", Metric::new(over as f64, "count"));
    if max >= cap {
        r.note(Severity::Info, "deviation reached the search radius; the true value is larger");
    }
    if max > tol {
        r.note(Severity::Error, format!("not symmetric within {tol} mm: a half-model run would be wrong"));
    }
    let mut out = (*g).clone();
    out.point_attribs.set("dist", AttribArray::Float(dev));
    Ok(vec![Data::report(r), Data::geometry(out)])
}

// ------------------------------------------------------------ vehicle prep

fn components(g: &Geometry) -> Vec<Vec<usize>> {
    let mut parent: Vec<u32> = (0..g.points.len() as u32).collect();
    fn find(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            parent[i as usize] = parent[parent[i as usize] as usize];
            i = parent[i as usize];
        }
        i
    }
    for prim in &g.prims {
        if let Some(&first) = prim.vertices.first() {
            for &v in &prim.vertices[1..] {
                let (a, b) = (find(&mut parent, first), find(&mut parent, v));
                if a != b {
                    parent[b as usize] = a;
                }
            }
        }
    }
    let mut groups: HashMap<u32, Vec<usize>> = HashMap::new();
    for (pi, prim) in g.prims.iter().enumerate() {
        if let Some(&first) = prim.vertices.first() {
            let root = find(&mut parent, first);
            groups.entry(root).or_default().push(pi);
        }
    }
    let mut out: Vec<Vec<usize>> = groups.into_values().collect();
    out.sort_by_key(|v| v[0]);
    out
}

fn bbox_of(g: &Geometry, prims: &[usize]) -> (DVec3, DVec3) {
    let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
    for &pi in prims {
        for &v in &g.prims[pi].vertices {
            let q = g.points[v as usize];
            lo = lo.min(q);
            hi = hi.max(q);
        }
    }
    (lo, hi)
}

pub(crate) fn prep_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.vehicle_prep", "Prepare Vehicle", Stage::Cfd)
        .input(PortSpec::new("body", DataKind::Geometry))
        .input(PortSpec::optional("wheels", DataKind::Geometry))
        .input(PortSpec::optional("kit", DataKind::Geometry))
        .output(PortSpec::new("vehicle", DataKind::Geometry))
        .output(PortSpec::new("reference", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::choice("nose_direction", "Nose points toward", "+x", &["+x", "-x", "+y", "-y"]).help("Direction the nose points in the input; the vehicle is rotated so the nose faces the oncoming flow (lowest x)"))
        .param(ParamSpec::float("sink_mm", "Wheel sink below ground", 2.0, 0.0, 5.0).unit("mm").help("The ground trims a flat contact patch instead of a point contact"))
        .param(ParamSpec::bool("align_origin", "Align nose to x=0 and centre y", true))
        .param(ParamSpec::bool("half_model", "Half model (y >= 0)", true))
        .param(ParamSpec::float("pixel_mm", "Area row height", 2.0, 0.5, 20.0).unit("mm"))
        .help("Puts body, wheels and kit into the flow frame (nose upstream, ground at z = 0), tags every surface with a patch name, sinks the wheels and measures the reference geometry: frontal area, length, axle positions, wheel radius.")
        .emulates("CFD pre-processing: vehicle surface set preparation")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

fn flow_frame_matrix(dir: &str, pivot: DVec3) -> DMat4 {
    let rot = match dir {
        "+x" => DMat4::from_rotation_z(std::f64::consts::PI),
        "+y" => DMat4::from_rotation_z(std::f64::consts::FRAC_PI_2),
        "-y" => DMat4::from_rotation_z(-std::f64::consts::FRAC_PI_2),
        _ => DMat4::IDENTITY,
    };
    DMat4::from_translation(pivot) * rot * DMat4::from_translation(-pivot)
}

pub(crate) fn prep_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let body = inputs::geometry(ins, 0)?;
    if body.triangles().is_empty() {
        return Err(ctx.fail("body needs a triangulated surface"));
    }
    let wheels_in = inputs::opt_geometry(ins, 1);
    let kit_in = inputs::opt_geometry(ins, 2);
    let mut report = Report::new("Vehicle preparation");

    let mut parts: Vec<(Geometry, bool, bool)> = vec![(strip(&body), false, false)];
    if let Some(w) = &wheels_in {
        parts.push((strip(w), true, false));
    }
    if let Some(k) = &kit_in {
        parts.push((strip(k), false, true));
    }
    let mut all = Geometry::new();
    for (g, _, _) in &parts {
        all.merge(g);
    }
    let dir = p.str("nose_direction")?;
    let pivot = all.bounds().center();
    let m = flow_frame_matrix(dir, DVec3::new(pivot.x, pivot.y, 0.0));
    if dir != "-x" {
        report.note(Severity::Info, format!("rotated into the flow frame (nose was toward {dir}, now toward -x)"));
    }
    for (g, _, _) in &mut parts {
        g.transform(m);
    }
    // align: nose to x = 0, centre y, lowest wheel point at -sink
    let mut bb = vernon_core::Aabb::empty();
    for (g, _, _) in &parts {
        let b = g.bounds();
        if !b.is_empty() {
            bb.grow(b.min);
            bb.grow(b.max);
        }
    }
    let sink = p.float("sink_mm")?;
    let wheel_min_z = parts.iter().filter(|(_, w, _)| *w).map(|(g, _, _)| g.bounds().min.z).fold(f64::INFINITY, f64::min);
    let (dx, dy) = if p.bool("align_origin")? {
        (-bb.min.x, -0.5 * (bb.min.y + bb.max.y))
    } else {
        (0.0, 0.0)
    };
    let dz = if wheel_min_z.is_finite() {
        -sink - wheel_min_z
    } else {
        report.note(Severity::Warning, "no wheel surfaces supplied: axle positions and wheel radius are proportional estimates, and the ground-contact sink was not applied");
        -bb.min.z
    };
    let shift = DMat4::from_translation(DVec3::new(dx, dy, dz));
    for (g, _, _) in &mut parts {
        g.transform(shift);
    }

    // tag and merge
    let mut out = Geometry::new();
    let mut part_ids: Vec<i64> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut patches: Vec<String> = Vec::new();
    let mut kit_box: Option<(DVec3, DVec3)> = None;
    let mut wheel_groups: Vec<(DVec3, DVec3, usize)> = Vec::new(); // centre, extent, part slot
    let mut wheel_count = 0usize;
    for (g, is_wheel, is_kit) in &parts {
        let base_prims = out.prims.len();
        out.merge(g);
        let n = g.prims.len();
        if *is_wheel {
            let comps = components(g);
            wheel_count = comps.len();
            names.resize(base_prims + n, String::new());
            part_ids.resize(base_prims + n, 0);
            for (ci, c) in comps.iter().enumerate() {
                let (lo, hi) = bbox_of(g, c);
                wheel_groups.push(((lo + hi) * 0.5, hi - lo, ci));
                for &pi in c {
                    names[base_prims + pi] = format!("wheel_tmp{ci}");
                }
            }
        } else {
            names.resize(base_prims, String::new());
            part_ids.resize(base_prims, 0);
            let name = if *is_kit { "kit" } else { "body" };
            for _ in 0..n {
                names.push(name.to_string());
                part_ids.push(if *is_kit { 5 } else { 0 });
            }
            if *is_kit {
                let all_prims: Vec<usize> = (0..n).collect();
                kit_box = Some(bbox_of(g, &all_prims));
            }
        }
    }
    names.resize(out.prims.len(), String::new());
    part_ids.resize(out.prims.len(), 0);

    // name the wheels by position: sort by x (front = lowest x), y < 0 is "l"
    let mut order: Vec<usize> = (0..wheel_groups.len()).collect();
    order.sort_by(|&a, &b| wheel_groups[a].0.x.total_cmp(&wheel_groups[b].0.x));
    let mut wheel_names: HashMap<usize, (String, i64)> = HashMap::new();
    if wheel_groups.len() == 4 {
        for (rank, &gi) in order.iter().enumerate() {
            let front = rank < 2;
            let left = wheel_groups[gi].0.y < 0.0;
            let (n, id) = match (front, left) {
                (true, true) => ("wheel_fl", 1),
                (true, false) => ("wheel_fr", 2),
                (false, true) => ("wheel_rl", 3),
                (false, false) => ("wheel_rr", 4),
            };
            wheel_names.insert(wheel_groups[gi].2, (n.to_string(), id));
        }
    } else if !wheel_groups.is_empty() {
        report.note(Severity::Warning, format!("expected 4 separate wheels, found {}; wheels are numbered", wheel_groups.len()));
        for (rank, &gi) in order.iter().enumerate() {
            wheel_names.insert(wheel_groups[gi].2, (format!("wheel_{rank}"), 10 + rank as i64));
        }
    }
    for (pi, name) in names.iter_mut().enumerate() {
        if let Some(rest) = name.strip_prefix("wheel_tmp") {
            if let Ok(ci) = rest.parse::<usize>() {
                if let Some((n, id)) = wheel_names.get(&ci) {
                    *name = n.clone();
                    part_ids[pi] = *id;
                }
            }
        }
    }
    for n in &names {
        if !patches.contains(n) {
            patches.push(n.clone());
        }
    }
    out.prim_attribs.set("patch", AttribArray::Str(names.clone()));
    out.prim_attribs.set("part", AttribArray::Int(part_ids));
    out.validate().map_err(|e| ctx.fail(e.to_string()))?;

    // reference geometry (metres)
    let b = out.bounds();
    let area = projected_area_mm2(&out, 0, p.float("pixel_mm")?) * 1e-6;
    let mut rf = RefGeom::reference_car();
    rf.a_front_m2 = area;
    rf.length_m = (b.max.x - b.min.x) * 1e-3;
    rf.width_m = (b.max.y - b.min.y) * 1e-3;
    rf.height_m = b.max.z * 1e-3;
    rf.nose_x_m = b.min.x * 1e-3;
    rf.tail_x_m = b.max.x * 1e-3;
    rf.z_min_m = b.min.z * 1e-3;
    rf.half_model = p.bool("half_model")?;
    rf.patches = patches.clone();
    rf.kit_bbox_m = kit_box.map(|(lo, hi)| [(lo * 1e-3).to_array(), (hi * 1e-3).to_array()]);
    rf.wheel_centres_m.clear();
    if wheel_groups.len() == 4 {
        let mut fx = 0.0;
        let mut rx = 0.0;
        let mut r = 0.0;
        let mut ty = 0.0;
        for (rank, &gi) in order.iter().enumerate() {
            let c = wheel_groups[gi].0;
            if rank < 2 {
                fx += c.x * 0.5;
            } else {
                rx += c.x * 0.5;
            }
            r += wheel_groups[gi].1.z * 0.5 * 0.25;
            ty += c.y.abs() * 0.25;
            let name = wheel_names[&wheel_groups[gi].2].0.clone();
            rf.wheel_centres_m.insert(name, (c * 1e-3).to_array());
        }
        rf.x_front_axle_m = fx * 1e-3;
        rf.x_rear_axle_m = rx * 1e-3;
        rf.wheel_radius_m = r * 1e-3;
        rf.track_m = 2.0 * ty * 1e-3;
    } else {
        let l = rf.length_m;
        rf.x_front_axle_m = rf.nose_x_m + 0.2 * l;
        rf.x_rear_axle_m = rf.nose_x_m + 0.8 * l;
        rf.wheel_radius_m = 0.068 * l;
        rf.track_m = 0.85 * rf.width_m;
    }
    report
        .metric("a_front_m2", Metric::new(rf.a_front_m2, "m2"))
        .metric("length_m", Metric::new(rf.length_m, "m"))
        .metric("width_m", Metric::new(rf.width_m, "m"))
        .metric("height_m", Metric::new(rf.height_m, "m"))
        .metric("wheelbase_m", Metric::new(rf.x_rear_axle_m - rf.x_front_axle_m, "m"))
        .metric("wheel_radius_m", Metric::new(rf.wheel_radius_m, "m"))
        .metric("wheels_found", Metric::new(wheel_count as f64, "count").with_limit(4.0, false))
        .metric("sink_mm", Metric::new(if wheel_count > 0 { sink } else { 0.0 }, "mm"));
    Ok(vec![Data::geometry(out), Data::json(rf.to_json()), Data::report(report)])
}

/// Geometry reduced to points + prims (drops foreign attributes so the merge is clean).
fn strip(g: &Geometry) -> Geometry {
    Geometry {
        points: g.points.clone(),
        prims: g.prims.clone(),
        ..Default::default()
    }
}

pub(crate) fn register(reg: &mut Registry) {
    reg.add(FnNode::new(frontal_info, frontal_cook));
    reg.add(FnNode::new(symmetry_info, symmetry_cook));
    reg.add(FnNode::new(prep_info, prep_cook));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::*;
    use vernon_graph::CookError;

    fn uv_sphere(r: f64, nlat: usize, nlon: usize) -> Geometry {
        let mut pts = vec![DVec3::new(0.0, 0.0, r), DVec3::new(0.0, 0.0, -r)];
        for i in 1..nlat {
            let th = std::f64::consts::PI * i as f64 / nlat as f64;
            for j in 0..nlon {
                let ph = std::f64::consts::TAU * j as f64 / nlon as f64;
                pts.push(DVec3::new(r * th.sin() * ph.cos(), r * th.sin() * ph.sin(), r * th.cos()));
            }
        }
        let ring = |i: usize, j: usize| (2 + (i - 1) * nlon + j % nlon) as u32;
        let mut faces = Vec::new();
        for j in 0..nlon {
            faces.push(vec![0, ring(1, j), ring(1, j + 1)]);
            faces.push(vec![1, ring(nlat - 1, j + 1), ring(nlat - 1, j)]);
        }
        for i in 1..nlat - 1 {
            for j in 0..nlon {
                faces.push(vec![ring(i, j), ring(i + 1, j), ring(i + 1, j + 1), ring(i, j + 1)]);
            }
        }
        Geometry::from_polygons(pts, faces)
    }

    #[test]
    fn frontal_area_of_box_is_exact() {
        // 4.5 x 1.8 x 1.4 m box -> 2.52 m2
        let g = Geometry::make_box(DVec3::new(4500.0, 1800.0, 1400.0));
        let a = projected_area_mm2(&g, 0, 2.0) * 1e-6;
        assert!((a - 2.52).abs() < 1e-9, "{a}");
        let top = projected_area_mm2(&g, 2, 2.0) * 1e-6;
        assert!((top - 8.1).abs() < 1e-9, "{top}");
    }

    #[test]
    fn frontal_area_of_sphere() {
        let g = uv_sphere(1000.0, 64, 128);
        let a = projected_area_mm2(&g, 0, 2.0) * 1e-6;
        assert!((a / std::f64::consts::PI - 1.0).abs() < 0.005, "{a}");
    }

    #[test]
    fn frontal_area_unions_overlapping_parts() {
        let mut g = box_between(DVec3::ZERO, DVec3::new(100.0, 1000.0, 1000.0));
        g.merge(&box_between(DVec3::new(0.0, 500.0, 500.0), DVec3::new(100.0, 1500.0, 1500.0)));
        let a = projected_area_mm2(&g, 0, 2.0);
        assert!((a - (1.0e6 + 1.0e6 - 0.25e6)).abs() < 1.0, "{a}");
    }

    #[test]
    fn frontal_node_outputs() {
        let ctx = ctx("frontal");
        let p = Params::from_specs(&frontal_info().params);
        let g = Geometry::make_box(DVec3::new(4500.0, 1800.0, 1400.0));
        let out = frontal_cook(&ctx, &[Some(Data::geometry(g))], &p).unwrap();
        assert_eq!(out.len(), 2);
        let r = out[0].as_report().unwrap();
        assert!((r.metrics["a_front_m2"].value - 2.52).abs() < 1e-6);
        assert!(!out[1].as_geometry().unwrap().prims.is_empty());
        assert!(frontal_cook(&ctx, &[None], &p).is_err());
    }

    fn two_halves(shift_mm: f64) -> Geometry {
        let mut g = Geometry::make_box(DVec3::new(1000.0, 1800.0, 600.0));
        for q in &mut g.points {
            if q.y > 0.0 {
                q.y += shift_mm;
            }
        }
        g
    }

    #[test]
    fn symmetry_golden_pass_and_fail() {
        let ctx = ctx("sym");
        let p = Params::from_specs(&symmetry_info().params);
        let ok = symmetry_cook(&ctx, &[Some(Data::geometry(two_halves(0.3)))], &p).unwrap();
        let r = ok[0].as_report().unwrap();
        assert!((r.metrics["max_dev_mm"].value - 0.3).abs() < 1e-6);
        assert!(r.passes());
        let bad = symmetry_cook(&ctx, &[Some(Data::geometry(two_halves(5.0)))], &p).unwrap();
        let r = bad[0].as_report().unwrap();
        assert!((r.metrics["max_dev_mm"].value - 5.0).abs() < 1e-6);
        assert!(!r.passes());
        assert_eq!(bad[1].as_geometry().unwrap().point_attribs.floats("dist").unwrap().len(), 8);
    }

    #[test]
    fn symmetric_car_passes_and_offset_car_fails() {
        let ctx = ctx("sym2");
        let (body, _) = synthetic_car();
        let p = Params::from_specs(&symmetry_info().params);
        let out = symmetry_cook(&ctx, &[Some(Data::geometry(body.clone()))], &p).unwrap();
        assert!(out[0].as_report().unwrap().passes());
        let mut moved = body;
        moved.transform(DMat4::from_translation(DVec3::new(0.0, 20.0, 0.0)));
        let out = symmetry_cook(&ctx, &[Some(Data::geometry(moved))], &p).unwrap();
        assert!(!out[0].as_report().unwrap().passes());
    }

    #[test]
    fn closest_point_cases() {
        let (a, b, c) = (DVec3::ZERO, DVec3::X, DVec3::Y);
        assert!((closest_on_triangle(DVec3::new(0.2, 0.2, 5.0), a, b, c) - DVec3::new(0.2, 0.2, 0.0)).length() < 1e-12);
        assert!((closest_on_triangle(DVec3::new(2.0, -1.0, 0.0), a, b, c) - DVec3::X).length() < 1e-12);
        assert!((closest_on_triangle(DVec3::new(1.0, 1.0, 0.0), a, b, c) - DVec3::new(0.5, 0.5, 0.0)).length() < 1e-12);
    }

    fn run_prep(dir: &str, with_wheels: bool) -> Vec<Data> {
        let ctx = ctx("prep");
        let (body, wheels) = synthetic_car();
        let mut params = Params::from_specs(&prep_info().params);
        params.set("nose_direction", vernon_graph::ParamValue::Str(dir.into()));
        let ins = vec![Some(Data::geometry(body)), with_wheels.then(|| Data::geometry(wheels)), None];
        prep_cook(&ctx, &ins, &params).unwrap()
    }

    #[test]
    fn prep_puts_nose_upstream_and_wheels_in_ground_contact() {
        let out = run_prep("+x", true);
        let g = out[0].as_geometry().unwrap();
        let b = g.bounds();
        assert!(b.min.x.abs() < 1e-9, "nose at x = 0");
        // frame test (H1): the pointed end must now be at min x
        let spread = |at_min: bool| {
            let target = if at_min { b.min.x } else { b.max.x };
            let ys: Vec<f64> = g.points.iter().filter(|q| (q.x - target).abs() < 1.0 && q.z > 400.0).map(|q| q.y).collect();
            ys.iter().cloned().fold(f64::MIN, f64::max) - ys.iter().cloned().fold(f64::MAX, f64::min)
        };
        assert!(spread(true) < 0.5 * spread(false), "nose faces the flow");
        assert!((b.min.z + 2.0).abs() < 1e-9, "wheels sunk 2 mm");
        let patches = g.prim_attribs.strs("patch").unwrap();
        for n in ["body", "wheel_fl", "wheel_fr", "wheel_rl", "wheel_rr"] {
            assert!(patches.iter().any(|p| p == n), "{n}");
        }
        let rf = RefGeom::from_json(out[1].as_json().unwrap()).unwrap();
        assert!((rf.length_m - 4.0).abs() < 1e-6);
        assert!((rf.wheel_radius_m - 0.32).abs() < 0.01, "{}", rf.wheel_radius_m);
        assert!(rf.x_front_axle_m < rf.x_rear_axle_m);
        assert!((rf.x_rear_axle_m - rf.x_front_axle_m - 2.2).abs() < 1e-6);
        // right wheels are at +y in the flow frame
        assert!(rf.wheel_centres_m["wheel_fr"][1] > 0.0 && rf.wheel_centres_m["wheel_fl"][1] < 0.0);
        assert!(out[2].as_report().unwrap().passes());
        assert!(rf.a_front_m2 > 1.0);
    }

    #[test]
    fn prep_handles_other_nose_directions_and_missing_wheels() {
        let out = run_prep("-x", true);
        // already in the flow frame: the car stays pointed toward -x? (our synthetic nose is +x, so rotate "+x" only)
        assert!(out[0].as_geometry().unwrap().bounds().min.x.abs() < 1e-9);
        let out = run_prep("+x", false);
        let r = out[2].as_report().unwrap();
        assert!(!r.passes(), "missing wheels must fail the gate");
        assert!(r.notes.iter().any(|(s, _)| *s == Severity::Warning));
        assert!(out[0].as_geometry().unwrap().bounds().min.z.abs() < 1e-9);
    }

    #[test]
    fn prep_rejects_empty_input() {
        let ctx = ctx("prep-empty");
        let p = Params::from_specs(&prep_info().params);
        assert!(matches!(prep_cook(&ctx, &[None], &p), Err(CookError::MissingInput { .. })));
        let pts = Geometry::from_points(vec![DVec3::ZERO]);
        assert!(prep_cook(&ctx, &[Some(Data::geometry(pts))], &p).is_err());
    }
}

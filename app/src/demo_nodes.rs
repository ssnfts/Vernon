//! Local demo nodes (ids `demo.*`), one or more per pipeline stage, so the
//! shell is usable before the stage crates are wired in. Each analysis node
//! emits a `Report` whose metrics carry pass/fail limits for the gate
//! dashboard. Algorithms are deliberately simple and documented in `help`.

use std::collections::HashMap;
use std::path::Path;

use glam::{DMat4, DQuat, DVec3, EulerRot};
use vernon_core::{AttribArray, Geometry, Metric, Prim, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{
    CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage,
};

const SPEC: &str = "docs/specs/ui-dcc-conventions.md";

type CookFn = fn(&CookContext, &[Option<Data>], &Params) -> CookResult<Vec<Data>>;

/// A node type made of two plain functions.
struct Demo {
    info: fn() -> NodeTypeInfo,
    cook: CookFn,
}

impl NodeOp for Demo {
    fn info(&self) -> NodeTypeInfo {
        (self.info)()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        (self.cook)(ctx, inputs, params)
    }
}

pub fn register(reg: &mut Registry) {
    let all: [(fn() -> NodeTypeInfo, CookFn); 16] = [
        (box_info, box_cook),
        (sphere_info, sphere_cook),
        (plane_info, plane_cook),
        (transform_info, transform_cook),
        (merge_info, merge_cook),
        (import_info, import_cook),
        (scan_info, scan_cook),
        (smooth_info, smooth_cook),
        (deviation_info, deviation_cook),
        (height_surface_info, height_surface_cook),
        (cut_info, cut_cook),
        (drag_info, drag_cook),
        (load_info, load_cook),
        (solar_info, solar_cook),
        (flatten_info, flatten_cook),
        (top_speed_info, top_speed_cook),
    ];
    for (info, cook) in all {
        reg.add(Demo { info, cook });
    }
}

fn geo_port() -> PortSpec {
    PortSpec::new("geo", DataKind::Geometry)
}

fn report_port() -> PortSpec {
    PortSpec::new("report", DataKind::Report)
}

fn mm(p: ParamSpec) -> ParamSpec {
    p.unit("mm")
}

// ---------------------------------------------------------------- geometry

fn box_info() -> NodeTypeInfo {
    NodeTypeInfo::new("demo.box", "Box", Stage::Geometry)
        .output(geo_port())
        .param(mm(ParamSpec::vec3("size", "Size", DVec3::splat(100.0))).help("Edge lengths along X, Y and Z"))
        .param(mm(ParamSpec::vec3("center", "Centre", DVec3::new(0.0, 0.0, 50.0))).help("Box centre"))
        .help("Axis-aligned box made of six quads.")
        .spec(SPEC)
        .implemented()
}

fn box_cook(ctx: &CookContext, _: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let size = p.vec3("size")?;
    if size.min_element() <= 0.0 {
        return Err(ctx.fail("every edge length must be positive"));
    }
    let mut g = Geometry::make_box(size);
    g.transform(DMat4::from_translation(p.vec3("center")?));
    Ok(vec![Data::geometry(g)])
}

fn sphere_info() -> NodeTypeInfo {
    NodeTypeInfo::new("demo.sphere", "Sphere", Stage::Geometry)
        .output(geo_port())
        .param(mm(ParamSpec::float("radius", "Radius", 50.0, 0.001, 1e6)))
        .param(ParamSpec::int("segments", "Segments", 32, 3, 1024).help("Divisions around Z"))
        .param(ParamSpec::int("rings", "Rings", 16, 2, 512).help("Divisions from pole to pole"))
        .param(mm(ParamSpec::vec3("center", "Centre", DVec3::new(0.0, 0.0, 50.0))))
        .help("UV sphere: segments x (rings - 1) + 2 points, 2 x segments x (rings - 1) triangles.")
        .spec(SPEC)
        .implemented()
}

pub fn uv_sphere(r: f64, seg: usize, rings: usize) -> Geometry {
    let mut pts = vec![DVec3::new(0.0, 0.0, r)];
    for i in 1..rings {
        let th = std::f64::consts::PI * i as f64 / rings as f64;
        for j in 0..seg {
            let ph = std::f64::consts::TAU * j as f64 / seg as f64;
            pts.push(DVec3::new(r * th.sin() * ph.cos(), r * th.sin() * ph.sin(), r * th.cos()));
        }
    }
    let south = pts.len() as u32;
    pts.push(DVec3::new(0.0, 0.0, -r));
    let at = |i: usize, j: usize| (1 + (i - 1) * seg + j % seg) as u32;
    let mut faces = Vec::new();
    for j in 0..seg {
        faces.push(vec![0, at(1, j), at(1, j + 1)]);
    }
    for i in 1..rings - 1 {
        for j in 0..seg {
            faces.push(vec![at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
            faces.push(vec![at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
        }
    }
    for j in 0..seg {
        faces.push(vec![at(rings - 1, j), south, at(rings - 1, j + 1)]);
    }
    Geometry::from_polygons(pts, faces)
}

fn sphere_cook(_: &CookContext, _: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let seg = p.int("segments")?.clamp(3, 1024) as usize;
    let rings = p.int("rings")?.clamp(2, 512) as usize;
    let mut g = uv_sphere(p.float("radius")?.max(1e-6), seg, rings);
    g.transform(DMat4::from_translation(p.vec3("center")?));
    Ok(vec![Data::geometry(g)])
}

fn plane_info() -> NodeTypeInfo {
    NodeTypeInfo::new("demo.plane", "Plane", Stage::Geometry)
        .output(geo_port())
        .param(mm(ParamSpec::float("size_x", "Length", 200.0, 0.001, 1e6)))
        .param(mm(ParamSpec::float("size_y", "Width", 200.0, 0.001, 1e6)))
        .param(ParamSpec::int("divisions", "Divisions", 10, 1, 1000))
        .help("Flat quad grid on the XY plane, centred at the origin.")
        .spec(SPEC)
        .implemented()
}

fn plane_cook(_: &CookContext, _: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let (sx, sy) = (p.float("size_x")?, p.float("size_y")?);
    let n = p.int("divisions")?.clamp(1, 1000) as usize;
    let mut pts = Vec::with_capacity((n + 1) * (n + 1));
    for j in 0..=n {
        for i in 0..=n {
            pts.push(DVec3::new(sx * (i as f64 / n as f64 - 0.5), sy * (j as f64 / n as f64 - 0.5), 0.0));
        }
    }
    let id = |i: usize, j: usize| (j * (n + 1) + i) as u32;
    let mut faces = Vec::with_capacity(n * n);
    for j in 0..n {
        for i in 0..n {
            faces.push(vec![id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1)]);
        }
    }
    Ok(vec![Data::geometry(Geometry::from_polygons(pts, faces))])
}

fn transform_info() -> NodeTypeInfo {
    NodeTypeInfo::new("demo.transform", "Transform", Stage::Geometry)
        .input(geo_port())
        .output(geo_port())
        .param(mm(ParamSpec::vec3("translate", "Move", DVec3::ZERO)))
        .param(ParamSpec::vec3("rotate", "Rotate", DVec3::ZERO).unit("deg").help("XYZ Euler angles"))
        .param(ParamSpec::float("scale", "Scale", 1.0, 0.001, 1000.0).help("Uniform scale factor"))
        .param(mm(ParamSpec::vec3("pivot", "Pivot", DVec3::ZERO)).help("Centre of rotation and scale"))
        .help("Scale and rotate about the pivot, then move. The Move, Rotate and Scale tools drive this node.")
        .spec(SPEC)
        .implemented()
}

pub fn transform_matrix(p: &Params) -> CookResult<DMat4> {
    let r = p.vec3("rotate")?;
    let q = DQuat::from_euler(EulerRot::XYZ, r.x.to_radians(), r.y.to_radians(), r.z.to_radians());
    let pivot = p.vec3("pivot")?;
    Ok(DMat4::from_translation(p.vec3("translate")? + pivot)
        * DMat4::from_quat(q)
        * DMat4::from_scale(DVec3::splat(p.float("scale")?))
        * DMat4::from_translation(-pivot))
}

fn transform_cook(_: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let mut g = (*inputs::geometry(ins, 0)?).clone();
    g.transform(transform_matrix(p)?);
    g.point_attribs.remove("N");
    g.prim_attribs.remove("N");
    Ok(vec![Data::geometry(g)])
}

fn merge_info() -> NodeTypeInfo {
    NodeTypeInfo::new("demo.merge", "Merge", Stage::Geometry)
        .input(geo_port())
        .input(PortSpec::optional("other", DataKind::Geometry))
        .output(geo_port())
        .help("Append the second input to the first.")
        .spec(SPEC)
        .implemented()
}

fn merge_cook(_: &CookContext, ins: &[Option<Data>], _: &Params) -> CookResult<Vec<Data>> {
    let mut g = (*inputs::geometry(ins, 0)?).clone();
    if let Some(b) = inputs::opt_geometry(ins, 1) {
        g.merge(&b);
    }
    Ok(vec![Data::geometry(g)])
}

// ---------------------------------------------------------------- capture

fn import_info() -> NodeTypeInfo {
    NodeTypeInfo::new("demo.import", "Import Mesh", Stage::Capture)
        .output(geo_port())
        .output(report_port())
        .param(ParamSpec::path("file", "File", vernon_io::Format::extensions(), false))
        .param(ParamSpec::choice("unit", "File unit", "mm", &["mm", "cm", "m", "in"]).help("Unit the file's numbers are in"))
        .param(ParamSpec::bool("weld", "Weld vertices", true).help("Merge coincident STL vertices"))
        .help("Read a mesh or point cloud (OBJ, STL, PLY, 3MF, XYZ, PTS) and convert to millimetres.")
        .spec(SPEC)
        .implemented()
}

fn import_cook(ctx: &CookContext, _: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let file = p.str("file")?;
    if file.trim().is_empty() {
        return Err(ctx.fail("choose a file"));
    }
    let unit = match p.str("unit")? {
        "cm" => vernon_io::FileUnit::Centimetre,
        "m" => vernon_io::FileUnit::Metre,
        "in" => vernon_io::FileUnit::Inch,
        _ => vernon_io::FileUnit::Millimetre,
    };
    let opts = vernon_io::ReadOptions {
        unit,
        weld: p.bool("weld")?,
        ..Default::default()
    };
    let g = vernon_io::read(Path::new(file), &opts).map_err(|e| ctx.fail(e.to_string()))?;
    let size = g.bounds().size();
    let mut r = Report::new("Import");
    r.metric("points", Metric::new(g.point_count() as f64, "count"))
        .metric("prims", Metric::new(g.prim_count() as f64, "count"))
        .metric("size_x", Metric::new(size.x, "mm"))
        .metric("size_y", Metric::new(size.y, "mm"))
        .metric("size_z", Metric::new(size.z, "mm"));
    Ok(vec![Data::geometry(g), Data::report(r)])
}

/// Small deterministic generator (xorshift64*), so cooks stay pure.
struct Rng(u64);

impl Rng {
    fn new(seed: i64) -> Self {
        Rng((seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn normal(&mut self) -> f64 {
        let u = self.next_f64().max(1e-300);
        let v = self.next_f64();
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
}

fn scan_info() -> NodeTypeInfo {
    NodeTypeInfo::new("demo.scan_sim", "Simulated Scan", Stage::Capture)
        .input(geo_port())
        .output(geo_port())
        .output(report_port())
        .param(ParamSpec::int("samples", "Samples", 20_000, 100, 5_000_000))
        .param(mm(ParamSpec::float("noise", "Noise (1 sigma)", 0.2, 0.0, 100.0)).help("Gaussian error along the surface normal"))
        .param(ParamSpec::int("seed", "Seed", 1, 0, 1_000_000))
        .help("Area-weighted random surface samples with normal noise: a stand-in for a real scan.")
        .spec(SPEC)
        .implemented()
}

fn scan_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(ins, 0)?;
    let tris = g.triangles();
    if tris.is_empty() {
        return Err(ctx.fail("input has no polygons to sample"));
    }
    let mut cdf = Vec::with_capacity(tris.len());
    let mut acc = 0.0;
    for (_, t) in &tris {
        let [a, b, c] = t.map(|i| g.points[i as usize]);
        acc += (b - a).cross(c - a).length() * 0.5;
        cdf.push(acc);
    }
    let n = p.int("samples")?.clamp(1, 5_000_000) as usize;
    let sigma = p.float("noise")?.max(0.0);
    let mut rng = Rng::new(p.int("seed")?);
    let mut pts = Vec::with_capacity(n);
    let mut nrm = Vec::with_capacity(n);
    for _ in 0..n {
        let x = rng.next_f64() * acc;
        let k = cdf.partition_point(|&c| c < x).min(tris.len() - 1);
        let [a, b, c] = tris[k].1.map(|i| g.points[i as usize]);
        let (mut u, mut v) = (rng.next_f64(), rng.next_f64());
        if u + v > 1.0 {
            u = 1.0 - u;
            v = 1.0 - v;
        }
        let normal = (b - a).cross(c - a).normalize_or_zero();
        pts.push(a + (b - a) * u + (c - a) * v + normal * rng.normal() * sigma);
        nrm.push(normal);
    }
    let mut out = Geometry::from_points(pts);
    out.point_attribs.set("N", AttribArray::Vec3(nrm));
    let mut r = Report::new("Simulated scan");
    r.metric("points", Metric::new(n as f64, "count"))
        .metric("surface_area", Metric::new(acc, "mm2"))
        .metric("mean_spacing", Metric::new((acc / n as f64).sqrt(), "mm"));
    Ok(vec![Data::geometry(out), Data::report(r)])
}

// ---------------------------------------------------------------- clean

fn smooth_info() -> NodeTypeInfo {
    NodeTypeInfo::new("demo.smooth", "Smooth", Stage::Clean)
        .input(geo_port())
        .output(geo_port())
        .param(ParamSpec::int("iterations", "Iterations", 5, 0, 500))
        .param(ParamSpec::float("strength", "Strength", 0.5, 0.0, 1.0))
        .help("Laplacian smoothing over polygon edges. Point clouds pass through unchanged.")
        .spec(SPEC)
        .implemented()
}

fn smooth_cook(_: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let src = inputs::geometry(ins, 0)?;
    let mut g = (*src).clone();
    let n = g.points.len();
    let mut nb: Vec<Vec<u32>> = vec![Vec::new(); n];
    for prim in &g.prims {
        let v = &prim.vertices;
        let m = v.len();
        let edges = if prim.closed { m } else { m.saturating_sub(1) };
        for i in 0..edges {
            let (a, b) = (v[i], v[(i + 1) % m]);
            if (a as usize) < n && (b as usize) < n {
                nb[a as usize].push(b);
                nb[b as usize].push(a);
            }
        }
    }
    let k = p.float("strength")?.clamp(0.0, 1.0);
    for _ in 0..p.int("iterations")?.clamp(0, 500) {
        let prev = g.points.clone();
        for (i, list) in nb.iter().enumerate() {
            if list.is_empty() {
                continue;
            }
            let avg = list.iter().map(|&j| prev[j as usize]).sum::<DVec3>() / list.len() as f64;
            g.points[i] = prev[i] + (avg - prev[i]) * k;
        }
    }
    g.point_attribs.remove("N");
    g.prim_attribs.remove("N");
    Ok(vec![Data::geometry(g)])
}

/// Uniform-grid nearest neighbour search over points.
struct PointGrid<'a> {
    pts: &'a [DVec3],
    cell: f64,
    origin: DVec3,
    map: HashMap<(i64, i64, i64), Vec<u32>>,
}

impl<'a> PointGrid<'a> {
    fn new(pts: &'a [DVec3]) -> Self {
        let mut lo = DVec3::splat(f64::MAX);
        let mut hi = DVec3::splat(f64::MIN);
        for p in pts {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        let diag = (hi - lo).length().max(1e-6);
        let cell = (diag / (pts.len() as f64).cbrt().max(1.0)).max(1e-6);
        let mut map: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
        let mut g = PointGrid {
            pts,
            cell,
            origin: lo,
            map: HashMap::new(),
        };
        for (i, p) in pts.iter().enumerate() {
            map.entry(g.key(*p)).or_default().push(i as u32);
        }
        g.map = map;
        g
    }

    fn key(&self, p: DVec3) -> (i64, i64, i64) {
        let q = ((p - self.origin) / self.cell).floor();
        (q.x as i64, q.y as i64, q.z as i64)
    }

    fn nearest(&self, p: DVec3) -> f64 {
        let (cx, cy, cz) = self.key(p);
        let mut best = f64::MAX;
        for r in 0i64..=64 {
            for x in -r..=r {
                for y in -r..=r {
                    for z in -r..=r {
                        if x.abs().max(y.abs()).max(z.abs()) != r {
                            continue;
                        }
                        if let Some(list) = self.map.get(&(cx + x, cy + y, cz + z)) {
                            for &i in list {
                                best = best.min((self.pts[i as usize] - p).length());
                            }
                        }
                    }
                }
            }
            // Every unvisited cell is at least r cells away.
            if best <= r as f64 * self.cell {
                return best;
            }
        }
        if best == f64::MAX {
            best = self.pts.iter().map(|q| (*q - p).length()).fold(f64::MAX, f64::min);
        }
        best
    }
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn deviation_info() -> NodeTypeInfo {
    NodeTypeInfo::new("demo.deviation", "Deviation Check", Stage::Clean)
        .input(PortSpec::new("test", DataKind::Geometry))
        .input(PortSpec::new("reference", DataKind::Geometry))
        .output(geo_port())
        .output(report_port())
        .param(mm(ParamSpec::float("tolerance", "P95 tolerance", 0.5, 0.0, 1e4)).help("Gate: 95th percentile deviation must stay below this"))
        .help("Point-to-nearest-point distance from the test geometry to the reference, written to point attribute dist.")
        .spec(SPEC)
        .implemented()
}

fn deviation_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let test = inputs::geometry(ins, 0)?;
    let reference = inputs::geometry(ins, 1)?;
    if reference.points.is_empty() {
        return Err(ctx.fail("reference has no points"));
    }
    let grid = PointGrid::new(&reference.points);
    let d: Vec<f64> = test.points.iter().map(|q| grid.nearest(*q)).collect();
    let mut sorted = d.clone();
    sorted.sort_by(f64::total_cmp);
    let mean = if d.is_empty() { 0.0 } else { d.iter().sum::<f64>() / d.len() as f64 };
    let mut g = (*test).clone();
    g.point_attribs.set("dist", AttribArray::Float(d));
    let mut r = Report::new("Deviation");
    r.metric("mean", Metric::new(mean, "mm"))
        .metric("p95", Metric::new(percentile(&sorted, 0.95), "mm").with_limit(p.float("tolerance")?, true))
        .metric("max", Metric::new(sorted.last().copied().unwrap_or(0.0), "mm"));
    Ok(vec![Data::geometry(g), Data::report(r)])
}

// ---------------------------------------------------------------- surface

fn height_surface_info() -> NodeTypeInfo {
    NodeTypeInfo::new("demo.height_surface", "Height-Field Surface", Stage::Surface)
        .input(geo_port())
        .output(geo_port())
        .output(report_port())
        .param(ParamSpec::int("resolution", "Resolution", 24, 2, 400).help("Cells along the longer side"))
        .param(mm(ParamSpec::float("max_rms", "RMS limit", 1.0, 0.0, 1e4)).help("Gate: RMS distance of input points to the surface"))
        .help("Fits a regular quad surface z(x, y) to points by cell averaging; empty cells borrow from neighbours.")
        .spec(SPEC)
}

fn height_surface_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(ins, 0)?;
    if g.points.len() < 3 {
        return Err(ctx.fail("need at least 3 points"));
    }
    let b = g.bounds();
    let size = b.size();
    let res = p.int("resolution")?.clamp(2, 400) as usize;
    let long = size.x.max(size.y).max(1e-6);
    let nx = ((size.x / long * res as f64).round() as usize).max(1);
    let ny = ((size.y / long * res as f64).round() as usize).max(1);
    let cell = |q: DVec3| -> (usize, usize) {
        let i = (((q.x - b.min.x) / size.x.max(1e-9)) * nx as f64).floor() as isize;
        let j = (((q.y - b.min.y) / size.y.max(1e-9)) * ny as f64).floor() as isize;
        (i.clamp(0, nx as isize - 1) as usize, j.clamp(0, ny as isize - 1) as usize)
    };
    let mut sum = vec![0.0; nx * ny];
    let mut cnt = vec![0usize; nx * ny];
    for q in &g.points {
        let (i, j) = cell(*q);
        sum[j * nx + i] += q.z;
        cnt[j * nx + i] += 1;
    }
    let mut z: Vec<Option<f64>> = (0..nx * ny).map(|k| (cnt[k] > 0).then(|| sum[k] / cnt[k] as f64)).collect();
    let filled_cells = z.iter().filter(|v| v.is_some()).count();
    for _ in 0..(nx + ny) {
        if z.iter().all(Option::is_some) {
            break;
        }
        let prev = z.clone();
        for j in 0..ny {
            for i in 0..nx {
                if prev[j * nx + i].is_some() {
                    continue;
                }
                let mut s = 0.0;
                let mut c = 0;
                for (di, dj) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                    let (a, bb) = (i as i64 + di, j as i64 + dj);
                    if a >= 0 && bb >= 0 && (a as usize) < nx && (bb as usize) < ny {
                        if let Some(v) = prev[bb as usize * nx + a as usize] {
                            s += v;
                            c += 1;
                        }
                    }
                }
                if c > 0 {
                    z[j * nx + i] = Some(s / c as f64);
                }
            }
        }
    }
    let zc: Vec<f64> = z.iter().map(|v| v.unwrap_or(b.center().z)).collect();
    // Corner heights: average of the adjacent cells.
    let corner = |i: usize, j: usize| -> f64 {
        let mut s = 0.0;
        let mut c = 0;
        for (ci, cj) in [(i as i64 - 1, j as i64 - 1), (i as i64, j as i64 - 1), (i as i64 - 1, j as i64), (i as i64, j as i64)] {
            if ci >= 0 && cj >= 0 && (ci as usize) < nx && (cj as usize) < ny {
                s += zc[cj as usize * nx + ci as usize];
                c += 1;
            }
        }
        s / c.max(1) as f64
    };
    let mut pts = Vec::with_capacity((nx + 1) * (ny + 1));
    for j in 0..=ny {
        for i in 0..=nx {
            let x = b.min.x + size.x * i as f64 / nx as f64;
            let y = b.min.y + size.y * j as f64 / ny as f64;
            pts.push(DVec3::new(x, y, corner(i, j)));
        }
    }
    let id = |i: usize, j: usize| (j * (nx + 1) + i) as u32;
    let mut faces = Vec::with_capacity(nx * ny);
    for j in 0..ny {
        for i in 0..nx {
            faces.push(vec![id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1)]);
        }
    }
    let rms = (g
        .points
        .iter()
        .map(|q| {
            let (i, j) = cell(*q);
            (q.z - zc[j * nx + i]).powi(2)
        })
        .sum::<f64>()
        / g.points.len() as f64)
        .sqrt();
    let mut r = Report::new("Surface fit");
    r.metric("rms", Metric::new(rms, "mm").with_limit(p.float("max_rms")?, true))
        .metric("coverage", Metric::new(100.0 * filled_cells as f64 / (nx * ny) as f64, "%"))
        .metric("patches", Metric::new((nx * ny) as f64, "count"));
    Ok(vec![Data::geometry(Geometry::from_polygons(pts, faces)), Data::report(r)])
}

// ---------------------------------------------------------------- print

fn cut_info() -> NodeTypeInfo {
    NodeTypeInfo::new("demo.planar_cut", "Planar Cut", Stage::Print)
        .input(geo_port())
        .output(geo_port())
        .output(report_port())
        .param(mm(ParamSpec::float("height", "Cut height", 40.0, -1e6, 1e6)).help("Z of the horizontal cutting plane"))
        .param(ParamSpec::choice("keep", "Keep", "above", &["above", "below"]))
        .param(mm(ParamSpec::float("build_height", "Printer build height", 250.0, 1.0, 1e5)).help("Gate: the kept part must fit"))
        .help("Clips polygons against a horizontal plane (no capping) and checks the part height against the printer.")
        .spec(SPEC)
        .implemented()
}

fn cut_cook(_: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(ins, 0)?;
    let h = p.float("height")?;
    let sign = if p.str("keep")? == "below" { -1.0 } else { 1.0 };
    let inside = |q: DVec3| (q.z - h) * sign >= 0.0;
    let mut out = Geometry::new();
    let mut kept: HashMap<u32, u32> = HashMap::new();
    let mut cuts: HashMap<(u32, u32), u32> = HashMap::new();
    let mut keep_point = |out: &mut Geometry, i: u32| *kept.entry(i).or_insert_with(|| out.add_point(g.points[i as usize]));
    for prim in &g.prims {
        let v = &prim.vertices;
        let m = v.len();
        let mut poly = Vec::new();
        let edges = if prim.closed { m } else { m.saturating_sub(1) };
        if m == 0 {
            continue;
        }
        if !prim.closed && inside(g.points[v[0] as usize]) {
            poly.push(keep_point(&mut out, v[0]));
        }
        for e in 0..edges {
            let (a, b) = (v[e], v[(e + 1) % m]);
            let (pa, pb) = (g.points[a as usize], g.points[b as usize]);
            let (ia, ib) = (inside(pa), inside(pb));
            if ia != ib {
                let key = (a.min(b), a.max(b));
                let idx = *cuts.entry(key).or_insert_with(|| {
                    let t = (h - pa.z) / (pb.z - pa.z);
                    out.add_point(pa + (pb - pa) * t)
                });
                poly.push(idx);
            }
            if ib {
                poly.push(keep_point(&mut out, b));
            }
        }
        if prim.closed && poly.len() >= 3 {
            out.add_prim(Prim::polygon(poly));
        } else if !prim.closed && poly.len() >= 2 {
            out.add_prim(Prim::polyline(poly));
        }
    }
    if g.prims.is_empty() {
        for (i, q) in g.points.iter().enumerate() {
            if inside(*q) {
                keep_point(&mut out, i as u32);
            }
        }
    }
    let size = out.bounds().size();
    let height = if out.points.is_empty() { 0.0 } else { size.z };
    let mut r = Report::new("Planar cut");
    r.metric("part_height", Metric::new(height, "mm").with_limit(p.float("build_height")?, true))
        .metric("kept_area", Metric::new(out.surface_area(), "mm2"))
        .metric("kept_points", Metric::new(out.point_count() as f64, "count"));
    if out.points.is_empty() {
        r.note(Severity::Warning, "nothing is left on the kept side of the plane");
    }
    Ok(vec![Data::geometry(out), Data::report(r)])
}

// ---------------------------------------------------------------- analyses

/// Sum of `max(n . dir, 0) * area` over polygons: projected frontal area.
fn projected_area(g: &Geometry, dir: DVec3) -> f64 {
    (0..g.prims.len())
        .filter(|&i| g.prims[i].closed && g.prims[i].vertices.len() >= 3)
        .map(|i| g.prim_normal(i).dot(dir).max(0.0) * g.prim_area(i))
        .sum()
}

fn drag_info() -> NodeTypeInfo {
    NodeTypeInfo::new("demo.drag_estimate", "Drag Estimate", Stage::Cfd)
        .input(geo_port())
        .output(geo_port())
        .output(report_port())
        .param(ParamSpec::float("speed", "Speed", 120.0, 0.0, 500.0).unit("km/h"))
        .param(ParamSpec::float("rho", "Air density", 1.225, 0.5, 1.5).unit("kg/m³"))
        .param(ParamSpec::float("cd", "Drag coefficient", 0.32, 0.01, 3.0).help("Assumed until an airflow solver runs"))
        .param(ParamSpec::float("max_cda", "CdA limit", 0.7, 0.0, 10.0).unit("m²").help("Gate: CdA must stay below this"))
        .help("Frontal area along +X (vehicle forward) times an assumed Cd; colours faces by a windward/leeward estimate (Cd attribute).")
        .spec(SPEC)
        .emulates("cloud wind-tunnel drag summary (black-box numbers only)")
}

fn drag_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let src = inputs::geometry(ins, 0)?;
    if src.prims.is_empty() {
        return Err(ctx.fail("needs a surface mesh, not a point cloud"));
    }
    let mut g = (*src).clone();
    let area_m2 = projected_area(&g, DVec3::X) / 1e6;
    let v = p.float("speed")? / 3.6;
    let rho = p.float("rho")?;
    let cd = p.float("cd")?;
    let q = 0.5 * rho * v * v;
    let drag = q * cd * area_m2;
    let mut cp = Vec::with_capacity(g.prims.len());
    let mut col = Vec::with_capacity(g.prims.len());
    for i in 0..g.prims.len() {
        let nx = g.prim_normal(i).x;
        let c = nx * nx.abs();
        cp.push(c);
        let rgb = crate::viewport::mesh::ramp((c + 1.0) * 0.5);
        col.push(DVec3::new(rgb[0] as f64, rgb[1] as f64, rgb[2] as f64) / 255.0);
    }
    g.prim_attribs.set("cp", AttribArray::Float(cp));
    g.prim_attribs.set("Cd", AttribArray::Vec3(col));
    let mut r = Report::new("Drag estimate");
    r.metric("frontal_area", Metric::new(area_m2, "m²"))
        .metric("cda", Metric::new(cd * area_m2, "m²").with_limit(p.float("max_cda")?, true))
        .metric("dynamic_pressure", Metric::new(q, "Pa"))
        .metric("drag", Metric::new(drag, "N"))
        .metric("drag_power", Metric::new(drag * v / 1000.0, "kW"));
    r.note(Severity::Info, "Cd is assumed, not solved: run an airflow solver for real numbers");
    Ok(vec![Data::geometry(g), Data::report(r)])
}

fn load_info() -> NodeTypeInfo {
    NodeTypeInfo::new("demo.load_check", "Load Check", Stage::Fea)
        .input(geo_port())
        .output(geo_port())
        .output(report_port())
        .param(ParamSpec::float("load", "Tip load", 200.0, 0.0, 1e6).unit("N"))
        .param(mm(ParamSpec::float("thickness", "Wall thickness", 3.0, 0.1, 100.0)))
        .param(ParamSpec::float("yield", "Material yield", 40.0, 1.0, 2000.0).unit("MPa"))
        .param(ParamSpec::float("min_sf", "Safety factor limit", 2.0, 1.0, 20.0).help("Gate: safety factor must reach this"))
        .help("Treats the part as a cantilever plate along X: stress = 6 F L / (b t²), root at minimum X. Writes stress_mpa.")
        .spec(SPEC)
}

fn load_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let src = inputs::geometry(ins, 0)?;
    let b = src.bounds();
    let size = b.size();
    if src.points.is_empty() || size.x <= 0.0 || size.y <= 0.0 {
        return Err(ctx.fail("needs a part with extent in X and Y"));
    }
    let f = p.float("load")?;
    let t = p.float("thickness")?;
    let sigma = 6.0 * f * size.x / (size.y * t * t);
    let sf = if sigma > 0.0 { p.float("yield")? / sigma } else { f64::INFINITY };
    let mut g = (*src).clone();
    let s: Vec<f64> = g.points.iter().map(|q| sigma * (1.0 - (q.x - b.min.x) / size.x)).collect();
    g.point_attribs.set("stress_mpa", AttribArray::Float(s));
    let mut r = Report::new("Load check");
    r.metric("max_stress", Metric::new(sigma, "MPa"))
        .metric("safety_factor", Metric::new(sf.min(1e9), "").with_limit(p.float("min_sf")?, false));
    Ok(vec![Data::geometry(g), Data::report(r)])
}

fn solar_info() -> NodeTypeInfo {
    NodeTypeInfo::new("demo.solar_heat", "Solar Heating", Stage::Solar)
        .input(geo_port())
        .output(geo_port())
        .output(report_port())
        .param(ParamSpec::float("elevation", "Sun elevation", 70.0, 0.0, 90.0).unit("deg"))
        .param(ParamSpec::float("azimuth", "Sun azimuth", 180.0, -360.0, 360.0).unit("deg").help("From +X towards +Y"))
        .param(ParamSpec::float("irradiance", "Irradiance", 1000.0, 0.0, 1500.0).unit("W/m²"))
        .param(ParamSpec::float("absorptivity", "Absorptivity", 0.9, 0.0, 1.0))
        .param(ParamSpec::float("ambient", "Ambient", 40.0, -40.0, 60.0).unit("°C"))
        .param(ParamSpec::float("h", "Convection coefficient", 15.0, 1.0, 200.0).unit("W/m²K"))
        .param(ParamSpec::float("hdt", "Material heat limit", 100.0, 20.0, 400.0).unit("°C").help("Gate: hottest surface must stay below this"))
        .help("Steady surface balance T = T_air + a G cos(theta) / h per point; writes temp_c.")
        .spec(SPEC)
        .implemented()
}

fn solar_cook(_: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let mut g = (*inputs::geometry(ins, 0)?).clone();
    let (e, a) = (p.float("elevation")?.to_radians(), p.float("azimuth")?.to_radians());
    let sun = DVec3::new(e.cos() * a.cos(), e.cos() * a.sin(), e.sin());
    if !g.prims.is_empty() && g.point_attribs.vec3s("N").is_err() {
        g.compute_normals();
    }
    let normals: Vec<DVec3> = match g.point_attribs.vec3s("N") {
        Ok(n) if n.len() == g.points.len() => n.to_vec(),
        _ => vec![DVec3::Z; g.points.len()],
    };
    let amb = p.float("ambient")?;
    let gain = p.float("absorptivity")? * p.float("irradiance")? / p.float("h")?.max(1e-6);
    let temps: Vec<f64> = normals.iter().map(|n| amb + gain * n.dot(sun).max(0.0)).collect();
    let tmax = temps.iter().copied().fold(amb, f64::max);
    let hdt = p.float("hdt")?;
    g.point_attribs.set("temp_c", AttribArray::Float(temps));
    let mut r = Report::new("Solar heating");
    r.metric("max_temp", Metric::new(tmax, "°C").with_limit(hdt, true))
        .metric("heat_margin", Metric::new(hdt - tmax, "K").with_limit(0.0, false));
    Ok(vec![Data::geometry(g), Data::report(r)])
}

fn flatten_info() -> NodeTypeInfo {
    NodeTypeInfo::new("demo.flatten", "Planar Flatten", Stage::Flatten)
        .input(geo_port())
        .output(geo_port())
        .output(report_port())
        .param(ParamSpec::choice("axis", "Projection axis", "auto", &["auto", "x", "y", "z"]))
        .param(ParamSpec::float("min_ratio", "Area ratio limit", 0.95, 0.0, 1.0).help("Gate: flat area / 3D area must reach this"))
        .help("Projects the surface onto the plane facing its mean normal; outputs the flat pattern with a uv attribute.")
        .spec(SPEC)
}

fn flatten_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let src = inputs::geometry(ins, 0)?;
    if src.prims.is_empty() {
        return Err(ctx.fail("needs a surface mesh"));
    }
    let axis = match p.str("axis")? {
        "x" => DVec3::X,
        "y" => DVec3::Y,
        "z" => DVec3::Z,
        _ => {
            let mean: DVec3 = (0..src.prims.len()).map(|i| src.prim_normal(i) * src.prim_area(i)).sum();
            let m = mean.abs();
            if m.x >= m.y && m.x >= m.z {
                DVec3::X
            } else if m.y >= m.z {
                DVec3::Y
            } else {
                DVec3::Z
            }
        }
    };
    let (u_axis, v_axis) = if axis == DVec3::X {
        (DVec3::Y, DVec3::Z)
    } else if axis == DVec3::Y {
        (DVec3::X, DVec3::Z)
    } else {
        (DVec3::X, DVec3::Y)
    };
    let area3: f64 = src.surface_area();
    let flat_area: f64 = (0..src.prims.len())
        .map(|i| src.prim_normal(i).dot(axis).abs() * src.prim_area(i))
        .sum();
    let mut g = (*src).clone();
    let uv: Vec<DVec3> = g.points.iter().map(|q| DVec3::new(q.dot(u_axis), q.dot(v_axis), 0.0)).collect();
    g.points = uv.clone();
    g.point_attribs.set("uv", AttribArray::Vec3(uv));
    g.point_attribs.remove("N");
    g.prim_attribs.remove("N");
    let ratio = if area3 > 0.0 { flat_area / area3 } else { 0.0 };
    let mut r = Report::new("Flatten");
    r.metric("area_ratio", Metric::new(ratio, "").with_limit(p.float("min_ratio")?, false))
        .metric("flat_area", Metric::new(flat_area, "mm2"));
    Ok(vec![Data::geometry(g), Data::report(r)])
}

fn top_speed_info() -> NodeTypeInfo {
    NodeTypeInfo::new("demo.top_speed", "Top Speed Estimate", Stage::Performance)
        .input(PortSpec::optional("aero", DataKind::Report))
        .output(report_port())
        .param(ParamSpec::float("cda", "CdA (no input)", 0.7, 0.01, 10.0).unit("m²").help("Used when no report with a cda metric is connected"))
        .param(ParamSpec::float("mass", "Mass", 1200.0, 50.0, 10_000.0).unit("kg"))
        .param(ParamSpec::float("power", "Wheel power", 110.0, 1.0, 2000.0).unit("kW"))
        .param(ParamSpec::float("crr", "Rolling resistance", 0.012, 0.0, 0.1))
        .param(ParamSpec::float("rho", "Air density", 1.225, 0.5, 1.5).unit("kg/m³"))
        .param(ParamSpec::float("target", "Target top speed", 180.0, 0.0, 600.0).unit("km/h").help("Gate: estimate must reach this"))
        .help("Solves P = (0.5 rho CdA v² + Crr m g) v for v by bisection; reads cda from an upstream drag report.")
        .spec(SPEC)
}

pub fn top_speed(power_w: f64, cda: f64, mass: f64, crr: f64, rho: f64) -> f64 {
    let need = |v: f64| (0.5 * rho * cda * v * v + crr * mass * 9.80665) * v;
    let (mut lo, mut hi) = (0.0, 400.0);
    for _ in 0..100 {
        let mid = 0.5 * (lo + hi);
        if need(mid) > power_w {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    lo
}

fn top_speed_cook(_: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let mut r = Report::new("Top speed");
    let cda = match inputs::opt_report(ins, 0).and_then(|a| a.metrics.get("cda").map(|m| m.value)) {
        Some(v) => {
            r.note(Severity::Info, "CdA taken from the connected report");
            v
        }
        None => p.float("cda")?,
    };
    let v = top_speed(p.float("power")? * 1000.0, cda, p.float("mass")?, p.float("crr")?, p.float("rho")?);
    r.metric("cda_used", Metric::new(cda, "m²"))
        .metric("top_speed", Metric::new(v * 3.6, "km/h").with_limit(p.float("target")?, false));
    Ok(vec![Data::report(r)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use vernon_graph::Graph;

    fn reg() -> Registry {
        let mut r = Registry::new();
        register(&mut r);
        r
    }

    #[test]
    fn sphere_counts_match_formula() {
        let g = uv_sphere(50.0, 32, 16);
        assert_eq!(g.point_count(), 482);
        assert_eq!(g.triangles().len(), 960);
        let g = uv_sphere(50.0, 128, 64);
        assert_eq!(g.point_count(), 8066);
        assert_eq!(g.triangles().len(), 16128);
        // Outward winding: normals point away from the centre.
        for i in 0..g.prims.len() {
            let c = g.prims[i].vertices.iter().map(|&v| g.points[v as usize]).sum::<DVec3>();
            assert!(g.prim_normal(i).dot(c) > 0.0);
        }
    }

    #[test]
    fn every_node_cooks_on_a_box() {
        let reg = reg();
        let mut graph = Graph::new();
        let b = graph.add_node(&reg, "demo.box").unwrap();
        for info in reg.types() {
            if info.id == "demo.import" || info.id == "demo.box" {
                continue;
            }
            let n = graph.add_node(&reg, &info.id).unwrap();
            if !info.inputs.is_empty() {
                if info.inputs[0].kind == DataKind::Report {
                    let d = graph.add_node(&reg, "demo.drag_estimate").unwrap();
                    graph.connect(b, 0, d, 0).unwrap();
                    graph.connect(d, 1, n, 0).unwrap();
                } else {
                    graph.connect(b, 0, n, 0).unwrap();
                }
            }
            if info.inputs.len() > 1 && info.inputs[1].required {
                graph.connect(b, 0, n, 1).unwrap();
            }
            let out = graph.cook(&reg, n).unwrap_or_else(|e| panic!("{}: {e}", info.id));
            assert_eq!(out.len(), info.outputs.len());
        }
    }

    #[test]
    fn import_without_file_fails_cleanly() {
        let reg = reg();
        let mut g = Graph::new();
        let n = g.add_node(&reg, "demo.import").unwrap();
        assert!(g.cook(&reg, n).is_err());
    }

    #[test]
    fn cut_box_report() {
        let reg = reg();
        let mut g = Graph::new();
        let b = g.add_node(&reg, "demo.box").unwrap();
        let c = g.add_node(&reg, "demo.planar_cut").unwrap();
        g.connect(b, 0, c, 0).unwrap();
        let out = g.cook(&reg, c).unwrap();
        let r = out[1].as_report().unwrap();
        assert!((r.metrics["part_height"].value - 60.0).abs() < 1e-9);
        assert!(r.passes());
        // Side area 4 x 100 x 60 + top 100 x 100.
        assert!((r.metrics["kept_area"].value - 34_000.0).abs() < 1e-6);
    }

    #[test]
    fn deviation_of_identical_geometry_is_zero() {
        let reg = reg();
        let mut g = Graph::new();
        let b = g.add_node(&reg, "demo.sphere").unwrap();
        let d = g.add_node(&reg, "demo.deviation").unwrap();
        g.connect(b, 0, d, 0).unwrap();
        g.connect(b, 0, d, 1).unwrap();
        let out = g.cook(&reg, d).unwrap();
        let r = out[1].as_report().unwrap();
        assert_eq!(r.metrics["max"].value, 0.0);
        assert!(r.passes());
        let pts = [DVec3::ZERO, DVec3::new(10.0, 0.0, 0.0)];
        assert!((PointGrid::new(&pts).nearest(DVec3::new(7.0, 0.0, 0.0)) - 3.0).abs() < 1e-12);
    }

    #[test]
    fn drag_and_top_speed_numbers() {
        let reg = reg();
        let mut g = Graph::new();
        let b = g.add_node(&reg, "demo.box").unwrap();
        let d = g.add_node(&reg, "demo.drag_estimate").unwrap();
        g.connect(b, 0, d, 0).unwrap();
        let r = g.cook(&reg, d).unwrap()[1].as_report().unwrap().clone();
        // 100 x 100 mm face = 0.01 m².
        assert!((r.metrics["frontal_area"].value - 0.01).abs() < 1e-12);
        let v = top_speed(1.0e5, 0.64, 1200.0, 0.0, 1.11);
        let p = 0.5 * 1.11 * 0.64 * v.powi(3);
        assert!((p - 1.0e5).abs() < 1.0);
    }

    #[test]
    fn solar_hot_spot_on_box_top() {
        let reg = reg();
        let mut g = Graph::new();
        let b = g.add_node(&reg, "demo.box").unwrap();
        let s = g.add_node(&reg, "demo.solar_heat").unwrap();
        g.connect(b, 0, s, 0).unwrap();
        g.set_param(s, "elevation", vernon_graph::ParamValue::Float(90.0)).unwrap();
        let out = g.cook(&reg, s).unwrap();
        let r = out[1].as_report().unwrap();
        // Box corner normals tilt by acos(1/sqrt3): T = 40 + 0.9*1000/15 * 0.57735.
        let want = 40.0 + 60.0 / 3f64.sqrt();
        assert!((r.metrics["max_temp"].value - want).abs() < 1e-6);
    }
}

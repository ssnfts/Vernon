//! Display meshes: geometry flattened to f32 triangles with optional
//! per-triangle colours, display decimation and ray picking.

use std::collections::{HashMap, HashSet};

use glam::{DVec3, Vec3};
use vernon_core::{AttribArray, Geometry};

/// Above this many triangles the display mesh is simplified by vertex
/// clustering (a closed surface stays closed; no triangles are dropped at
/// random, which would punch holes).
pub const MAX_DISPLAY_TRIS: usize = 200_000;
/// Above this many points a point cloud is thinned for display.
pub const MAX_DISPLAY_POINTS: usize = 200_000;

/// What drives the display colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorBy {
    Object,
    /// Float attribute `dist` (deviation, mm).
    Dist,
    /// Float attribute `temp_c` (surface temperature).
    TempC,
    /// Colour attribute `Cd` (0..1 RGB).
    Cd,
}

impl ColorBy {
    pub const ALL: [ColorBy; 4] = [ColorBy::Object, ColorBy::Dist, ColorBy::TempC, ColorBy::Cd];

    pub fn label(self) -> &'static str {
        match self {
            ColorBy::Object => "Object colour",
            ColorBy::Dist => "Deviation (dist)",
            ColorBy::TempC => "Temperature (temp_c)",
            ColorBy::Cd => "Colour attribute (Cd)",
        }
    }

    fn attrib(self) -> Option<&'static str> {
        match self {
            ColorBy::Object => None,
            ColorBy::Dist => Some("dist"),
            ColorBy::TempC => Some("temp_c"),
            ColorBy::Cd => Some("Cd"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RenderMesh {
    pub positions: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    /// One colour per entry of `tris` when colouring by an attribute.
    pub tri_colors: Option<Vec<[u8; 3]>>,
    /// Point colours for point clouds coloured by an attribute.
    pub point_colors: Option<Vec<[u8; 3]>>,
    /// Open polyline segments.
    pub lines: Vec<[u32; 2]>,
    /// Indices of points drawn as dots (point clouds only).
    pub dots: Vec<u32>,
    pub min: Vec3,
    pub max: Vec3,
    /// Triangle count before decimation.
    pub source_tris: usize,
    /// Value range of the colour attribute, for the legend.
    pub range: Option<(f64, f64)>,
}

impl RenderMesh {
    pub fn is_decimated(&self) -> bool {
        self.source_tris > self.tris.len()
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }
}

/// Blue-cyan-green-yellow-red ramp for `t` in 0..1.
pub fn ramp(t: f64) -> [u8; 3] {
    let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
    let stops: [[f64; 3]; 5] = [
        [40.0, 70.0, 220.0],
        [40.0, 200.0, 230.0],
        [70.0, 200.0, 90.0],
        [240.0, 210.0, 60.0],
        [225.0, 60.0, 50.0],
    ];
    let x = t * 4.0;
    let i = (x.floor() as usize).min(3);
    let f = x - i as f64;
    let a = stops[i];
    let b = stops[i + 1];
    [0, 1, 2].map(|c| (a[c] + (b[c] - a[c]) * f).round() as u8)
}

enum Source<'a> {
    Point(&'a AttribArray),
    Prim(&'a AttribArray),
}

/// Build the display mesh of `geo`.
pub fn build(geo: &Geometry, color_by: ColorBy) -> RenderMesh {
    let mut m = RenderMesh {
        positions: geo.points.iter().map(|p| p.as_vec3()).collect(),
        ..Default::default()
    };
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &m.positions {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    if m.positions.is_empty() {
        lo = Vec3::ZERO;
        hi = Vec3::ZERO;
    }
    m.min = lo;
    m.max = hi;

    let all = geo.triangulated();
    m.source_tris = all.len();
    let kept = decimate(&geo.points, all, MAX_DISPLAY_TRIS);

    for prim in geo.prims.iter().filter(|p| !p.closed) {
        for w in prim.vertices.windows(2) {
            m.lines.push([w[0], w[1]]);
        }
    }
    if geo.prims.is_empty() {
        let n = geo.points.len();
        let stride = n.div_ceil(MAX_DISPLAY_POINTS).max(1);
        m.dots = (0..n as u32).step_by(stride).collect();
    }

    let source = color_by.attrib().and_then(|name| {
        geo.point_attribs
            .get(name)
            .filter(|a| a.len() == geo.points.len())
            .map(Source::Point)
            .or_else(|| {
                geo.prim_attribs
                    .get(name)
                    .filter(|a| a.len() == geo.prims.len())
                    .map(Source::Prim)
            })
    });
    if let Some(src) = source {
        let arr = match src {
            Source::Point(a) | Source::Prim(a) => a,
        };
        let range = match arr {
            AttribArray::Float(v) => min_max(v),
            _ => None,
        };
        m.range = range;
        let color = |i: usize| -> [u8; 3] {
            match arr {
                AttribArray::Float(v) => {
                    let (a, b) = range.unwrap_or((0.0, 1.0));
                    let span = if (b - a).abs() < 1e-12 { 1.0 } else { b - a };
                    ramp((v[i] - a) / span)
                }
                AttribArray::Vec3(v) => {
                    let c = v[i].clamp(DVec3::ZERO, DVec3::ONE) * 255.0;
                    [c.x.round() as u8, c.y.round() as u8, c.z.round() as u8]
                }
                AttribArray::Int(v) => ramp((v[i].rem_euclid(8)) as f64 / 7.0),
                AttribArray::Str(_) => [200, 200, 200],
            }
        };
        match src {
            Source::Point(_) => {
                let pc: Vec<[u8; 3]> = (0..geo.points.len()).map(color).collect();
                m.tri_colors = Some(
                    kept.iter()
                        .map(|(_, t)| {
                            let mut s = [0u32; 3];
                            for &v in t {
                                let c = pc[v as usize];
                                for (acc, ch) in s.iter_mut().zip(c) {
                                    *acc += ch as u32;
                                }
                            }
                            s.map(|x| (x / 3) as u8)
                        })
                        .collect(),
                );
                m.point_colors = Some(pc);
            }
            Source::Prim(_) => {
                m.tri_colors = Some(kept.iter().map(|(p, _)| color(*p)).collect());
            }
        }
    }
    m.tris = kept.into_iter().map(|(_, t)| t).collect();
    m
}

/// Reduce `tris` (source primitive, point indices) to at most `max` by vertex
/// clustering: the points are binned in a uniform grid, every point is
/// replaced by the point nearest its cell's centroid, and triangles that
/// collapse (two corners in one cell) or repeat are dropped. Neighbouring
/// triangles keep sharing their edges, so a closed surface stays closed. The
/// grid is the finest one (up to 2048 cells along the longest side) that
/// leaves no more than `max` triangles.
pub fn decimate(points: &[DVec3], mut tris: Vec<(usize, [u32; 3])>, max: usize) -> Vec<(usize, [u32; 3])> {
    if tris.len() <= max {
        return tris;
    }
    let mut used = vec![false; points.len()];
    for (_, t) in &tris {
        for &v in t {
            if let Some(u) = used.get_mut(v as usize) {
                *u = true;
            }
        }
    }
    let ids: Vec<u32> = (0..points.len() as u32).filter(|&i| used[i as usize]).collect();
    let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
    for &i in &ids {
        let p = points[i as usize];
        if p.is_finite() {
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    if !lo.is_finite() || !hi.is_finite() {
        tris.truncate(max);
        return tris;
    }
    let size = (hi - lo).max_element().max(1e-12);
    // Non-finite points sit at the corner rather than poisoning a cell.
    let at = |i: u32| {
        let p = points[i as usize];
        if p.is_finite() { p } else { lo }
    };
    // The representative point of every used point on a `res`^3 grid.
    let reps_for = |res: u64| -> Vec<u32> {
        let scale = res as f64 / size;
        let cell = |p: DVec3| -> u64 {
            let q = |v: f64| ((v * scale).floor().max(0.0) as u64).min(res - 1);
            let d = p - lo;
            q(d.x) | q(d.y) << 16 | q(d.z) << 32
        };
        let mut sums: HashMap<u64, (DVec3, u32)> = HashMap::new();
        for &i in &ids {
            let p = at(i);
            let e = sums.entry(cell(p)).or_insert((DVec3::ZERO, 0));
            e.0 += p;
            e.1 += 1;
        }
        let mut best: HashMap<u64, (u32, f64)> = HashMap::new();
        for &i in &ids {
            let p = at(i);
            let c = cell(p);
            let (sum, n) = sums[&c];
            let d2 = (p - sum / f64::from(n)).length_squared();
            let e = best.entry(c).or_insert((i, d2));
            if d2 < e.1 {
                *e = (i, d2);
            }
        }
        let mut reps = vec![u32::MAX; points.len()];
        for &i in &ids {
            reps[i as usize] = best[&cell(at(i))].0;
        }
        reps
    };
    // Surviving triangles on the grid `reps`; stops once more than `limit`.
    let survivors = |reps: &[u32], limit: usize, out: &mut Vec<(usize, [u32; 3])>| -> bool {
        let mut seen: HashSet<[u32; 3]> = HashSet::new();
        out.clear();
        for &(prim, t) in &tris {
            let r = [reps[t[0] as usize], reps[t[1] as usize], reps[t[2] as usize]];
            if r[0] == r[1] || r[1] == r[2] || r[0] == r[2] || r.contains(&u32::MAX) {
                continue;
            }
            let mut key = r;
            key.sort_unstable();
            if seen.insert(key) {
                out.push((prim, r));
                if out.len() > limit {
                    return false;
                }
            }
        }
        true
    };
    // The finest grid that fits (a single cell always does: no triangles).
    let (mut fit, mut over) = (1u64, 2049u64);
    let mut scratch = Vec::new();
    while over - fit > 1 {
        let mid = (fit + over) / 2;
        if survivors(&reps_for(mid), max, &mut scratch) {
            fit = mid;
        } else {
            over = mid;
        }
    }
    let reps = reps_for(fit);
    let mut out = Vec::new();
    survivors(&reps, max, &mut out);
    out
}

fn min_max(v: &[f64]) -> Option<(f64, f64)> {
    let mut it = v.iter().copied().filter(|x| x.is_finite());
    let first = it.next()?;
    Some(it.fold((first, first), |(a, b), x| (a.min(x), b.max(x))))
}

/// Moller-Trumbore ray/triangle intersection: `(t, u, v)` for a hit with t > 0.
pub fn ray_triangle(o: DVec3, d: DVec3, v0: DVec3, v1: DVec3, v2: DVec3) -> Option<(f64, f64, f64)> {
    let e1 = v1 - v0;
    let e2 = v2 - v0;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let t0 = o - v0;
    let u = t0.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = t0.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some((t, u, v))
}

/// Slab test: entry distance of a ray into a box, if it hits.
pub fn ray_box(o: DVec3, d: DVec3, min: DVec3, max: DVec3) -> Option<f64> {
    let mut t0 = f64::NEG_INFINITY;
    let mut t1 = f64::INFINITY;
    for i in 0..3 {
        if d[i].abs() < 1e-15 {
            if o[i] < min[i] || o[i] > max[i] {
                return None;
            }
        } else {
            let a = (min[i] - o[i]) / d[i];
            let b = (max[i] - o[i]) / d[i];
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
        }
    }
    (t1 >= t0.max(0.0)).then_some(t0.max(0.0))
}

/// Nearest hit distance of a ray against a display mesh. Point clouds and
/// polylines are picked within `tol` mm of the ray.
pub fn pick(mesh: &RenderMesh, o: DVec3, d: DVec3, tol: f64) -> Option<f64> {
    let pad = DVec3::splat(tol.max(0.0));
    ray_box(o, d, mesh.min.as_dvec3() - pad, mesh.max.as_dvec3() + pad)?;
    let p = |i: u32| mesh.positions[i as usize].as_dvec3();
    let mut best: Option<f64> = None;
    let mut keep = |t: f64| {
        if best.is_none_or(|b| t < b) {
            best = Some(t);
        }
    };
    for t in &mesh.tris {
        if let Some((hit, _, _)) = ray_triangle(o, d, p(t[0]), p(t[1]), p(t[2])) {
            keep(hit);
        }
    }
    let near_ray = |q: DVec3| {
        let t = (q - o).dot(d);
        (t > 0.0 && (o + d * t - q).length() <= tol).then_some(t)
    };
    for &i in &mesh.dots {
        if let Some(t) = near_ray(p(i)) {
            keep(t);
        }
    }
    for l in &mesh.lines {
        let (a, b) = (p(l[0]), p(l[1]));
        for k in 0..=8 {
            if let Some(t) = near_ray(a.lerp(b, k as f64 / 8.0)) {
                keep(t);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use vernon_core::Prim;

    #[test]
    fn golden_ray_pick() {
        let o = DVec3::new(0.25, 0.25, 10.0);
        let d = -DVec3::Z;
        let (t, u, v) = ray_triangle(o, d, DVec3::ZERO, DVec3::X, DVec3::Y).unwrap();
        assert!((t - 10.0).abs() < 1e-12 && (u - 0.25).abs() < 1e-12 && (v - 0.25).abs() < 1e-12);
        let o = DVec3::new(0.8, 0.8, 10.0);
        assert!(ray_triangle(o, d, DVec3::ZERO, DVec3::X, DVec3::Y).is_none());
    }

    #[test]
    fn box_mesh_and_pick() {
        let g = Geometry::make_box(DVec3::splat(100.0));
        let m = build(&g, ColorBy::Object);
        assert_eq!(m.tris.len(), 12);
        assert_eq!(m.source_tris, 12);
        assert!(!m.is_decimated());
        let t = pick(&m, DVec3::new(0.0, 0.0, 500.0), -DVec3::Z, 0.0).unwrap();
        assert!((t - 450.0).abs() < 1e-6);
        assert!(pick(&m, DVec3::new(300.0, 0.0, 500.0), -DVec3::Z, 0.0).is_none());
    }

    #[test]
    fn decimates_large_meshes() {
        let n = 250_000u32;
        let mut g = Geometry::new();
        g.points = vec![DVec3::ZERO, DVec3::X, DVec3::Y];
        for _ in 0..n {
            g.prims.push(Prim::polygon(vec![0, 1, 2]));
        }
        let m = build(&g, ColorBy::Object);
        assert!(m.tris.len() <= MAX_DISPLAY_TRIS);
        assert_eq!(m.source_tris, n as usize);
        assert!(m.is_decimated());
    }

    #[test]
    fn decimation_keeps_a_closed_surface_closed() {
        // A UV sphere of 2 * 300 * 400 - ... > 200k triangles.
        let (nu, nv) = (400usize, 300usize);
        let mut pts = Vec::new();
        for j in 0..=nv {
            let th = std::f64::consts::PI * j as f64 / nv as f64;
            for i in 0..nu {
                let ph = std::f64::consts::TAU * i as f64 / nu as f64;
                pts.push(DVec3::new(th.sin() * ph.cos(), th.sin() * ph.sin(), th.cos()) * 100.0);
            }
        }
        let id = |i: usize, j: usize| (j * nu + i % nu) as u32;
        let mut faces = Vec::new();
        for j in 0..nv {
            for i in 0..nu {
                faces.push(vec![id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
                faces.push(vec![id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
            }
        }
        let g = Geometry::from_polygons(pts, faces);
        let m = build(&g, ColorBy::Object);
        assert!(m.source_tris > MAX_DISPLAY_TRIS);
        assert!(m.is_decimated());
        // Reasonably dense, not a handful of triangles.
        assert!(m.tris.len() <= MAX_DISPLAY_TRIS && m.tris.len() > MAX_DISPLAY_TRIS / 8, "{}", m.tris.len());
        // Every ray at the centre hits: nothing was punched out.
        let mut hits = 0;
        let n = 300;
        for k in 0..n {
            let a = k as f64 * 2.399963;
            let z = 1.0 - 2.0 * (k as f64 + 0.5) / n as f64;
            let r = (1.0 - z * z).sqrt();
            let dir = DVec3::new(r * a.cos(), r * a.sin(), z);
            if pick(&m, dir * 500.0, -dir, 0.0).is_some() {
                hits += 1;
            }
        }
        assert_eq!(hits, n, "rays that slipped through a hole");
    }

    #[test]
    fn colours_by_float_attribute() {
        let mut g = Geometry::make_box(DVec3::splat(10.0));
        let dist: Vec<f64> = (0..8).map(|i| i as f64).collect();
        g.point_attribs.set("dist", AttribArray::Float(dist));
        let m = build(&g, ColorBy::Dist);
        assert_eq!(m.tri_colors.as_ref().unwrap().len(), m.tris.len());
        assert_eq!(m.range, Some((0.0, 7.0)));
        assert_eq!(ramp(0.0), [40, 70, 220]);
        assert_eq!(ramp(1.0), [225, 60, 50]);
        // Missing attribute falls back to object colour.
        assert!(build(&g, ColorBy::TempC).tri_colors.is_none());
    }

    #[test]
    fn point_clouds_become_dots() {
        let g = Geometry::from_points(vec![DVec3::ZERO, DVec3::new(0.0, 0.0, 5.0)]);
        let m = build(&g, ColorBy::Object);
        assert_eq!(m.dots.len(), 2);
        let t = pick(&m, DVec3::new(0.1, 0.0, 100.0), -DVec3::Z, 0.5).unwrap();
        assert!((t - 95.0).abs() < 1e-9);
    }
}

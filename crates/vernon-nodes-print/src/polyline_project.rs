//! Projection of a 3D polyline onto a triangle mesh, and cutting the mesh along it.
//!
//! Method
//! * Each polyline point is moved to its nearest point on the mesh (closest point on a
//!   triangle by Voronoi-region case analysis, Ericson, "Real-Time Collision Detection",
//!   section 5.1.5; exact edition page UNVERIFIED) and classified as a vertex hit, an
//!   edge point or a face point.
//! * Consecutive projected points are joined by walking across triangle edges: the cutting
//!   plane through both points, parallel to the average surface normal, is followed from
//!   triangle to triangle in the direction of the next point. The walk is monotone along the
//!   segment, so it terminates; it fails with an error rather than looping when the surface
//!   folds back, and with [`PathError::LeavesMesh`] when it runs into a mesh boundary.
//! * Cutting inserts every path point as a vertex (splitting edges or faces), forces every
//!   path segment to be a mesh edge by removing the triangles it crosses and re-triangulating
//!   the two pockets (constrained segment insertion), then labels face components that are
//!   not connected across a path edge and gives each its own vertices. Optional caps close
//!   each open boundary loop by an ear-clipped planar-projected polygon.
//!
//! Units are the caller's (millimetres in Vernon). All tolerances scale with the mesh bounding
//! box diagonal. Limitations: an open path that does not separate the mesh is reported as
//! [`PathError::NotSeparating`]; a path that crosses itself is rejected; the walk follows the
//! cutting plane monotonically, so a polyline whose consecutive points are far apart on a
//! strongly curved surface may fail with [`PathError::WalkFailed`].
use glam::{DVec2, DVec3};
use std::collections::{HashMap, HashSet};
use std::fmt;

/// Indexed triangle mesh.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TriMesh {
    pub verts: Vec<DVec3>,
    pub tris: Vec<[u32; 3]>,
}

impl TriMesh {
    pub fn new(verts: Vec<DVec3>, tris: Vec<[u32; 3]>) -> Self {
        TriMesh { verts, tris }
    }

    /// Volume by the divergence theorem (positive for outward winding).
    pub fn signed_volume(&self) -> f64 {
        self.tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| self.verts[i as usize]);
                a.dot(b.cross(c)) / 6.0
            })
            .sum()
    }

    /// Axis-aligned bounds; `None` for an empty vertex list.
    pub fn bounds(&self) -> Option<(DVec3, DVec3)> {
        let first = *self.verts.first()?;
        Some(self.verts.iter().fold((first, first), |(lo, hi), v| (lo.min(*v), hi.max(*v))))
    }

    /// Every directed edge appears exactly once and has its reverse exactly once.
    pub fn is_watertight(&self) -> bool {
        if self.tris.is_empty() {
            return false;
        }
        let mut dir: HashMap<(u32, u32), u32> = HashMap::new();
        for t in &self.tris {
            for k in 0..3 {
                *dir.entry((t[k], t[(k + 1) % 3])).or_default() += 1;
            }
        }
        dir.iter().all(|(&(a, b), &n)| n == 1 && dir.get(&(b, a)) == Some(&1))
    }

    fn scale(&self) -> f64 {
        self.bounds().map_or(1.0, |(lo, hi)| (hi - lo).length().max(1e-12))
    }
}

/// Where a projected point sits on the original mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Site {
    Vertex(u32),
    /// On edge `a < b` at `a + t (b - a)`.
    Edge { a: u32, b: u32, t: f64 },
    /// Interior of a triangle.
    Face(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathPoint {
    pub pos: DVec3,
    pub site: Site,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PathError {
    EmptyMesh,
    TooFewPoints,
    NonFinite,
    /// Input point `index` is `distance` away from the mesh, more than the tolerance.
    FarFromMesh { index: usize, distance: f64 },
    /// The walk from path point `segment` reached a mesh boundary.
    LeavesMesh { segment: usize },
    WalkFailed { segment: usize, reason: &'static str },
    /// A path point could not be located on the mesh while cutting.
    NotOnMesh,
    SelfIntersecting,
    /// The path does not split the mesh into at least two components.
    NotSeparating,
    CapFailed,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::EmptyMesh => write!(f, "mesh has no triangles"),
            PathError::TooFewPoints => write!(f, "polyline needs at least 2 distinct points (3 when closed)"),
            PathError::NonFinite => write!(f, "polyline contains a non-finite coordinate"),
            PathError::FarFromMesh { index, distance } => {
                write!(f, "point {index} is {distance} from the mesh")
            }
            PathError::LeavesMesh { segment } => write!(f, "path leaves the mesh in segment {segment}"),
            PathError::WalkFailed { segment, reason } => write!(f, "walk failed in segment {segment}: {reason}"),
            PathError::NotOnMesh => write!(f, "path point is not on the mesh"),
            PathError::SelfIntersecting => write!(f, "path crosses itself"),
            PathError::NotSeparating => write!(f, "path does not separate the mesh"),
            PathError::CapFailed => write!(f, "could not cap a boundary loop"),
        }
    }
}
impl std::error::Error for PathError {}

fn ekey(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

struct Topo {
    edge_tris: HashMap<(u32, u32), Vec<usize>>,
    vert_tris: Vec<Vec<usize>>,
}

impl Topo {
    fn new(m: &TriMesh) -> Topo {
        let mut edge_tris: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        let mut vert_tris = vec![Vec::new(); m.verts.len()];
        for (i, t) in m.tris.iter().enumerate() {
            for k in 0..3 {
                edge_tris.entry(ekey(t[k], t[(k + 1) % 3])).or_default().push(i);
                vert_tris[t[k] as usize].push(i);
            }
        }
        Topo { edge_tris, vert_tris }
    }

    fn incident(&self, s: &Site) -> Vec<usize> {
        match *s {
            Site::Face(f) => vec![f],
            Site::Edge { a, b, .. } => self.edge_tris.get(&ekey(a, b)).cloned().unwrap_or_default(),
            Site::Vertex(v) => self.vert_tris[v as usize].clone(),
        }
    }

    fn on_boundary(&self, m: &TriMesh, s: &Site) -> bool {
        match *s {
            Site::Face(_) => false,
            Site::Edge { a, b, .. } => self.edge_tris.get(&ekey(a, b)).map_or(false, |v| v.len() == 1),
            Site::Vertex(v) => self.vert_tris[v as usize].iter().any(|&t| {
                let tri = m.tris[t];
                (0..3).any(|k| {
                    let e = ekey(tri[k], tri[(k + 1) % 3]);
                    (e.0 == v || e.1 == v) && self.edge_tris[&e].len() == 1
                })
            }),
        }
    }
}

/// Barycentric weights of the point of triangle `abc` closest to `p`.
fn closest_bary(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> [f64; 3] {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return [1.0, 0.0, 0.0];
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return [0.0, 1.0, 0.0];
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return [1.0 - v, v, 0.0];
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return [0.0, 0.0, 1.0];
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return [1.0 - w, 0.0, w];
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return [0.0, 1.0 - w, w];
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    [1.0 - v - w, v, w]
}

const SNAP: f64 = 1e-9;

fn edge_site(m: &TriMesh, ti: u32, tj: u32, s: f64) -> PathPoint {
    // s runs from ti to tj.
    let (a, b, t) = if ti < tj { (ti, tj, s) } else { (tj, ti, 1.0 - s) };
    let pos = m.verts[a as usize].lerp(m.verts[b as usize], t);
    if t < SNAP {
        return PathPoint { pos: m.verts[a as usize], site: Site::Vertex(a) };
    }
    if t > 1.0 - SNAP {
        return PathPoint { pos: m.verts[b as usize], site: Site::Vertex(b) };
    }
    PathPoint { pos, site: Site::Edge { a, b, t } }
}

fn classify(m: &TriMesh, f: usize, w: [f64; 3]) -> PathPoint {
    let t = m.tris[f];
    let zero = [w[0] < SNAP, w[1] < SNAP, w[2] < SNAP];
    match zero.iter().filter(|z| **z).count() {
        2 => {
            let k = (0..3).find(|&k| !zero[k]).unwrap_or(0);
            PathPoint { pos: m.verts[t[k] as usize], site: Site::Vertex(t[k]) }
        }
        1 => {
            let z = (0..3).find(|&k| zero[k]).unwrap_or(0);
            let (i, j) = ((z + 1) % 3, (z + 2) % 3);
            edge_site(m, t[i], t[j], w[j] / (w[i] + w[j]))
        }
        _ => {
            let [a, b, c] = t.map(|i| m.verts[i as usize]);
            PathPoint { pos: a * w[0] + b * w[1] + c * w[2], site: Site::Face(f) }
        }
    }
}

fn nearest(m: &TriMesh, p: DVec3) -> Option<(PathPoint, f64)> {
    let mut best: Option<(usize, [f64; 3], f64)> = None;
    for (i, t) in m.tris.iter().enumerate() {
        let [a, b, c] = t.map(|k| m.verts[k as usize]);
        if (b - a).cross(c - a).length_squared() < 1e-300 {
            continue;
        }
        let w = closest_bary(p, a, b, c);
        let d = (a * w[0] + b * w[1] + c * w[2] - p).length();
        if best.map_or(true, |(_, _, bd)| d < bd) {
            best = Some((i, w, d));
        }
    }
    best.map(|(i, w, d)| (classify(m, i, w), d))
}

fn tri_normal(m: &TriMesh, t: usize) -> DVec3 {
    let [a, b, c] = m.tris[t].map(|k| m.verts[k as usize]);
    (b - a).cross(c - a).normalize_or_zero()
}

enum WalkFail {
    Leaves,
    Failed(&'static str),
}

/// Intermediate path points strictly between `p` and `q` (both on the surface).
fn walk(m: &TriMesh, topo: &Topo, eps: f64, p: &PathPoint, q: &PathPoint) -> Result<Vec<PathPoint>, WalkFail> {
    let tq: HashSet<usize> = topo.incident(&q.site).into_iter().collect();
    let tp = topo.incident(&p.site);
    if tp.iter().any(|t| tq.contains(t)) {
        return Ok(Vec::new());
    }
    let dir = q.pos - p.pos;
    let len = dir.length();
    if len <= eps {
        return Ok(Vec::new());
    }
    let dirn = dir / len;
    let n_sum: DVec3 = tp.iter().chain(tq.iter()).map(|&t| tri_normal(m, t)).sum();
    let mut plane_n = dirn.cross(n_sum);
    if plane_n.length() < 1e-9 * n_sum.length().max(1e-12) {
        plane_n = tp.first().map_or(DVec3::ZERO, |&t| dirn.cross(tri_normal(m, t)));
    }
    let plane_n = plane_n.try_normalize().ok_or(WalkFail::Failed("degenerate cutting plane"))?;

    let mut c = *p;
    let mut out = Vec::new();
    for _ in 0..(m.tris.len() * 2 + 8) {
        let inc = topo.incident(&c.site);
        if inc.iter().any(|t| tq.contains(t)) {
            return Ok(out);
        }
        let mut best: Option<PathPoint> = None;
        let mut best_dot = eps;
        for &t in &inc {
            let tri = m.tris[t];
            let pts = tri.map(|k| m.verts[k as usize]);
            let mut d = [0.0; 3];
            for i in 0..3 {
                let v = (pts[i] - p.pos).dot(plane_n);
                d[i] = if v.abs() < eps { 0.0 } else { v };
            }
            let mut cands: Vec<PathPoint> = Vec::new();
            for i in 0..3 {
                if d[i] == 0.0 {
                    cands.push(PathPoint { pos: pts[i], site: Site::Vertex(tri[i]) });
                }
                let j = (i + 1) % 3;
                if d[i] * d[j] < 0.0 {
                    cands.push(edge_site(m, tri[i], tri[j], d[i] / (d[i] - d[j])));
                }
            }
            for cand in cands {
                let dot = (cand.pos - c.pos).dot(dirn);
                if dot > best_dot {
                    best_dot = dot;
                    best = Some(cand);
                }
            }
        }
        match best {
            None => {
                return Err(if topo.on_boundary(m, &c.site) {
                    WalkFail::Leaves
                } else {
                    WalkFail::Failed("no forward crossing")
                })
            }
            Some(b) => {
                if (b.pos - p.pos).dot(dirn) > len + eps {
                    return Err(WalkFail::Failed("overshoot"));
                }
                out.push(b);
                c = b;
            }
        }
    }
    Err(WalkFail::Failed("too many steps"))
}

/// Project `pts` onto `mesh` and join the projections by edge-walking.
/// Returns the full path (input projections and crossings), without repeating the first
/// point at the end of a closed path.
pub fn project_polyline(
    mesh: &TriMesh,
    pts: &[DVec3],
    closed: bool,
    max_distance: f64,
) -> Result<Vec<PathPoint>, PathError> {
    if mesh.tris.is_empty() {
        return Err(PathError::EmptyMesh);
    }
    if pts.iter().any(|p| !p.is_finite()) {
        return Err(PathError::NonFinite);
    }
    let eps = 1e-9 * mesh.scale();
    let mut proj: Vec<PathPoint> = Vec::new();
    for (index, p) in pts.iter().enumerate() {
        let (pp, distance) = nearest(mesh, *p).ok_or(PathError::EmptyMesh)?;
        if distance > max_distance {
            return Err(PathError::FarFromMesh { index, distance });
        }
        if proj.last().map_or(true, |l| l.pos.distance(pp.pos) > eps) {
            proj.push(pp);
        }
    }
    if closed && proj.len() > 1 && proj[0].pos.distance(proj[proj.len() - 1].pos) <= eps {
        proj.pop();
    }
    if proj.len() < if closed { 3 } else { 2 } {
        return Err(PathError::TooFewPoints);
    }
    let topo = Topo::new(mesh);
    let n = proj.len();
    let segs = if closed { n } else { n - 1 };
    let mut out = Vec::new();
    for k in 0..segs {
        out.push(proj[k]);
        let mid = walk(mesh, &topo, eps, &proj[k], &proj[(k + 1) % n]).map_err(|e| match e {
            WalkFail::Leaves => PathError::LeavesMesh { segment: k },
            WalkFail::Failed(reason) => PathError::WalkFailed { segment: k, reason },
        })?;
        out.extend(mid);
    }
    if !closed {
        out.push(proj[n - 1]);
    }
    Ok(out)
}

/// Total length of a path (including the closing segment).
pub fn path_length(path: &[PathPoint], closed: bool) -> f64 {
    let n = path.len();
    let segs = if closed && n > 1 { n } else { n.saturating_sub(1) };
    (0..segs).map(|k| path[k].pos.distance(path[(k + 1) % n].pos)).sum()
}

/// For every path segment, its length and the triangles of `mesh` that contain it
/// (triangles incident to both end sites).
pub fn path_segments(mesh: &TriMesh, path: &[PathPoint], closed: bool) -> Vec<(f64, Vec<usize>)> {
    let topo = Topo::new(mesh);
    let n = path.len();
    let segs = if closed && n > 1 { n } else { n.saturating_sub(1) };
    (0..segs)
        .map(|k| {
            let (a, b) = (&path[k], &path[(k + 1) % n]);
            let tb: HashSet<usize> = topo.incident(&b.site).into_iter().collect();
            let mut tris: Vec<usize> = topo.incident(&a.site).into_iter().filter(|t| tb.contains(t)).collect();
            tris.sort_unstable();
            (a.pos.distance(b.pos), tris)
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Cutting
// ---------------------------------------------------------------------------------------

fn dist_to_segment(p: DVec3, a: DVec3, b: DVec3) -> f64 {
    let ab = b - a;
    let l2 = ab.length_squared();
    let t = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

fn split_edge(w: &mut TriMesh, a: u32, b: u32, v: u32) {
    let key = ekey(a, b);
    let mut extra = Vec::new();
    for t in w.tris.iter_mut() {
        for k in 0..3 {
            let (x, y, z) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
            if ekey(x, y) == key {
                *t = [x, v, z];
                extra.push([v, y, z]);
                break;
            }
        }
    }
    w.tris.extend(extra);
}

fn insert_point(w: &mut TriMesh, pos: DVec3, eps: f64) -> Result<u32, PathError> {
    if let Some(i) = w.verts.iter().position(|v| v.distance(pos) <= eps) {
        return Ok(i as u32);
    }
    // On an existing edge.
    let mut hit: Option<(u32, u32)> = None;
    'outer: for t in &w.tris {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if dist_to_segment(pos, w.verts[a as usize], w.verts[b as usize]) <= eps {
                hit = Some((a, b));
                break 'outer;
            }
        }
    }
    if let Some((a, b)) = hit {
        let v = w.verts.len() as u32;
        w.verts.push(pos);
        split_edge(w, a, b, v);
        return Ok(v);
    }
    // Inside a face.
    for i in 0..w.tris.len() {
        let [a, b, c] = w.tris[i].map(|k| w.verts[k as usize]);
        let n = (b - a).cross(c - a);
        let area2 = n.length();
        if area2 < 1e-300 {
            continue;
        }
        let nh = n / area2;
        if (pos - a).dot(nh).abs() > eps {
            continue;
        }
        let wa = (c - b).cross(pos - b).dot(nh) / area2;
        let wb = (a - c).cross(pos - c).dot(nh) / area2;
        let wc = (b - a).cross(pos - a).dot(nh) / area2;
        if wa >= -SNAP && wb >= -SNAP && wc >= -SNAP {
            let v = w.verts.len() as u32;
            w.verts.push(pos);
            let [ia, ib, ic] = w.tris[i];
            w.tris[i] = [ia, ib, v];
            w.tris.push([ib, ic, v]);
            w.tris.push([ic, ia, v]);
            return Ok(v);
        }
    }
    Err(PathError::NotOnMesh)
}

fn project2(p: DVec3, u: DVec3, v: DVec3) -> DVec2 {
    DVec2::new(p.dot(u), p.dot(v))
}

fn frame(n: DVec3) -> (DVec3, DVec3) {
    let helper = if n.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
    let u = n.cross(helper).normalize();
    (u, n.cross(u))
}

/// Triangulate a ring of vertex ids lying (nearly) in a plane, orienting every triangle
/// so that its normal agrees with `n`.
fn triangulate_ring(w: &TriMesh, ring: &[u32], n: DVec3) -> Vec<[u32; 3]> {
    if ring.len() < 3 {
        return Vec::new();
    }
    let (u, v) = frame(n);
    let pts: Vec<DVec2> = ring.iter().map(|&i| project2(w.verts[i as usize], u, v)).collect();
    poly_triangulate(&pts)
        .into_iter()
        .map(|t| {
            let tri = [ring[t[0]], ring[t[1]], ring[t[2]]];
            let [a, b, c] = tri.map(|i| w.verts[i as usize]);
            if (b - a).cross(c - a).dot(n) >= 0.0 {
                tri
            } else {
                [tri[0], tri[2], tri[1]]
            }
        })
        .collect()
}

fn poly_triangulate(pts: &[DVec2]) -> Vec<[usize; 3]> {
    crate::poly::triangulate(pts, &[])
}

/// Make the segment `u`-`v` a chain of mesh edges; returns the vertex chain `u..v`.
fn insert_segment(
    w: &mut TriMesh,
    u: u32,
    v: u32,
    eps: f64,
    cut: &HashSet<(u32, u32)>,
) -> Result<Vec<u32>, PathError> {
    let mut chain = vec![u];
    let mut cur = u;
    let mut guard = 0usize;
    while cur != v {
        guard += 1;
        if guard > w.verts.len() + w.tris.len() + 16 {
            return Err(PathError::WalkFailed { segment: 0, reason: "segment insertion did not terminate" });
        }
        let (pc, pv) = (w.verts[cur as usize], w.verts[v as usize]);
        let d = pv - pc;
        let dl2 = d.length_squared();
        // Existing edge to v, or to a vertex lying on the segment: advance along it.
        let mut next: Option<(u32, f64)> = None;
        for t in &w.tris {
            if !t.contains(&cur) {
                continue;
            }
            for &x in t {
                if x == cur {
                    continue;
                }
                let px = w.verts[x as usize];
                let along = (px - pc).dot(d);
                if (x == v || dist_to_segment(px, pc, pv) <= eps) && along > 0.0 && next.map_or(true, |(_, a)| along > a) {
                    next = Some((x, along));
                }
            }
        }
        if let Some((x, _)) = next {
            chain.push(x);
            cur = x;
            continue;
        }
        // Find the fan triangle whose wedge contains the direction to v.
        let mut start: Option<(usize, u32, u32)> = None;
        for (i, t) in w.tris.iter().enumerate() {
            let Some(k) = t.iter().position(|&x| x == cur) else { continue };
            let (x, y) = (t[(k + 1) % 3], t[(k + 2) % 3]);
            let (ux, uy) = (w.verts[x as usize] - pc, w.verts[y as usize] - pc);
            let n = ux.cross(uy);
            let coplanar = n.try_normalize().map_or(false, |nh| d.dot(nh).abs() <= eps + 1e-9 * d.length());
            if coplanar && ux.cross(d).dot(n) >= 0.0 && d.cross(uy).dot(n) >= 0.0 && dl2 > 0.0 {
                start = Some((i, x, y));
                break;
            }
        }
        let (i0, x, y) = start.ok_or(PathError::WalkFailed { segment: 0, reason: "no wedge contains the segment" })?;
        let n0 = {
            let t = w.tris[i0];
            let [a, b, c] = t.map(|k| w.verts[k as usize]);
            (b - a).cross(c - a).normalize_or_zero()
        };
        if n0 == DVec3::ZERO {
            return Err(PathError::WalkFailed { segment: 0, reason: "degenerate triangle" });
        }
        let dn = d.length();
        let mut end = v;
        let (mut rx, mut ly) = (x, y);
        let mut right = vec![cur, x];
        let mut left = vec![cur, y];
        let mut removed = vec![i0];
        loop {
            if cut.contains(&ekey(rx, ly)) {
                return Err(PathError::SelfIntersecting);
            }
            // Triangle across edge rx-ly holds the directed edge ly -> rx.
            let mut found: Option<(usize, u32)> = None;
            for (j, t) in w.tris.iter().enumerate() {
                for k in 0..3 {
                    if t[k] == ly && t[(k + 1) % 3] == rx {
                        found = Some((j, t[(k + 2) % 3]));
                    }
                }
            }
            let (j, z) = found.ok_or(PathError::LeavesMesh { segment: 0 })?;
            removed.push(j);
            if z == end {
                right.push(end);
                left.push(end);
                break;
            }
            let side = (w.verts[z as usize] - pc).cross(d).dot(n0) / dn;
            if side.abs() <= eps {
                end = z;
                right.push(z);
                left.push(z);
                break;
            }
            if side > 0.0 {
                right.push(z);
                rx = z;
            } else {
                left.push(z);
                ly = z;
            }
        }
        let mut new_tris = triangulate_ring(w, &right, n0);
        new_tris.extend(triangulate_ring(w, &left, n0));
        let drop: HashSet<usize> = removed.into_iter().collect();
        let mut idx = 0usize;
        w.tris.retain(|_| {
            let keep = !drop.contains(&idx);
            idx += 1;
            keep
        });
        w.tris.extend(new_tris);
        chain.push(end);
        cur = end;
    }
    Ok(chain)
}

fn components(w: &TriMesh, cut: &HashSet<(u32, u32)>) -> Vec<Vec<usize>> {
    let mut edge_tris: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, t) in w.tris.iter().enumerate() {
        for k in 0..3 {
            edge_tris.entry(ekey(t[k], t[(k + 1) % 3])).or_default().push(i);
        }
    }
    let mut seen = vec![false; w.tris.len()];
    let mut out = Vec::new();
    for s in 0..w.tris.len() {
        if seen[s] {
            continue;
        }
        seen[s] = true;
        let mut comp = vec![s];
        let mut k = 0;
        while k < comp.len() {
            let t = w.tris[comp[k]];
            k += 1;
            for e in 0..3 {
                let key = ekey(t[e], t[(e + 1) % 3]);
                if cut.contains(&key) {
                    continue;
                }
                for &o in &edge_tris[&key] {
                    if !seen[o] {
                        seen[o] = true;
                        comp.push(o);
                    }
                }
            }
        }
        comp.sort_unstable();
        out.push(comp);
    }
    out
}

fn extract(w: &TriMesh, faces: &[usize]) -> TriMesh {
    let mut map: HashMap<u32, u32> = HashMap::new();
    let mut m = TriMesh::default();
    for &f in faces {
        let t = w.tris[f].map(|v| {
            *map.entry(v).or_insert_with(|| {
                m.verts.push(w.verts[v as usize]);
                (m.verts.len() - 1) as u32
            })
        });
        m.tris.push(t);
    }
    m
}

/// Close every boundary loop of `m` with a planar-projected ear-clipped polygon.
pub fn cap_boundaries(m: &mut TriMesh) -> Result<(), PathError> {
    let mut dir: HashSet<(u32, u32)> = HashSet::new();
    for t in &m.tris {
        for k in 0..3 {
            dir.insert((t[k], t[(k + 1) % 3]));
        }
    }
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut starts: Vec<(u32, u32)> = Vec::new();
    for t in &m.tris {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if !dir.contains(&(b, a)) {
                next.entry(a).or_default().push(b);
                starts.push((a, b));
            }
        }
    }
    let mut used: HashSet<(u32, u32)> = HashSet::new();
    let mut new_tris = Vec::new();
    for (a0, b0) in starts {
        if used.contains(&(a0, b0)) {
            continue;
        }
        let mut lp = vec![a0];
        let (mut a, mut b) = (a0, b0);
        loop {
            used.insert((a, b));
            if b == a0 {
                break;
            }
            lp.push(b);
            let nb = next
                .get(&b)
                .and_then(|v| v.iter().copied().find(|&c| !used.contains(&(b, c))))
                .ok_or(PathError::CapFailed)?;
            a = b;
            b = nb;
        }
        lp.reverse(); // the cap runs against the boundary half-edges
        let mut n = DVec3::ZERO;
        for k in 0..lp.len() {
            n += m.verts[lp[k] as usize].cross(m.verts[lp[(k + 1) % lp.len()] as usize]);
        }
        let n = n.try_normalize().ok_or(PathError::CapFailed)?;
        let tris = triangulate_ring(m, &lp, n);
        if tris.is_empty() {
            return Err(PathError::CapFailed);
        }
        new_tris.extend(tris);
    }
    m.tris.extend(new_tris);
    Ok(())
}

/// Cut `mesh` along a path produced by [`project_polyline`]. Returns the components ordered
/// by their smallest original triangle; each is capped when `cap` is set.
pub fn cut_along_path(mesh: &TriMesh, path: &[PathPoint], closed: bool, cap: bool) -> Result<Vec<TriMesh>, PathError> {
    if mesh.tris.is_empty() {
        return Err(PathError::EmptyMesh);
    }
    let eps = 1e-9 * mesh.scale();
    let mut w = mesh.clone();
    let mut ids: Vec<u32> = Vec::new();
    for p in path {
        let id = insert_point(&mut w, p.pos, eps)?;
        if ids.last() != Some(&id) {
            ids.push(id);
        }
    }
    if closed && ids.len() > 1 && ids.first() == ids.last() {
        ids.pop();
    }
    let n = ids.len();
    if n < 2 {
        return Err(PathError::TooFewPoints);
    }
    let mut cut: HashSet<(u32, u32)> = HashSet::new();
    let segs = if closed { n } else { n - 1 };
    for k in 0..segs {
        let (u, v) = (ids[k], ids[(k + 1) % n]);
        if u == v {
            continue;
        }
        let chain = insert_segment(&mut w, u, v, eps, &cut)?;
        for pair in chain.windows(2) {
            cut.insert(ekey(pair[0], pair[1]));
        }
    }
    let comps = components(&w, &cut);
    if comps.len() < 2 {
        return Err(PathError::NotSeparating);
    }
    let mut pieces: Vec<TriMesh> = comps.iter().map(|c| extract(&w, c)).collect();
    if cap {
        for p in pieces.iter_mut() {
            cap_boundaries(p)?;
        }
    }
    Ok(pieces)
}

/// Result of [`cut_along_polyline`].
#[derive(Debug, Clone)]
pub struct PolylineCut {
    pub pieces: Vec<TriMesh>,
    pub path: Vec<PathPoint>,
    pub seam_length: f64,
}

/// Project `pts` onto `mesh` and cut along the result.
pub fn cut_along_polyline(
    mesh: &TriMesh,
    pts: &[DVec3],
    closed: bool,
    max_distance: f64,
    cap: bool,
) -> Result<PolylineCut, PathError> {
    let path = project_polyline(mesh, pts, closed, max_distance)?;
    let pieces = cut_along_path(mesh, &path, closed, cap)?;
    let seam_length = path_length(&path, closed);
    Ok(PolylineCut { pieces, path, seam_length })
}

#[cfg(test)]
pub(crate) mod testmesh {
    use super::*;

    /// Axis-aligned box with outward winding; vertex `i` has bits x=1, y=2, z=4.
    pub fn box_mesh(l: f64, w: f64, h: f64) -> TriMesh {
        let verts = (0..8)
            .map(|i| DVec3::new(if i & 1 != 0 { l } else { 0.0 }, if i & 2 != 0 { w } else { 0.0 }, if i & 4 != 0 { h } else { 0.0 }))
            .collect();
        let tris = vec![
            [0, 1, 5], [0, 5, 4], // front  (-y)
            [2, 7, 3], [2, 6, 7], // back   (+y)
            [0, 4, 6], [0, 6, 2], // left   (-x)
            [1, 3, 7], [1, 7, 5], // right  (+x)
            [0, 2, 3], [0, 3, 1], // bottom (-z)
            [4, 5, 7], [4, 7, 6], // top    (+z)
        ];
        TriMesh::new(verts, tris)
    }
}

#[cfg(test)]
mod tests {
    use super::testmesh::box_mesh;
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// Every directed edge is matched by its reverse (a zero-thickness double-sided sheet
    /// passes; an open surface does not).
    fn balanced(m: &TriMesh) -> bool {
        let mut dir: HashMap<(u32, u32), i32> = HashMap::new();
        for t in &m.tris {
            for k in 0..3 {
                *dir.entry((t[k], t[(k + 1) % 3])).or_default() += 1;
            }
        }
        dir.iter().all(|(&(a, b), &n)| dir.get(&(b, a)) == Some(&n))
    }

    #[test]
    fn polyline_project_box_mesh_is_a_unit_volume_closed_cube() {
        let c = box_mesh(1.0, 1.0, 1.0);
        assert!(c.is_watertight());
        assert!(close(c.signed_volume(), 1.0));
    }

    #[test]
    fn polyline_project_vertex_hit_and_edge_hit() {
        let c = box_mesh(1.0, 1.0, 1.0);
        let (p, d) = nearest(&c, DVec3::new(-0.2, -0.2, -0.2)).unwrap();
        assert_eq!(p.site, Site::Vertex(0));
        assert!(close(d, 0.2 * 3f64.sqrt()));
        let (p, _) = nearest(&c, DVec3::new(0.5, -0.3, 0.0)).unwrap();
        assert!(matches!(p.site, Site::Edge { a: 0, b: 1, t } if close(t, 0.5)), "{:?}", p.site);
        let (p, _) = nearest(&c, DVec3::new(0.3, -0.1, 0.2)).unwrap();
        assert!(matches!(p.site, Site::Face(_)));
        assert!(close(p.pos.y, 0.0));
    }

    #[test]
    fn polyline_project_closed_loop_splits_cube_into_two_watertight_halves() {
        let c = box_mesh(1.0, 1.0, 1.0);
        let loop_pts = [
            DVec3::new(0.5, 0.0, 0.5),
            DVec3::new(1.0, 0.5, 0.5),
            DVec3::new(0.5, 1.0, 0.5),
            DVec3::new(0.0, 0.5, 0.5),
        ];
        let r = cut_along_polyline(&c, &loop_pts, true, 1e-6, true).unwrap();
        assert_eq!(r.pieces.len(), 2);
        // Seam is the unit-square perimeter at mid height.
        assert!(close(r.seam_length, 4.0), "{}", r.seam_length);
        let mut vols: Vec<f64> = r.pieces.iter().map(|p| p.signed_volume()).collect();
        for p in &r.pieces {
            assert!(p.is_watertight());
        }
        vols.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(close(vols[0], 0.5) && close(vols[1], 0.5), "{vols:?}");
    }

    #[test]
    fn polyline_project_path_along_vertices_isolates_a_face() {
        // Closed loop through the four corners of the front face (all vertex hits).
        let c = box_mesh(1.0, 1.0, 1.0);
        let corners = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(1.0, 0.0, 1.0),
            DVec3::new(0.0, 0.0, 1.0),
        ];
        let path = project_polyline(&c, &corners, true, 1e-9).unwrap();
        assert!(path.iter().all(|p| matches!(p.site, Site::Vertex(_))));
        let r = cut_along_polyline(&c, &corners, true, 1e-9, true).unwrap();
        assert_eq!(r.pieces.len(), 2);
        let total: f64 = r.pieces.iter().map(|p| p.signed_volume()).sum();
        assert!(close(total, 1.0), "{total}");
        assert!(r.pieces.iter().all(balanced));
    }

    #[test]
    fn polyline_project_path_inside_one_face_is_inserted_as_edges() {
        // A polyline wandering inside one triangle plus a closed ring that encloses a patch:
        // an inner triangle on the top face, with three interior points.
        let c = box_mesh(1.0, 1.0, 1.0);
        let ring = [DVec3::new(0.3, 0.2, 1.0), DVec3::new(0.6, 0.3, 1.0), DVec3::new(0.35, 0.45, 1.0)];
        let r = cut_along_polyline(&c, &ring, true, 1e-9, true).unwrap();
        assert_eq!(r.pieces.len(), 2);
        let total: f64 = r.pieces.iter().map(|p| p.signed_volume()).sum();
        assert!(close(total, 1.0), "{total}");
        assert!(r.pieces.iter().all(balanced));
    }

    #[test]
    fn polyline_project_path_that_leaves_the_mesh_is_an_error() {
        let c = box_mesh(1.0, 1.0, 1.0);
        let far = [DVec3::new(0.5, 0.0, 0.5), DVec3::new(0.5, 5.0, 0.5)];
        assert!(matches!(
            project_polyline(&c, &far, false, 0.01),
            Err(PathError::FarFromMesh { index: 1, .. })
        ));
    }

    #[test]
    fn polyline_project_walk_hits_boundary_of_open_mesh() {
        // L-shaped strip: straight line between the ends passes outside the strip.
        let v = vec![
            DVec3::new(0.0, 0.0, 0.0), DVec3::new(1.0, 0.0, 0.0), DVec3::new(1.0, 1.0, 0.0),
            DVec3::new(0.0, 1.0, 0.0), DVec3::new(3.0, 0.0, 0.0), DVec3::new(3.0, 1.0, 0.0),
            DVec3::new(1.0, 3.0, 0.0), DVec3::new(0.0, 3.0, 0.0),
        ];
        let tris = vec![[0, 1, 2], [0, 2, 3], [1, 4, 5], [1, 5, 2], [3, 2, 6], [3, 6, 7]];
        let m = TriMesh::new(v, tris);
        let e = project_polyline(&m, &[DVec3::new(2.5, 0.5, 0.0), DVec3::new(0.5, 2.5, 0.0)], false, 1e-9);
        assert!(matches!(e, Err(PathError::LeavesMesh { .. }) | Err(PathError::WalkFailed { .. })), "{e:?}");
    }

    #[test]
    fn polyline_project_open_path_that_does_not_separate_is_rejected() {
        let c = box_mesh(1.0, 1.0, 1.0);
        let pts = [DVec3::new(0.3, 0.0, 0.2), DVec3::new(0.7, 0.0, 0.8)];
        assert_eq!(cut_along_polyline(&c, &pts, false, 1e-9, true).unwrap_err(), PathError::NotSeparating);
    }

    #[test]
    fn polyline_project_rejects_bad_input() {
        let c = box_mesh(1.0, 1.0, 1.0);
        assert_eq!(project_polyline(&c, &[DVec3::ZERO], false, 1.0).unwrap_err(), PathError::TooFewPoints);
        assert_eq!(
            project_polyline(&c, &[DVec3::ZERO, DVec3::new(f64::NAN, 0.0, 0.0)], false, 1.0).unwrap_err(),
            PathError::NonFinite
        );
        assert_eq!(
            project_polyline(&TriMesh::default(), &[DVec3::ZERO, DVec3::X], false, 1.0).unwrap_err(),
            PathError::EmptyMesh
        );
    }
}

//! Joint features between capped halves: plug and socket, dowel pockets with a
//! loose pin, dovetail key and groove. Features are built analytically in the
//! plane of the cap and merged by *cap-hole construction*: the feature outline
//! is cut out of the cap polygon and the walls and floor (pocket) or walls and
//! top (protrusion) are added, so no mesh boolean is needed and the shells stay
//! closed and 2-manifold. A feature is only placed when it fits inside both
//! caps with a wall margin and, for pockets, when the body is deep enough.

use std::collections::{BTreeMap, HashMap, HashSet};

use glam::{DVec2, DVec3};
use vernon_core::{AttribType, Geometry, Prim};

use crate::cut::chain_loops;
use crate::mesh::{self, CAP, PART};
use crate::poly;

/// Prim Int attribute marking joint feature faces (feature index + 1).
pub const JOINT: &str = "joint";

#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub origin: DVec3,
    pub u: DVec3,
    pub v: DVec3,
    pub n: DVec3,
}

impl Frame {
    pub fn new(origin: DVec3, n: DVec3) -> Frame {
        let (u, v) = mesh::plane_frame(n);
        Frame { origin, u, v, n }
    }

    pub fn p2(&self, p: DVec3) -> DVec2 {
        let d = p - self.origin;
        DVec2::new(d.dot(self.u), d.dot(self.v))
    }

    pub fn p3(&self, q: DVec2) -> DVec3 {
        self.origin + self.u * q.x + self.v * q.y
    }
}

/// The cap faces of one part on one cut plane.
#[derive(Debug, Clone)]
pub struct CapRegion {
    pub part: i64,
    pub cap_id: i64,
    pub prims: Vec<usize>,
    /// Outward normal of the part at the cap.
    pub normal: DVec3,
    pub origin: DVec3,
    /// Boundary loops as point ids.
    pub loops: Vec<Vec<u32>>,
}

/// Find the cap regions (prims with `cap > 0`) of every part.
pub fn find_caps(geo: &Geometry) -> Vec<CapRegion> {
    let caps = match geo.prim_attribs.ints(CAP) {
        Ok(v) => v.to_vec(),
        Err(_) => return Vec::new(),
    };
    let parts = mesh::part_ids(geo);
    let mut groups: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    for (i, pr) in geo.prims.iter().enumerate() {
        if pr.closed && caps[i] > 0 {
            groups.entry((parts[i], caps[i])).or_default().push(i);
        }
    }
    let mut out = Vec::new();
    for ((part, cap_id), prims) in groups {
        let mut n = DVec3::ZERO;
        for &pi in &prims {
            n += geo.prim_normal(pi) * geo.prim_area(pi);
        }
        let n = n.normalize_or_zero();
        if n == DVec3::ZERO {
            continue;
        }
        let origin = geo.points[geo.prims[prims[0]].vertices[0] as usize];
        let frame = Frame::new(origin, n);
        let mut counts: HashMap<(u32, u32), u32> = HashMap::new();
        for &pi in &prims {
            let v = &geo.prims[pi].vertices;
            for k in 0..v.len() {
                let (a, b) = (v[k], v[(k + 1) % v.len()]);
                *counts.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        let mut edges: Vec<(u32, u32)> = counts.into_iter().filter(|(_, c)| *c == 1).map(|(e, _)| e).collect();
        edges.sort_unstable();
        let loops = match chain_loops(&edges, &|i| frame.p2(geo.points[i as usize])) {
            Ok(l) => l,
            Err(_) => continue,
        };
        out.push(CapRegion { part, cap_id, prims, normal: n, origin, loops });
    }
    out
}

// ---------------------------------------------------------------- shapes

/// A prism standing on the cap: `base` lies in the plane, `top` is the same
/// outline (same vertex count, corresponding edges) displaced by `depth`.
/// Both are counter-clockwise about the reference normal.
#[derive(Debug, Clone)]
pub struct Prism2 {
    pub base: Vec<DVec2>,
    pub top: Vec<DVec2>,
    pub depth: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Grows out of the cap (plug, tail).
    Protrude,
    /// Cut into the body (socket, groove, dowel hole).
    Pocket,
}

/// Regular polygon with vertices on the circle of radius `r`.
pub fn circle_prism(c: DVec2, r: f64, depth: f64, sides: usize) -> Prism2 {
    let ring: Vec<DVec2> = (0..sides)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / sides as f64;
            c + DVec2::new(a.cos(), a.sin()) * r
        })
        .collect();
    Prism2 { base: ring.clone(), top: ring, depth }
}

/// Trapezoid prism: a rectangle of half-width `half_w0` at the cap that grows
/// to `half_w1` at `depth`, `half_len` long each way along `dir`.
pub fn trapezoid_prism(c: DVec2, dir: DVec2, half_w0: f64, half_w1: f64, half_len: f64, depth: f64) -> Prism2 {
    let q = dir.normalize();
    let s = DVec2::new(-q.y, q.x);
    let corners = |hw: f64| {
        vec![
            c + s * (-hw) + q * (-half_len),
            c + s * hw + q * (-half_len),
            c + s * hw + q * half_len,
            c + s * (-hw) + q * half_len,
        ]
    };
    let (mut base, mut top) = (corners(half_w0), corners(half_w1));
    if poly::signed_area(&base) < 0.0 {
        base.reverse();
        top.reverse();
    }
    Prism2 { base, top, depth }
}

/// Free-end width of a dovetail with neck `w`, depth `d` and flap angle `a_deg`.
pub fn dovetail_free_width(w: f64, d: f64, a_deg: f64) -> f64 {
    w + 2.0 * d * a_deg.to_radians().tan()
}

/// What one joint adds to the two parts of a seam.
#[derive(Debug, Clone)]
pub struct JointShape {
    pub a: Prism2,
    pub a_kind: Kind,
    pub b: Prism2,
    pub b_kind: Kind,
    /// Loose pin (radius, length) centred on the plane.
    pub pin: Option<(f64, f64)>,
}

pub struct JointSpec {
    pub shape: Box<dyn Fn(DVec2) -> JointShape>,
    pub spacing: f64,
    pub wall_min: f64,
    pub male_lower: bool,
    /// 0 = as many as the spacing allows.
    pub max_count: usize,
    pub pin_sides: usize,
}

pub struct JointOutcome {
    pub geo: Geometry,
    pub pins: Geometry,
    pub joints: usize,
    pub pairs: usize,
    pub skipped: usize,
}

/// Closed cylinder along `axis` centred at `center` (outward winding).
pub fn cylinder_mesh(center: DVec3, axis: DVec3, r: f64, len: f64, sides: usize) -> Geometry {
    let axis = axis.normalize();
    let (u, v) = mesh::plane_frame(axis);
    let mut g = Geometry::new();
    let ring = |h: f64| -> Vec<DVec3> {
        (0..sides)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / sides as f64;
                center + axis * h + (u * a.cos() + v * a.sin()) * r
            })
            .collect()
    };
    let b: Vec<u32> = ring(-len / 2.0).into_iter().map(|p| g.add_point(p)).collect();
    let t: Vec<u32> = ring(len / 2.0).into_iter().map(|p| g.add_point(p)).collect();
    for i in 0..sides {
        let j = (i + 1) % sides;
        g.add_prim(Prim::polygon(vec![b[i], b[j], t[j]]));
        g.add_prim(Prim::polygon(vec![b[i], t[j], t[i]]));
    }
    for i in 1..sides - 1 {
        g.add_prim(Prim::polygon(vec![t[0], t[i], t[i + 1]]));
        g.add_prim(Prim::polygon(vec![b[0], b[i + 1], b[i]]));
    }
    g
}

fn outline_samples(shape: &JointShape) -> Vec<DVec2> {
    let mut out = Vec::new();
    for ring in [&shape.a.base, &shape.a.top, &shape.b.base, &shape.b.top] {
        let n = ring.len();
        for i in 0..n {
            let (p, q) = (ring[i], ring[(i + 1) % n]);
            let pieces = ((q - p).length() / 2.0).ceil().max(1.0) as usize;
            for k in 0..pieces {
                out.push(p + (q - p) * (k as f64 / pieces as f64));
            }
        }
    }
    out
}

// -------------------------------------------------------------- placement

struct Cand {
    c: DVec2,
    clearance: f64,
}

fn candidates(
    rings_a: &[Vec<DVec2>],
    rings_b: &[Vec<DVec2>],
    spacing: f64,
    wall: f64,
    shape: &dyn Fn(DVec2) -> JointShape,
) -> Vec<Cand> {
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for r in rings_a {
        for &p in r {
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    if lo.x > hi.x {
        return Vec::new();
    }
    let valid = |c: DVec2| -> Option<Cand> {
        if !poly::inside_rings(c, rings_a) || !poly::inside_rings(c, rings_b) {
            return None;
        }
        for q in outline_samples(&shape(c)) {
            if !poly::inside_rings(q, rings_a) || !poly::inside_rings(q, rings_b) {
                return None;
            }
            if poly::dist_to_rings(q, rings_a) < wall || poly::dist_to_rings(q, rings_b) < wall {
                return None;
            }
        }
        let clearance = poly::dist_to_rings(c, rings_a).min(poly::dist_to_rings(c, rings_b));
        Some(Cand { c, clearance })
    };
    let grid = |step: f64, cap_n: usize| -> Vec<Cand> {
        let ext = hi - lo;
        let nx = ((ext.x / step).floor() as usize + 1).min(cap_n);
        let ny = ((ext.y / step).floor() as usize + 1).min(cap_n);
        let mid = (lo + hi) * 0.5;
        let mut v = Vec::new();
        for i in 0..nx {
            for j in 0..ny {
                let c = mid + DVec2::new((i as f64 - (nx - 1) as f64 / 2.0) * step, (j as f64 - (ny - 1) as f64 / 2.0) * step);
                if let Some(cand) = valid(c) {
                    v.push(cand);
                }
            }
        }
        v
    };
    let mut v = grid(spacing.max(1.0), 60);
    if v.is_empty() {
        // Nothing on the coarse lattice: probe finely and take the roomiest spot.
        v = grid((spacing / 8.0).max(0.5), 60);
        v.sort_by(|a, b| b.clearance.partial_cmp(&a.clearance).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(1);
        return v;
    }
    v.sort_by(|a, b| {
        b.clearance
            .partial_cmp(&a.clearance)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.c.x.partial_cmp(&b.c.x).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.c.y.partial_cmp(&b.c.y).unwrap_or(std::cmp::Ordering::Equal))
    });
    v
}

fn host_triangles(geo: &Geometry, region: &CapRegion) -> Vec<[DVec3; 3]> {
    let parts = mesh::part_ids(geo);
    let skip: HashSet<usize> = region.prims.iter().copied().collect();
    geo.triangles()
        .into_iter()
        .filter(|(pi, _)| parts[*pi] == region.part && !skip.contains(pi))
        .map(|(_, t)| [geo.points[t[0] as usize], geo.points[t[1] as usize], geo.points[t[2] as usize]])
        .collect()
}

/// Smallest distance from the sample points along `dir` (into the body) to the part surface.
fn depth_clear(tris: &[[DVec3; 3]], pts: &[DVec3], dir: DVec3, need: f64) -> bool {
    for &p in pts {
        let mut hit = f64::INFINITY;
        for t in tris {
            if let Some(d) = mesh::ray_triangle(p, dir, t[0], t[1], t[2]) {
                hit = hit.min(d);
            }
        }
        if hit < need {
            return false;
        }
    }
    true
}

struct Add {
    /// Base ring in 3D, counter-clockwise about the host's outward normal.
    base: Vec<DVec3>,
    /// End outline in the cap plane (same vertex count as `base`); it is displaced by `top_off`.
    top: Vec<DVec3>,
    top_off: DVec3,
    tag: i64,
}

fn push_tri(g: &mut Geometry, ids: [u32; 3], part: i64, cap: i64, joint: i64) {
    let k = g.add_prim(Prim::polygon(ids.to_vec())) as usize;
    if let Some(vernon_core::AttribArray::Int(c)) = g.prim_attribs.get_mut(PART) {
        c[k] = part;
    }
    if let Some(vernon_core::AttribArray::Int(c)) = g.prim_attribs.get_mut(CAP) {
        c[k] = cap;
    }
    if let Some(vernon_core::AttribArray::Int(c)) = g.prim_attribs.get_mut(JOINT) {
        c[k] = joint;
    }
}

/// Add the walls and the end face of a prism; returns the base point ids.
fn add_prism(g: &mut Geometry, a: &Add, part: i64) -> Vec<u32> {
    let n = a.base.len();
    let b: Vec<u32> = a.base.iter().map(|&p| g.add_point(p)).collect();
    let t: Vec<u32> = a.top.iter().map(|&p| g.add_point(p + a.top_off)).collect();
    for i in 0..n {
        let j = (i + 1) % n;
        push_tri(g, [b[i], b[j], t[j]], part, 0, a.tag);
        push_tri(g, [b[i], t[j], t[i]], part, 0, a.tag);
    }
    for i in 1..n - 1 {
        push_tri(g, [t[0], t[i], t[i + 1]], part, 0, a.tag);
    }
    b
}

/// Place joints on every matching pair of caps and rebuild the affected caps.
pub fn apply_joints(geo: &Geometry, spec: &JointSpec) -> JointOutcome {
    let regions = find_caps(geo);
    let mut by_cap: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
    for (i, r) in regions.iter().enumerate() {
        by_cap.entry(r.cap_id).or_default().push(i);
    }
    let mut adds: HashMap<usize, Vec<Add>> = HashMap::new();
    let mut tri_cache: HashMap<usize, Vec<[DVec3; 3]>> = HashMap::new();
    let mut pins = Geometry::new();
    let (mut joints, mut pairs, mut skipped) = (0usize, 0usize, 0usize);
    let mut tag = 0i64;
    for ids in by_cap.values() {
        let n0 = regions[ids[0]].normal;
        let (s1, s2): (Vec<usize>, Vec<usize>) = ids.iter().copied().partition(|&i| regions[i].normal.dot(n0) > 0.0);
        for &i in &s1 {
            for &j in &s2 {
                let (ri, rj) = (&regions[i], &regions[j]);
                let i_is_lower = ri.part < rj.part;
                let (ia, ib) = if i_is_lower == spec.male_lower { (i, j) } else { (j, i) };
                let (ra, rb) = (&regions[ia], &regions[ib]);
                let frame = Frame::new(ra.origin, ra.normal);
                let rings = |r: &CapRegion| -> Vec<Vec<DVec2>> {
                    r.loops.iter().map(|l| l.iter().map(|&p| frame.p2(geo.points[p as usize])).collect()).collect()
                };
                let (rings_a, rings_b) = (rings(ra), rings(rb));
                pairs += 1;
                let cands = candidates(&rings_a, &rings_b, spec.spacing, spec.wall_min, &*spec.shape);
                let mut accepted: Vec<(DVec2, f64)> = Vec::new();
                for cand in cands {
                    if spec.max_count > 0 && accepted.len() >= spec.max_count {
                        break;
                    }
                    let shape = (spec.shape)(cand.c);
                    let reach = outline_samples(&shape).iter().map(|q| (*q - cand.c).length()).fold(0.0, f64::max);
                    if accepted.iter().any(|(c, r)| (*c - cand.c).length() < reach + r + spec.wall_min) {
                        skipped += 1;
                        continue;
                    }
                    // Pockets need body depth behind them.
                    let mut ok = true;
                    for (reg, idx, prism, kind, dir) in [
                        (ra, ia, &shape.a, shape.a_kind, -ra.normal),
                        (rb, ib, &shape.b, shape.b_kind, -rb.normal),
                    ] {
                        if kind == Kind::Pocket {
                            let tris = tri_cache.entry(idx).or_insert_with(|| host_triangles(geo, reg));
                            let pts: Vec<DVec3> = prism.base.iter().chain(prism.top.iter()).map(|&q| frame.p3(q)).collect();
                            if !depth_clear(tris, &pts, dir, prism.depth + spec.wall_min) {
                                ok = false;
                            }
                        }
                    }
                    if !ok {
                        skipped += 1;
                        continue;
                    }
                    tag += 1;
                    for (idx, host_n, prism, kind, reverse) in [
                        (ia, ra.normal, &shape.a, shape.a_kind, false),
                        (ib, rb.normal, &shape.b, shape.b_kind, true),
                    ] {
                        let mut ring: Vec<DVec3> = prism.base.iter().map(|&q| frame.p3(q)).collect();
                        let mut top: Vec<DVec3> = prism.top.iter().map(|&q| frame.p3(q)).collect();
                        if reverse {
                            ring.reverse();
                            top.reverse();
                        }
                        let sign = if kind == Kind::Protrude { 1.0 } else { -1.0 };
                        adds.entry(idx).or_default().push(Add { base: ring, top, top_off: host_n * (sign * prism.depth), tag });
                    }
                    if let Some((r, len)) = shape.pin {
                        let mut pin = cylinder_mesh(frame.p3(cand.c), ra.normal, r, len, spec.pin_sides.max(8));
                        let pin_id = joints as i64;
                        let np = pin.prims.len();
                        mesh::set_int_prim_attr(&mut pin, PART, vec![pin_id; np]);
                        pins.merge(&pin);
                    }
                    accepted.push((cand.c, reach));
                    joints += 1;
                }
            }
        }
    }
    // Rebuild: drop the old cap faces of modified regions, add new caps and features.
    let remove: HashSet<usize> = adds.keys().flat_map(|&i| regions[i].prims.iter().copied()).collect();
    let keep: Vec<usize> = (0..geo.prims.len()).filter(|i| !remove.contains(i)).collect();
    let mut g = Geometry {
        points: geo.points.clone(),
        point_attribs: geo.point_attribs.clone(),
        prims: keep.iter().map(|&i| geo.prims[i].clone()).collect(),
        prim_attribs: geo.prim_attribs.gather_all(&keep),
        ..Default::default()
    };
    let n = g.prims.len();
    for name in [PART, CAP, JOINT] {
        let _ = g.prim_attribs.ensure(name, AttribType::Int, n);
    }
    let mut order: Vec<usize> = adds.keys().copied().collect();
    order.sort_unstable();
    for idx in order {
        let region = &regions[idx];
        let frame = Frame::new(region.origin, region.normal);
        let mut rings: Vec<Vec<DVec2>> = region
            .loops
            .iter()
            .map(|l| l.iter().map(|&p| frame.p2(geo.points[p as usize])).collect())
            .collect();
        let mut flat: Vec<u32> = region.loops.iter().flatten().copied().collect();
        for add in &adds[&idx] {
            let base_ids = add_prism(&mut g, add, region.part);
            rings.push(add.base.iter().map(|&p| frame.p2(p)).collect());
            flat.extend(base_ids);
        }
        let tri = poly::triangulate_rings(&rings);
        for t in tri.tris {
            push_tri(&mut g, [flat[t[0]], flat[t[1]], flat[t[2]]], region.part, region.cap_id, 0);
        }
    }
    JointOutcome { geo: g, pins, joints, pairs, skipped }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cut::{cut_all, Keep, Plane};
    use crate::mesh::testutil::tri_box;
    use std::f64::consts::PI;

    fn halves(z: f64) -> Geometry {
        let cube = tri_box(DVec3::ZERO, DVec3::splat(100.0));
        let plane = Plane::new(DVec3::new(0.0, 0.0, z), DVec3::Z).unwrap();
        cut_all(&cube, &[plane], 1e-6, true, Keep::Both).unwrap().geo
    }

    fn part_volume(g: &Geometry, p: i64) -> f64 {
        mesh::signed_volume(&mesh::extract_part(g, p))
    }

    fn polygon_area(r: f64, n: usize) -> f64 {
        0.5 * n as f64 * r * r * (std::f64::consts::TAU / n as f64).sin()
    }

    fn plug_spec(r: f64, depth: f64, tol: f64, gap: f64, male_lower: bool) -> JointSpec {
        JointSpec {
            shape: Box::new(move |c| JointShape {
                a: circle_prism(c, r - tol, depth, 64),
                a_kind: Kind::Protrude,
                b: circle_prism(c, r, depth + gap, 64),
                b_kind: Kind::Pocket,
                pin: None,
            }),
            spacing: 40.0,
            wall_min: 1.5,
            male_lower,
            max_count: 0,
            pin_sides: 64,
        }
    }

    #[test]
    fn plug_pair_geometry_and_volumes() {
        let g = halves(50.0);
        let (v0, v1) = (part_volume(&g, 0), part_volume(&g, 1));
        let out = apply_joints(&g, &plug_spec(4.0, 6.0, 0.15, 0.3, true));
        assert_eq!(out.pairs, 1);
        assert!(out.joints >= 5, "joints {}", out.joints);
        let n = out.joints as f64;
        // male is the lower part (id 0): volume grows by the plug prisms
        let dv0 = part_volume(&out.geo, 0) - v0;
        let dv1 = part_volume(&out.geo, 1) - v1;
        assert!((dv0 - n * polygon_area(3.85, 64) * 6.0).abs() < 1e-6, "{dv0}");
        assert!((dv1 + n * polygon_area(4.0, 64) * 6.3).abs() < 1e-6, "{dv1}");
        // male volume within 0.2 % of the circle formula
        let exact = PI * 3.85 * 3.85 * 6.0;
        assert!((polygon_area(3.85, 64) * 6.0 - exact).abs() / exact < 0.002);
        for p in 0..2 {
            let part = mesh::extract_part(&out.geo, p);
            let s = mesh::edge_stats(&part);
            assert_eq!((s.open, s.non_manifold), (0, 0), "part {p}");
        }
        // protruding plug radius measured on the mesh
        let male = mesh::extract_part(&out.geo, 0);
        let tops: Vec<DVec3> = male
            .points
            .iter()
            .copied()
            .filter(|p| (p.z - 56.0).abs() < 1e-9 && (p.x - 50.0).abs() < 10.0 && (p.y - 50.0).abs() < 10.0)
            .collect();
        let c = tops.iter().copied().sum::<DVec3>() / tops.len() as f64;
        let r = tops.iter().map(|p| ((p.x - c.x).powi(2) + (p.y - c.y).powi(2)).sqrt()).fold(0.0, f64::max);
        assert_eq!(tops.len(), 64);
        assert!((r - 3.85).abs() < 1e-9);
    }

    #[test]
    fn thin_part_refuses_pocket() {
        let g = halves(5.0); // lower half only 5 mm thick
        let out = apply_joints(&g, &plug_spec(4.0, 6.0, 0.15, 0.3, false)); // pockets go into the thin part
        assert_eq!(out.joints, 0);
        assert!(out.skipped > 0);
        assert_eq!(mesh::open_edges_by_part(&out.geo), 0);
    }

    #[test]
    fn dowel_pockets_both_sides_with_pin() {
        let g = halves(50.0);
        let (v0, v1) = (part_volume(&g, 0), part_volume(&g, 1));
        let spec = JointSpec {
            shape: Box::new(|c| JointShape {
                a: circle_prism(c, 4.0, 8.0, 64),
                a_kind: Kind::Pocket,
                b: circle_prism(c, 4.0, 8.0, 64),
                b_kind: Kind::Pocket,
                pin: Some((3.85, 15.7)),
            }),
            spacing: 40.0,
            wall_min: 1.5,
            male_lower: true,
            max_count: 0,
            pin_sides: 64,
        };
        let out = apply_joints(&g, &spec);
        let n = out.joints as f64;
        assert!(n >= 5.0);
        let pocket = polygon_area(4.0, 64) * 8.0;
        assert!((part_volume(&out.geo, 0) - (v0 - n * pocket)).abs() < 1e-6);
        assert!((part_volume(&out.geo, 1) - (v1 - n * pocket)).abs() < 1e-6);
        assert_eq!(mesh::open_edges_by_part(&out.geo), 0);
        let pin_v = mesh::signed_volume(&out.pins);
        assert!((pin_v - n * polygon_area(3.85, 64) * 15.7).abs() < 1e-6);
        assert_eq!(mesh::unique_parts(&out.pins).len(), out.joints);
    }

    #[test]
    fn dovetail_free_width_and_clearance() {
        assert!((dovetail_free_width(12.0, 6.0, 15.0) - 15.2154).abs() < 1e-3);
        let (w, d, a, t, len) = (12.0, 6.0, 15.0f64, 0.15, 20.0);
        let ta = a.to_radians().tan();
        let w_free = dovetail_free_width(w, d, a);
        let tail = trapezoid_prism(DVec2::ZERO, DVec2::Y, w / 2.0, w_free / 2.0, len / 2.0, d);
        let g0 = a.to_radians().cos();
        let hw0 = w / 2.0 + t / g0;
        let groove = trapezoid_prism(DVec2::ZERO, DVec2::Y, hw0, hw0 + (d + t) * ta, len / 2.0 + t, d + t);
        // perpendicular clearance between the inclined side walls = tolerance
        // wall line of the tail through (w/2, 0) with direction (tan a, 1)
        let p_groove = DVec2::new(hw0, 0.0); // groove wall point at the plane
        let p_tail = DVec2::new(w / 2.0, 0.0);
        let dir = DVec2::new(ta, 1.0).normalize();
        let nrm = DVec2::new(dir.y, -dir.x);
        assert!(((p_groove - p_tail).dot(nrm) - t).abs() < 1e-12);
        // tail area check: trapezoid area 81.646 per unit length
        let area = (w + w_free) / 2.0 * d;
        assert!((area - 81.646).abs() < 1e-3);
        let _ = (tail, groove);
    }

    #[test]
    fn dovetail_joint_volumes_and_manifold() {
        let g = halves(50.0);
        let (w, d, a, t, len) = (12.0, 6.0, 15.0f64, 0.15, 20.0);
        let ta = a.to_radians().tan();
        let w_free = dovetail_free_width(w, d, a);
        let spec = JointSpec {
            shape: Box::new(move |c| {
                let hw0 = w / 2.0 + t / a.to_radians().cos();
                JointShape {
                    a: trapezoid_prism(c, DVec2::Y, w / 2.0, w_free / 2.0, len / 2.0, d),
                    a_kind: Kind::Protrude,
                    b: trapezoid_prism(c, DVec2::Y, hw0, hw0 + (d + t) * ta, len / 2.0 + t, d + t),
                    b_kind: Kind::Pocket,
                    pin: None,
                }
            }),
            spacing: 40.0,
            wall_min: 1.5,
            male_lower: true,
            max_count: 1,
            pin_sides: 64,
        };
        let (v0, v1) = (part_volume(&g, 0), part_volume(&g, 1));
        let out = apply_joints(&g, &spec);
        assert_eq!(out.joints, 1);
        let tail_v = (w + w_free) / 2.0 * d * len;
        assert!((part_volume(&out.geo, 0) - v0 - tail_v).abs() < 1e-6, "dv {} tail {}", part_volume(&out.geo, 0) - v0, tail_v);
        let hw0 = w / 2.0 + t / a.to_radians().cos();
        let groove_v = (2.0 * hw0 + 2.0 * hw0 + 2.0 * (d + t) * ta) / 2.0 * (d + t) * (len + 2.0 * t);
        assert!((part_volume(&out.geo, 1) - v1 + groove_v).abs() < 1e-6);
        assert_eq!(mesh::open_edges_by_part(&out.geo), 0);
    }

    #[test]
    fn cylinder_mesh_is_closed_and_outward() {
        let c = cylinder_mesh(DVec3::new(1.0, 2.0, 3.0), DVec3::new(1.0, 1.0, 0.0), 2.0, 10.0, 64);
        assert_eq!(mesh::edge_stats(&c).open, 0);
        assert!((mesh::signed_volume(&c) - polygon_area(2.0, 64) * 10.0).abs() < 1e-9);
    }
}

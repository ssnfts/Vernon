//! Planar cutting of closed triangle meshes with capping (spec A1).
//!
//! Crossing points are created once per edge, so both neighbours share them
//! and each half stays 2-manifold. Cap loops are chained from the open
//! on-plane edges of each half, nested by point-in-polygon and filled by
//! ear clipping with hole bridging.

use std::collections::{BTreeMap, HashMap};

use glam::{DVec2, DVec3};
use vernon_core::{AttribArray, AttribType, Geometry, Prim};

use crate::mesh::{self, CAP, PART};
use crate::poly;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub origin: DVec3,
    /// Unit normal.
    pub normal: DVec3,
}

impl Plane {
    /// `None` for non-finite input or a (near) zero normal.
    pub fn new(origin: DVec3, normal: DVec3) -> Option<Plane> {
        if !origin.is_finite() || !normal.is_finite() || normal.length() < 1e-12 {
            return None;
        }
        Some(Plane { origin, normal: normal.normalize() })
    }

    pub fn dist(&self, p: DVec3) -> f64 {
        self.normal.dot(p - self.origin)
    }
}

/// Error from the cutter: a stable code plus a message.
#[derive(Debug, Clone, PartialEq)]
pub struct CutError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for CutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

pub struct CutResult {
    pub lower: Geometry,
    pub upper: Geometry,
    /// Net area of the cap polygons (one side).
    pub cap_area: f64,
    pub loops: usize,
    /// Open edges that do not lie on the cut plane (input was not closed there).
    pub foreign_open_edges: usize,
}

#[derive(Clone, Copy)]
enum Src {
    Orig(u32),
    /// `lo + t * (hi - lo)` for the edge (lo, hi), lo < hi.
    Cross(u32, u32, f64),
}

#[derive(Default)]
struct Side {
    pts: Vec<DVec3>,
    src: Vec<Src>,
    on_plane: Vec<bool>,
    remap: HashMap<u32, u32>,
    cross: HashMap<(u32, u32), u32>,
    tris: Vec<[u32; 3]>,
    tri_src: Vec<usize>,
}

/// A polygon corner while clipping: either an original vertex or a crossing.
#[derive(Clone, Copy)]
enum Corner {
    Orig(u32),
    Cross(u32, u32),
}

impl Side {
    fn corner(&mut self, c: Corner, geo: &Geometry, s: &HashMap<u32, f64>) -> u32 {
        match c {
            Corner::Orig(v) => {
                if let Some(&i) = self.remap.get(&v) {
                    return i;
                }
                let i = self.pts.len() as u32;
                self.pts.push(geo.points[v as usize]);
                self.src.push(Src::Orig(v));
                self.on_plane.push(s[&v] == 0.0);
                self.remap.insert(v, i);
                i
            }
            Corner::Cross(lo, hi) => {
                if let Some(&i) = self.cross.get(&(lo, hi)) {
                    return i;
                }
                let (sl, sh) = (s[&lo], s[&hi]);
                let t = sl / (sl - sh);
                let (pl, ph) = (geo.points[lo as usize], geo.points[hi as usize]);
                let i = self.pts.len() as u32;
                self.pts.push(pl + (ph - pl) * t);
                self.src.push(Src::Cross(lo, hi, t));
                self.on_plane.push(true);
                self.cross.insert((lo, hi), i);
                i
            }
        }
    }

    fn build(&self, geo: &Geometry) -> Geometry {
        let mut g = Geometry::new();
        g.points = self.pts.clone();
        for (name, col) in geo.point_attribs.iter() {
            let arr = match col {
                AttribArray::Float(v) => AttribArray::Float(
                    self.src
                        .iter()
                        .map(|s| match *s {
                            Src::Orig(i) => v[i as usize],
                            Src::Cross(a, b, t) => v[a as usize] + t * (v[b as usize] - v[a as usize]),
                        })
                        .collect(),
                ),
                AttribArray::Vec3(v) => AttribArray::Vec3(
                    self.src
                        .iter()
                        .map(|s| match *s {
                            Src::Orig(i) => v[i as usize],
                            Src::Cross(a, b, t) => {
                                let x = v[a as usize] + (v[b as usize] - v[a as usize]) * t;
                                if name == "N" {
                                    x.normalize_or_zero()
                                } else {
                                    x
                                }
                            }
                        })
                        .collect(),
                ),
                AttribArray::Int(v) => AttribArray::Int(
                    self.src
                        .iter()
                        .map(|s| match *s {
                            Src::Orig(i) => v[i as usize],
                            Src::Cross(a, b, t) => v[if t < 0.5 { a } else { b } as usize],
                        })
                        .collect(),
                ),
                AttribArray::Str(v) => AttribArray::Str(
                    self.src
                        .iter()
                        .map(|s| match *s {
                            Src::Orig(i) => v[i as usize].clone(),
                            Src::Cross(a, b, t) => v[if t < 0.5 { a } else { b } as usize].clone(),
                        })
                        .collect(),
                ),
            };
            g.point_attribs.set(name, arr);
        }
        g.prims = self.tris.iter().map(|t| Prim::polygon(t.to_vec())).collect();
        g.prim_attribs = geo.prim_attribs.gather_all(&self.tri_src);
        let _ = g.prim_attribs.ensure(CAP, AttribType::Int, g.prims.len());
        g
    }
}

fn ccw_angle(d_in: DVec2, d_out: DVec2) -> f64 {
    d_in.perp_dot(d_out).atan2(d_in.dot(d_out))
}

/// Chain undirected edges into closed loops of vertex ids.
pub fn chain_loops(edges: &[(u32, u32)], pos2: &dyn Fn(u32) -> DVec2) -> Result<Vec<Vec<u32>>, CutError> {
    let mut adj: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, &(a, b)) in edges.iter().enumerate() {
        adj.entry(a).or_default().push(i);
        adj.entry(b).or_default().push(i);
    }
    if let Some((v, _)) = adj.iter().find(|(_, e)| e.len() % 2 == 1) {
        return Err(CutError {
            code: "E_OPEN_LOOP",
            message: format!("cut outline is open at point {v}: the input mesh is not closed across the cut plane"),
        });
    }
    let mut used = vec![false; edges.len()];
    let mut loops = Vec::new();
    for e0 in 0..edges.len() {
        if used[e0] {
            continue;
        }
        used[e0] = true;
        let (start, mut v) = edges[e0];
        let mut prev = start;
        let mut ring = vec![start];
        while v != start {
            ring.push(v);
            let cands: Vec<usize> = adj[&v].iter().copied().filter(|&e| !used[e]).collect();
            if cands.is_empty() {
                return Err(CutError {
                    code: "E_OPEN_LOOP",
                    message: "cut outline does not close".into(),
                });
            }
            let d_in = pos2(v) - pos2(prev);
            let best = cands
                .into_iter()
                .min_by(|&x, &y| {
                    let ox = if edges[x].0 == v { edges[x].1 } else { edges[x].0 };
                    let oy = if edges[y].0 == v { edges[y].1 } else { edges[y].0 };
                    let ax = ccw_angle(d_in, pos2(ox) - pos2(v));
                    let ay = ccw_angle(d_in, pos2(oy) - pos2(v));
                    ax.partial_cmp(&ay).unwrap_or(std::cmp::Ordering::Equal)
                })
                .expect("candidate exists");
            used[best] = true;
            let w = if edges[best].0 == v { edges[best].1 } else { edges[best].0 };
            prev = v;
            v = w;
        }
        loops.push(ring);
    }
    Ok(loops)
}

/// Cut one mesh by a plane. `cap` closes both halves with planar caps tagged
/// with `cap_id` in the prim Int attribute `cap`.
pub fn cut_mesh(geo: &Geometry, plane: &Plane, eps_rel: f64, cap: bool, cap_id: i64) -> Result<CutResult, CutError> {
    let bounds = geo.bounds();
    let diag = bounds.size().length().max(1e-9);
    let eps = (eps_rel * diag).max(1e-12);
    // Signed distances, snapped to zero near the plane.
    let mut s: HashMap<u32, f64> = HashMap::new();
    for (_, tri) in geo.triangles() {
        for v in tri {
            s.entry(v).or_insert_with(|| {
                let d = plane.dist(geo.points[v as usize]);
                if d.abs() < eps {
                    0.0
                } else {
                    d
                }
            });
        }
    }
    let mut lower = Side::default();
    let mut upper = Side::default();
    for (pi, tri) in geo.triangles() {
        let sv = [s[&tri[0]], s[&tri[1]], s[&tri[2]]];
        let any_pos = sv.iter().any(|&x| x > 0.0);
        let any_neg = sv.iter().any(|&x| x < 0.0);
        let push = |side: &mut Side, poly: &[Corner]| {
            if poly.len() < 3 {
                return;
            }
            let ids: Vec<u32> = poly.iter().map(|&c| side.corner(c, geo, &s)).collect();
            for k in 1..ids.len() - 1 {
                side.tris.push([ids[0], ids[k], ids[k + 1]]);
                side.tri_src.push(pi);
            }
        };
        let corners = [Corner::Orig(tri[0]), Corner::Orig(tri[1]), Corner::Orig(tri[2])];
        if !any_neg && any_pos {
            push(&mut upper, &corners);
        } else if !any_pos && any_neg {
            push(&mut lower, &corners);
        } else if !any_pos && !any_neg {
            // Face lying in the plane: it belongs to the side its outward normal points away from.
            let n = (geo.points[tri[1] as usize] - geo.points[tri[0] as usize])
                .cross(geo.points[tri[2] as usize] - geo.points[tri[0] as usize]);
            if n.dot(plane.normal) >= 0.0 {
                push(&mut lower, &corners);
            } else {
                push(&mut upper, &corners);
            }
        } else {
            let mut up: Vec<Corner> = Vec::new();
            let mut lo: Vec<Corner> = Vec::new();
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                let (sa, sb) = (sv[k], sv[(k + 1) % 3]);
                if sa >= 0.0 {
                    up.push(Corner::Orig(a));
                }
                if sa <= 0.0 {
                    lo.push(Corner::Orig(a));
                }
                if (sa > 0.0 && sb < 0.0) || (sa < 0.0 && sb > 0.0) {
                    let c = Corner::Cross(a.min(b), a.max(b));
                    up.push(c);
                    lo.push(c);
                }
            }
            push(&mut upper, &up);
            push(&mut lower, &lo);
        }
    }
    let mut lo_geo = lower.build(geo);
    let mut up_geo = upper.build(geo);
    let mut result = CutResult { lower: Geometry::new(), upper: Geometry::new(), cap_area: 0.0, loops: 0, foreign_open_edges: 0 };
    let (u, v) = mesh::plane_frame(plane.normal);
    for (side, g, flip) in [(&lower, &mut lo_geo, false), (&upper, &mut up_geo, true)] {
        // Open edges of this half.
        let mut counts: HashMap<(u32, u32), u32> = HashMap::new();
        for t in &side.tris {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if a != b {
                    *counts.entry((a.min(b), a.max(b))).or_insert(0) += 1;
                }
            }
        }
        let mut plane_edges: Vec<(u32, u32)> = Vec::new();
        let mut foreign = 0;
        for (&(a, b), &c) in &counts {
            if c == 1 {
                if side.on_plane[a as usize] && side.on_plane[b as usize] {
                    plane_edges.push((a, b));
                } else {
                    foreign += 1;
                }
            }
        }
        plane_edges.sort_unstable();
        result.foreign_open_edges = result.foreign_open_edges.max(foreign);
        if !cap || plane_edges.is_empty() {
            continue;
        }
        let pos2 = |i: u32| {
            let d = side.pts[i as usize] - plane.origin;
            DVec2::new(d.dot(u), d.dot(v))
        };
        let loops = chain_loops(&plane_edges, &pos2)?;
        let rings: Vec<Vec<DVec2>> = loops.iter().map(|l| l.iter().map(|&i| pos2(i)).collect()).collect();
        let flat: Vec<u32> = loops.iter().flatten().copied().collect();
        let tri = poly::triangulate_rings(&rings);
        for t in tri.tris {
            let mut ids = [flat[t[0]], flat[t[1]], flat[t[2]]];
            if flip {
                ids.swap(1, 2);
            }
            let k = g.add_prim(Prim::polygon(ids.to_vec()));
            if let Some(AttribArray::Int(c)) = g.prim_attribs.get_mut(CAP) {
                c[k as usize] = cap_id;
            }
        }
        if !flip {
            result.cap_area = tri.area;
            result.loops = loops.len();
        }
    }
    result.lower = lo_geo;
    result.upper = up_geo;
    Ok(result)
}

/// Area of the cross-section of a closed mesh at `plane` (shoelace over the
/// oriented crossing segments).
pub fn cross_section_area(geo: &Geometry, plane: &Plane) -> f64 {
    let (u, v) = mesh::plane_frame(plane.normal);
    let proj = |p: DVec3| {
        let d = p - plane.origin;
        DVec2::new(d.dot(u), d.dot(v))
    };
    let mut sum = 0.0;
    for (_, t) in geo.triangles() {
        let p = [geo.points[t[0] as usize], geo.points[t[1] as usize], geo.points[t[2] as usize]];
        let d = [plane.dist(p[0]), plane.dist(p[1]), plane.dist(p[2])];
        let mut down: Option<DVec2> = None;
        let mut up: Option<DVec2> = None;
        for k in 0..3 {
            let (a, b) = (k, (k + 1) % 3);
            if d[a] < 0.0 && d[b] >= 0.0 || d[a] >= 0.0 && d[b] < 0.0 {
                let tt = d[a] / (d[a] - d[b]);
                let x = proj(p[a] + (p[b] - p[a]) * tt);
                if d[a] < 0.0 {
                    up = Some(x);
                } else {
                    down = Some(x);
                }
            }
        }
        if let (Some(a), Some(b)) = (down, up) {
            sum += a.perp_dot(b);
        }
    }
    (sum * 0.5).abs()
}

/// One finished piece after cutting.
pub struct Cut {
    pub geo: Geometry,
    /// Side mask: bit k set when the piece lies on the positive side of plane k.
    pub mask: u64,
    /// Part id the piece came from before cutting.
    pub origin_part: i64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Keep {
    Both,
    Upper,
    Lower,
}

pub struct CutAll {
    /// All pieces merged; prim Int `part` (dense), `cell` (side mask), `cap`.
    pub geo: Geometry,
    pub cap_area: f64,
    pub loops: usize,
    pub foreign_open_edges: usize,
}

/// Cut every part of `geo` by the planes in order. Pieces that end up empty are dropped.
pub fn cut_all(geo: &Geometry, planes: &[Plane], eps_rel: f64, cap: bool, keep: Keep) -> Result<CutAll, CutError> {
    if planes.len() > 62 {
        return Err(CutError { code: "E_TOO_MANY_PLANES", message: "at most 62 planes per cut node".into() });
    }
    let mut pieces: Vec<Cut> = mesh::unique_parts(geo)
        .into_iter()
        .map(|p| Cut { geo: mesh::extract_part(geo, p), mask: 0, origin_part: p })
        .collect();
    let mut cap_area = 0.0;
    let mut loops = 0;
    let mut foreign = 0;
    for (k, plane) in planes.iter().enumerate() {
        let mut next = Vec::with_capacity(pieces.len() * 2);
        for piece in pieces {
            let r = cut_mesh(&piece.geo, plane, eps_rel, cap, k as i64 + 1)?;
            cap_area += r.cap_area;
            loops += r.loops;
            foreign += r.foreign_open_edges;
            if keep != Keep::Upper && !r.lower.prims.is_empty() {
                next.push(Cut { geo: r.lower, mask: piece.mask, origin_part: piece.origin_part });
            }
            if keep != Keep::Lower && !r.upper.prims.is_empty() {
                next.push(Cut { geo: r.upper, mask: piece.mask | (1 << k), origin_part: piece.origin_part });
            }
        }
        pieces = next;
    }
    pieces.sort_by_key(|p| (p.origin_part, p.mask));
    let mut out = Geometry::new();
    for (i, mut p) in pieces.into_iter().enumerate() {
        let n = p.geo.prims.len();
        mesh::set_int_prim_attr(&mut p.geo, PART, vec![i as i64; n]);
        mesh::set_int_prim_attr(&mut p.geo, "cell", vec![p.mask as i64; n]);
        out.merge(&p.geo);
    }
    Ok(CutAll { geo: out, cap_area, loops, foreign_open_edges: foreign })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::testutil::{tri_box, uv_sphere};

    fn z_plane(z: f64) -> Plane {
        Plane::new(DVec3::new(0.0, 0.0, z), DVec3::Z).unwrap()
    }

    #[test]
    fn cube_cut_golden_numbers() {
        let cube = tri_box(DVec3::ZERO, DVec3::splat(100.0));
        let r = cut_mesh(&cube, &z_plane(40.0), 1e-6, true, 1).unwrap();
        let (vl, vu) = (mesh::signed_volume(&r.lower), mesh::signed_volume(&r.upper));
        assert!((vl - 400_000.0).abs() / 400_000.0 < 1e-9, "lower {vl}");
        assert!((vu - 600_000.0).abs() / 600_000.0 < 1e-9, "upper {vu}");
        assert!(((vl + vu) - 1.0e6).abs() / 1.0e6 < 1e-9);
        assert!((r.cap_area - 10_000.0).abs() < 1e-6);
        assert_eq!(mesh::edge_stats(&r.lower).open, 0);
        assert_eq!(mesh::edge_stats(&r.upper).open, 0);
        assert_eq!(mesh::edge_stats(&r.lower).non_manifold, 0);
        // crossings lie on the plane
        for p in r.upper.points.iter().filter(|p| (p.z - 40.0).abs() < 1.0) {
            assert!((p.z - 40.0).abs() < 1e-6);
        }
    }

    #[test]
    fn uncapped_halves_conserve_area() {
        let cube = tri_box(DVec3::ZERO, DVec3::splat(100.0));
        let r = cut_mesh(&cube, &z_plane(33.3), 1e-6, false, 1).unwrap();
        let total = r.lower.surface_area() + r.upper.surface_area();
        assert!((total - cube.surface_area()).abs() / cube.surface_area() < 1e-9);
    }

    #[test]
    fn sphere_cut_matches_analytic_cap() {
        let s = uv_sphere(50.0, 128, 64);
        let r = cut_mesh(&s, &z_plane(20.0), 1e-6, true, 1).unwrap();
        let expect = std::f64::consts::PI * (2500.0 - 400.0);
        assert!((r.cap_area - expect).abs() / expect < 0.005, "cap {} vs {expect}", r.cap_area);
        let cap_vol = std::f64::consts::PI * 30.0 * 30.0 * (150.0 - 30.0) / 3.0;
        let v = mesh::signed_volume(&r.upper);
        assert!((v - cap_vol).abs() / cap_vol < 0.005, "volume {v} vs {cap_vol}");
        assert_eq!(mesh::edge_stats(&r.upper).open, 0);
        assert_eq!(mesh::edge_stats(&r.lower).open, 0);
    }

    #[test]
    fn plane_through_top_face_gives_one_part() {
        let cube = tri_box(DVec3::ZERO, DVec3::splat(100.0));
        let r = cut_mesh(&cube, &z_plane(100.0), 1e-6, true, 1).unwrap();
        assert!(r.upper.prims.is_empty());
        assert_eq!(r.lower.prims.len(), cube.prims.len());
        for (i, _) in r.lower.prims.iter().enumerate() {
            assert!(r.lower.prim_area(i) > 1e-9, "zero-area triangle");
        }
    }

    #[test]
    fn open_mesh_is_rejected() {
        let mut cube = tri_box(DVec3::ZERO, DVec3::splat(100.0));
        // delete the +X wall (points with x == 100 on every vertex of the triangle)
        let keep: Vec<usize> = (0..cube.prims.len())
            .filter(|&i| !cube.prims[i].vertices.iter().all(|&v| cube.points[v as usize].x == 100.0))
            .collect();
        cube = mesh::extract_prims(&cube, &keep);
        let e = cut_mesh(&cube, &z_plane(40.0), 1e-6, true, 1).err().expect("must fail");
        assert_eq!(e.code, "E_OPEN_LOOP");
    }

    #[test]
    fn two_islands_two_loops() {
        let mut g = tri_box(DVec3::ZERO, DVec3::splat(20.0));
        g.merge(&tri_box(DVec3::new(70.0, 0.0, 0.0), DVec3::splat(20.0)));
        let r = cut_mesh(&g, &z_plane(10.0), 1e-6, true, 1).unwrap();
        assert_eq!(r.loops, 2);
        assert!((r.cap_area - 800.0).abs() < 1e-6);
    }

    #[test]
    fn section_area_matches_cap() {
        let s = uv_sphere(50.0, 64, 32);
        let a = cross_section_area(&s, &z_plane(20.0));
        let r = cut_mesh(&s, &z_plane(20.0), 1e-6, true, 1).unwrap();
        assert!((a - r.cap_area).abs() < 1e-6);
    }

    #[test]
    fn sequential_cuts_give_four_parts_with_part_and_cell() {
        let cube = tri_box(DVec3::splat(-50.0), DVec3::splat(100.0));
        let planes = [
            Plane::new(DVec3::ZERO, DVec3::X).unwrap(),
            Plane::new(DVec3::ZERO, DVec3::Y).unwrap(),
        ];
        let r = cut_all(&cube, &planes, 1e-6, true, Keep::Both).unwrap();
        assert_eq!(mesh::unique_parts(&r.geo), vec![0, 1, 2, 3]);
        for p in 0..4 {
            let g = mesh::extract_part(&r.geo, p);
            assert_eq!(mesh::edge_stats(&g).open, 0);
            assert!((mesh::signed_volume(&g) - 250_000.0).abs() < 1e-3);
        }
        assert!((mesh::signed_volume(&r.geo) - 1.0e6).abs() < 1e-3);
        // ascending mask: part 3 is the +x,+y quadrant
        let cells = r.geo.prim_attribs.ints("cell").unwrap();
        let parts = r.geo.prim_attribs.ints(PART).unwrap();
        for i in 0..parts.len() {
            assert_eq!(parts[i], cells[i]);
        }
    }

    #[test]
    fn zero_normal_is_rejected() {
        assert!(Plane::new(DVec3::ZERO, DVec3::ZERO).is_none());
        assert!(Plane::new(DVec3::ZERO, DVec3::new(f64::NAN, 0.0, 1.0)).is_none());
    }
}

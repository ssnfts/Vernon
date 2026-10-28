//! Piecewise-affine projected-polyline cutting. Each monotone guide segment
//! defines a planar strip; refining at strip boundaries makes the shear exact.

use glam::{DVec2, DVec3};
use std::collections::BTreeMap;
use vernon_core::{AttribArray, Geometry};

use crate::cut::{cut_mesh_absolute, Keep, Plane};
use crate::{mesh, nodes_split};

const MAX_GUIDE_POINTS: usize = 128;
const MAX_TRIANGLES: usize = 1_000_000;

pub struct CurtainResult {
    pub geo: Geometry,
    pub segments: usize,
    pub cap_area: f64,
    pub loops: usize,
    pub cap_start: i64,
}

struct Guide {
    origin: DVec3,
    u: DVec3,
    v: DVec3,
    knots: Vec<DVec2>,
}

struct Tolerances {
    clip: f64,
    refine: f64,
    weld: f64,
    boundary: f64,
}

impl Guide {
    fn new(points: &[DVec3], view: DVec3) -> Result<Self, String> {
        if points.len() < 2 || points.len() > MAX_GUIDE_POINTS {
            return Err(format!("guide needs 2 to {MAX_GUIDE_POINTS} points"));
        }
        if !view.is_finite() || view.length() < 1e-12
            || points.iter().any(|p| !p.is_finite() || p.abs().max_element() > 1e12)
        {
            return Err("guide coordinates must be finite and at most 1e12 mm in magnitude, with a finite nonzero view direction".into());
        }
        let d = (view / view.abs().max_element()).normalize();
        let origin = points[0];
        let delta = points[points.len() - 1] - origin;
        // Two cross products avoid normalizing the parallel cancellation
        // residual of delta - d * dot(delta, d) into a bogus in-plane axis.
        let projected = d.cross(delta);
        let resolution = (64.0 * f64::EPSILON * delta.length().max(1.0)).max(1e-10);
        if !projected.is_finite() || projected.length() <= resolution {
            return Err("projected guide endpoints must be distinct and numerically resolved perpendicular to the view direction".into());
        }
        let u = projected.cross(d).normalize();
        let v = d.cross(u).normalize();
        if !u.is_finite() || !v.is_finite() || u.dot(d).abs() > 1e-10
            || v.dot(d).abs() > 1e-10 || u.dot(v).abs() > 1e-10
            || (u.length_squared() - 1.0).abs() > 1e-10
            || (v.length_squared() - 1.0).abs() > 1e-10
        {
            return Err("projected guide numerical conditioning cannot form an orthonormal view frame".into());
        }
        let knots: Vec<_> = points
            .iter()
            .map(|p| {
                let q = *p - origin;
                DVec2::new(q.dot(u), q.dot(v))
            })
            .collect();
        if knots.windows(2).any(|w| w[1].x - w[0].x <= 1e-10) {
            return Err("projected guide must advance monotonically from its first endpoint to its last; folds and projected duplicate points are unsupported".into());
        }
        // An exact-sign orientation predicate avoids treating a real small bend
        // as collinear solely because the coordinates have a large magnitude.
        for w in knots.windows(3) {
            let turn = robust::orient2d(
                robust::Coord {
                    x: w[0].x,
                    y: w[0].y,
                },
                robust::Coord {
                    x: w[1].x,
                    y: w[1].y,
                },
                robust::Coord {
                    x: w[2].x,
                    y: w[2].y,
                },
            );
            if !turn.is_finite() {
                return Err("projected guide exceeds the supported numeric range".into());
            }
        }
        Ok(Self {
            origin,
            u,
            v,
            knots,
        })
    }

    fn tolerances(&self, geo: &Geometry, eps: f64) -> Result<Tolerances, String> {
        if !eps.is_finite() || eps <= 0.0 || eps > 1e-3 {
            return Err("curtain snap tolerance must be finite, positive and at most 0.001 of the input diagonal".into());
        }
        let diagonal = geo.bounds().size().length().max(1e-9);
        let clip = (eps * diagonal).max(1e-12);
        let slope = self.knots.windows(2)
            .map(|w| ((w[1].y - w[0].y) / (w[1].x - w[0].x)).abs())
            .fold(0.0, f64::max);
        let height = geo.points.iter()
            .map(|p| self.height((*p - self.origin).dot(self.u)).abs())
            .chain(self.knots.iter().map(|p| p.y.abs()))
            .fold(0.0, f64::max);
        let coordinates = geo.points.iter().map(|p| p.abs().max_element())
            .fold(self.origin.abs().max_element(), f64::max);
        // A shear and its inverse each have infinity-norm condition at most
        // 1 + max |f'|. Include coordinate subtraction and determinant arithmetic
        // conservatively; do not expand the user's physical snap tolerance.
        let roundoff_world = 64.0 * f64::EPSILON * (1.0 + slope)
            * (2.0 * coordinates + height).max(1.0);
        // Empty region selections do not perform a shear or a clip at all.
        if !geo.prims.is_empty() && (!roundoff_world.is_finite() || roundoff_world > clip) {
            return Err(format!("curtain numerical conditioning exceeds the physical snap tolerance: estimated roundoff {roundoff_world:e} mm > {clip:e} mm; use a less steep projected guide or split it into separate cuts"));
        }
        Ok(Tolerances {
            clip,
            refine: (diagonal * 1e-13).max(1e-12),
            weld: (diagonal * 1e-12).max(1e-10),
            boundary: (diagonal * 1e-11).max(1e-9),
        })
    }

    fn height(&self, x: f64) -> f64 {
        let k = self
            .knots
            .partition_point(|p| p.x < x)
            .saturating_sub(1)
            .min(self.knots.len() - 2);
        let (a, b) = (self.knots[k], self.knots[k + 1]);
        // orient(a,b,(x,0)) = -(b.x-a.x)*f(x). The adaptive
        // determinant retains the sign when almost-collinear data cancels.
        -robust::orient2d(
            robust::Coord { x: a.x, y: a.y },
            robust::Coord { x: b.x, y: b.y },
            robust::Coord { x, y: 0.0 },
        ) / (b.x - a.x)
    }

    fn shear(&self, geo: &mut Geometry, inverse: bool) {
        let sign = if inverse { 1.0 } else { -1.0 };
        for p in &mut geo.points {
            let x = (*p - self.origin).dot(self.u);
            *p += self.v * (sign * self.height(x));
        }
    }

    fn refine(&self, mut geo: Geometry, tolerance: &Tolerances, sheared: bool) -> Result<Geometry, String> {
        for knot in &self.knots[1..self.knots.len() - 1] {
            let plane =
                Plane::new(self.origin + self.u * knot.x, self.u).expect("finite guide frame");
            let r = cut_mesh_absolute(&geo, &plane, tolerance.refine, false, 0).map_err(|e| e.to_string())?;
            geo = r.lower;
            geo.merge(&r.upper);
            weld(&mut geo, self, sheared, tolerance.weld);
            if geo.prims.len() > MAX_TRIANGLES {
                return Err(format!(
                    "curtain refinement exceeds {MAX_TRIANGLES} triangles"
                ));
            }
        }
        Ok(geo)
    }

    fn refine_cap(&self, side: Geometry, cap_id: i64, tolerance: &Tolerances) -> Result<Geometry, String> {
        let caps = side
            .prim_attribs
            .ints(mesh::CAP)
            .map_err(|e| e.to_string())?;
        let mut boundary = vec![false; side.points.len()];
        for (_, pr) in side
            .prims
            .iter()
            .enumerate()
            .filter(|(i, _)| caps[*i] != cap_id)
        {
            for &v in &pr.vertices {
                boundary[v as usize] = true;
            }
        }
        let tol = tolerance.boundary;
        let mut samples: Vec<_> = side
            .points
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                (boundary[i] && (*p - self.origin).dot(self.v).abs() <= tol)
                    .then_some(((*p - self.origin).dot(self.u), i))
            })
            .collect();
        samples.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut refined = self.refine(side.clone(), tolerance, true)?;
        let caps = refined
            .prim_attribs
            .ints(mesh::CAP)
            .map_err(|e| e.to_string())?;
        let mut on_body = vec![false; refined.points.len()];
        let mut on_cap = vec![false; refined.points.len()];
        for (i, pr) in refined.prims.iter().enumerate() {
            if caps[i] != cap_id {
                for &v in &pr.vertices {
                    on_body[v as usize] = true;
                }
            } else {
                for &v in &pr.vertices {
                    on_cap[v as usize] = true;
                }
            }
        }
        let d = self.u.cross(self.v);
        for (i, p) in refined
            .points
            .iter()
            .enumerate()
            .filter(|(i, _)| on_cap[*i] && !on_body[*i])
        {
            // A new cap-interior knot vertex has no original surface attribute.
            // Interpolate along its constant-u cross-section, where the inverse
            // shear is affine, rather than across a different guide strip.
            let u = (*p - self.origin).dot(self.u);
            let z = (*p - self.origin).dot(d);
            let mut below: Option<(usize, f64)> = None;
            let mut above: Option<(usize, f64)> = None;
            let start = samples.partition_point(|s| s.0 < u - tol);
            let end = samples.partition_point(|s| s.0 <= u + tol);
            for &(_, j) in &samples[start..end] {
                let q = side.points[j] - self.origin;
                let qz = q.dot(d);
                if qz <= z + tol && below.map_or(true, |(_, b)| qz > b) {
                    below = Some((j, qz));
                }
                if qz >= z - tol && above.map_or(true, |(_, a)| qz < a) {
                    above = Some((j, qz));
                }
            }
            let (Some((a, za)), Some((b, zb))) = (below, above) else {
                continue;
            };
            let t = if (zb - za).abs() < tol {
                0.0
            } else {
                ((z - za) / (zb - za)).clamp(0.0, 1.0)
            };
            for (name, col) in side.point_attribs.iter() {
                if let Some(dst) = refined.point_attribs.get_mut(name) {
                    match (col, dst) {
                        (AttribArray::Float(src), AttribArray::Float(dst)) => {
                            dst[i] = src[a] + t * (src[b] - src[a])
                        }
                        (AttribArray::Vec3(src), AttribArray::Vec3(dst)) => {
                            let v = src[a] + t * (src[b] - src[a]);
                            dst[i] = if name == "N" {
                                v.normalize_or_zero()
                            } else {
                                v
                            };
                        }
                        (AttribArray::Int(src), AttribArray::Int(dst)) => {
                            dst[i] = src[if t < 0.5 { a } else { b }]
                        }
                        (AttribArray::Str(src), AttribArray::Str(dst)) => {
                            dst[i] = src[if t < 0.5 { a } else { b }].clone()
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(refined)
    }
}

fn weld(geo: &mut Geometry, guide: &Guide, sheared: bool, tolerance: f64) {
    // Do not average coordinates: strip refinement must retain its exact
    // piecewise-affine inverse. Point attributes keep the first representative.
    // Compare distances after the inverse shear. Nearby transformed points can
    // be physically far apart when the guide is steep.
    let world;
    let points = if sheared {
        world = geo.points.iter().map(|p| {
            let x = (*p - guide.origin).dot(guide.u);
            *p + guide.v * guide.height(x)
        }).collect::<Vec<_>>();
        &world
    } else {
        &geo.points
    };
    let map = nodes_split::weld_map(points, tolerance);
    for pr in &mut geo.prims {
        for v in &mut pr.vertices {
            *v = map[*v as usize];
        }
    }
}

fn local_volume(geo: &Geometry) -> f64 {
    let origin = geo.points.first().copied().unwrap_or(DVec3::ZERO);
    geo.triangles().iter().map(|(_, triangle)| {
        let [a, b, c] = triangle.map(|v| geo.points[v as usize] - origin);
        a.dot(b.cross(c)) / 6.0
    }).sum()
}

fn closed_oriented(geo: &Geometry) -> bool {
    let mut edges: BTreeMap<(u32, u32), (usize, i32)> = BTreeMap::new();
    for prim in geo.prims.iter().filter(|p| p.closed) {
        for i in 0..prim.vertices.len() {
            let (a, b) = (prim.vertices[i], prim.vertices[(i + 1) % prim.vertices.len()]);
            if a == b { return false; }
            let e = edges.entry((a.min(b), a.max(b))).or_default();
            e.0 += 1;
            e.1 += if a < b { 1 } else { -1 };
        }
    }
    !edges.is_empty() && edges.values().all(|&(count, balance)| count == 2 && balance == 0)
}

pub fn cut(
    geo: &Geometry,
    points: &[DVec3],
    view: DVec3,
    eps: f64,
    cap: bool,
    keep: Keep,
    cap_id: i64,
) -> Result<CurtainResult, String> {
    geo.validate().map_err(|e| e.to_string())?;
    if geo
        .points
        .iter()
        .any(|p| !p.is_finite() || p.abs().max_element() > 1e12)
    {
        return Err("body coordinates must be finite and at most 1e12 mm in magnitude".into());
    }
    if geo.prims.len() > MAX_TRIANGLES {
        return Err(format!("curtain input exceeds {MAX_TRIANGLES} primitives"));
    }
    let guide = Guide::new(points, view)?;
    let tolerance = guide.tolerances(geo, eps)?;
    cap_id
        .checked_add((points.len() - 2) as i64)
        .ok_or("cap id overflow")?;
    let plane = Plane::new(guide.origin, guide.v).expect("finite guide frame");
    let mut result = CurtainResult {
        geo: Geometry::new(),
        segments: points.len() - 1,
        cap_area: 0.0,
        loops: 0,
        cap_start: cap_id,
    };
    let mut next_part = 0;
    for id in mesh::unique_parts(geo) {
        let input = mesh::extract_part(geo, id);
        let stats = mesh::edge_stats(&input);
        let closed_input = cap && stats.edges > 0 && stats.open == 0 && stats.non_manifold == 0;
        let volume_before = local_volume(&input);
        let mut volume_after = 0.0;
        let mut refined = guide.refine(input, &tolerance, false)?;
        guide.shear(&mut refined, false);
        let r = cut_mesh_absolute(&refined, &plane, tolerance.clip, cap, cap_id).map_err(|e| e.to_string())?;
        result.loops += r.loops;
        for (side, retain) in [
            (r.lower, keep != Keep::Upper),
            (r.upper, keep != Keep::Lower),
        ] {
            if !retain || side.prims.is_empty() {
                continue;
            }
            let mut side = guide.refine_cap(side, cap_id, &tolerance)?;
            guide.shear(&mut side, true);
            weld(&mut side, &guide, false, tolerance.weld);
            if side.points.iter().any(|p| !p.is_finite()) || (closed_input && !closed_oriented(&side)) {
                return Err("curtain numerical conditioning could not preserve a finite closed oriented manifold; use a less steep guide or split it into separate cuts".into());
            }
            volume_after += local_volume(&side);
            let caps = side
                .prim_attribs
                .ints(mesh::CAP)
                .map_err(|e| e.to_string())?;
            let cap_prims: Vec<_> = caps
                .iter()
                .enumerate()
                .filter_map(|(i, &c)| (c == cap_id).then_some(i))
                .collect();
            result.cap_area += mesh::area(&mesh::extract_prims(&side, &cap_prims));
            // Each curtain segment is planar and receives its own cap region,
            // so downstream joint tools never fit one plane across a bend.
            let segment_ids: Vec<_> = side
                .prims
                .iter()
                .map(|pr| {
                    let x = pr
                        .vertices
                        .iter()
                        .map(|&v| (side.points[v as usize] - guide.origin).dot(guide.u))
                        .sum::<f64>()
                        / pr.vertices.len().max(1) as f64;
                    guide
                        .knots
                        .partition_point(|k| k.x < x)
                        .saturating_sub(1)
                        .min(guide.knots.len() - 2) as i64
                })
                .collect();
            if let Some(AttribArray::Int(ids)) = side.prim_attribs.get_mut(mesh::CAP) {
                for (i, id) in ids.iter_mut().enumerate() {
                    if *id == cap_id {
                        *id = cap_id
                            .checked_add(segment_ids[i])
                            .ok_or("cap id overflow")?;
                    }
                }
            }
            let components = nodes_split::connectivity_parts(&side, false, 0.0);
            mesh::set_int_prim_attr(&mut side, mesh::PART, components);
            for component in mesh::unique_parts(&side) {
                let mut piece = mesh::extract_part(&side, component);
                let n = piece.prims.len();
                mesh::set_int_prim_attr(&mut piece, mesh::PART, vec![next_part; n]);
                piece.validate().map_err(|e| e.to_string())?;
                result.geo.merge(&piece);
                next_part += 1;
            }
        }
        if closed_input && keep == Keep::Both {
            let error = (volume_after - volume_before).abs() / volume_before.abs();
            if !error.is_finite() || error > 0.005 {
                return Err(format!("curtain numerical conditioning could not preserve the closed input volume within 0.5% (relative error {error:e}); use a less steep guide or split it into separate cuts"));
            }
        }
    }
    if keep == Keep::Both {
        result.cap_area *= 0.5;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wp5_parallel_guide_cannot_construct_a_frame() {
        for direction in [
            DVec3::ONE,
            DVec3::new(1.0, 2.0, 3.0),
            DVec3::new(3.0, 2.0, 1.0),
            DVec3::new(1.0, 0.0, 1.0),
            DVec3::new(1.0, 2.0, 0.0),
        ] {
            for scale in [1.0, 1e3, 1e6, 1e10] {
                assert!(Guide::new(&[DVec3::ZERO, direction * scale], direction).is_err(),
                    "parallel direction {direction:?} at scale {scale}");
            }
        }
    }
}

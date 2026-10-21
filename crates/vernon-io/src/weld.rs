//! Hash-grid vertex weld.

use std::collections::HashMap;

use vernon_core::{DVec3, Geometry, Prim};

/// Merge points that lie within `tolerance` (mm) of an earlier kept point.
///
/// Uses a uniform hash grid with cell size `tolerance`, so each query looks at
/// 27 cells. A tolerance of zero (or a non-finite one) merges only exactly
/// equal points. The first point of each cluster is kept along with its point
/// attributes. Primitive vertices are remapped; consecutive repeated vertices
/// are collapsed and primitives that become degenerate (closed with fewer than
/// three vertices, open with fewer than two) are removed together with their
/// primitive attributes. Groups are remapped.
///
/// Returns the number of points removed. The geometry must be valid.
pub fn weld_points(geo: &mut Geometry, tolerance: f64) -> usize {
    let n = geo.points.len();
    if n == 0 {
        return 0;
    }
    let mut remap = vec![0u32; n];
    let mut keep: Vec<usize> = Vec::new();

    if tolerance > 0.0 && tolerance.is_finite() {
        let inv = 1.0 / tolerance;
        let tol2 = tolerance * tolerance;
        let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
        for (i, &p) in geo.points.iter().enumerate() {
            let c = cell(p, inv);
            let mut found = None;
            'search: for dx in -1i64..=1 {
                for dy in -1i64..=1 {
                    for dz in -1i64..=1 {
                        let key = [
                            c[0].saturating_add(dx),
                            c[1].saturating_add(dy),
                            c[2].saturating_add(dz),
                        ];
                        if let Some(list) = grid.get(&key) {
                            for &k in list {
                                if geo.points[keep[k as usize]].distance_squared(p) <= tol2 {
                                    found = Some(k);
                                    break 'search;
                                }
                            }
                        }
                    }
                }
            }
            remap[i] = match found {
                Some(k) => k,
                None => {
                    let k = keep.len() as u32;
                    keep.push(i);
                    grid.entry(c).or_default().push(k);
                    k
                }
            };
        }
    } else {
        let mut map: HashMap<[u64; 3], u32> = HashMap::new();
        for (i, &p) in geo.points.iter().enumerate() {
            // `+ 0.0` folds -0.0 into 0.0 so they weld.
            let key = [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()];
            remap[i] = *map.entry(key).or_insert_with(|| {
                keep.push(i);
                (keep.len() - 1) as u32
            });
        }
    }

    let removed = n - keep.len();
    if removed == 0 {
        return 0;
    }

    geo.points = keep.iter().map(|&i| geo.points[i]).collect();
    geo.point_attribs = geo.point_attribs.gather_all(&keep);
    for idx in geo.point_groups.values_mut() {
        let mut v: Vec<u32> = idx.iter().map(|&i| remap[i as usize]).collect();
        v.sort_unstable();
        v.dedup();
        *idx = v;
    }

    let mut kept_prims = Vec::with_capacity(geo.prims.len());
    let mut new_prims = Vec::with_capacity(geo.prims.len());
    for (pi, prim) in geo.prims.iter().enumerate() {
        let mut v: Vec<u32> = Vec::with_capacity(prim.vertices.len());
        for &x in &prim.vertices {
            let r = remap[x as usize];
            if v.last() != Some(&r) {
                v.push(r);
            }
        }
        if prim.closed && v.len() > 1 && v.first() == v.last() {
            v.pop();
        }
        let min = if prim.closed { 3 } else { 2 };
        if v.len() >= min {
            kept_prims.push(pi);
            new_prims.push(Prim {
                vertices: v,
                closed: prim.closed,
            });
        }
    }
    if kept_prims.len() != geo.prims.len() {
        geo.prim_attribs = geo.prim_attribs.gather_all(&kept_prims);
        let mut pmap = vec![u32::MAX; geo.prims.len()];
        for (new, &old) in kept_prims.iter().enumerate() {
            pmap[old] = new as u32;
        }
        for idx in geo.prim_groups.values_mut() {
            idx.retain(|&i| pmap[i as usize] != u32::MAX);
            for i in idx.iter_mut() {
                *i = pmap[*i as usize];
            }
        }
    }
    geo.prims = new_prims;
    removed
}

fn cell(p: DVec3, inv: f64) -> [i64; 3] {
    // `as` saturates for out-of-range floats and maps NaN to 0.
    [
        (p.x * inv).floor() as i64,
        (p.y * inv).floor() as i64,
        (p.z * inv).floor() as i64,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use vernon_core::AttribArray;

    /// Box as 12 separate triangles (36 points), like an STL.
    fn triangle_soup() -> Geometry {
        let b = Geometry::make_box(DVec3::new(1.0, 2.0, 3.0));
        let mut g = Geometry::new();
        for (_, t) in b.triangles() {
            let base = g.points.len() as u32;
            for v in t {
                g.points.push(b.points[v as usize]);
            }
            g.prims.push(Prim::polygon(vec![base, base + 1, base + 2]));
        }
        g
    }

    #[test]
    fn welds_soup_to_box() {
        let mut g = triangle_soup();
        let n = g.prim_count();
        g.prim_attribs
            .set("id", AttribArray::Int((0..n as i64).collect()));
        assert_eq!(weld_points(&mut g, 1e-6), 28);
        assert_eq!(g.point_count(), 8);
        assert_eq!(g.prim_count(), 12);
        g.validate().unwrap();
        assert!((g.surface_area() - 2.0 * (2.0 + 3.0 + 6.0)).abs() < 1e-9);
    }

    #[test]
    fn exact_mode_and_tolerance() {
        let mut g = Geometry::from_points(vec![
            DVec3::ZERO,
            DVec3::new(-0.0, 0.0, 0.0),
            DVec3::new(5e-4, 0.0, 0.0),
        ]);
        let mut h = g.clone();
        assert_eq!(weld_points(&mut g, 0.0), 1);
        assert_eq!(g.point_count(), 2);
        assert_eq!(weld_points(&mut h, 1e-3), 2);
        assert_eq!(h.point_count(), 1);
    }

    #[test]
    fn drops_degenerate_prims_and_remaps_groups() {
        let mut g = Geometry::from_polygons(
            vec![
                DVec3::ZERO,
                DVec3::X,
                DVec3::new(1e-9, 0.0, 0.0),
                DVec3::Y,
                DVec3::new(1.0, 1.0, 0.0),
            ],
            vec![vec![0, 1, 2], vec![0, 1, 4, 3]],
        );
        g.prim_attribs
            .set("w", AttribArray::Float(vec![1.0, 2.0]));
        g.prim_groups.insert("both".into(), vec![0, 1]);
        g.point_groups.insert("p".into(), vec![2, 4]);
        weld_points(&mut g, 1e-6);
        assert_eq!(g.prim_count(), 1);
        assert_eq!(g.prim_attribs.floats("w").unwrap(), &[2.0]);
        assert_eq!(g.prim_groups["both"], vec![0]);
        assert_eq!(g.point_groups["p"], vec![0, 3]);
        g.validate().unwrap();
    }
}

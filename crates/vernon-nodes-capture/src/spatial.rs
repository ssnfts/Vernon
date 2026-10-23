//! Uniform hash grid for exact radius / nearest queries, voxel grouping and
//! PCA normal estimation. Pure Rust, deterministic.

use std::collections::HashMap;

use glam::DVec3;

use crate::linalg::sym_eigen;

type Key = [i64; 3];

/// Largest cell index magnitude; coordinates beyond it share the edge cell
/// (a point at 1e300 mm must not overflow the key or stall a query).
const KEY_LIMIT: f64 = (1u64 << 52) as f64;

/// Hash grid over a point slice. Queries return indices into that slice.
pub struct Grid {
    cell: f64,
    inv: f64,
    cells: HashMap<Key, Vec<u32>>,
}

fn cell_index(v: f64) -> i64 {
    // NaN casts to 0.
    v.floor().clamp(-KEY_LIMIT, KEY_LIMIT) as i64
}

fn key_of(p: DVec3, inv: f64) -> Key {
    [cell_index(p.x * inv), cell_index(p.y * inv), cell_index(p.z * inv)]
}

impl Grid {
    pub fn build(points: &[DVec3], cell: f64) -> Self {
        let cell = if cell.is_finite() && cell > 1e-9 { cell } else { 1.0 };
        let inv = 1.0 / cell;
        let mut cells: HashMap<Key, Vec<u32>> = HashMap::new();
        for (i, p) in points.iter().enumerate() {
            cells.entry(key_of(*p, inv)).or_default().push(i as u32);
        }
        Grid { cell, inv, cells }
    }

    /// The cell range to probe around a query of radius `r`, or `None` when
    /// probing `(2 span + 1)^3` cells would cost more than testing every
    /// point (a radius far larger than the cell, or an infinite one): the
    /// caller then scans the point slice instead.
    fn span(&self, r: f64, n_points: usize) -> Option<i64> {
        let span = (r / self.cell).ceil().max(1.0);
        let probes = (2.0 * span + 1.0).powi(3);
        (probes <= n_points.max(27) as f64).then_some(span as i64)
    }

    /// Append the indices of all points within `r` of `p` to `out`.
    pub fn radius_query(&self, points: &[DVec3], p: DVec3, r: f64, out: &mut Vec<u32>) {
        let r2 = r * r;
        let Some(span) = self.span(r, points.len()) else {
            out.extend((0..points.len() as u32).filter(|&i| (points[i as usize] - p).length_squared() <= r2));
            return;
        };
        let k = key_of(p, self.inv);
        for dx in -span..=span {
            for dy in -span..=span {
                for dz in -span..=span {
                    if let Some(v) = self.cells.get(&[k[0].wrapping_add(dx), k[1].wrapping_add(dy), k[2].wrapping_add(dz)]) {
                        for &i in v {
                            if (points[i as usize] - p).length_squared() <= r2 {
                                out.push(i);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Nearest point within `r` (index, squared distance).
    pub fn nearest(&self, points: &[DVec3], p: DVec3, r: f64) -> Option<(u32, f64)> {
        let mut best: Option<(u32, f64)> = None;
        let mut limit = r * r;
        let mut visit = |i: u32, best: &mut Option<(u32, f64)>| {
            let d2 = (points[i as usize] - p).length_squared();
            if d2 <= limit && (best.is_none() || d2 < limit) {
                *best = Some((i, d2));
                limit = d2;
            }
        };
        let Some(span) = self.span(r, points.len()) else {
            for i in 0..points.len() as u32 {
                visit(i, &mut best);
            }
            return best;
        };
        let k = key_of(p, self.inv);
        for dx in -span..=span {
            for dy in -span..=span {
                for dz in -span..=span {
                    if let Some(v) = self.cells.get(&[k[0].wrapping_add(dx), k[1].wrapping_add(dy), k[2].wrapping_add(dz)]) {
                        for &i in v {
                            visit(i, &mut best);
                        }
                    }
                }
            }
        }
        best
    }
}

/// Group point indices by voxel of edge `voxel`, in first-seen order.
pub fn voxel_groups(points: &[DVec3], voxel: f64) -> Vec<Vec<u32>> {
    let inv = 1.0 / voxel.max(1e-9);
    let mut index: HashMap<Key, usize> = HashMap::new();
    let mut groups: Vec<Vec<u32>> = Vec::new();
    for (i, p) in points.iter().enumerate() {
        let k = key_of(*p, inv);
        let g = *index.entry(k).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[g].push(i as u32);
    }
    groups
}

/// One centroid per occupied voxel.
pub fn voxel_centroids(points: &[DVec3], voxel: f64) -> Vec<DVec3> {
    voxel_groups(points, voxel)
        .iter()
        .map(|g| g.iter().map(|&i| points[i as usize]).sum::<DVec3>() / g.len() as f64)
        .collect()
}

/// Local plane fit around `p` from `neigh`: `(normal, lambda_min, lambda_mid,
/// lambda_max, n)`, with eigenvalues of the covariance of the neighbours.
pub fn plane_fit(points: &[DVec3], neigh: &[u32]) -> Option<(DVec3, [f64; 3])> {
    if neigh.len() < 3 {
        return None;
    }
    let c = neigh.iter().map(|&i| points[i as usize]).sum::<DVec3>() / neigh.len() as f64;
    let mut m = [0.0; 9];
    for &i in neigh {
        let d = (points[i as usize] - c).to_array();
        for r in 0..3 {
            for k in 0..3 {
                m[r * 3 + k] += d[r] * d[k];
            }
        }
    }
    let (vals, vecs) = sym_eigen(3, &m);
    let mut order = [0usize, 1, 2];
    order.sort_by(|&a, &b| vals[a].total_cmp(&vals[b]));
    let k = order[0];
    let n = DVec3::new(vecs[k], vecs[3 + k], vecs[6 + k]).normalize_or_zero();
    Some((n, [vals[order[0]], vals[order[1]], vals[order[2]]]))
}

/// PCA normals (unoriented) from neighbours within `radius`, using at most
/// `max_nn` nearest of them. Points with fewer than three neighbours get
/// `DVec3::ZERO`.
pub fn estimate_normals(points: &[DVec3], radius: f64, max_nn: usize) -> Vec<DVec3> {
    let grid = Grid::build(points, radius);
    let mut buf = Vec::new();
    points
        .iter()
        .map(|&p| {
            buf.clear();
            grid.radius_query(points, p, radius, &mut buf);
            if buf.len() > max_nn {
                buf.sort_by(|&a, &b| {
                    (points[a as usize] - p)
                        .length_squared()
                        .total_cmp(&(points[b as usize] - p).length_squared())
                });
                buf.truncate(max_nn);
            }
            plane_fit(points, &buf).map_or(DVec3::ZERO, |(n, _)| n)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lattice(n: i32) -> Vec<DVec3> {
        let mut v = Vec::new();
        for x in 0..n {
            for y in 0..n {
                v.push(DVec3::new(f64::from(x), f64::from(y), 0.0));
            }
        }
        v
    }

    #[test]
    fn radius_and_nearest_match_brute_force() {
        let pts = lattice(12);
        let g = Grid::build(&pts, 2.5);
        let q = DVec3::new(5.3, 6.1, 0.4);
        let mut out = Vec::new();
        g.radius_query(&pts, q, 2.5, &mut out);
        out.sort_unstable();
        let brute: Vec<u32> = (0..pts.len() as u32)
            .filter(|&i| (pts[i as usize] - q).length() <= 2.5)
            .collect();
        assert_eq!(out, brute);
        let (i, d2) = g.nearest(&pts, q, 2.5).unwrap();
        assert_eq!(pts[i as usize], DVec3::new(5.0, 6.0, 0.0));
        assert!((d2 - (0.09 + 0.01 + 0.16)).abs() < 1e-12);
        assert!(g.nearest(&pts, DVec3::new(100.0, 0.0, 0.0), 2.5).is_none());
    }

    #[test]
    fn huge_radii_and_coordinates_neither_hang_nor_overflow() {
        let mut pts = lattice(10);
        pts.push(DVec3::new(1e300, -1e300, 5e17));
        let g = Grid::build(&pts, 2.0);
        let q = DVec3::new(4.2, 4.7, 0.1);
        // A radius thousands of cells wide takes the scan path and agrees
        // with a brute-force pass.
        for r in [1e6, 1e300, f64::INFINITY] {
            let mut out = Vec::new();
            g.radius_query(&pts, q, r, &mut out);
            out.sort_unstable();
            let brute: Vec<u32> = (0..pts.len() as u32)
                .filter(|&i| (pts[i as usize] - q).length_squared() <= r * r)
                .collect();
            assert_eq!(out, brute, "r = {r}");
        }
        assert_eq!(g.nearest(&pts, q, 1e300).map(|(i, _)| pts[i as usize]), Some(DVec3::new(4.0, 5.0, 0.0)));
        // The outlier is found from its own neighbourhood.
        let far = *pts.last().unwrap();
        assert_eq!(g.nearest(&pts, far, 1.0).map(|(i, _)| i), Some(pts.len() as u32 - 1));
        assert_eq!(voxel_groups(&pts, 1e-9).len(), pts.len());
    }

    #[test]
    fn voxel_grouping_counts() {
        let pts = lattice(10);
        assert_eq!(voxel_groups(&pts, 2.0).len(), 25);
        assert_eq!(voxel_centroids(&pts, 100.0).len(), 1);
    }

    #[test]
    fn plane_normals_point_along_z() {
        let pts = lattice(10);
        let n = estimate_normals(&pts, 2.5, 30);
        assert!(n.iter().all(|v| v.z.abs() > 0.999));
    }
}

//! Uniform hash grid for exact radius / nearest queries, voxel grouping and
//! PCA normal estimation. Pure Rust, deterministic.

use std::collections::HashMap;

use glam::DVec3;

use crate::linalg::sym_eigen;

type Key = [i32; 3];

/// Hash grid over a point slice. Queries return indices into that slice.
pub struct Grid {
    cell: f64,
    inv: f64,
    cells: HashMap<Key, Vec<u32>>,
}

fn key_of(p: DVec3, inv: f64) -> Key {
    [
        (p.x * inv).floor() as i32,
        (p.y * inv).floor() as i32,
        (p.z * inv).floor() as i32,
    ]
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

    /// Append the indices of all points within `r` of `p` to `out`.
    pub fn radius_query(&self, points: &[DVec3], p: DVec3, r: f64, out: &mut Vec<u32>) {
        let r2 = r * r;
        let span = (r / self.cell).ceil().max(1.0) as i32;
        let k = key_of(p, self.inv);
        for dx in -span..=span {
            for dy in -span..=span {
                for dz in -span..=span {
                    if let Some(v) = self.cells.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) {
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
        let span = (r / self.cell).ceil().max(1.0) as i32;
        let k = key_of(p, self.inv);
        let mut best: Option<(u32, f64)> = None;
        let mut limit = r * r;
        for dx in -span..=span {
            for dy in -span..=span {
                for dz in -span..=span {
                    if let Some(v) = self.cells.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) {
                        for &i in v {
                            let d2 = (points[i as usize] - p).length_squared();
                            if d2 <= limit && (best.is_none() || d2 < limit) {
                                best = Some((i, d2));
                                limit = d2;
                            }
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

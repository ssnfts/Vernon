//! Synthetic meshes with known geometry.

use vernon_core::{DVec3, Geometry};

/// Triangulated height field over a `size` x `size` mm square, close to `target_tris` triangles.
/// Smooth but not symmetric, so a rigid fit is well conditioned in all six degrees of freedom.
pub fn heightfield(target_tris: usize, size: f64) -> Geometry {
    let n = ((target_tris as f64 / 2.0).sqrt().round() as usize).max(2);
    let mut pts = Vec::with_capacity((n + 1) * (n + 1));
    for j in 0..=n {
        for i in 0..=n {
            let (x, y) = (size * i as f64 / n as f64, size * j as f64 / n as f64);
            let z = 15.0 * (x / 40.0).sin() * (y / 55.0).cos() + 8.0 * (x / 17.0 + y / 23.0).sin() + 0.02 * x;
            pts.push(DVec3::new(x, y, z));
        }
    }
    let at = |i: usize, j: usize| (j * (n + 1) + i) as u32;
    let mut faces = Vec::with_capacity(2 * n * n);
    for j in 0..n {
        for i in 0..n {
            faces.push(vec![at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
            faces.push(vec![at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
        }
    }
    Geometry::from_polygons(pts, faces)
}

/// Open cylinder strip: radius `r`, sweep `angle` radians, width `w` along Z. A developable surface.
pub fn cylinder_strip(r: f64, angle: f64, w: f64, nu: usize, nv: usize) -> Geometry {
    let mut pts = Vec::new();
    for j in 0..=nv {
        for i in 0..=nu {
            let a = angle * i as f64 / nu as f64;
            pts.push(DVec3::new(r * a.cos(), r * a.sin(), w * j as f64 / nv as f64));
        }
    }
    let at = |i: usize, j: usize| (j * (nu + 1) + i) as u32;
    let mut faces = Vec::new();
    for j in 0..nv {
        for i in 0..nu {
            faces.push(vec![at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
            faces.push(vec![at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
        }
    }
    Geometry::from_polygons(pts, faces)
}

/// Spherical cap of radius `r` up to polar angle `theta` (radians). Not developable.
pub fn sphere_cap(r: f64, theta: f64, nu: usize, nv: usize) -> Geometry {
    let mut pts = vec![DVec3::new(0.0, 0.0, r)];
    for j in 1..=nv {
        let t = theta * j as f64 / nv as f64;
        for i in 0..nu {
            let a = std::f64::consts::TAU * i as f64 / nu as f64;
            pts.push(DVec3::new(r * t.sin() * a.cos(), r * t.sin() * a.sin(), r * t.cos()));
        }
    }
    let ring = |j: usize, i: usize| (1 + (j - 1) * nu + i % nu) as u32;
    let mut faces = Vec::new();
    for i in 0..nu {
        faces.push(vec![0, ring(1, i), ring(1, i + 1)]);
    }
    for j in 1..nv {
        for i in 0..nu {
            faces.push(vec![ring(j, i), ring(j + 1, i), ring(j + 1, i + 1)]);
            faces.push(vec![ring(j, i), ring(j + 1, i + 1), ring(j, i + 1)]);
        }
    }
    Geometry::from_polygons(pts, faces)
}

pub fn max_point_distance(a: &Geometry, b: &Geometry) -> f64 {
    a.points.iter().zip(&b.points).map(|(p, q)| p.distance(*q)).fold(0.0, f64::max)
}

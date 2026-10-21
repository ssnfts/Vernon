//! Home-grid line generation (pure).

use glam::Vec3;

use crate::camera::GridPlane;
use crate::mesh::PointVertex;

/// Maximum grid lines per side of the centre; the spacing is multiplied by
/// 10 until the grid fits.
pub const MAX_GRID_LINES_PER_SIDE: f32 = 100.0;

/// Line colours (linear RGB) for the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridColors {
    pub minor: [f32; 3],
    /// Every 10th line.
    pub major: [f32; 3],
    /// The two lines through the world origin.
    pub axis: [f32; 3],
}

/// Spacing actually used for a grid of half-size `extent` (mm): `spacing`
/// times a power of 10 so there are at most [`MAX_GRID_LINES_PER_SIDE`]
/// lines per side.
pub fn effective_spacing(spacing: f32, extent: f32) -> f32 {
    let mut s = if spacing.is_finite() && spacing > 0.0 { spacing } else { 10.0 };
    let extent = if extent.is_finite() { extent.abs() } else { 0.0 };
    while extent / s > MAX_GRID_LINES_PER_SIDE {
        s *= 10.0;
    }
    s
}

/// Build a square grid as a line list (two vertices per line) in `plane`,
/// centred near `center` (snapped to the spacing) with half-size `extent`.
pub fn build_grid(
    plane: GridPlane,
    center: Vec3,
    extent: f32,
    spacing: f32,
    colors: &GridColors,
) -> Vec<PointVertex> {
    let s = effective_spacing(spacing, extent);
    let k = (extent.abs() / s).ceil().max(1.0) as i64;
    let (cu, cv) = match plane {
        GridPlane::XY => (center.x, center.y),
        GridPlane::XZ => (center.x, center.z),
        GridPlane::YZ => (center.y, center.z),
    };
    let iu = (cu / s).round() as i64;
    let iv = (cv / s).round() as i64;
    let to3 = |u: f32, v: f32| match plane {
        GridPlane::XY => [u, v, 0.0],
        GridPlane::XZ => [u, 0.0, v],
        GridPlane::YZ => [0.0, u, v],
    };
    let color_for = |i: i64| {
        if i == 0 {
            colors.axis
        } else if i % 10 == 0 {
            colors.major
        } else {
            colors.minor
        }
    };
    let (u0, u1) = ((iu - k) as f32 * s, (iu + k) as f32 * s);
    let (v0, v1) = ((iv - k) as f32 * s, (iv + k) as f32 * s);
    let mut out = Vec::with_capacity(((2 * k + 1) * 4) as usize);
    // Lines of constant u, then constant v.
    for i in (iu - k)..=(iu + k) {
        let u = i as f32 * s;
        let color = color_for(i);
        out.push(PointVertex { pos: to3(u, v0), color });
        out.push(PointVertex { pos: to3(u, v1), color });
    }
    for i in (iv - k)..=(iv + k) {
        let v = i as f32 * s;
        let color = color_for(i);
        out.push(PointVertex { pos: to3(u0, v), color });
        out.push(PointVertex { pos: to3(u1, v), color });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: GridColors = GridColors {
        minor: [0.1; 3],
        major: [0.2; 3],
        axis: [0.0; 3],
    };

    #[test]
    fn grid_line_counts_and_plane() {
        let g = build_grid(GridPlane::XY, Vec3::ZERO, 100.0, 10.0, &C);
        // 21 lines each way, 2 vertices per line.
        assert_eq!(g.len(), 2 * 21 * 2);
        assert!(g.iter().all(|v| v.pos[2] == 0.0));
        // Origin axis lines are present and coloured as axes.
        assert!(g.iter().any(|v| v.pos[0] == 0.0 && v.color == C.axis));
        let xz = build_grid(GridPlane::XZ, Vec3::ZERO, 100.0, 10.0, &C);
        assert!(xz.iter().all(|v| v.pos[1] == 0.0));
        let yz = build_grid(GridPlane::YZ, Vec3::ZERO, 100.0, 10.0, &C);
        assert!(yz.iter().all(|v| v.pos[0] == 0.0));
    }

    #[test]
    fn grid_spacing_adapts() {
        assert_eq!(effective_spacing(10.0, 500.0), 10.0);
        assert_eq!(effective_spacing(10.0, 25_000.0), 1000.0);
        let g = build_grid(GridPlane::XY, Vec3::new(5000.0, 0.0, 0.0), 25_000.0, 10.0, &C);
        let lines = g.len() / 2;
        assert!(lines <= 2 * (2 * MAX_GRID_LINES_PER_SIDE as usize + 1));
        // Centre snapped to the effective spacing.
        assert!(g.iter().all(|v| (v.pos[0] / 1000.0).fract().abs() < 1e-4));
    }
}

//! Home grid (spec 2.5 and 3.9): adaptive spacing, line lists, edge fade, and
//! the world-space lines a tile draws. The backends draw them: depth-tested on
//! the GPU, behind everything on the CPU painter, which has no depth buffer.

use eframe::egui::Color32;
use glam::{DVec2, DVec3};

use super::camera::{Camera, ViewKind};
use crate::theme;

/// Home grid settings. Spacing in mm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSettings {
    pub spacing: f64,
    pub major_every: i64,
    /// Perspective grid half-length in grid squares.
    pub extent: i64,
    pub adaptive: bool,
    /// Stop subdividing below `spacing` when zooming in.
    pub inhibit_subdivision: bool,
}

impl Default for GridSettings {
    fn default() -> Self {
        Self {
            spacing: 10.0,
            major_every: 10,
            extent: 7,
            adaptive: true,
            inhibit_subdivision: true,
        }
    }
}

/// Grow or shrink the spacing by powers of ten so a square stays 8-80 px.
pub fn adaptive_spacing(base: f64, world_per_px: f64, inhibit: bool) -> f64 {
    let mut s = base;
    if !(world_per_px.is_finite() && world_per_px > 0.0 && base > 0.0) {
        return base;
    }
    for _ in 0..40 {
        if s / world_per_px < 8.0 {
            s *= 10.0;
        } else {
            break;
        }
    }
    let floor = if inhibit { base } else { base / 1000.0 };
    for _ in 0..40 {
        if s / world_per_px > 80.0 && s > floor * (1.0 + 1e-9) {
            s /= 10.0;
        } else {
            break;
        }
    }
    s
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridLine {
    /// Line index: the line lies at `k * spacing`.
    pub k: i64,
    pub major: bool,
}

/// Lines `kmin..=kmax`; every `major_every`-th line (and the axis) is major.
pub fn lines(kmin: i64, kmax: i64, major_every: i64) -> Vec<GridLine> {
    let n = major_every.max(2);
    (kmin..=kmax)
        .map(|k| GridLine { k, major: k % n == 0 })
        .collect()
}

/// Lines of a grid of half-length `extent` squares.
pub fn extent_lines(extent: i64, major_every: i64) -> Vec<GridLine> {
    lines(-extent, extent, major_every)
}

/// Opacity of line `k`: 1 inside 80 % of the extent, fading to 0 at the edge.
pub fn fade(k: i64, extent: i64) -> f32 {
    let e = extent.max(1) as f32;
    let a = k.unsigned_abs() as f32 / e;
    if a <= 0.8 {
        1.0
    } else {
        ((1.0 - a) / 0.2).clamp(0.0, 1.0)
    }
}

/// Short text for the status bar, e.g. `10 mm` or `1 m`.
pub fn format_spacing(mm: f64) -> String {
    if mm >= 1000.0 {
        format!("{} m", trim(mm / 1000.0))
    } else {
        format!("{} mm", trim(mm))
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The construction plane of a view: normal and the two in-plane axes. Side
/// views use the world plane they face most, because a stored front view (spec
/// 2.4) can turn them away from their usual axis.
pub fn plane_of(kind: ViewKind, back: DVec3) -> (DVec3, DVec3, DVec3) {
    let b = back.abs();
    match kind {
        ViewKind::Front | ViewKind::Back | ViewKind::Left | ViewKind::Right if b.z < b.x.max(b.y) => {
            if b.x >= b.y {
                (DVec3::X, DVec3::Y, DVec3::Z)
            } else {
                (DVec3::Y, DVec3::X, DVec3::Z)
            }
        }
        _ => (DVec3::Z, DVec3::X, DVec3::Y),
    }
}

/// One line of the grid, already faded against the viewport background.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSegment {
    pub a: DVec3,
    pub b: DVec3,
    pub color: Color32,
}

/// What a tile draws for its home grid.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GridGeometry {
    /// Size of one grid square in mm, for the status bar.
    pub spacing: f64,
    pub segments: Vec<GridSegment>,
    /// The ground plane under the grid when the camera is above it: it hides
    /// geometry below Z = 0. Vertical construction planes hide nothing, or
    /// they would cut the model in half.
    pub ground: Option<[DVec3; 4]>,
}

/// Grid of a tile (spec 3.9): across the whole visible region in a head-on
/// orthographic view, otherwise a fading square around the view target.
pub fn build(cam: &Camera, kind: ViewKind, size: DVec2, gs: &GridSettings) -> GridGeometry {
    let wpp = cam.world_per_pixel(size.y);
    let spacing = if gs.adaptive {
        adaptive_spacing(gs.spacing, wpp, gs.inhibit_subdivision)
    } else {
        gs.spacing
    };
    let (n, u, v) = plane_of(kind, cam.back());
    let minor = Color32::from_rgb(0x44, 0x44, 0x44);
    let major = Color32::from_rgb(0x52, 0x52, 0x52);
    let axis = Color32::from_rgb(0x22, 0x22, 0x22);
    let mut segments = Vec::new();
    let (lo, hi);
    if cam.ortho && cam.back().dot(n).abs() > 0.999 {
        // Lines across the whole visible region.
        let corners = [
            cam.focal_point(DVec2::ZERO, size),
            cam.focal_point(size, size),
            cam.focal_point(DVec2::new(size.x, 0.0), size),
            cam.focal_point(DVec2::new(0.0, size.y), size),
        ];
        let range = |a: DVec3| {
            let lo = corners.iter().map(|c| c.dot(a)).fold(f64::MAX, f64::min);
            let hi = corners.iter().map(|c| c.dot(a)).fold(f64::MIN, f64::max);
            (lo, hi)
        };
        let ((lo_u, hi_u), (lo_v, hi_v)) = (range(u), range(v));
        for (dir, across, l, h, l2, h2) in [(u, v, lo_u, hi_u, lo_v, hi_v), (v, u, lo_v, hi_v, lo_u, hi_u)] {
            let k0 = (l / spacing).floor() as i64;
            let k1 = (h / spacing).ceil() as i64;
            if k1 - k0 > 600 {
                continue;
            }
            for line in lines(k0, k1, gs.major_every) {
                let c = line.k as f64 * spacing;
                let color = if line.k == 0 { axis } else if line.major { major } else { minor };
                segments.push(GridSegment { a: dir * c + across * l2, b: dir * c + across * h2, color });
            }
        }
        (lo, hi) = (u * (lo_u - spacing) + v * (lo_v - spacing), u * (hi_u + spacing) + v * (hi_v + spacing));
    } else {
        let e = (gs.extent * gs.major_every).clamp(4, 200);
        let half = e as f64 * spacing;
        // Centre the grid on the major line nearest the view target.
        let step = spacing * gs.major_every as f64;
        let cu = (cam.target.dot(u) / step).round() * step;
        let cv = (cam.target.dot(v) / step).round() * step;
        for (dir, across, c0, c1) in [(u, v, cu, cv), (v, u, cv, cu)] {
            for line in extent_lines(e, gs.major_every) {
                let alpha = fade(line.k, e);
                if alpha <= 0.0 {
                    continue;
                }
                let world_k = ((c0 / spacing).round() as i64) + line.k;
                let c = world_k as f64 * spacing;
                let base = if world_k == 0 {
                    axis
                } else if world_k % gs.major_every.max(2) == 0 {
                    major
                } else {
                    minor
                };
                segments.push(GridSegment {
                    a: dir * c + across * (c1 - half),
                    b: dir * c + across * (c1 + half),
                    color: theme::mix(theme::VIEWPORT_BG, base, alpha),
                });
            }
        }
        (lo, hi) = (u * (cu - half) + v * (cv - half), u * (cu + half) + v * (cv + half));
    }
    let above = if cam.ortho { cam.back().z > 1e-6 } else { cam.eye().z > 0.0 };
    let ground = (n == DVec3::Z && above).then(|| {
        [lo, DVec3::new(hi.x, lo.y, 0.0), hi, DVec3::new(lo.x, hi.y, 0.0)]
    });
    GridGeometry { spacing, segments, ground }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_lies_in_the_view_plane_and_only_the_ground_hides_geometry() {
        let size = DVec2::new(800.0, 600.0);
        let gs = GridSettings::default();
        for kind in [ViewKind::Top, ViewKind::Perspective] {
            let g = build(&Camera::new(kind), kind, size, &gs);
            assert!(!g.segments.is_empty());
            assert!(g.segments.iter().all(|s| s.a.z == 0.0 && s.b.z == 0.0), "{kind:?}");
            let q = g.ground.expect("camera above the ground");
            assert!(q.iter().all(|p| p.z == 0.0));
            // The plane covers every grid line.
            let (lo, hi) = (q[0], q[2]);
            for s in &g.segments {
                assert!(s.a.x >= lo.x - 1e-9 && s.b.x <= hi.x + 1e-9, "{kind:?}");
                assert!(s.a.y >= lo.y - 1e-9 && s.b.y <= hi.y + 1e-9, "{kind:?}");
            }
        }
        // From below the plane hides nothing, and so does a vertical plane.
        for kind in [ViewKind::Bottom, ViewKind::Front, ViewKind::Left] {
            assert_eq!(build(&Camera::new(kind), kind, size, &gs).ground, None, "{kind:?}");
        }
        let front = build(&Camera::new(ViewKind::Front), ViewKind::Front, size, &gs);
        assert!(front.segments.iter().all(|s| s.a.x == 0.0 && s.b.x == 0.0));
    }

    #[test]
    fn grid_spacing_matches_the_adaptive_rule() {
        let cam = Camera::new(ViewKind::Top);
        let size = DVec2::new(800.0, 500.0);
        let g = build(&cam, ViewKind::Top, size, &GridSettings::default());
        let expect = adaptive_spacing(10.0, cam.world_per_pixel(size.y), true);
        assert_eq!(g.spacing, expect);
    }

    #[test]
    fn golden_line_counts() {
        let l = extent_lines(7, 10);
        assert_eq!(l.len(), 15);
        assert_eq!(l.iter().filter(|g| g.major).count(), 1);
        let l = extent_lines(10, 10);
        assert_eq!(l.len(), 21);
        let majors: Vec<i64> = l.iter().filter(|g| g.major).map(|g| g.k * 100).collect();
        assert_eq!(majors, vec![-1000, 0, 1000]);
    }

    #[test]
    fn golden_adaptive() {
        assert_eq!(adaptive_spacing(10.0, 5.0, true), 100.0);
        assert_eq!(adaptive_spacing(10.0, 0.5, true), 10.0);
        // Inhibit stops at the base; without it the grid keeps subdividing.
        assert_eq!(adaptive_spacing(10.0, 0.001, true), 10.0);
        assert!(adaptive_spacing(10.0, 0.001, false) < 10.0);
    }

    #[test]
    fn fade_and_format() {
        assert_eq!(fade(0, 10), 1.0);
        assert_eq!(fade(10, 10), 0.0);
        assert!(fade(9, 10) > 0.0 && fade(9, 10) < 1.0);
        assert_eq!(format_spacing(10.0), "10 mm");
        assert_eq!(format_spacing(2500.0), "2.5 m");
    }
}

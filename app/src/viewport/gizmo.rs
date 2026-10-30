//! Transform gizmo (spec 3.5): world-space handle lines at constant screen
//! size, which handle a pointer is over, and how a drag along an axis maps to
//! millimetres. Pure; the backends draw the lines on top of the scene.

use eframe::egui::Color32;
use glam::{DVec2, DVec3};

use super::camera::Camera;
use super::grid::GridSegment;

/// Length of a gizmo handle on screen, in pixels.
pub const HANDLE_PX: f64 = 70.0;
/// Pointer tolerance of a handle, in pixels.
pub const HIT_PX: f64 = 6.0;

/// Which handles the active tool shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Axis arrows.
    Move,
    /// A ring about each axis.
    Rotate,
    /// Axes ending in small squares.
    Scale,
}

const AXES: [DVec3; 3] = [DVec3::X, DVec3::Y, DVec3::Z];

/// World length that covers `px` pixels at the depth of `p` (spec 3.5).
pub fn world_scale(cam: &Camera, p: DVec3, height_px: f64, px: f64) -> f64 {
    if cam.ortho {
        cam.world_per_pixel(height_px) * px
    } else {
        let d = (-cam.world_to_cam(p).z).max(cam.near());
        2.0 * d * (cam.fov_y * 0.5).tan() * px / height_px.max(1.0)
    }
}

/// The line segments of handle `axis`.
fn handle(kind: Kind, axis: usize, c: DVec3, s: f64) -> Vec<(DVec3, DVec3)> {
    let (a, u, v) = (AXES[axis], AXES[(axis + 1) % 3], AXES[(axis + 2) % 3]);
    match kind {
        Kind::Move => {
            let tip = c + a * s;
            let mut out = vec![(c, tip)];
            for side in [u, -u, v, -v] {
                out.push((tip, tip - a * s * 0.18 + side * s * 0.06));
            }
            out
        }
        Kind::Rotate => {
            let at = |k: usize| {
                let t = k as f64 / 48.0 * std::f64::consts::TAU;
                c + (u * t.cos() + v * t.sin()) * s
            };
            (0..48).map(|k| (at(k), at(k + 1))).collect()
        }
        Kind::Scale => {
            let tip = c + a * s;
            let q = [u + v, u - v, -u - v, -u + v].map(|d| tip + d * s * 0.06);
            let mut out = vec![(c, tip)];
            out.extend((0..4).map(|k| (q[k], q[(k + 1) % 4])));
            out
        }
    }
}

/// Coloured lines of all three handles; `hot` is drawn in `hot_color`.
pub fn segments(
    kind: Kind,
    cam: &Camera,
    c: DVec3,
    height_px: f64,
    colors: [Color32; 3],
    hot: Option<usize>,
    hot_color: Color32,
) -> Vec<GridSegment> {
    let s = world_scale(cam, c, height_px, HANDLE_PX);
    let mut out = Vec::new();
    for (axis, base) in colors.into_iter().enumerate() {
        let color = if hot == Some(axis) { hot_color } else { base };
        out.extend(handle(kind, axis, c, s).into_iter().map(|(a, b)| GridSegment { a, b, color }));
    }
    out
}

/// Handle under a pointer (pixels in the tile), nearest first.
pub fn hit(kind: Kind, cam: &Camera, c: DVec3, size: DVec2, p: DVec2) -> Option<usize> {
    let s = world_scale(cam, c, size.y, HANDLE_PX);
    (0..3)
        .filter_map(|axis| {
            let d = handle(kind, axis, c, s)
                .into_iter()
                .filter_map(|(a, b)| {
                    let (pa, _) = cam.project(a, size)?;
                    let (pb, _) = cam.project(b, size)?;
                    Some(segment_distance(p, pa, pb))
                })
                .fold(f64::INFINITY, f64::min);
            (d <= HIT_PX).then_some((d, axis))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, axis)| axis)
}

fn segment_distance(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    let d = b - a;
    let t = if d.length_squared() < 1e-12 { 0.0 } else { ((p - a).dot(d) / d.length_squared()).clamp(0.0, 1.0) };
    p.distance(a + d * t)
}

/// Millimetres along `axis` for a pixel drag: the drag projected onto the
/// axis as it appears on screen. An axis seen end-on does not move.
pub fn axis_drag(cam: &Camera, c: DVec3, axis: usize, drag: DVec2, size: DVec2) -> f64 {
    let s = world_scale(cam, c, size.y, HANDLE_PX);
    let (Some((a, _)), Some((b, _))) = (cam.project(c, size), cam.project(c + AXES[axis] * s, size)) else {
        return 0.0;
    };
    let d = b - a;
    if d.length() < 4.0 {
        return 0.0;
    }
    drag.dot(d) / d.length_squared() * s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewport::camera::ViewKind;

    const SIZE: DVec2 = DVec2::new(800.0, 600.0);

    fn colors() -> [Color32; 3] {
        [Color32::RED, Color32::GREEN, Color32::BLUE]
    }

    #[test]
    fn handles_keep_their_screen_size() {
        for kind in [ViewKind::Perspective, ViewKind::Top] {
            let mut cam = Camera::new(kind);
            let c = DVec3::new(30.0, 20.0, 0.0);
            let right = cam.right();
            for _ in 0..2 {
                let s = world_scale(&cam, c, SIZE.y, HANDLE_PX);
                let (a, _) = cam.project(c, SIZE).unwrap();
                let (b, _) = cam.project(c + right * s, SIZE).unwrap();
                assert!((a.distance(b) - HANDLE_PX).abs() < 0.5, "{kind:?}: {}", a.distance(b));
                cam.zoom_at(SIZE * 0.5, SIZE, -8.0);
            }
        }
    }

    #[test]
    fn hit_finds_the_axis_under_the_pointer() {
        let cam = Camera::new(ViewKind::Top);
        let c = cam.target;
        let (o, _) = cam.project(c, SIZE).unwrap();
        // From the top: +X right, +Y up; Z is end-on, so only reachable at the centre.
        let hit_at = |d: DVec2, k| hit(k, &cam, c, SIZE, o + d);
        assert_eq!(hit_at(DVec2::new(40.0, 0.0), Kind::Move), Some(0));
        assert_eq!(hit_at(DVec2::new(0.0, -40.0), Kind::Move), Some(1));
        assert_eq!(hit_at(DVec2::new(40.0, 40.0), Kind::Move), None);
        assert_eq!(hit_at(DVec2::new(0.0, -70.0), Kind::Scale), Some(1));
        // The Z ring lies in the view plane at the handle radius.
        let r = HANDLE_PX * std::f64::consts::FRAC_1_SQRT_2;
        assert_eq!(hit_at(DVec2::new(r, r), Kind::Rotate), Some(2));
        assert_eq!(hit_at(DVec2::new(10.0, 10.0), Kind::Rotate), None);
    }

    #[test]
    fn axis_drag_moves_one_to_one_with_the_pointer() {
        let cam = Camera::new(ViewKind::Top);
        let c = cam.target;
        let w = cam.world_per_pixel(SIZE.y);
        let mm = axis_drag(&cam, c, 0, DVec2::new(100.0, 30.0), SIZE);
        assert!((mm - 100.0 * w).abs() < 1e-9, "only the drag along the axis counts");
        let mm = axis_drag(&cam, c, 1, DVec2::new(0.0, 50.0), SIZE);
        assert!((mm + 50.0 * w).abs() < 1e-9, "+Y is up, the screen is y down");
        assert_eq!(axis_drag(&cam, c, 2, DVec2::new(50.0, 50.0), SIZE), 0.0, "end-on");
    }

    #[test]
    fn segments_use_axis_colours_and_highlight_the_hot_handle() {
        let cam = Camera::new(ViewKind::Perspective);
        let plain = segments(Kind::Move, &cam, cam.target, SIZE.y, colors(), None, Color32::WHITE);
        assert_eq!(plain.len(), 15);
        assert_eq!(plain[0].color, Color32::RED);
        assert_eq!(plain[14].color, Color32::BLUE);
        let hot = segments(Kind::Move, &cam, cam.target, SIZE.y, colors(), Some(1), Color32::WHITE);
        assert_eq!(hot.iter().filter(|s| s.color == Color32::WHITE).count(), 5);
    }
}

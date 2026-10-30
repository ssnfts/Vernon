//! The viewport camera, shared by the app and the renderer: Z-up, millimetres.
//! Implements spec 3.1 (fit), 3.2 (orbit), 3.3 (pan) and 3.4 (wheel zoom to
//! cursor). State is `f64`; matrices are `f32`, made at upload.
//!
//! Screen coordinates are pixels from the viewport's top-left corner, x right,
//! y down. Camera space looks down -Z with +Y up.

use glam::{DMat3, DQuat, DVec2, DVec3, Mat4};
use vernon_core::Aabb;

/// Point of view of a viewport tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewKind {
    Top,
    Bottom,
    Front,
    Back,
    Left,
    Right,
    Perspective,
    /// Orthographic view at a free angle (an orbited standard view).
    User,
}

impl ViewKind {
    pub const MENU: [ViewKind; 8] = [
        ViewKind::Perspective,
        ViewKind::User,
        ViewKind::Top,
        ViewKind::Bottom,
        ViewKind::Front,
        ViewKind::Back,
        ViewKind::Left,
        ViewKind::Right,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ViewKind::Top => "Top",
            ViewKind::Bottom => "Bottom",
            ViewKind::Front => "Front",
            ViewKind::Back => "Back",
            ViewKind::Left => "Left",
            ViewKind::Right => "Right",
            ViewKind::Perspective => "Perspective",
            ViewKind::User => "Orthographic",
        }
    }

    pub fn shortcut(self) -> &'static str {
        match self {
            ViewKind::Top => "T",
            ViewKind::Bottom => "B",
            ViewKind::Front => "F",
            ViewKind::Left => "L",
            ViewKind::Perspective => "P",
            ViewKind::User => "U",
            _ => "",
        }
    }

    pub fn is_ortho(self) -> bool {
        !matches!(self, ViewKind::Perspective)
    }

    /// The axis-aligned direction from the target towards the eye, on the
    /// vehicle axes of the spec (x forward, y left, z up): Front looks at
    /// the nose from +X, Back at the tail from -X, Left at the left flank
    /// from +Y and Right at the right flank from -Y.
    pub fn back(self) -> DVec3 {
        match self {
            ViewKind::Top => DVec3::Z,
            ViewKind::Bottom => -DVec3::Z,
            ViewKind::Front => DVec3::X,
            ViewKind::Back => -DVec3::X,
            ViewKind::Left => DVec3::Y,
            ViewKind::Right => -DVec3::Y,
            ViewKind::Perspective | ViewKind::User => DVec3::new(1.0, -1.4, 0.9).normalize(),
        }
    }

    /// Camera-to-world rotation of the standard view.
    pub fn rotation(self) -> DQuat {
        let back = self.back();
        let (right, up) = match self {
            ViewKind::Top => (DVec3::X, DVec3::Y),
            ViewKind::Bottom => (DVec3::X, -DVec3::Y),
            _ => basis_from_back(back),
        };
        DQuat::from_mat3(&DMat3::from_cols(right, up, back)).normalize()
    }
}

/// Right and up vectors for a camera looking along `-back` with world +Z up.
pub fn basis_from_back(back: DVec3) -> (DVec3, DVec3) {
    let mut right = DVec3::Z.cross(back);
    if right.length_squared() < 1e-12 {
        right = DVec3::X;
    }
    let right = right.normalize();
    let up = back.cross(right).normalize();
    (right, up)
}

/// Plane the renderer's built-in home grid is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GridPlane {
    /// Ground plane (Z = 0).
    XY,
    /// Left/Right construction plane (Y = 0).
    XZ,
    /// Front/Back construction plane (X = 0).
    YZ,
}

/// Orbit behaviour for Alt + middle drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrbitMode {
    /// Yaw about world +Z, pitch clamped to +-89.9 degrees (keeps Z up).
    Turntable,
    /// Free trackball rotation (spec 3.2).
    Arcball,
}

pub const MIN_DISTANCE: f64 = 1e-3;
pub const MAX_DISTANCE: f64 = 1e8;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub target: DVec3,
    /// Camera-to-world rotation.
    pub rot: DQuat,
    /// Eye distance from the target along `back`.
    pub distance: f64,
    /// Vertical field of view in radians (perspective).
    pub fov_y: f64,
    pub ortho: bool,
    /// Half the visible height in mm (orthographic).
    pub half_height: f64,
}

impl Camera {
    pub fn new(kind: ViewKind) -> Self {
        Self {
            target: DVec3::new(0.0, 0.0, 50.0),
            rot: kind.rotation(),
            distance: 600.0,
            fov_y: 45f64.to_radians(),
            ortho: kind.is_ortho(),
            half_height: 250.0,
        }
    }

    /// Switch to a standard view, keeping target and zoom.
    pub fn set_view(&mut self, kind: ViewKind) {
        if kind != ViewKind::User {
            self.rot = kind.rotation();
        }
        if kind.is_ortho() && !self.ortho {
            self.half_height = self.distance * (self.fov_y * 0.5).tan();
        } else if !kind.is_ortho() && self.ortho {
            self.distance = (self.half_height / (self.fov_y * 0.5).tan()).max(MIN_DISTANCE);
        }
        self.ortho = kind.is_ortho();
    }

    pub fn right(&self) -> DVec3 {
        self.rot * DVec3::X
    }

    pub fn up(&self) -> DVec3 {
        self.rot * DVec3::Y
    }

    pub fn back(&self) -> DVec3 {
        self.rot * DVec3::Z
    }

    pub fn eye(&self) -> DVec3 {
        self.target + self.back() * self.distance
    }

    pub fn near(&self) -> f64 {
        (self.distance * 1e-3).max(1e-4)
    }

    /// World point to camera space.
    pub fn world_to_cam(&self, p: DVec3) -> DVec3 {
        self.rot.inverse() * (p - self.eye())
    }

    /// Camera-space point to pixels and view depth (positive in front).
    pub fn project_cam(&self, c: DVec3, size: DVec2) -> Option<(DVec2, f64)> {
        let depth = -c.z;
        let (sx, sy) = if self.ortho {
            (c.x / self.half_height, c.y / self.half_height)
        } else {
            if depth <= self.near() {
                return None;
            }
            let t = (self.fov_y * 0.5).tan();
            (c.x / (depth * t), c.y / (depth * t))
        };
        let half = size.y * 0.5;
        Some((DVec2::new(size.x * 0.5 + sx * half, half - sy * half), depth))
    }

    pub fn project(&self, p: DVec3, size: DVec2) -> Option<(DVec2, f64)> {
        self.project_cam(self.world_to_cam(p), size)
    }

    /// Ray through a pixel: (origin, unit direction).
    pub fn ray(&self, px: DVec2, size: DVec2) -> (DVec3, DVec3) {
        let half = size.y * 0.5;
        let sx = (px.x - size.x * 0.5) / half;
        let sy = (half - px.y) / half;
        if self.ortho {
            let o = self.eye() + self.right() * sx * self.half_height + self.up() * sy * self.half_height;
            (o, -self.back())
        } else {
            let t = (self.fov_y * 0.5).tan();
            let d = (self.right() * sx * t + self.up() * sy * t - self.back()).normalize();
            (self.eye(), d)
        }
    }

    /// Point under the pixel on the plane through the target facing the camera.
    pub fn focal_point(&self, px: DVec2, size: DVec2) -> DVec3 {
        let (o, d) = self.ray(px, size);
        let n = self.back();
        let denom = d.dot(n);
        if denom.abs() < 1e-12 {
            return self.target;
        }
        let t = (self.target - o).dot(n) / denom;
        o + d * t
    }

    /// Point under the pixel on a world plane `dot(n, p) = offset`.
    pub fn plane_point(&self, px: DVec2, size: DVec2, n: DVec3, offset: f64) -> Option<DVec3> {
        let (o, d) = self.ray(px, size);
        let denom = d.dot(n);
        if denom.abs() < 1e-9 {
            return None;
        }
        let t = (offset - o.dot(n)) / denom;
        (t > 0.0 || self.ortho).then(|| o + d * t)
    }

    /// World millimetres per pixel at the target depth (spec 3.3).
    pub fn world_per_pixel(&self, height_px: f64) -> f64 {
        let h = height_px.max(1.0);
        if self.ortho {
            2.0 * self.half_height / h
        } else {
            2.0 * self.distance * (self.fov_y * 0.5).tan() / h
        }
    }

    /// Pan by a pixel drag (spec 3.3): the scene follows the pointer.
    pub fn pan(&mut self, drag: DVec2, height_px: f64) {
        let w = self.world_per_pixel(height_px);
        self.target -= (self.right() * drag.x - self.up() * drag.y) * w;
    }

    /// Rotate the camera rig about `pivot` by the world rotation `w`.
    fn rotate_about(&mut self, w: DQuat, pivot: DVec3) {
        self.target = pivot + w * (self.target - pivot);
        self.rot = (w * self.rot).normalize();
    }

    /// Turntable orbit from a pixel drag: yaw about world Z, pitch clamped.
    pub fn orbit_turntable(&mut self, drag: DVec2, pivot: DVec3) {
        const K: f64 = 0.008;
        let limit = 89.9f64.to_radians();
        let elev = self.back().z.clamp(-1.0, 1.0).asin();
        let new_elev = (elev + drag.y * K).clamp(-limit, limit);
        let pitch = DQuat::from_axis_angle(self.right(), -(new_elev - elev));
        let yaw = DQuat::from_axis_angle(DVec3::Z, -drag.x * K);
        self.rotate_about(yaw * pitch, pivot);
    }

    /// Arcball orbit for a pointer move from `p0` to `p1` (pixels, spec 3.2).
    pub fn orbit_arcball(&mut self, p0: DVec2, p1: DVec2, size: DVec2, pivot: DVec3) {
        let r = size.x.min(size.y) * 0.5;
        let c = size * 0.5;
        let to_ball = |p: DVec2| arcball_point(DVec2::new(p.x - c.x, c.y - p.y), r);
        let q = arcball_quat(to_ball(p0), to_ball(p1));
        // q rotates the scene in camera space; the camera turns the other way.
        let w = self.rot * q.inverse() * self.rot.inverse();
        self.rotate_about(w.normalize(), pivot);
    }

    /// Wheel zoom about the point under the cursor (spec 3.4).
    pub fn zoom_at(&mut self, px: DVec2, size: DVec2, notches: f64) {
        let s = 1.1f64.powf(-notches);
        let a = self.focal_point(px, size);
        if self.ortho {
            let hh = self.half_height * s;
            if !(MIN_DISTANCE..=MAX_DISTANCE).contains(&hh) {
                return;
            }
            self.half_height = hh;
        } else {
            let d = self.distance * s;
            if !(MIN_DISTANCE..=MAX_DISTANCE).contains(&d) {
                return;
            }
            self.distance = d;
        }
        self.target = a + (self.target - a) * s;
    }

    /// Zoom region (Ctrl+W): fit the pixel rectangle `a`-`b` to the view,
    /// centred on its middle. Rectangles under 4 px are ignored.
    pub fn zoom_region(&mut self, a: DVec2, b: DVec2, size: DVec2) {
        let d = (b - a).abs();
        if d.x < 4.0 || d.y < 4.0 {
            return;
        }
        let c = (a + b) * 0.5;
        self.pan(size * 0.5 - c, size.y);
        let s = (d.x / size.x.max(1.0)).max(d.y / size.y.max(1.0)).clamp(1e-4, 1.0);
        if self.ortho {
            self.half_height = (self.half_height * s).max(MIN_DISTANCE);
        } else {
            self.distance = (self.distance * s).max(MIN_DISTANCE);
        }
    }

    /// World -> view matrix for upload (camera looks down its local -Z).
    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_to_rh(self.eye().as_vec3(), (-self.back()).as_vec3(), self.up().as_vec3())
    }

    /// View -> clip matrix for upload, reverse-Z (near = 1, far = 0): the
    /// vertical field of view (or ortho half height) spans the viewport
    /// height. `radius` is the distance from the target that must stay inside
    /// the orthographic depth range.
    pub fn proj_matrix(&self, aspect: f32, radius: f64) -> Mat4 {
        let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 };
        if self.ortho {
            let hh = self.half_height.max(1e-6) as f32;
            let hw = hh * aspect;
            let d = self.distance;
            let r = radius.max(1.0) + 1.0;
            Mat4::orthographic_rh(-hw, hw, -hh, hh, (d + r) as f32, (d - r) as f32)
        } else {
            Mat4::perspective_infinite_reverse_rh(self.fov_y as f32, aspect, self.near() as f32)
        }
    }

    /// Plane of the built-in home grid: the construction plane facing a
    /// head-on orthographic side view, else the ground.
    pub fn grid_plane(&self) -> GridPlane {
        let b = self.back().abs();
        if self.ortho && b.x > 0.999 {
            GridPlane::YZ
        } else if self.ortho && b.y > 0.999 {
            GridPlane::XZ
        } else {
            GridPlane::XY
        }
    }

    /// The camera a fraction `t` of the way to `to`: rotation by slerp, the
    /// rest linear. The projection type switches at once.
    pub fn lerp(&self, to: &Camera, t: f64) -> Camera {
        let t = t.clamp(0.0, 1.0);
        Camera {
            target: self.target.lerp(to.target, t),
            rot: self.rot.slerp(to.rot, t).normalize(),
            distance: self.distance + (to.distance - self.distance) * t,
            fov_y: self.fov_y + (to.fov_y - self.fov_y) * t,
            ortho: to.ortho,
            half_height: self.half_height + (to.half_height - self.half_height) * t,
        }
    }

    /// Frame a bounding box (spec 3.1). Empty boxes frame a 1000 mm cube.
    pub fn fit(&mut self, bounds: &Aabb, aspect: f64, margin: f64) {
        let (c, r) = if bounds.is_empty() {
            (DVec3::ZERO, 500.0 * 3f64.sqrt())
        } else {
            let r = (bounds.max - bounds.min).length() * 0.5;
            (bounds.center(), if r < 1e-6 { 1.0 } else { r })
        };
        let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 };
        let t = (self.fov_y * 0.5).tan();
        let te = t.min(aspect * t);
        self.target = c;
        self.distance = (margin * r / te.atan().sin()).clamp(MIN_DISTANCE, MAX_DISTANCE);
        // Half-height so that the sphere fits both the height and the width.
        self.half_height = margin * r * (1.0f64).max(1.0 / aspect);
    }
}

/// Map a centre-relative pixel (y up) onto the arcball sphere of radius `r`.
pub fn arcball_point(p: DVec2, r: f64) -> DVec3 {
    let v = p / r.max(1e-9);
    let l2 = v.length_squared();
    if l2 <= 1.0 {
        DVec3::new(v.x, v.y, (1.0 - l2).sqrt())
    } else {
        let n = v / l2.sqrt();
        DVec3::new(n.x, n.y, 0.0)
    }
}

/// Shoemake's arcball quaternion: rotates by twice the angle between points.
pub fn arcball_quat(p0: DVec3, p1: DVec3) -> DQuat {
    let v = p0.cross(p1);
    DQuat::from_xyzw(v.x, v.y, v.z, p0.dot(p1)).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Aabb {
        Aabb {
            min: DVec3::splat(-50.0),
            max: DVec3::splat(50.0),
        }
    }

    #[test]
    fn golden_zoom_extents() {
        let mut cam = Camera::new(ViewKind::Perspective);
        cam.fit(&cube(), 16.0 / 9.0, 1.0);
        assert!((cam.distance - 226.303).abs() < 1e-3, "{}", cam.distance);
        cam.fit(&cube(), 16.0 / 9.0, 1.05);
        assert!((cam.distance - 237.619).abs() < 1e-3);
        cam.fit(&cube(), 9.0 / 16.0, 1.0);
        assert!((cam.distance - 381.648).abs() < 1e-3);
        cam.fit(&cube(), 9.0 / 16.0, 1.05);
        assert!((cam.distance - 400.731).abs() < 1e-3);
    }

    #[test]
    fn golden_pan_scale() {
        let mut cam = Camera::new(ViewKind::Perspective);
        cam.fit(&cube(), 16.0 / 9.0, 1.0);
        let w = cam.world_per_pixel(1080.0);
        assert!((w - 0.17359).abs() < 1e-5);
        let t0 = cam.target;
        cam.pan(DVec2::new(100.0, 0.0), 1080.0);
        assert!(((cam.target - t0).length() - 17.359).abs() < 1e-3);
    }

    #[test]
    fn golden_arcball() {
        let p0 = arcball_point(DVec2::ZERO, 100.0);
        let p1 = arcball_point(DVec2::new(50.0, 0.0), 100.0);
        assert!((p1 - DVec3::new(0.5, 0.0, 0.75f64.sqrt())).length() < 1e-9);
        let (axis, angle) = arcball_quat(p0, p1).to_axis_angle();
        assert!((angle.to_degrees() - 60.0).abs() < 1e-6);
        assert!((axis - DVec3::Y).length() < 1e-9);
        let rim = arcball_point(DVec2::new(100.0, 0.0), 100.0);
        let (_, a) = arcball_quat(p0, rim).to_axis_angle();
        assert!((a.to_degrees() - 180.0).abs() < 1e-6);
        // Out and back returns the camera to where it was.
        let mut cam = Camera::new(ViewKind::Perspective);
        let start = cam;
        let size = DVec2::new(800.0, 600.0);
        cam.orbit_arcball(DVec2::new(400.0, 300.0), DVec2::new(520.0, 250.0), size, cam.target);
        cam.orbit_arcball(DVec2::new(520.0, 250.0), DVec2::new(400.0, 300.0), size, cam.target);
        assert!((cam.eye() - start.eye()).length() < 1e-9);
        assert!(cam.rot.abs_diff_eq(start.rot, 1e-9) || cam.rot.abs_diff_eq(-start.rot, 1e-9));
    }

    #[test]
    fn golden_wheel_zoom_keeps_anchor() {
        for kind in [ViewKind::Perspective, ViewKind::Front] {
            let mut cam = Camera::new(kind);
            let size = DVec2::new(1920.0, 1080.0);
            let px = DVec2::new(960.0 + 640.0, 540.0);
            let anchor = cam.focal_point(px, size);
            cam.zoom_at(px, size, 1.0);
            let (s, _) = cam.project(anchor, size).unwrap();
            assert!((s - px).length() < 0.5, "{kind:?}: {s:?}");
            cam.zoom_at(px, size, -3.0);
            let (s, _) = cam.project(anchor, size).unwrap();
            assert!((s - px).length() < 0.5);
        }
    }

    #[test]
    fn standard_views_are_z_up() {
        for kind in [ViewKind::Front, ViewKind::Back, ViewKind::Left, ViewKind::Right] {
            let cam = Camera::new(kind);
            assert!((cam.up() - DVec3::Z).length() < 1e-9, "{kind:?}");
        }
        let top = Camera::new(ViewKind::Top);
        assert!((top.up() - DVec3::Y).length() < 1e-9);
        assert!((top.right() - DVec3::X).length() < 1e-9);
        // Vehicle axes (x forward, y left): the nose looks at the Front
        // camera, whose screen right is the car's left (+Y); the Left camera
        // sees the car's nose on its screen left.
        assert!((ViewKind::Front.back() - DVec3::X).length() < 1e-9);
        assert!((ViewKind::Back.back() + DVec3::X).length() < 1e-9);
        assert!((ViewKind::Left.back() - DVec3::Y).length() < 1e-9);
        assert!((ViewKind::Right.back() + DVec3::Y).length() < 1e-9);
        let front = Camera::new(ViewKind::Front);
        assert!((front.right() - DVec3::Y).length() < 1e-9);
        let left = Camera::new(ViewKind::Left);
        assert!((left.right() + DVec3::X).length() < 1e-9);
        let right = Camera::new(ViewKind::Right);
        assert!((right.right() - DVec3::X).length() < 1e-9);
        // Projection centre and ray round trip.
        let cam = Camera::new(ViewKind::Perspective);
        let size = DVec2::new(640.0, 480.0);
        let (s, _) = cam.project(cam.target, size).unwrap();
        assert!((s - size * 0.5).length() < 1e-9);
        let (o, d) = cam.ray(DVec2::new(100.0, 50.0), size);
        let (s, _) = cam.project(o + d * 300.0, size).unwrap();
        assert!((s - DVec2::new(100.0, 50.0)).length() < 1e-6);
    }

    #[test]
    fn turntable_keeps_horizon_and_clamps_pitch() {
        let mut cam = Camera::new(ViewKind::Perspective);
        let pivot = cam.target;
        cam.orbit_turntable(DVec2::new(120.0, 40.0), pivot);
        assert!(cam.right().z.abs() < 1e-9);
        cam.orbit_turntable(DVec2::new(0.0, 1e5), pivot);
        assert!(cam.back().z < 1.0 && cam.back().z > 0.9999);
        assert!((cam.target - pivot).length() < 1e-9);
    }

    #[test]
    fn upload_matrices_match_the_projection_and_stay_in_depth_range() {
        for kind in [ViewKind::Perspective, ViewKind::Top, ViewKind::Front] {
            let mut cam = Camera::new(kind);
            cam.target = DVec3::new(10.0, -20.0, 5.0);
            let size = DVec2::new(800.0, 600.0);
            let p = DVec3::new(40.0, 10.0, 30.0);
            let (px, _) = cam.project(p, size).unwrap();
            let clip = cam.proj_matrix(800.0 / 600.0, 500.0)
                * cam.view_matrix()
                * glam::Vec4::new(p.x as f32, p.y as f32, p.z as f32, 1.0);
            let ndc = clip.truncate() / clip.w;
            let sx = (ndc.x as f64 + 1.0) * 0.5 * size.x;
            let sy = (1.0 - ndc.y as f64) * 0.5 * size.y;
            assert!((sx - px.x).abs() < 0.05 && (sy - px.y).abs() < 0.05, "{kind:?}: {sx},{sy} vs {px:?}");
            assert!((0.0..=1.0).contains(&ndc.z), "{kind:?}: depth {}", ndc.z);
        }
    }

    #[test]
    fn grid_plane_follows_the_view() {
        assert_eq!(Camera::new(ViewKind::Top).grid_plane(), GridPlane::XY);
        assert_eq!(Camera::new(ViewKind::Front).grid_plane(), GridPlane::YZ);
        assert_eq!(Camera::new(ViewKind::Left).grid_plane(), GridPlane::XZ);
        assert_eq!(Camera::new(ViewKind::Perspective).grid_plane(), GridPlane::XY);
    }

    #[test]
    fn lerp_runs_from_one_camera_to_the_other() {
        let a = Camera::new(ViewKind::Front);
        let mut b = Camera::new(ViewKind::Left);
        b.target = DVec3::new(100.0, 0.0, 0.0);
        b.distance = 900.0;
        assert_eq!(a.lerp(&b, 0.0).back(), a.back());
        assert!((a.lerp(&b, 1.0).back() - b.back()).length() < 1e-12);
        let m = a.lerp(&b, 0.5);
        assert!((m.back().angle_between(a.back()).to_degrees() - 45.0).abs() < 1e-9);
        assert_eq!(m.target, DVec3::new(50.0, 0.0, 25.0));
        assert_eq!(m.distance, 750.0);
    }

    #[test]
    fn zoom_region_frames_the_rectangle() {
        let size = DVec2::new(800.0, 600.0);
        let mut cam = Camera::new(ViewKind::Top);
        let (t0, h0) = (cam.target, cam.half_height);
        // The middle quarter: same centre, half the height.
        cam.zoom_region(DVec2::new(200.0, 150.0), DVec2::new(600.0, 450.0), size);
        assert!((cam.target - t0).length() < 1e-9);
        assert!((cam.half_height - h0 * 0.5).abs() < 1e-9);
        // An off-centre rectangle: the point under its middle ends up in
        // the middle of the view.
        let p = cam.focal_point(DVec2::new(700.0, 100.0), size);
        cam.zoom_region(DVec2::new(650.0, 50.0), DVec2::new(750.0, 150.0), size);
        assert!((cam.focal_point(size * 0.5, size) - p).length() < 1e-6);
        // A click is not a region.
        let before = cam;
        cam.zoom_region(DVec2::new(10.0, 10.0), DVec2::new(11.0, 12.0), size);
        assert_eq!(cam, before);
    }
}

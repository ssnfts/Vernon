//! Orbit camera maths (pure, no GPU). Z-up, millimetres, right-handed.
//!
//! The camera orbits a `target` point at `distance`. `yaw` rotates about the
//! world Z axis and `pitch` tilts above (+) or below (-) the XY plane:
//!
//! The axis views follow the vehicle axes of the spec (x forward, y left,
//! z up), the same as the application's own viewport camera:
//!
//! | yaw   | pitch | looks from  | screen right | screen up |
//! |-------|-------|-------------|--------------|-----------|
//! | 90    | 0     | +X (Front)  | +Y           | +Z        |
//! | -90   | 0     | -X (Back)   | -Y           | +Z        |
//! | 180   | 0     | +Y (Left)   | -X           | +Z        |
//! | 0     | 0     | -Y (Right)  | +X           | +Z        |
//! | 0     | 90    | +Z (Top)    | +X           | +Y        |
//! | 0     | -90   | -Z (Bottom) | +X           | -Y        |
//!
//! The basis is built from yaw directly, so looking straight down is not a
//! singularity. Projections use reverse-Z (near = 1, far = 0) for depth
//! precision across a whole car at millimetre scale.

use glam::{DVec3, Mat4, Vec2, Vec3, Vec4Swizzles};
use vernon_core::Aabb;

/// One of the six axis-aligned orthographic views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewAxis {
    Top,
    Bottom,
    Front,
    Back,
    Left,
    Right,
}

impl ViewAxis {
    /// (yaw, pitch) in radians for this view.
    pub fn angles(self) -> (f32, f32) {
        use std::f32::consts::{FRAC_PI_2, PI};
        match self {
            ViewAxis::Top => (0.0, FRAC_PI_2),
            ViewAxis::Bottom => (0.0, -FRAC_PI_2),
            ViewAxis::Front => (FRAC_PI_2, 0.0),
            ViewAxis::Back => (-FRAC_PI_2, 0.0),
            ViewAxis::Left => (PI, 0.0),
            ViewAxis::Right => (0.0, 0.0),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ViewAxis::Top => "Top",
            ViewAxis::Bottom => "Bottom",
            ViewAxis::Front => "Front",
            ViewAxis::Back => "Back",
            ViewAxis::Left => "Left",
            ViewAxis::Right => "Right",
        }
    }
}

/// Viewport presets, as in the reference DCC's viewport label menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewPreset {
    Top,
    Bottom,
    Front,
    Back,
    Left,
    Right,
    Perspective,
}

impl ViewPreset {
    pub fn axis(self) -> Option<ViewAxis> {
        match self {
            ViewPreset::Top => Some(ViewAxis::Top),
            ViewPreset::Bottom => Some(ViewAxis::Bottom),
            ViewPreset::Front => Some(ViewAxis::Front),
            ViewPreset::Back => Some(ViewAxis::Back),
            ViewPreset::Left => Some(ViewAxis::Left),
            ViewPreset::Right => Some(ViewAxis::Right),
            ViewPreset::Perspective => None,
        }
    }
}

/// Plane the home grid is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GridPlane {
    /// Ground plane (Z = 0). Used by Top/Bottom/Perspective/user views.
    XY,
    /// Left/Right construction plane (Y = 0).
    XZ,
    /// Front/Back construction plane (X = 0).
    YZ,
}

/// Default vertical field of view (radians).
pub const DEFAULT_FOV_Y: f32 = 45.0 * std::f32::consts::PI / 180.0;

/// Orbit camera. All lengths in millimetres, angles in radians.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// Orbit centre (world, mm).
    pub target: Vec3,
    /// Distance from eye to target (mm). In orthographic mode it sets the
    /// zoom: the view is `2 * distance * tan(fov_y / 2)` mm tall.
    pub distance: f32,
    /// Rotation about world +Z (radians). 0 = looking from -Y (Right view).
    pub yaw: f32,
    /// Elevation above the XY plane (radians), clamped to [-pi/2, pi/2].
    pub pitch: f32,
    /// Orthographic projection when true.
    pub ortho: bool,
    /// The axis view this camera is locked to, if any. Orbiting clears it
    /// (an axis view becomes a free "Orthographic" user view).
    pub ortho_axis: Option<ViewAxis>,
    /// Vertical field of view (radians).
    pub fov_y: f32,
    /// Approximate scene radius (mm); sets the orthographic depth range.
    pub scene_radius: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self::preset(ViewPreset::Perspective)
    }
}

impl Camera {
    /// A camera for a preset view, looking at the origin from 1 m.
    pub fn preset(p: ViewPreset) -> Self {
        let mut c = Camera {
            target: Vec3::ZERO,
            distance: 1000.0,
            yaw: 0.0,
            pitch: 0.0,
            ortho: false,
            ortho_axis: None,
            fov_y: DEFAULT_FOV_Y,
            scene_radius: 1000.0,
        };
        c.set_preset(p);
        c
    }

    /// Switch to a preset view, keeping target and distance.
    pub fn set_preset(&mut self, p: ViewPreset) {
        match p.axis() {
            Some(axis) => {
                let (y, pt) = axis.angles();
                self.yaw = y;
                self.pitch = pt;
                self.ortho = true;
                self.ortho_axis = Some(axis);
            }
            None => {
                self.yaw = (-45.0f32).to_radians();
                self.pitch = 30.0f32.to_radians();
                self.ortho = false;
                self.ortho_axis = None;
            }
        }
    }

    /// Viewport label in the reference DCC's style.
    pub fn label(&self) -> &'static str {
        match (self.ortho, self.ortho_axis) {
            (true, Some(a)) => a.label(),
            (true, None) => "Orthographic",
            (false, _) => "Perspective",
        }
    }

    /// Unit vector from the target towards the eye.
    pub fn eye_dir(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(sy * cp, -cy * cp, sp)
    }

    /// Eye position (world, mm).
    pub fn eye(&self) -> Vec3 {
        self.target + self.eye_dir() * self.distance
    }

    /// Unit view direction (eye towards target).
    pub fn forward(&self) -> Vec3 {
        -self.eye_dir()
    }

    /// Unit screen-right vector (always horizontal).
    pub fn right(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        Vec3::new(cy, sy, 0.0)
    }

    /// Unit screen-up vector.
    pub fn up(&self) -> Vec3 {
        self.right().cross(self.forward()).normalize()
    }

    /// World -> view matrix (right-handed, camera looks down -Z).
    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_to_rh(self.eye(), self.forward(), self.up())
    }

    /// Half the visible height (mm) at the target distance.
    pub fn half_height_at_target(&self) -> f32 {
        self.distance * (self.fov_y * 0.5).tan()
    }

    /// View -> clip matrix (reverse-Z: near maps to depth 1, far to 0).
    pub fn proj_matrix(&self, aspect: f32) -> Mat4 {
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        let d = self.distance.max(1e-6);
        if self.ortho {
            let hh = self.half_height_at_target().max(1e-6);
            let hw = hh * aspect;
            let range = 2.0 * self.scene_radius.max(1.0) + d;
            // Swap near/far for reverse-Z.
            Mat4::orthographic_rh(-hw, hw, -hh, hh, d + range, d - range)
        } else {
            let near = (d * 2e-3).max(1e-3);
            Mat4::perspective_infinite_reverse_rh(self.fov_y, aspect, near)
        }
    }

    /// `proj * view`.
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.proj_matrix(aspect) * self.view_matrix()
    }

    /// Plane the home grid should be drawn in for this view.
    pub fn grid_plane(&self) -> GridPlane {
        match (self.ortho, self.ortho_axis) {
            (true, Some(ViewAxis::Front | ViewAxis::Back)) => GridPlane::YZ,
            (true, Some(ViewAxis::Left | ViewAxis::Right)) => GridPlane::XZ,
            _ => GridPlane::XY,
        }
    }

    /// Orbit about the target by yaw/pitch deltas (radians). An axis view
    /// becomes a free orthographic view; perspective stays perspective.
    pub fn orbit(&mut self, d_yaw: f32, d_pitch: f32) {
        use std::f32::consts::{FRAC_PI_2, TAU};
        if d_yaw == 0.0 && d_pitch == 0.0 {
            return;
        }
        self.yaw = (self.yaw + d_yaw).rem_euclid(TAU);
        if self.yaw > std::f32::consts::PI {
            self.yaw -= TAU;
        }
        self.pitch = (self.pitch + d_pitch).clamp(-FRAC_PI_2, FRAC_PI_2);
        self.ortho_axis = None;
    }

    /// Orbit from a mouse drag in pixels (0.5 degree per pixel). Dragging
    /// right spins the scene right; dragging down tilts the view up.
    pub fn orbit_pixels(&mut self, dx: f32, dy: f32) {
        let k = 0.5f32.to_radians();
        self.orbit(-dx * k, dy * k);
    }

    /// World millimetres per screen pixel at the target distance.
    pub fn world_per_pixel(&self, viewport_height_px: f32) -> f32 {
        2.0 * self.half_height_at_target() / viewport_height_px.max(1.0)
    }

    /// Pan from a mouse drag in pixels (egui screen coords, y down). The
    /// scene follows the cursor at the target depth.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_height_px: f32) {
        let s = self.world_per_pixel(viewport_height_px);
        self.target += (-self.right() * dx + self.up() * dy) * s;
    }

    /// Multiply the distance by `factor` (< 1 zooms in).
    pub fn zoom(&mut self, factor: f32) {
        if factor.is_finite() && factor > 0.0 {
            self.distance = (self.distance * factor).clamp(1e-3, 1e9);
        }
    }

    /// Zoom from a scroll amount in points (positive = zoom in).
    pub fn zoom_scroll(&mut self, scroll: f32) {
        self.zoom((-scroll * 0.0025).exp());
    }

    /// Zoom by `factor` (< 1 zooms in) keeping the point of the target plane
    /// under the cursor fixed on screen. `cursor` is relative to the
    /// viewport's top-left corner, `size` in the same units. Exact for both
    /// perspective and orthographic views.
    pub fn zoom_at(&mut self, factor: f32, cursor: Vec2, size: Vec2) {
        if !(factor.is_finite() && factor > 0.0) {
            return;
        }
        let size = size.max(Vec2::ONE);
        let ndc = Vec2::new(cursor.x / size.x * 2.0 - 1.0, 1.0 - cursor.y / size.y * 2.0);
        let before = self.half_height_at_target();
        self.zoom(factor);
        let shrink = before - self.half_height_at_target();
        let aspect = size.x / size.y;
        self.target += (self.right() * (ndc.x * aspect) + self.up() * ndc.y) * shrink;
    }

    /// Frame a bounding box: centre it and pick a distance so its bounding
    /// sphere fits the viewport for both perspective and orthographic modes.
    /// Empty boxes are ignored.
    pub fn zoom_extents(&mut self, bounds: &Aabb, aspect: f32) {
        if bounds.is_empty() {
            return;
        }
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        let c = bounds.center();
        let r = ((bounds.size().length() * 0.5) as f32).max(0.5);
        let margin = 1.05;
        let tan_y = (self.fov_y * 0.5).tan();
        let tan_x = tan_y * aspect;
        // Tight fit: project the eight box corners onto the camera axes
        // (relative to the box centre) instead of fitting the bounding sphere.
        let (right, up, fwd) = (self.right(), self.up(), self.forward());
        let (mut ex, mut ey) = (0.0f32, 0.0f32);
        let mut dist = 0.0f32;
        let mut z_min = f32::INFINITY;
        for i in 0..8 {
            let corner = DVec3::new(
                if i & 1 == 0 {
                    bounds.min.x
                } else {
                    bounds.max.x
                },
                if i & 2 == 0 {
                    bounds.min.y
                } else {
                    bounds.max.y
                },
                if i & 4 == 0 {
                    bounds.min.z
                } else {
                    bounds.max.z
                },
            ) - c;
            let o = Vec3::new(corner.x as f32, corner.y as f32, corner.z as f32);
            let (x, y, z) = (o.dot(right).abs(), o.dot(up).abs(), o.dot(fwd));
            ex = ex.max(x);
            ey = ey.max(y);
            z_min = z_min.min(z);
            // Perspective: the corner at depth `d + z` must satisfy
            // `x / ((d + z) tan_x) <= 1 / margin`, likewise for y.
            dist = dist.max(margin * x / tan_x - z).max(margin * y / tan_y - z);
        }
        self.distance = if self.ortho {
            // Orthographic: the visible half-height is `distance * tan_y`.
            margin * ey.max(ex / aspect) / tan_y
        } else {
            // Keep the nearest corner clearly in front of the eye.
            dist.max(0.1 * r - z_min)
        };
        self.distance = self.distance.clamp(1e-3, 1e9);
        self.target = Vec3::new(c.x as f32, c.y as f32, c.z as f32);
        self.scene_radius = r;
    }

    /// World-space ray through a pixel: (origin, unit direction).
    /// `pos` is relative to the viewport's top-left corner, `size` in the
    /// same units.
    pub fn screen_ray(&self, pos: Vec2, size: Vec2) -> (Vec3, Vec3) {
        let size = size.max(Vec2::ONE);
        let ndc = Vec2::new(pos.x / size.x * 2.0 - 1.0, 1.0 - pos.y / size.y * 2.0);
        let inv = self.view_proj(size.x / size.y).inverse();
        let near = inv * glam::Vec4::new(ndc.x, ndc.y, 1.0, 1.0);
        let mid = inv * glam::Vec4::new(ndc.x, ndc.y, 0.5, 1.0);
        let a = near.xyz() / near.w;
        let b = mid.xyz() / mid.w;
        let dir = (b - a).normalize_or_zero();
        let dir = if dir == Vec3::ZERO {
            self.forward()
        } else {
            dir
        };
        (a, dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{DVec3, Vec4};

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-5
    }

    #[test]
    fn axis_views_look_down_correct_axes() {
        let cases = [
            (ViewPreset::Top, Vec3::NEG_Z, Vec3::X, Vec3::Y),
            (ViewPreset::Bottom, Vec3::Z, Vec3::X, Vec3::NEG_Y),
            (ViewPreset::Front, Vec3::NEG_X, Vec3::Y, Vec3::Z),
            (ViewPreset::Back, Vec3::X, Vec3::NEG_Y, Vec3::Z),
            (ViewPreset::Left, Vec3::NEG_Y, Vec3::NEG_X, Vec3::Z),
            (ViewPreset::Right, Vec3::Y, Vec3::X, Vec3::Z),
        ];
        for (p, fwd, right, up) in cases {
            let c = Camera::preset(p);
            assert!(c.ortho, "{p:?} should be orthographic");
            assert!(close(c.forward(), fwd), "{p:?} forward {:?}", c.forward());
            assert!(close(c.right(), right), "{p:?} right {:?}", c.right());
            assert!(close(c.up(), up), "{p:?} up {:?}", c.up());
            // The view matrix maps the world axes onto view axes: right -> +X,
            // up -> +Y, forward -> -Z.
            let v = c.view_matrix();
            assert!(close(v.transform_vector3(right), Vec3::X), "{p:?}");
            assert!(close(v.transform_vector3(up), Vec3::Y), "{p:?}");
            assert!(close(v.transform_vector3(fwd), Vec3::NEG_Z), "{p:?}");
            assert!(close(
                v.transform_point3(c.target),
                Vec3::new(0.0, 0.0, -c.distance)
            ));
            assert_eq!(c.label(), p.axis().unwrap().label());
        }
        let p = Camera::preset(ViewPreset::Perspective);
        assert!(!p.ortho);
        assert_eq!(p.label(), "Perspective");
        assert!(
            p.forward().z < 0.0,
            "perspective looks down onto the ground"
        );
    }

    #[test]
    fn top_view_projection_maps_world_axes_to_screen() {
        let c = Camera::preset(ViewPreset::Top);
        let vp = c.view_proj(1.0);
        let px = vp.project_point3(Vec3::new(100.0, 0.0, 0.0));
        let py = vp.project_point3(Vec3::new(0.0, 100.0, 0.0));
        assert!(px.x > 0.0 && px.y.abs() < 1e-5);
        assert!(py.y > 0.0 && py.x.abs() < 1e-5);
        // Reverse-Z: closer to the eye (higher Z) => larger depth.
        let hi = vp.project_point3(Vec3::new(0.0, 0.0, 50.0));
        let lo = vp.project_point3(Vec3::new(0.0, 0.0, -50.0));
        assert!(hi.z > lo.z);
        assert!((0.0..=1.0).contains(&hi.z) && (0.0..=1.0).contains(&lo.z));
    }

    #[test]
    fn grid_plane_follows_view() {
        assert_eq!(Camera::preset(ViewPreset::Top).grid_plane(), GridPlane::XY);
        assert_eq!(
            Camera::preset(ViewPreset::Front).grid_plane(),
            GridPlane::YZ
        );
        assert_eq!(Camera::preset(ViewPreset::Left).grid_plane(), GridPlane::XZ);
        assert_eq!(
            Camera::preset(ViewPreset::Perspective).grid_plane(),
            GridPlane::XY
        );
    }

    fn corners(b: &Aabb) -> Vec<Vec3> {
        let mut out = Vec::new();
        for i in 0..8 {
            let x = if i & 1 == 0 { b.min.x } else { b.max.x };
            let y = if i & 2 == 0 { b.min.y } else { b.max.y };
            let z = if i & 4 == 0 { b.min.z } else { b.max.z };
            out.push(Vec3::new(x as f32, y as f32, z as f32));
        }
        out
    }

    #[test]
    fn zoom_extents_fits_box() {
        let b = Aabb {
            min: DVec3::new(-2300.0, -900.0, 0.0),
            max: DVec3::new(2300.0, 900.0, 1300.0),
        };
        let presets = [
            ViewPreset::Perspective,
            ViewPreset::Top,
            ViewPreset::Front,
            ViewPreset::Left,
            ViewPreset::Back,
        ];
        for aspect in [0.5f32, 1.0, 16.0 / 9.0] {
            for p in presets {
                let mut c = Camera::preset(p);
                c.target = Vec3::new(5e4, -3e4, 7e3);
                c.distance = 1.0;
                c.zoom_extents(&b, aspect);
                let vp = c.view_proj(aspect);
                let mut max_extent = 0.0f32;
                for corner in corners(&b) {
                    let clip = vp * Vec4::new(corner.x, corner.y, corner.z, 1.0);
                    assert!(clip.w > 0.0, "{p:?} corner behind the eye");
                    let ndc = clip.xyz() / clip.w;
                    assert!(
                        ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0,
                        "{p:?} aspect {aspect}: {ndc:?} outside view"
                    );
                    assert!((0.0..=1.0).contains(&ndc.z), "{p:?} depth {}", ndc.z);
                    max_extent = max_extent.max(ndc.x.abs()).max(ndc.y.abs());
                }
                // Not absurdly far away either.
                assert!(
                    max_extent > 0.35,
                    "{p:?} aspect {aspect}: box too small ({max_extent})"
                );
                assert!(close(c.target, Vec3::new(0.0, 0.0, 650.0)));
            }
        }
        // Empty box leaves the camera untouched.
        let mut c = Camera::default();
        let before = c;
        c.zoom_extents(&Aabb::empty(), 1.0);
        assert_eq!(c, before);
    }

    #[test]
    fn orbit_pan_zoom() {
        let mut c = Camera::preset(ViewPreset::Top);
        c.orbit(0.1, -0.2);
        assert!(c.ortho && c.ortho_axis.is_none());
        assert_eq!(c.label(), "Orthographic");
        c.orbit(0.0, 10.0);
        assert!((c.pitch - std::f32::consts::FRAC_PI_2).abs() < 1e-6);

        // Pan in the Front view (looking along -X, screen right = +Y):
        // dragging right moves the target to -Y, dragging down moves it to
        // +Z (scene follows the cursor).
        let mut f = Camera::preset(ViewPreset::Front);
        f.pan(10.0, 0.0, 500.0);
        assert!(f.target.y < 0.0 && f.target.x.abs() < 1e-4 && f.target.z.abs() < 1e-4);
        let mut f = Camera::preset(ViewPreset::Front);
        f.pan(0.0, 10.0, 500.0);
        assert!(f.target.z > 0.0);
        let wpp = f.world_per_pixel(500.0);
        assert!((f.target.z - 10.0 * wpp).abs() < 1e-3);

        let mut z = Camera::default();
        z.zoom(0.5);
        assert!((z.distance - 500.0).abs() < 1e-3);
        z.zoom(-1.0);
        assert!((z.distance - 500.0).abs() < 1e-3);
        z.zoom_scroll(100.0);
        assert!(z.distance < 500.0);
    }

    /// World point on the target plane under a pixel.
    fn plane_point(c: &Camera, px: Vec2, size: Vec2) -> Vec3 {
        let (o, d) = c.screen_ray(px, size);
        let t = (c.target - o).dot(c.forward()) / d.dot(c.forward());
        o + d * t
    }

    #[test]
    fn zoom_at_keeps_cursor_point_fixed() {
        let size = Vec2::new(800.0, 600.0);
        let cursor = Vec2::new(620.0, 140.0);
        for p in [ViewPreset::Perspective, ViewPreset::Top, ViewPreset::Front] {
            let mut c = Camera::preset(p);
            c.target = Vec3::new(10.0, -20.0, 30.0);
            c.distance = 500.0;
            let before = plane_point(&c, cursor, size);
            c.zoom_at(0.5, cursor, size);
            assert!((c.distance - 250.0).abs() < 1e-3, "{p:?}");
            let after = plane_point(&c, cursor, size);
            assert!(
                (before - after).length() < 1e-2,
                "{p:?}: {before:?} vs {after:?}"
            );
        }
        // Non-finite factors are ignored.
        let mut c = Camera::default();
        let before = c;
        c.zoom_at(f32::NAN, cursor, size);
        assert_eq!(c, before);
    }

    #[test]
    fn zoom_extents_is_tight_for_flat_views() {
        // A long flat box seen from the front fills most of the viewport.
        let b = Aabb {
            min: DVec3::new(-2300.0, -900.0, 0.0),
            max: DVec3::new(2300.0, 900.0, 1300.0),
        };
        for ortho in [false, true] {
            let mut c = Camera::preset(ViewPreset::Front);
            c.ortho = ortho;
            c.zoom_extents(&b, 16.0 / 9.0);
            let vp = c.view_proj(16.0 / 9.0);
            let mut ext = 0.0f32;
            for corner in corners(&b) {
                let clip = vp * Vec4::new(corner.x, corner.y, corner.z, 1.0);
                let ndc = clip.xyz() / clip.w;
                ext = ext.max(ndc.x.abs()).max(ndc.y.abs());
            }
            assert!((0.9..=1.0).contains(&ext), "ortho {ortho}: extent {ext}");
        }
    }

    #[test]
    fn screen_ray_through_centre_hits_target() {
        for p in [ViewPreset::Perspective, ViewPreset::Top, ViewPreset::Left] {
            let mut c = Camera::preset(p);
            c.target = Vec3::new(10.0, 20.0, 30.0);
            let (o, d) = c.screen_ray(Vec2::new(400.0, 300.0), Vec2::new(800.0, 600.0));
            assert!(close(d, c.forward()), "{p:?} dir {d:?}");
            // Distance from target to the ray line is ~0.
            let t = (c.target - o).dot(d);
            assert!(((o + d * t) - c.target).length() < 1e-2, "{p:?}");
        }
    }
}

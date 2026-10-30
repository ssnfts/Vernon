//! View cube (spec 2.4, 3.6): 26 clickable regions (6 faces, 12 edges, 8
//! corners), the view each gives, picking from the cube's screen position,
//! drag snapping, roll, the compass ring, the stored front view and the
//! transition easing. Pure functions; the app paints the cube in egui.

use glam::{DMat3, DQuat, DVec2, DVec3, IVec3};

use crate::camera::{basis_from_back, ViewKind};

/// Edge band of a face, as a fraction of the half-width (spec 3.6).
pub const BAND: f64 = 0.3;
/// A drag ending within this angle of a region direction snaps to it.
pub const SNAP_DEG: f64 = 10.0;
/// Cube opacity in inactive viewports.
pub const INACTIVE_OPACITY: f32 = 0.5;

/// Cube size setting: 0 Large, 1 Normal, 2 Small, 3 Tiny. Half-width in
/// pixels.
pub fn size_px(size: u8) -> f32 {
    [30.0, 22.0, 16.0, 11.0][usize::from(size.min(3))]
}

/// The 26 region directions: every sign triple except (0, 0, 0).
pub fn regions() -> Vec<IVec3> {
    let mut out = Vec::with_capacity(26);
    for x in -1..=1 {
        for y in -1..=1 {
            for z in -1..=1 {
                if (x, y, z) != (0, 0, 0) {
                    out.push(IVec3::new(x, y, z));
                }
            }
        }
    }
    out
}

/// Region kind by the number of non-zero signs: 1 face, 2 edge, 3 corner.
pub fn order(s: IVec3) -> usize {
    (s.x != 0) as usize + (s.y != 0) as usize + (s.z != 0) as usize
}

/// Region of a point `h` on the surface of the cube of half-width 1
/// (spec 3.6): the face axis keeps its sign, other axes count when they
/// are within the band of the face edge.
pub fn region_of(h: DVec3, band: f64) -> IVec3 {
    let a = h.abs();
    let face = if a.x >= a.y && a.x >= a.z {
        0
    } else if a.y >= a.z {
        1
    } else {
        2
    };
    let mut s = IVec3::ZERO;
    for i in 0..3 {
        if i == face || a[i] > 1.0 - band {
            s[i] = if h[i] >= 0.0 { 1 } else { -1 };
        }
    }
    s
}

/// First hit of the ray `o + t d` with the cube [-1, 1]^3 (slab method).
pub fn hit(o: DVec3, d: DVec3) -> Option<DVec3> {
    let (mut t0, mut t1) = (f64::NEG_INFINITY, f64::INFINITY);
    for i in 0..3 {
        if d[i].abs() < 1e-12 {
            if o[i].abs() > 1.0 {
                return None;
            }
            continue;
        }
        let (a, b) = ((-1.0 - o[i]) / d[i], (1.0 - o[i]) / d[i]);
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
    }
    (t0 <= t1 && t1 >= 0.0).then(|| o + d * t0.max(0.0))
}

/// Region under a pointer `p` (pixels from the cube centre, y down) for a
/// cube drawn with half-width `scale` pixels by a camera with
/// camera-to-world rotation `rot` (orthographic view of the world axes).
pub fn pick(p: DVec2, scale: f64, rot: DQuat) -> Option<IVec3> {
    let o_cam = DVec3::new(p.x / scale, -p.y / scale, 4.0);
    hit(rot * o_cam, rot * -DVec3::Z).map(|h| region_of(h, BAND))
}

/// Camera-to-world rotation that looks at the target from direction `s`
/// (the camera sits along `s`). Top and bottom keep +Y / -Y up like the
/// named views; every other region keeps world +Z up.
pub fn rotation_for(s: IVec3) -> DQuat {
    let back = s.as_dvec3().normalize();
    let (right, up) = match (s.x, s.y, s.z) {
        (0, 0, 1) => (DVec3::X, DVec3::Y),
        (0, 0, -1) => (DVec3::X, -DVec3::Y),
        _ => basis_from_back(back),
    };
    DQuat::from_mat3(&DMat3::from_cols(right, up, back)).normalize()
}

/// The named view of a face region.
pub fn face_view(s: IVec3) -> Option<ViewKind> {
    Some(match (s.x, s.y, s.z) {
        (0, 0, 1) => ViewKind::Top,
        (0, 0, -1) => ViewKind::Bottom,
        // Vehicle axes: x forward, y left.
        (1, 0, 0) => ViewKind::Front,
        (-1, 0, 0) => ViewKind::Back,
        (0, 1, 0) => ViewKind::Left,
        (0, -1, 0) => ViewKind::Right,
        _ => return None,
    })
}

/// Label of a face region.
pub fn face_label(s: IVec3) -> &'static str {
    match face_view(s) {
        Some(ViewKind::Top) => "TOP",
        Some(ViewKind::Bottom) => "BOTTOM",
        Some(ViewKind::Front) => "FRONT",
        Some(ViewKind::Back) => "BACK",
        Some(ViewKind::Left) => "LEFT",
        Some(ViewKind::Right) => "RIGHT",
        _ => "",
    }
}

/// The region direction nearest to the view direction `back` when it is
/// within `max_deg` (drag snapping).
pub fn snap(back: DVec3, max_deg: f64) -> Option<IVec3> {
    let b = back.normalize();
    let (s, ang) = regions()
        .into_iter()
        .map(|s| (s, b.angle_between(s.as_dvec3().normalize()).to_degrees()))
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    (ang <= max_deg).then_some(s)
}

/// The face the camera looks straight at, if any (a face view).
pub fn facing(rot: DQuat) -> Option<IVec3> {
    let b = rot * DVec3::Z;
    let s = IVec3::new(b.x.round() as i32, b.y.round() as i32, b.z.round() as i32);
    (order(s) == 1 && (s.as_dvec3() - b).length() < 1e-6).then_some(s)
}

/// Roll the view 90 degrees about the view axis; `clockwise` as the image
/// turns.
pub fn roll(rot: DQuat, clockwise: bool) -> DQuat {
    let a = std::f64::consts::FRAC_PI_2 * if clockwise { 1.0 } else { -1.0 };
    (rot * DQuat::from_rotation_z(a)).normalize()
}

/// In a face view, the face beside the current one in screen direction
/// `dir` (0 up, 1 right, 2 down, 3 left).
pub fn neighbour(rot: DQuat, dir: usize) -> IVec3 {
    let v = match dir % 4 {
        0 => rot * DVec3::Y,
        1 => rot * DVec3::X,
        2 => -(rot * DVec3::Y),
        _ => -(rot * DVec3::X),
    };
    IVec3::new(v.x.round() as i32, v.y.round() as i32, v.z.round() as i32)
}

/// Length of an animated view change, in seconds (spec 3.6).
pub const TRANSITION_S: f64 = 0.3;
/// Compass ring radius in cube half-widths, and its height under the cube.
pub const COMPASS_RADIUS: f64 = 1.7;
const COMPASS_Z: f64 = -1.0;
/// Pointer tolerance of the ring and its letters, in pixels.
const COMPASS_TOL: f64 = 5.0;

/// Ease in-out over `t` in 0..1 (smoothstep).
pub fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The rotation that takes cube axes to the world when the camera rotation
/// `rot` is stored as the Front view; identity for the standard Front.
pub fn front_frame(rot: DQuat) -> DQuat {
    (rot * ViewKind::Front.rotation().inverse()).normalize()
}

/// The four compass directions N, E, S, W on the cube's ground plane. North
/// is +X (the nose) at a north angle of 0 and turns counter-clockwise, seen
/// from above, as the angle grows.
pub fn compass_dirs(north_deg: f64) -> [DVec3; 4] {
    let n = DQuat::from_rotation_z(north_deg.to_radians()) * DVec3::X;
    [n, DVec3::new(n.y, -n.x, 0.0), -n, DVec3::new(-n.y, n.x, 0.0)]
}

/// Pixels from the cube centre (y down) of a point in cube space, for a cube
/// of half-width `scale` seen with relative rotation `rot`.
pub fn to_px(v: DVec3, scale: f64, rot: DQuat) -> DVec2 {
    let c = rot.inverse() * v;
    DVec2::new(c.x, -c.y) * scale
}

/// A point of the compass ring at angle `a` (radians) in cube space.
pub fn compass_point(a: f64) -> DVec3 {
    DVec3::new(a.cos(), a.sin(), 0.0) * COMPASS_RADIUS + DVec3::new(0.0, 0.0, COMPASS_Z)
}

/// A compass letter's position in cube space.
pub fn compass_letter(dir: DVec3) -> DVec3 {
    dir * (COMPASS_RADIUS + 0.28) + DVec3::new(0.0, 0.0, COMPASS_Z)
}

/// What the pointer is over on the compass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompassHit {
    /// A letter: index into [`compass_dirs`].
    Letter(usize),
    /// The ring itself: dragging it turns the view.
    Ring,
}

/// Compass part under a pointer `p` (pixels from the cube centre, y down).
pub fn compass_hit(p: DVec2, scale: f64, rot: DQuat, north_deg: f64) -> Option<CompassHit> {
    let dirs = compass_dirs(north_deg);
    if let Some(k) = (0..4).find(|k| to_px(compass_letter(dirs[*k]), scale, rot).distance(p) < COMPASS_TOL + 2.0) {
        return Some(CompassHit::Letter(k));
    }
    (0..64)
        .map(|k| to_px(compass_point(k as f64 / 64.0 * std::f64::consts::TAU), scale, rot).distance(p))
        .any(|d| d < COMPASS_TOL)
        .then_some(CompassHit::Ring)
}

/// Turn about the cube's up axis so the camera looks from compass direction
/// `dir`, keeping its elevation and roll.
pub fn look_from_heading(rot: DQuat, dir: DVec3) -> DQuat {
    let back = rot * DVec3::Z;
    // Straight down or up there is no horizontal view direction: what is at
    // the bottom of the screen is the side the camera looks from.
    let h = if back.x.hypot(back.y) > 1e-3 { back } else { -(rot * DVec3::Y) };
    let delta = dir.y.atan2(dir.x) - h.y.atan2(h.x);
    (DQuat::from_rotation_z(delta) * rot).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deg(a: IVec3, b: IVec3) -> f64 {
        a.as_dvec3().angle_between(b.as_dvec3()).to_degrees()
    }

    /// Spec golden 6.
    #[test]
    fn golden_view_cube() {
        let r = regions();
        assert_eq!(r.len(), 26);
        let count = |k| r.iter().filter(|s| order(**s) == k).count();
        assert_eq!((count(1), count(2), count(3)), (6, 12, 8));
        let (top, edge, corner) = (IVec3::Z, IVec3::new(1, 0, 1), IVec3::ONE);
        assert!((deg(top, IVec3::X) - 90.0).abs() < 1e-9);
        assert!((deg(top, edge) - 45.0).abs() < 1e-9);
        assert!((deg(top, corner) - 54.7356).abs() < 1e-4);
        assert_eq!(region_of(DVec3::new(0.2, 0.1, 1.0), BAND), top);
        assert_eq!(face_view(top), Some(ViewKind::Top));
        assert!((rotation_for(top) * DVec3::Y - DVec3::Y).length() < 1e-12, "top view has +Y up");
        assert_eq!(region_of(DVec3::new(0.8, 0.0, 1.0), BAND), edge);
        assert_eq!(region_of(DVec3::new(0.8, 0.8, 1.0), BAND), corner);
        // Roll: 90 degrees per click, four clicks are the identity.
        let q0 = rotation_for(IVec3::NEG_Y);
        let q1 = roll(q0, true);
        assert!(((q1 * DVec3::Y).angle_between(q0 * DVec3::Y).to_degrees() - 90.0).abs() < 1e-9);
        assert!(((q1 * DVec3::Z) - (q0 * DVec3::Z)).length() < 1e-12, "roll keeps the view axis");
        let q4 = (0..4).fold(q0, |q, _| roll(q, true));
        assert!(q4.dot(q0).abs() > 1.0 - 1e-12);
        // Sizes strictly decreasing, inactive opacity 0.5.
        assert!((0..3).all(|i| size_px(i) > size_px(i + 1)));
        assert_eq!(INACTIVE_OPACITY, 0.5);
    }

    #[test]
    fn stored_front_view_maps_the_cube_onto_it() {
        assert!(front_frame(ViewKind::Front.rotation()).dot(DQuat::IDENTITY).abs() > 1.0 - 1e-12);
        // Storing the Left view as Front: the cube's +X now points at +Y and
        // its Front region gives back exactly that view; Top stays on top.
        let left = ViewKind::Left.rotation();
        let f = front_frame(left);
        assert!((f * DVec3::X - DVec3::Y).length() < 1e-12);
        assert!((f * DVec3::Z - DVec3::Z).length() < 1e-12);
        assert!((f * rotation_for(IVec3::X)).dot(left).abs() > 1.0 - 1e-12);
        // Seen through the frame the stored view reads as the Front face.
        assert_eq!(facing(f.inverse() * left), Some(IVec3::X));
    }

    #[test]
    fn compass_directions_letters_and_headings() {
        let [n, e, s, w] = compass_dirs(0.0);
        assert!((n - DVec3::X).length() < 1e-12);
        assert!((e + DVec3::Y).length() < 1e-12, "east is clockwise of north from above");
        assert!((s + DVec3::X).length() < 1e-12);
        assert!((w - DVec3::Y).length() < 1e-12);
        assert!((compass_dirs(90.0)[0] - DVec3::Y).length() < 1e-12);

        // From the top view, north (+X) is on the right of the screen.
        let top = ViewKind::Top.rotation();
        let at = to_px(compass_letter(n), 20.0, top);
        assert!(at.x > 30.0 && at.y.abs() < 1e-9);
        assert_eq!(compass_hit(at, 20.0, top, 0.0), Some(CompassHit::Letter(0)));
        let ring = to_px(compass_point(0.7), 20.0, top);
        assert_eq!(compass_hit(ring, 20.0, top, 0.0), Some(CompassHit::Ring));
        assert_eq!(compass_hit(DVec2::ZERO, 20.0, top, 0.0), None);

        // Clicking a letter looks from that side and keeps the elevation.
        let q = rotation_for(IVec3::new(1, 0, 1));
        let r = look_from_heading(q, DVec3::Y);
        assert!((r * DVec3::Z - DVec3::new(0.0, 1.0, 1.0).normalize()).length() < 1e-12);
        // From above, the chosen side comes to the bottom of the screen.
        let r = look_from_heading(top, DVec3::X);
        assert!((r * DVec3::Y + DVec3::X).length() < 1e-12);
        assert!((r * DVec3::Z - DVec3::Z).length() < 1e-12);
    }

    #[test]
    fn transition_easing_is_symmetric_and_clamped() {
        assert_eq!(TRANSITION_S, 0.3);
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert_eq!(ease(0.5), 0.5);
        assert_eq!(ease(2.0), 1.0);
        assert!(ease(0.25) < 0.25 && ease(0.75) > 0.75, "slow at both ends");
    }

    #[test]
    fn named_faces_match_the_standard_views_and_picking_works() {
        for s in regions().into_iter().filter(|s| order(*s) == 1) {
            let v = face_view(s).unwrap();
            assert!(rotation_for(s).dot(v.rotation()).abs() > 1.0 - 1e-9, "{v:?}");
        }
        // Seen from the top, the cube centre is the top face; a pointer
        // near the right edge is the (+X, +Z) edge; near the upper right
        // corner (y down on screen) the (+X, +Y, +Z) corner.
        let rot = ViewKind::Top.rotation();
        assert_eq!(pick(DVec2::ZERO, 20.0, rot), Some(IVec3::Z));
        assert_eq!(pick(DVec2::new(18.0, 0.0), 20.0, rot), Some(IVec3::new(1, 0, 1)));
        assert_eq!(pick(DVec2::new(18.0, -18.0), 20.0, rot), Some(IVec3::ONE));
        assert_eq!(pick(DVec2::new(40.0, 0.0), 20.0, rot), None);
        assert_eq!(facing(rot), Some(IVec3::Z));
        assert_eq!(facing(rotation_for(IVec3::ONE)), None);
        // From the front (the nose, +X), the face above is the top and the
        // right is the car's left (+Y).
        let front = ViewKind::Front.rotation();
        assert_eq!(neighbour(front, 0), IVec3::Z);
        assert_eq!(neighbour(front, 1), IVec3::Y);
        assert_eq!(face_view(IVec3::X), Some(ViewKind::Front));
        assert_eq!(face_view(IVec3::Y), Some(ViewKind::Left));
        // Snapping.
        let near = (DVec3::ONE.normalize() + DVec3::new(0.05, 0.0, 0.0)).normalize();
        assert_eq!(snap(near, SNAP_DEG), Some(IVec3::ONE));
        assert_eq!(snap(DVec3::new(1.0, 0.45, 0.0), SNAP_DEG), None, "between a face and an edge");
    }
}

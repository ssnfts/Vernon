//! Uniform rectangular cantilever hook geometry in a seam frame.
//!
//! The beam root-to-catch length is the strain length. A short return shoulder
//! reaches the catch, followed by the specified lead-in ramp and nose. The
//! mating recess includes lateral clearance on the deflection side, so the
//! beam can move through the narrow entry before the catch seats.

use crate::joints::{JointShape, Kind, Prism2};
use glam::DVec2;

#[derive(Clone, Copy)]
pub struct HookDimensions {
    pub width: f64,
    pub thickness: f64,
    pub length: f64,
    pub undercut: f64,
    pub lead_angle_deg: f64,
    pub flex_room: f64,
    pub clearance: f64,
}

impl HookDimensions {
    pub fn validate(&self) -> Result<(), &'static str> {
        let values = [
            self.width,
            self.thickness,
            self.length,
            self.undercut,
            self.lead_angle_deg,
            self.flex_room,
            self.clearance,
        ];
        if values.iter().any(|v| !v.is_finite() || *v <= 0.0) {
            return Err("snap dimensions must be finite and positive");
        }
        if self.width > 30.0
            || self.thickness > 5.0
            || self.length > 30.0
            || self.undercut > 2.0
            || self.flex_room > 3.0
            || self.clearance > 0.4
        {
            return Err("snap dimensions exceed the supported feature-size limits");
        }
        if !(5.0..=80.0).contains(&self.lead_angle_deg) {
            return Err("snap lead angle must be between 5 and 80 degrees");
        }
        if self.flex_room < self.undercut {
            return Err("flex room must cover the full catch undercut");
        }
        if self.undercut / 75.0_f64.to_radians().tan() >= self.length {
            return Err("the return shoulder is longer than the flexing arm");
        }
        Ok(())
    }

    pub fn strain_pct(&self) -> f64 {
        100.0 * 1.5 * self.thickness * self.undercut / self.length.powi(2)
    }
}

fn rectangle(c: DVec2, half_width: f64, low: f64, high: f64) -> Vec<DVec2> {
    vec![
        c + DVec2::new(-half_width, low),
        c + DVec2::new(half_width, low),
        c + DVec2::new(half_width, high),
        c + DVec2::new(-half_width, high),
    ]
}

pub fn hook_shapes(c: DVec2, d: HookDimensions) -> JointShape {
    let half_t = d.thickness * 0.5;
    let shoulder = d.undercut / 75.0_f64.to_radians().tan();
    let ramp = d.undercut / d.lead_angle_deg.to_radians().tan();
    let male = |extra| rectangle(c, d.width * 0.5, -half_t, half_t + extra);
    let female = |extra| {
        rectangle(
            c,
            d.width * 0.5 + d.clearance,
            -half_t - d.flex_room - d.clearance,
            half_t + d.clearance + extra,
        )
    };
    JointShape {
        a: Prism2 {
            base: male(0.0),
            top: male(0.0),
            depth: d.length + ramp,
            intermediate: vec![
                (d.length - shoulder, male(0.0)),
                (d.length, male(d.undercut)),
            ],
        },
        a_kind: Kind::Protrude,
        b: Prism2 {
            base: female(0.0),
            top: female(0.0),
            depth: d.length + ramp + d.clearance,
            intermediate: vec![
                (d.length - shoulder, female(0.0)),
                (d.length, female(d.undercut)),
            ],
        },
        b_kind: Kind::Pocket,
        pin: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dimensions() -> HookDimensions {
        HookDimensions {
            width: 8.0,
            thickness: 1.2,
            length: 8.0,
            undercut: 0.4,
            lead_angle_deg: 35.0,
            flex_room: 0.6,
            clearance: 0.15,
        }
    }

    #[test]
    fn hook_sections_have_a_real_catch_and_flex_clearance() {
        let d = dimensions();
        let shape = hook_shapes(DVec2::ZERO, d);
        let high = |r: &[DVec2]| r.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
        let low = |r: &[DVec2]| r.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        assert!((high(&shape.a.intermediate[1].1) - high(&shape.a.base) - 0.4).abs() < 1e-12);
        assert_eq!(shape.a.intermediate[1].0, 8.0);
        assert!((low(&shape.a.base) - low(&shape.b.base) - 0.75).abs() < 1e-12);
        let ramp = shape.a.depth - shape.a.intermediate[1].0;
        assert!((0.4 / ramp - 35.0_f64.to_radians().tan()).abs() < 1e-12);
        assert!((d.strain_pct() - 1.125).abs() < 1e-12);
        let mut longer = d;
        longer.length *= 2.0;
        assert!((longer.strain_pct() - d.strain_pct() / 4.0).abs() < 1e-12);
    }
}

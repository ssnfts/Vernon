//! Numeric transform type-in (spec 2.9, 3.11): the status-bar X/Y/Z fields
//! and the F12 dialog. Absolute sets the value (p' = entry); Offset adds the
//! entry (p' = p + entry) and the field goes back to its neutral value.
//! Rotation is in degrees, scale in percent (uniform: any axis sets it, and
//! an offset multiplies, so its neutral value is 100).

use glam::DVec3;
use vernon_graph::{ParamValue, Params};

use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Absolute,
    Offset,
}

/// The transform channel a tool types into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Translate,
    Rotate,
    Scale,
}

impl Channel {
    pub fn unit(self) -> &'static str {
        match self {
            Channel::Translate => "mm",
            Channel::Rotate => "deg",
            Channel::Scale => "%",
        }
    }
}

/// The values of one transform node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Xform {
    pub translate: DVec3,
    pub rotate: DVec3,
    pub scale: f64,
}

impl Default for Xform {
    fn default() -> Self {
        Self {
            translate: DVec3::ZERO,
            rotate: DVec3::ZERO,
            scale: 1.0,
        }
    }
}

impl Xform {
    pub fn of(p: &Params) -> Self {
        Self {
            translate: p.vec3("translate").unwrap_or_default(),
            rotate: p.vec3("rotate").unwrap_or_default(),
            scale: p.float("scale").unwrap_or(1.0),
        }
    }
}

/// What the three fields show: the values in Absolute mode, the neutral
/// entry (0, or 100 % for scale) in Offset mode.
pub fn readout(ch: Channel, x: &Xform, mode: Mode) -> DVec3 {
    match (mode, ch) {
        (Mode::Offset, Channel::Scale) => DVec3::splat(100.0),
        (Mode::Offset, _) => DVec3::ZERO,
        (Mode::Absolute, Channel::Translate) => x.translate,
        (Mode::Absolute, Channel::Rotate) => x.rotate,
        (Mode::Absolute, Channel::Scale) => DVec3::splat(x.scale * 100.0),
    }
}

/// Apply `value` typed into field `axis` (0 = X, 1 = Y, 2 = Z).
pub fn apply(ch: Channel, mode: Mode, mut x: Xform, axis: usize, value: f64) -> Xform {
    let axis = axis.min(2);
    match (ch, mode) {
        (Channel::Translate, Mode::Absolute) => x.translate[axis] = value,
        (Channel::Translate, Mode::Offset) => x.translate[axis] += value,
        (Channel::Rotate, Mode::Absolute) => x.rotate[axis] = value,
        (Channel::Rotate, Mode::Offset) => x.rotate[axis] += value,
        (Channel::Scale, Mode::Absolute) => x.scale = value / 100.0,
        (Channel::Scale, Mode::Offset) => x.scale *= value / 100.0,
    }
    x.scale = x.scale.clamp(0.001, 1000.0);
    x
}

impl AppState {
    /// The transform values the fields show for the primary selection:
    /// its own when it is a transform node, identity otherwise. `None`
    /// with nothing selected (the fields are disabled).
    pub fn primary_xform(&self) -> Option<Xform> {
        let p = self.primary()?;
        let n = self.graph.nodes.get(&p)?;
        Some(if n.type_id == "geo.transform" { Xform::of(&n.params) } else { Xform::default() })
    }

    /// Type a value into field `axis` for every selected object: each gets
    /// a transform on top of its stack when it has none. Repeated entries
    /// into one field within a short time share an undo step.
    pub fn type_in(&mut self, ch: Channel, mode: Mode, axis: usize, value: f64, now: f64) -> Result<(), String> {
        if self.selection.is_empty() {
            return Err("select an object to type a transform".into());
        }
        if !value.is_finite() {
            return Err("not a number".into());
        }
        if ch == Channel::Scale && value <= 0.0 {
            return Err("scale must be above 0 %".into());
        }
        self.checkpoint_coalesced(&format!("typein:{ch:?}:{mode:?}:{axis}"), now);
        for obj in self.selection.clone() {
            if self.scene.locked.contains(&obj) {
                continue;
            }
            let t = self.ensure_transform(obj)?;
            let x = apply(ch, mode, Xform::of(&self.graph.nodes[&t].params), axis, value);
            self.set_param_silent(t, "translate", ParamValue::Vec3(x.translate));
            self.set_param_silent(t, "rotate", ParamValue::Vec3(x.rotate));
            self.set_param_silent(t, "scale", ParamValue::Float(x.scale));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::build_registry;

    /// Spec golden 11: object at (10, 20, 30); offset 5 on X gives
    /// (15, 20, 30) and the field returns to 0; absolute 5 on X gives
    /// (5, 20, 30); nothing selected gives a disabled readout.
    #[test]
    fn golden_numeric_transform() {
        let mut st = AppState::new(build_registry());
        assert!(st.primary_xform().is_none(), "disabled with nothing selected");
        assert!(st.type_in(Channel::Translate, Mode::Absolute, 0, 5.0, 0.0).is_err());

        let b = st.create("geo.box").unwrap();
        let t = st.ensure_transform(b).unwrap();
        st.set_param(t, "translate", ParamValue::Vec3(DVec3::new(10.0, 20.0, 30.0)), 0.0);
        st.select_only(t);
        let at = |st: &AppState| st.graph.nodes[&t].params.vec3("translate").unwrap();

        st.type_in(Channel::Translate, Mode::Offset, 0, 5.0, 10.0).unwrap();
        assert_eq!(at(&st), DVec3::new(15.0, 20.0, 30.0));
        let x = st.primary_xform().unwrap();
        assert_eq!(readout(Channel::Translate, &x, Mode::Offset), DVec3::ZERO);

        st.type_in(Channel::Translate, Mode::Absolute, 0, 5.0, 20.0).unwrap();
        assert_eq!(at(&st), DVec3::new(5.0, 20.0, 30.0));
        assert_eq!(readout(Channel::Translate, &st.primary_xform().unwrap(), Mode::Absolute), DVec3::new(5.0, 20.0, 30.0));

        // One undo step per entry.
        assert!(st.undo());
        assert_eq!(at(&st), DVec3::new(15.0, 20.0, 30.0));
    }

    #[test]
    fn type_in_adds_a_transform_and_handles_rotate_and_scale() {
        let mut st = AppState::new(build_registry());
        let b = st.create("geo.box").unwrap();
        st.type_in(Channel::Rotate, Mode::Absolute, 2, 90.0, 0.0).unwrap();
        let t = st.primary().unwrap();
        assert_ne!(t, b, "a transform went on top of the box");
        assert_eq!(st.graph.nodes[&t].type_id, "geo.transform");
        st.type_in(Channel::Rotate, Mode::Offset, 2, 45.0, 10.0).unwrap();
        assert_eq!(st.primary_xform().unwrap().rotate, DVec3::new(0.0, 0.0, 135.0));
        st.type_in(Channel::Scale, Mode::Absolute, 0, 200.0, 20.0).unwrap();
        st.type_in(Channel::Scale, Mode::Offset, 1, 50.0, 30.0).unwrap();
        assert!((st.primary_xform().unwrap().scale - 1.0).abs() < 1e-12);
        assert_eq!(readout(Channel::Scale, &st.primary_xform().unwrap(), Mode::Offset), DVec3::splat(100.0));
        assert!(st.type_in(Channel::Scale, Mode::Absolute, 0, 0.0, 40.0).is_err());
    }
}

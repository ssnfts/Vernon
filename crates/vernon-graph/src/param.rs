use glam::DVec3;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{CookError, CookResult};

/// What kind of control a parameter shows and what values it accepts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ParamKind {
    Float { default: f64, min: f64, max: f64 },
    Int { default: i64, min: i64, max: i64 },
    Bool { default: bool },
    Vec3 { default: DVec3 },
    Str { default: String },
    /// A file path; `extensions` like `["stl", "obj"]` filter the picker.
    /// `save = true` shows a save dialog instead of an open dialog.
    Path { default: String, extensions: Vec<String>, save: bool },
    /// A choice from fixed options; the value is the option string.
    Enum { default: String, options: Vec<String> },
}

/// Declares one parameter of a node type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamSpec {
    /// Stable machine name, e.g. `max_iterations`.
    pub name: String,
    /// Human label, e.g. `Max iterations`.
    pub label: String,
    pub kind: ParamKind,
    /// Display unit, e.g. `mm`, `deg`, `°C`, `W/m²`.
    pub unit: Option<String>,
    /// One-line tooltip.
    pub help: String,
    /// A one-shot action (start an external program, upload a file): the
    /// app resets it after starting a cook and never saves it switched on,
    /// so a re-cook, an undo or opening a file cannot start the program again.
    /// The graph keeps a result produced with the action on until an input
    /// or another parameter changes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub action: bool,
}

impl ParamSpec {
    fn base(name: &str, label: &str, kind: ParamKind) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            kind,
            unit: None,
            help: String::new(),
            action: false,
        }
    }

    pub fn float(name: &str, label: &str, default: f64, min: f64, max: f64) -> Self {
        Self::base(name, label, ParamKind::Float { default, min, max })
    }

    pub fn int(name: &str, label: &str, default: i64, min: i64, max: i64) -> Self {
        Self::base(name, label, ParamKind::Int { default, min, max })
    }

    pub fn bool(name: &str, label: &str, default: bool) -> Self {
        Self::base(name, label, ParamKind::Bool { default })
    }

    pub fn vec3(name: &str, label: &str, default: DVec3) -> Self {
        Self::base(name, label, ParamKind::Vec3 { default })
    }

    pub fn string(name: &str, label: &str, default: &str) -> Self {
        Self::base(name, label, ParamKind::Str { default: default.into() })
    }

    pub fn path(name: &str, label: &str, extensions: &[&str], save: bool) -> Self {
        Self::base(
            name,
            label,
            ParamKind::Path {
                default: String::new(),
                extensions: extensions.iter().map(|s| s.to_string()).collect(),
                save,
            },
        )
    }

    pub fn choice(name: &str, label: &str, default: &str, options: &[&str]) -> Self {
        Self::base(
            name,
            label,
            ParamKind::Enum {
                default: default.into(),
                options: options.iter().map(|s| s.to_string()).collect(),
            },
        )
    }

    pub fn unit(mut self, unit: &str) -> Self {
        self.unit = Some(vernon_core::units::canonical(unit).into_owned());
        self
    }

    pub fn help(mut self, help: &str) -> Self {
        self.help = help.into();
        self
    }

    /// Mark a Bool parameter as a one-shot action (see [`ParamSpec::action`]).
    pub fn action(mut self) -> Self {
        self.action = true;
        self
    }

    pub fn default_value(&self) -> ParamValue {
        match &self.kind {
            ParamKind::Float { default, .. } => ParamValue::Float(*default),
            ParamKind::Int { default, .. } => ParamValue::Int(*default),
            ParamKind::Bool { default } => ParamValue::Bool(*default),
            ParamKind::Vec3 { default } => ParamValue::Vec3(*default),
            ParamKind::Str { default } => ParamValue::Str(default.clone()),
            ParamKind::Path { default, .. } => ParamValue::Str(default.clone()),
            ParamKind::Enum { default, .. } => ParamValue::Str(default.clone()),
        }
    }
}

/// A concrete parameter value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Vec3(DVec3),
    Str(String),
}

/// Parameter values of one node instance, keyed by `ParamSpec::name`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Params(pub IndexMap<String, ParamValue>);

impl Params {
    pub fn from_specs(specs: &[ParamSpec]) -> Self {
        Params(
            specs
                .iter()
                .map(|s| (s.name.clone(), s.default_value()))
                .collect(),
        )
    }

    pub fn set(&mut self, name: &str, value: ParamValue) {
        self.0.insert(name.to_string(), value);
    }

    pub fn get(&self, name: &str) -> Option<&ParamValue> {
        self.0.get(name)
    }

    /// Feed every name and value except those in `skip` into `state`
    /// (floats by bit pattern), for cache signatures.
    pub fn hash_into<H: std::hash::Hasher>(&self, state: &mut H, skip: &dyn Fn(&str) -> bool) {
        use std::hash::Hash;
        for (name, value) in &self.0 {
            if skip(name) {
                continue;
            }
            name.hash(state);
            match value {
                ParamValue::Bool(b) => (0u8, *b).hash(state),
                ParamValue::Int(i) => (1u8, *i).hash(state),
                ParamValue::Float(f) => (2u8, f.to_bits()).hash(state),
                ParamValue::Vec3(v) => (3u8, v.x.to_bits(), v.y.to_bits(), v.z.to_bits()).hash(state),
                ParamValue::Str(s) => (4u8, s.as_str()).hash(state),
            }
        }
    }

    pub fn float(&self, name: &str) -> CookResult<f64> {
        match self.0.get(name) {
            Some(ParamValue::Float(v)) => Ok(*v),
            Some(ParamValue::Int(v)) => Ok(*v as f64),
            _ => Err(CookError::BadParam(name.into())),
        }
    }

    pub fn int(&self, name: &str) -> CookResult<i64> {
        match self.0.get(name) {
            Some(ParamValue::Int(v)) => Ok(*v),
            Some(ParamValue::Float(v)) => Ok(v.round() as i64),
            _ => Err(CookError::BadParam(name.into())),
        }
    }

    pub fn bool(&self, name: &str) -> CookResult<bool> {
        match self.0.get(name) {
            Some(ParamValue::Bool(v)) => Ok(*v),
            Some(ParamValue::Int(v)) => Ok(*v != 0),
            _ => Err(CookError::BadParam(name.into())),
        }
    }

    pub fn vec3(&self, name: &str) -> CookResult<DVec3> {
        match self.0.get(name) {
            Some(ParamValue::Vec3(v)) => Ok(*v),
            _ => Err(CookError::BadParam(name.into())),
        }
    }

    pub fn str(&self, name: &str) -> CookResult<&str> {
        match self.0.get(name) {
            Some(ParamValue::Str(v)) => Ok(v),
            _ => Err(CookError::BadParam(name.into())),
        }
    }
}

/// Copy of `params` with every numeric value forced into its spec range:
/// Float and Int values are clamped to `[min, max]` and a non-finite Float
/// falls back to the default. Values without a spec, or of another kind,
/// are kept. A hand-edited file therefore cannot push a node outside the
/// range it declared.
pub fn clamp_to_specs(specs: &[ParamSpec], params: &Params) -> Params {
    let mut out = params.clone();
    for spec in specs {
        let Some(v) = out.0.get_mut(&spec.name) else {
            continue;
        };
        let fixed = match (&spec.kind, &*v) {
            (ParamKind::Float { default, min, max }, ParamValue::Float(x)) => {
                let x = if x.is_finite() { *x } else { *default };
                ParamValue::Float(if min <= max { x.clamp(*min, *max) } else { x })
            }
            (ParamKind::Float { min, max, .. }, ParamValue::Int(i)) if min <= max => {
                let f = *i as f64;
                if f < *min || f > *max {
                    ParamValue::Float(f.clamp(*min, *max))
                } else {
                    ParamValue::Int(*i)
                }
            }
            (ParamKind::Int { min, max, .. }, ParamValue::Int(i)) if min <= max => ParamValue::Int((*i).clamp(*min, *max)),
            (ParamKind::Int { default, min, max }, ParamValue::Float(f)) => {
                let i = if f.is_finite() { f.round() as i64 } else { *default };
                ParamValue::Int(if min <= max { i.clamp(*min, *max) } else { i })
            }
            _ => continue,
        };
        *v = fixed;
    }
    out
}

/// Names of the numeric parameters of `params` outside their spec range
/// (or not finite): what [`clamp_to_specs`] would change, for a warning
/// when a file is opened.
pub fn out_of_range(specs: &[ParamSpec], params: &Params) -> Vec<String> {
    let clamped = clamp_to_specs(specs, params);
    specs
        .iter()
        .filter(|s| match (params.get(&s.name), clamped.get(&s.name)) {
            (Some(ParamValue::Float(a)), Some(ParamValue::Float(b))) => a.to_bits() != b.to_bits(),
            (Some(a), Some(b)) => a != b,
            _ => false,
        })
        .map(|s| s.name.clone())
        .collect()
}

#[cfg(test)]
mod clamp_tests {
    use super::*;

    #[test]
    fn numeric_params_are_forced_into_their_range() {
        let specs = vec![
            ParamSpec::float("clip", "Clip", 2.0, 0.0, 20.0),
            ParamSpec::int("digits", "Digits", 2, 1, 6),
            ParamSpec::string("name", "Name", "a"),
        ];
        let mut p = Params::from_specs(&specs);
        p.set("clip", ParamValue::Float(1e30));
        p.set("digits", ParamValue::Int(i64::MAX));
        p.set("name", ParamValue::Str("x".into()));
        let c = clamp_to_specs(&specs, &p);
        assert_eq!(c.float("clip").unwrap(), 20.0);
        assert_eq!(c.int("digits").unwrap(), 6);
        assert_eq!(c.str("name").unwrap(), "x");
        p.set("clip", ParamValue::Float(f64::NAN));
        p.set("digits", ParamValue::Float(-1e300));
        let c = clamp_to_specs(&specs, &p);
        assert_eq!(c.float("clip").unwrap(), 2.0);
        assert_eq!(c.int("digits").unwrap(), 1);
        p.set("clip", ParamValue::Float(-1.0));
        assert_eq!(clamp_to_specs(&specs, &p).float("clip").unwrap(), 0.0);
        p.set("clip", ParamValue::Float(7.5));
        assert_eq!(clamp_to_specs(&specs, &p).float("clip").unwrap(), 7.5);
        assert_eq!(out_of_range(&specs, &p), vec!["digits".to_string()]);
        p.set("digits", ParamValue::Int(3));
        assert!(out_of_range(&specs, &p).is_empty());
    }
}

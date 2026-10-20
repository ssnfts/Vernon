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
}

impl ParamSpec {
    fn base(name: &str, label: &str, kind: ParamKind) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            kind,
            unit: None,
            help: String::new(),
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
        self.unit = Some(unit.into());
        self
    }

    pub fn help(mut self, help: &str) -> Self {
        self.help = help.into();
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

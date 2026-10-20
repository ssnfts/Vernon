use glam::DVec3;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{GeoError, GeoResult};

/// The element type of an attribute array.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttribType {
    Float,
    Int,
    Vec3,
    Str,
}

/// A column of attribute values, one per element (point, prim, or one for detail).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AttribArray {
    Float(Vec<f64>),
    Int(Vec<i64>),
    Vec3(Vec<DVec3>),
    Str(Vec<String>),
}

impl AttribArray {
    pub fn new(ty: AttribType, len: usize) -> Self {
        match ty {
            AttribType::Float => AttribArray::Float(vec![0.0; len]),
            AttribType::Int => AttribArray::Int(vec![0; len]),
            AttribType::Vec3 => AttribArray::Vec3(vec![DVec3::ZERO; len]),
            AttribType::Str => AttribArray::Str(vec![String::new(); len]),
        }
    }

    pub fn ty(&self) -> AttribType {
        match self {
            AttribArray::Float(_) => AttribType::Float,
            AttribArray::Int(_) => AttribType::Int,
            AttribArray::Vec3(_) => AttribType::Vec3,
            AttribArray::Str(_) => AttribType::Str,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            AttribArray::Float(v) => v.len(),
            AttribArray::Int(v) => v.len(),
            AttribArray::Vec3(v) => v.len(),
            AttribArray::Str(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Resize, filling new slots with the type's zero value.
    pub fn resize(&mut self, len: usize) {
        match self {
            AttribArray::Float(v) => v.resize(len, 0.0),
            AttribArray::Int(v) => v.resize(len, 0),
            AttribArray::Vec3(v) => v.resize(len, DVec3::ZERO),
            AttribArray::Str(v) => v.resize(len, String::new()),
        }
    }

    /// Build a new array by picking elements at `indices` (used when filtering
    /// or reordering elements).
    pub fn gather(&self, indices: &[usize]) -> Self {
        match self {
            AttribArray::Float(v) => AttribArray::Float(indices.iter().map(|&i| v[i]).collect()),
            AttribArray::Int(v) => AttribArray::Int(indices.iter().map(|&i| v[i]).collect()),
            AttribArray::Vec3(v) => AttribArray::Vec3(indices.iter().map(|&i| v[i]).collect()),
            AttribArray::Str(v) => AttribArray::Str(indices.iter().map(|&i| v[i].clone()).collect()),
        }
    }

    /// Append another array of the same type. Mismatched types pad with zeros.
    pub fn append(&mut self, other: &AttribArray, other_len: usize) {
        match (self, other) {
            (AttribArray::Float(a), AttribArray::Float(b)) => a.extend_from_slice(b),
            (AttribArray::Int(a), AttribArray::Int(b)) => a.extend_from_slice(b),
            (AttribArray::Vec3(a), AttribArray::Vec3(b)) => a.extend_from_slice(b),
            (AttribArray::Str(a), AttribArray::Str(b)) => a.extend_from_slice(b),
            (me, _) => {
                let n = me.len();
                me.resize(n + other_len);
            }
        }
    }
}

/// Named attribute columns that all have the same length.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AttribTable {
    columns: IndexMap<String, AttribArray>,
}

impl AttribTable {
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.columns.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &AttribArray)> {
        self.columns.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn contains(&self, name: &str) -> bool {
        self.columns.contains_key(name)
    }

    pub fn get(&self, name: &str) -> Option<&AttribArray> {
        self.columns.get(name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut AttribArray> {
        self.columns.get_mut(name)
    }

    /// Insert or replace a column. The caller must keep lengths consistent;
    /// [`crate::Geometry::validate`] checks this.
    pub fn set(&mut self, name: impl Into<String>, values: AttribArray) {
        self.columns.insert(name.into(), values);
    }

    /// Get a column, creating it zero-filled with `len` elements if missing.
    pub fn ensure(&mut self, name: &str, ty: AttribType, len: usize) -> GeoResult<&mut AttribArray> {
        let col = self
            .columns
            .entry(name.to_string())
            .or_insert_with(|| AttribArray::new(ty, len));
        if col.ty() != ty {
            return Err(GeoError::AttribType {
                name: name.to_string(),
                expected: ty,
                found: col.ty(),
            });
        }
        Ok(col)
    }

    pub fn remove(&mut self, name: &str) -> Option<AttribArray> {
        self.columns.shift_remove(name)
    }

    pub fn floats(&self, name: &str) -> GeoResult<&[f64]> {
        match self.columns.get(name) {
            Some(AttribArray::Float(v)) => Ok(v),
            Some(other) => Err(type_err(name, AttribType::Float, other.ty())),
            None => Err(GeoError::MissingAttrib(name.to_string())),
        }
    }

    pub fn ints(&self, name: &str) -> GeoResult<&[i64]> {
        match self.columns.get(name) {
            Some(AttribArray::Int(v)) => Ok(v),
            Some(other) => Err(type_err(name, AttribType::Int, other.ty())),
            None => Err(GeoError::MissingAttrib(name.to_string())),
        }
    }

    pub fn vec3s(&self, name: &str) -> GeoResult<&[DVec3]> {
        match self.columns.get(name) {
            Some(AttribArray::Vec3(v)) => Ok(v),
            Some(other) => Err(type_err(name, AttribType::Vec3, other.ty())),
            None => Err(GeoError::MissingAttrib(name.to_string())),
        }
    }

    pub fn strs(&self, name: &str) -> GeoResult<&[String]> {
        match self.columns.get(name) {
            Some(AttribArray::Str(v)) => Ok(v),
            Some(other) => Err(type_err(name, AttribType::Str, other.ty())),
            None => Err(GeoError::MissingAttrib(name.to_string())),
        }
    }

    pub fn resize_all(&mut self, len: usize) {
        for col in self.columns.values_mut() {
            col.resize(len);
        }
    }

    pub fn gather_all(&self, indices: &[usize]) -> AttribTable {
        AttribTable {
            columns: self
                .columns
                .iter()
                .map(|(k, v)| (k.clone(), v.gather(indices)))
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }
}

fn type_err(name: &str, expected: AttribType, found: AttribType) -> GeoError {
    GeoError::AttribType {
        name: name.to_string(),
        expected,
        found,
    }
}

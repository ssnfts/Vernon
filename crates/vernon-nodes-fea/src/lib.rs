//! Stage 6 FEA: CalculiX decks, anisotropic printed materials at temperature, safety factors.

pub mod deck;
pub mod failure;
pub mod frd;
pub mod linalg;
pub mod loads;
pub mod material;
pub mod mesh;
pub mod spatial;
pub mod study;
pub mod types;

use vernon_graph::Registry;

/// Register every node type in this crate.
pub fn register(_reg: &mut Registry) {}

//! Stage 2 Clean/Align: decimate, outliers, coarse+ICP alignment, signed deviation reports, watertight wrap.

// Numerical kernels index several parallel arrays and take many geometric
// arguments; the indexed loops and wide signatures are deliberate.
#![allow(clippy::needless_range_loop, clippy::too_many_arguments, clippy::type_complexity)]

use vernon_graph::Registry;

pub mod align;
mod conservative;
pub mod decimate;
pub mod deviation;
pub mod distance;
pub mod filters;
pub mod kdtree;
pub mod linalg;
pub mod meshedit;
pub mod meshfix;
pub mod qem_simplify;
pub mod remesh;
pub mod stats;
pub mod symmetry;
pub mod trimesh;
pub mod util;
pub mod wrap;

#[cfg(test)]
mod testkit;
#[cfg(test)]
mod tests_core;
#[cfg(test)]
mod tests_registry;

/// Register every node type in this crate.
pub fn register(reg: &mut Registry) {
    align::register(reg);
    decimate::register(reg);
    deviation::register(reg);
    distance::register(reg);
    filters::register(reg);
    meshfix::register(reg);
    remesh::register(reg);
    symmetry::register(reg);
    wrap::register(reg);
}

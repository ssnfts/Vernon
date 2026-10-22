//! Stage 2 Clean/Align: decimate, outliers, coarse+ICP alignment, signed deviation reports, watertight wrap.

use vernon_graph::Registry;

pub mod align;
pub mod distance;
pub mod filters;
pub mod kdtree;
pub mod linalg;
pub mod stats;
pub mod trimesh;
pub mod util;

#[cfg(test)]
mod testkit;
#[cfg(test)]
mod tests_core;

/// Register every node type in this crate.
pub fn register(reg: &mut Registry) {
    align::register(reg);
    distance::register(reg);
    filters::register(reg);
}

//! Stage 3 Mesh to Surface: quad remesh, SubD, auto-surfacing, mesh sections, curve fit, loft, STEP hand-off.

#![allow(clippy::needless_range_loop, clippy::too_many_arguments, clippy::type_complexity)]

pub mod bvh;
pub mod curvefit;
pub mod linalg;
pub mod nurbs;
pub mod sections;
#[cfg(test)]
pub mod testutil;
pub mod topology;
pub mod util;

use vernon_graph::Registry;

/// Register every node type in this crate.
pub fn register(reg: &mut Registry) {
    reg.add(sections::SectionCurvesNode);
    reg.add(curvefit::FitSectionCurvesNode);
}

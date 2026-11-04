//! Stage 5 CFD: OpenFOAM automotive case generation, convergence study, force coefficients, Cp maps.
//!
//! The chain, in graph order: vehicle preparation, frontal area and mirror
//! symmetry, virtual tunnel, ground and wheels, porous cores, refinement plan,
//! mesh dictionaries, mesh build and check, solver case, steady solve, force
//! coefficients, run and mesh convergence, baseline-vs-kit delta, site loads,
//! surface pressure map and benchmark check. Lengths on graph wires are
//! millimetres; the Json hand-offs between CFD nodes and everything written
//! for the solver are metres and SI, with unit strings in the reports.
//!
//! OpenFOAM (GPL) is only ever run as a separate process: this crate writes
//! its input files and reads its output files.

use vernon_graph::Registry;

mod air;
mod case;
mod coeffs;
mod extproc;
mod foam;
mod loads;
mod mesh;
mod pressure;
mod reference;
mod reference_record;
mod study;
#[cfg(test)]
mod tests;
mod tunnel;
mod types;
mod util;
mod vehicle;

pub use air::{dynamic_pressure, moist_air, Air};
pub use coeffs::{axle_split, window_stats, ForceResult};
pub use loads::site_loads;
pub use study::{grid_convergence, Gci};

/// Register every node type in this crate.
pub fn register(reg: &mut Registry) {
    air::register(reg);
    vehicle::register(reg);
    tunnel::register(reg);
    mesh::register(reg);
    case::register(reg);
    coeffs::register(reg);
    study::register(reg);
    loads::register(reg);
    pressure::register(reg);
}

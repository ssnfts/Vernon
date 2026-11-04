//! Stage 8 Performance: aero calculator, point-mass lap model, coastdown and pressure-tap validation.
//!
//! Each stage-8 node takes numbers from the airflow stage (CdA, ClA, Cp maps), the vehicle
//! facts and a track, and emits numbers a person can check against the road: lap time and
//! speed trace, the lap-time value of a drag or downforce change, and a noise-floor verdict
//! for the cheap physical tests (coastdown, surface-pressure taps, tufts).
//!
//! Data contracts (all `Data::Json`, tagged with a `kind` field): `env.v1`, `vehicle.v1`,
//! `track.v1`, `lap_result.v1`, `coast_fit.v1`, `cp_table.v1`, `table.v1`.
//! Specification: `docs/specs/performance-validation.md`.
//!
//! Layout: pure numerics ([`physics`], [`track`], [`lapsim`], [`coast`], [`cp`], [`gate`])
//! with unit tests, and thin node wrappers (`nodes_*`) that read ports and parameters.

pub mod coast;
pub mod cp;
pub mod gate;
pub mod lapsim;
pub mod nodes_calc;
pub mod nodes_gate;
pub mod nodes_lap;
pub mod nodes_track;
pub mod nodes_valid;
pub mod physics;
pub mod track;
pub mod tuft;
pub mod tuft_detect;
pub mod util;

use vernon_graph::Registry;

/// Register every node type in this crate.
pub fn register(reg: &mut Registry) {
    nodes_calc::register(reg);
    nodes_track::register(reg);
    nodes_lap::register(reg);
    nodes_valid::register(reg);
    nodes_gate::register(reg);
}

#[cfg(test)]
mod tests;

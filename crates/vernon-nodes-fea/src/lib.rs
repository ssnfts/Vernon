//! Stage 6 FEA: CalculiX decks, anisotropic printed materials at temperature, safety factors.
//!
//! Units are mm, N, tonne, s, MPa (`docs/specs/structural-fea.md`). The chain is
//! material -> axes -> loads -> mounts -> deck -> solve -> read -> safety factor
//! -> mount report, each node handing the next a report with the numbers it gates on.

pub mod deck;
pub mod failure;
pub mod frd;
pub mod linalg;
pub mod loads;
pub mod material;
pub mod mesh;
pub mod nodes_load;
pub mod nodes_material;
pub mod nodes_mesh;
pub mod nodes_result;
pub mod nodes_solve;
pub mod refsolve;
pub mod solver;
pub mod sparse;
pub mod spatial;
pub mod study;
pub mod types;
pub mod util;

use vernon_graph::Registry;

/// Register every node type in this crate.
pub fn register(reg: &mut Registry) {
    reg.add(nodes_material::MaterialFdm);
    reg.add(nodes_material::LayerAxes);
    reg.add(nodes_material::MaterialDerate);
    reg.add(nodes_material::TemperatureMap);
    reg.add(nodes_material::CreepPlan);
    reg.add(nodes_material::MaterialTable);
    reg.add(nodes_mesh::TetMesh);
    reg.add(nodes_mesh::MeshCheck);
    reg.add(nodes_mesh::MountSites);
    reg.add(nodes_load::LoadAero);
    reg.add(nodes_load::LoadBump);
    reg.add(nodes_load::LoadCases);
    reg.add(nodes_solve::DeckWrite);
    reg.add(nodes_solve::SolveExternal);
    reg.add(nodes_solve::SolveLinear);
    reg.add(nodes_result::ResultRead);
    reg.add(nodes_result::SafetyFactor);
    reg.add(nodes_result::MountReport);
    reg.add(nodes_result::MeshStudy);
    reg.add(nodes_result::HandCheck);
}

#[cfg(test)]
mod cantilever;
#[cfg(test)]
mod tests;

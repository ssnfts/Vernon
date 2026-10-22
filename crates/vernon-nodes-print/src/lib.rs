//! Stage 4 Split/Print: body-line splitting, grid split, dovetail/dowel keys, bed fit, 3MF, slicer hand-off.

pub mod common;
pub mod cut;
pub mod joints;
pub mod mesh;
pub mod nodes_cut;
pub mod nodes_split;
pub mod poly;

use vernon_graph::Registry;

/// Register every node type in this crate.
pub fn register(reg: &mut Registry) {
    reg.add(nodes_cut::PlaneCut);
    reg.add(nodes_cut::PolylineCut);
    reg.add(nodes_cut::CutRegionSelect);
    reg.add(nodes_split::GridPartition);
    reg.add(nodes_split::SmartPartition);
    reg.add(nodes_split::Connectivity);
    reg.add(nodes_split::ByAttribute);
    reg.add(nodes_split::PartExtract);
    reg.add(nodes_split::PartLabel);
}

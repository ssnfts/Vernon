//! General geometry: primitives, transform, merge, mirror, normals, file
//! read/write, selection, measurement and topology clean-up.
//!
//! Every node here is a real implementation (no white-box stubs) and
//! follows `docs/specs/geometry-basics.md`. The generator functions in
//! [`primitives`] (notably [`primitives::CarPanel`]) are public so other
//! stages can build synthetic test inputs without a graph.

pub mod file;
pub mod measure;
pub mod noise;
pub mod primitives;
pub mod select;
pub mod topology;
pub mod util;
pub mod xform;

use vernon_graph::Registry;

/// Register every node type in this crate.
pub fn register(reg: &mut Registry) {
    reg.add(primitives::BoxNode);
    reg.add(primitives::GridNode);
    reg.add(primitives::SphereNode);
    reg.add(primitives::TubeNode);
    reg.add(primitives::CarPanelNode);
    reg.add(file::FileReadNode);
    reg.add(file::FileWriteNode);
    reg.add(xform::TransformNode);
    reg.add(xform::MergeNode);
    reg.add(xform::MirrorNode);
    reg.add(topology::NormalsNode);
    reg.add(topology::ReverseNode);
    reg.add(topology::TriangulateNode);
    reg.add(select::DeleteNode);
    reg.add(select::GroupNode);
    reg.add(select::AttribCreateNode);
    reg.add(measure::MeasureNode);
    reg.add(topology::FuseNode);
    reg.add(topology::SubdivideNode);
    reg.add(noise::NoiseNode);
}

#[cfg(test)]
mod tests;

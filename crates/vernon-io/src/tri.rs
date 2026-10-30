//! Triangle-only writers (STL, 3MF) use the shared triangulator in
//! `vernon-core`, so every output agrees with the viewport.

pub(crate) use vernon_core::triangulate::triangulate_ring;
use vernon_core::Geometry;

/// Triangulate every closed polygon of `geo`; each triangle is tagged with the
/// index of the primitive it came from.
pub(crate) fn triangulate(geo: &Geometry) -> Vec<(usize, [u32; 3])> {
    geo.triangulated()
}

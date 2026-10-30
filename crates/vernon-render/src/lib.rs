//! Vernon viewport renderer.
//!
//! An offscreen `wgpu` renderer for `egui` viewports, version-matched to
//! `eframe` 0.32 / `egui-wgpu` 0.32 / `wgpu` 25. Each on-screen viewport owns
//! an offscreen colour + depth target (resized on demand, MSAA when the
//! adapter allows it); the resolved colour texture is registered with egui as
//! a native texture, so the viewport is shown with a plain `ui.image(..)` and
//! nothing is read back to the CPU.
//!
//! Typical use from the app:
//!
//! ```text
//! // once, in the creation context
//! let mut vr = ViewportRenderer::new(render_state);
//! // when a node's output changes (key = node id, re-upload only then)
//! let handle = vr.upload(key, &geometry);
//! // each frame, per viewport
//! let view = ViewDesc::from_camera(&camera, size_px);
//! let tex = vr.render(render_state, &view, &[DrawItem::new(handle)]);
//! ui.image((tex, rect.size()));
//! ```
//!
//! Modules:
//! * [`camera`] - the viewport camera, shared with the app (orbit, pan, zoom,
//!   standard views, zoom extents, picking rays); pure maths, no GPU.
//! * [`navcube`] - view cube regions, snapping, roll, compass and stored front
//!   view; pure maths.
//! * [`mesh`] - `Geometry` -> vertex/index buffers (ear-clip triangulation,
//!   normals, colours, deduplicated edges, polylines, points); pure CPU.
//! * [`grid`] - adaptive ground-grid line builder; pure CPU.
//! * [`renderer`] - GPU resources, pipelines, the depth-tested grid, the
//!   always-on-top overlay (gizmo) pass, ID picking and egui registration.
//!
//! Units are millimetres, Z is up (see `vernon-core`).

pub mod camera;
pub mod grid;
pub mod mesh;
pub mod navcube;
pub mod renderer;

/// WGSL source of all pipelines (mesh, edge, curve and point passes).
pub const SHADER_WGSL: &str = include_str!("shader.wgsl");

pub use camera::{Camera, GridPlane, OrbitMode, ViewKind};
pub use grid::{build_grid, effective_spacing, GridColors, MAX_GRID_LINES_PER_SIDE};
pub use mesh::{MeshBuffers, MeshVertex, PointVertex, DEFAULT_MESH_COLOR};
pub use renderer::{
    DrawItem, MeshHandle, MeshStats, ShadingMode, ViewDesc, ViewportRenderer, COLOR_FORMAT,
    DEPTH_FORMAT, PICK_FORMAT, SELECTED_WIRE_COLOR,
};

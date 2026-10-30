//! Viewport maths and rendering backends.
//!
//! [`ViewportBackend`] is the seam where a GPU renderer plugs in: the app
//! hands it a tile rectangle, the camera and the display meshes of all
//! visible objects. [`painter::PainterBackend`] is the always-available CPU
//! fallback that draws through egui's painter.

pub mod gizmo;
pub mod gpu;
pub mod grid;
pub mod layout;
pub mod mesh;
pub mod painter;

use std::sync::Arc;

use eframe::egui::{Color32, Painter, Pos2, Rect};
use vernon_graph::NodeId;

pub use vernon_render::navcube as viewcube;
pub use vernon_render::{camera, Camera, OrbitMode, ViewKind};
pub use grid::{GridGeometry, GridSegment};
pub use mesh::{ColorBy, RenderMesh};

/// How surfaces are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shading {
    Shaded,
    Wireframe,
}

impl Shading {
    pub fn label(self) -> &'static str {
        match self {
            Shading::Shaded => "Shaded",
            Shading::Wireframe => "Wireframe",
        }
    }
}

/// Per-tile view settings handed to the backend.
#[derive(Debug, Clone)]
pub struct ViewParams {
    pub camera: Camera,
    pub shading: Shading,
    /// Draw triangle edges over shaded surfaces.
    pub edged: bool,
    /// Home grid of the tile, when shown.
    pub grid: Option<Arc<GridGeometry>>,
    /// Lines drawn on top of everything: the transform gizmo.
    pub overlay: Vec<GridSegment>,
}

/// One visible object.
#[derive(Debug, Clone)]
pub struct DrawItem {
    pub id: NodeId,
    pub mesh: Arc<RenderMesh>,
    pub color: Color32,
    pub selected: bool,
}

/// A renderer for viewport tiles.
pub trait ViewportBackend {
    /// Human-readable backend name for the Utilities panel.
    fn name(&self) -> &str;

    /// Called once per frame before any tile is painted. A GPU backend can
    /// reach its render state through `frame` here.
    fn begin_frame(&mut self, _frame: &mut eframe::Frame) {}

    /// Draw `items` into `rect` and the view's grid (other overlays are drawn by the app).
    fn paint(&mut self, painter: &Painter, rect: Rect, view: &ViewParams, items: &[DrawItem]);

    /// The object at `at` (screen points) within `tol` points, as the backend
    /// sees it. `None` when the backend has no picking of its own; the caller
    /// then casts a ray.
    fn pick(&mut self, _rect: Rect, _view: &ViewParams, _items: &[DrawItem], _at: Pos2, _tol: f32, _ppp: f32) -> Option<Option<NodeId>> {
        None
    }
}

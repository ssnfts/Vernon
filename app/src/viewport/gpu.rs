//! GPU backend: `vernon-render`'s offscreen wgpu renderer behind
//! [`ViewportBackend`]. Each tile renders into its own depth-tested,
//! multisampled texture which is then shown as an egui image. The texture
//! is cleared transparent over the tile background; the home grid and the
//! ground plane are depth-tested with the scene.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use eframe::egui::{pos2, Color32, Painter, Pos2, Rect};
use eframe::egui_wgpu::RenderState;
use glam::{DVec2, Vec3};
use vernon_core::Aabb;
use vernon_graph::NodeId;
use vernon_render::{
    DrawItem as GpuItem, MeshBuffers, MeshHandle, MeshVertex, PointVertex, ShadingMode, ViewDesc,
    ViewportRenderer,
};

use super::{DrawItem, GridGeometry, GridSegment, RenderMesh, Shading, ViewParams, ViewportBackend};

struct Uploaded {
    mesh: Arc<RenderMesh>,
    color: Color32,
}

pub struct GpuBackend {
    rs: RenderState,
    renderer: ViewportRenderer,
    uploaded: HashMap<u64, Uploaded>,
    /// Objects drawn since the last `begin_frame`.
    seen: HashSet<u64>,
    /// Tile counter within a frame; tiles are painted in a fixed order, so
    /// it doubles as the offscreen target id.
    tile: u64,
    tiles_last_frame: u64,
    name: String,
}

impl GpuBackend {
    pub fn new(rs: RenderState) -> Self {
        let renderer = ViewportRenderer::new(&rs);
        let info = rs.adapter.get_info();
        let name = format!(
            "GPU ({:?}, {}x MSAA): {}",
            info.backend,
            renderer.sample_count(),
            info.name
        );
        Self {
            rs,
            renderer,
            uploaded: HashMap::new(),
            seen: HashSet::new(),
            tile: 0,
            tiles_last_frame: 0,
            name,
        }
    }

    fn handle(&mut self, item: &DrawItem) -> MeshHandle {
        let key = item.id as u64;
        self.seen.insert(key);
        let fresh = match self.uploaded.get(&key) {
            Some(u) => !(Arc::ptr_eq(&u.mesh, &item.mesh) && u.color == item.color),
            None => true,
        };
        if fresh || !self.renderer.has_mesh(key) {
            let buffers = buffers(&item.mesh, item.color);
            self.renderer.upload_buffers(key, &buffers);
            self.uploaded.insert(
                key,
                Uploaded {
                    mesh: item.mesh.clone(),
                    color: item.color,
                },
            );
        }
        MeshHandle(key)
    }
}

impl ViewportBackend for GpuBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn begin_frame(&mut self, _frame: &mut eframe::Frame) {
        // Free meshes of objects that were not drawn last frame and targets
        // of tiles that are no longer shown (e.g. after maximising).
        let seen = std::mem::take(&mut self.seen);
        let stale: Vec<u64> = self
            .uploaded
            .keys()
            .copied()
            .filter(|k| !seen.contains(k))
            .collect();
        for k in stale {
            self.uploaded.remove(&k);
            self.renderer.remove_mesh(k);
        }
        if self.tile > 0 {
            for id in self.tile..self.tiles_last_frame {
                self.renderer.free_viewport(&self.rs, id);
            }
            self.tiles_last_frame = self.tile;
        }
        self.tile = 0;
    }

    fn paint(&mut self, painter: &Painter, rect: Rect, view: &ViewParams, items: &[DrawItem]) {
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return;
        }
        let ppp = painter.ctx().pixels_per_point();
        let size = [
            (rect.width() * ppp).round().max(1.0) as u32,
            (rect.height() * ppp).round().max(1.0) as u32,
        ];
        let gpu_items: Vec<GpuItem> = items
            .iter()
            .map(|it| {
                let mut g = GpuItem::new(self.handle(it));
                g.selected = it.selected;
                g.wire_color = it.color;
                g
            })
            .collect();
        let mut desc = view_desc(view, items, size, ppp);
        desc.viewport_id = self.tile;
        self.tile += 1;
        let tex = self.renderer.render(&self.rs, &desc, &gpu_items);
        let painter = painter.with_clip_rect(rect);
        painter.image(
            tex,
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );

        let mut overlay = Vec::new();
        let sz = DVec2::new(rect.width() as f64, rect.height() as f64);
        for it in items.iter().filter(|i| i.selected) {
            super::painter::brackets(
                &mut overlay,
                &view.camera,
                rect,
                sz,
                it.mesh.min.as_dvec3(),
                it.mesh.max.as_dvec3(),
            );
        }
        painter.extend(overlay);
    }

    fn pick(&mut self, rect: Rect, view: &ViewParams, items: &[DrawItem], at: Pos2, tol: f32, ppp: f32) -> Option<Option<NodeId>> {
        let size = [
            (rect.width() * ppp).round().max(1.0) as u32,
            (rect.height() * ppp).round().max(1.0) as u32,
        ];
        let gpu_items: Vec<GpuItem> = items.iter().map(|it| GpuItem::new(self.handle(it))).collect();
        let desc = view_desc(view, items, size, ppp);
        let px = [
            ((at.x - rect.min.x) * ppp).max(0.0) as u32,
            ((at.y - rect.min.y) * ppp).max(0.0) as u32,
        ];
        let hit = self.renderer.pick(&desc, &gpu_items, px, (tol * ppp).ceil() as u32);
        Some(hit.map(|i| items[i].id))
    }
}

/// Offscreen view description for a tile of `size` physical pixels:
/// transparent background and the tile's grid, if any.
pub fn view_desc(view: &ViewParams, items: &[DrawItem], size: [u32; 2], ppp: f32) -> ViewDesc {
    let aspect = size[0].max(1) as f32 / size[1].max(1) as f32;
    let mut desc = ViewDesc::new(size);
    desc.view = view.camera.view_matrix();
    desc.proj = view.camera.proj_matrix(aspect, scene_radius(view, items));
    desc.shading = match (view.shading, view.edged) {
        (Shading::Wireframe, _) => ShadingMode::Wireframe,
        (Shading::Shaded, true) => ShadingMode::ShadedEdges,
        (Shading::Shaded, false) => ShadingMode::Shaded,
    };
    desc.background = Color32::TRANSPARENT;
    desc.show_grid = view.grid.is_some();
    desc.grid_lines = view.grid.as_ref().map(|g| grid_vertices(g));
    desc.occluder = view.grid.as_ref().and_then(|g| g.ground).map(|q| q.map(|p| p.as_vec3()));
    desc.overlay_lines = view.overlay.iter().flat_map(segment_vertices).collect();
    desc.point_size = 2.0 * ppp;
    desc
}

fn grid_vertices(g: &GridGeometry) -> Vec<PointVertex> {
    g.segments.iter().flat_map(segment_vertices).collect()
}

fn segment_vertices(s: &GridSegment) -> [PointVertex; 2] {
    let color = lin([s.color.r(), s.color.g(), s.color.b()]);
    [s.a, s.b].map(|p| PointVertex { pos: p.as_vec3().to_array(), color })
}

/// Distance from the camera target that encloses every item and the grid,
/// which sets the orthographic depth range.
fn scene_radius(view: &ViewParams, items: &[DrawItem]) -> f64 {
    let cam = &view.camera;
    let grid = view.grid.iter().flat_map(|g| {
        let lines = g.segments.iter().flat_map(|s| [s.a, s.b]);
        lines.chain(g.ground.into_iter().flatten())
    });
    items
        .iter()
        .flat_map(|i| {
            let (lo, hi) = (i.mesh.min.as_dvec3(), i.mesh.max.as_dvec3());
            (0..8).map(move |k| {
                glam::DVec3::new(
                    if k & 1 == 0 { lo.x } else { hi.x },
                    if k & 2 == 0 { lo.y } else { hi.y },
                    if k & 4 == 0 { lo.z } else { hi.z },
                )
            })
        })
        .chain(grid)
        .map(|c| (c - cam.target).length())
        .fold(1.0, f64::max)
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn lin(c: [u8; 3]) -> [f32; 3] {
    c.map(srgb_to_linear)
}

/// GPU buffers from a display mesh: flat-shaded triangles in the
/// attribute or object colour, de-duplicated triangle edges, polylines and
/// loose points.
pub fn buffers(m: &RenderMesh, color: Color32) -> MeshBuffers {
    let obj = lin([color.r(), color.g(), color.b()]);
    let mut b = MeshBuffers {
        has_point_color: m.point_colors.is_some(),
        smooth: false,
        bounds: Aabb {
            min: m.min.as_dvec3(),
            max: m.max.as_dvec3(),
        },
        ..Default::default()
    };
    b.point_vertices = m
        .positions
        .iter()
        .enumerate()
        .map(|(i, p)| PointVertex {
            pos: p.to_array(),
            color: m
                .point_colors
                .as_ref()
                .and_then(|c| c.get(i))
                .map_or(obj, |c| lin(*c)),
        })
        .collect();
    let n = m.positions.len() as u32;
    let mut edges = HashSet::new();
    for (ti, t) in m.tris.iter().enumerate() {
        if t.iter().any(|&i| i >= n) {
            continue;
        }
        let [a, bb, c] = t.map(|i| m.positions[i as usize]);
        let normal = (bb - a).cross(c - a).normalize_or(Vec3::Z);
        let col = m
            .tri_colors
            .as_ref()
            .and_then(|c| c.get(ti))
            .map_or(obj, |c| lin(*c));
        let base = b.tri_vertices.len() as u32;
        for p in [a, bb, c] {
            b.tri_vertices.push(MeshVertex {
                pos: p.to_array(),
                normal: normal.to_array(),
                color: col,
            });
        }
        b.tri_indices.extend([base, base + 1, base + 2]);
        for (p, q) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let k = (p.min(q), p.max(q));
            if edges.insert(k) {
                b.edge_indices.extend([k.0, k.1]);
            }
        }
    }
    b.curve_indices = m
        .lines
        .iter()
        .filter(|l| l[0] < n && l[1] < n)
        .flat_map(|l| [l[0], l[1]])
        .collect();
    b.loose_points = m.dots.iter().copied().filter(|&i| i < n).collect();
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewport::mesh::{build, ColorBy};
    use crate::viewport::{Camera, ViewKind};
    use glam::DVec3;
    use vernon_core::Geometry;

    #[test]
    fn buffers_carry_cd_colours_and_unique_edges() {
        let mut g = Geometry::make_box(DVec3::splat(10.0));
        let cd = vec![DVec3::new(1.0, 0.0, 0.0); g.points.len()];
        g.point_attribs
            .set("Cd", vernon_core::AttribArray::Vec3(cd));
        let m = build(&g, ColorBy::Cd);
        let b = buffers(&m, Color32::from_rgb(0, 0, 255));
        assert_eq!(b.triangle_count(), 12);
        assert_eq!(b.edge_count(), 18); // 12 box edges + 6 quad diagonals
        assert!(b.has_point_color);
        assert!(b.tri_vertices.iter().all(|v| v.color == [1.0, 0.0, 0.0]));
        let plain = buffers(&build(&g, ColorBy::Object), Color32::from_rgb(0, 0, 255));
        assert!(plain
            .tri_vertices
            .iter()
            .all(|v| v.color == [0.0, 0.0, 1.0]));
    }

    /// Renders the deviation-coloured car panel offscreen through the same
    /// buffers, matrices and view description the app uses, in a perspective
    /// and an orthographic tile. Skips when no adapter exists (headless CI).
    #[test]
    fn renders_panel_offscreen_when_a_gpu_exists() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Ok(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        else {
            eprintln!("no adapter available; skipping");
            return;
        };
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        else {
            eprintln!("no device available; skipping");
            return;
        };
        let mut r = ViewportRenderer::with_device(&device, &queue, 1);
        let mut geo = vernon_nodes_geo::primitives::CarPanel::default().build();
        let cd = vec![DVec3::new(1.0, 0.0, 0.0); geo.points.len()];
        geo.point_attribs
            .set("Cd", vernon_core::AttribArray::Vec3(cd));
        let mesh = Arc::new(build(&geo, ColorBy::Cd));
        r.upload_buffers(1, &buffers(&mesh, Color32::GRAY));
        let item = DrawItem {
            id: 1,
            mesh: mesh.clone(),
            color: Color32::GRAY,
            selected: false,
        };
        for kind in [ViewKind::Perspective, ViewKind::Top] {
            let mut camera = Camera::new(kind);
            let bounds = geo.bounds();
            camera.fit(&bounds, 1.0, 1.1);
            let view = ViewParams {
                camera,
                shading: Shading::Shaded,
                edged: false,
                grid: None,
                overlay: Vec::new(),
            };
            let desc = view_desc(&view, std::slice::from_ref(&item), [96, 96], 1.0);
            r.draw(&desc, &[GpuItem::new(MeshHandle(1))]);
            let (w, h, px) = r.read_pixels(0).expect("target exists");
            let at = |x: u32, y: u32| {
                let i = ((y * w + x) * 4) as usize;
                [px[i], px[i + 1], px[i + 2], px[i + 3]]
            };
            assert_eq!(at(0, 0)[3], 0, "{kind:?}: corner must stay transparent");
            let c = at(w / 2, h / 2);
            assert_eq!(c[3], 255, "{kind:?}: panel covers the centre");
            assert!(
                c[0] > 2 * c[1].max(c[2]) && c[0] > 40,
                "{kind:?}: Cd red expected, got {c:?}"
            );
        }
    }

    /// A red box against the ground plane, seen through the real view
    /// description. Skips when no adapter exists.
    #[test]
    fn ground_plane_hides_what_is_below_it_when_a_gpu_exists() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Ok(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        else {
            eprintln!("no adapter available; skipping");
            return;
        };
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        else {
            eprintln!("no device available; skipping");
            return;
        };
        let mut r = ViewportRenderer::with_device(&device, &queue, 1);
        let size = DVec2::new(128.0, 128.0);
        let gs = crate::viewport::grid::GridSettings::default();
        let mut shoot = |kind: ViewKind, z: f64, with_grid: bool| {
            let mut g = Geometry::make_box(DVec3::splat(100.0));
            for p in &mut g.points {
                p.z += z;
            }
            let mesh = Arc::new(build(&g, ColorBy::Object));
            r.upload_buffers(1, &buffers(&mesh, Color32::RED));
            let it = DrawItem { id: 1, mesh, color: Color32::RED, selected: false };
            let mut camera = Camera::new(kind);
            camera.target = DVec3::new(0.0, 0.0, z);
            let grid = with_grid.then(|| Arc::new(crate::viewport::grid::build(&camera, kind, size, &gs)));
            let view = ViewParams { camera, shading: Shading::Shaded, edged: false, grid, overlay: Vec::new() };
            let desc = view_desc(&view, std::slice::from_ref(&it), [128, 128], 1.0);
            r.draw(&desc, &[GpuItem::new(MeshHandle(1))]);
            let (w, h, px) = r.read_pixels(0).unwrap();
            let count = |f: &dyn Fn(&[u8]) -> bool| px.chunks(4).take((w * h) as usize).filter(|p| f(p)).count();
            let red = count(&|p| p[3] > 0 && p[0] > 40 && p[0] > 2 * p[1].max(p[2]));
            let lines = count(&|p| p[3] > 0 && p[0] == p[1] && p[1] == p[2]);
            (red, lines)
        };
        // Below the ground, seen from above: hidden, but only with the grid.
        for kind in [ViewKind::Perspective, ViewKind::Top] {
            assert!(shoot(kind, -200.0, false).0 > 100, "{kind:?}: visible without the grid");
            assert_eq!(shoot(kind, -200.0, true).0, 0, "{kind:?}: hidden by the ground");
        }
        // Seen from below, the same box is in front of the plane.
        assert!(shoot(ViewKind::Bottom, -200.0, true).0 > 100);
        // A box above the ground hides the grid behind it, not the reverse.
        let (red, lines_over) = shoot(ViewKind::Top, 200.0, true);
        assert!(red > 100, "box above the ground stays visible");
        let (_, lines_clear) = shoot(ViewKind::Top, -200.0, true);
        assert!(lines_over < lines_clear, "{lines_over} vs {lines_clear}");
    }
}

//! GPU side: mesh cache, offscreen targets, pipelines and egui registration.

use std::collections::HashMap;
use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use egui::{Color32, Rgba, TextureId};
use glam::{DVec3, Mat4, Vec3, Vec4Swizzles};
use vernon_core::{Aabb, Geometry};
use wgpu::util::DeviceExt;

use crate::camera::{Camera, GridPlane};
use crate::grid::{build_grid, effective_spacing, GridColors};
use crate::mesh::{MeshBuffers, MeshVertex, PointVertex};
use crate::SHADER_WGSL;

/// Format of the offscreen colour target (what egui expects for native
/// textures; the shader writes gamma-encoded colour into it).
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Format of the offscreen depth target (reverse-Z).
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Wire colour used for selected items (the reference DCC draws selection
/// in white).
pub const SELECTED_WIRE_COLOR: Color32 = Color32::WHITE;
/// Ambient term of the headlight model.
pub const AMBIENT: f32 = 0.25;
/// Alpha of the edge overlay drawn on selected items in `Shaded` mode.
pub const SELECTED_EDGE_ALPHA: f32 = 0.35;

const UNIFORM_SIZE: u64 = std::mem::size_of::<Uniforms>() as u64;

/// Handle to an uploaded mesh. It is just the caller's cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MeshHandle(pub u64);

/// Viewport shading mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ShadingMode {
    /// Polygon edges, polylines and loose points in the wire colour.
    Wireframe,
    /// Lit faces (selected items get a faint white edge overlay).
    #[default]
    Shaded,
    /// Lit faces plus edges ("Edged Faces").
    ShadedEdges,
    /// Every point as a small screen-aligned square.
    Points,
}

/// Everything needed to render one viewport.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewDesc {
    /// Identifies the offscreen target: use one id per on-screen viewport
    /// (e.g. 0..4 for a quad layout). Each id gets its own texture.
    pub viewport_id: u64,
    /// Target size in physical pixels (`rect.size() * pixels_per_point`).
    pub size: [u32; 2],
    /// World -> view.
    pub view: Mat4,
    /// View -> clip, reverse-Z (see [`Camera::proj_matrix`]).
    pub proj: Mat4,
    pub shading: ShadingMode,
    /// Clear colour (sRGB, as egui colours are).
    pub background: Color32,
    pub show_grid: bool,
    /// Minor grid spacing (mm); multiplied by 10 while the grid would be
    /// too dense.
    pub grid_spacing: f32,
    /// Plane of the home grid (XY ground plane in perspective views).
    pub grid_plane: GridPlane,
    /// The grid is centred on this point projected into `grid_plane`.
    pub grid_center: Vec3,
    /// Half-size of the grid (mm).
    pub grid_extent: f32,
    pub grid_color: Color32,
    pub grid_major_color: Color32,
    pub grid_axis_color: Color32,
    /// Explicit grid line list (world, linear colours) that replaces the
    /// built-in square grid when set; still needs `show_grid`.
    pub grid_lines: Option<Vec<PointVertex>>,
    /// World-space quad that writes depth only: an opaque ground plane that
    /// hides what lies behind it without drawing anything itself.
    pub occluder: Option<[Vec3; 4]>,
    /// Size of points in physical pixels.
    pub point_size: f32,
}

impl ViewDesc {
    /// Identity matrices and default colours.
    pub fn new(size: [u32; 2]) -> Self {
        Self {
            viewport_id: 0,
            size,
            view: Mat4::IDENTITY,
            proj: Mat4::IDENTITY,
            shading: ShadingMode::Shaded,
            background: Color32::from_gray(61),
            show_grid: true,
            grid_spacing: 10.0,
            grid_plane: GridPlane::XY,
            grid_center: Vec3::ZERO,
            grid_extent: 1000.0,
            grid_color: Color32::from_gray(82),
            grid_major_color: Color32::from_gray(104),
            grid_axis_color: Color32::from_gray(24),
            grid_lines: None,
            occluder: None,
            point_size: 2.0,
        }
    }

    /// Matrices, grid plane and grid placement from an orbit camera.
    pub fn from_camera(camera: &Camera, size: [u32; 2]) -> Self {
        let aspect = size[0].max(1) as f32 / size[1].max(1) as f32;
        Self {
            view: camera.view_matrix(),
            proj: camera.proj_matrix(aspect),
            grid_plane: camera.grid_plane(),
            grid_center: camera.target,
            grid_extent: camera.distance * 3.0,
            ..Self::new(size)
        }
    }

    /// True for an orthographic projection matrix.
    pub fn is_ortho(&self) -> bool {
        self.proj.z_axis.w.abs() < 1e-6
    }
}

/// One object to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawItem {
    pub mesh: MeshHandle,
    /// Object -> world transform (mm).
    pub model: Mat4,
    /// Draw the wire in [`SELECTED_WIRE_COLOR`] (and edges in Shaded mode).
    pub selected: bool,
    /// Object wire colour (sRGB).
    pub wire_color: Color32,
}

impl DrawItem {
    pub fn new(mesh: MeshHandle) -> Self {
        Self {
            mesh,
            model: Mat4::IDENTITY,
            selected: false,
            wire_color: Color32::from_rgb(0x8a, 0xb4, 0xe6),
        }
    }
}

/// Counts for an uploaded mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MeshStats {
    pub triangles: usize,
    pub edges: usize,
    pub curve_segments: usize,
    pub points: usize,
    pub loose_points: usize,
}

/// Per-draw uniform block; must match `Uniforms` in `shader.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    normal_mat: [[f32; 4]; 4],
    wire_color: [f32; 4],
    eye: [f32; 4],
    viewport: [f32; 4],
    params: [f32; 4],
}

struct GpuMesh {
    tri_vb: Option<wgpu::Buffer>,
    tri_ib: Option<wgpu::Buffer>,
    tri_index_count: u32,
    point_vb: Option<wgpu::Buffer>,
    point_count: u32,
    edge_ib: Option<wgpu::Buffer>,
    edge_index_count: u32,
    curve_ib: Option<wgpu::Buffer>,
    curve_index_count: u32,
    /// Loose-point subset; `None` with `loose_all` means "all points".
    loose_vb: Option<wgpu::Buffer>,
    loose_count: u32,
    loose_all: bool,
    has_point_color: bool,
    bounds: Aabb,
    stats: MeshStats,
}

struct Frame {
    size: [u32; 2],
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    msaa_view: Option<wgpu::TextureView>,
    depth_view: wgpu::TextureView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GridKey {
    plane: GridPlane,
    snapped: [i64; 3],
    extent: u32,
    spacing: u32,
    colors: [Color32; 3],
}

struct Target {
    frame: Frame,
    texture_id: Option<TextureId>,
    reregister: bool,
    uniform_buf: wgpu::Buffer,
    uniform_slots: usize,
    bind_group: wgpu::BindGroup,
    grid: Option<(GridSource, wgpu::Buffer, u32)>,
    occluder: Option<([Vec3; 4], wgpu::Buffer)>,
}

/// What the cached grid buffer was built from.
#[derive(Debug, Clone, PartialEq)]
enum GridSource {
    Built(GridKey),
    Lines(Vec<PointVertex>),
}

struct Pipelines {
    mesh: wgpu::RenderPipeline,
    edge: wgpu::RenderPipeline,
    curve: wgpu::RenderPipeline,
    point: wgpu::RenderPipeline,
    occluder: wgpu::RenderPipeline,
}

const MESH_ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3];
const LINE_ATTRS: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];

/// Offscreen mesh renderer for egui viewports. See the crate docs.
pub struct ViewportRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    samples: u32,
    uniform_stride: u64,
    bind_group_layout: wgpu::BindGroupLayout,
    pipelines: Pipelines,
    meshes: HashMap<u64, GpuMesh>,
    /// Generation each versioned mesh was uploaded at (see `upload_versioned`).
    generations: HashMap<u64, u64>,
    uploads: u64,
    targets: HashMap<u64, Target>,
}

impl ViewportRenderer {
    /// Create from eframe's render state (`frame.wgpu_render_state()` or
    /// `CreationContext::wgpu_render_state`). Uses 4x MSAA when the adapter
    /// supports it for both target formats.
    pub fn new(rs: &egui_wgpu::RenderState) -> Self {
        let c = rs.adapter.get_texture_format_features(COLOR_FORMAT).flags;
        let d = rs.adapter.get_texture_format_features(DEPTH_FORMAT).flags;
        let samples = if c.sample_count_supported(4) && d.sample_count_supported(4) {
            4
        } else {
            1
        };
        Self::with_device(&rs.device, &rs.queue, samples)
    }

    /// Create from a bare device/queue (headless use, tests). `samples` is
    /// the MSAA sample count (1 or 4).
    pub fn with_device(device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) -> Self {
        let samples = if samples >= 4 { 4 } else { 1 };
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment).max(1);
        let uniform_stride = UNIFORM_SIZE.div_ceil(align) * align;
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vernon-render uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(UNIFORM_SIZE),
                },
                count: None,
            }],
        });
        let pipelines = create_pipelines(device, &bind_group_layout, samples);
        Self {
            device: device.clone(),
            queue: queue.clone(),
            samples,
            uniform_stride,
            bind_group_layout,
            pipelines,
            meshes: HashMap::new(),
            generations: HashMap::new(),
            uploads: 0,
            targets: HashMap::new(),
        }
    }

    /// MSAA sample count in use (1 or 4).
    pub fn sample_count(&self) -> u32 {
        self.samples
    }

    /// Upload (or replace) the mesh cached under `key`.
    pub fn upload(&mut self, key: u64, geo: &Geometry) -> MeshHandle {
        let b = MeshBuffers::from_geometry(geo);
        self.upload_buffers(key, &b)
    }

    /// Upload `geo` under `key` only when `generation` differs from the one
    /// already cached (use the node cook generation). Returns the handle and
    /// whether an upload happened.
    pub fn upload_versioned(
        &mut self,
        key: u64,
        generation: u64,
        geo: &Geometry,
    ) -> (MeshHandle, bool) {
        if self.meshes.contains_key(&key) && self.generations.get(&key) == Some(&generation) {
            return (MeshHandle(key), false);
        }
        let h = self.upload(key, geo);
        self.generations.insert(key, generation);
        (h, true)
    }

    /// Number of mesh uploads performed so far (for cache tests).
    pub fn upload_count(&self) -> u64 {
        self.uploads
    }

    /// Upload pre-built buffers (lets callers build them off-thread).
    pub fn upload_buffers(&mut self, key: u64, b: &MeshBuffers) -> MeshHandle {
        self.uploads += 1;
        self.generations.remove(&key);
        let dev = &self.device;
        let buf = |label: &str, bytes: &[u8], usage: wgpu::BufferUsages| -> Option<wgpu::Buffer> {
            (!bytes.is_empty()).then(|| {
                dev.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: bytes,
                    usage,
                })
            })
        };
        let vtx = wgpu::BufferUsages::VERTEX;
        let idx = wgpu::BufferUsages::INDEX;
        let has_tris = !b.tri_indices.is_empty();
        let n_points = b.point_vertices.len();
        let loose_all = !b.loose_points.is_empty() && b.loose_points.len() == n_points;
        let loose: Vec<PointVertex> = if loose_all {
            Vec::new()
        } else {
            b.loose_points
                .iter()
                .map(|&i| b.point_vertices[i as usize])
                .collect()
        };
        let mesh = GpuMesh {
            tri_vb: if has_tris {
                buf("vernon tri vb", bytemuck::cast_slice(&b.tri_vertices), vtx)
            } else {
                None
            },
            tri_ib: buf("vernon tri ib", bytemuck::cast_slice(&b.tri_indices), idx),
            tri_index_count: b.tri_indices.len() as u32,
            point_vb: buf(
                "vernon point vb",
                bytemuck::cast_slice(&b.point_vertices),
                vtx,
            ),
            point_count: n_points as u32,
            edge_ib: buf("vernon edge ib", bytemuck::cast_slice(&b.edge_indices), idx),
            edge_index_count: b.edge_indices.len() as u32,
            curve_ib: buf(
                "vernon curve ib",
                bytemuck::cast_slice(&b.curve_indices),
                idx,
            ),
            curve_index_count: b.curve_indices.len() as u32,
            loose_vb: buf("vernon loose vb", bytemuck::cast_slice(&loose), vtx),
            loose_count: if loose_all {
                n_points as u32
            } else {
                loose.len() as u32
            },
            loose_all,
            has_point_color: b.has_point_color,
            bounds: b.bounds,
            stats: MeshStats {
                triangles: b.triangle_count(),
                edges: b.edge_count(),
                curve_segments: b.curve_segment_count(),
                points: n_points,
                loose_points: b.loose_points.len(),
            },
        };
        if let Some(old) = self.meshes.insert(key, mesh) {
            destroy_mesh(old);
        }
        MeshHandle(key)
    }

    pub fn has_mesh(&self, key: u64) -> bool {
        self.meshes.contains_key(&key)
    }

    /// Drop a cached mesh. Returns true if it existed.
    pub fn remove_mesh(&mut self, key: u64) -> bool {
        self.generations.remove(&key);
        self.meshes.remove(&key).map(destroy_mesh).is_some()
    }

    pub fn clear_meshes(&mut self) {
        self.generations.clear();
        for (_, m) in self.meshes.drain() {
            destroy_mesh(m);
        }
    }

    pub fn mesh_stats(&self, mesh: MeshHandle) -> Option<MeshStats> {
        self.meshes.get(&mesh.0).map(|m| m.stats)
    }

    /// Object-space bounds of a cached mesh.
    pub fn mesh_bounds(&self, mesh: MeshHandle) -> Option<Aabb> {
        self.meshes.get(&mesh.0).map(|m| m.bounds)
    }

    /// World-space bounds of a set of items (for zoom extents).
    pub fn items_bounds(&self, items: &[DrawItem]) -> Aabb {
        let mut out = Aabb::empty();
        for it in items {
            let Some(b) = self.mesh_bounds(it.mesh) else {
                continue;
            };
            if b.is_empty() {
                continue;
            }
            let m = it.model.as_dmat4();
            for i in 0..8 {
                let p = DVec3::new(
                    if i & 1 == 0 { b.min.x } else { b.max.x },
                    if i & 2 == 0 { b.min.y } else { b.max.y },
                    if i & 4 == 0 { b.min.z } else { b.max.z },
                );
                out.grow(m.transform_point3(p));
            }
        }
        out
    }

    /// Render a viewport and hand it to egui. Returns the texture id to
    /// show with `ui.image((id, rect.size()))`. The id is stable per
    /// `view.viewport_id`; a resize re-points it at the new texture.
    pub fn render(
        &mut self,
        rs: &egui_wgpu::RenderState,
        view: &ViewDesc,
        items: &[DrawItem],
    ) -> TextureId {
        self.draw(view, items);
        let t = self
            .targets
            .get_mut(&view.viewport_id)
            .expect("draw() creates the target");
        let filter = wgpu::FilterMode::Nearest;
        let mut r = rs.renderer.write();
        match t.texture_id {
            Some(id) if !t.reregister => id,
            Some(id) => {
                r.update_egui_texture_from_wgpu_texture(
                    &self.device,
                    &t.frame.color_view,
                    filter,
                    id,
                );
                t.reregister = false;
                id
            }
            None => {
                let id = r.register_native_texture(&self.device, &t.frame.color_view, filter);
                t.texture_id = Some(id);
                t.reregister = false;
                id
            }
        }
    }

    /// Free a viewport's offscreen target and its egui texture id.
    pub fn free_viewport(&mut self, rs: &egui_wgpu::RenderState, viewport_id: u64) {
        if let Some(t) = self.targets.remove(&viewport_id) {
            if let Some(id) = t.texture_id {
                rs.renderer.write().free_texture(&id);
            }
            t.frame.color.destroy();
        }
    }

    /// The resolved colour view of a viewport (after `draw`/`render`).
    pub fn texture_view(&self, viewport_id: u64) -> Option<&wgpu::TextureView> {
        self.targets.get(&viewport_id).map(|t| &t.frame.color_view)
    }

    /// Render a viewport offscreen without touching egui (headless use).
    pub fn draw(&mut self, view: &ViewDesc, items: &[DrawItem]) {
        let max = self.device.limits().max_texture_dimension_2d.max(1);
        let size = [view.size[0].clamp(1, max), view.size[1].clamp(1, max)];
        let slots = items.len() + 1;
        self.ensure_target(view.viewport_id, size, slots);

        // ---- uniforms -------------------------------------------------
        let view_proj = view.proj * view.view;
        let inv_view = view.view.inverse();
        let eye = if view.is_ortho() {
            let d = inv_view.z_axis.xyz().normalize_or_zero();
            [d.x, d.y, d.z, 0.0]
        } else {
            let p = inv_view.w_axis.xyz();
            [p.x, p.y, p.z, 1.0]
        };
        let base = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            model: Mat4::IDENTITY.to_cols_array_2d(),
            normal_mat: Mat4::IDENTITY.to_cols_array_2d(),
            wire_color: [0.0, 0.0, 0.0, 1.0],
            eye,
            viewport: [
                size[0] as f32,
                size[1] as f32,
                view.point_size.max(1.0),
                1.0,
            ],
            params: [1.0, AMBIENT, 0.0, 0.0],
        };
        let stride = self.uniform_stride as usize;
        let mut bytes = vec![0u8; slots * stride];
        bytes[..UNIFORM_SIZE as usize].copy_from_slice(bytemuck::bytes_of(&base));
        for (i, it) in items.iter().enumerate() {
            let has_color = self
                .meshes
                .get(&it.mesh.0)
                .is_some_and(|m| m.has_point_color);
            let wire = if it.selected {
                SELECTED_WIRE_COLOR
            } else {
                it.wire_color
            };
            let edge_alpha = match view.shading {
                ShadingMode::Shaded => SELECTED_EDGE_ALPHA,
                _ => 1.0,
            };
            let mut u = base;
            u.model = it.model.to_cols_array_2d();
            u.normal_mat = it.model.inverse().transpose().to_cols_array_2d();
            u.wire_color = linear(wire);
            u.viewport[3] = edge_alpha;
            u.params[0] = if has_color { 1.0 } else { 0.0 };
            let o = (i + 1) * stride;
            bytes[o..o + UNIFORM_SIZE as usize].copy_from_slice(bytemuck::bytes_of(&u));
        }

        let Self {
            device,
            queue,
            pipelines,
            meshes,
            targets,
            ..
        } = self;
        let target = targets.get_mut(&view.viewport_id).expect("ensured above");
        queue.write_buffer(&target.uniform_buf, 0, &bytes);

        // ---- grid -----------------------------------------------------
        let grid = if view.show_grid {
            let source = match &view.grid_lines {
                Some(lines) => GridSource::Lines(lines.clone()),
                None => GridSource::Built(grid_key(view)),
            };
            let stale = target.grid.as_ref().is_none_or(|(k, _, _)| *k != source);
            if stale {
                let verts = match &source {
                    GridSource::Lines(lines) => lines.clone(),
                    GridSource::Built(_) => build_grid(
                        view.grid_plane,
                        view.grid_center,
                        view.grid_extent,
                        view.grid_spacing,
                        &GridColors {
                            minor: rgb(view.grid_color),
                            major: rgb(view.grid_major_color),
                            axis: rgb(view.grid_axis_color),
                        },
                    ),
                };
                let b = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("vernon grid"),
                    contents: bytemuck::cast_slice(&verts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                if let Some((_, old, _)) = target.grid.take() {
                    old.destroy();
                }
                target.grid = Some((source, b, verts.len() as u32));
            }
            target.grid.as_ref().map(|(_, b, n)| (b, *n))
        } else {
            None
        };
        if target.occluder.as_ref().map(|(q, _)| q) != view.occluder.as_ref() {
            if let Some((_, old)) = target.occluder.take() {
                old.destroy();
            }
            target.occluder = view.occluder.map(|q| (q, occluder_buffer(device, &q)));
        }

        // ---- pass -----------------------------------------------------
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vernon viewport"),
        });
        {
            let f = &target.frame;
            let bg = view.background;
            let clear = wgpu::Color {
                r: bg.r() as f64 / 255.0,
                g: bg.g() as f64 / 255.0,
                b: bg.b() as f64 / 255.0,
                a: bg.a() as f64 / 255.0,
            };
            let (attach, resolve, store) = match &f.msaa_view {
                Some(m) => (m, Some(&f.color_view), wgpu::StoreOp::Discard),
                None => (&f.color_view, None, wgpu::StoreOp::Store),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("vernon viewport"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: attach,
                    resolve_target: resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &f.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let slot = |i: usize| (i * stride) as u32;

            if let Some((_, b)) = &target.occluder {
                pass.set_pipeline(&pipelines.occluder);
                pass.set_bind_group(0, &target.bind_group, &[slot(0)]);
                pass.set_vertex_buffer(0, b.slice(..));
                pass.draw(0..6, 0..1);
            }

            if let Some((b, n)) = grid {
                pass.set_pipeline(&pipelines.curve);
                pass.set_bind_group(0, &target.bind_group, &[slot(0)]);
                pass.set_vertex_buffer(0, b.slice(..));
                pass.draw(0..n, 0..1);
            }

            let shaded = matches!(view.shading, ShadingMode::Shaded | ShadingMode::ShadedEdges);
            let resolved: Vec<(usize, &DrawItem, &GpuMesh)> = items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| meshes.get(&it.mesh.0).map(|m| (i + 1, it, m)))
                .collect();

            if shaded {
                pass.set_pipeline(&pipelines.mesh);
                for (s, _, m) in &resolved {
                    if let (Some(vb), Some(ib)) = (&m.tri_vb, &m.tri_ib) {
                        pass.set_bind_group(0, &target.bind_group, &[slot(*s)]);
                        pass.set_vertex_buffer(0, vb.slice(..));
                        pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..m.tri_index_count, 0, 0..1);
                    }
                }
            }

            pass.set_pipeline(&pipelines.edge);
            for (s, it, m) in &resolved {
                let want = match view.shading {
                    ShadingMode::Wireframe | ShadingMode::ShadedEdges => true,
                    ShadingMode::Shaded => it.selected,
                    ShadingMode::Points => false,
                };
                if let (true, Some(vb), Some(ib)) = (want, &m.point_vb, &m.edge_ib) {
                    pass.set_bind_group(0, &target.bind_group, &[slot(*s)]);
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..m.edge_index_count, 0, 0..1);
                }
            }

            if view.shading != ShadingMode::Points {
                pass.set_pipeline(&pipelines.curve);
                for (s, _, m) in &resolved {
                    if let (Some(vb), Some(ib)) = (&m.point_vb, &m.curve_ib) {
                        pass.set_bind_group(0, &target.bind_group, &[slot(*s)]);
                        pass.set_vertex_buffer(0, vb.slice(..));
                        pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..m.curve_index_count, 0, 0..1);
                    }
                }
            }

            pass.set_pipeline(&pipelines.point);
            for (s, _, m) in &resolved {
                let (vb, n) = if view.shading == ShadingMode::Points || m.loose_all {
                    (m.point_vb.as_ref(), m.point_count)
                } else {
                    (m.loose_vb.as_ref(), m.loose_count)
                };
                if let (Some(vb), true) = (vb, n > 0) {
                    pass.set_bind_group(0, &target.bind_group, &[slot(*s)]);
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.draw(0..6, 0..n);
                }
            }
        }
        queue.submit(Some(encoder.finish()));
    }

    /// Read back a viewport's last rendered image as tightly packed RGBA8
    /// (gamma-encoded, rows top to bottom). Blocks until the GPU is done.
    pub fn read_pixels(&self, viewport_id: u64) -> Option<(u32, u32, Vec<u8>)> {
        let t = self.targets.get(&viewport_id)?;
        let [w, h] = t.frame.size;
        let row = w * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vernon readback"),
            size: u64::from(padded) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("vernon readback"),
            });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &t.frame.color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(enc.finish()));
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::Wait).ok()?;
        rx.recv().ok()?.ok()?;
        let mut out = Vec::with_capacity((row * h) as usize);
        {
            let data = slice.get_mapped_range();
            for y in 0..h as usize {
                let start = y * padded as usize;
                out.extend_from_slice(&data[start..start + row as usize]);
            }
        }
        buf.unmap();
        buf.destroy();
        Some((w, h, out))
    }

    fn ensure_target(&mut self, id: u64, size: [u32; 2], slots: usize) {
        let samples = self.samples;
        let stride = self.uniform_stride;
        let device = &self.device;
        let layout = &self.bind_group_layout;
        let target = self.targets.entry(id).or_insert_with(|| {
            let cap = slots.next_power_of_two().max(16);
            let (uniform_buf, bind_group) = create_uniforms(device, layout, cap, stride);
            Target {
                frame: create_frame(device, size, samples),
                texture_id: None,
                reregister: false,
                uniform_buf,
                uniform_slots: cap,
                bind_group,
                grid: None,
                occluder: None,
            }
        });
        if target.frame.size != size {
            target.frame.color.destroy();
            target.frame = create_frame(device, size, samples);
            target.reregister = true;
        }
        if target.uniform_slots < slots {
            let cap = slots.next_power_of_two();
            let (b, g) = create_uniforms(device, layout, cap, stride);
            target.uniform_buf.destroy();
            target.uniform_buf = b;
            target.bind_group = g;
            target.uniform_slots = cap;
        }
    }
}

impl Drop for ViewportRenderer {
    fn drop(&mut self) {
        self.clear_meshes();
    }
}

fn destroy_mesh(m: GpuMesh) {
    for b in [
        m.tri_vb, m.tri_ib, m.point_vb, m.edge_ib, m.curve_ib, m.loose_vb,
    ]
    .into_iter()
    .flatten()
    {
        b.destroy();
    }
}

/// Linear RGBA from an sRGB egui colour (alpha forced opaque).
fn linear(c: Color32) -> [f32; 4] {
    let l = Rgba::from(c);
    [l.r(), l.g(), l.b(), 1.0]
}

fn rgb(c: Color32) -> [f32; 3] {
    let l = linear(c);
    [l[0], l[1], l[2]]
}

/// Two triangles of an occluder quad; only their positions matter.
fn occluder_buffer(device: &wgpu::Device, q: &[Vec3; 4]) -> wgpu::Buffer {
    let verts = [q[0], q[1], q[2], q[0], q[2], q[3]].map(|p| MeshVertex {
        pos: p.to_array(),
        normal: [0.0, 0.0, 1.0],
        color: [0.0; 3],
    });
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("vernon occluder"),
        contents: bytemuck::cast_slice(&verts),
        usage: wgpu::BufferUsages::VERTEX,
    })
}

fn grid_key(view: &ViewDesc) -> GridKey {
    let s = effective_spacing(view.grid_spacing, view.grid_extent);
    let c = view.grid_center / s;
    GridKey {
        plane: view.grid_plane,
        snapped: [c.x.round() as i64, c.y.round() as i64, c.z.round() as i64],
        extent: (view.grid_extent / s).ceil().max(0.0) as u32,
        spacing: s.to_bits(),
        colors: [view.grid_color, view.grid_major_color, view.grid_axis_color],
    }
}

fn create_frame(device: &wgpu::Device, size: [u32; 2], samples: u32) -> Frame {
    let extent = wgpu::Extent3d {
        width: size[0],
        height: size[1],
        depth_or_array_layers: 1,
    };
    let tex = |label: &str, format, sample_count, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: extent,
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let color = tex(
        "vernon viewport colour",
        COLOR_FORMAT,
        1,
        wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
    );
    let msaa_view = (samples > 1).then(|| {
        tex(
            "vernon viewport msaa",
            COLOR_FORMAT,
            samples,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        )
        .create_view(&Default::default())
    });
    let depth_view = tex(
        "vernon viewport depth",
        DEPTH_FORMAT,
        samples,
        wgpu::TextureUsages::RENDER_ATTACHMENT,
    )
    .create_view(&Default::default());
    Frame {
        size,
        color_view: color.create_view(&Default::default()),
        color,
        msaa_view,
        depth_view,
    }
}

fn create_uniforms(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    slots: usize,
    stride: u64,
) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vernon uniforms"),
        size: stride * slots as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vernon uniforms"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buf,
                offset: 0,
                size: NonZeroU64::new(UNIFORM_SIZE),
            }),
        }],
    });
    (buf, group)
}

fn create_pipelines(device: &wgpu::Device, bgl: &wgpu::BindGroupLayout, samples: u32) -> Pipelines {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("vernon viewport shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER_WGSL.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("vernon viewport"),
        bind_group_layouts: &[bgl],
        push_constant_ranges: &[],
    });
    let mesh_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<MeshVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &MESH_ATTRS,
    };
    let line_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<PointVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &LINE_ATTRS,
    };
    let point_layout = wgpu::VertexBufferLayout {
        step_mode: wgpu::VertexStepMode::Instance,
        ..line_layout.clone()
    };
    struct Spec<'a> {
        label: &'a str,
        vs: &'a str,
        fs: &'a str,
        buffer: wgpu::VertexBufferLayout<'a>,
        topology: wgpu::PrimitiveTopology,
        depth_write: bool,
        bias: wgpu::DepthBiasState,
        blend: wgpu::BlendState,
        write_mask: wgpu::ColorWrites,
    }
    let make = |s: Spec<'_>| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(s.label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some(s.vs),
                compilation_options: Default::default(),
                buffers: &[s.buffer],
            },
            primitive: wgpu::PrimitiveState {
                topology: s.topology,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: s.depth_write,
                // Reverse-Z: nearer is larger.
                depth_compare: wgpu::CompareFunction::GreaterEqual,
                stencil: Default::default(),
                bias: s.bias,
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(s.fs),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(s.blend),
                    write_mask: s.write_mask,
                })],
            }),
            multiview: None,
            cache: None,
        })
    };
    let none = wgpu::DepthBiasState::default();
    let all = wgpu::ColorWrites::ALL;
    // Push faces away (reverse-Z => negative) so edges and grid lines win.
    let faces_back = wgpu::DepthBiasState {
        constant: -8,
        slope_scale: -1.5,
        clamp: 0.0,
    };
    Pipelines {
        mesh: make(Spec {
            label: "vernon mesh",
            vs: "vs_mesh",
            fs: "fs_mesh",
            buffer: mesh_layout.clone(),
            topology: wgpu::PrimitiveTopology::TriangleList,
            depth_write: true,
            bias: faces_back,
            blend: wgpu::BlendState::REPLACE,
            write_mask: all,
        }),
        occluder: make(Spec {
            label: "vernon occluder",
            vs: "vs_mesh",
            fs: "fs_mesh",
            buffer: mesh_layout,
            topology: wgpu::PrimitiveTopology::TriangleList,
            depth_write: true,
            bias: faces_back,
            blend: wgpu::BlendState::REPLACE,
            write_mask: wgpu::ColorWrites::empty(),
        }),
        edge: make(Spec {
            label: "vernon edges",
            vs: "vs_edge",
            fs: "fs_flat",
            buffer: line_layout.clone(),
            topology: wgpu::PrimitiveTopology::LineList,
            depth_write: false,
            bias: none,
            blend: wgpu::BlendState::ALPHA_BLENDING,
            write_mask: all,
        }),
        curve: make(Spec {
            label: "vernon curves",
            vs: "vs_curve",
            fs: "fs_flat",
            buffer: line_layout,
            topology: wgpu::PrimitiveTopology::LineList,
            depth_write: true,
            bias: none,
            blend: wgpu::BlendState::REPLACE,
            write_mask: all,
        }),
        point: make(Spec {
            label: "vernon points",
            vs: "vs_point",
            fs: "fs_flat",
            buffer: point_layout,
            topology: wgpu::PrimitiveTopology::TriangleList,
            depth_write: true,
            bias: none,
            blend: wgpu::BlendState::REPLACE,
            write_mask: all,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_block_is_256_bytes() {
        assert_eq!(UNIFORM_SIZE, 256);
    }

    #[test]
    fn view_desc_from_camera() {
        let cam = Camera::preset(crate::ViewPreset::Front);
        let v = ViewDesc::from_camera(&cam, [800, 600]);
        assert!(v.is_ortho());
        assert_eq!(v.grid_plane, GridPlane::YZ);
        let p = ViewDesc::from_camera(&Camera::default(), [800, 600]);
        assert!(!p.is_ortho());
        assert_eq!(p.grid_plane, GridPlane::XY);
    }

    #[test]
    fn colour_conversion_round_trips_through_shader_gamma() {
        // linear() then the shader's gamma encode should give back ~sRGB.
        let c = Color32::from_rgb(200, 100, 30);
        let l = linear(c);
        let enc = |x: f32| {
            if x < 0.0031308 {
                x * 12.92
            } else {
                1.055 * x.powf(1.0 / 2.4) - 0.055
            }
        };
        assert!((enc(l[0]) * 255.0 - 200.0).abs() < 1.0);
        assert!((enc(l[1]) * 255.0 - 100.0).abs() < 1.0);
        assert!((enc(l[2]) * 255.0 - 30.0).abs() < 1.0);
    }
}

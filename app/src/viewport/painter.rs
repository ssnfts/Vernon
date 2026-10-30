//! CPU fallback backend: depth-sorted flat shading plus wire, drawn as egui
//! meshes (painter's algorithm). Display meshes above
//! [`super::mesh::MAX_DISPLAY_TRIS`] triangles arrive already decimated.

use eframe::egui::{epaint::Mesh, pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};
use glam::{DVec2, DVec3};

use super::{DrawItem, Shading, ViewParams, ViewportBackend};

/// Above this many visible triangles, edged-faces mode skips the edges.
pub const EDGE_LIMIT: usize = 60_000;
const CHUNK_VERTS: usize = 60_000;

#[derive(Default)]
pub struct PainterBackend;

struct Tri {
    depth: f32,
    p: [Pos2; 3],
    color: Color32,
    edge: Option<Color32>,
}

/// Collects triangles into egui meshes no larger than `CHUNK_VERTS`.
#[derive(Default)]
struct MeshBuilder {
    mesh: Mesh,
    shapes: Vec<Shape>,
}

impl MeshBuilder {
    fn reserve(&mut self, verts: usize) {
        if self.mesh.vertices.len() + verts > CHUNK_VERTS {
            self.shapes.push(Shape::mesh(std::mem::take(&mut self.mesh)));
        }
    }

    fn tri(&mut self, p: [Pos2; 3], c: Color32) {
        self.reserve(3);
        let i = self.mesh.vertices.len() as u32;
        for q in p {
            self.mesh.colored_vertex(q, c);
        }
        self.mesh.add_triangle(i, i + 1, i + 2);
    }

    fn quad(&mut self, p: [Pos2; 4], c: Color32) {
        self.reserve(4);
        let i = self.mesh.vertices.len() as u32;
        for q in p {
            self.mesh.colored_vertex(q, c);
        }
        self.mesh.add_triangle(i, i + 1, i + 2);
        self.mesh.add_triangle(i, i + 2, i + 3);
    }

    fn line(&mut self, a: Pos2, b: Pos2, width: f32, c: Color32) {
        let d = b - a;
        let len = d.length();
        if len < 1e-3 {
            return;
        }
        let n = vec2(-d.y, d.x) / len * (width * 0.5);
        self.quad([a + n, b + n, b - n, a - n], c);
    }

    fn dot(&mut self, p: Pos2, r: f32, c: Color32) {
        self.quad(
            [
                pos2(p.x - r, p.y - r),
                pos2(p.x + r, p.y - r),
                pos2(p.x + r, p.y + r),
                pos2(p.x - r, p.y + r),
            ],
            c,
        );
    }

    fn finish(mut self, painter: &Painter) {
        if !self.mesh.is_empty() {
            self.shapes.push(Shape::mesh(self.mesh));
        }
        painter.extend(self.shapes);
    }
}

fn shade(c: Color32, k: f64) -> Color32 {
    let k = k.clamp(0.0, 1.0);
    let s = |v: u8| (v as f64 * k).round() as u8;
    Color32::from_rgb(s(c.r()), s(c.g()), s(c.b()))
}

impl ViewportBackend for PainterBackend {
    fn name(&self) -> &str {
        "CPU painter (fallback)"
    }

    fn paint(&mut self, painter: &Painter, rect: Rect, view: &ViewParams, items: &[DrawItem]) {
        let painter = painter.with_clip_rect(rect);
        let cam = &view.camera;
        let size = DVec2::new(rect.width() as f64, rect.height() as f64);
        let inv = cam.rot.inverse();
        let eye = cam.eye();
        let total: usize = items.iter().map(|i| i.mesh.tris.len()).sum();
        let edged = view.edged && total <= EDGE_LIMIT;
        let mut tris: Vec<Tri> = Vec::with_capacity(if view.shading == Shading::Shaded { total } else { 0 });
        let mut wire = MeshBuilder::default();
        let mut overlay: Vec<Shape> = Vec::new();

        // No depth buffer here: the grid goes behind everything.
        if let Some(grid) = &view.grid {
            for s in &grid.segments {
                if let Some(pts) = project_segment(cam, rect, size, s.a, s.b) {
                    painter.line_segment(pts, Stroke::new(1.0_f32, s.color));
                }
            }
        }

        for item in items {
            let m = &item.mesh;
            let cams: Vec<DVec3> = m.positions.iter().map(|p| inv * (p.as_dvec3() - eye)).collect();
            let proj: Vec<Option<Pos2>> = cams
                .iter()
                .map(|c| {
                    cam.project_cam(*c, size)
                        .map(|(s, _)| rect.min + vec2(s.x as f32, s.y as f32))
                })
                .collect();
            let wire_color = if item.selected { Color32::WHITE } else { item.color };
            match view.shading {
                Shading::Shaded => {
                    let edge = edged.then_some(if item.selected {
                        Color32::from_white_alpha(200)
                    } else {
                        Color32::from_black_alpha(130)
                    });
                    for (ti, t) in m.tris.iter().enumerate() {
                        let [ia, ib, ic] = t.map(|i| i as usize);
                        let (Some(a), Some(b), Some(c)) = (proj[ia], proj[ib], proj[ic]) else {
                            continue;
                        };
                        let (ca, cb, cc) = (cams[ia], cams[ib], cams[ic]);
                        let n = (cb - ca).cross(cc - ca).normalize_or_zero();
                        let to_eye = if cam.ortho {
                            DVec3::Z
                        } else {
                            (-(ca + cb + cc) / 3.0).normalize_or_zero()
                        };
                        let k = 0.28 + 0.72 * n.dot(to_eye).abs();
                        let base = m
                            .tri_colors
                            .as_ref()
                            .map(|c| Color32::from_rgb(c[ti][0], c[ti][1], c[ti][2]))
                            .unwrap_or(item.color);
                        tris.push(Tri {
                            depth: (-(ca.z + cb.z + cc.z) / 3.0) as f32,
                            p: [a, b, c],
                            color: shade(base, k),
                            edge,
                        });
                    }
                }
                Shading::Wireframe => {
                    for t in &m.tris {
                        let [a, b, c] = t.map(|i| proj[i as usize]);
                        for (p, q) in [(a, b), (b, c), (c, a)] {
                            if let (Some(p), Some(q)) = (p, q) {
                                wire.line(p, q, 1.0, wire_color);
                            }
                        }
                    }
                }
            }
            for l in &m.lines {
                if let (Some(a), Some(b)) = (proj[l[0] as usize], proj[l[1] as usize]) {
                    wire.line(a, b, 1.4, wire_color);
                }
            }
            for &i in &m.dots {
                if let Some(p) = proj[i as usize] {
                    let c = match (&m.point_colors, item.selected) {
                        (Some(pc), _) => {
                            let c = pc[i as usize];
                            Color32::from_rgb(c[0], c[1], c[2])
                        }
                        (None, true) => Color32::WHITE,
                        (None, false) => item.color,
                    };
                    wire.dot(p, 1.2, c);
                }
            }
            if item.selected {
                brackets(&mut overlay, cam, rect, size, m.min.as_dvec3(), m.max.as_dvec3());
            }
        }

        tris.sort_by(|a, b| b.depth.total_cmp(&a.depth));
        let mut faces = MeshBuilder::default();
        for t in &tris {
            faces.tri(t.p, t.color);
            if let Some(ec) = t.edge {
                faces.line(t.p[0], t.p[1], 1.0, ec);
                faces.line(t.p[1], t.p[2], 1.0, ec);
                faces.line(t.p[2], t.p[0], 1.0, ec);
            }
        }
        faces.finish(&painter);
        wire.finish(&painter);
        painter.extend(overlay);
    }
}

/// Clip a world segment to the near plane and project it.
fn project_segment(cam: &super::Camera, rect: Rect, size: DVec2, a: DVec3, b: DVec3) -> Option<[Pos2; 2]> {
    let (mut ca, mut cb) = (cam.world_to_cam(a), cam.world_to_cam(b));
    if !cam.ortho {
        let near = cam.near() * 2.0;
        let (da, db) = (-ca.z, -cb.z);
        if da < near && db < near {
            return None;
        }
        if da < near {
            ca += (cb - ca) * ((near - da) / (db - da));
        } else if db < near {
            cb += (ca - cb) * ((near - db) / (da - db));
        }
    }
    let (sa, _) = cam.project_cam(ca, size)?;
    let (sb, _) = cam.project_cam(cb, size)?;
    Some([rect.min + vec2(sa.x as f32, sa.y as f32), rect.min + vec2(sb.x as f32, sb.y as f32)])
}

/// White corner brackets around a selected object's bounding box.
pub(crate) fn brackets(out: &mut Vec<Shape>, cam: &super::Camera, rect: Rect, size: DVec2, min: DVec3, max: DVec3) {
    let stroke = Stroke::new(1.0_f32, Color32::WHITE);
    let ext = max - min;
    for i in 0..8 {
        let s = DVec3::new((i & 1) as f64, ((i >> 1) & 1) as f64, ((i >> 2) & 1) as f64);
        let corner = min + ext * s;
        for axis in 0..3 {
            let mut d = DVec3::ZERO;
            d[axis] = if s[axis] > 0.5 { -ext[axis] } else { ext[axis] } * 0.25;
            if d[axis] == 0.0 {
                continue;
            }
            if let (Some((a, _)), Some((b, _))) = (cam.project(corner, size), cam.project(corner + d, size)) {
                out.push(Shape::line_segment(
                    [rect.min + vec2(a.x as f32, a.y as f32), rect.min + vec2(b.x as f32, b.y as f32)],
                    stroke,
                ));
            }
        }
    }
}

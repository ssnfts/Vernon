//! Quad viewports: tiles, splitters, label menus, orientation gizmo, home
//! grid, navigation (MMB pan, Alt+MMB orbit, wheel zoom) and the
//! Select/Move/Rotate/Scale tools.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CursorIcon, FontId, Painter, PointerButton, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, Ui, UiBuilder,
};
use glam::{DVec2, DVec3, Vec3};
use vernon_core::{Aabb, Geometry};
use vernon_graph::{NodeId, ParamValue};

use super::{Tool, VernonApp};
use crate::state::AppState;
use crate::theme;
use crate::viewport::camera::{Camera, OrbitMode, ViewKind};
use crate::viewport::grid::{self, GridSettings};
use crate::viewport::layout::QuadLayout;
use crate::viewport::mesh::{self, ColorBy, RenderMesh};
use crate::viewport::{DrawItem, Shading, ViewParams};

const DEFAULT_KINDS: [ViewKind; 4] = [ViewKind::Top, ViewKind::Front, ViewKind::Left, ViewKind::Perspective];

pub struct Tile {
    pub kind: ViewKind,
    pub cam: Camera,
    pub shading: Shading,
    pub edged: bool,
    pub rect: Rect,
    history: Vec<Camera>,
    future: Vec<Camera>,
}

impl Tile {
    fn new(kind: ViewKind) -> Self {
        Self {
            kind,
            cam: Camera::new(kind),
            shading: Shading::Shaded,
            edged: false,
            rect: Rect::from_min_size(Pos2::ZERO, vec2(800.0, 450.0)),
            history: Vec::new(),
            future: Vec::new(),
        }
    }

    fn remember(&mut self) {
        self.history.push(self.cam);
        if self.history.len() > 100 {
            self.history.remove(0);
        }
        self.future.clear();
    }

    fn size(&self) -> DVec2 {
        DVec2::new(self.rect.width().max(1.0) as f64, self.rect.height().max(1.0) as f64)
    }
}

struct CachedMesh {
    geo: Arc<Geometry>,
    color_by: ColorBy,
    mesh: Arc<RenderMesh>,
}

enum Drag {
    Pan,
    Orbit,
    Band { tile: usize, from: Pos2 },
    Tool { tile: usize, targets: Vec<(NodeId, DVec3, DVec3, f64)>, total: egui::Vec2 },
    Splitter,
}

pub struct Viewports {
    pub tiles: [Tile; 4],
    pub layout: QuadLayout,
    pub grid: GridSettings,
    pub show_grid: bool,
    pub show_gizmo: bool,
    pub orbit: OrbitMode,
    /// Cursor position on the construction plane of the hovered tile.
    pub cursor_world: Option<DVec3>,
    /// Grid square size in the active tile, for the status bar.
    pub grid_readout: f64,
    meshes: HashMap<NodeId, CachedMesh>,
    drag: Option<Drag>,
    fit_pending: bool,
}

impl Default for Viewports {
    fn default() -> Self {
        Self {
            tiles: DEFAULT_KINDS.map(Tile::new),
            layout: QuadLayout::default(),
            grid: GridSettings::default(),
            show_grid: true,
            show_gizmo: true,
            orbit: OrbitMode::Turntable,
            cursor_world: None,
            grid_readout: 10.0,
            meshes: HashMap::new(),
            drag: None,
            fit_pending: false,
        }
    }
}

impl Viewports {
    pub fn request_fit(&mut self) {
        self.fit_pending = true;
    }

    /// Frame the scene after the first object appears.
    pub fn request_fit_if_first(&mut self, st: &AppState) {
        if st.objects().len() == 1 {
            self.fit_pending = true;
        }
    }

    pub fn set_view(&mut self, tile: usize, kind: ViewKind) {
        let t = &mut self.tiles[tile];
        t.remember();
        t.kind = kind;
        t.cam.set_view(kind);
    }

    pub fn view_undo(&mut self, tile: usize) {
        let t = &mut self.tiles[tile];
        if let Some(c) = t.history.pop() {
            t.future.push(t.cam);
            t.cam = c;
        }
    }

    pub fn view_redo(&mut self, tile: usize) {
        let t = &mut self.tiles[tile];
        if let Some(c) = t.future.pop() {
            t.history.push(t.cam);
            t.cam = c;
        }
    }

    /// Bounds of visible objects (selected only when asked and any are).
    pub fn bounds(&self, st: &AppState, selected_only: bool) -> Aabb {
        let mut b = Aabb::empty();
        let sel_any = st.selection.iter().any(|s| self.meshes.contains_key(s));
        for (id, c) in &self.meshes {
            if st.is_hidden(*id) || (selected_only && sel_any && !st.selection.contains(id)) || c.mesh.is_empty() {
                continue;
            }
            b.grow(c.mesh.min.as_dvec3());
            b.grow(c.mesh.max.as_dvec3());
        }
        b
    }

    /// Zoom extents for one tile, or all tiles with `None`.
    pub fn zoom_extents(&mut self, st: &AppState, tile: Option<usize>, selected_only: bool) {
        let b = self.bounds(st, selected_only);
        let range = match tile {
            Some(i) => i..i + 1,
            None => 0..4,
        };
        for i in range {
            let t = &mut self.tiles[i];
            t.remember();
            let aspect = (t.rect.width() / t.rect.height().max(1.0)) as f64;
            t.cam.fit(&b, aspect, 1.1);
        }
    }

    /// Refresh the display-mesh cache and return what to draw.
    fn items(&mut self, st: &AppState, color_by: ColorBy) -> Vec<DrawItem> {
        let objects = st.objects();
        self.meshes.retain(|id, _| objects.contains(id));
        let mut out = Vec::new();
        for obj in objects {
            let Some(geo) = st.geometry(st.display_node(obj)) else {
                self.meshes.remove(&obj);
                continue;
            };
            let fresh = match self.meshes.get(&obj) {
                Some(c) => !(Arc::ptr_eq(&c.geo, &geo) && c.color_by == color_by),
                None => true,
            };
            if fresh {
                let mesh = Arc::new(mesh::build(&geo, color_by));
                self.meshes.insert(obj, CachedMesh { geo, color_by, mesh });
            }
            if st.is_hidden(obj) {
                continue;
            }
            let c = st.color_of(obj);
            out.push(DrawItem {
                id: obj,
                mesh: self.meshes[&obj].mesh.clone(),
                color: Color32::from_rgb(c[0], c[1], c[2]),
                selected: st.selection.contains(&obj),
            });
        }
        out
    }
}

/// The construction plane of a view: normal and the two in-plane axes.
fn plane_of(kind: ViewKind) -> (DVec3, DVec3, DVec3) {
    match kind {
        ViewKind::Front | ViewKind::Back => (DVec3::Y, DVec3::X, DVec3::Z),
        ViewKind::Left | ViewKind::Right => (DVec3::X, DVec3::Y, DVec3::Z),
        _ => (DVec3::Z, DVec3::X, DVec3::Y),
    }
}

fn to_screen(rect: Rect, s: DVec2) -> Pos2 {
    rect.min + vec2(s.x as f32, s.y as f32)
}

/// Clip a world segment to the near plane and project it.
fn project_segment(cam: &Camera, rect: Rect, size: DVec2, a: DVec3, b: DVec3) -> Option<(Pos2, Pos2)> {
    let (mut ca, mut cb) = (cam.world_to_cam(a), cam.world_to_cam(b));
    if !cam.ortho {
        let near = cam.near() * 2.0;
        let (da, db) = (-ca.z, -cb.z);
        if da < near && db < near {
            return None;
        }
        if da < near {
            ca = ca + (cb - ca) * ((near - da) / (db - da));
        } else if db < near {
            cb = cb + (ca - cb) * ((near - db) / (da - db));
        }
    }
    let (sa, _) = cam.project_cam(ca, size)?;
    let (sb, _) = cam.project_cam(cb, size)?;
    Some((to_screen(rect, sa), to_screen(rect, sb)))
}

/// Draw the home grid; returns the grid square size used.
fn draw_grid(painter: &Painter, rect: Rect, tile: &Tile, gs: &GridSettings) -> f64 {
    let cam = &tile.cam;
    let size = tile.size();
    let wpp = cam.world_per_pixel(size.y);
    let spacing = if gs.adaptive {
        grid::adaptive_spacing(gs.spacing, wpp, gs.inhibit_subdivision)
    } else {
        gs.spacing
    };
    let (n, u, v) = plane_of(tile.kind);
    let minor = Color32::from_rgb(0x44, 0x44, 0x44);
    let major = Color32::from_rgb(0x52, 0x52, 0x52);
    let axis = Color32::from_rgb(0x22, 0x22, 0x22);
    let axis_aligned = cam.back().dot(n).abs() > 0.999;
    if cam.ortho && axis_aligned {
        // Lines across the whole visible region.
        let corners = [
            cam.focal_point(DVec2::ZERO, size),
            cam.focal_point(size, size),
            cam.focal_point(DVec2::new(size.x, 0.0), size),
            cam.focal_point(DVec2::new(0.0, size.y), size),
        ];
        let lo_u = corners.iter().map(|c| c.dot(u)).fold(f64::MAX, f64::min);
        let hi_u = corners.iter().map(|c| c.dot(u)).fold(f64::MIN, f64::max);
        let lo_v = corners.iter().map(|c| c.dot(v)).fold(f64::MAX, f64::min);
        let hi_v = corners.iter().map(|c| c.dot(v)).fold(f64::MIN, f64::max);
        let draw = |dir: DVec3, across: DVec3, lo: f64, hi: f64, lo2: f64, hi2: f64| {
            let k0 = (lo / spacing).floor() as i64;
            let k1 = (hi / spacing).ceil() as i64;
            if k1 - k0 > 600 {
                return;
            }
            for line in grid::lines(k0, k1, gs.major_every) {
                let c = line.k as f64 * spacing;
                let a = dir * c + across * lo2;
                let b = dir * c + across * hi2;
                if let Some((pa, pb)) = project_segment(cam, rect, size, a, b) {
                    let col = if line.k == 0 { axis } else if line.major { major } else { minor };
                    painter.line_segment([pa, pb], Stroke::new(1.0_f32, col));
                }
            }
        };
        draw(u, v, lo_u, hi_u, lo_v, hi_v);
        draw(v, u, lo_v, hi_v, lo_u, hi_u);
    } else {
        let e = (gs.extent * gs.major_every).clamp(4, 200);
        let half = e as f64 * spacing;
        // Centre the grid on the major line nearest the view target.
        let step = spacing * gs.major_every as f64;
        let cu = (cam.target.dot(u) / step).round() * step;
        let cv = (cam.target.dot(v) / step).round() * step;
        for (dir, across, c0, c1) in [(u, v, cu, cv), (v, u, cv, cu)] {
            for line in grid::extent_lines(e, gs.major_every) {
                let world_k = ((c0 / spacing).round() as i64) + line.k;
                let c = world_k as f64 * spacing;
                let a = dir * c + across * (c1 - half);
                let b = dir * c + across * (c1 + half);
                let alpha = grid::fade(line.k, e);
                if alpha <= 0.0 {
                    continue;
                }
                if let Some((pa, pb)) = project_segment(cam, rect, size, a, b) {
                    let base = if world_k == 0 {
                        axis
                    } else if world_k % gs.major_every.max(2) == 0 {
                        major
                    } else {
                        minor
                    };
                    let col = theme::mix(theme::VIEWPORT_BG, base, alpha);
                    painter.line_segment([pa, pb], Stroke::new(1.0_f32, col));
                }
            }
        }
    }
    spacing
}

/// Orientation gizmo: world axes seen from the tile camera. Returns the view
/// to switch to when an axis end or the home mark was clicked.
fn gizmo(painter: &Painter, rect: Rect, cam: &Camera, click: Option<Pos2>, hover: Option<Pos2>) -> Option<ViewKind> {
    let c = pos2(rect.right() - 42.0, rect.top() + 44.0);
    let len = 26.0;
    let inv = cam.rot.inverse();
    let axes = [
        (DVec3::X, theme::AXIS_X, "X", ViewKind::Right, ViewKind::Left),
        (DVec3::Y, theme::AXIS_Y, "Y", ViewKind::Back, ViewKind::Front),
        (DVec3::Z, theme::AXIS_Z, "Z", ViewKind::Top, ViewKind::Bottom),
    ];
    painter.circle_filled(c, len + 9.0, Color32::from_black_alpha(40));
    let mut ends: Vec<(f64, Pos2, Color32, Option<&str>, ViewKind)> = Vec::new();
    for (a, col, name, pos_view, neg_view) in axes {
        let v = inv * a;
        let off = vec2(v.x as f32, -v.y as f32) * len;
        ends.push((v.z, c + off, col, Some(name), pos_view));
        ends.push((-v.z, c - off, col, None, neg_view));
    }
    ends.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut picked = None;
    for (_, p, col, name, view) in &ends {
        let hot = hover.is_some_and(|h| h.distance(*p) < 8.0);
        match name {
            Some(n) => {
                painter.line_segment([c, *p], Stroke::new(2.0_f32, *col));
                painter.circle_filled(*p, if hot { 8.0 } else { 6.5 }, *col);
                painter.text(*p, Align2::CENTER_CENTER, *n, FontId::proportional(10.0), Color32::BLACK);
            }
            None => {
                painter.circle_stroke(*p, if hot { 5.5 } else { 4.0 }, Stroke::new(1.5_f32, *col));
            }
        }
        if click.is_some_and(|k| k.distance(*p) < 8.0) {
            picked = Some(*view);
        }
    }
    // Home mark: a small house outline at the gizmo's upper left.
    let h = pos2(c.x - len - 6.0, c.y - len - 4.0);
    let hot = hover.is_some_and(|p| p.distance(h) < 8.0);
    let col = if hot { theme::ACCENT } else { theme::TEXT_DIM };
    let st = Stroke::new(1.3_f32, col);
    painter.line_segment([h + vec2(-6.0, 0.0), h + vec2(0.0, -6.0)], st);
    painter.line_segment([h + vec2(0.0, -6.0), h + vec2(6.0, 0.0)], st);
    painter.rect_stroke(Rect::from_min_max(h + vec2(-4.0, 0.0), h + vec2(4.0, 6.0)), 0.0, st, StrokeKind::Middle);
    if click.is_some_and(|k| k.distance(h) < 8.0) {
        picked = Some(ViewKind::User); // sentinel: home
    }
    picked
}

pub fn show(app: &mut VernonApp, ctx: &egui::Context) {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(theme::SURFACE_DEEP))
        .show(ctx, |ui| {
            let area = ui.max_rect();
            let items = app.vp.items(&app.st, app.color_by);
            let tiles = app.vp.layout.tiles(area);
            for (i, r) in tiles.iter().enumerate() {
                if let Some(r) = r {
                    app.vp.tiles[i].rect = *r;
                }
            }
            if app.vp.fit_pending && !items.is_empty() {
                app.vp.fit_pending = false;
                app.vp.zoom_extents(&app.st, None, false);
            }
            app.vp.cursor_world = None;
            for (i, r) in tiles.iter().enumerate() {
                if let Some(r) = r {
                    tile_ui(app, ui, i, *r, &items);
                }
            }
            splitters(app, ui, area);
        });
}

fn splitters(app: &mut VernonApp, ui: &mut Ui, area: Rect) {
    let bars = app.vp.layout.splitters(area);
    for (axis, bar) in bars.iter().enumerate() {
        if bar.width() <= 0.0 {
            continue;
        }
        let resp = ui.interact(bar.expand(2.0), ui.id().with(("split", axis)), Sense::drag());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(if axis == 0 {
                CursorIcon::ResizeHorizontal
            } else {
                CursorIcon::ResizeVertical
            });
        }
        if resp.drag_started() {
            app.vp.drag = Some(Drag::Splitter);
        }
        if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let v = if axis == 0 { p.x } else { p.y };
                app.vp.layout.drag_splitter(axis, v, area);
            }
        }
        if resp.drag_stopped() {
            app.vp.drag = None;
        }
        if resp.double_clicked() {
            app.vp.layout.split[axis] = 0.5;
        }
    }
}

fn label_menus(app: &mut VernonApp, ui: &mut Ui, i: usize, rect: Rect) {
    let area = Rect::from_min_size(rect.min + vec2(4.0, 3.0), vec2(rect.width() - 8.0, 22.0));
    ui.scope_builder(UiBuilder::new().max_rect(area), |ui| {
        let v = &mut ui.style_mut().visuals.widgets;
        v.inactive.weak_bg_fill = Color32::TRANSPARENT;
        v.inactive.bg_stroke = Stroke::NONE;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let small = |s: &str| RichText::new(s).size(12.0).color(theme::TEXT);
            ui.menu_button(small("[ View ]"), |ui| {
                let max = app.vp.layout.maximized;
                if ui.button(if max { "Restore Layout   Alt+W" } else { "Maximise   Alt+W" }).clicked() {
                    app.vp.layout.active = i;
                    app.vp.layout.toggle_maximize();
                }
                ui.checkbox(&mut app.vp.show_grid, "Show Grid (G)");
                ui.checkbox(&mut app.vp.show_gizmo, "Show Orientation Gizmo");
                if ui.button("Zoom Extents   Z").clicked() {
                    app.vp.zoom_extents(&app.st, Some(i), true);
                }
                if ui.button("Reset View").clicked() {
                    let t = &mut app.vp.tiles[i];
                    t.remember();
                    t.kind = DEFAULT_KINDS[i];
                    t.cam = Camera::new(t.kind);
                    app.vp.zoom_extents(&app.st, Some(i), false);
                }
                if ui.button("Reset Layout").clicked() {
                    app.vp.layout.reset();
                }
            });
            let kind = app.vp.tiles[i].kind;
            ui.menu_button(small(&format!("[ {} ]", kind.label())), |ui| {
                for k in ViewKind::MENU {
                    let text = if k.shortcut().is_empty() {
                        k.label().to_string()
                    } else {
                        format!("{}   {}", k.label(), k.shortcut())
                    };
                    if ui.selectable_label(kind == k, text).clicked() {
                        app.vp.set_view(i, k);
                    }
                }
            });
            let t = &mut app.vp.tiles[i];
            let shade = format!(
                "[ {}{} ]",
                t.shading.label(),
                if t.edged && t.shading == Shading::Shaded { " + Edges" } else { "" }
            );
            ui.menu_button(small(&shade), |ui| {
                ui.radio_value(&mut t.shading, Shading::Shaded, "Shaded   F3");
                ui.radio_value(&mut t.shading, Shading::Wireframe, "Wireframe   F3");
                ui.checkbox(&mut t.edged, "Edged Faces   F4");
            });
        });
    });
}

fn tile_ui(app: &mut VernonApp, ui: &mut Ui, i: usize, rect: Rect, items: &[DrawItem]) {
    let resp = ui.interact(rect, ui.id().with(("tile", i)), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::VIEWPORT_BG);
    let active = app.vp.layout.active == i;
    let size = app.vp.tiles[i].size();
    let hover = resp.hover_pos();

    // Activate on any press inside the tile.
    if resp.contains_pointer() && ui.input(|inp| inp.pointer.any_pressed()) {
        app.vp.layout.active = i;
    }

    // Grid and scene.
    if app.vp.show_grid {
        let s = draw_grid(&painter, rect, &app.vp.tiles[i], &app.vp.grid);
        if active {
            app.vp.grid_readout = s;
        }
    }
    let view = ViewParams {
        camera: app.vp.tiles[i].cam,
        shading: app.vp.tiles[i].shading,
        edged: app.vp.tiles[i].edged,
    };
    app.backend.paint(&painter, rect, &view, items);

    // Cursor readout on the construction plane.
    if let Some(h) = hover {
        let (n, _, _) = plane_of(app.vp.tiles[i].kind);
        let px = DVec2::new((h.x - rect.min.x) as f64, (h.y - rect.min.y) as f64);
        app.vp.cursor_world = app.vp.tiles[i].cam.plane_point(px, size, n, 0.0);
    }

    // Gizmo (handles its own clicks before picking).
    let click_pos = if resp.clicked_by(PointerButton::Primary) {
        resp.interact_pointer_pos()
    } else {
        None
    };
    let mut gizmo_hit = false;
    if app.vp.show_gizmo {
        if let Some(v) = gizmo(&painter, rect, &app.vp.tiles[i].cam, click_pos, hover) {
            gizmo_hit = true;
            if v == ViewKind::User {
                // Home: back to the default view of this tile, framed.
                let t = &mut app.vp.tiles[i];
                t.remember();
                t.kind = DEFAULT_KINDS[i];
                t.cam.set_view(t.kind);
                app.vp.zoom_extents(&app.st, Some(i), false);
            } else {
                app.vp.set_view(i, v);
            }
        }
    }

    navigate(app, ui, &resp, i, rect);
    tools(app, ui, &resp, i, rect, items, gizmo_hit, &painter);

    // Quad-style context menu: right-click activates without changing selection.
    resp.context_menu(|ui| {
        app.vp.layout.active = i;
        ui.label(RichText::new("Transform").small().color(theme::TEXT_DIM));
        for (tool, text) in [(Tool::Select, "Select   Q"), (Tool::Move, "Move   W"), (Tool::Rotate, "Rotate   E"), (Tool::Scale, "Scale   R")] {
            if ui.selectable_label(app.tool == tool, text).clicked() {
                app.tool = tool;
            }
        }
        ui.separator();
        ui.label(RichText::new("Display").small().color(theme::TEXT_DIM));
        if ui.button("Hide Selection   Alt+H").clicked() {
            app.hide_selected();
        }
        if ui.button("Unhide All   Alt+U").clicked() {
            app.unhide_all();
        }
        if ui.button("Zoom Extents   Z").clicked() {
            app.vp.zoom_extents(&app.st, Some(i), true);
        }
        ui.separator();
        if ui.add_enabled(!app.st.selection.is_empty(), egui::Button::new("Delete   Del")).clicked() {
            app.st.delete_selected();
        }
        if ui.button("Properties (Modify panel)").clicked() {
            app.tab = super::PanelTab::Modify;
        }
    });

    label_menus(app, ui, i, rect);

    // Colour legend in the active tile.
    if active && app.color_by != ColorBy::Object && app.color_by != ColorBy::Cd {
        if let Some((lo, hi)) = items.iter().find_map(|it| it.mesh.range) {
            legend(&painter, rect, lo, hi, app.color_by);
        }
    }
    // Decimation note.
    let decimated: usize = items.iter().filter(|it| it.mesh.is_decimated()).count();
    if decimated > 0 && active {
        painter.text(
            rect.left_bottom() + vec2(6.0, -6.0),
            Align2::LEFT_BOTTOM,
            format!("{decimated} object(s) decimated for display (>{} tris)", mesh::MAX_DISPLAY_TRIS),
            FontId::proportional(11.0),
            theme::TEXT_DIM,
        );
    }
    let border = if active {
        Stroke::new(2.0_f32, theme::ACCENT)
    } else {
        Stroke::new(1.0_f32, Color32::from_rgb(0x2A, 0x2A, 0x2A))
    };
    painter.rect_stroke(rect, 0.0, border, StrokeKind::Inside);
}

fn legend(painter: &Painter, rect: Rect, lo: f64, hi: f64, by: ColorBy) {
    let r = Rect::from_min_size(rect.right_bottom() + vec2(-150.0, -40.0), vec2(120.0, 10.0));
    let n = 24;
    for k in 0..n {
        let c = mesh::ramp(k as f64 / (n - 1) as f64);
        let w = r.width() / n as f32;
        painter.rect_filled(
            Rect::from_min_size(pos2(r.left() + w * k as f32, r.top()), vec2(w + 0.5, r.height())),
            0.0,
            Color32::from_rgb(c[0], c[1], c[2]),
        );
    }
    let unit = if by == ColorBy::TempC { "°C" } else { "mm" };
    let f = FontId::proportional(10.0);
    painter.text(r.left_bottom() + vec2(0.0, 2.0), Align2::LEFT_TOP, format!("{lo:.2}"), f.clone(), theme::TEXT);
    painter.text(r.right_bottom() + vec2(0.0, 2.0), Align2::RIGHT_TOP, format!("{hi:.2} {unit}"), f.clone(), theme::TEXT);
    painter.text(r.left_top() - vec2(0.0, 2.0), Align2::LEFT_BOTTOM, by.label(), f, theme::TEXT_DIM);
}

fn navigate(app: &mut VernonApp, ui: &mut Ui, resp: &egui::Response, i: usize, rect: Rect) {
    let size = app.vp.tiles[i].size();
    let alt = ui.input(|inp| inp.modifiers.alt);
    // Wheel zoom about the cursor.
    if resp.contains_pointer() {
        let scroll = ui.input(|inp| inp.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            if let Some(h) = resp.hover_pos() {
                let px = DVec2::new((h.x - rect.min.x) as f64, (h.y - rect.min.y) as f64);
                app.vp.tiles[i].cam.zoom_at(px, size, scroll as f64 / 50.0);
            }
        }
    }
    if resp.drag_started_by(PointerButton::Middle) {
        app.vp.tiles[i].remember();
        app.vp.drag = Some(if alt { Drag::Orbit } else { Drag::Pan });
    }
    if resp.dragged_by(PointerButton::Middle) {
        let d = resp.drag_delta();
        let delta = DVec2::new(d.x as f64, d.y as f64);
        let orbit = alt || matches!(app.vp.drag, Some(Drag::Orbit));
        let t = &mut app.vp.tiles[i];
        if orbit {
            let pivot = t.cam.target;
            match app.vp.orbit {
                OrbitMode::Turntable => t.cam.orbit_turntable(delta, pivot),
                OrbitMode::Arcball => {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let p1 = DVec2::new((p.x - rect.min.x) as f64, (p.y - rect.min.y) as f64);
                        t.cam.orbit_arcball(p1 - delta, p1, size, pivot);
                    }
                }
            }
            if t.kind.is_ortho() && t.kind != ViewKind::User {
                t.kind = ViewKind::User;
            }
            ui.ctx().set_cursor_icon(CursorIcon::AllScroll);
        } else {
            t.cam.pan(delta, size.y);
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
    }
    if resp.drag_stopped_by(PointerButton::Middle) {
        app.vp.drag = None;
    }
}

#[allow(clippy::too_many_arguments)]
fn tools(
    app: &mut VernonApp,
    ui: &mut Ui,
    resp: &egui::Response,
    i: usize,
    rect: Rect,
    items: &[DrawItem],
    gizmo_hit: bool,
    painter: &Painter,
) {
    let size = app.vp.tiles[i].size();
    let (ctrl, alt) = ui.input(|inp| (inp.modifiers.ctrl || inp.modifiers.command, inp.modifiers.alt));
    let to_px = |p: Pos2| DVec2::new((p.x - rect.min.x) as f64, (p.y - rect.min.y) as f64);

    // Click: pick.
    if resp.clicked_by(PointerButton::Primary) && !gizmo_hit {
        if let Some(p) = resp.interact_pointer_pos() {
            let cam = app.vp.tiles[i].cam;
            let (o, d) = cam.ray(to_px(p), size);
            let tol = 4.0 * cam.world_per_pixel(size.y);
            let hit = items
                .iter()
                .filter_map(|it| mesh::pick(&it.mesh, o, d, tol).map(|t| (t, it.id)))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, id)| id);
            match (hit, ctrl, alt) {
                (Some(id), true, _) => app.st.toggle_select(id),
                (Some(id), _, true) => app.st.selection.retain(|s| *s != id),
                (Some(id), _, _) => app.st.select_only(id),
                (None, false, false) => app.st.clear_selection(),
                _ => {}
            }
        }
    }

    // Drag with the left button: rubber band or transform tool.
    if resp.drag_started_by(PointerButton::Primary) {
        let from = resp.interact_pointer_pos().unwrap_or(rect.center());
        if app.tool == Tool::Select || app.st.selection.is_empty() {
            app.vp.drag = Some(Drag::Band { tile: i, from });
        } else {
            app.st.checkpoint();
            let mut targets = Vec::new();
            for obj in app.st.selection.clone() {
                match app.st.ensure_transform(obj) {
                    Ok(t) => {
                        let p = &app.st.graph.nodes[&t].params;
                        let tr = p.vec3("translate").unwrap_or_default();
                        let ro = p.vec3("rotate").unwrap_or_default();
                        let sc = p.float("scale").unwrap_or(1.0);
                        targets.push((t, tr, ro, sc));
                    }
                    Err(e) => app.st.error_text(e),
                }
            }
            app.vp.drag = Some(Drag::Tool {
                tile: i,
                targets,
                total: egui::Vec2::ZERO,
            });
        }
    }
    if resp.dragged_by(PointerButton::Primary) {
        let delta = resp.drag_delta();
        let cam = app.vp.tiles[i].cam;
        let snap = app.snap.then_some(app.vp.grid_readout);
        let angle_snap = app.angle_snap;
        let tool = app.tool;
        match &mut app.vp.drag {
            Some(Drag::Band { tile, from }) if *tile == i => {
                if let Some(p) = resp.interact_pointer_pos() {
                    let r = Rect::from_two_pos(*from, p);
                    painter.rect_filled(r, 0.0, theme::ACCENT.gamma_multiply(0.08));
                    painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, theme::ACCENT), StrokeKind::Inside);
                }
            }
            Some(Drag::Tool { tile, targets, total }) if *tile == i => {
                *total += delta;
                let w = cam.world_per_pixel(size.y);
                for (id, tr, ro, sc) in targets.iter() {
                    match tool {
                        Tool::Move => {
                            let mut v = *tr + (cam.right() * total.x as f64 - cam.up() * total.y as f64) * w;
                            if let Some(s) = snap {
                                v = (v / s).round() * s;
                            }
                            app.st.set_param_silent(*id, "translate", ParamValue::Vec3(v));
                        }
                        Tool::Rotate => {
                            let mut a = ro.z + total.x as f64 * 0.5;
                            if angle_snap {
                                a = (a / 5.0).round() * 5.0;
                            }
                            app.st.set_param_silent(*id, "rotate", ParamValue::Vec3(DVec3::new(ro.x, ro.y, a)));
                        }
                        Tool::Scale => {
                            let f = (-(total.y as f64) * 0.01).exp();
                            app.st.set_param_silent(*id, "scale", ParamValue::Float((sc * f).clamp(0.001, 1000.0)));
                        }
                        Tool::Select => {}
                    }
                }
            }
            _ => {}
        }
    }
    if resp.drag_stopped_by(PointerButton::Primary) {
        if let Some(Drag::Band { tile, from }) = app.vp.drag.take() {
            if tile == i {
                if let Some(p) = resp.interact_pointer_pos() {
                    let r = Rect::from_two_pos(from, p);
                    let cam = app.vp.tiles[i].cam;
                    let inside: Vec<NodeId> = items
                        .iter()
                        .filter(|it| {
                            let c: Vec3 = it.mesh.center();
                            cam.project(c.as_dvec3(), size)
                                .is_some_and(|(s, _)| r.contains(to_screen(rect, s)))
                        })
                        .map(|it| it.id)
                        .collect();
                    if !ctrl {
                        app.st.clear_selection();
                    }
                    for id in inside {
                        if !app.st.selection.contains(&id) {
                            app.st.selection.push(id);
                        }
                    }
                }
            }
        } else {
            app.vp.drag = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::build_registry;

    /// Spec golden 12: scene undo (Ctrl+Z) and viewport undo (Shift+Z) are
    /// separate histories.
    #[test]
    fn golden_undo_split() {
        let mut st = AppState::new(build_registry());
        let b = st.create("geo.box").unwrap();
        let t = st.ensure_transform(b).unwrap();
        let moved = |st: &AppState| st.graph.nodes[&t].params.vec3("translate").unwrap();
        let mut vp = Viewports::default();
        let cam0 = vp.tiles[3].cam;

        // Move the object (scene history), then orbit the perspective tile.
        let offset = DVec3::new(10.0, 0.0, 0.0);
        st.set_param(t, "translate", ParamValue::Vec3(offset), 100.0);
        vp.tiles[3].remember();
        vp.tiles[3].cam.orbit_turntable(DVec2::new(40.0, 10.0), DVec3::ZERO);
        let cam1 = vp.tiles[3].cam;
        assert_ne!(cam0, cam1);

        // Ctrl+Z reverts the move and leaves the camera.
        assert!(st.undo());
        assert_eq!(moved(&st), DVec3::ZERO);
        assert_eq!(vp.tiles[3].cam, cam1);

        // Shift+Z reverts the orbit and leaves the object.
        assert!(st.redo());
        vp.view_undo(3);
        assert_eq!(vp.tiles[3].cam, cam0);
        assert_eq!(moved(&st), offset);
        vp.view_redo(3);
        assert_eq!(vp.tiles[3].cam, cam1);
    }
}

//! Quad viewports: tiles, splitters, label menus, view cube, home
//! grid, navigation (MMB pan, Alt+MMB orbit, wheel zoom) and the
//! Select/Move/Rotate/Scale tools.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CursorIcon, FontId, Painter, PointerButton, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, Ui, UiBuilder,
};
use glam::{DQuat, DVec2, DVec3, IVec3, Vec3};
use vernon_core::{Aabb, Geometry};
use vernon_graph::{NodeId, ParamValue};

use super::icons::{self, Icon};
use super::{Tool, VernonApp};
use crate::state::AppState;
use crate::theme;
use crate::viewport::camera::{Camera, OrbitMode, ViewKind};
use crate::viewport::grid::{self, GridSettings};
use crate::viewport::layout::QuadLayout;
use crate::viewport::gizmo;
use crate::viewport::mesh::{self, ColorBy, RenderMesh};
use crate::viewport::viewcube::{self, CompassHit};
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
    anim: Option<ViewAnim>,
}

/// An animated view change: `cam` is already the destination; only what is
/// drawn runs from `from` to it.
#[derive(Debug, Clone, Copy)]
struct ViewAnim {
    from: Camera,
    /// Set by the first frame that shows it.
    start: Option<f64>,
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
            anim: None,
        }
    }

    /// The camera to draw at time `now`.
    fn shown_cam(&self, now: f64) -> Camera {
        match self.anim {
            Some(ViewAnim { from, start }) => {
                let t = start.map_or(0.0, |s| (now - s) / viewcube::TRANSITION_S);
                from.lerp(&self.cam, viewcube::ease(t))
            }
            None => self.cam,
        }
    }

    /// Advance the animation; true while it still runs.
    fn tick(&mut self, now: f64) -> bool {
        let Some(a) = &mut self.anim else {
            return false;
        };
        let start = *a.start.get_or_insert(now);
        if now - start >= viewcube::TRANSITION_S {
            self.anim = None;
            return false;
        }
        true
    }

    fn remember(&mut self) {
        self.anim = None;
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
    Tool { tile: usize, targets: Vec<(NodeId, DVec3, DVec3, f64)>, total: egui::Vec2, handle: Option<(usize, DVec3)> },
    Splitter,
    /// Orbiting by dragging the view cube.
    Cube { tile: usize },
    /// Turning the view by dragging the compass ring.
    Compass { tile: usize },
    /// Zoom region rectangle (Ctrl+W).
    Zoom { tile: usize, from: Pos2 },
}

pub struct Viewports {
    pub tiles: [Tile; 4],
    pub layout: QuadLayout,
    pub grid: GridSettings,
    pub show_grid: bool,
    pub show_gizmo: bool,
    /// View cube size: 0 Large, 1 Normal, 2 Small, 3 Tiny.
    pub cube_size: u8,
    pub show_compass: bool,
    /// Compass north angle in degrees; 0 points north along the cube's front.
    pub north_deg: f64,
    /// Run view changes as a short transition instead of jumping.
    pub animate_views: bool,
    /// The stored front view: rotation from cube axes to the world.
    pub front: DQuat,
    /// The next left drag in a viewport zooms to its rectangle.
    pub zoom_region: bool,
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
            cube_size: 1,
            show_compass: true,
            north_deg: 0.0,
            animate_views: true,
            front: DQuat::IDENTITY,
            zoom_region: false,
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

    /// Zoom a tile about its centre by wheel `notches` (`[` and `]`).
    pub fn zoom_step(&mut self, tile: usize, notches: f64) {
        let t = &mut self.tiles[tile];
        t.remember();
        let size = t.size();
        t.cam.zoom_at(size * 0.5, size, notches);
    }

    pub fn set_view(&mut self, tile: usize, kind: ViewKind) {
        let front = self.front;
        let t = &mut self.tiles[tile];
        let from = t.cam;
        t.remember();
        t.kind = kind;
        t.cam.set_view(kind);
        if kind != ViewKind::User {
            t.cam.rot = (front * kind.rotation()).normalize();
        }
        self.transition(tile, from);
    }

    /// Show the change of a tile's camera from `from` as a transition.
    fn transition(&mut self, tile: usize, from: Camera) {
        let t = &mut self.tiles[tile];
        t.anim = (self.animate_views && from != t.cam).then_some(ViewAnim { from, start: None });
    }

    /// Store a tile's current view as the cube's Front (spec 2.4). The tile's
    /// camera stays where it is and now reads as Front.
    pub fn set_front(&mut self, tile: usize) {
        let t = &mut self.tiles[tile];
        self.front = viewcube::front_frame(t.cam.rot);
        if t.cam.ortho {
            t.kind = ViewKind::Front;
        }
    }

    /// Back to the standard front view; cameras stay where they are.
    pub fn reset_front(&mut self) {
        self.front = DQuat::IDENTITY;
    }

    /// A camera's rotation relative to the cube's axes.
    fn cube_rot(&self, cam: &Camera) -> DQuat {
        (self.front.inverse() * cam.rot).normalize()
    }

    /// Compass letter: look from that side, keeping the elevation.
    pub fn look_from(&mut self, tile: usize, letter: usize) {
        let dir = viewcube::compass_dirs(self.north_deg)[letter % 4];
        let rel = self.cube_rot(&self.tiles[tile].cam);
        let front = self.front;
        let t = &mut self.tiles[tile];
        let from = t.cam;
        t.remember();
        let rel = viewcube::look_from_heading(rel, dir);
        t.cam.rot = (front * rel).normalize();
        if t.kind != ViewKind::Perspective {
            t.kind = viewcube::facing(rel).and_then(viewcube::face_view).unwrap_or(ViewKind::User);
        }
        self.transition(tile, from);
    }

    /// Look at the target from view-cube region `s`: a face gives its named
    /// orthographic view, an edge or corner an angled view (perspective
    /// tiles stay perspective).
    pub fn orient(&mut self, tile: usize, s: IVec3) {
        self.tiles[tile].remember();
        self.orient_snapped(tile, s);
    }

    /// [`orient`](Self::orient) without a view-history entry (the cube drag
    /// already made one).
    fn orient_snapped(&mut self, tile: usize, s: IVec3) {
        let front = self.front;
        let t = &mut self.tiles[tile];
        let from = t.cam;
        match viewcube::face_view(s) {
            Some(kind) => {
                t.kind = kind;
                t.cam.set_view(kind);
            }
            None => {
                if t.kind != ViewKind::Perspective {
                    t.kind = ViewKind::User;
                    t.cam.set_view(ViewKind::User);
                }
            }
        }
        t.cam.rot = (front * viewcube::rotation_for(s)).normalize();
        self.transition(tile, from);
    }

    /// Home icon: the tile's default view, framed.
    fn home(&mut self, tile: usize, st: &AppState) {
        let front = self.front;
        let t = &mut self.tiles[tile];
        let from = t.cam;
        t.remember();
        t.kind = DEFAULT_KINDS[tile];
        t.cam.set_view(t.kind);
        t.cam.rot = (front * t.kind.rotation()).normalize();
        self.zoom_extents(st, Some(tile), false);
        self.transition(tile, from);
    }

    /// Roll arrow: turn the view 90 degrees about the view axis.
    pub fn roll(&mut self, tile: usize, clockwise: bool) {
        let t = &mut self.tiles[tile];
        let from = t.cam;
        t.remember();
        t.cam.rot = viewcube::roll(t.cam.rot, clockwise);
        if t.kind != ViewKind::Perspective {
            t.kind = ViewKind::User;
        }
        self.transition(tile, from);
    }

    pub fn view_undo(&mut self, tile: usize) {
        let t = &mut self.tiles[tile];
        if let Some(c) = t.history.pop() {
            t.anim = None;
            t.future.push(t.cam);
            t.cam = c;
        }
    }

    pub fn view_redo(&mut self, tile: usize) {
        let t = &mut self.tiles[tile];
        if let Some(c) = t.future.pop() {
            t.anim = None;
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

fn to_screen(rect: Rect, s: DVec2) -> Pos2 {
    rect.min + vec2(s.x as f32, s.y as f32)
}

/// What the pointer is over on the view cube.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CubeHit {
    /// One of the 26 regions (face, edge or corner).
    Region(IVec3),
    Home,
    /// Roll arrow; true = clockwise.
    Roll(bool),
    /// Face-view triangle towards the neighbouring face (0 up, 1 right,
    /// 2 down, 3 left).
    Neighbour(usize),
    Compass(CompassHit),
}

/// Settings of the view cube as drawn in a tile.
#[derive(Debug, Clone, Copy)]
struct CubeLook {
    half: f32,
    alpha: f32,
    /// Camera rotation relative to the cube's axes.
    rot: DQuat,
    /// North angle when the compass is shown.
    compass: Option<f64>,
}

fn cube_centre(rect: Rect, half: f32) -> Pos2 {
    pos2(rect.right() - half * 2.2 - 16.0, rect.top() + half * 2.2 + 14.0)
}

/// Area of the cube that starts an orbit drag.
fn cube_area(rect: Rect, half: f32) -> Rect {
    Rect::from_center_size(cube_centre(rect, half), vec2(half * 2.6, half * 2.6))
}

/// View cube (spec 2.4): the world-aligned cube seen from the tile camera
/// with face labels, the home mark, and in a face view the roll arrows and
/// the triangles to the neighbouring faces. Returns what `at` is over; the
/// hovered part is highlighted. `alpha` dims the cube in inactive tiles.
fn view_cube(painter: &Painter, rect: Rect, look: CubeLook, at: Option<Pos2>) -> Option<CubeHit> {
    let CubeLook { half, alpha, rot, compass } = look;
    let c = cube_centre(rect, half);
    let inv = rot.inverse();
    let screen = |w: DVec3| {
        let v = inv * w;
        c + vec2(v.x as f32, -v.y as f32) * half
    };
    let fade = |col: Color32| col.gamma_multiply(alpha);
    let face_view = viewcube::facing(rot);

    // What the pointer is over: home, roll arrows, triangles, then the cube.
    let home = pos2(c.x - half * 1.75, c.y - half * 1.75);
    let rolls = [(false, c + vec2(half * 0.85, -half * 1.85)), (true, c + vec2(half * 1.85, -half * 0.85))];
    let tri_dirs = [vec2(0.0, -1.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(-1.0, 0.0)];
    let tri_at = |k: usize| c + tri_dirs[k] * half * 1.5;
    let hit = at.and_then(|p| {
        if p.distance(home) < 8.0 {
            return Some(CubeHit::Home);
        }
        if face_view.is_some() {
            if let Some((cw, _)) = rolls.iter().find(|(_, q)| p.distance(*q) < 8.0) {
                return Some(CubeHit::Roll(*cw));
            }
            if let Some(k) = (0..4).find(|k| p.distance(tri_at(*k)) < 7.0) {
                return Some(CubeHit::Neighbour(k));
            }
        }
        let d = p - c;
        let d = DVec2::new(d.x as f64, d.y as f64);
        viewcube::pick(d, half as f64, rot).map(CubeHit::Region).or_else(|| {
            compass.and_then(|n| viewcube::compass_hit(d, half as f64, rot, n)).map(CubeHit::Compass)
        })
    });

    // Compass ring under the cube, letters on its outside.
    if let Some(north) = compass {
        let hot = matches!(hit, Some(CubeHit::Compass(CompassHit::Ring)));
        let st = Stroke::new(if hot { 2.0_f32 } else { 1.2 }, fade(if hot { theme::ACCENT } else { theme::TEXT_DIM }));
        let ring: Vec<Pos2> = (0..=48)
            .map(|k| screen(viewcube::compass_point(k as f64 / 48.0 * std::f64::consts::TAU)))
            .collect();
        painter.add(egui::Shape::line(ring, st));
        for (k, (d, label)) in viewcube::compass_dirs(north).into_iter().zip(["N", "E", "S", "W"]).enumerate() {
            let hot = hit == Some(CubeHit::Compass(CompassHit::Letter(k)));
            let col = if hot || k == 0 { theme::ACCENT } else { theme::TEXT_DIM };
            let font = FontId::proportional((half * 0.42).max(8.0));
            painter.text(screen(viewcube::compass_letter(d)), Align2::CENTER_CENTER, label, font, fade(col));
        }
    }

    // Faces, far first.
    let mut faces: Vec<(f64, IVec3)> = [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z, IVec3::NEG_Z]
        .into_iter()
        .map(|n| ((inv * n.as_dvec3()).z, n))
        .filter(|(d, _)| *d > 1e-6)
        .collect();
    faces.sort_by(|a, b| a.0.total_cmp(&b.0));
    let base = theme::mix(theme::SURFACE, theme::TEXT, 0.18);
    let edge = Stroke::new(1.0_f32, fade(theme::TEXT_DIM));
    for (_, n) in &faces {
        let nv = n.as_dvec3();
        let (u, v) = (DVec3::new(nv.y.abs(), nv.z.abs(), nv.x.abs()), DVec3::new(nv.z.abs(), nv.x.abs(), nv.y.abs()));
        let pts: Vec<Pos2> = [(1.0, 1.0), (1.0, -1.0), (-1.0, -1.0), (-1.0, 1.0)]
            .into_iter()
            .map(|(a, b)| screen(nv + u * a + v * b))
            .collect();
        let hot = hit == Some(CubeHit::Region(*n));
        let fill = if hot { theme::mix(base, theme::ACCENT, 0.55) } else { base };
        painter.add(egui::Shape::convex_polygon(pts, fade(fill), edge));
        let font = FontId::proportional((half * 0.36).max(7.0));
        painter.text(screen(nv), Align2::CENTER_CENTER, viewcube::face_label(*n), font, fade(theme::TEXT));
    }
    // Edge or corner under the pointer.
    if let Some(CubeHit::Region(s)) = hit {
        if viewcube::order(s) > 1 {
            painter.circle_filled(screen(s.as_dvec3() * 0.92), half * 0.17, fade(theme::ACCENT));
        }
    }
    // World axes from the cube's lower corner.
    let o = DVec3::new(-1.0, -1.0, -1.0) * 1.08;
    for (a, col) in [(DVec3::X, theme::AXIS_X), (DVec3::Y, theme::AXIS_Y), (DVec3::Z, theme::AXIS_Z)] {
        painter.line_segment([screen(o), screen(o + a * 0.9)], Stroke::new(1.5_f32, fade(col)));
    }
    // Home mark.
    let hot = hit == Some(CubeHit::Home);
    let st = Stroke::new(1.3_f32, fade(if hot { theme::ACCENT } else { theme::TEXT_DIM }));
    painter.line_segment([home + vec2(-6.0, 0.0), home + vec2(0.0, -6.0)], st);
    painter.line_segment([home + vec2(0.0, -6.0), home + vec2(6.0, 0.0)], st);
    painter.rect_stroke(Rect::from_min_max(home + vec2(-4.0, 0.0), home + vec2(4.0, 6.0)), 0.0, st, StrokeKind::Middle);
    // Face view: roll arrows and neighbour triangles.
    if face_view.is_some() {
        for (cw, q) in rolls {
            let hot = hit == Some(CubeHit::Roll(cw));
            let icon = if cw { Icon::RollRight } else { Icon::RollLeft };
            let col = fade(if hot { theme::ACCENT } else { theme::TEXT_DIM });
            icons::paint(painter, Rect::from_center_size(q, vec2(14.0, 14.0)), icon, col);
        }
        for (k, d) in tri_dirs.into_iter().enumerate() {
            let t = tri_at(k);
            let side = vec2(-d.y, d.x) * 5.0;
            let hot = hit == Some(CubeHit::Neighbour(k));
            let col = fade(if hot { theme::ACCENT } else { theme::TEXT_DIM });
            painter.add(egui::Shape::convex_polygon(vec![t + d * 4.0, t - d * 2.0 + side, t - d * 2.0 - side], col, Stroke::NONE));
        }
    }
    hit
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
            let now = ui.input(|inp| inp.time);
            for t in &mut app.vp.tiles {
                if t.tick(now) {
                    ui.ctx().request_repaint();
                }
            }
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
                ui.checkbox(&mut app.vp.show_gizmo, "Show View Cube");
                ui.checkbox(&mut app.vp.show_compass, "Show Compass");
                ui.checkbox(&mut app.vp.animate_views, "Animate View Changes");
                if ui.button("Set Current View as Front").clicked() {
                    app.vp.set_front(i);
                }
                if ui.button("Reset Front").clicked() {
                    app.vp.reset_front();
                }
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

/// Gizmo of the active tool, with its centre: the middle of the selected
/// unlocked objects. The select tool has none.
fn gizmo_of(app: &VernonApp, items: &[DrawItem]) -> Option<(gizmo::Kind, DVec3)> {
    let kind = match app.tool {
        Tool::Select => return None,
        Tool::Move => gizmo::Kind::Move,
        Tool::Rotate => gizmo::Kind::Rotate,
        Tool::Scale => gizmo::Kind::Scale,
    };
    let mut b = Aabb::empty();
    for it in items.iter().filter(|it| it.selected && !app.st.is_locked(it.id) && !it.mesh.is_empty()) {
        b.grow(it.mesh.min.as_dvec3());
        b.grow(it.mesh.max.as_dvec3());
    }
    (!b.is_empty()).then(|| (kind, b.center()))
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
    let now = ui.input(|inp| inp.time);
    let shown = app.vp.tiles[i].shown_cam(now);
    let grid_geo = app.vp.show_grid.then(|| {
        Arc::new(grid::build(&shown, app.vp.tiles[i].kind, size, &app.vp.grid))
    });
    if let (true, Some(g)) = (active, &grid_geo) {
        app.vp.grid_readout = g.spacing;
    }
    let view = ViewParams {
        camera: shown,
        shading: app.vp.tiles[i].shading,
        edged: app.vp.tiles[i].edged,
        grid: grid_geo,
        overlay: gizmo_of(app, items)
            .map(|(kind, c)| {
                let cam = &app.vp.tiles[i].cam;
                let hot = match &app.vp.drag {
                    Some(Drag::Tool { tile, handle, .. }) if *tile == i => handle.map(|h| h.0),
                    _ => hover.and_then(|h| {
                        gizmo::hit(kind, cam, c, size, DVec2::new((h.x - rect.min.x) as f64, (h.y - rect.min.y) as f64))
                    }),
                };
                let colors = [theme::AXIS_X, theme::AXIS_Y, theme::AXIS_Z];
                gizmo::segments(kind, cam, c, size.y, colors, hot, theme::ACCENT)
            })
            .unwrap_or_default(),
    };
    app.backend.paint(&painter, rect, &view, items);

    // Cursor readout on the construction plane.
    if let Some(h) = hover {
        let (n, _, _) = grid::plane_of(app.vp.tiles[i].kind, app.vp.tiles[i].cam.back());
        let px = DVec2::new((h.x - rect.min.x) as f64, (h.y - rect.min.y) as f64);
        app.vp.cursor_world = app.vp.tiles[i].cam.plane_point(px, size, n, 0.0);
    }

    // View cube (handles its own clicks and drags before picking).
    let click_pos = if resp.clicked_by(PointerButton::Primary) {
        resp.interact_pointer_pos()
    } else {
        None
    };
    let mut gizmo_hit = false;
    if app.vp.show_gizmo {
        let half = viewcube::size_px(app.vp.cube_size);
        let alpha = if active { 1.0 } else { viewcube::INACTIVE_OPACITY };
        let cam = app.vp.tiles[i].cam;
        let rot = app.vp.cube_rot(&shown);
        let compass = app.vp.show_compass.then_some(app.vp.north_deg);
        let look = CubeLook { half, alpha, rot, compass };
        let under = view_cube(&painter, rect, look, click_pos.or(hover));
        if let (Some(_), Some(h)) = (click_pos, under) {
            gizmo_hit = true;
            match h {
                CubeHit::Home => app.vp.home(i, &app.st),
                CubeHit::Region(s) => app.vp.orient(i, s),
                CubeHit::Roll(cw) => app.vp.roll(i, cw),
                CubeHit::Neighbour(k) => {
                    let rel = app.vp.cube_rot(&cam);
                    app.vp.orient(i, viewcube::neighbour(rel, k));
                }
                CubeHit::Compass(CompassHit::Letter(k)) => app.vp.look_from(i, k),
                CubeHit::Compass(CompassHit::Ring) => {}
            }
        }
        // Dragging the cube orbits; letting go near a region snaps to it.
        // Dragging the compass ring only turns the view.
        if resp.drag_started_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                let d = p - cube_centre(rect, half);
                let d = DVec2::new(d.x as f64, d.y as f64);
                let on_ring = compass.is_some_and(|n| viewcube::compass_hit(d, half as f64, rot, n).is_some());
                if cube_area(rect, half).contains(p) {
                    app.vp.tiles[i].remember();
                    app.vp.drag = Some(Drag::Cube { tile: i });
                } else if on_ring {
                    app.vp.tiles[i].remember();
                    app.vp.drag = Some(Drag::Compass { tile: i });
                }
            }
        }
        if matches!(app.vp.drag, Some(Drag::Compass { tile }) if tile == i) {
            gizmo_hit = true;
            if resp.dragged_by(PointerButton::Primary) {
                let d = resp.drag_delta();
                let t = &mut app.vp.tiles[i];
                let pivot = t.cam.target;
                t.cam.orbit_turntable(DVec2::new(d.x as f64, 0.0), pivot);
                if t.kind != ViewKind::Perspective {
                    t.kind = ViewKind::User;
                }
            }
            if resp.drag_stopped_by(PointerButton::Primary) {
                app.vp.drag = None;
            }
        }
        if matches!(app.vp.drag, Some(Drag::Cube { tile }) if tile == i) {
            gizmo_hit = true;
            if resp.dragged_by(PointerButton::Primary) {
                let d = resp.drag_delta();
                let t = &mut app.vp.tiles[i];
                let pivot = t.cam.target;
                t.cam.orbit_turntable(DVec2::new(d.x as f64, d.y as f64), pivot);
                if t.kind != ViewKind::Perspective {
                    t.kind = ViewKind::User;
                }
            }
            if resp.drag_stopped_by(PointerButton::Primary) {
                app.vp.drag = None;
                let rel = app.vp.cube_rot(&app.vp.tiles[i].cam);
                if let Some(s) = viewcube::snap(rel * DVec3::Z, viewcube::SNAP_DEG) {
                    app.vp.orient_snapped(i, s);
                }
            }
        }
    }

    navigate(app, ui, &resp, i, rect);
    tools(app, ui, &resp, i, rect, items, gizmo_hit, &painter, &view);

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
        if ui.button("Hide Unselected   Alt+I").clicked() {
            app.st.hide_unselected();
        }
        if ui.button("Unhide All   Alt+U").clicked() {
            app.unhide_all();
        }
        let iso = if app.isolated.is_empty() { "Isolate Selection   Alt+Q" } else { "End Isolate   Alt+Q" };
        if ui.button(iso).clicked() {
            app.toggle_isolate();
        }
        if ui.button("Zoom Extents   Z").clicked() {
            app.vp.zoom_extents(&app.st, Some(i), true);
        }
        if ui.button("Zoom Region   Ctrl+W").clicked() {
            app.vp.zoom_region = true;
        }
        if ui.button("Set Current View as Front").clicked() {
            app.vp.set_front(i);
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
            app.vp.tiles[i].anim = None;
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
    view: &ViewParams,
) {
    let size = app.vp.tiles[i].size();
    let (ctrl, alt) = ui.input(|inp| (inp.modifiers.ctrl || inp.modifiers.command, inp.modifiers.alt));
    let to_px = |p: Pos2| DVec2::new((p.x - rect.min.x) as f64, (p.y - rect.min.y) as f64);

    if gizmo_hit || matches!(app.vp.drag, Some(Drag::Cube { .. } | Drag::Compass { .. })) {
        return;
    }
    // Zoom region (Ctrl+W): the next left drag frames its rectangle.
    if app.vp.zoom_region {
        if resp.drag_started_by(PointerButton::Primary) {
            let from = resp.interact_pointer_pos().unwrap_or(rect.center());
            app.vp.drag = Some(Drag::Zoom { tile: i, from });
        }
        if let (Some(Drag::Zoom { tile, from }), Some(p)) = (&app.vp.drag, resp.interact_pointer_pos()) {
            if *tile == i {
                let r = Rect::from_two_pos(*from, p);
                painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, theme::ACCENT), StrokeKind::Inside);
            }
        }
        if resp.drag_stopped_by(PointerButton::Primary) {
            if let (Some(Drag::Zoom { tile, from }), Some(p)) = (app.vp.drag.take(), resp.interact_pointer_pos()) {
                if tile == i {
                    let t = &mut app.vp.tiles[i];
                    t.remember();
                    t.cam.zoom_region(to_px(from), to_px(p), size);
                }
            }
            app.vp.zoom_region = false;
        }
        ui.ctx().set_cursor_icon(CursorIcon::ZoomIn);
        return;
    }
    // Click: pick (locked objects cannot be picked; selection lock keeps
    // the selection).
    if resp.clicked_by(PointerButton::Primary) && !app.selection_locked {
        if let Some(p) = resp.interact_pointer_pos() {
            let cam = app.vp.tiles[i].cam;
            let pickable: Vec<DrawItem> = items.iter().filter(|it| !app.st.is_locked(it.id)).cloned().collect();
            let ppp = ui.ctx().pixels_per_point();
            let hit = app.backend.pick(rect, view, &pickable, p, 4.0, ppp).unwrap_or_else(|| {
                let (o, d) = cam.ray(to_px(p), size);
                let tol = 4.0 * cam.world_per_pixel(size.y);
                pickable
                    .iter()
                    .filter_map(|it| mesh::pick(&it.mesh, o, d, tol).map(|t| (t, it.id)))
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, id)| id)
            });
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
            if !app.selection_locked {
                app.vp.drag = Some(Drag::Band { tile: i, from });
            }
        } else {
            // On a gizmo handle the drag is constrained to its axis.
            let handle = gizmo_of(app, items).and_then(|(kind, c)| {
                gizmo::hit(kind, &app.vp.tiles[i].cam, c, size, to_px(from)).map(|axis| (axis, c))
            });
            // One undo entry for the drag, however many transforms it adds.
            let objects = app.st.selection.clone();
            let (targets, errors) = app.st.as_one_undo_step(|st| {
                let mut targets = Vec::new();
                let mut errors = Vec::new();
                for obj in objects {
                    if st.is_locked(obj) {
                        continue;
                    }
                    match st.ensure_transform(obj) {
                        Ok(t) => {
                            let p = &st.graph.nodes[&t].params;
                            let tr = p.vec3("translate").unwrap_or_default();
                            let ro = p.vec3("rotate").unwrap_or_default();
                            let sc = p.float("scale").unwrap_or(1.0);
                            targets.push((t, tr, ro, sc));
                        }
                        Err(e) => errors.push(e),
                    }
                }
                (targets, errors)
            });
            for e in errors {
                app.st.error_text(e);
            }
            app.vp.drag = Some(Drag::Tool {
                tile: i,
                targets,
                total: egui::Vec2::ZERO,
                handle,
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
            Some(Drag::Tool { tile, targets, total, handle }) if *tile == i => {
                *total += delta;
                let w = cam.world_per_pixel(size.y);
                let drag = DVec2::new(total.x as f64, total.y as f64);
                for (id, tr, ro, sc) in targets.iter() {
                    match tool {
                        Tool::Move => {
                            let mut v = match handle {
                                Some((axis, c)) => {
                                    let mut d = DVec3::ZERO;
                                    d[*axis] = gizmo::axis_drag(&cam, *c, *axis, drag, size);
                                    *tr + d
                                }
                                None => *tr + (cam.right() * total.x as f64 - cam.up() * total.y as f64) * w,
                            };
                            if let Some(s) = snap {
                                v = (v / s).round() * s;
                            }
                            app.st.set_param_silent(*id, "translate", ParamValue::Vec3(v));
                        }
                        Tool::Rotate => {
                            let axis = handle.map_or(2, |h| h.0);
                            let mut a = ro[axis] + total.x as f64 * 0.5;
                            if angle_snap {
                                a = (a / 5.0).round() * 5.0;
                            }
                            let mut v = *ro;
                            v[axis] = a;
                            app.st.set_param_silent(*id, "rotate", ParamValue::Vec3(v));
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
                        .filter(|it| !app.st.is_locked(it.id))
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

    #[test]
    fn view_changes_animate_from_the_old_camera_to_the_new() {
        let mut vp = Viewports::default();
        let before = vp.tiles[0].cam;
        vp.orient(0, IVec3::X);
        let after = vp.tiles[0].cam;
        assert_eq!(after.rot, ViewKind::Front.rotation(), "the state changes at once");
        // Before the first frame, the old view is still what is drawn.
        assert_eq!(vp.tiles[0].shown_cam(5.0), before);
        assert!(vp.tiles[0].tick(5.0));
        assert_eq!(vp.tiles[0].shown_cam(5.0), before);
        // Half way in time is half way in angle (the easing is symmetric).
        let mid = vp.tiles[0].shown_cam(5.0 + viewcube::TRANSITION_S / 2.0);
        let total = before.rot.angle_between(after.rot);
        assert!((mid.rot.angle_between(before.rot) - total / 2.0).abs() < 1e-9);
        // Then it ends on exactly the new view.
        assert!(!vp.tiles[0].tick(5.0 + viewcube::TRANSITION_S + 0.01));
        assert_eq!(vp.tiles[0].shown_cam(9.0), after);
        // Off: the view just jumps.
        vp.animate_views = false;
        vp.orient(0, IVec3::Y);
        assert!(vp.tiles[0].anim.is_none());
        // Navigating cancels a running transition.
        vp.animate_views = true;
        vp.orient(0, IVec3::Z);
        assert!(vp.tiles[0].anim.is_some());
        vp.zoom_step(0, 1.0);
        assert!(vp.tiles[0].anim.is_none());
    }

    #[test]
    fn current_view_can_become_the_front_of_the_cube() {
        let mut vp = Viewports { animate_views: false, ..Default::default() };
        vp.orient(1, IVec3::Y);
        let stored = vp.tiles[1].cam;
        vp.set_front(1);
        assert_eq!(vp.tiles[1].cam, stored, "storing does not move the camera");
        assert_eq!(vp.tiles[1].kind, ViewKind::Front);
        assert_eq!(viewcube::facing(vp.cube_rot(&stored)), Some(IVec3::X));
        // The Front region and the Front command now give the stored view.
        vp.orient(0, IVec3::X);
        assert!(vp.tiles[0].cam.rot.dot(stored.rot).abs() > 1.0 - 1e-9);
        vp.set_view(2, ViewKind::Front);
        assert!(vp.tiles[2].cam.rot.dot(stored.rot).abs() > 1.0 - 1e-9);
        // The neighbours follow: the cube's Left is 90 degrees round from it.
        vp.orient(0, IVec3::Y);
        assert!((vp.tiles[0].cam.back() + DVec3::X).length() < 1e-9);
        // Top stays on top.
        vp.orient(0, IVec3::Z);
        assert!((vp.tiles[0].cam.back() - DVec3::Z).length() < 1e-9);
        // The grid still lies in the plane the rotated side view faces.
        let g = grid::build(&vp.tiles[2].cam, vp.tiles[2].kind, DVec2::new(400.0, 300.0), &vp.grid);
        assert!(g.segments.iter().all(|s| s.a.y == 0.0 && s.b.y == 0.0));
        // Reset restores the standard views, leaving cameras alone.
        let cam = vp.tiles[2].cam;
        vp.reset_front();
        assert_eq!(vp.tiles[2].cam, cam);
        vp.orient(0, IVec3::X);
        assert!((vp.tiles[0].cam.back() - DVec3::X).length() < 1e-9);
    }

    #[test]
    fn compass_letters_turn_the_view_and_keep_elevation() {
        let mut vp = Viewports { animate_views: false, ..Default::default() };
        let elev = vp.tiles[3].cam.back().z;
        vp.look_from(3, 3); // West: +Y at a north angle of 0.
        let b = vp.tiles[3].cam.back();
        assert!((b.z - elev).abs() < 1e-9);
        assert!(b.y > 0.0 && b.x.abs() < 1e-9);
        assert_eq!(vp.tiles[3].kind, ViewKind::Perspective);
        // North angle 90 moves north onto +Y.
        vp.north_deg = 90.0;
        vp.look_from(1, 0);
        assert!((vp.tiles[1].cam.back() - DVec3::Y).length() < 1e-9);
        assert_eq!(vp.tiles[1].kind, ViewKind::Left, "still a face view");
    }

    #[test]
    fn view_cube_orients_rolls_and_keeps_history() {
        let mut vp = Viewports::default();
        // A face gives the named view; an edge in an orthographic tile a
        // user view; a corner in the perspective tile stays perspective.
        vp.orient(0, -IVec3::Y);
        assert_eq!(vp.tiles[0].kind, ViewKind::Right);
        vp.orient(0, IVec3::new(1, 0, 1));
        assert_eq!(vp.tiles[0].kind, ViewKind::User);
        assert!((vp.tiles[0].cam.back() - DVec3::new(1.0, 0.0, 1.0).normalize()).length() < 1e-12);
        vp.orient(3, IVec3::ONE);
        assert_eq!(vp.tiles[3].kind, ViewKind::Perspective);
        assert!(!vp.tiles[3].cam.ortho);
        // Four clockwise rolls of a face view come back to it.
        vp.orient(1, IVec3::NEG_Y);
        let front = vp.tiles[1].cam.rot;
        for _ in 0..4 {
            vp.roll(1, true);
        }
        assert!(vp.tiles[1].cam.rot.dot(front).abs() > 1.0 - 1e-9);
        // Each step is one view-undo entry.
        vp.view_undo(1);
        vp.view_undo(1);
        vp.view_undo(1);
        vp.view_undo(1);
        assert!(vp.tiles[1].cam.rot.dot(front).abs() > 1.0 - 1e-9);
    }
}

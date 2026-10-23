//! Node editor window: the same graph the stacks project. Drag nodes, drag
//! from an output port to an input port to wire, right-click for actions.
//! Failed nodes have a red border; white-box nodes a dashed one.

use std::collections::HashMap;

use eframe::egui::{
    self, epaint::CubicBezierShape, pos2, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke,
    StrokeKind,
};
use vernon_graph::{DataKind, Graph, NodeId};

use super::{PanelTab, VernonApp};
use crate::theme;

const NODE_W: f32 = 168.0;
const HEADER: f32 = 34.0;
const ROW: f32 = 16.0;

#[derive(Default)]
pub struct EditorState {
    pan: egui::Vec2,
    wire: Option<(NodeId, usize)>,
}

fn kind_color(k: DataKind) -> Color32 {
    match k {
        DataKind::Geometry => Color32::from_rgb(0x8F, 0xB8, 0xDE),
        DataKind::Report => theme::ACCENT,
        DataKind::File => Color32::from_rgb(0x9C, 0xCC, 0x7A),
        DataKind::Json => Color32::from_rgb(0xD9, 0x86, 0xA8),
        DataKind::Any => theme::TEXT_DIM,
    }
}

/// Longest-path layering: rank = 1 + max rank of inputs; rows by id.
pub fn layout_positions(graph: &Graph) -> HashMap<NodeId, [f32; 2]> {
    fn rank(g: &Graph, id: NodeId, memo: &mut HashMap<NodeId, usize>, depth: usize) -> usize {
        if let Some(r) = memo.get(&id) {
            return *r;
        }
        let r = if depth > 512 {
            0
        } else {
            g.nodes
                .get(&id)
                .map(|n| {
                    n.inputs
                        .iter()
                        .flatten()
                        .map(|(s, _)| rank(g, *s, memo, depth + 1) + 1)
                        .max()
                        .unwrap_or(0)
                })
                .unwrap_or(0)
        };
        memo.insert(id, r);
        r
    }
    let mut memo = HashMap::new();
    let mut rows: HashMap<usize, usize> = HashMap::new();
    let mut out = HashMap::new();
    for id in graph.nodes.keys() {
        let r = rank(graph, *id, &mut memo, 0);
        let row = rows.entry(r).or_insert(0);
        out.insert(*id, [40.0 + *row as f32 * (NODE_W + 40.0), 30.0 + r as f32 * 110.0]);
        *row += 1;
    }
    out
}

fn node_height(inputs: usize, outputs: usize) -> f32 {
    HEADER + ROW * inputs.max(outputs).max(1) as f32 + 4.0
}

fn bezier(painter: &egui::Painter, a: Pos2, b: Pos2, col: Color32, width: f32) {
    let dx = ((b.x - a.x).abs() * 0.5).max(30.0);
    painter.add(CubicBezierShape::from_points_stroke(
        [a, a + vec2(dx, 0.0), b - vec2(dx, 0.0), b],
        false,
        Color32::TRANSPARENT,
        Stroke::new(width, col),
    ));
}

pub fn show(app: &mut VernonApp, ctx: &egui::Context) {
    let mut open = app.show.node_editor;
    egui::Window::new("Node Editor")
        .open(&mut open)
        .default_size([780.0, 440.0])
        .resizable(true)
        .collapsible(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Arrange").on_hover_text("Lay nodes out by dependency depth").clicked() {
                    app.st.checkpoint();
                    for (id, p) in layout_positions(&app.st.graph) {
                        app.st.move_node(id, p);
                    }
                    app.editor.pan = egui::Vec2::ZERO;
                }
                if ui.button("Home").on_hover_text("Reset the view offset").clicked() {
                    app.editor.pan = egui::Vec2::ZERO;
                }
                ui.label(
                    RichText::new("drag body: move · drag ● output → input: wire · right-click: actions · middle-drag: pan")
                        .small()
                        .color(theme::TEXT_DIM),
                );
            });
            canvas(app, ui);
        });
    app.show.node_editor = open;
}

fn canvas(app: &mut VernonApp, ui: &mut egui::Ui) {
    let (rect, bg) = ui.allocate_exact_size(ui.available_size().max(vec2(200.0, 120.0)), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(0x1F, 0x1F, 0x1F));
    // Dot grid.
    let step = 24.0;
    let off = vec2(app.editor.pan.x.rem_euclid(step), app.editor.pan.y.rem_euclid(step));
    let mut y = rect.top() + off.y;
    while y < rect.bottom() {
        let mut x = rect.left() + off.x;
        while x < rect.right() {
            painter.circle_filled(pos2(x, y), 0.8, Color32::from_rgb(0x38, 0x38, 0x38));
            x += step;
        }
        y += step;
    }
    if bg.dragged_by(egui::PointerButton::Middle) || bg.dragged_by(egui::PointerButton::Primary) {
        app.editor.pan += bg.drag_delta();
    }
    if bg.clicked() {
        app.st.clear_selection();
    }
    let origin = rect.min + app.editor.pan;

    // Layout pass.
    struct NodeBox {
        id: NodeId,
        rect: Rect,
        ins: Vec<(Pos2, DataKind, String)>,
        outs: Vec<(Pos2, DataKind, String)>,
    }
    let mut boxes: Vec<NodeBox> = Vec::new();
    for (id, n) in &app.st.graph.nodes {
        let Some(info) = app.st.registry.info(&n.type_id) else { continue };
        let h = node_height(info.inputs.len().max(n.inputs.len()), info.outputs.len());
        let r = Rect::from_min_size(origin + vec2(n.pos[0], n.pos[1]), vec2(NODE_W, h));
        let ins = (0..info.inputs.len().max(n.inputs.len()))
            .map(|i| {
                let p = info.inputs.get(i);
                (
                    pos2(r.left(), r.top() + HEADER + ROW * (i as f32 + 0.5)),
                    p.map(|p| p.kind).unwrap_or(DataKind::Any),
                    p.map(|p| format!("{}{}", p.name, if p.required { "" } else { " (opt)" }))
                        .unwrap_or_else(|| format!("in {i}")),
                )
            })
            .collect();
        let outs = info
            .outputs
            .iter()
            .enumerate()
            .map(|(i, p)| (pos2(r.right(), r.top() + HEADER + ROW * (i as f32 + 0.5)), p.kind, p.name.clone()))
            .collect();
        boxes.push(NodeBox { id: *id, rect: r, ins, outs });
    }
    let out_pos: HashMap<(NodeId, usize), (Pos2, DataKind)> = boxes
        .iter()
        .flat_map(|b| b.outs.iter().enumerate().map(move |(i, (p, k, _))| ((b.id, i), (*p, *k))))
        .collect();

    // Wires under the nodes.
    for b in &boxes {
        let Some(n) = app.st.graph.nodes.get(&b.id) else { continue };
        for (i, w) in n.inputs.iter().enumerate() {
            if let (Some((src, o)), Some((pin, _, _))) = (w, b.ins.get(i)) {
                if let Some((pout, k)) = out_pos.get(&(*src, *o)) {
                    bezier(&painter, *pout, *pin, kind_color(*k).gamma_multiply(0.85), 2.0);
                }
            }
        }
    }

    let now = app.now;
    let mut drop_target: Option<(NodeId, usize)> = None;
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let released = ui.input(|i| i.pointer.any_released());
    for b in &boxes {
        let id = b.id;
        let Some(node) = app.st.graph.nodes.get(&id).cloned() else { continue };
        let Some(info) = app.st.registry.info(&node.type_id).cloned() else { continue };
        let resp = ui.interact(b.rect, ui.id().with(("node", id)), Sense::click_and_drag());
        if resp.drag_started_by(egui::PointerButton::Primary) {
            app.st.checkpoint_coalesced(&format!("move:{id}"), now);
        }
        if resp.dragged_by(egui::PointerButton::Primary) {
            let d = resp.drag_delta();
            app.st.move_node(id, [node.pos[0] + d.x, node.pos[1] + d.y]);
        }
        if resp.clicked() {
            if ui.input(|i| i.modifiers.ctrl || i.modifiers.command) {
                app.st.toggle_select(id);
            } else {
                app.st.select_only(id);
            }
        }
        if resp.double_clicked() {
            let obj = app.st.object_of(id);
            app.st.select_only(obj);
            app.st.stack_entry = Some(id);
            app.tab = PanelTab::Modify;
        }
        let error = app.st.error_of(id).map(str::to_string);
        let selected = app.st.selection.contains(&id);
        let is_object = app.st.is_object(id);
        resp.context_menu(|ui| {
            if ui.button("Edit in Modify panel").clicked() {
                let obj = app.st.object_of(id);
                app.st.select_only(obj);
                app.st.stack_entry = Some(id);
                app.tab = PanelTab::Modify;
            }
            if ui.button(if node.bypass { "Turn on" } else { "Bypass" }).clicked() {
                app.st.toggle_bypass(id);
            }
            if ui.button("Delete").clicked() {
                app.st.delete(&[id]);
            }
        });
        let resp = match &error {
            Some(e) => resp.on_hover_text(RichText::new(e).color(theme::FAIL)),
            None => resp.on_hover_text(&info.help),
        };
        let _ = resp;

        // Body.
        let fill = if node.bypass {
            Color32::from_rgb(0x3A, 0x36, 0x24)
        } else if selected {
            Color32::from_rgb(0x4A, 0x4A, 0x4A)
        } else {
            Color32::from_rgb(0x33, 0x33, 0x33)
        };
        painter.rect_filled(b.rect, 4.0, fill);
        let header = Rect::from_min_size(b.rect.min, vec2(NODE_W, HEADER - 4.0));
        let c = app.st.color_of(id);
        painter.rect_filled(
            Rect::from_min_size(header.min, vec2(4.0, b.rect.height())),
            2.0,
            Color32::from_rgb(c[0], c[1], c[2]),
        );
        painter.text(
            header.min + vec2(9.0, 3.0),
            Align2::LEFT_TOP,
            &info.label,
            FontId::proportional(13.0),
            theme::TEXT,
        );
        painter.text(
            header.min + vec2(9.0, 18.0),
            Align2::LEFT_TOP,
            format!("{}{}", node.name, if is_object { "  · object" } else { "" }),
            FontId::proportional(10.5),
            theme::TEXT_DIM,
        );
        if info.whitebox {
            let r = b.rect.expand(2.0);
            let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
            painter.extend(Shape::dashed_line(&pts, Stroke::new(1.2_f32, theme::WARN), 5.0, 3.0));
        }
        let border = if error.is_some() {
            Stroke::new(2.0_f32, theme::FAIL)
        } else if selected {
            Stroke::new(1.5_f32, Color32::WHITE)
        } else {
            Stroke::new(1.0_f32, Color32::from_rgb(0x55, 0x55, 0x55))
        };
        painter.rect_stroke(b.rect, 4.0, border, StrokeKind::Inside);
        if error.is_some() {
            painter.text(b.rect.right_top() + vec2(-6.0, 4.0), Align2::RIGHT_TOP, "✖", FontId::proportional(12.0), theme::FAIL);
        }

        // Ports.
        for (i, (p, k, name)) in b.ins.iter().enumerate() {
            let pr = ui.interact(
                Rect::from_center_size(*p, vec2(14.0, 14.0)),
                ui.id().with(("in", id, i)),
                Sense::click(),
            );
            let connected = node.inputs.get(i).is_some_and(|w| w.is_some());
            let hot = pr.hovered() || (pointer.is_some_and(|q| q.distance(*p) < 10.0) && app.editor.wire.is_some());
            painter.circle_filled(*p, if hot { 6.0 } else { 4.5 }, if connected { kind_color(*k) } else { Color32::from_rgb(0x22, 0x22, 0x22) });
            painter.circle_stroke(*p, 4.5, Stroke::new(1.0_f32, kind_color(*k)));
            painter.text(*p + vec2(8.0, 0.0), Align2::LEFT_CENTER, name, FontId::proportional(10.5), theme::TEXT_DIM);
            if pr.secondary_clicked() && connected {
                app.st.disconnect(id, i);
            }
            pr.on_hover_text(format!("{name}: {k:?} (right-click to disconnect)"));
            if released && app.editor.wire.is_some() && pointer.is_some_and(|q| q.distance(*p) < 10.0) {
                drop_target = Some((id, i));
            }
        }
        for (o, (p, k, name)) in b.outs.iter().enumerate() {
            let pr = ui.interact(
                Rect::from_center_size(*p, vec2(14.0, 14.0)),
                ui.id().with(("out", id, o)),
                Sense::drag(),
            );
            painter.circle_filled(*p, if pr.hovered() { 6.0 } else { 4.5 }, kind_color(*k));
            painter.text(*p - vec2(8.0, 0.0), Align2::RIGHT_CENTER, name, FontId::proportional(10.5), theme::TEXT_DIM);
            if pr.drag_started() {
                app.editor.wire = Some((id, o));
            }
            pr.on_hover_text(format!("{name}: {k:?} (drag to an input)"));
        }
    }

    // Wire being dragged.
    if let (Some((src, o)), Some(p)) = (app.editor.wire, pointer) {
        if let Some((from, k)) = out_pos.get(&(src, o)) {
            bezier(&painter, *from, p, kind_color(*k), 2.5);
        }
    }
    if released {
        if let (Some((src, o)), Some((dst, i))) = (app.editor.wire, drop_target) {
            if let Err(e) = app.st.connect(src, o, dst, i) {
                app.st.error_text(format!("cannot connect: {e}"));
            }
        }
        app.editor.wire = None;
    }
    if app.st.graph.nodes.is_empty() {
        painter.text(rect.center(), Align2::CENTER_CENTER, "No nodes yet", FontId::proportional(14.0), theme::TEXT_DIM);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::build_registry;

    #[test]
    fn golden_layering() {
        let reg = build_registry();
        let mut g = Graph::new();
        // Chain of 9: ranks 0..8.
        let mut prev = g.add_node(&reg, "geo.box").unwrap();
        let mut chain = vec![prev];
        for _ in 0..8 {
            let n = g.add_node(&reg, "clean.smooth_mesh").unwrap();
            g.connect(prev, 0, n, 0).unwrap();
            chain.push(n);
            prev = n;
        }
        let pos = layout_positions(&g);
        for (r, id) in chain.iter().enumerate() {
            assert_eq!(pos[id][1], 30.0 + r as f32 * 110.0);
        }
        // Diamond A->B, A->C, B->D, C->D: ranks 0, 1, 1, 2.
        let mut g = Graph::new();
        let a = g.add_node(&reg, "geo.box").unwrap();
        let b = g.add_node(&reg, "clean.smooth_mesh").unwrap();
        let c = g.add_node(&reg, "clean.smooth_mesh").unwrap();
        let d = g.add_node(&reg, "geo.merge").unwrap();
        g.connect(a, 0, b, 0).unwrap();
        g.connect(a, 0, c, 0).unwrap();
        g.connect(b, 0, d, 0).unwrap();
        g.connect(c, 0, d, 1).unwrap();
        let pos = layout_positions(&g);
        let rank = |id: NodeId| ((pos[&id][1] - 30.0) / 110.0).round() as i32;
        assert_eq!([rank(a), rank(b), rank(c), rank(d)], [0, 1, 1, 2]);
        assert_ne!(pos[&b][0], pos[&c][0]);
    }
}

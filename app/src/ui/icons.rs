//! In-house toolbar icons drawn with egui shapes (24-point grid, 1.5 stroke,
//! no text, tinted at draw time).

use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Response, Sense, Shape, Stroke, Ui};

use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Select,
    Move,
    Rotate,
    Scale,
    Undo,
    Redo,
    Snap,
    AngleSnap,
    ZoomExtents,
    Pan,
    Orbit,
    Maximize,
    ToStart,
    PrevFrame,
    Play,
    Pause,
    NextFrame,
    ToEnd,
    NodeEditor,
    Gates,
    Report,
    Bulb,
    BulbOff,
}

fn arrow_head(out: &mut Vec<Shape>, tip: Pos2, dir: egui::Vec2, s: f32, stroke: Stroke) {
    let d = dir.normalized() * s;
    let n = vec2(-d.y, d.x) * 0.6;
    out.push(Shape::line(vec![tip - d + n, tip, tip - d - n], stroke));
}

fn arc(c: Pos2, r: f32, a0: f32, a1: f32) -> Vec<Pos2> {
    (0..=16)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / 16.0;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect()
}

/// Paint `icon` into `rect` with colour `c`.
pub fn paint(painter: &egui::Painter, rect: Rect, icon: Icon, c: Color32) {
    let s = rect.width().min(rect.height()) / 24.0;
    let p = |x: f32, y: f32| rect.center() + vec2((x - 12.0) * s, (y - 12.0) * s);
    let st = Stroke::new(1.5 * s.max(0.7), c);
    let mut v: Vec<Shape> = Vec::new();
    match icon {
        Icon::Select => {
            v.push(Shape::convex_polygon(
                vec![p(7.0, 4.0), p(18.0, 13.0), p(12.5, 13.5), p(9.5, 19.0)],
                Color32::TRANSPARENT,
                st,
            ));
        }
        Icon::Move => {
            v.push(Shape::line_segment([p(12.0, 4.0), p(12.0, 20.0)], st));
            v.push(Shape::line_segment([p(4.0, 12.0), p(20.0, 12.0)], st));
            for (tip, d) in [
                (p(12.0, 4.0), vec2(0.0, -1.0)),
                (p(12.0, 20.0), vec2(0.0, 1.0)),
                (p(4.0, 12.0), vec2(-1.0, 0.0)),
                (p(20.0, 12.0), vec2(1.0, 0.0)),
            ] {
                arrow_head(&mut v, tip, d, 3.0 * s, st);
            }
        }
        Icon::Rotate => {
            v.push(Shape::line(arc(p(12.0, 12.0), 7.0 * s, -2.6, 2.2), st));
            let tip = p(12.0, 12.0) + vec2(2.2f32.cos(), 2.2f32.sin()) * 7.0 * s;
            arrow_head(&mut v, tip, vec2(-(2.2f32.sin()), 2.2f32.cos()), 3.0 * s, st);
        }
        Icon::Scale => {
            v.push(Shape::rect_stroke(
                Rect::from_min_max(p(4.0, 9.0), p(15.0, 20.0)),
                0.0,
                st,
                egui::StrokeKind::Middle,
            ));
            v.push(Shape::rect_stroke(
                Rect::from_min_max(p(9.0, 4.0), p(20.0, 15.0)),
                0.0,
                Stroke::new(st.width, c.gamma_multiply(0.6)),
                egui::StrokeKind::Middle,
            ));
        }
        Icon::Undo | Icon::Redo => {
            let flip = if icon == Icon::Redo { -1.0 } else { 1.0 };
            let q = |x: f32, y: f32| p(12.0 + (x - 12.0) * flip, y);
            let pts: Vec<Pos2> = arc(pos2(0.0, 0.0), 1.0, -1.57, 1.57)
                .into_iter()
                .map(|a| q(12.0 + a.x * 6.0, 12.0 + a.y * 5.0))
                .collect();
            v.push(Shape::line(pts, st));
            v.push(Shape::line_segment([q(12.0, 7.0), q(5.0, 7.0)], st));
            arrow_head(&mut v, q(5.0, 7.0), vec2(-flip, 0.0), 3.0 * s, st);
            v.push(Shape::line_segment([q(12.0, 17.0), q(8.0, 17.0)], st));
        }
        Icon::Snap | Icon::AngleSnap => {
            v.push(Shape::line(arc(p(12.0, 11.0), 6.0 * s, 0.0, std::f32::consts::PI), st));
            v.push(Shape::line_segment([p(6.0, 11.0), p(6.0, 5.0)], st));
            v.push(Shape::line_segment([p(18.0, 11.0), p(18.0, 5.0)], st));
            if icon == Icon::AngleSnap {
                v.push(Shape::line(vec![p(4.0, 21.0), p(20.0, 21.0), p(14.0, 15.0)], st));
            }
        }
        Icon::ZoomExtents => {
            for (x, y, dx, dy) in [
                (4.0, 4.0, 1.0, 1.0),
                (20.0, 4.0, -1.0, 1.0),
                (4.0, 20.0, 1.0, -1.0),
                (20.0, 20.0, -1.0, -1.0),
            ] {
                v.push(Shape::line(vec![p(x + 5.0 * dx, y), p(x, y), p(x, y + 5.0 * dy)], st));
            }
            v.push(Shape::rect_stroke(
                Rect::from_min_max(p(9.0, 9.0), p(15.0, 15.0)),
                0.0,
                st,
                egui::StrokeKind::Middle,
            ));
        }
        Icon::Pan => {
            v.push(Shape::circle_stroke(p(12.0, 12.0), 2.0 * s, st));
            for (tip, d) in [
                (p(12.0, 4.0), vec2(0.0, -1.0)),
                (p(12.0, 20.0), vec2(0.0, 1.0)),
                (p(4.0, 12.0), vec2(-1.0, 0.0)),
                (p(20.0, 12.0), vec2(1.0, 0.0)),
            ] {
                arrow_head(&mut v, tip, d, 3.0 * s, st);
            }
        }
        Icon::Orbit => {
            v.push(Shape::circle_stroke(p(12.0, 12.0), 3.0 * s, st));
            let pts: Vec<Pos2> = arc(pos2(0.0, 0.0), 1.0, 0.0, std::f32::consts::TAU)
                .into_iter()
                .map(|a| p(12.0 + a.x * 9.0, 12.0 + a.y * 4.0))
                .collect();
            v.push(Shape::line(pts, st));
        }
        Icon::Maximize => {
            v.push(Shape::rect_stroke(
                Rect::from_min_max(p(5.0, 5.0), p(19.0, 19.0)),
                1.0,
                st,
                egui::StrokeKind::Middle,
            ));
            v.push(Shape::line_segment([p(12.0, 5.0), p(12.0, 19.0)], Stroke::new(st.width * 0.6, c)));
            v.push(Shape::line_segment([p(5.0, 12.0), p(19.0, 12.0)], Stroke::new(st.width * 0.6, c)));
        }
        Icon::Play => {
            v.push(Shape::convex_polygon(vec![p(8.0, 5.0), p(19.0, 12.0), p(8.0, 19.0)], c, Stroke::NONE));
        }
        Icon::Pause => {
            v.push(Shape::rect_filled(Rect::from_min_max(p(7.0, 5.0), p(10.5, 19.0)), 0.0, c));
            v.push(Shape::rect_filled(Rect::from_min_max(p(13.5, 5.0), p(17.0, 19.0)), 0.0, c));
        }
        Icon::PrevFrame | Icon::NextFrame | Icon::ToStart | Icon::ToEnd => {
            let flip = matches!(icon, Icon::NextFrame | Icon::ToEnd);
            let q = |x: f32, y: f32| if flip { p(24.0 - x, y) } else { p(x, y) };
            v.push(Shape::convex_polygon(vec![q(17.0, 6.0), q(17.0, 18.0), q(8.0, 12.0)], c, Stroke::NONE));
            if matches!(icon, Icon::ToStart | Icon::ToEnd) {
                v.push(Shape::line_segment([q(6.0, 6.0), q(6.0, 18.0)], st));
            }
        }
        Icon::NodeEditor => {
            for (x, y) in [(4.0, 5.0), (14.0, 14.0)] {
                v.push(Shape::rect_stroke(
                    Rect::from_min_size(p(x, y), vec2(6.0 * s, 5.0 * s)),
                    1.0,
                    st,
                    egui::StrokeKind::Middle,
                ));
            }
            v.push(Shape::line(vec![p(10.0, 7.5), p(12.0, 7.5), p(12.0, 16.5), p(14.0, 16.5)], st));
        }
        Icon::Gates => {
            for (i, y) in [6.0, 12.0, 18.0].into_iter().enumerate() {
                v.push(Shape::circle_stroke(p(6.0, y), 1.8 * s, st));
                let w = [12.0, 9.0, 11.0][i];
                v.push(Shape::line_segment([p(10.0, y), p(10.0 + w, y)], st));
            }
        }
        Icon::Report => {
            v.push(Shape::rect_stroke(
                Rect::from_min_max(p(6.0, 4.0), p(18.0, 20.0)),
                1.0,
                st,
                egui::StrokeKind::Middle,
            ));
            for y in [9.0, 12.5, 16.0] {
                v.push(Shape::line_segment([p(9.0, y), p(15.0, y)], Stroke::new(st.width * 0.8, c)));
            }
        }
        Icon::Bulb | Icon::BulbOff => {
            let fill = if icon == Icon::Bulb { c } else { Color32::TRANSPARENT };
            v.push(Shape::circle_filled(p(12.0, 10.0), 5.5 * s, fill));
            v.push(Shape::circle_stroke(p(12.0, 10.0), 5.5 * s, st));
            v.push(Shape::line_segment([p(9.5, 18.0), p(14.5, 18.0)], st));
            v.push(Shape::line_segment([p(10.0, 20.5), p(14.0, 20.5)], st));
        }
    }
    painter.extend(v);
}

/// A square icon button; `on` draws it latched (active tool or toggle).
pub fn button(ui: &mut Ui, icon: Icon, on: bool, tip: &str) -> Response {
    button_sized(ui, icon, on, tip, 24.0)
}

pub fn button_sized(ui: &mut Ui, icon: Icon, on: bool, tip: &str, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let enabled = ui.is_enabled();
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let bg = if on {
            theme::mix(theme::ACCENT, theme::SURFACE, 0.55)
        } else if resp.hovered() && enabled {
            Color32::from_rgb(0x45, 0x45, 0x45)
        } else {
            Color32::TRANSPARENT
        };
        painter.rect_filled(rect, 3.0, bg);
        if resp.hovered() && enabled {
            painter.rect_stroke(rect, 3.0, Stroke::new(1.0_f32, theme::ACCENT), egui::StrokeKind::Inside);
        }
        let color = if !enabled {
            theme::DISABLED
        } else if on {
            Color32::WHITE
        } else {
            theme::TEXT
        };
        paint(painter, rect.shrink(size * 0.12), icon, color);
    }
    resp.on_hover_text(tip)
}

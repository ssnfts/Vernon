//! Time slider, track bar and the status bar (listener, prompt, selection
//! count, cursor X/Y/Z, grid size, playback and navigation buttons).

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Key, Rect, RichText, Sense, Stroke, StrokeKind};

use super::icons::{self, Icon};
use super::VernonApp;
use crate::hotkeys::Action;
use crate::theme;
use crate::viewport::grid;

fn frame_x(app: &VernonApp, rect: Rect, f: i64) -> f32 {
    let len = app.time.length().max(1) as f32;
    rect.left() + (f - app.time.start) as f32 / len * rect.width()
}

pub fn time_slider(app: &mut VernonApp, ctx: &egui::Context) {
    egui::TopBottomPanel::bottom("time_slider")
        .exact_height(24.0)
        .frame(egui::Frame::NONE.fill(theme::SURFACE).inner_margin(egui::Margin::symmetric(6, 2)))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.small_button("<").on_hover_text("Previous frame (,)").clicked() {
                    app.do_action(Action::PrevFrame, ctx);
                }
                let w = ui.available_width() - 28.0;
                let (rect, resp) = ui.allocate_exact_size(vec2(w.max(50.0), 18.0), Sense::click_and_drag());
                let p = ui.painter_at(rect);
                p.rect_filled(rect, 2.0, Color32::from_rgb(0x1C, 0x1C, 0x1C));
                let thumb_w = 64.0;
                let track = rect.shrink2(vec2(thumb_w * 0.5, 0.0));
                if let Some(pos) = resp.interact_pointer_pos() {
                    if resp.dragged() || resp.clicked() {
                        let t = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
                        let f = app.time.start + (t * app.time.length() as f32).round() as i64;
                        app.play.set_frame(&app.time, f, app.now);
                    }
                }
                let x = frame_x(app, track, app.play.frame);
                let thumb = Rect::from_center_size(pos2(x, rect.center().y), vec2(thumb_w, rect.height() - 2.0));
                p.rect_filled(thumb, 2.0, Color32::from_rgb(0x55, 0x55, 0x55));
                p.rect_stroke(thumb, 2.0, Stroke::new(1.0_f32, theme::TEXT_DIM), StrokeKind::Inside);
                p.text(
                    thumb.center(),
                    Align2::CENTER_CENTER,
                    app.time.slider_text(app.play.frame),
                    FontId::proportional(11.0),
                    theme::TEXT,
                );
                resp.on_hover_text("Study slider: each frame is one sample of the active sweep");
                if ui.small_button(">").on_hover_text("Next frame (.)").clicked() {
                    app.do_action(Action::NextFrame, ctx);
                }
            });
        });
}

pub fn track_bar(app: &mut VernonApp, ctx: &egui::Context) {
    egui::TopBottomPanel::bottom("track_bar")
        .exact_height(22.0)
        .frame(egui::Frame::NONE.fill(Color32::from_rgb(0x2C, 0x2C, 0x2C)).inner_margin(egui::Margin::symmetric(6, 1)))
        .show(ctx, |ui| {
            let rect = ui.available_rect_before_wrap();
            let p = ui.painter_at(rect);
            let track = rect.shrink2(vec2(32.0 + 6.0, 0.0));
            let len = app.time.length().max(1);
            let step = [1, 5, 10, 25, 50, 100, 250, 500, 1000]
                .into_iter()
                .find(|s| track.width() / (len as f32 / *s as f32) >= 6.0)
                .unwrap_or(1000);
            let mut f = app.time.start - app.time.start.rem_euclid(step);
            while f <= app.time.end {
                if f >= app.time.start {
                    let x = frame_x(app, track, f);
                    let major = f % (step * 5) == 0;
                    let h = if major { 9.0 } else { 4.0 };
                    p.line_segment(
                        [pos2(x, rect.top() + 1.0), pos2(x, rect.top() + 1.0 + h)],
                        Stroke::new(1.0_f32, theme::DISABLED),
                    );
                    if major {
                        p.text(
                            pos2(x + 2.0, rect.bottom() - 1.0),
                            Align2::LEFT_BOTTOM,
                            app.time.format(f),
                            FontId::proportional(9.5),
                            theme::TEXT_DIM,
                        );
                    }
                }
                f += step;
            }
            let x = frame_x(app, track, app.play.frame);
            p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(2.0_f32, theme::AXIS_Z));
            ui.allocate_rect(rect, Sense::hover());
        });
}

pub fn status_bar(app: &mut VernonApp, ctx: &egui::Context) {
    egui::TopBottomPanel::bottom("status_bar")
        .exact_height(50.0)
        .frame(egui::Frame::NONE.fill(theme::SURFACE_DEEP).inner_margin(egui::Margin::symmetric(6, 3)))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Mini listener: input line over the last output line.
                ui.vertical(|ui| {
                    ui.set_width(220.0);
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut app.listener_line)
                            .hint_text("add box | cook | select <name>")
                            .desired_width(216.0)
                            .font(egui::TextStyle::Monospace),
                    );
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        let line = std::mem::take(&mut app.listener_line);
                        app.run_command(&line);
                        edit.request_focus();
                    }
                    if edit.has_focus() && ui.input(|i| i.key_pressed(Key::ArrowUp)) {
                        if let Some(last) = app.listener_history.last() {
                            app.listener_line = last.clone();
                        }
                    }
                    let (text, col) = match app.st.log.last() {
                        Some(l) if l.error => (l.text.as_str(), theme::FAIL),
                        Some(l) => (l.text.as_str(), theme::TEXT_DIM),
                        None => ("", theme::TEXT_DIM),
                    };
                    ui.add(egui::Label::new(RichText::new(text).small().color(col)).truncate());
                });
                ui.separator();
                // Selection count and prompt.
                ui.vertical(|ui| {
                    ui.set_width((ui.available_width() - 640.0).max(160.0));
                    let n = app.st.selection.len();
                    let sel = match n {
                        0 => "None Selected".to_string(),
                        1 => format!("1 Object Selected: {}", app.st.name(app.st.selection[0])),
                        n => format!("{n} Objects Selected"),
                    };
                    ui.add(egui::Label::new(RichText::new(sel).color(theme::TEXT)).truncate());
                    let (prompt, warn) = app.prompt();
                    let col = if warn { theme::WARN } else { theme::TEXT_DIM };
                    ui.add(egui::Label::new(RichText::new(prompt).small().color(col)).truncate());
                });
                ui.separator();
                // Cursor coordinates and grid.
                ui.vertical(|ui| {
                    ui.set_width(250.0);
                    let enabled = app.vp.cursor_world.is_some();
                    let p = app.vp.cursor_world.unwrap_or_default();
                    let col = if enabled { theme::TEXT } else { theme::DISABLED };
                    ui.horizontal(|ui| {
                        for (axis, v, c) in [("X", p.x, theme::AXIS_X), ("Y", p.y, theme::AXIS_Y), ("Z", p.z, theme::AXIS_Z)] {
                            ui.label(RichText::new(axis).strong().color(c));
                            ui.label(RichText::new(format!("{v:>9.2}")).monospace().color(col));
                        }
                    });
                    ui.label(
                        RichText::new(format!("Grid = {}", grid::format_spacing(app.vp.grid_readout)))
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                });
                ui.separator();
                // Playback.
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        let b = |ui: &mut egui::Ui, i: Icon, tip: &str| icons::button_sized(ui, i, false, tip, 20.0).clicked();
                        if b(ui, Icon::ToStart, "Go to start (Home)") {
                            app.do_action(Action::GoStart, ctx);
                        }
                        if b(ui, Icon::PrevFrame, "Previous frame (,)") {
                            app.do_action(Action::PrevFrame, ctx);
                        }
                        let play_icon = if app.play.playing { Icon::Pause } else { Icon::Play };
                        if icons::button_sized(ui, play_icon, app.play.playing, "Play / stop (/)", 20.0).clicked() {
                            app.do_action(Action::Play, ctx);
                        }
                        if b(ui, Icon::NextFrame, "Next frame (.)") {
                            app.do_action(Action::NextFrame, ctx);
                        }
                        if b(ui, Icon::ToEnd, "Go to end (End)") {
                            app.do_action(Action::GoEnd, ctx);
                        }
                    });
                    ui.horizontal(|ui| {
                        let mut f = app.play.frame;
                        let r = ui.add(
                            egui::DragValue::new(&mut f)
                                .range(app.time.start..=app.time.end)
                                .speed(0.5),
                        );
                        if r.changed() {
                            app.play.set_frame(&app.time, f, app.now);
                        }
                        ui.label(RichText::new(format!("{} fps", app.time.fps)).small().color(theme::TEXT_DIM));
                    });
                });
                ui.separator();
                // Navigation buttons for the active viewport.
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        if icons::button_sized(ui, Icon::ZoomExtents, false, "Zoom extents selected (Z)", 20.0).clicked() {
                            app.do_action(Action::ZoomExtentsSelected, ctx);
                        }
                        if icons::button_sized(ui, Icon::Pan, false, "Pan: middle-drag in a viewport", 20.0).clicked() {
                            app.st.info_text("Pan: drag with the middle mouse button");
                        }
                        if icons::button_sized(ui, Icon::Orbit, false, "Orbit: Alt + middle-drag", 20.0).clicked() {
                            app.st.info_text("Orbit: hold Alt and drag with the middle mouse button");
                        }
                        let max = app.vp.layout.maximized;
                        if icons::button_sized(ui, Icon::Maximize, max, "Maximise viewport (Alt+W)", 20.0).clicked() {
                            app.do_action(Action::Maximize, ctx);
                        }
                    });
                    if icons::button_sized(ui, Icon::ZoomExtents, false, "Zoom extents all viewports (Shift+Ctrl+Z)", 20.0)
                        .clicked()
                    {
                        app.do_action(Action::ZoomExtentsAllViews, ctx);
                    }
                });
            });
        });
}

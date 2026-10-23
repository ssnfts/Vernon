//! Time slider, track bar, the status bar (listener, prompt, selection
//! count, X/Y/Z cursor readout or transform type-in, grid size, playback and
//! navigation buttons) and the F12 Transform Type-In dialog.

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Key, Rect, RichText, Sense, Stroke, StrokeKind};

use super::icons::{self, Icon};
use super::VernonApp;
use crate::hotkeys::Action;
use crate::study::SampleState;
use crate::theme;
use crate::typein::{readout, Channel, Mode};
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
                let sample = match app.study.state(app.play.frame) {
                    _ if !app.study.active() => "no study sweep defined (Motion tab)",
                    Some(SampleState::Solved) => "this sample is solved",
                    Some(SampleState::Failed) => "this sample failed",
                    Some(SampleState::Stale) => "this sample is stale",
                    None => "this sample is not solved yet",
                };
                resp.on_hover_text(format!("Study slider: each frame is one sample of the sweep; {sample}"));
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
            // study samples: solved, failed or stale since a later edit
            let w = (track.width() / len as f32).clamp(2.0, 8.0);
            let (mut solved, mut failed, mut stale) = (0, 0, 0);
            for (&f, &s) in app.study.samples.range(app.time.start..=app.time.end) {
                let color = match s {
                    SampleState::Solved => {
                        solved += 1;
                        theme::PASS
                    }
                    SampleState::Failed => {
                        failed += 1;
                        theme::FAIL
                    }
                    SampleState::Stale => {
                        stale += 1;
                        theme::DISABLED
                    }
                };
                let x = frame_x(app, track, f);
                p.rect_filled(Rect::from_min_max(pos2(x - w * 0.5, rect.top() + 12.0), pos2(x + w * 0.5, rect.bottom() - 1.0)), 1.0, color);
            }
            let x = frame_x(app, track, app.play.frame);
            p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(2.0_f32, theme::AXIS_Z));
            let hover = if app.study.active() || !app.study.samples.is_empty() {
                format!("Study samples: {solved} solved (green), {failed} failed (red), {stale} stale (grey)")
            } else {
                "Define a study sweep in the Motion tab to mark solved, failed and stale samples here".into()
            };
            ui.allocate_rect(rect, Sense::hover()).on_hover_text(hover);
        });
}

/// X, Y and Z fields: editable transform values (spec 3.11) when a
/// transform tool is active and something is selected, else the cursor
/// readout (greyed when the pointer is outside the viewports).
fn typein_fields(app: &mut VernonApp, ui: &mut egui::Ui, mode: Mode) {
    let axes = [("X", theme::AXIS_X), ("Y", theme::AXIS_Y), ("Z", theme::AXIS_Z)];
    match (app.typein_channel(), app.st.primary_xform()) {
        (Some(ch), Some(x)) => {
            let vals = readout(ch, &x, mode);
            let speed = match ch {
                Channel::Translate => (app.vp.grid_readout * 0.1).max(0.01),
                Channel::Rotate => 0.5,
                Channel::Scale => 0.5,
            };
            for (axis, (name, c)) in axes.into_iter().enumerate() {
                ui.label(RichText::new(name).strong().color(c));
                let mut v = vals[axis];
                let r = ui
                    .add(
                        egui::DragValue::new(&mut v)
                            .speed(speed)
                            .max_decimals(3)
                            .update_while_editing(false)
                            .min_decimals(if ch == Channel::Translate { 2 } else { 1 }),
                    )
                    .on_hover_text(format!("{name} {} ({}); Enter applies", ch_label(ch), ch.unit()));
                if r.changed() && v != vals[axis] {
                    if let Err(e) = app.st.type_in(ch, mode, axis, v, app.now) {
                        app.st.error_text(e);
                    }
                }
            }
        }
        _ => {
            let enabled = app.vp.cursor_world.is_some();
            let p = app.vp.cursor_world.unwrap_or_default();
            let col = if enabled { theme::TEXT } else { theme::DISABLED };
            for (axis, (name, c)) in axes.into_iter().enumerate() {
                ui.label(RichText::new(name).strong().color(c));
                ui.label(RichText::new(format!("{:>9.2}", p[axis])).monospace().color(col))
                    .on_hover_text("Cursor position. Pick Move, Rotate or Scale with an object selected to type a transform.");
            }
        }
    }
}

fn ch_label(ch: Channel) -> &'static str {
    match ch {
        Channel::Translate => "position",
        Channel::Rotate => "rotation",
        Channel::Scale => "scale",
    }
}

/// F12 Transform Type-In: the Absolute and Offset fields side by side.
pub fn typein_window(app: &mut VernonApp, ctx: &egui::Context) {
    if !app.show.typein {
        return;
    }
    let mut open = true;
    let title = match app.typein_channel() {
        Some(ch) => format!("{} Transform Type-In", match ch {
            Channel::Translate => "Move",
            Channel::Rotate => "Rotate",
            Channel::Scale => "Scale",
        }),
        None => "Transform Type-In".to_string(),
    };
    egui::Window::new(title)
        .id(egui::Id::new("typein_window"))
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            let Some(ch) = app.typein_channel() else {
                ui.label(RichText::new("Pick Move, Rotate or Scale (W, E, R).").color(theme::TEXT_DIM));
                return;
            };
            if app.st.primary_xform().is_none() {
                ui.label(RichText::new("Nothing selected.").color(theme::TEXT_DIM));
            }
            for (mode, label) in [(Mode::Absolute, "Absolute"), (Mode::Offset, "Offset")] {
                ui.label(RichText::new(format!("{label} ({})", ch.unit())).small().color(theme::TEXT_DIM));
                ui.horizontal(|ui| typein_fields(app, ui, mode));
            }
        });
    if !open {
        app.show.typein = false;
    }
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
                        Some(l) if l.error => (l.text.as_str(), theme::FAIL_TEXT),
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
                    match app.st.cook_status() {
                        Some(s) if s.busy => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                if ui.small_button("Stop").on_hover_text("Stop after the node being cooked").clicked() {
                                    app.st.stop_cook();
                                }
                                let node = s.progress.current.as_deref().unwrap_or("…");
                                let text = format!("Cooking {node} ({} done, {:.0} s); results shown are out of date", s.progress.cooked, s.seconds);
                                ui.add(egui::Label::new(RichText::new(text).small().color(theme::WARN)).truncate());
                            });
                        }
                        Some(s) if s.stale => {
                            let text = if s.stopped {
                                "Cook stopped: results are out of date until the next change"
                            } else {
                                "Results are out of date"
                            };
                            ui.add(egui::Label::new(RichText::new(text).small().color(theme::WARN)).truncate());
                        }
                        _ => {
                            let (prompt, warn) = app.prompt();
                            let col = if warn { theme::WARN } else { theme::TEXT_DIM };
                            ui.add(egui::Label::new(RichText::new(prompt).small().color(col)).truncate());
                        }
                    }
                });
                ui.separator();
                // Coordinates: the transform type-in with a transform tool
                // and a selection (Absolute / Offset toggle), else the
                // cursor on the construction plane.
                ui.vertical(|ui| {
                    ui.set_width(280.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        let offset = app.typein_mode == Mode::Offset;
                        let tip = "Type-in mode. Absolute (off): typed values replace the transform. \
                                   Offset (on): typed values are added and the fields return to 0. F12 opens the dialog.";
                        if ui.selectable_label(offset, if offset { "Off" } else { "Abs" }).on_hover_text(tip).clicked() {
                            app.typein_mode = if offset { Mode::Absolute } else { Mode::Offset };
                        }
                        let mode = app.typein_mode;
                        typein_fields(app, ui, mode);
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

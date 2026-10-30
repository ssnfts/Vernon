//! Scene Explorer: one row per object (a node nobody consumes) with
//! visibility and lock toggles, colour, name, gate, stage and last run,
//! sortable by name, stage or last run; selection synced both ways with the
//! viewports, and the object's stack listed underneath.

use std::time::SystemTime;

use eframe::egui::{self, vec2, Color32, RichText, Sense};
use vernon_graph::{NodeId, Stage};

use super::icons::{self, Icon};
use super::{ExplorerSort, PanelTab, VernonApp};
use crate::gate::{report_state, GateState};
use crate::theme;

fn gate_glyph(app: &VernonApp, id: NodeId) -> Option<(GateState, Color32)> {
    if app.st.error_of(id).is_some() {
        return Some((GateState::Error, theme::FAIL_TEXT));
    }
    let r = app.st.report(id)?;
    let s = report_state(&r);
    let c = match s {
        GateState::Pass => theme::PASS,
        GateState::Fail => theme::FAIL_TEXT,
        _ => theme::TEXT_DIM,
    };
    Some((s, c))
}

pub fn show(app: &mut VernonApp, ctx: &egui::Context) {
    egui::SidePanel::left("scene_explorer")
        .resizable(true)
        .default_width(220.0)
        .width_range(160.0..=420.0)
        .show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Scene Explorer").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{}", app.st.objects().len())).color(theme::TEXT_DIM))
                        .on_hover_text("Objects in the scene");
                });
            });
            let find = ui.add(
                egui::TextEdit::singleline(&mut app.explorer_filter)
                    .hint_text("Find by name…")
                    .desired_width(f32::INFINITY),
            );
            if std::mem::take(&mut app.focus_explorer_filter) {
                find.request_focus();
            }
            // Column headers: click to sort by that column, again for scene order.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new("Sort").small().color(theme::TEXT_DIM));
                for (s, label) in [(ExplorerSort::Name, "Name"), (ExplorerSort::Stage, "Stage"), (ExplorerSort::LastRun, "Last run")] {
                    let on = app.explorer_sort == s;
                    if ui.selectable_label(on, RichText::new(label).small()).clicked() {
                        app.explorer_sort = if on { ExplorerSort::Scene } else { s };
                    }
                }
            });
            ui.separator();
            let filter = app.explorer_filter.to_lowercase();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let mut objects = app.st.objects();
                sort_objects(app, &mut objects);
                if objects.is_empty() {
                    ui.label(
                        RichText::new("Empty scene.\nCreate an object from the ribbon or the Create panel.")
                            .color(theme::TEXT_DIM),
                    );
                }
                for obj in objects {
                    let stack = app.st.graph.stack(obj);
                    let matches = filter.is_empty()
                        || stack.iter().any(|n| app.st.name(*n).to_lowercase().contains(&filter));
                    if !matches {
                        continue;
                    }
                    object_row(app, ui, obj, &stack);
                }
            });
        });
}

fn stage_rank(app: &VernonApp, id: NodeId) -> usize {
    app.st
        .info(id)
        .and_then(|i| Stage::ALL.iter().position(|s| *s == i.stage))
        .unwrap_or(usize::MAX)
}

fn sort_objects(app: &VernonApp, objects: &mut [NodeId]) {
    let name = |id: &NodeId| app.st.name(*id).to_lowercase();
    match app.explorer_sort {
        ExplorerSort::Scene => {}
        ExplorerSort::Name => objects.sort_by_key(name),
        ExplorerSort::Stage => objects.sort_by_key(|id| (stage_rank(app, *id), name(id))),
        // Newest first, never-run objects last.
        ExplorerSort::LastRun => objects.sort_by_key(|id| std::cmp::Reverse(app.st.last_run.get(id).copied())),
    }
}

/// "12 s", "5 min", "3 h" since `t`.
pub fn ago(t: SystemTime) -> String {
    let s = t.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} min", s / 60),
        _ => format!("{} h", s / 3600),
    }
}

fn object_row(app: &mut VernonApp, ui: &mut egui::Ui, obj: NodeId, stack: &[NodeId]) {
    let hidden = app.st.is_hidden(obj);
    let selected = app.st.selection.contains(&obj);
    let id = ui.make_persistent_id(("explorer_obj", obj));
    let state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    state
        .show_header(ui, |ui| {
            let icon = if hidden { Icon::BulbOff } else { Icon::Bulb };
            if icons::button_sized(ui, icon, false, if hidden { "Show" } else { "Hide" }, 18.0).clicked() {
                app.st.set_hidden(obj, !hidden);
            }
            let locked = app.st.is_locked(obj);
            let (icon, tip) = if locked {
                (Icon::Lock, "Locked: cannot be picked or transformed in the viewports. Click to unlock.")
            } else {
                (Icon::Unlock, "Lock: stop picking and transforming in the viewports")
            };
            if icons::button_sized(ui, icon, locked, tip, 16.0).clicked() {
                app.st.set_locked(obj, !locked);
            }
            let c = app.st.color_of(obj);
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().rect_filled(r, 2.0, Color32::from_rgb(c[0], c[1], c[2]));
            let name = app.st.name(obj).to_string();
            let mut text = RichText::new(&name);
            if hidden {
                text = text.color(theme::DISABLED);
            }
            let resp = ui.selectable_label(selected, text);
            if resp.clicked() {
                let ctrl = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
                if ctrl {
                    app.st.toggle_select(obj);
                } else {
                    app.st.select_only(obj);
                }
            }
            if resp.double_clicked() {
                app.st.select_only(obj);
                app.tab = PanelTab::Modify;
            }
            if let Some((s, col)) = gate_glyph(app, obj) {
                ui.label(RichText::new(s.glyph()).color(col)).on_hover_text(s.text());
            }
            // Stage and last run, right-aligned.
            let stage = app.st.info(obj).map(|i| i.stage.label()).unwrap_or("?");
            let run = app.st.last_run.get(&obj).copied();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (text, tip) = match run {
                    Some(t) => (ago(t), "Time since this object last produced a new result"),
                    None => ("–".to_string(), "Not run yet in this session"),
                };
                ui.label(RichText::new(text).small().color(theme::TEXT_DIM)).on_hover_text(tip);
                ui.add(egui::Label::new(RichText::new(stage).small().color(theme::TEXT_DIM)).truncate())
                    .on_hover_text(format!("Stage: {stage}"));
            });
            resp.context_menu(|ui| {
                if ui.button(if hidden { "Unhide" } else { "Hide" }).clicked() {
                    app.st.set_hidden(obj, !hidden);
                }
                if ui.button("Isolate   Alt+Q").clicked() {
                    app.st.end_isolate(&std::mem::take(&mut app.isolated));
                    app.isolated = app.st.isolate(&[obj]);
                }
                if ui.button(if app.st.is_locked(obj) { "Unlock" } else { "Lock" }).clicked() {
                    let l = app.st.is_locked(obj);
                    app.st.set_locked(obj, !l);
                }
                if ui.button("Select").clicked() {
                    app.st.select_only(obj);
                }
                if ui.button("Delete").clicked() {
                    app.st.delete(&[obj]);
                }
            });
        })
        .body(|ui| {
            for n in stack.iter().rev() {
                let info = app.st.info(*n).map(|i| (i.label.clone(), i.stage.label()));
                let (label, stage) = info.unwrap_or_else(|| ("?".into(), ""));
                let mut text = RichText::new(format!("{}  ·  {label}", app.st.name(*n))).small();
                if app.st.graph.nodes.get(n).is_some_and(|x| x.bypass) {
                    text = text.color(theme::DISABLED).strikethrough();
                } else if app.st.error_of(*n).is_some() {
                    text = text.color(theme::FAIL_TEXT);
                }
                let current = app.st.primary() == Some(obj) && app.st.current_entry() == Some(*n);
                let r = ui.selectable_label(current, text).on_hover_text(stage);
                if r.clicked() {
                    app.st.select_only(obj);
                    app.st.stack_entry = Some(*n);
                    app.tab = PanelTab::Modify;
                }
            }
        });
}

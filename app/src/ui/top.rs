//! Menu bar, main toolbar and the stage ribbon.

use eframe::egui::{self, Button, Color32, RichText, Stroke, Ui, ViewportCommand};
use vernon_graph::{DataKind, Stage};

use super::icons::{self, Icon};
use super::{FileMode, PanelTab, Tool, Utility, VernonApp};
use crate::hotkeys::{shortcut_for, Action};
use crate::theme;
use crate::viewport::OrbitMode;

fn item(ui: &mut Ui, text: &str, action: Option<Action>) -> bool {
    let sc = action.map(shortcut_for).unwrap_or_default();
    ui.add(Button::new(text).shortcut_text(sc)).clicked()
}

/// Node types of a stage that take a geometry-like first input.
fn modifier_types(app: &VernonApp, stage: Stage) -> Vec<(String, String, String)> {
    app.st
        .registry
        .by_stage(stage)
        .into_iter()
        .filter(|i| i.inputs.first().is_some_and(|p| p.kind != DataKind::Report))
        .map(|i| (i.id.clone(), i.label.clone(), i.help.clone()))
        .collect()
}

pub fn menu_bar(app: &mut VernonApp, ctx: &egui::Context) {
    egui::TopBottomPanel::top("menu_bar")
        .frame(egui::Frame::NONE.fill(theme::SURFACE_DEEP).inner_margin(egui::Margin::symmetric(6, 2)))
        .show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.label(RichText::new("◆ Vernon").strong().color(theme::ACCENT));
                ui.add_space(6.0);
                ui.menu_button("File", |ui| file_menu(app, ui));
                ui.menu_button("Edit", |ui| edit_menu(app, ui, ctx));
                ui.menu_button("Create", |ui| {
                    for stage in Stage::ALL {
                        let types: Vec<(String, String, String)> = app
                            .st
                            .registry
                            .by_stage(stage)
                            .into_iter()
                            .map(|i| (i.id.clone(), i.label.clone(), i.help.clone()))
                            .collect();
                        ui.menu_button(stage.label(), |ui| {
                            for (id, label, help) in types {
                                if ui.button(label).on_hover_text(help).clicked() {
                                    app.create(&id);
                                }
                            }
                        });
                    }
                });
                ui.menu_button("Modifiers", |ui| {
                    let enabled = app.st.primary().is_some();
                    if !enabled {
                        ui.label(RichText::new("Select an object first").color(theme::TEXT_DIM));
                    }
                    for stage in Stage::ALL {
                        let types = modifier_types(app, stage);
                        if types.is_empty() {
                            continue;
                        }
                        ui.add_enabled_ui(enabled, |ui| {
                            ui.menu_button(stage.label(), |ui| {
                                for (id, label, help) in types {
                                    if ui.button(label).on_hover_text(help).clicked() {
                                        app.add_modifier(&id);
                                    }
                                }
                            });
                        });
                    }
                });
                ui.menu_button("Pipeline", |ui| {
                    if ui.button("Cook All").clicked() {
                        app.run_command("cook");
                    }
                    if ui.button("Load Demo Pipeline").clicked() {
                        if let Err(e) = app.st.build_demo_pipeline() {
                            app.st.error_text(e);
                        }
                        app.vp.request_fit();
                    }
                    ui.separator();
                    ui.checkbox(&mut app.show.gates, "Gate Dashboard");
                    ui.checkbox(&mut app.show.report, "Report Viewer");
                    if ui.add(Button::new("Node Editor").shortcut_text(shortcut_for(Action::NodeEditor))).clicked() {
                        app.show.node_editor ^= true;
                    }
                });
                ui.menu_button("Views", |ui| views_menu(app, ui, ctx));
                ui.menu_button("Help", |ui| {
                    if ui.button("Keyboard Shortcuts").clicked() {
                        app.show.shortcuts = true;
                    }
                    if ui.button("Clean-Room Notice").clicked() {
                        app.show.clean_room = true;
                    }
                    if ui.button("Listener Commands").clicked() {
                        app.tab = PanelTab::Utilities;
                        app.utility = Some(Utility::Listener);
                    }
                    ui.separator();
                    if ui.button("About Vernon").clicked() {
                        app.show.about = true;
                    }
                });
            });
        });
}

fn file_menu(app: &mut VernonApp, ui: &mut Ui) {
    if item(ui, "New", Some(Action::New)) {
        app.file_new();
    }
    if item(ui, "Open…", Some(Action::Open)) {
        app.open_dialog(FileMode::Open);
    }
    if item(ui, "Save", Some(Action::Save)) {
        app.file_save();
    }
    if item(ui, "Save As…", None) {
        app.open_dialog(FileMode::SaveAs);
    }
    ui.separator();
    if item(ui, "Import Mesh…", None) {
        app.open_dialog(FileMode::Import);
    }
    if ui
        .add_enabled(app.st.primary().is_some(), Button::new("Export Selected…"))
        .clicked()
    {
        app.open_dialog(FileMode::Export);
    }
    ui.separator();
    if item(ui, "Exit", None) {
        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
    }
}

fn edit_menu(app: &mut VernonApp, ui: &mut Ui, ctx: &egui::Context) {
    if ui
        .add_enabled(app.st.can_undo(), Button::new("Undo").shortcut_text(shortcut_for(Action::Undo)))
        .clicked()
    {
        app.do_action(Action::Undo, ctx);
    }
    if ui
        .add_enabled(app.st.can_redo(), Button::new("Redo").shortcut_text(shortcut_for(Action::Redo)))
        .clicked()
    {
        app.do_action(Action::Redo, ctx);
    }
    if item(ui, "Undo View Change", Some(Action::ViewUndo)) {
        app.do_action(Action::ViewUndo, ctx);
    }
    ui.separator();
    if ui
        .add_enabled(!app.st.selection.is_empty(), Button::new("Delete").shortcut_text(shortcut_for(Action::Delete)))
        .clicked()
    {
        app.do_action(Action::Delete, ctx);
    }
    ui.separator();
    for (text, a) in [
        ("Select All", Action::SelectAll),
        ("Select None", Action::SelectNone),
        ("Select Invert", Action::SelectInvert),
        ("Hide Selection", Action::HideSelected),
        ("Unhide All", Action::UnhideAll),
    ] {
        if item(ui, text, Some(a)) {
            app.do_action(a, ctx);
        }
    }
}

fn views_menu(app: &mut VernonApp, ui: &mut Ui, ctx: &egui::Context) {
    let max = app.vp.layout.maximized;
    if item(ui, if max { "Restore Quad Layout" } else { "Maximise Active Viewport" }, Some(Action::Maximize)) {
        app.do_action(Action::Maximize, ctx);
    }
    for (text, a) in [
        ("Zoom Extents Selected", Action::ZoomExtentsSelected),
        ("Zoom Extents", Action::ZoomExtentsAll),
        ("Zoom Extents All Viewports", Action::ZoomExtentsAllViews),
        ("Wireframe / Shaded", Action::ToggleWire),
        ("Edged Faces", Action::ToggleEdges),
        ("Home Grid", Action::ToggleGrid),
    ] {
        if item(ui, text, Some(a)) {
            app.do_action(a, ctx);
        }
    }
    ui.separator();
    ui.label(RichText::new("Orbit mode").small().color(theme::TEXT_DIM));
    ui.radio_value(&mut app.vp.orbit, OrbitMode::Turntable, "Turntable (keeps Z up)");
    ui.radio_value(&mut app.vp.orbit, OrbitMode::Arcball, "Trackball");
    ui.separator();
    ui.checkbox(&mut app.ribbon_open, "Show Ribbon");
    if ui.button("Reset Viewport Layout").clicked() {
        app.vp.layout.reset();
    }
}

pub fn toolbar(app: &mut VernonApp, ctx: &egui::Context) {
    egui::TopBottomPanel::top("main_toolbar")
        .frame(egui::Frame::NONE.fill(theme::SURFACE).inner_margin(egui::Margin::symmetric(6, 3)))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                if ui
                    .add_enabled_ui(app.st.can_undo(), |ui| icons::button(ui, Icon::Undo, false, "Undo (Ctrl+Z)"))
                    .inner
                    .clicked()
                {
                    app.do_action(Action::Undo, ctx);
                }
                if ui
                    .add_enabled_ui(app.st.can_redo(), |ui| icons::button(ui, Icon::Redo, false, "Redo (Ctrl+Y)"))
                    .inner
                    .clicked()
                {
                    app.do_action(Action::Redo, ctx);
                }
                ui.separator();
                for (tool, icon, tip) in [
                    (Tool::Select, Icon::Select, "Select (Q)"),
                    (Tool::Move, Icon::Move, "Select and Move (W)"),
                    (Tool::Rotate, Icon::Rotate, "Select and Rotate (E)"),
                    (Tool::Scale, Icon::Scale, "Select and Scale (R)"),
                ] {
                    if icons::button(ui, icon, app.tool == tool, tip).clicked() {
                        app.tool = tool;
                    }
                }
                ui.separator();
                if icons::button(ui, Icon::Snap, app.snap, "Grid snap (S)").clicked() {
                    app.snap ^= true;
                }
                if icons::button(ui, Icon::AngleSnap, app.angle_snap, "Angle snap, 5° (A)").clicked() {
                    app.angle_snap ^= true;
                }
                ui.label(RichText::new("mm").monospace().color(theme::TEXT_DIM))
                    .on_hover_text("Scene unit: millimetres (Z up)");
                ui.separator();
                if icons::button(ui, Icon::NodeEditor, app.show.node_editor, "Node Editor (M)").clicked() {
                    app.show.node_editor ^= true;
                }
                if icons::button(ui, Icon::Gates, app.show.gates, "Gate Dashboard").clicked() {
                    app.show.gates ^= true;
                }
                if icons::button(ui, Icon::Report, app.show.report, "Report Viewer").clicked() {
                    app.show.report ^= true;
                }
                ui.separator();
                let gates = crate::gate::stage_gates(&app.st);
                let overall = crate::gate::overall(&gates);
                let col = match overall {
                    crate::gate::GateState::Pass => theme::PASS,
                    crate::gate::GateState::Fail | crate::gate::GateState::Error => theme::FAIL,
                    _ => theme::TEXT_DIM,
                };
                if ui
                    .add(Button::new(
                        RichText::new(format!("{} Pipeline {}", overall.glyph(), overall.text())).color(col),
                    ))
                    .on_hover_text("Worst gate over all stages; click for the dashboard")
                    .clicked()
                {
                    app.show.gates = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if app.ribbon_open { "▴ Ribbon" } else { "▾ Ribbon" };
                    if ui.small_button(label).clicked() {
                        app.ribbon_open ^= true;
                    }
                });
            });
        });
}

pub fn ribbon(app: &mut VernonApp, ctx: &egui::Context) {
    egui::TopBottomPanel::top("ribbon")
        .frame(
            egui::Frame::NONE
                .fill(Color32::from_rgb(0x2C, 0x2C, 0x2C))
                .inner_margin(egui::Margin::symmetric(6, 3))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(0x1A, 0x1A, 0x1A))),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for stage in Stage::ALL {
                    let on = app.ribbon_stage == stage;
                    let text = RichText::new(stage.label()).size(12.0);
                    if ui.selectable_label(on, text).clicked() {
                        if on {
                            app.ribbon_open ^= true;
                        } else {
                            app.ribbon_stage = stage;
                            app.ribbon_open = true;
                        }
                    }
                }
            });
            if !app.ribbon_open {
                return;
            }
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                let stage = app.ribbon_stage;
                let types: Vec<_> = app.st.registry.by_stage(stage).into_iter().cloned().collect();
                for info in types {
                    let mut text = RichText::new(&info.label);
                    if info.whitebox {
                        text = text.italics();
                    }
                    let mut tip = info.help.clone();
                    if info.whitebox {
                        tip.push_str("\n\nWhite-box: correct ports and parameters, placeholder algorithm.");
                    }
                    let takes_input = !info.inputs.is_empty();
                    tip.push_str(if takes_input {
                        "\n\nAdds on top of the selected object's stack."
                    } else {
                        "\n\nCreates a new object."
                    });
                    if ui.add(Button::new(text).min_size(egui::vec2(84.0, 26.0))).on_hover_text(tip).clicked() {
                        app.create(&info.id);
                        if takes_input {
                            app.tab = PanelTab::Modify;
                        }
                    }
                }
            });
        });
}

//! Command panel: Create, Modify, Hierarchy, Motion, Display, Utilities.

use eframe::egui::{self, vec2, Color32, RichText, Stroke, Ui};
use vernon_graph::{NodeId, Stage};

use super::icons::{self, Icon};
use super::{params, PanelTab, Utility, VernonApp};
use crate::gate::{metric_state, report_state, GateState};
use crate::theme;
use crate::timeline::{PlayMode, TimeFormat};
use crate::viewport::{ColorBy, OrbitMode, Shading};

pub fn show(app: &mut VernonApp, ctx: &egui::Context) {
    egui::SidePanel::right("command_panel")
        .resizable(true)
        .default_width(300.0)
        .width_range(250.0..=560.0)
        .show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                for tab in PanelTab::ALL {
                    if ui.selectable_label(app.tab == tab, RichText::new(tab.label()).size(12.0)).clicked() {
                        app.tab = tab;
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match app.tab {
                PanelTab::Create => create_tab(app, ui),
                PanelTab::Modify => modify_tab(app, ui),
                PanelTab::Hierarchy => hierarchy_tab(app, ui),
                PanelTab::Motion => motion_tab(app, ui),
                PanelTab::Display => display_tab(app, ui),
                PanelTab::Utilities => utilities_tab(app, ui),
            });
        });
}

fn rollout(ui: &mut Ui, title: &str, open: bool, body: impl FnOnce(&mut Ui)) {
    egui::CollapsingHeader::new(RichText::new(title).strong())
        .default_open(open)
        .show(ui, body);
}

fn name_and_color(app: &mut VernonApp, ui: &mut Ui, id: NodeId) {
    if app.name_buf.0 != Some(id) {
        app.name_buf = (Some(id), app.st.name(id).to_string());
    }
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut app.name_buf.1).desired_width(ui.available_width() - 34.0));
        if r.lost_focus() && app.name_buf.1 != app.st.name(id) {
            let name = app.name_buf.1.clone();
            if let Err(e) = app.st.rename(id, &name, app.now) {
                app.st.error_text(e);
                app.name_buf.1 = app.st.name(id).to_string();
            }
        }
        let mut c = app.st.color_of(id);
        if ui.color_edit_button_srgb(&mut c).changed() {
            app.st.set_color(id, c);
        }
    });
}

fn whitebox_badge(ui: &mut Ui) {
    let (r, resp) = ui.allocate_exact_size(vec2(78.0, 18.0), egui::Sense::hover());
    let p = ui.painter();
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    p.extend(egui::Shape::dashed_line(&pts, Stroke::new(1.0_f32, theme::WARN), 4.0, 3.0));
    p.text(r.center(), egui::Align2::CENTER_CENTER, "WHITE-BOX", egui::FontId::proportional(10.0), theme::WARN);
    resp.on_hover_text("Correct ports and parameters; the algorithm is a placeholder until the real one lands.");
}

fn create_tab(app: &mut VernonApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label("Stage");
        egui::ComboBox::from_id_salt("create_stage")
            .selected_text(app.create_stage.label())
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for s in Stage::ALL {
                    ui.selectable_value(&mut app.create_stage, s, s.label());
                }
            });
    });
    let types: Vec<_> = app.st.registry.by_stage(app.create_stage).into_iter().cloned().collect();
    rollout(ui, "Object Type", true, |ui| {
        egui::Grid::new("object_types").num_columns(2).spacing([4.0, 4.0]).show(ui, |ui| {
            let w = (ui.available_width() - 6.0).max(120.0) / 2.0;
            for (k, info) in types.iter().enumerate() {
                let mut text = RichText::new(&info.label);
                if info.whitebox {
                    text = text.italics();
                }
                let tip = if info.inputs.is_empty() {
                    format!("{}\n\nCreates a new object.", info.help)
                } else {
                    format!("{}\n\nAdds on top of the selected object.", info.help)
                };
                if ui.add(egui::Button::new(text).min_size(vec2(w, 24.0))).on_hover_text(tip).clicked() {
                    app.create(&info.id);
                }
                if k % 2 == 1 {
                    ui.end_row();
                }
            }
        });
    });
    if let Some(p) = app.st.primary() {
        rollout(ui, "Name and Colour", true, |ui| name_and_color(app, ui, p));
        rollout(ui, "Parameters", true, |ui| params::rollout(app, ui, p));
    }
}

fn modify_tab(app: &mut VernonApp, ui: &mut Ui) {
    let Some(primary) = app.st.primary() else {
        ui.label(RichText::new("Select an object to see its stack.").color(theme::TEXT_DIM));
        return;
    };
    name_and_color(app, ui, primary);
    let entry = app.st.current_entry().unwrap_or(primary);

    // Modifier List: types whose first input takes the entry's output.
    let mut choices: Vec<(Stage, String, String)> = app
        .st
        .registry
        .types()
        .filter(|i| {
            i.inputs
                .first()
                .is_some_and(|p| app.st.compatible_output(entry, p.kind).is_some())
        })
        .map(|i| (i.stage, i.id.clone(), i.label.clone()))
        .collect();
    choices.sort();
    let mut pick = None;
    egui::ComboBox::from_id_salt("modifier_list")
        .selected_text("Modifier List")
        .width(ui.available_width())
        .height(420.0)
        .show_ui(ui, |ui| {
            let mut last = None;
            for (stage, id, label) in &choices {
                if last != Some(*stage) {
                    ui.label(RichText::new(stage.label()).small().color(theme::ACCENT));
                    last = Some(*stage);
                }
                if ui.selectable_label(false, format!("   {label}")).clicked() {
                    pick = Some(id.clone());
                }
            }
        });
    if let Some(id) = pick {
        app.add_modifier(&id);
    }

    // The stack, top entry first.
    let stack = app.st.graph.stack(primary);
    egui::Frame::new()
        .fill(Color32::from_rgb(0x1E, 0x1E, 0x1E))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(0x44, 0x44, 0x44)))
        .inner_margin(4)
        .show(ui, |ui| {
            ui.set_min_height(90.0);
            ui.set_width(ui.available_width());
            for (k, n) in stack.iter().enumerate().rev() {
                let info = app.st.info(*n).cloned();
                let node = app.st.graph.nodes.get(n).cloned();
                let (Some(info), Some(node)) = (info, node) else { continue };
                ui.horizontal(|ui| {
                    let base = k == 0 || info.inputs.is_empty();
                    let icon = if node.bypass { Icon::BulbOff } else { Icon::Bulb };
                    let tip = if base { "Base object (no input to pass through)" } else { "On / off (bypass)" };
                    if ui
                        .add_enabled_ui(!base, |ui| icons::button_sized(ui, icon, false, tip, 18.0))
                        .inner
                        .clicked()
                    {
                        app.st.toggle_bypass(*n);
                    }
                    let mut text = RichText::new(format!("{}  ({})", info.label, node.name));
                    if node.bypass {
                        text = text.color(theme::DISABLED);
                    } else if app.st.error_of(*n).is_some() {
                        text = text.color(theme::FAIL);
                    }
                    if k == 0 {
                        text = text.strong();
                    }
                    let extra = info.inputs.len().saturating_sub(1);
                    let r = ui.selectable_label(entry == *n, text);
                    if r.clicked() {
                        app.st.stack_entry = Some(*n);
                    }
                    if extra > 0 {
                        ui.label(RichText::new(format!("+{extra} in")).small().color(theme::TEXT_DIM))
                            .on_hover_text("Extra inputs are wired in the Node Editor");
                    }
                    if info.whitebox {
                        ui.label(RichText::new("WB").small().color(theme::WARN))
                            .on_hover_text("White-box node");
                    }
                });
            }
        });
    ui.horizontal(|ui| {
        ui.toggle_value(&mut app.st.show_end_result, "Show end result")
            .on_hover_text("Off: the viewport shows the object only up to the highlighted entry");
        if ui.button("Remove").on_hover_text("Delete the highlighted entry and reconnect around it").clicked() {
            app.st.delete(&[entry]);
        }
        if ui.button("Reset").on_hover_text("Reset the highlighted entry's parameters").clicked() {
            app.st.reset_params(entry);
        }
    });
    ui.separator();

    let Some(info) = app.st.info(entry).cloned() else { return };
    rollout(ui, &format!("{} parameters", info.label), true, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(info.stage.label()).small().color(theme::ACCENT));
            ui.label(RichText::new(&info.id).small().monospace().color(theme::TEXT_DIM));
            if info.whitebox {
                whitebox_badge(ui);
            }
            if let Some(tool) = &info.external_tool {
                ui.label(RichText::new(format!("runs {tool}")).small().color(theme::TEXT_DIM));
            }
        });
        if !info.help.is_empty() {
            ui.label(RichText::new(&info.help).small().color(theme::TEXT_DIM));
        }
        if let Some(e) = app.st.error_of(entry) {
            ui.label(RichText::new(format!("✖ {e}")).color(theme::FAIL));
        }
        ui.add_space(4.0);
        params::rollout(app, ui, entry);
    });
    if let Some(r) = app.st.report(entry) {
        rollout(ui, &format!("Report: {}", r.title), true, |ui| report_grid(ui, &r));
        if ui.button("Open in Report Viewer").clicked() {
            app.report_node = Some(entry);
            app.show.report = true;
        }
    }
}

/// Metrics table shared with the report viewer.
pub fn report_grid(ui: &mut Ui, r: &vernon_core::Report) {
    egui::Grid::new(("report_grid", r.title.as_str())).num_columns(4).striped(true).show(ui, |ui| {
        for (name, m) in &r.metrics {
            ui.label(name);
            ui.label(RichText::new(format!("{:.4} {}", m.value, m.unit)).monospace());
            match m.limit {
                Some(l) => ui.label(
                    RichText::new(format!("{} {l:.4}", if m.lower_is_better { "≤" } else { "≥" }))
                        .small()
                        .color(theme::TEXT_DIM),
                ),
                None => ui.label(""),
            };
            let s = metric_state(m);
            ui.label(RichText::new(format!("{} {}", s.glyph(), s.text())).color(state_color(s)));
            ui.end_row();
        }
    });
    for (sev, text) in &r.notes {
        let col = match sev {
            vernon_core::Severity::Error => theme::FAIL,
            vernon_core::Severity::Warning => theme::WARN,
            vernon_core::Severity::Info => theme::TEXT_DIM,
        };
        ui.label(RichText::new(format!("{sev:?}: {text}")).small().color(col));
    }
}

pub fn state_color(s: GateState) -> Color32 {
    match s {
        GateState::Pass => theme::PASS,
        GateState::Fail | GateState::Error => theme::FAIL,
        _ => theme::TEXT_DIM,
    }
}

fn upstream_tree(app: &mut VernonApp, ui: &mut Ui, id: NodeId, depth: usize) {
    let inputs: Vec<(usize, NodeId)> = app
        .st
        .graph
        .nodes
        .get(&id)
        .map(|n| n.inputs.iter().enumerate().filter_map(|(i, w)| w.map(|(s, _)| (i, s))).collect())
        .unwrap_or_default();
    let label = format!(
        "{}  ·  {}",
        app.st.name(id),
        app.st.info(id).map(|i| i.label.as_str()).unwrap_or("?")
    );
    if inputs.is_empty() || depth > 32 {
        if ui.selectable_label(app.st.current_entry() == Some(id), label).clicked() {
            app.st.stack_entry = Some(id);
        }
        return;
    }
    egui::CollapsingHeader::new(label)
        .id_salt(("tree", id, depth))
        .default_open(depth < 3)
        .show(ui, |ui| {
            for (slot, src) in inputs {
                ui.label(RichText::new(format!("input {slot}")).small().color(theme::TEXT_DIM));
                upstream_tree(app, ui, src, depth + 1);
            }
        });
}

fn hierarchy_tab(app: &mut VernonApp, ui: &mut Ui) {
    let Some(p) = app.st.primary() else {
        ui.label(RichText::new("Select an object to see what feeds it.").color(theme::TEXT_DIM));
        return;
    };
    rollout(ui, "Pipeline tree (upstream)", true, |ui| upstream_tree(app, ui, p, 0));
    rollout(ui, "Parts", true, |ui| {
        let geo = app.st.geometry(app.st.display_node(p));
        match geo.as_ref().and_then(|g| g.prim_attribs.ints("part").ok().map(|v| v.to_vec())) {
            Some(parts) => {
                let mut counts = std::collections::BTreeMap::new();
                for v in parts {
                    *counts.entry(v).or_insert(0usize) += 1;
                }
                for (part, n) in counts {
                    ui.label(format!("part {part}: {n} faces"));
                }
            }
            None => {
                ui.label(RichText::new("No 'part' attribute yet: split nodes write it.").color(theme::TEXT_DIM));
            }
        }
    });
}

fn motion_tab(app: &mut VernonApp, ui: &mut Ui) {
    ui.label(
        RichText::new("The time slider drives study sweeps: each frame is one sample (speed, yaw, load case…).")
            .small()
            .color(theme::TEXT_DIM),
    );
    rollout(ui, "Time configuration", true, |ui| {
        let t = &mut app.time;
        egui::Grid::new("time_cfg").num_columns(2).show(ui, |ui| {
            ui.label("Start");
            let end = t.end;
            ui.add(egui::DragValue::new(&mut t.start).range(-100_000..=end - 1));
            ui.end_row();
            ui.label("End");
            let start = t.start;
            ui.add(egui::DragValue::new(&mut t.end).range(start + 1..=1_000_000));
            ui.end_row();
            ui.label("Frame rate");
            egui::ComboBox::from_id_salt("fps")
                .selected_text(format!("{} fps", t.fps))
                .show_ui(ui, |ui| {
                    for f in [24, 25, 30, 60] {
                        ui.selectable_value(&mut t.fps, f, format!("{f} fps"));
                    }
                });
            ui.end_row();
            ui.label("Speed");
            egui::ComboBox::from_id_salt("speed")
                .selected_text(format!("{}x", t.speed))
                .show_ui(ui, |ui| {
                    for s in [0.25, 0.5, 1.0, 2.0, 4.0] {
                        ui.selectable_value(&mut t.speed, s, format!("{s}x"));
                    }
                });
            ui.end_row();
            ui.label("Playback");
            ui.vertical(|ui| {
                for m in PlayMode::ALL {
                    ui.radio_value(&mut t.mode, m, m.label());
                }
            });
            ui.end_row();
            ui.label("Time display");
            ui.vertical(|ui| {
                for f in TimeFormat::ALL {
                    ui.radio_value(&mut t.format, f, f.label());
                }
            });
            ui.end_row();
        });
        ui.label(
            RichText::new(format!(
                "{} frames, {:.3} s",
                t.frame_count(),
                t.seconds(t.length())
            ))
            .small()
            .color(theme::TEXT_DIM),
        );
    });
    let f = app.time.clamp(app.play.frame);
    if f != app.play.frame {
        app.play.set_frame(&app.time, f, app.now);
    }
}

fn display_tab(app: &mut VernonApp, ui: &mut Ui) {
    let active = app.vp.layout.active;
    rollout(ui, "Active viewport", true, |ui| {
        let t = &mut app.vp.tiles[active];
        ui.label(RichText::new(format!("{} viewport", t.kind.label())).color(theme::TEXT_DIM));
        ui.horizontal(|ui| {
            ui.radio_value(&mut t.shading, Shading::Shaded, "Shaded");
            ui.radio_value(&mut t.shading, Shading::Wireframe, "Wireframe");
        });
        ui.checkbox(&mut t.edged, "Edged faces (F4)");
        ui.checkbox(&mut app.vp.show_grid, "Home grid (G)");
        ui.checkbox(&mut app.vp.show_gizmo, "Orientation gizmo");
    });
    rollout(ui, "Colour by", true, |ui| {
        for c in ColorBy::ALL {
            ui.radio_value(&mut app.color_by, c, c.label());
        }
        ui.label(RichText::new("Objects without the attribute keep their object colour.").small().color(theme::TEXT_DIM));
    });
    rollout(ui, "Hide", true, |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Hide Selected").clicked() {
                app.hide_selected();
            }
            if ui.button("Hide Unselected").clicked() {
                for o in app.st.objects() {
                    if !app.st.selection.contains(&o) {
                        app.st.set_hidden(o, true);
                    }
                }
            }
            if ui.button("Unhide All").clicked() {
                app.unhide_all();
            }
        });
    });
    rollout(ui, "Home grid", false, |ui| {
        let g = &mut app.vp.grid;
        egui::Grid::new("grid_cfg").num_columns(2).show(ui, |ui| {
            ui.label("Spacing");
            ui.add(egui::DragValue::new(&mut g.spacing).range(0.001..=1e6).suffix(" mm").speed(0.5));
            ui.end_row();
            ui.label("Major every");
            ui.add(egui::DragValue::new(&mut g.major_every).range(2..=100));
            ui.end_row();
            ui.label("Perspective extent");
            ui.add(egui::DragValue::new(&mut g.extent).range(1..=20));
            ui.end_row();
        });
        ui.checkbox(&mut g.adaptive, "Adaptive spacing");
        ui.checkbox(&mut g.inhibit_subdivision, "Stop subdividing below spacing");
    });
    rollout(ui, "Navigation", false, |ui| {
        ui.radio_value(&mut app.vp.orbit, OrbitMode::Turntable, "Turntable orbit (Z stays up)");
        ui.radio_value(&mut app.vp.orbit, OrbitMode::Arcball, "Trackball orbit");
    });
}

fn utilities_tab(app: &mut VernonApp, ui: &mut Ui) {
    egui::Grid::new("utility_buttons").num_columns(2).spacing([4.0, 4.0]).show(ui, |ui| {
        let w = (ui.available_width() - 6.0).max(120.0) / 2.0;
        for (k, (u, label)) in [
            (Utility::Measure, "Measure"),
            (Utility::Reports, "Reports"),
            (Utility::Statistics, "Scene Statistics"),
            (Utility::Listener, "Listener"),
            (Utility::CleanRoom, "Clean-Room Notice"),
        ]
        .into_iter()
        .enumerate()
        {
            let on = app.utility == Some(u);
            if ui.add(egui::Button::new(label).selected(on).min_size(vec2(w, 24.0))).clicked() {
                app.utility = if on { None } else { Some(u) };
            }
            if k % 2 == 1 {
                ui.end_row();
            }
        }
    });
    ui.separator();
    match app.utility {
        None => {
            ui.label(RichText::new("Pick a utility above.").color(theme::TEXT_DIM));
        }
        Some(Utility::Measure) => measure(app, ui),
        Some(Utility::Reports) => {
            let nodes = app.st.report_nodes();
            if nodes.is_empty() {
                ui.label(RichText::new("No reports yet.").color(theme::TEXT_DIM));
            }
            for n in nodes {
                let Some(r) = app.st.report(n) else { continue };
                let s = report_state(&r);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} {}", s.glyph(), s.text())).color(state_color(s)));
                    if ui.link(format!("{}: {}", app.st.name(n), r.title)).clicked() {
                        app.report_node = Some(n);
                        app.show.report = true;
                    }
                });
            }
        }
        Some(Utility::Statistics) => {
            let objects = app.st.objects();
            let mut tris = 0;
            let mut pts = 0;
            for o in &objects {
                if let Some(g) = app.st.geometry(app.st.display_node(*o)) {
                    tris += g.triangles().len();
                    pts += g.point_count();
                }
            }
            egui::Grid::new("stats").num_columns(2).show(ui, |ui| {
                for (k, v) in [
                    ("Objects", objects.len().to_string()),
                    ("Nodes", app.st.graph.nodes.len().to_string()),
                    ("Points", pts.to_string()),
                    ("Triangles", tris.to_string()),
                    ("Undo steps", if app.st.can_undo() { "yes".into() } else { "none".into() }),
                    ("Viewport renderer", app.backend.name().to_string()),
                ] {
                    ui.label(k);
                    ui.label(RichText::new(v).monospace());
                    ui.end_row();
                }
            });
        }
        Some(Utility::Listener) => {
            ui.label(RichText::new(crate::listener::HELP).small().color(theme::TEXT_DIM));
            egui::ScrollArea::vertical().max_height(300.0).stick_to_bottom(true).show(ui, |ui| {
                for l in &app.st.log {
                    let col = if l.error { theme::FAIL } else { theme::TEXT };
                    ui.label(RichText::new(&l.text).monospace().size(11.0).color(col));
                }
            });
            let r = ui.add(
                egui::TextEdit::singleline(&mut app.listener_line)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace)
                    .hint_text("type a command and press Enter"),
            );
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let line = std::mem::take(&mut app.listener_line);
                app.run_command(&line);
                r.request_focus();
            }
        }
        Some(Utility::CleanRoom) => {
            ui.label(super::windows::CLEAN_ROOM_TEXT);
        }
    }
}

fn measure(app: &mut VernonApp, ui: &mut Ui) {
    let sel = app.st.selection.clone();
    if sel.is_empty() {
        ui.label(RichText::new("Select one object for its size, two for the distance between them.").color(theme::TEXT_DIM));
        return;
    }
    let geos: Vec<_> = sel
        .iter()
        .filter_map(|id| app.st.geometry(app.st.display_node(*id)).map(|g| (*id, g)))
        .collect();
    for (id, g) in &geos {
        let b = g.bounds();
        let s = b.size();
        rollout(ui, app.st.name(*id), true, |ui| {
            egui::Grid::new(("measure", *id)).num_columns(2).show(ui, |ui| {
                for (k, v) in [
                    ("Size X", format!("{:.3} mm", s.x)),
                    ("Size Y", format!("{:.3} mm", s.y)),
                    ("Size Z", format!("{:.3} mm", s.z)),
                    ("Centre", format!("{:.2}, {:.2}, {:.2}", b.center().x, b.center().y, b.center().z)),
                    ("Surface area", format!("{:.1} mm²", g.surface_area())),
                    ("Points", g.point_count().to_string()),
                    ("Primitives", g.prim_count().to_string()),
                ] {
                    ui.label(k);
                    ui.label(RichText::new(v).monospace());
                    ui.end_row();
                }
            });
        });
    }
    if geos.len() >= 2 {
        let a = geos[0].1.bounds().center();
        let b = geos[1].1.bounds().center();
        ui.label(format!("Centre distance: {:.3} mm", (a - b).length()));
    }
    if geos.is_empty() {
        ui.label(RichText::new("The selection has no geometry.").color(theme::TEXT_DIM));
    }
}

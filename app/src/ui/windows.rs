//! Floating windows: Gate Dashboard, Report Viewer, About, Clean-Room
//! notice, Shortcuts and the in-app file dialog.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use vernon_graph::NodeId;

use super::panel::{report_grid, state_color};
use super::VernonApp;
use crate::gate::{self, metric_state};
use crate::hotkeys;
use crate::theme;
use crate::unsaved::Choice;

pub const CLEAN_ROOM_TEXT: &str = "Vernon is a clean-room, open-source project. Its features are re-implemented from \
public documentation, published papers and observed behaviour only. No proprietary code, binaries, icons, \
artwork or help text are used, and the interface never names other products. Interface conventions follow \
generic workspace practice; all icons are drawn in-house.";

#[derive(Debug, Clone, PartialEq)]
pub enum FileMode {
    Open,
    SaveAs,
    Import,
    Export,
    /// Fill a path parameter of a node.
    Param { node: NodeId, param: String },
}

pub struct FileDialog {
    pub mode: FileMode,
    pub dir: PathBuf,
    pub name: String,
    pub extensions: Vec<String>,
    pub save: bool,
    pub dir_text: String,
    /// Existing file a save would replace, waiting for confirmation.
    pub overwrite: Option<PathBuf>,
}

/// The file a confirmed dialog writes: Save As adds `.vrn` to a bare name.
pub fn save_target(mode: &FileMode, path: &Path) -> PathBuf {
    if *mode == FileMode::SaveAs && path.extension().is_none() {
        path.with_extension("vrn")
    } else {
        path.to_path_buf()
    }
}

/// True when a save to `target` must ask before replacing an existing file.
pub fn needs_overwrite_confirm(save: bool, target: &Path, confirmed: Option<&Path>) -> bool {
    save && target.exists() && confirmed != Some(target)
}

impl FileDialog {
    pub fn new(mode: FileMode, dir: PathBuf) -> Self {
        let (extensions, save): (Vec<String>, bool) = match &mode {
            FileMode::Open => (vec!["vrn".into()], false),
            FileMode::SaveAs => (vec!["vrn".into()], true),
            FileMode::Import => (vernon_io::Format::extensions().iter().map(|s| s.to_string()).collect(), false),
            FileMode::Export => (vec!["stl".into(), "obj".into(), "ply".into(), "3mf".into()], true),
            FileMode::Param { .. } => (vec![], false),
        };
        let dir_text = dir.display().to_string();
        Self {
            mode,
            dir,
            name: String::new(),
            extensions,
            save,
            dir_text,
            overwrite: None,
        }
    }

    fn title(&self) -> &'static str {
        match self.mode {
            FileMode::Open => "Open Scene",
            FileMode::SaveAs => "Save Scene As",
            FileMode::Import => "Import Mesh",
            FileMode::Export => "Export Selected",
            FileMode::Param { .. } => "Choose File",
        }
    }
}

/// Directory entries: folders first, then files matching `extensions`
/// (all files when empty), each sorted case-insensitively.
pub fn list_dir(dir: &Path, extensions: &[String]) -> Vec<(String, bool)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            dirs.push(name);
        } else {
            let ext = Path::new(&name)
                .extension()
                .map(|x| x.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if extensions.is_empty() || extensions.iter().any(|x| x.eq_ignore_ascii_case(&ext)) {
                files.push(name);
            }
        }
    }
    dirs.sort_by_key(|s| s.to_lowercase());
    files.sort_by_key(|s| s.to_lowercase());
    dirs.into_iter().map(|d| (d, true)).chain(files.into_iter().map(|f| (f, false))).collect()
}

pub fn show(app: &mut VernonApp, ctx: &egui::Context) {
    gate_dashboard(app, ctx);
    report_viewer(app, ctx);
    file_dialog(app, ctx);
    unsaved_prompt(app, ctx);
    trust_prompt(app, ctx);
    egui::Window::new("About Vernon")
        .open(&mut app.show.about)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.heading(RichText::new("Vernon").color(theme::ACCENT));
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.label("An independent, open-source, node-based 3D toolchain for 3D-printed body kits: scan, clean, surface, split and print, airflow, structure, solar and flattening, performance.");
            ui.label("Licensed under the Apache License 2.0.");
            ui.separator();
            ui.label(RichText::new("Third-party: egui and eframe (MIT or Apache-2.0), wgpu (MIT or Apache-2.0), glam (MIT or Apache-2.0), serde (MIT or Apache-2.0); bundled fonts under OFL-1.1 and the Ubuntu Font Licence.").small().color(theme::TEXT_DIM));
        });
    egui::Window::new("Clean-Room Notice")
        .open(&mut app.show.clean_room)
        .collapsible(false)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.label(CLEAN_ROOM_TEXT);
        });
    egui::Window::new("Keyboard Shortcuts")
        .open(&mut app.show.shortcuts)
        .default_width(360.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                egui::Grid::new("keys").striped(true).num_columns(2).show(ui, |ui| {
                    for (c, _, text) in hotkeys::BINDINGS {
                        ui.label(RichText::new(hotkeys::chord_text(*c)).monospace().color(theme::ACCENT));
                        ui.label(*text);
                        ui.end_row();
                    }
                    for (c, text) in [
                        ("Middle drag", "Pan"),
                        ("Alt + middle drag", "Orbit"),
                        ("Wheel", "Zoom about the cursor"),
                        ("Right click", "Viewport menu"),
                    ] {
                        ui.label(RichText::new(c).monospace().color(theme::ACCENT));
                        ui.label(text);
                        ui.end_row();
                    }
                });
            });
        });
}

/// Ask before an opened scene may start programs or write files at paths
/// it chose; lists exactly what it would do.
fn trust_prompt(app: &mut VernonApp, ctx: &egui::Context) {
    if !app.st.trust_prompt || app.st.trusted {
        app.st.trust_prompt = false;
        return;
    }
    let findings = app.st.trust_findings();
    let mut open = true;
    let mut choice = None;
    egui::Window::new("Trust This Scene?")
        .open(&mut open)
        .collapsible(false)
        .default_width(480.0)
        .show(ctx, |ui| {
            ui.label("This scene came from a file. Until you trust it, Vernon will not start its external programs or uploads, and holds the nodes that write files at paths the file chose:");
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for line in &findings {
                    ui.label(RichText::new(line).monospace().small().color(theme::TEXT));
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Trust this scene").clicked() {
                    choice = Some(true);
                }
                if ui.button("Not now").clicked() {
                    choice = Some(false);
                }
            });
        });
    match choice {
        Some(true) => app.st.trust_scene(),
        Some(false) => app.st.trust_prompt = false,
        None if !open => app.st.trust_prompt = false,
        None => {}
    }
}

fn gate_dashboard(app: &mut VernonApp, ctx: &egui::Context) {
    let mut open = app.show.gates;
    egui::Window::new("Gate Dashboard")
        .open(&mut open)
        .default_size([620.0, 420.0])
        .show(ctx, |ui| {
            let gates = gate::stage_gates(&app.st);
            let overall = gate::overall(&gates);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Pipeline:").strong());
                ui.label(
                    RichText::new(format!("{} {}", overall.glyph(), overall.text()))
                        .strong()
                        .color(state_color(overall)),
                );
                ui.label(RichText::new("· worst gate over every report node of a stage; click a node to select it").small().color(theme::TEXT_DIM));
            });
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("gate_grid").striped(true).num_columns(6).show(ui, |ui| {
                    for h in ["Stage", "State", "Node", "Metric", "Value", "Limit / margin"] {
                        ui.label(RichText::new(h).strong().color(theme::TEXT_DIM));
                    }
                    ui.end_row();
                    for g in &gates {
                        if g.nodes.is_empty() {
                            ui.label(g.stage.label());
                            ui.label(RichText::new(format!("{} {}", g.state.glyph(), g.state.text())).color(state_color(g.state)));
                            ui.end_row();
                            continue;
                        }
                        for (i, ng) in g.nodes.iter().enumerate() {
                            // Stage name and rolled-up state on the first row only.
                            if i == 0 {
                                ui.label(g.stage.label());
                                ui.label(
                                    RichText::new(format!("{} {}", g.state.glyph(), g.state.text())).color(state_color(g.state)),
                                );
                            } else {
                                ui.label("");
                                ui.label("");
                            }
                            let node_text = RichText::new(format!("{} {}", ng.state.glyph(), app.st.name(ng.node)))
                                .color(state_color(ng.state));
                            if ui.link(node_text).clicked() {
                                let obj = app.st.object_of(ng.node);
                                app.st.select_only(obj);
                                app.st.stack_entry = Some(ng.node);
                                app.tab = super::PanelTab::Modify;
                            }
                            if let Some(e) = &ng.error {
                                ui.label(RichText::new(e).small().color(theme::FAIL_TEXT));
                                ui.end_row();
                                continue;
                            }
                            let mut first = true;
                            if let Some(r) = &ng.report {
                                for (name, m) in &r.metrics {
                                    if !first {
                                        ui.label("");
                                        ui.label("");
                                        ui.label("");
                                    }
                                    first = false;
                                    let s = metric_state(m);
                                    let excused = s == gate::GateState::Fail && ng.expected.contains(name);
                                    let label = if excused {
                                        let why = app.st.graph.nodes.get(&ng.node).and_then(|n| n.expect_fail.get(name));
                                        format!("{} {name} (expected: {})", s.glyph(), why.map_or("", |w| w.as_str()))
                                    } else {
                                        format!("{} {name}", s.glyph())
                                    };
                                    let col = if excused { theme::TEXT_DIM } else { state_color(s) };
                                    ui.label(RichText::new(label).color(col));
                                    ui.label(RichText::new(format!("{:.3} {}", m.value, m.unit)).monospace());
                                    match (m.limit, gate::margin(m)) {
                                        (Some(l), Some(mg)) => ui.label(
                                            RichText::new(format!(
                                                "{} {l:.3}  ({mg:+.3})",
                                                if m.lower_is_better { "\u{2264}" } else { "\u{2265}" }
                                            ))
                                            .monospace()
                                            .color(theme::TEXT_DIM),
                                        ),
                                        _ => ui.label(RichText::new("n/a").color(theme::TEXT_DIM)),
                                    };
                                    ui.end_row();
                                }
                            }
                            if first {
                                ui.end_row();
                            }
                        }
                    }
                });
            });
        });
    app.show.gates = open;
}

fn report_viewer(app: &mut VernonApp, ctx: &egui::Context) {
    let mut open = app.show.report;
    egui::Window::new("Report Viewer")
        .open(&mut open)
        .default_size([460.0, 380.0])
        .show(ctx, |ui| {
            let nodes = app.st.report_nodes();
            if app.report_node.is_none_or(|n| !nodes.contains(&n)) {
                app.report_node = nodes.last().copied();
            }
            let current = app.report_node;
            let label = current.map(|n| app.st.name(n).to_string()).unwrap_or_else(|| "(no reports)".into());
            egui::ComboBox::from_id_salt("report_pick").selected_text(label).show_ui(ui, |ui| {
                for n in &nodes {
                    let title = app.st.report(*n).map(|r| r.title.clone()).unwrap_or_default();
                    if ui
                        .selectable_label(current == Some(*n), format!("{}: {title}", app.st.name(*n)))
                        .clicked()
                    {
                        app.report_node = Some(*n);
                    }
                }
            });
            let Some(r) = current.and_then(|n| app.st.report(n)) else {
                ui.label(RichText::new("Analysis nodes emit reports; cook a pipeline to see them here.").color(theme::TEXT_DIM));
                return;
            };
            ui.separator();
            let s = gate::report_state(&r);
            ui.horizontal(|ui| {
                ui.heading(&r.title);
                ui.label(RichText::new(format!("{} {}", s.glyph(), s.text())).color(state_color(s)));
            });
            report_grid(ui, &r);
            ui.separator();
            if ui.button("Copy as JSON").clicked() {
                if let Ok(text) = serde_json::to_string_pretty(&*r) {
                    ui.ctx().copy_text(text);
                    app.st.info_text("report copied to the clipboard");
                }
            }
        });
    app.show.report = open;
}

fn file_dialog(app: &mut VernonApp, ctx: &egui::Context) {
    let Some(d) = app.file_dialog.as_mut() else { return };
    let mut open = true;
    let mut chosen: Option<PathBuf> = None;
    let mut cancel = false;
    egui::Window::new(d.title())
        .open(&mut open)
        .collapsible(false)
        .default_size([520.0, 400.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("⬆ Up").clicked() {
                    if let Some(p) = d.dir.parent() {
                        d.dir = p.to_path_buf();
                        d.dir_text = d.dir.display().to_string();
                    }
                }
                let r = ui.add(egui::TextEdit::singleline(&mut d.dir_text).desired_width(f32::INFINITY));
                if r.lost_focus() && Path::new(&d.dir_text).is_dir() {
                    d.dir = PathBuf::from(&d.dir_text);
                }
            });
            if !d.extensions.is_empty() {
                ui.label(
                    RichText::new(format!("Showing: {}", d.extensions.iter().map(|e| format!("*.{e}")).collect::<Vec<_>>().join(" ")))
                        .small()
                        .color(theme::TEXT_DIM),
                );
            }
            egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                for (name, is_dir) in list_dir(&d.dir, &d.extensions) {
                    let text = if is_dir { format!("▸ {name}/") } else { format!("   {name}") };
                    let r = ui.selectable_label(d.name == name && !is_dir, text);
                    if r.clicked() {
                        if is_dir {
                            d.dir = d.dir.join(&name);
                            d.dir_text = d.dir.display().to_string();
                        } else {
                            d.name = name.clone();
                        }
                    }
                    if r.double_clicked() && !is_dir {
                        chosen = Some(d.dir.join(&name));
                    }
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("File name");
                ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(260.0));
                let ok = if d.save { "Save" } else { "Open" };
                if ui.add_enabled(!d.name.trim().is_empty(), egui::Button::new(ok)).clicked() {
                    chosen = Some(d.dir.join(d.name.trim()));
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
            if let Some(p) = d.overwrite.clone() {
                ui.separator();
                ui.label(RichText::new(format!("{} already exists. Replace it?", p.display())).color(theme::WARN));
                ui.horizontal(|ui| {
                    if ui.button("Replace").clicked() {
                        chosen = Some(p.clone());
                    }
                    if ui.button("Keep").clicked() {
                        d.overwrite = None;
                    }
                });
            }
        });
    if let Some(path) = chosen {
        let mode = d.mode.clone();
        if !d.save && !path.exists() {
            app.st.error_text(format!("{} does not exist", path.display()));
            return;
        }
        let target = save_target(&mode, &path);
        if needs_overwrite_confirm(d.save, &target, d.overwrite.as_deref()) {
            d.overwrite = Some(target);
            return;
        }
        app.file_dialog = None;
        app.file_chosen(mode, path);
    } else if cancel || !open {
        let mode = d.mode.clone();
        app.file_dialog = None;
        app.file_dialog_cancelled(&mode);
    }
}

/// Save / Don't Save / Cancel before New, Open or closing a modified scene.
fn unsaved_prompt(app: &mut VernonApp, ctx: &egui::Context) {
    let Some(action) = app.guard.prompt else { return };
    let name = app
        .st
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".into());
    let mut choice = None;
    egui::Window::new("Unsaved Changes")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!("Save changes to {name} before {}?", action.verb()));
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    choice = Some(Choice::Save);
                }
                if ui.button("Don't Save").clicked() {
                    choice = Some(Choice::Discard);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(Choice::Cancel);
                }
            });
        });
    if let Some(c) = choice {
        app.answer_unsaved(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_dir_filters_and_orders() {
        let dir = std::env::temp_dir().join(format!("vernon_app_listdir_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for f in ["b.stl", "A.obj", "notes.txt"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let all = list_dir(&dir, &["stl".into(), "obj".into()]);
        assert_eq!(
            all,
            vec![("sub".to_string(), true), ("A.obj".to_string(), false), ("b.stl".to_string(), false)]
        );
        assert_eq!(list_dir(&dir, &[]).len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_over_an_existing_file_asks_first() {
        let dir = std::env::temp_dir().join(format!("vernon_app_overwrite_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let existing = dir.join("kit.vrn");
        std::fs::write(&existing, b"x").unwrap();
        // a bare Save As name gets the scene extension and hits the existing file
        let target = save_target(&FileMode::SaveAs, &dir.join("kit"));
        assert_eq!(target, existing);
        assert_eq!(save_target(&FileMode::Export, &dir.join("kit")), dir.join("kit"));
        assert!(needs_overwrite_confirm(true, &target, None));
        // confirmed for this file: no second prompt; another file asks again
        assert!(!needs_overwrite_confirm(true, &target, Some(existing.as_path())));
        assert!(needs_overwrite_confirm(true, &target, Some(dir.join("other.vrn").as_path())));
        // new files and open dialogs never ask
        assert!(!needs_overwrite_confirm(true, &dir.join("new.vrn"), None));
        assert!(!needs_overwrite_confirm(false, &target, None));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

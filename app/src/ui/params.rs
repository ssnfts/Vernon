//! Parameter rollouts built from `ParamSpec`: the widget comes from the
//! spec's kind (never from the value, which is untagged in JSON).

use eframe::egui::{self, DragValue, RichText, Ui};
use glam::DVec3;
use vernon_graph::{NodeId, ParamKind, ParamSpec, ParamValue};

use super::{FileDialog, FileMode, VernonApp};
use crate::theme;

/// What the user did with one parameter row.
enum Edit {
    None,
    Set(ParamValue),
    Browse { save: bool, extensions: Vec<String> },
}

fn speed_for(v: f64, min: f64, max: f64) -> f64 {
    let span = if (max - min).is_finite() { max - min } else { 1e6 };
    (v.abs() * 0.005).max(span * 1e-5).clamp(1e-4, 50.0)
}

fn row(ui: &mut Ui, spec: &ParamSpec, value: Option<&ParamValue>) -> Edit {
    let unit = spec.unit.clone().map(|u| format!(" {u}")).unwrap_or_default();
    match &spec.kind {
        ParamKind::Float { default, min, max } => {
            let mut v = match value {
                Some(ParamValue::Float(v)) => *v,
                Some(ParamValue::Int(v)) => *v as f64,
                _ => *default,
            };
            let speed = speed_for(v, *min, *max);
            let r = ui.add(
                DragValue::new(&mut v)
                    .range(*min..=*max)
                    .speed(speed)
                    .max_decimals(4)
                    .suffix(unit),
            );
            if r.changed() {
                return Edit::Set(ParamValue::Float(v));
            }
        }
        ParamKind::Int { default, min, max } => {
            let mut v = match value {
                Some(ParamValue::Int(v)) => *v,
                Some(ParamValue::Float(v)) => v.round() as i64,
                _ => *default,
            };
            let r = ui.add(DragValue::new(&mut v).range(*min..=*max).speed(0.2).suffix(unit));
            if r.changed() {
                return Edit::Set(ParamValue::Int(v));
            }
        }
        // A one-shot action: a button, never a checkbox that stays on.
        ParamKind::Bool { .. } if spec.action => {
            if ui.button(&spec.label).on_hover_text(&spec.help).clicked() {
                return Edit::Set(ParamValue::Bool(true));
            }
        }
        ParamKind::Bool { default } => {
            let mut v = match value {
                Some(ParamValue::Bool(b)) => *b,
                Some(ParamValue::Int(i)) => *i != 0,
                _ => *default,
            };
            if ui.checkbox(&mut v, "").changed() {
                return Edit::Set(ParamValue::Bool(v));
            }
        }
        ParamKind::Vec3 { default } => {
            let mut v = match value {
                Some(ParamValue::Vec3(v)) => *v,
                _ => *default,
            };
            let mut changed = false;
            ui.horizontal(|ui| {
                for (i, axis) in ["X", "Y", "Z"].into_iter().enumerate() {
                    let mut c = v[i];
                    let speed = speed_for(c, -1e6, 1e6).max(0.05);
                    let r = ui.add(
                        DragValue::new(&mut c)
                            .speed(speed)
                            .max_decimals(3)
                            .prefix(format!("{axis} ")),
                    );
                    if r.changed() && c.is_finite() {
                        v[i] = c;
                        changed = true;
                    }
                }
                if !unit.is_empty() {
                    ui.label(RichText::new(unit.trim()).small().color(theme::TEXT_DIM));
                }
            });
            if changed {
                return Edit::Set(ParamValue::Vec3(DVec3::new(v.x, v.y, v.z)));
            }
        }
        ParamKind::Str { default } => {
            let mut s = match value {
                Some(ParamValue::Str(s)) => s.clone(),
                _ => default.clone(),
            };
            if ui.add(egui::TextEdit::singleline(&mut s).desired_width(150.0)).changed() {
                return Edit::Set(ParamValue::Str(s));
            }
        }
        ParamKind::Path { default, extensions, save } => {
            let s = match value {
                Some(ParamValue::Str(s)) => s.clone(),
                _ => default.clone(),
            };
            let mut edit = Edit::None;
            ui.horizontal(|ui| {
                // A typed path is kept as a draft and set when the field
                // loses focus (or on Enter), so half-typed paths are not
                // read or written on every keystroke.
                let key = ui.make_persistent_id(("path_draft", &spec.name));
                let draft: Option<String> = ui.data(|d| d.get_temp(key));
                let mut text = draft.clone().unwrap_or_else(|| s.clone());
                let resp = ui.add(egui::TextEdit::singleline(&mut text).desired_width(124.0).hint_text("path"));
                if resp.has_focus() {
                    ui.data_mut(|d| d.insert_temp(key, text.clone()));
                } else if draft.is_some() {
                    ui.data_mut(|d| d.remove::<String>(key));
                    if resp.lost_focus() && text != s {
                        edit = Edit::Set(ParamValue::Str(text.clone()));
                    }
                }
                if ui.small_button("…").on_hover_text("Browse").clicked() {
                    edit = Edit::Browse {
                        save: *save,
                        extensions: extensions.clone(),
                    };
                }
            });
            return edit;
        }
        ParamKind::Enum { default, options } => {
            let cur = match value {
                Some(ParamValue::Str(s)) => s.clone(),
                _ => default.clone(),
            };
            let mut chosen = None;
            egui::ComboBox::from_id_salt(("enum", &spec.name))
                .selected_text(&cur)
                .show_ui(ui, |ui| {
                    for o in options {
                        if ui.selectable_label(*o == cur, o).clicked() {
                            chosen = Some(o.clone());
                        }
                    }
                });
            if let Some(o) = chosen {
                if o != cur {
                    return Edit::Set(ParamValue::Str(o));
                }
            }
        }
    }
    Edit::None
}

/// The parameter grid of node `id`.
pub fn rollout(app: &mut VernonApp, ui: &mut Ui, id: NodeId) {
    let Some(info) = app.st.info(id).cloned() else { return };
    if info.params.is_empty() {
        ui.label(RichText::new("No parameters.").color(theme::TEXT_DIM));
        return;
    }
    egui::Grid::new(("params", id))
        .num_columns(2)
        .spacing([8.0, 4.0])
        .striped(false)
        .show(ui, |ui| {
            for spec in &info.params {
                let value = app.st.graph.nodes.get(&id).and_then(|n| n.params.get(&spec.name).cloned());
                let mut label = ui.label(&spec.label);
                if !spec.help.is_empty() {
                    label = label.on_hover_text(&spec.help);
                }
                label.context_menu(|ui| {
                    if ui.button("Reset to default").clicked() {
                        app.st.set_param(id, &spec.name, spec.default_value(), app.now + 10.0);
                    }
                });
                match row(ui, spec, value.as_ref()) {
                    Edit::None => {}
                    Edit::Set(v) => app.st.set_param(id, &spec.name, v, app.now),
                    Edit::Browse { save, extensions } => {
                        let dir = value
                            .as_ref()
                            .and_then(|v| match v {
                                ParamValue::Str(s) if !s.is_empty() => {
                                    std::path::Path::new(s).parent().map(|p| p.to_path_buf())
                                }
                                _ => None,
                            })
                            .or_else(|| std::env::current_dir().ok())
                            .unwrap_or_default();
                        let mut d = FileDialog::new(
                            FileMode::Param {
                                node: id,
                                param: spec.name.clone(),
                            },
                            dir,
                        );
                        d.extensions = extensions;
                        d.save = save;
                        app.file_dialog = Some(d);
                    }
                }
                ui.end_row();
            }
        });
}

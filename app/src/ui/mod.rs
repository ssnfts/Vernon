//! The egui shell. `VernonApp` owns the document state and the UI state and
//! lays out the regions top to bottom, left to right each frame.

mod bottom;
mod explorer;
pub mod icons;
mod node_editor;
mod panel;
mod params;
mod top;
mod viewports;
mod windows;

use std::path::PathBuf;

use eframe::egui::{self, Event, ViewportCommand};
use vernon_graph::{NodeId, Stage};

use crate::hotkeys::{self, Action, Chord};
use crate::registry::build_registry;
use crate::state::AppState;
use crate::timeline::{Playback, TimeConfig};
use crate::viewport::painter::PainterBackend;
use crate::viewport::{ColorBy, Shading, ViewKind, ViewportBackend};

pub use windows::{FileDialog, FileMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Move,
    Rotate,
    Scale,
}

impl Tool {
    pub fn prompt(self) -> &'static str {
        match self {
            Tool::Select => "Click or drag to select objects (Ctrl adds, Alt removes)",
            Tool::Move => "Drag a selected object to move it in the view plane",
            Tool::Rotate => "Drag a selected object left/right to rotate it about Z",
            Tool::Scale => "Drag a selected object up/down to scale it",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelTab {
    Create,
    Modify,
    Hierarchy,
    Motion,
    Display,
    Utilities,
}

impl PanelTab {
    pub const ALL: [PanelTab; 6] = [
        PanelTab::Create,
        PanelTab::Modify,
        PanelTab::Hierarchy,
        PanelTab::Motion,
        PanelTab::Display,
        PanelTab::Utilities,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PanelTab::Create => "Create",
            PanelTab::Modify => "Modify",
            PanelTab::Hierarchy => "Hierarchy",
            PanelTab::Motion => "Motion",
            PanelTab::Display => "Display",
            PanelTab::Utilities => "Utilities",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Utility {
    Measure,
    Reports,
    Statistics,
    Listener,
    CleanRoom,
}

#[derive(Debug, Default)]
pub struct Windows {
    pub node_editor: bool,
    pub gates: bool,
    pub report: bool,
    pub about: bool,
    pub clean_room: bool,
    pub shortcuts: bool,
}

pub struct VernonApp {
    pub st: AppState,
    pub vp: viewports::Viewports,
    pub backend: Box<dyn ViewportBackend>,
    pub tool: Tool,
    pub snap: bool,
    pub angle_snap: bool,
    pub tab: PanelTab,
    pub create_stage: Stage,
    pub ribbon_stage: Stage,
    pub ribbon_open: bool,
    pub show: Windows,
    pub editor: node_editor::EditorState,
    pub report_node: Option<NodeId>,
    pub listener_line: String,
    pub listener_history: Vec<String>,
    pub explorer_filter: String,
    pub time: TimeConfig,
    pub play: Playback,
    pub color_by: ColorBy,
    pub utility: Option<Utility>,
    pub file_dialog: Option<FileDialog>,
    pub name_buf: (Option<NodeId>, String),
    /// Seconds since start, refreshed every frame (used to coalesce undo).
    pub now: f64,
    title: String,
}

impl VernonApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::theme::apply(&cc.egui_ctx);
        let mut st = AppState::new(build_registry());
        st.info_text("Vernon ready. Try Pipeline > Load Demo Pipeline, or type 'add box' below.");
        // Command line: `--demo` loads the demo pipeline, a `.vrn` path opens it.
        let mut fit = false;
        for arg in std::env::args().skip(1) {
            let r = if arg == "--demo" {
                st.build_demo_pipeline()
            } else {
                st.open(std::path::Path::new(&arg))
            };
            match r {
                Ok(()) => fit = true,
                Err(e) => st.error_text(e),
            }
        }
        let mut vp = viewports::Viewports::default();
        if fit {
            vp.request_fit();
        }
        Self {
            st,
            vp,
            backend: Box::new(PainterBackend),
            tool: Tool::Select,
            snap: false,
            angle_snap: false,
            tab: PanelTab::Create,
            create_stage: Stage::Geometry,
            ribbon_stage: Stage::Geometry,
            ribbon_open: true,
            show: Windows::default(),
            editor: node_editor::EditorState::default(),
            report_node: None,
            listener_line: String::new(),
            listener_history: Vec::new(),
            explorer_filter: String::new(),
            time: TimeConfig::default(),
            play: Playback::default(),
            color_by: ColorBy::Object,
            utility: None,
            file_dialog: None,
            name_buf: (None, String::new()),
            now: 0.0,
            title: String::new(),
        }
    }

    /// Run one listener line and log the outcome.
    pub fn run_command(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        self.listener_history.push(line.to_string());
        self.st.info_text(format!("> {line}"));
        match crate::listener::run(&mut self.st, line) {
            Ok(out) if !out.is_empty() => self.st.info_text(out),
            Ok(_) => {}
            Err(e) => self.st.error_text(e),
        }
    }

    /// Create from the Create panel, ribbon or menu.
    pub fn create(&mut self, type_id: &str) {
        match self.st.create(type_id) {
            Ok(_) => {
                self.vp.request_fit_if_first(&self.st);
            }
            Err(e) => self.st.error_text(e),
        }
    }

    pub fn add_modifier(&mut self, type_id: &str) {
        if let Err(e) = self.st.add_modifier(type_id) {
            self.st.error_text(e);
        } else {
            self.tab = PanelTab::Modify;
        }
    }

    pub fn file_new(&mut self) {
        self.st.new_scene();
        self.vp.request_fit();
    }

    pub fn file_save(&mut self) {
        match self.st.path.clone() {
            Some(p) => {
                if let Err(e) = self.st.save(&p) {
                    self.st.error_text(e);
                }
            }
            None => self.open_dialog(FileMode::SaveAs),
        }
    }

    pub fn open_dialog(&mut self, mode: FileMode) {
        let dir = self
            .st
            .path
            .as_ref()
            .and_then(|p| p.parent().map(PathBuf::from))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        self.file_dialog = Some(FileDialog::new(mode, dir));
    }

    /// Carry out a confirmed file dialog.
    pub fn file_chosen(&mut self, mode: FileMode, path: PathBuf) {
        let result = match mode {
            FileMode::Open => self.st.open(&path).map(|_| self.vp.request_fit()),
            FileMode::SaveAs => {
                let path = if path.extension().is_none() { path.with_extension("vrn") } else { path };
                self.st.save(&path)
            }
            FileMode::Import => self.st.import(&path).map(|_| self.vp.request_fit()),
            FileMode::Export => self.st.export(&path),
            FileMode::Param { node, ref param } => {
                let now = self.now;
                self.st.set_param(node, param, vernon_graph::ParamValue::Str(path.display().to_string()), now);
                Ok(())
            }
        };
        if let Err(e) = result {
            self.st.error_text(e);
        }
    }

    pub fn hide_selected(&mut self) {
        for id in self.st.selection.clone() {
            self.st.set_hidden(id, true);
        }
        self.st.clear_selection();
    }

    pub fn unhide_all(&mut self) {
        for id in self.st.scene.hidden.clone() {
            self.st.set_hidden(id, false);
        }
    }

    pub fn do_action(&mut self, a: Action, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let active = self.vp.layout.active;
        match a {
            Action::Undo => {
                if !self.st.undo() {
                    self.st.info_text("nothing to undo");
                }
            }
            Action::Redo => {
                if !self.st.redo() {
                    self.st.info_text("nothing to redo");
                }
            }
            Action::ViewUndo => self.vp.view_undo(active),
            Action::ViewRedo => self.vp.view_redo(active),
            Action::New => self.file_new(),
            Action::Open => self.open_dialog(FileMode::Open),
            Action::Save => self.file_save(),
            Action::Delete => self.st.delete_selected(),
            Action::SelectAll => self.st.select_all(),
            Action::SelectNone => self.st.clear_selection(),
            Action::SelectInvert => self.st.select_invert(),
            Action::ToolSelect => self.tool = Tool::Select,
            Action::ToolMove => self.tool = Tool::Move,
            Action::ToolRotate => self.tool = Tool::Rotate,
            Action::ToolScale => self.tool = Tool::Scale,
            Action::Snap => self.snap = !self.snap,
            Action::AngleSnap => self.angle_snap = !self.angle_snap,
            Action::ZoomExtentsSelected => self.vp.zoom_extents(&self.st, Some(active), true),
            Action::ZoomExtentsAll => self.vp.zoom_extents(&self.st, Some(active), false),
            Action::ZoomExtentsAllViews => self.vp.zoom_extents(&self.st, None, false),
            Action::Maximize => self.vp.layout.toggle_maximize(),
            Action::ToggleWire => {
                let t = &mut self.vp.tiles[active];
                t.shading = if t.shading == Shading::Shaded { Shading::Wireframe } else { Shading::Shaded };
            }
            Action::ToggleEdges => self.vp.tiles[active].edged ^= true,
            Action::ToggleGrid => self.vp.show_grid ^= true,
            Action::ViewTop => self.vp.set_view(active, ViewKind::Top),
            Action::ViewBottom => self.vp.set_view(active, ViewKind::Bottom),
            Action::ViewFront => self.vp.set_view(active, ViewKind::Front),
            Action::ViewLeft => self.vp.set_view(active, ViewKind::Left),
            Action::ViewPerspective => self.vp.set_view(active, ViewKind::Perspective),
            Action::ViewUser => self.vp.set_view(active, ViewKind::User),
            Action::NodeEditor => self.show.node_editor ^= true,
            Action::Play => self.play.toggle(now),
            Action::PrevFrame => self.play.set_frame(&self.time, self.play.frame - 1, now),
            Action::NextFrame => self.play.set_frame(&self.time, self.play.frame + 1, now),
            Action::GoStart => self.play.set_frame(&self.time, self.time.start, now),
            Action::GoEnd => self.play.set_frame(&self.time, self.time.end, now),
            Action::HideSelected => self.hide_selected(),
            Action::UnhideAll => self.unhide_all(),
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let presses: Vec<Chord> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        modifiers,
                        ..
                    } => Some(Chord {
                        ctrl: modifiers.ctrl || modifiers.command,
                        shift: modifiers.shift,
                        alt: modifiers.alt,
                        key: *key,
                    }),
                    _ => None,
                })
                .collect()
        });
        for chord in presses {
            if let Some(a) = hotkeys::lookup(chord) {
                self.do_action(a, ctx);
            }
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let name = self
            .st
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        let title = format!("{name}{} - Vernon", if self.st.dirty { " *" } else { "" });
        if title != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    /// Prompt line: a missing input or failing gate wins over the tool hint.
    pub fn prompt(&self) -> (String, bool) {
        for (id, _) in self.st.graph.nodes.iter() {
            if let Some(e) = self.st.error_of(*id) {
                if e.contains("not connected") {
                    return (format!("{}: {e}", self.st.name(*id)), true);
                }
            }
        }
        let gates = crate::gate::stage_gates(&self.st);
        if let Some(g) = gates.iter().find(|g| g.state == crate::gate::GateState::Fail) {
            let metric = g
                .report
                .as_ref()
                .and_then(|r| r.metrics.iter().find(|(_, m)| m.passes() == Some(false)).map(|(k, _)| k.clone()))
                .unwrap_or_default();
            return (format!("Gate failing: {} {metric}", g.stage.label()), true);
        }
        (self.tool.prompt().to_string(), false)
    }
}

impl eframe::App for VernonApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.now = ctx.input(|i| i.time);
        self.st.cook_all();
        self.shortcuts(ctx);
        if self.play.tick(&self.time, self.now) || self.play.playing {
            ctx.request_repaint();
        }
        self.backend.begin_frame(frame);

        top::menu_bar(self, ctx);
        top::toolbar(self, ctx);
        top::ribbon(self, ctx);
        bottom::status_bar(self, ctx);
        bottom::track_bar(self, ctx);
        bottom::time_slider(self, ctx);
        explorer::show(self, ctx);
        panel::show(self, ctx);
        viewports::show(self, ctx);
        node_editor::show(self, ctx);
        windows::show(self, ctx);
        // Edits made while drawing take effect this frame.
        self.st.cook_all();
        self.update_title(ctx);
    }
}

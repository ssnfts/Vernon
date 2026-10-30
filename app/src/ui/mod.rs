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
use crate::study::Study;
use crate::unsaved::{Choice, Guard, Pending, Step};
use crate::viewport::gpu::GpuBackend;
use crate::viewport::painter::PainterBackend;
use crate::viewport::{ColorBy, OrbitMode, Shading, ViewKind, ViewportBackend};

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
    Units,
    ExternalTools,
}

/// Scene Explorer sort order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExplorerSort {
    /// Creation order.
    #[default]
    Scene,
    Name,
    Stage,
    LastRun,
}

#[derive(Debug, Default)]
pub struct Windows {
    pub typein: bool,
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
    /// Save prompt before New, Open or closing a modified scene.
    pub guard: Guard,
    /// Closing was confirmed (saved or discarded): let the next close through.
    allow_close: bool,
    close_now: bool,
    /// Time of the last crash-recovery copy.
    last_autosave: f64,
    /// Parameter sweep the time slider drives (Motion tab).
    pub study: Study,
    /// Status-bar and F12 transform type-in mode.
    pub typein_mode: crate::typein::Mode,
    /// Viewport clicks and drags do not change the selection.
    pub selection_locked: bool,
    /// Objects hidden by Isolate (Alt+Q), shown again when it ends.
    pub isolated: Vec<NodeId>,
    /// Pin Stack: the Modify panel stays on this object.
    pub pinned: Option<NodeId>,
    /// Type-to-find text of the modifier list.
    pub modifier_filter: String,
    pub explorer_sort: ExplorerSort,
    /// Utilities > External Tools: path being typed per program.
    pub tool_paths: std::collections::BTreeMap<String, String>,
}

/// Seconds between crash-recovery copies of a modified scene.
const AUTOSAVE_SECS: f64 = 120.0;

impl VernonApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::theme::apply(&cc.egui_ctx);
        let mut st = AppState::new(build_registry());
        st.info_text("Vernon ready. Try Pipeline > Replace Scene with Demo (the body-kit example), or type 'add box' below.");
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
        // GPU renderer when eframe runs on wgpu; VERNON_RENDERER=cpu forces
        // the painter fallback.
        let force_cpu = std::env::var("VERNON_RENDERER").is_ok_and(|v| v.eq_ignore_ascii_case("cpu"));
        let backend: Box<dyn ViewportBackend> = match (&cc.wgpu_render_state, force_cpu) {
            (Some(rs), false) => Box::new(GpuBackend::new(rs.clone())),
            _ => Box::new(PainterBackend),
        };
        eprintln!("vernon: viewport renderer: {}", backend.name());
        st.info_text(format!("viewport renderer: {}", backend.name()));
        Self {
            st,
            vp,
            backend,
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
            // Cd when the geometry carries it (deviation, temperature
            // maps), the object colour otherwise.
            color_by: ColorBy::Cd,
            utility: None,
            file_dialog: None,
            name_buf: (None, String::new()),
            now: 0.0,
            title: String::new(),
            guard: Guard::default(),
            allow_close: false,
            close_now: false,
            last_autosave: 0.0,
            study: Study::default(),
            typein_mode: crate::typein::Mode::Absolute,
            selection_locked: false,
            isolated: Vec::new(),
            pinned: None,
            modifier_filter: String::new(),
            explorer_sort: ExplorerSort::Scene,
            tool_paths: Default::default(),
        }
    }

    /// Drive the study sweep: record the finished sample, then apply the
    /// parameter value of the current frame when the frame moved.
    fn drive_study(&mut self) {
        if !self.study.active() {
            return;
        }
        let Some(node) = self.study.node.filter(|n| self.st.graph.nodes.contains_key(n)) else {
            self.study.enabled = false;
            self.st.info_text("study sweep switched off: its node was removed");
            return;
        };
        self.study.observe(self.st.edit_count());
        if self.study.awaiting() && self.st.cooked() {
            let errors = self.st.graph.nodes.keys().any(|id| self.st.error_of(*id).is_some());
            let failing = crate::gate::stage_gates(&self.st).iter().any(|g| g.state == crate::gate::GateState::Fail);
            self.study.record(!errors && !failing);
        }
        if let Some(v) = self.study.due(&self.time, self.play.frame) {
            let param = self.study.param.clone();
            let is_int = self
                .st
                .info(node)
                .and_then(|i| i.params.iter().find(|s| s.name == param))
                .is_some_and(|s| matches!(s.kind, vernon_graph::ParamKind::Int { .. }));
            let value = if is_int { vernon_graph::ParamValue::Int(v.round() as i64) } else { vernon_graph::ParamValue::Float(v) };
            self.st.graph.frame = self.play.frame as f64;
            self.st.set_param_silent(node, &param, value);
            self.study.applied_revision(self.st.edit_count());
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

    /// File > New: asks to save a modified scene first.
    pub fn file_new(&mut self) {
        let step = self.guard.request(Pending::New, self.st.dirty);
        self.step(step);
    }

    /// Pipeline > Replace Scene with Demo: asks to save a modified scene first.
    pub fn replace_with_demo(&mut self) {
        let step = self.guard.request(Pending::Demo, self.st.dirty);
        self.step(step);
    }

    /// File > Open: asks to save a modified scene first.
    pub fn file_open(&mut self) {
        let step = self.guard.request(Pending::Open, self.st.dirty);
        self.step(step);
    }

    /// Answer to the unsaved-changes prompt.
    pub fn answer_unsaved(&mut self, c: Choice) {
        let step = self.guard.answer(c, self.st.path.is_some());
        self.step(step);
    }

    fn step(&mut self, s: Step) {
        match s {
            Step::Proceed(a) => self.proceed(a),
            Step::SaveThen(a) => {
                if self.file_save() {
                    self.proceed(a);
                }
            }
            Step::SaveAs => self.open_dialog(FileMode::SaveAs),
            Step::Wait => {}
        }
    }

    fn proceed(&mut self, a: Pending) {
        match a {
            Pending::New => {
                self.st.new_scene();
                self.study = Study::default();
                self.vp.request_fit();
            }
            Pending::Open => self.open_dialog(FileMode::Open),
            Pending::Demo => {
                self.st.new_scene();
                self.study = Study::default();
                if let Err(e) = self.st.build_demo_pipeline() {
                    self.st.error_text(e);
                }
                self.vp.request_fit();
            }
            Pending::Close => {
                self.allow_close = true;
                self.close_now = true;
            }
        }
    }

    /// Save to the scene's path (Save As when it has none); true when saved.
    pub fn file_save(&mut self) -> bool {
        match self.st.path.clone() {
            Some(p) => match self.st.save(&p) {
                Ok(()) => true,
                Err(e) => {
                    self.st.error_text(e);
                    false
                }
            },
            None => {
                self.open_dialog(FileMode::SaveAs);
                false
            }
        }
    }

    /// The Save As dialog closed without saving: drop any action waiting on it.
    pub fn file_dialog_cancelled(&mut self, mode: &FileMode) {
        if *mode == FileMode::SaveAs {
            self.guard.save_as_done(false);
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
            FileMode::Open => self.st.open(&path).map(|_| {
                self.study = Study::default();
                self.vp.request_fit()
            }),
            FileMode::SaveAs => {
                let r = self.st.save(&windows::save_target(&mode, &path));
                if let Some(a) = self.guard.save_as_done(r.is_ok()) {
                    self.proceed(a);
                }
                r
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
        self.isolated.clear();
    }

    /// Alt+Q: hide everything but the selection; again to end it.
    pub fn toggle_isolate(&mut self) {
        if !self.isolated.is_empty() {
            let hidden = std::mem::take(&mut self.isolated);
            self.st.end_isolate(&hidden);
            self.st.info_text("isolation ended");
        } else if self.st.selection.is_empty() {
            self.st.info_text("select the objects to isolate");
        } else {
            let keep = self.st.selection.clone();
            self.isolated = self.st.isolate(&keep);
            self.st.info_text(format!("isolated: {} object(s) hidden (Alt+Q ends it)", self.isolated.len()));
        }
    }

    /// The transform channel the active tool types into, if any.
    pub fn typein_channel(&self) -> Option<crate::typein::Channel> {
        use crate::typein::Channel;
        match self.tool {
            Tool::Select => None,
            Tool::Move => Some(Channel::Translate),
            Tool::Rotate => Some(Channel::Rotate),
            Tool::Scale => Some(Channel::Scale),
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
            Action::Open => self.file_open(),
            Action::Save => {
                self.file_save();
            }
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
            Action::HideUnselected => {
                let n = self.st.hide_unselected();
                self.st.info_text(format!("{n} unselected object(s) hidden"));
            }
            Action::Isolate => self.toggle_isolate(),
            Action::SelectionLock => {
                self.selection_locked ^= true;
                let s = if self.selection_locked { "on: viewport clicks keep the selection" } else { "off" };
                self.st.info_text(format!("selection lock {s}"));
            }
            Action::ZoomRegion => {
                self.vp.zoom_region ^= true;
                if self.vp.zoom_region {
                    self.st.info_text("zoom region: drag a rectangle in a viewport");
                }
            }
            Action::TypeIn => self.show.typein ^= true,
            Action::HomeView => {
                self.vp.set_view(active, ViewKind::Perspective);
                self.vp.zoom_extents(&self.st, Some(active), false);
            }
            Action::OrbitMode => {
                self.vp.orbit = match self.vp.orbit {
                    OrbitMode::Turntable => OrbitMode::Arcball,
                    OrbitMode::Arcball => OrbitMode::Turntable,
                };
                let name = if self.vp.orbit == OrbitMode::Turntable { "turntable (Z stays up)" } else { "trackball" };
                self.st.info_text(format!("orbit: {name}"));
            }
            Action::ZoomIn => self.vp.zoom_step(active, 1.0),
            Action::ZoomOut => self.vp.zoom_step(active, -1.0),
            Action::SelectSimilar => {
                let n = self.st.select_similar();
                self.st.info_text(format!("{n} object(s) of the same type selected"));
            }
            Action::Statistics => {
                self.tab = PanelTab::Utilities;
                self.utility = Some(Utility::Statistics);
            }
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
            } else if let Some(what) = hotkeys::reserved(chord) {
                self.st.info_text(format!("{}: {what} is not available in Vernon yet", hotkeys::chord_text(chord)));
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
                .failing_metrics()
                .first()
                .map(|(n, k)| format!("{} {k}", self.st.name(*n)))
                .unwrap_or_default();
            return (format!("Gate failing: {} {metric}", g.stage.label()), true);
        }
        (self.tool.prompt().to_string(), false)
    }
}

impl eframe::App for VernonApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.now = ctx.input(|i| i.time);
        // A modified scene is never dropped by closing the window: ask first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close && self.st.dirty {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            let step = self.guard.request(Pending::Close, true);
            self.step(step);
        }
        if std::mem::take(&mut self.close_now) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        // Crash-recovery copy of a modified scene, next to it (never over it).
        if self.st.dirty && self.now - self.last_autosave >= AUTOSAVE_SECS {
            self.last_autosave = self.now;
            if let Err(e) = self.st.autosave() {
                self.st.error_text(format!("autosave failed: {e}"));
            }
        }
        // Cooking runs on its own thread; the frame only collects results.
        self.st.cook_step();
        self.shortcuts(ctx);
        if self.play.tick(&self.time, self.now) || self.play.playing {
            ctx.request_repaint();
        }
        self.drive_study();
        if self.study.awaiting() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
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
        bottom::typein_window(self, ctx);
        // Edits made while drawing start cooking now; keep repainting while
        // a cook runs so its results appear when it ends.
        if self.st.cook_step() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            // Held-back writes and changed input files need a frame too.
            ctx.request_repaint_after(self.st.wake_after());
        }
        self.update_title(ctx);
    }
}

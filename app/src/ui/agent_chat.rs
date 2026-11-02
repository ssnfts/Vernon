//! Agent chat panel: the model connection (URL, model name, key kept in
//! memory only), a chat log with one line per tool call, Run / Cancel, and
//! Apply / Undo. The agent works on a private copy of the scene on a worker
//! thread; Apply puts the result into the document as one undo step.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use eframe::egui::{self, RichText};
use vernon_agent::{AgentConfig, AgentEvent, AgentOutcome, ClientConfig, OpenAiClient, StopReason, ToolLogEntry};
use vernon_graph::Graph;

use super::VernonApp;
use crate::agent_host::GraphHost;
use crate::registry::build_registry;
use crate::theme;

enum Line {
    User(String),
    Assistant(String),
    Tool(ToolLogEntry),
    Status { text: String, error: bool },
}

enum Msg {
    Event(AgentEvent),
    Done { outcome: AgentOutcome, graph_json: String },
}

pub struct AgentChat {
    url: String,
    model: String,
    /// Typed per session or read from `VERNON_AGENT_KEY`; never saved.
    key: String,
    input: String,
    settings_open: bool,
    lines: Vec<Line>,
    rx: Option<Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
    /// The agent's finished scene, waiting for Apply.
    proposal: Option<String>,
    applied: bool,
}

impl AgentChat {
    pub fn new() -> Self {
        let c = ClientConfig::from_env();
        Self {
            settings_open: c.model.is_empty(),
            url: c.base_url,
            model: c.model,
            key: c.api_key.unwrap_or_default(),
            input: String::new(),
            lines: Vec::new(),
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            proposal: None,
            applied: false,
        }
    }

    fn running(&self) -> bool {
        self.rx.is_some()
    }

    fn status(&mut self, text: impl Into<String>, error: bool) {
        self.lines.push(Line::Status { text: text.into(), error });
    }

    fn start(&mut self, graph_json: String, work_root: Option<std::path::PathBuf>) {
        let goal = self.input.trim().to_string();
        self.input.clear();
        self.lines.push(Line::User(goal.clone()));
        self.proposal = None;
        self.applied = false;
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let client = ClientConfig {
            base_url: self.url.clone(),
            model: self.model.clone(),
            api_key: Some(self.key.clone()).filter(|k| !k.is_empty()),
            ..Default::default()
        };
        std::thread::spawn(move || {
            let mut graph = Graph::from_json(&graph_json).unwrap_or_default();
            graph.work_root = work_root;
            let mut host = GraphHost::new(graph, build_registry());
            let mut model = OpenAiClient::new(client);
            let cfg = AgentConfig { system_prompt: vernon_agent::load_system_prompt(std::env::current_dir().ok().as_deref()), ..Default::default() };
            let outcome = vernon_agent::run_agent(&mut model, &mut host, &goal, &cfg, &cancel, &mut |e| {
                let _ = tx.send(Msg::Event(e));
            });
            let _ = tx.send(Msg::Done { outcome, graph_json: host.graph.to_json() });
        });
    }

    fn poll(&mut self) {
        loop {
            let Some(rx) = &self.rx else { return };
            match rx.try_recv() {
                Ok(Msg::Event(AgentEvent::Assistant(t))) => self.lines.push(Line::Assistant(t)),
                Ok(Msg::Event(AgentEvent::Tool(t))) => self.lines.push(Line::Tool(t)),
                Ok(Msg::Done { outcome, graph_json }) => {
                    self.rx = None;
                    let calls = outcome.log.len();
                    match &outcome.stop {
                        StopReason::Done => self.status(format!("Finished after {} steps, {calls} tool calls.", outcome.steps), false),
                        StopReason::StepLimit => self.status("Stopped: step limit reached. The result so far can still be applied.", true),
                        StopReason::Cancelled => self.status("Cancelled. The result so far can still be applied.", true),
                        StopReason::Stuck => self.status("Stopped: the model kept repeating itself or sent empty replies.", true),
                        StopReason::ModelError(e) => self.status(format!("Model error: {e}"), true),
                    }
                    if calls > 0 {
                        self.proposal = Some(graph_json);
                    }
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.rx = None;
                    self.status("The agent stopped unexpectedly.", true);
                    return;
                }
            }
        }
    }
}

pub fn show(app: &mut VernonApp, ctx: &egui::Context) {
    app.agent.poll();
    if !app.show.agent {
        return;
    }
    if app.agent.running() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    egui::SidePanel::right("agent_chat").resizable(true).default_width(340.0).width_range(260.0..=640.0).show(ctx, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Agent").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Close").clicked() {
                    app.show.agent = false;
                }
                let open = app.agent.settings_open;
                if ui.small_button(if open { "Hide settings" } else { "Settings" }).clicked() {
                    app.agent.settings_open = !open;
                }
            });
        });
        if app.agent.settings_open {
            settings(&mut app.agent, ui);
        }
        ui.separator();
        egui::TopBottomPanel::bottom("agent_input").show_separator_line(false).show_inside(ui, |ui| input_area(app, ui));
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
            if app.agent.lines.is_empty() {
                ui.label(RichText::new("Describe what to build, e.g. \"add a box 300 mm wide and cook it\". The agent edits a copy of the scene; Apply adds the result as one undo step.").color(theme::TEXT_DIM));
            }
            for line in &app.agent.lines {
                draw_line(ui, line);
            }
        });
    });
}

fn settings(a: &mut AgentChat, ui: &mut egui::Ui) {
    egui::Grid::new("agent_settings").num_columns(2).show(ui, |ui| {
        ui.label("URL");
        ui.add(egui::TextEdit::singleline(&mut a.url).desired_width(f32::INFINITY).hint_text("http://localhost:11434/v1"));
        ui.end_row();
        ui.label("Model");
        ui.add(egui::TextEdit::singleline(&mut a.model).desired_width(f32::INFINITY).hint_text("model name"));
        ui.end_row();
        ui.label("Key");
        ui.add(egui::TextEdit::singleline(&mut a.key).password(true).desired_width(f32::INFINITY).hint_text("optional"));
        ui.end_row();
    });
    ui.label(RichText::new("Works with any OpenAI-compatible server over http:// (Ollama, LM Studio, llama.cpp, a local gateway). The key stays in memory and is never saved; set VERNON_AGENT_KEY to prefill it.").small().color(theme::TEXT_DIM));
}

fn input_area(app: &mut VernonApp, ui: &mut egui::Ui) {
    let running = app.agent.running();
    let resp = ui.add_enabled(!running, egui::TextEdit::multiline(&mut app.agent.input).desired_rows(3).desired_width(f32::INFINITY).hint_text("What should the agent do?"));
    let ctrl_enter = resp.has_focus() && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
    ui.horizontal(|ui| {
        let ready = !app.agent.input.trim().is_empty() && !app.agent.model.trim().is_empty();
        let run = ui.add_enabled(!running && ready, egui::Button::new("Run")).on_hover_text("Ctrl+Enter. Needs a model name in Settings.");
        if (run.clicked() || (ctrl_enter && ready)) && !running {
            let json = app.st.graph.to_json();
            let root = app.st.graph.work_root.clone();
            app.agent.start(json, root);
        }
        if ui.add_enabled(running, egui::Button::new("Cancel")).clicked() {
            app.agent.cancel.store(true, Ordering::Relaxed);
        }
        if app.agent.applied {
            if ui.button("Undo").on_hover_text("Revert the applied agent result").clicked() {
                app.st.undo();
                app.agent.applied = false;
                app.agent.proposal = None;
            }
        } else if app.agent.proposal.is_some() && ui.add_enabled(!running, egui::Button::new("Apply")).on_hover_text("Put the agent's scene into the document as one undo step").clicked() {
            if let Some(json) = app.agent.proposal.take() {
                app.st.apply_graph_json(&json);
                app.agent.applied = true;
            }
        }
        if running {
            ui.spinner();
        }
    });
}

fn draw_line(ui: &mut egui::Ui, line: &Line) {
    match line {
        Line::User(t) => {
            ui.label(RichText::new(format!("You: {t}")).color(theme::ACCENT));
        }
        Line::Assistant(t) => {
            ui.label(t);
        }
        Line::Tool(t) => {
            let (mark, color) = if t.ok { ("ok ", theme::PASS) } else { ("err", theme::FAIL_TEXT) };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(mark).monospace().color(color));
                ui.label(RichText::new(format!("{} {}", t.name, t.args)).monospace().small()).on_hover_text(&t.summary);
            });
            if !t.ok {
                ui.label(RichText::new(&t.summary).small().color(theme::FAIL_TEXT));
            }
        }
        Line::Status { text, error } => {
            ui.label(RichText::new(text).italics().color(if *error { theme::WARN } else { theme::TEXT_DIM }));
        }
    }
    ui.add_space(2.0);
}

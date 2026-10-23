//! Vernon desktop app: a DCC-style workspace (menu bar, toolbar, stage
//! ribbon, quad viewports, command panel, time slider) over the Vernon node
//! graph. Non-GUI logic lives in `state`, `listener`, `gate`, `timeline` and
//! `viewport` and is unit-tested; `ui` draws it with egui.

mod gate;
mod hotkeys;
mod listener;
mod registry;
mod state;
mod study;
mod theme;
mod timeline;
mod ui;
mod unsaved;
mod viewport;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Vernon")
            .with_inner_size([1600.0, 960.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Vernon",
        options,
        Box::new(|cc| Ok(Box::new(ui::VernonApp::new(cc)))),
    )
}

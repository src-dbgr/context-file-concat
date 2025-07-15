use eframe::egui;

mod app;
mod core;
mod config;
mod utils;

use app::ContextFileConcatApp;

fn main() -> Result<(), eframe::Error> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "CFC - Context File Concatenator",
        options,
        Box::new(|cc| {
            // Use dark theme by default to match macOS
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            
            Ok(Box::new(ContextFileConcatApp::new(cc)))
        }),
    )
}
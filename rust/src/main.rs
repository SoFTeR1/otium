#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod data;
mod draw;
mod fonts;
mod input;
mod launch;
mod modal;
mod steam;

use std::sync::Arc;

fn main() -> eframe::Result {
    let fullscreen = data::Library::load().setting_bool("fullscreen", true);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("OTIUM")
        .with_inner_size([1600.0, 900.0])
        .with_min_inner_size([960.0, 540.0])
        .with_fullscreen(fullscreen);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../../build/icon.png")) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, vsync: true, persist_window: false, ..Default::default() };
    eframe::run_native("OTIUM", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}

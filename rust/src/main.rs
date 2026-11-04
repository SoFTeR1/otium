#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod account;
mod app;
mod data;
mod discord;
mod draw;
mod fonts;
mod input;
mod keyboard;
mod launch;
mod nostr;
mod overlay;
mod padtest;
mod modal;
mod sound;
mod steam;
mod media;
mod mixer;
mod steam_api;
mod thumbs;
mod tray;
mod voice;
mod watch;
mod win;

use std::sync::Arc;

fn main() -> eframe::Result {
    let fullscreen = data::Library::load().setting_bool("fullscreen", true);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("OTIUM")
        .with_inner_size([1600.0, 900.0])
        .with_min_inner_size([960.0, 540.0])
        .with_fullscreen(fullscreen)
        // the GL config is picked for the main window: it needs alpha so the overlay can be see-through
        .with_transparent(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../../build/icon.png")) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, vsync: true, persist_window: false, ..Default::default() };
    eframe::run_native("OTIUM", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod account;
mod ambient;
mod app;
mod data;
mod discord;
mod draw;
mod fonts;
mod input;
mod installer;
mod keyboard;
mod launch;
mod nostr;
mod overlay;
mod padtest;
mod modal;
mod mouse;
mod sound;
mod steam;
mod media;
mod mixer;
mod services;
mod steam_api;
mod thumbs;
mod tray;
mod update;
mod voice;
mod watch;
mod win;

use std::sync::Arc;

fn main() -> eframe::Result {
    // OTIUM-Setup-X.exe, the Electron updater, a self-update or the uninstaller: no window
    if installer::handle_args() {
        return Ok(());
    }
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

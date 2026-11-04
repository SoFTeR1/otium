//! Segoe UI from Windows (covers Cyrillic), with Segoe UI Symbol / Emoji as fallbacks for
//! controller glyphs and emoji. Falls back to egui's built-in fonts if a file is missing.

use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()) + "\\Fonts\\";
    let mut load = |name: &str, file: &str| -> bool {
        match std::fs::read(format!("{dir}{file}")) {
            Ok(bytes) => {
                fonts.font_data.insert(name.into(), Arc::new(FontData::from_owned(bytes)));
                true
            }
            Err(_) => false,
        }
    };
    let regular = load("segoe", "segoeui.ttf");
    let semi = load("segoe-semibold", "seguisb.ttf");
    let bold = load("segoe-bold", "segoeuib.ttf");
    let symbol = load("segoe-symbol", "seguisym.ttf");
    let emoji = load("segoe-emoji", "seguiemj.ttf");

    let mut fallbacks: Vec<String> = Vec::new();
    if symbol {
        fallbacks.push("segoe-symbol".into());
    }
    if emoji {
        fallbacks.push("segoe-emoji".into());
    }
    let builtin = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();

    let family = |main: Option<&str>| -> Vec<String> {
        let mut v: Vec<String> = main.map(|m| vec![m.to_string()]).unwrap_or_default();
        v.extend(fallbacks.iter().cloned());
        v.extend(builtin.iter().cloned());
        v
    };
    let reg = regular.then_some("segoe");
    fonts.families.insert(FontFamily::Proportional, family(reg));
    fonts.families.insert(FontFamily::Name("semibold".into()), family(if semi { Some("segoe-semibold") } else { reg }));
    fonts.families.insert(FontFamily::Name("bold".into()), family(if bold { Some("segoe-bold") } else { reg }));
    ctx.set_fonts(fonts);
}

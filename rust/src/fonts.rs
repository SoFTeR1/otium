//! Segoe UI from Windows (covers Cyrillic), with Segoe UI Symbol / Emoji as fallbacks for
//! controller glyphs and emoji. Falls back to egui's built-in fonts if a file is missing.

use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()) + "\\Fonts\\";
    let mut load = |name: &str, file: &str| -> bool {
        let path = format!("{dir}{file}");
        // mapped, not read: the pages are shared with every other app using the font and are
        // only touched for the glyphs actually drawn (Segoe UI Emoji alone is 13 MB)
        let data = match map_file(&path) {
            Some(bytes) => FontData::from_static(bytes),
            None => match std::fs::read(&path) {
                Ok(bytes) => FontData::from_owned(bytes),
                Err(_) => return false,
            },
        };
        fonts.font_data.insert(name.into(), Arc::new(data));
        true
    };
    let regular = load("segoe", "segoeui.ttf");
    let semi = load("segoe-semibold", "seguisb.ttf");
    let bold = load("segoe-bold", "segoeuib.ttf");
    let symbol = load("segoe-symbol", "seguisym.ttf");
    let emoji = load("segoe-emoji", "seguiemj.ttf");
    // Windows' own icon font (Windows 11, then Windows 10) for the Control Center tiles
    let icons = load("segoe-icons", "SegoeIcons.ttf") || load("segoe-icons", "segmdl2.ttf");

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
    let mut ic: Vec<String> = if icons { vec!["segoe-icons".into()] } else { Vec::new() };
    ic.extend(family(reg));
    fonts.families.insert(FontFamily::Name("icons".into()), ic);
    ctx.set_fonts(fonts);
}

/// A read-only view of a whole file, kept for the life of the process.
fn map_file(path: &str) -> Option<&'static [u8]> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ};
    use windows::Win32::Storage::FileSystem::{CreateFileW, GetFileSizeEx, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, OPEN_EXISTING};
    use windows::Win32::System::Memory::{CreateFileMappingW, MapViewOfFile, FILE_MAP_READ, PAGE_READONLY};
    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let file = CreateFileW(PCWSTR(wide.as_ptr()), GENERIC_READ.0, FILE_SHARE_READ, None, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, None).ok()?;
        let mut size = 0i64;
        let sized = GetFileSizeEx(file, &mut size).is_ok() && size > 0;
        let mapping = if sized { CreateFileMappingW(file, None, PAGE_READONLY, 0, 0, PCWSTR::null()).ok() } else { None };
        let _ = CloseHandle(file);
        let mapping = mapping?;
        let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
        // the view keeps the mapping alive
        let _ = CloseHandle(mapping);
        if view.Value.is_null() {
            return None;
        }
        Some(std::slice::from_raw_parts(view.Value as *const u8, size as usize))
    }
}

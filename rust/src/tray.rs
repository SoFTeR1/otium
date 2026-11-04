//! Tray icon (closing the window keeps OTIUM running for the Guide button and Discord status)
//! and starting with Windows.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub enum TrayAction {
    Show,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
    show_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
}

impl Tray {
    /// Must be created on the UI thread (it uses that thread's message loop). Clicking the icon
    /// shows the window right away, from the message loop, even while egui isn't repainting.
    pub fn new(own: Arc<AtomicIsize>, bus: crate::watch::Bus) -> Option<Self> {
        let show = MenuItem::new("Открыть OTIUM", true, None);
        let quit = MenuItem::new("Выйти", true, None);
        let menu = Menu::new();
        menu.append_items(&[&show, &PredefinedMenuItem::separator(), &quit]).ok()?;
        let img = image::load_from_memory(include_bytes!("../../build/icon.png")).ok()?.resize(32, 32, image::imageops::FilterType::Triangle).to_rgba8();
        let icon = tray_icon::Icon::from_rgba(img.as_raw().clone(), img.width(), img.height()).ok()?;
        let tray = TrayIconBuilder::new().with_tooltip("OTIUM").with_icon(icon).with_menu(Box::new(menu)).build().ok()?;
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, .. } | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } = e {
                crate::win::show(own.load(Ordering::Relaxed));
                bus.send(crate::watch::Event::Show);
            }
        }));
        Some(Self { _icon: tray, show_id: show.id().clone(), quit_id: quit.id().clone() })
    }

    pub fn poll(&self) -> Option<TrayAction> {
        let e = MenuEvent::receiver().try_recv().ok()?;
        if e.id == self.show_id {
            Some(TrayAction::Show)
        } else if e.id == self.quit_id {
            Some(TrayAction::Quit)
        } else {
            None
        }
    }
}

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_NAME: &str = "OTIUM";

/// Starts OTIUM at sign-in with `--autostart` (the splash then plays without sound).
pub fn set_autostart(on: bool) -> std::io::Result<()> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    let key = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)?;
    if on {
        let exe = std::env::current_exe()?;
        key.set_value(RUN_NAME, &format!("\"{}\" --autostart", exe.display()))
    } else {
        match key.delete_value(RUN_NAME) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }
}

pub fn started_at_login() -> bool {
    std::env::args().any(|a| a == "--autostart")
}

//! The Media tab's streaming services and the test catalog of the Store tab.
//!
//! A service opens as a fullscreen browser app window (Edge, else Chrome) with a profile of its
//! own under %LOCALAPPDATA%\OTIUM\Browser: logins stay, the window is a separate process OTIUM
//! can find, raise and close, and the user's own browser is never touched.

use std::path::PathBuf;
use std::process::{Child, Command};

pub struct Service {
    pub id: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    pub about: &'static str,
    pub color: [u8; 3],
}

pub const SERVICES: &[Service] = &[
    Service {
        id: "kinopoisk",
        name: "Кинопоиск",
        url: "https://hd.kinopoisk.ru/",
        about: "Фильмы, сериалы и спорт по подписке Яндекс Плюс",
        color: [255, 102, 0],
    },
    Service {
        id: "youtube",
        name: "YouTube",
        url: "https://www.youtube.com/",
        about: "Видео, подписки и музыка",
        color: [230, 33, 23],
    },
    Service {
        id: "twitch",
        name: "Twitch",
        url: "https://www.twitch.tv/",
        about: "Трансляции игр и каналы, на которые вы подписаны",
        color: [145, 70, 255],
    },
];

/// Edge first (it plays Кинопоиск's protected video out of the box), then Chrome.
pub fn browser() -> Option<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    for exe in ["msedge.exe", "chrome.exe"] {
        for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            let key = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{exe}");
            if let Ok(k) = winreg::RegKey::predef(hive).open_subkey(key) {
                if let Ok(p) = k.get_value::<String, _>("") {
                    let p = PathBuf::from(p.trim_matches('"'));
                    if p.exists() {
                        return Some(p);
                    }
                }
            }
        }
    }
    let env = |k: &str| std::env::var(k).ok().map(PathBuf::from);
    [
        env("ProgramFiles(x86)").map(|p| p.join(r"Microsoft\Edge\Application\msedge.exe")),
        env("ProgramFiles").map(|p| p.join(r"Microsoft\Edge\Application\msedge.exe")),
        env("ProgramFiles").map(|p| p.join(r"Google\Chrome\Application\chrome.exe")),
        env("LOCALAPPDATA").map(|p| p.join(r"Google\Chrome\Application\chrome.exe")),
    ]
    .into_iter()
    .flatten()
    .find(|p| p.exists())
}

fn profile_dir(id: &str) -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("OTIUM").join("Browser").join(id)
}

/// The browser's arguments for a service window.
fn args(s: &Service) -> Vec<String> {
    vec![
        format!("--user-data-dir={}", profile_dir(s.id).display()),
        format!("--app={}", s.url),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--autoplay-policy=no-user-gesture-required".into(),
    ]
}

/// Presses F11 in the service's window once it is in front (an app window ignores
/// --start-fullscreen). Gives up if the user switched elsewhere.
pub fn make_fullscreen(h: isize) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP};
    for _ in 0..20 {
        if crate::win::foreground() == h {
            // let the page take the window first, or the key goes to a blank frame
            std::thread::sleep(std::time::Duration::from_millis(1200));
            if crate::win::foreground() == h {
                unsafe {
                    keybd_event(0x7A, 0, Default::default(), 0);
                    keybd_event(0x7A, 0, KEYEVENTF_KEYUP, 0);
                }
            }
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// Starts the service's window. The child is the browser process that owns the window; it exits
/// when the window is closed.
pub fn launch(s: &Service) -> Result<Child, String> {
    let exe = browser().ok_or("Не нашёл Microsoft Edge или Google Chrome")?;
    let _ = std::fs::create_dir_all(profile_dir(s.id));
    Command::new(&exe).args(args(s)).spawn().map_err(|e| format!("Не удалось открыть {}: {e}", s.name))
}

// ---------- the Store (a test catalog for now: nothing is installed yet) ----------

pub struct StoreItem {
    pub name: &'static str,
    pub kind: &'static str,
    pub about: &'static str,
    /// what the user has to bring: their own copy of the original game
    pub needs: Option<&'static str>,
    pub color: [u8; 3],
}

pub const STORE: &[StoreItem] = &[
    StoreItem {
        name: "Zelda 64: Recompiled",
        kind: "Рекомпиляция N64",
        about: "Majora's Mask, переведённая в родной код для ПК: широкий экран, высокая частота кадров, моды",
        needs: Some("Свой ROM Majora's Mask (США)"),
        color: [46, 125, 90],
    },
    StoreItem {
        name: "Ship of Harkinian",
        kind: "Декомпиляция",
        about: "Ocarina of Time на ПК: улучшенная графика, рандомайзер и сотни настроек",
        needs: Some("Свой ROM Ocarina of Time"),
        color: [52, 96, 170],
    },
    StoreItem {
        name: "2 Ship 2 Harkinian",
        kind: "Декомпиляция",
        about: "Majora's Mask от авторов Ship of Harkinian",
        needs: Some("Свой ROM Majora's Mask (США)"),
        color: [120, 62, 160],
    },
    StoreItem {
        name: "SM64 CoopDX",
        kind: "Декомпиляция",
        about: "Super Mario 64 с сетевой игрой до 16 игроков и модами",
        needs: Some("Свой ROM Super Mario 64 (США)"),
        color: [200, 50, 50],
    },
    StoreItem {
        name: "OpenGOAL",
        kind: "Порт PS2",
        about: "Jak and Daxter: The Precursor Legacy, работающая на ПК без эмулятора",
        needs: Some("Свой образ диска игры"),
        color: [214, 140, 40],
    },
    StoreItem {
        name: "SuperTuxKart",
        kind: "Свободная игра",
        about: "Картинг с сюжетом, сетевыми гонками и игрой на разделённом экране",
        needs: None,
        color: [40, 150, 200],
    },
    StoreItem {
        name: "OpenTTD",
        kind: "Свободная игра",
        about: "Транспортная стратегия по мотивам Transport Tycoon Deluxe",
        needs: None,
        color: [90, 140, 60],
    },
    StoreItem {
        name: "Mindustry",
        kind: "Свободная игра",
        about: "Защита башнями и автоматизация производства",
        needs: None,
        color: [190, 120, 60],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_args() {
        let yt = SERVICES.iter().find(|s| s.id == "youtube").unwrap();
        let a = args(yt);
        assert!(a.iter().any(|x| x == "--app=https://www.youtube.com/"));
        assert!(a.iter().any(|x| x.starts_with("--user-data-dir=") && x.ends_with(r"OTIUM\Browser\youtube")));
    }
}

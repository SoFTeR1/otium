//! The Media tab's streaming services.
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
    /// opened as the site's TV version (posing as a TV through DevTools), driven by keys
    pub tv: bool,
    /// refuses to work through a VPN: OTIUM offers to switch it off first
    pub no_vpn: bool,
}

/// User agents of TV apps, best first: YouTube shows its TV interface (made for a remote) only to TVs.
pub const TV_AGENTS: &[&str] = &[
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/114.0.0.0 Safari/537.36 Cobalt/25.lts.30.1034943-gold (unlike Gecko) v8/8.8.278.8-jit gles Starboard/15, SystemIntegratorName_ChipsetModelNumber_2024/FirmwareVersion (BrandName, ModelName, Wired)",
    "Mozilla/5.0 (SMART-TV; LINUX; Tizen 6.0) AppleWebKit/537.36 (KHTML, like Gecko) 76.0.3809.146/6.0 TV Safari/537.36",
    "Mozilla/5.0 (PS4; Leanback Shell) Gecko/20100101 Firefox/65.0 LeanbackShell/01.00.01.75 Sony PS4/ (PS4, , no, CH)",
    "Mozilla/5.0 (Web0S; Linux/SmartTV) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/79.0.3945.79 Safari/537.36 WebAppManager",
];

pub const SERVICES: &[Service] = &[
    Service {
        id: "kinopoisk",
        name: "Кинопоиск",
        url: "https://hd.kinopoisk.ru/",
        about: "Фильмы, сериалы и спорт по подписке Яндекс Плюс",
        color: [255, 102, 0],
        tv: false,
        no_vpn: true,
    },
    Service {
        id: "youtube",
        name: "YouTube",
        url: "https://www.youtube.com/tv",
        about: "Версия для телевизора: крестовина листает, A — выбрать, B — назад",
        color: [230, 33, 23],
        tv: true,
        no_vpn: false,
    },
    Service {
        id: "twitch",
        name: "Twitch",
        url: "https://www.twitch.tv/",
        about: "Трансляции игр и каналы, на которые вы подписаны",
        color: [145, 70, 255],
        tv: false,
        no_vpn: false,
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

pub fn profile_dir(id: &str) -> PathBuf {
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
        // Edge signs a new profile in with the Windows account and shows promos over the page,
        // which take the focus from it (Chrome ignores names it doesn't know)
        "--disable-features=msImplicitSignin,msEdgeEnableNurturingFramework,msNurturing".into(),
        // a local DevTools port of the browser's choosing (see `cdp`): fullscreen, the TV pose
        "--remote-debugging-port=0".into(),
    ]
}

/// Starts the service's window. The child is the browser process that owns the window; it exits
/// when the window is closed.
pub fn launch(s: &Service) -> Result<Child, String> {
    let exe = browser().ok_or("Не нашёл Microsoft Edge или Google Chrome")?;
    let _ = std::fs::create_dir_all(profile_dir(s.id));
    // a port file left from the last run would point at a port nobody listens on
    let _ = std::fs::remove_file(profile_dir(s.id).join("DevToolsActivePort"));
    // with a compatibility layer in its environment Edge restarts itself at once, and the process
    // started here is gone while the window lives on in another one
    let child = Command::new(&exe).args(args(s)).env_remove("__COMPAT_LAYER").spawn().map_err(|e| format!("Не удалось открыть {}: {e}", s.name))?;
    remember_pid(s.id, child.id());
    Ok(child)
}

/// The process holding the service's window, for finding it again after OTIUM restarts.
pub fn remember_pid(id: &str, pid: u32) {
    let _ = std::fs::write(profile_dir(id).join("otium.pid"), pid.to_string());
}

/// The browser and window of this service left open from before (OTIUM was restarted meanwhile):
/// a new browser on the same profile would only hand the request to it and quit.
pub fn left_open(s: &Service) -> Option<(u32, isize)> {
    let pid: u32 = std::fs::read_to_string(profile_dir(s.id).join("otium.pid")).ok()?.trim().parse().ok()?;
    // the pid may have been given to another program since
    let stem = crate::win::exe_path(pid)?.file_stem()?.to_string_lossy().to_lowercase();
    if !matches!(stem.as_str(), "msedge" | "chrome") || !crate::win::alive(pid) {
        return None;
    }
    Some((pid, crate::win::main_window(pid)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_args() {
        let yt = SERVICES.iter().find(|s| s.id == "youtube").unwrap();
        let a = args(yt);
        assert!(a.iter().any(|x| x == "--app=https://www.youtube.com/tv"));
        assert!(a.iter().any(|x| x == "--remote-debugging-port=0"));
        assert!(a.iter().any(|x| x.starts_with("--user-data-dir=") && x.ends_with(r"OTIUM\Browser\youtube")));
    }
}

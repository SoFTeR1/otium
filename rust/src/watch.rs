//! Background watchers that work while OTIUM is not in front: the Guide button (XInput),
//! View+Start, the Ctrl+Alt+Home hotkey, and the Steam game currently running.

use crate::win;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone)]
pub enum Event {
    /// Guide pressed while OTIUM was already in front
    GuideToggle,
    /// Guide pressed over another window (normally the game); OTIUM has been raised over it
    GuideFront { prev: isize },
    /// Steam's running app changed (0 = none)
    SteamRunning { id: u32, prev: u32, at: SystemTime },
    /// an exe game we started has exited (its folder has no processes left)
    Exited { id: String, start: SystemTime, end: SystemTime },
    /// the started game's window appeared
    GameWindow { hwnd: isize },
    /// a message from background work (closing a game)
    Toast { text: String, error: bool },
    /// news / trophies of a Steam game arrived
    News { appid: String, items: Vec<crate::steam_api::News> },
    Trophies { appid: String, res: Result<crate::steam_api::Trophies, crate::steam_api::ApiError> },
    FeedDone { appid: String },
    /// achievements of a Steam game that just closed, to announce new trophies (`since` = unix ms)
    TrophyCheck { appid: String, since: f64, res: Result<crate::steam_api::Trophies, crate::steam_api::ApiError> },
    /// result of looking for a newer release on GitHub
    UpdateFound(Option<crate::update::Release>),
    /// downloading the update: percent
    UpdateProgress(u8),
    /// the update's installer is downloaded (its path) or failed
    UpdateReady(Result<String, String>),
    /// the Steam friend list arrived
    SteamFriends(Result<Vec<crate::steam_api::SteamFriend>, crate::steam_api::ApiError>),
    /// public art found for a game, for the Discord status ("" = nothing fits)
    PublicArt { id: String, name: String, url: String },
    /// show the window (tray click); egui has to be told, or it keeps it hidden
    Show,
    Nostr(crate::nostr::Msg),
    Voice(crate::voice::Event),
    /// what's playing changed (None = nothing)
    Music(Option<crate::media::Music>),
    /// a Media tab service window appeared / its browser exited (after `secs` seconds)
    ServiceWindow { id: &'static str, hwnd: isize },
    ServiceClosed { id: &'static str, secs: u64 },
}

/// Sends events to the UI and wakes it up.
#[derive(Clone)]
pub struct Bus {
    tx: Sender<Event>,
    ctx: egui::Context,
}

impl Bus {
    pub fn new(tx: Sender<Event>, ctx: egui::Context) -> Self {
        Self { tx, ctx }
    }
    pub fn send(&self, e: Event) {
        let _ = self.tx.send(e);
        self.ctx.request_repaint();
    }
}

// ---------- Guide button via XInputGetStateEx ----------

#[repr(C)]
#[derive(Default)]
struct XState {
    packet: u32,
    buttons: u16,
    lt: u8,
    rt: u8,
    lx: i16,
    ly: i16,
    rx: i16,
    ry: i16,
}

type GetStateEx = unsafe extern "system" fn(u32, *mut XState) -> u32;

fn load_get_state_ex() -> Option<GetStateEx> {
    use windows::core::{PCSTR, PCWSTR};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    for dll in ["xinput1_4.dll", "xinput1_3.dll"] {
        let wide: Vec<u16> = dll.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let Ok(m) = LoadLibraryW(PCWSTR(wide.as_ptr())) else { continue };
            // ordinal 100 is the undocumented variant that also reports the Guide button
            if let Some(f) = GetProcAddress(m, PCSTR(100usize as *const u8)) {
                return Some(std::mem::transmute::<unsafe extern "system" fn() -> isize, GetStateEx>(f));
            }
        }
    }
    None
}

const GUIDE: u16 = 0x0400;
const BACK: u16 = 0x0020;
const START: u16 = 0x0010;

/// `own` is OTIUM's window handle, filled in once the window exists.
pub fn start_guide(bus: Bus, own: Arc<AtomicIsize>) {
    let Some(get) = load_get_state_ex() else { return };
    std::thread::Builder::new()
        .name("guide".into())
        .spawn(move || {
            let mut prev = [0u16; 4];
            loop {
                for (i, p) in prev.iter_mut().enumerate() {
                    let mut s = XState::default();
                    if unsafe { get(i as u32, &mut s) } != 0 {
                        *p = 0;
                        continue;
                    }
                    let b = s.buttons;
                    let guide = b & GUIDE != 0 && *p & GUIDE == 0;
                    let combo = b & (BACK | START) == BACK | START && *p & (BACK | START) != BACK | START;
                    if guide || combo {
                        summon(&bus, own.load(Ordering::Relaxed));
                    }
                    *p = b;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
        })
        .ok();
}

fn summon(bus: &Bus, own: isize) {
    if own == 0 {
        return;
    }
    let fg = win::foreground();
    if fg == own && !win::is_minimized(own) {
        bus.send(Event::GuideToggle);
    } else {
        win::activate(own);
        bus.send(Event::GuideFront { prev: fg });
    }
}

/// Ctrl+Alt+Home from anywhere does what the Guide button does.
pub fn start_hotkey(bus: Bus, own: Arc<AtomicIsize>) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT};
    use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};
    std::thread::Builder::new()
        .name("hotkey".into())
        .spawn(move || unsafe {
            // hotkeys registered without a window are posted to this thread's queue
            if let Err(e) = RegisterHotKey(None, 1, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 0x24) {
                eprintln!("Ctrl+Alt+Home is taken: {e}");
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.message == WM_HOTKEY {
                    summon(&bus, own.load(Ordering::Relaxed));
                }
            }
        })
        .ok();
}

// ---------- Steam running app ----------

pub fn steam_running_now() -> u32 {
    use winreg::enums::HKEY_CURRENT_USER;
    winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Valve\\Steam")
        .and_then(|k| k.get_value::<u32, _>("RunningAppID"))
        .unwrap_or(0)
}

pub fn start_steam(bus: Bus) {
    std::thread::Builder::new()
        .name("steam-running".into())
        .spawn(move || {
            let mut current = steam_running_now();
            loop {
                std::thread::sleep(Duration::from_millis(2500));
                let id = steam_running_now();
                if id != current {
                    bus.send(Event::SteamRunning { id, prev: current, at: SystemTime::now() });
                    current = id;
                }
            }
        })
        .ok();
}

/// Looks for the started game's window for up to 90 s (emulators and launchers take a while)
/// and raises it above OTIUM.
pub fn focus_new_game(bus: Bus, own: isize, since: SystemTime) {
    std::thread::spawn(move || {
        for _ in 0..180 {
            if let Some(h) = win::find_new_window(own, since) {
                win::activate(h);
                bus.send(Event::GameWindow { hwnd: h });
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

//! The shell: splash, then the home screen with the game row, background art and Play button,
//! plus the Add / Steam import / Options menus on top of it.

use crate::data::{image_uri, Game, Library};
use crate::draw::{self, black, bold, font, semibold, white};
use crate::input::{Input, Nav, PadKind};
use crate::modal::{Action, Modal};
use crate::steam::{self, LocalStat};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

mod calls;
mod control;
mod top;
mod profile;
pub use control::MixerPanel;
mod edit;
mod pages;
mod run;
pub use calls::PickFor;
mod settings;
mod tabs;
pub(crate) use tabs::Tab;
mod social;
pub use pages::fmt_date;
use run::Launching;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Zone {
    /// the top bar: index into `top_items()`
    Top(usize),
    Row,
    Play,
    More,
}

struct Toast {
    text: String,
    error: bool,
    at: Instant,
}

/// Image candidates for one game, best first; failed ones are skipped when painting.
#[derive(Default, Clone)]
struct Art {
    covers: Vec<String>,
    backgrounds: Vec<String>,
    logos: Vec<String>,
}

pub struct App {
    lib: Library,
    games: Vec<Game>,
    art: HashMap<String, Art>,
    steam_root: Option<PathBuf>,
    steam_stats: HashMap<String, LocalStat>,
    input: Input,
    sel: usize,
    zone: Zone,
    scroll: f32,
    modal: Option<Modal>,
    modal_at: Instant,
    /// background of the previous game, faded out under the new one
    bg_prev: Vec<String>,
    bg_changed: Instant,
    splash_start: Option<Instant>,
    toast: Option<Toast>,
    overlay: crate::overlay::Overlay,
    overlay_demo_toasts: u32,
    timer: control::SleepTimer,
    /// game names looked up for Discord art this session
    art_searched: HashSet<String>,
    /// last sweep of images that are no longer drawn
    forget_at: Instant,
    /// Control Center tile to come back to when what it opened is closed with B
    cc_return: Option<usize>,
    updates: top::Updates,
    /// games found by the last search, for its results menu
    search_hits: Vec<String>,
    /// mouse wheel not yet turned into steps
    wheel: f32,
    fullscreen: bool,
    fs_tries: u32,
    started: Instant,
    backed_up: bool,
    refocus: bool,

    // ---- running games ----
    events: Receiver<crate::watch::Event>,
    bus: crate::watch::Bus,
    /// OTIUM's own window handle, shared with the Guide watcher
    own: Arc<AtomicIsize>,
    pids: crate::launch::Pids,
    /// ids of games that are running right now
    running: HashSet<String>,
    /// Steam app id -> when it started running
    steam_since: HashMap<u32, SystemTime>,
    steam_now: u32,
    launching: Option<Launching>,
    /// the game's window, to hand control back to it
    game_hwnd: isize,
    /// a game was started and OTIUM stepped aside for it
    away_for_game: bool,
    reload_steam_at: Option<Instant>,
    /// when the last game was started, to find its window again
    last_launch: Option<SystemTime>,
    /// last time the launch splash re-asserted "always on top"
    covered_at: Instant,

    // ---- pages below the home screen ----
    pages: pages::Pages,
    media_tx: std::sync::mpsc::Sender<crate::media::MediaCmd>,
    /// set while a game has the screen: background polling pauses
    idle: Arc<std::sync::atomic::AtomicBool>,
    steam_user_local: Option<String>,

    // ---- window / tray ----
    tray: Option<crate::tray::Tray>,
    quitting: bool,
    tray_hinted: bool,
    /// viewport commands queued by code that has no `egui::Context` at hand
    pending_cmd: Vec<egui::ViewportCommand>,
    last_text_target: Option<crate::keyboard::Target>,
    /// the Discord status should be rebuilt (a game started or stopped)
    discord_soon: bool,
    social: social::Social,
    calls: calls::Calls,
    /// the game the editor is open for
    edit_id: Option<String>,
    /// "Переставить": Left / Right move the selected tile
    reorder: bool,
    /// the Media and Store tabs
    tabs: tabs::Tabs,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        // tried before egui_extras' loaders: decodes in the background and downscales
        cc.egui_ctx.add_image_loader(std::sync::Arc::new(crate::thumbs::Thumbs::default()));
        crate::fonts::install(&cc.egui_ctx);
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = Color32::from_rgb(6, 8, 14);
        cc.egui_ctx.set_style(style);

        let lib = Library::load();
        let splash = lib.setting_bool("splash", true);
        let fullscreen = lib.setting_bool("fullscreen", true);
        let (steam_user_local, steam_stats) = steam::local_stats();
        let (media_tx, media_rx) = crate::media::channel();
        let idle = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, events) = std::sync::mpsc::channel();
        let bus = crate::watch::Bus::new(tx, cc.egui_ctx.clone());
        let own = Arc::new(AtomicIsize::new(0));
        crate::watch::start_guide(bus.clone(), own.clone());
        crate::watch::start_hotkey(bus.clone(), own.clone());
        crate::watch::start_steam(bus.clone());
        let steam_now = crate::watch::steam_running_now();
        let tray = crate::tray::Tray::new(own.clone(), bus.clone());
        let social_bus = bus.clone();
        let calls_bus = bus.clone();
        {
            let bus = bus.clone();
            crate::media::start(cc.egui_ctx.clone(), media_rx, idle.clone(), move |m| bus.send(crate::watch::Event::Music(m)));
        }
        let mut app = Self {
            games: Vec::new(),
            art: HashMap::new(),
            steam_root: steam::root(),
            steam_stats,
            input: Input::new(),
            sel: 0,
            zone: Zone::Row,
            scroll: 0.0,
            modal: None,
            modal_at: Instant::now(),
            bg_prev: Vec::new(),
            bg_changed: Instant::now() - Duration::from_secs(5),
            splash_start: splash.then(Instant::now),
            toast: None,
            overlay: Default::default(),
            overlay_demo_toasts: 0,
            timer: Default::default(),
            art_searched: HashSet::new(),
            forget_at: Instant::now(),
            cc_return: None,
            updates: Default::default(),
            search_hits: Vec::new(),
            wheel: 0.0,
            fullscreen,
            fs_tries: 0,
            started: Instant::now(),
            backed_up: false,
            refocus: false,
            events,
            bus,
            own,
            pids: Default::default(),
            running: HashSet::new(),
            steam_since: HashMap::new(),
            steam_now,
            launching: None,
            game_hwnd: 0,
            away_for_game: false,
            reload_steam_at: None,
            last_launch: None,
            covered_at: Instant::now(),
            pages: Default::default(),
            media_tx,
            idle,
            steam_user_local,
            tray,
            quitting: false,
            tray_hinted: false,
            pending_cmd: Vec::new(),
            last_text_target: None,
            discord_soon: false,
            social: social::Social::new(social_bus),
            calls: calls::Calls::new(calls_bus),
            edit_id: None,
            reorder: false,
            tabs: Default::default(),
            lib,
        };
        app.input.set_cal(app.lib.settings.get("padCal"));
        app.social_start();
        // the splash chord, except at sign-in (Windows is still starting, sound would be a surprise)
        let gain = crate::sound::splash_gain(app.lib.setting_str("splashSound"));
        if app.splash_start.is_some() && gain > 0.0 && !crate::tray::started_at_login() {
            crate::sound::play_gain(crate::sound::Sfx::Splash, gain);
        }
        if !app.lib.setting_str("discordId").is_empty() {
            let id = app.lib.setting_str("discordId").to_string();
            crate::discord::configure(Some(&id));
            app.update_discord();
        }
        // a Steam game that was already running when OTIUM started
        if steam_now != 0 {
            app.steam_since.insert(steam_now, SystemTime::now());
        }
        app.refresh(None);
        app
    }

    /// Re-sorts the row after the library changed, keeping `keep` (a game id) selected.
    fn refresh(&mut self, keep: Option<&str>) {
        let keep = keep.map(str::to_string).or_else(|| self.games.get(self.sel).map(|g| g.id.clone()));
        self.games = self.lib.sorted_games();
        let root = self.steam_root.as_deref();
        self.art = self.games.iter().map(|g| (g.id.clone(), art_for(g, root))).collect();
        self.sel = keep
            .and_then(|id| self.games.iter().position(|g| g.id == id))
            .unwrap_or(self.sel)
            .min(self.games.len().saturating_sub(1));
        self.scroll = self.scroll.min(self.games.len() as f32);
        if self.games.is_empty() {
            self.zone = Zone::Row;
        }
    }

    /// Writes library.json; the first write of a session keeps a copy of what was there before.
    fn save(&mut self) {
        if !self.backed_up {
            let file = Library::file();
            if file.exists() {
                let _ = std::fs::copy(&file, file.with_file_name("library.before-rust.json"));
            }
            self.backed_up = true;
        }
        if let Err(e) = self.lib.save() {
            self.toast(&format!("Не удалось сохранить библиотеку: {e}"), true);
        }
    }

    fn art(&self, g: &Game) -> Art {
        self.art.get(&g.id).cloned().unwrap_or_default()
    }

    fn select(&mut self, i: usize) {
        if i == self.sel || i >= self.games.len() {
            return;
        }
        self.bg_prev = self.games.get(self.sel).map(|g| self.art(g).backgrounds).unwrap_or_default();
        self.bg_changed = Instant::now();
        self.sel = i;
    }

    fn open(&mut self, m: Modal) {
        self.modal = Some(m);
        self.modal_at = Instant::now();
    }

    fn handle(&mut self, nav: Nav, frame: &eframe::Frame) {
        let before = (self.sel, self.zone, self.pages.page, self.modal.is_some(), self.launching.is_some(), self.tab());
        let from_cc = matches!(self.modal, Some(Modal::Control { .. }));
        self.handle_nav(nav, frame);
        self.after_control_center(nav, from_cc);
        if !self.sounds_on() {
            return;
        }
        let after = (self.sel, self.zone, self.pages.page, self.modal.is_some(), self.launching.is_some(), self.tab());
        let sfx = if self.launching.is_some() && !before.4 {
            Some(crate::sound::Sfx::Launch)
        } else {
            match nav {
                Nav::Up | Nav::Down | Nav::Left | Nav::Right => Some(crate::sound::Sfx::Move),
                Nav::Back if before != after || before.3 => Some(crate::sound::Sfx::Back),
                Nav::Ok | Nav::Options | Nav::Add | Nav::Guide | Nav::Menu => Some(crate::sound::Sfx::Ok),
                _ => None,
            }
        };
        if let Some(s) = sfx {
            crate::sound::play(s);
        }
    }

    fn handle_nav(&mut self, nav: Nav, frame: &eframe::Frame) {
        if nav == Nav::Guide {
            return self.guide_pressed();
        }
        // a press that opened a menu must not also pick its first item (double taps, key bounce)
        if nav == Nav::Ok && self.modal.is_some() && self.modal_at.elapsed() < Duration::from_millis(300) {
            return;
        }
        if self.call_menu_side(nav) {
            return;
        }
        // ≡ opens the settings from anywhere but the on-screen keyboard (there it means "done")
        if nav == Nav::Menu && !matches!(self.modal, Some(Modal::Keyboard(_)) | Some(Modal::PadTest(_))) {
            self.social.open = false;
            return self.open_settings(crate::modal::SetMenu::Main, 0);
        }
        if matches!(self.modal, Some(Modal::Control { .. })) {
            return self.control_nav(nav);
        }
        if matches!(self.modal, Some(Modal::Friend { .. })) {
            return self.friend_profile_nav(nav);
        }
        if let Some(m) = self.modal.as_mut() {
            let action = m.handle(nav);
            self.apply(action, frame);
            return;
        }
        // the call card answers A / B / Y before anything else
        if self.modal.is_none() && self.calls.card_open() && self.call_nav(nav) {
            return;
        }
        if self.social.open {
            return self.social_nav(nav);
        }
        if self.pages.page != 0 && !matches!(nav, Nav::Add | Nav::Options) {
            return self.handle_page(nav);
        }
        if self.reorder {
            return self.reorder_nav(nav);
        }
        let n = self.games.len();
        if let Zone::Top(i) = self.zone {
            return self.top_nav(i, nav);
        }
        if self.tab() != Tab::Games {
            return self.tab_nav(nav);
        }
        match (self.zone, nav) {
            (Zone::Row, Nav::Up) => self.enter_top(),
            (Zone::Row, Nav::Left) if self.sel > 0 => self.select(self.sel - 1),
            (Zone::Row, Nav::Right) if self.sel + 1 < n => self.select(self.sel + 1),
            (Zone::Row, Nav::Down) if n > 0 => self.zone = Zone::Play,
            (Zone::Row, Nav::Ok) if n > 0 => self.launch_selected(),
            (Zone::Row, Nav::Ok) => self.open(Modal::Add { focus: 0 }),
            (Zone::Play, Nav::Ok) => self.launch_selected(),
            (Zone::Play, Nav::Right) => self.zone = Zone::More,
            (Zone::More, Nav::Left) => self.zone = Zone::Play,
            (Zone::Play | Zone::More, Nav::Up | Nav::Back) => self.zone = Zone::Row,
            (Zone::Play | Zone::More, Nav::Down) => self.go_page(1),
            (Zone::More, Nav::Ok) | (_, Nav::Options) => self.open_options(),
            (_, Nav::Guide) => self.open_control(),
            (_, Nav::Add) => self.open(Modal::Add { focus: 0 }),
            _ => {}
        }
    }

    fn apply(&mut self, action: Action, frame: &eframe::Frame) {
        match action {
            Action::None => {}
            Action::Close => self.modal = None,
            Action::OpenImport => self.open_import(),
            Action::PickFile => {
                self.modal = None;
                self.pick_file(frame);
            }
            Action::Import(list) => {
                self.modal = None;
                self.import(list);
            }
            Action::Option(o) => self.option(o),
            Action::OpenUrl(url) => {
                if std::env::var_os("OTIUM_NO_OPEN").is_some() {
                    eprintln!("open url (suppressed): {url}");
                } else if url.starts_with("https://") || url.starts_with("steam://") {
                    let _ = open::that_detached(&url);
                }
            }
            Action::Setting(menu, key, focus) => self.apply_setting(menu, key, focus),
            Action::Text(o) => self.text_done(o),
            Action::Confirmed(k) => self.confirmed(k),
            Action::RequestFriend(pk, name) => self.request_friend(pk, name),
            Action::Picked(purpose, pks) => self.picked(purpose, pks),
            Action::AnswerRequest(pk, yes) => self.answer_request(pk, yes),
            Action::Delete => {
                self.modal = None;
                self.delete_selected();
            }
        }
    }

    fn open_import(&mut self) {
        match steam::scan() {
            Err(e) => self.open(Modal::Message { title: "Steam".into(), text: e }),
            Ok(found) => {
                let have: HashSet<String> = self.lib.games.iter().filter_map(Game::steam_app_id).collect();
                let fresh: Vec<_> = found.into_iter().filter(|g| !have.contains(&g.app_id)).collect();
                if fresh.is_empty() {
                    self.open(Modal::Message {
                        title: "Steam".into(),
                        text: "Все установленные игры Steam уже в библиотеке.".into(),
                    });
                } else {
                    self.open(Modal::import(fresh));
                }
            }
        }
    }

    fn import(&mut self, list: Vec<steam::SteamGame>) {
        if list.is_empty() {
            self.toast("Ничего не выбрано", false);
            return;
        }
        let now = now_ms();
        let first = Game::from_steam(&list[0], now);
        let first_id = first.id.clone();
        self.lib.games.push(first);
        for (i, g) in list.iter().enumerate().skip(1) {
            self.lib.games.push(Game::from_steam(g, now - i as f64));
        }
        self.save();
        self.refresh(Some(&first_id));
        let n = list.len();
        self.toast(&format!("Добавлено в библиотеку: {n} {}", plural(n, "игра", "игры", "игр")), false);
    }

    fn pick_file(&mut self, frame: &eframe::Frame) {
        let picked = rfd::FileDialog::new()
            .set_title("Выберите игру или приложение")
            .add_filter("Игры и ярлыки", &["exe", "lnk", "url", "bat", "cmd"])
            .set_parent(frame)
            .pick_file();
        // the dialog takes focus; bring the shell back to the front on the next frame
        self.refocus = true;
        let Some(path) = picked else { return };
        let path_str = path.to_string_lossy().to_string();
        if self.lib.games.iter().any(|g| g.path.eq_ignore_ascii_case(&path_str)) {
            self.toast("Эта игра уже в библиотеке", false);
            return;
        }
        let name = name_from_path(&path);
        let mut game = Game::new(&name, &path_str);
        game.cover = Library::exe_icon(&path_str);
        let id = game.id.clone();
        self.lib.games.push(game);
        self.save();
        self.refresh(Some(&id));
        self.toast(&format!("«{name}» добавлена в библиотеку"), false);
    }

    fn make_first(&mut self) {
        let Some(id) = self.games.get(self.sel).map(|g| g.id.clone()) else { return };
        // put the whole current order into `order`, then move this game in front of it
        for (i, g) in self.games.iter().enumerate() {
            if let Some(lg) = self.lib.games.iter_mut().find(|x| x.id == g.id) {
                lg.order = Some(i as f64 + 1.0);
            }
        }
        if let Some(lg) = self.lib.games.iter_mut().find(|x| x.id == id) {
            lg.order = Some(0.0);
        }
        self.lib.settings.insert("sortMode".into(), "custom".into());
        self.save();
        self.refresh(Some(&id));
        self.scroll = 0.0;
        self.zone = Zone::Row;
    }

    fn delete_selected(&mut self) {
        let Some(g) = self.games.get(self.sel).cloned() else { return };
        self.lib.games.retain(|x| x.id != g.id);
        self.save();
        let next = self.games.get(self.sel + 1).or_else(|| self.sel.checked_sub(1).and_then(|i| self.games.get(i)));
        let next = next.map(|x| x.id.clone());
        self.refresh(next.as_deref());
        self.zone = Zone::Row;
        self.toast(&format!("«{}» удалена из библиотеки", g.name), false);
    }

    /// winit reports fullscreen but keeps the window at its initial size when fullscreen is
    /// requested before the window is shown. Re-apply it until the window really covers the monitor.
    fn keep_fullscreen(&mut self, ctx: &egui::Context) {
        if !self.fullscreen || self.fs_tries >= 6 {
            return;
        }
        let (inner, monitor) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().monitor_size));
        let (Some(inner), Some(monitor)) = (inner, monitor) else { return };
        if (inner.width() - monitor.x).abs() < 2.0 && (inner.height() - monitor.y).abs() < 2.0 {
            self.fs_tries = 6;
            return;
        }
        // alternate off/on so winit actually resizes, every few frames to let it settle
        if ctx.cumulative_pass_nr() % 4 == 0 {
            let on = self.fs_tries % 2 == 1;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
            if on {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            self.fs_tries += 1;
        }
    }

    fn launch_selected(&mut self) {
        let Some(game) = self.games.get(self.sel).cloned() else { return };
        if self.is_running(&game) {
            // already running: Play just goes back to it
            return self.back_to_game();
        }
        self.start_launch(game);
    }

    /// Background music: only while OTIUM is in front and nothing else should be heard — not during
    /// the splash (it has its own chord), a game launch, calls, or music already playing in Windows.
    fn update_ambient(&self, front: bool) {
        if !crate::ambient::available() {
            return;
        }
        let setting = self.lib.setting_str("ambient");
        let busy = self.splash_start.is_some()
            || self.launching.is_some()
            || self.calls.call.state != crate::app::calls::CallState::Idle
            || self.calls.room.is_some()
            || self.calls.ring.is_some()
            || self.pages.music.as_ref().is_some_and(|m| m.playing);
        crate::ambient::set(front && !busy, crate::ambient::level(setting));
    }

    fn toast(&mut self, text: &str, error: bool) {
        if self.away() {
            return self.overlay.toast("OTIUM", text, None, error);
        }
        self.toast = Some(Toast { text: text.into(), error, at: Instant::now() });
    }

    /// A notification from someone: over the game it gets their name as the title and their
    /// avatar; inside OTIUM it is the usual toast.
    fn notify(&mut self, title: &str, text: &str, avatar: (String, String)) {
        if self.away() {
            return self.overlay.toast(title, text, Some(avatar), false);
        }
        self.toast(&format!("{title}: {text}"), false);
    }

    /// OTIUM isn't the window in front (a game or anything else is, or it's in the tray).
    fn away(&self) -> bool {
        let own = self.own.load(Ordering::Relaxed);
        own != 0 && crate::win::foreground() != own
    }

    /// Playtime in seconds: whichever is larger, ours or what the Steam client recorded.
    fn playtime(&self, g: &Game) -> f64 {
        let steam = g.steam_app_id().and_then(|id| self.steam_stats.get(&id)).map_or(0.0, |s| s.playtime);
        g.playtime.unwrap_or(0.0).max(steam)
    }
}

/// The native window handle of OTIUM's window (0 until it exists).
fn own_hwnd(frame: &eframe::Frame) -> isize {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
}

fn art_for(g: &Game, root: Option<&std::path::Path>) -> Art {
    // Art the user picked goes first. A plain Steam CDN link (what imports store) goes after Steam's
    // own cache: the cache is local, always current, and some old CDN links now serve the wrong image.
    fn merge(stored: Option<&str>, steam: Vec<String>) -> Vec<String> {
        let stored = stored.filter(|s| !s.trim().is_empty());
        let mut out = Vec::new();
        match stored {
            Some(s) if !s.starts_with(steam::CDN) => {
                out.extend(image_uri(s));
                out.extend(steam.iter().filter_map(|u| image_uri(u)));
            }
            Some(s) => {
                out.extend(steam.iter().filter(|u| !u.starts_with("http")).filter_map(|u| image_uri(u)));
                out.extend(image_uri(s));
                out.extend(steam.iter().filter(|u| u.starts_with("http")).filter_map(|u| image_uri(u)));
            }
            None => out.extend(steam.iter().filter_map(|u| image_uri(u))),
        }
        out
    }
    let mut a = Art::default();
    let id = g.steam_app_id();
    let steam = |f: fn(Option<&std::path::Path>, &str) -> Vec<String>| id.as_deref().map(|id| f(root, id)).unwrap_or_default();
    a.covers = merge(g.cover.as_deref(), steam(steam::cover_candidates));
    a.backgrounds.extend(g.custom_background().and_then(image_uri));
    a.backgrounds.extend(merge(g.hero.as_deref(), steam(steam::hero_candidates)));
    a.backgrounds.extend(a.covers.iter().cloned());
    a.logos = merge(g.logo.as_deref(), steam(steam::logo_candidates));
    for list in [&mut a.covers, &mut a.backgrounds, &mut a.logos] {
        let mut seen = HashSet::new();
        list.retain(|u| seen.insert(u.to_lowercase()));
    }
    a
}

fn name_from_path(path: &std::path::Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let cleaned: String = stem.chars().map(|c| if matches!(c, '_' | '.' | '-') { ' ' } else { c }).collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        "Игра".into()
    } else {
        cleaned
    }
}

pub(crate) fn plural(n: usize, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    match (n % 10, n % 100) {
        (1, x) if x != 11 => one,
        (2..=4, x) if !(12..=14).contains(&x) => few,
        _ => many,
    }
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // 60 fps only while OTIUM is in front: behind a game or in the tray a few frames a second
        // are enough for timers and calls (events, the Guide button and the overlay wake it at once),
        // and the GPU is left to the game
        ctx.request_repaint_after(Duration::from_millis(if self.away() { 250 } else { 16 }));
        if self.own.load(Ordering::Relaxed) == 0 {
            self.own.store(own_hwnd(frame), Ordering::Relaxed);
        }
        while let Ok(e) = self.events.try_recv() {
            self.on_event(e);
        }
        self.tick(ctx);
        self.tick_timer();
        self.tick_updates();
        if self.forget_at.elapsed() > Duration::from_secs(5) && std::env::var("OTIUM_MEM_EXP").ok().as_deref() != Some("tour-keep") {
            self.forget_at = Instant::now();
            draw::forget_unused(ctx, Duration::from_secs(30));
        }
        self.debug_open_profile();
        self.debug_open_social();
        // OTIUM_MEM_EXP=tour: walk through every game twice (loading every background and cover),
        // then rest on the first one, to measure what is kept in memory afterwards
        if std::env::var("OTIUM_MEM_EXP").is_ok_and(|v| v.starts_with("tour")) && self.splash_start.is_none() && !self.games.is_empty() {
            let k = (self.started.elapsed().as_millis() / 500) as usize;
            let n = self.games.len();
            let want = if k < 2 * n { k % n } else { 0 };
            if want != self.sel {
                self.select(want);
            }
        }
        self.tick_pages();
        self.social_tick();
        self.calls_tick();
        self.idle.store(self.away_for_game, Ordering::Relaxed);
        self.poll_tray();
        self.on_close_requested(ctx);
        for cmd in self.pending_cmd.drain(..) {
            ctx.send_viewport_cmd(cmd);
        }
        // while the on-screen keyboard is open, a physical keyboard types into it
        let typing = matches!(self.modal, Some(Modal::Keyboard(_)));
        self.input.text_mode = typing;
        if typing {
            self.type_from_keyboard(ctx);
        }
        self.input.test_mode = matches!(self.modal, Some(Modal::PadTest(_)));
        let own = self.own.load(Ordering::Relaxed);
        let front = !self.away() && !(own != 0 && crate::win::is_minimized(own));
        self.input.focused = front;
        self.update_ambient(front);
        let mut navs = self.input.poll(ctx);
        if front {
            navs.extend(self.mouse(ctx, frame));
        }
        if self.input.test_mode {
            self.pad_test_tick();
            ctx.request_repaint();
        }

        if ctx.input(|i| crate::input::key_pressed(i, egui::Key::F11)) {
            self.fullscreen = !self.fullscreen;
            self.fs_tries = 0;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        self.keep_fullscreen(ctx);
        if std::mem::take(&mut self.refocus) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        if let Some(start) = self.splash_start {
            let skip = !navs.is_empty() || ctx.input(|i| i.pointer.any_click());
            if skip || start.elapsed() > SPLASH {
                self.splash_start = None;
                self.started = Instant::now();
            }
        } else {
            for nav in navs {
                if std::env::var_os("OTIUM_NO_OPEN").is_some() {
                    eprintln!("nav {nav:?} page={} modal={}", self.pages.page, self.modal.is_some());
                }
                // the launch splash takes B / Esc to hide itself; everything else waits
                if self.launching.is_some() {
                    if nav == Nav::Back {
                        self.hide_launch();
                    }
                    continue;
                }
                self.handle(nav, frame);
            }
        }

        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(Color32::from_rgb(6, 8, 14))).show(ctx, |ui| {
            match self.splash_start {
                Some(start) => splash(ui, start.elapsed().as_secs_f32()),
                None => {
                    self.home(ui);
                    self.paint_launch(ui);
                }
            }
        });
        self.paint_overlay(ctx);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // the main window paints every pixel itself; only the overlay shows what's under it
        [0.0; 4]
    }
}

const SPLASH: Duration = Duration::from_millis(2200);

fn splash(ui: &egui::Ui, t: f32) {
    let rect = ui.max_rect();
    let p = ui.painter();
    let s = rect.height() / 1080.0;
    let c = rect.center();
    p.rect_filled(rect, 0.0, Color32::BLACK);

    let fade_out = ((SPLASH.as_secs_f32() - t) / 0.45).clamp(0.0, 1.0);
    let grow = ease_out((t / 1.1).min(1.0));
    let alpha = (t / 0.5).min(1.0) * fade_out;
    let blue = Color32::from_rgb(61, 123, 255);

    // background glow
    for i in 0..24 {
        let k = i as f32 / 24.0;
        p.circle_filled(c, 520.0 * s * (1.0 - k) * (0.6 + 0.4 * grow), blue.gamma_multiply(0.012 * alpha));
    }
    // drifting sparks
    for i in 0..40 {
        let fi = i as f32;
        let ang = fi * 2.399 + t * 0.15;
        let dist = (180.0 + (fi * 37.0) % 380.0) * s * (0.7 + 0.3 * grow);
        let pos = c + vec2(ang.cos(), ang.sin()) * dist + vec2(0.0, -t * 14.0 * s);
        let tw = 0.5 + 0.5 * (t * 3.0 + fi).sin();
        p.circle_filled(pos, (1.2 + (fi % 3.0)) * s, white(0.5 * tw * alpha));
    }
    // ring
    let r = 150.0 * s * (0.75 + 0.25 * grow);
    for i in 0..6 {
        let k = i as f32;
        p.circle_stroke(c, r + k * 3.0 * s, Stroke::new(6.0 * s, blue.gamma_multiply(0.09 * alpha * (6.0 - k) / 6.0)));
    }
    p.circle_stroke(c, r, Stroke::new(3.0 * s, white(0.9 * alpha)));

    let text_a = ((t - 0.45) / 0.6).clamp(0.0, 1.0) * fade_out;
    p.text(c + vec2(0.0, 2.0 * s), Align2::CENTER_CENTER, "OTIUM", semibold(54.0 * s), white(text_a));
    p.text(
        c + vec2(0.0, r + 70.0 * s),
        Align2::CENTER_CENTER,
        "Нажмите любую кнопку",
        font(20.0 * s),
        white(0.45 * text_a),
    );
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

impl App {
    fn home(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let s = (rect.height() / 1080.0).min(rect.width() / 1920.0 * 1.25);
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        let appear = ease_out((self.started.elapsed().as_secs_f32() / 0.5).min(1.0));

        let games_tab = self.tab() == Tab::Games;
        if games_tab {
            self.paint_background(ui, rect);
        } else {
            self.paint_tab_background(ui, rect);
        }
        self.paint_top_bar(ui, rect, s, appear);

        if !games_tab {
            self.paint_tab(ui, rect, s, dt, appear);
        } else if self.games.is_empty() {
            let p = ui.painter();
            p.text(rect.center() - vec2(0.0, 30.0 * s), Align2::CENTER_CENTER, "Библиотека пуста", semibold(40.0 * s), white(appear));
            let add = if self.pad_kind() == PadKind::PlayStation { "□" } else { "X" };
            p.text(
                rect.center() + vec2(0.0, 24.0 * s),
                Align2::CENTER_CENTER,
                format!("Нажмите {add} или N, чтобы добавить игры"),
                font(22.0 * s),
                white(0.65 * appear),
            );
        } else {
            let oy = -self.pages.pos * rect.height();
            if oy > -rect.height() {
                self.paint_row(ui, rect, s, dt, appear, oy);
            }
            self.paint_pages(ui, rect, s, dt);
        }

        let hints: Vec<(usize, &str)> = match (&self.modal, self.pages.page) {
            (Some(m), _) => m.hints(),
            (None, _) if self.calls.card_open() => vec![],
            (None, _) if self.social.open => vec![(0, "Выбрать"), (2, "Меню друга"), (1, "Назад")],
            (None, 0) if self.reorder => vec![(0, "Готово")],
            (None, 0) if matches!(self.zone, Zone::Top(_)) => vec![(0, "Выбрать"), (1, "Назад")],
            (None, 0) if !games_tab => self.tab_hints(),
            (None, 0) => vec![(0, "Выбрать"), (3, "Добавить"), (2, "Параметры")],
            (None, _) => vec![(0, "Выбрать"), (1, "Назад")],
        };
        if self.social.open {
            self.paint_social(ui, rect, s);
        }
        self.paint_call_pill(ui, rect, s);
        if self.calls.card_open() {
            self.paint_call_card(ui, rect, s);
        }
        self.paint_hints(ui, rect, s, &hints);
        if self.modal.is_none() && !self.social.open && self.pages.page == 0 && self.splash_start.is_none() {
            self.paint_build_label(ui, rect, s);
        }

        if let Some(m) = self.modal.as_mut() {
            let a = ease_out((self.modal_at.elapsed().as_secs_f32() / 0.18).min(1.0));
            m.paint(ui, rect, s, dt, a);
            if matches!(m, Modal::Control { .. }) {
                self.paint_control(ui, rect, s, dt, a);
            } else if matches!(m, Modal::Friend { .. }) {
                self.paint_friend_profile(ui, rect, s, a);
            }
        }
        self.paint_toast(ui, rect, s);
    }

    fn paint_row(&mut self, ui: &egui::Ui, rect: Rect, s: f32, dt: f32, appear: f32, oy: f32) {
        // the selected tile stays at the left edge, the rest scroll past it
        let tile = 116.0 * s;
        let big = 164.0 * s;
        let gap = 20.0 * s;
        let left = 96.0 * s;
        let row_y = 150.0 * s + oy;
        let k = 1.0 - (-dt * 16.0).exp();
        self.scroll += (self.sel as f32 - self.scroll) * k;

        let in_row = self.zone == Zone::Row && self.modal.is_none() && self.pages.page == 0;
        for (i, g) in self.games.iter().enumerate() {
            let d = i as f32 - self.scroll;
            // the selected slot is wider, so tiles after it shift right by the difference
            let shift = d.clamp(0.0, 1.0) * (big - tile);
            let x = left + d * (tile + gap) + shift;
            if x > rect.right() || x + big < rect.left() - big {
                continue;
            }
            let focus = 1.0 - d.abs().min(1.0);
            let size = tile + (big - tile) * focus;
            let r = Rect::from_min_size(pos2(x, row_y), vec2(size, size));
            let fade = if d < -0.5 { (1.0 + d).max(0.0) } else { 1.0 } * appear;
            let covers = self.art.get(&g.id).map(|a| a.covers.as_slice()).unwrap_or(&[]);
            paint_tile(ui, g, covers, r, 14.0 * s, fade, i == self.sel && in_row, s);
            crate::mouse::add(r, crate::mouse::Hit::Game(i));
        }

        let g = &self.games[self.sel];
        let art = self.art(g);
        if in_row && self.reorder {
            // arrows on both sides of the tile being moved
            let x = left + (self.sel as f32 - self.scroll) * (tile + gap);
            let cy = row_y + big / 2.0;
            let p = ui.painter();
            // centred in the gaps next to the tile
            if self.sel > 0 {
                p.text(pos2(x - gap / 2.0, cy), Align2::CENTER_CENTER, "◀", bold(16.0 * s), white(appear));
            }
            if self.sel + 1 < self.games.len() {
                p.text(pos2(x + big + gap / 2.0, cy), Align2::CENTER_CENTER, "▶", bold(16.0 * s), white(appear));
            }
        }
        if in_row {
            ui.painter().text(
                pos2(left + big + gap + 4.0 * s, row_y + big + 6.0 * s),
                Align2::LEFT_TOP,
                &g.name,
                semibold(24.0 * s),
                white(0.95 * appear),
            );
        }

        // ---- title art, Play button, info ----
        let info_y = rect.height() * 0.50 + oy;
        let logo_box = Rect::from_min_size(pos2(left, info_y - 190.0 * s), vec2(560.0 * s, 180.0 * s));
        if draw::contain_left(ui, &art.logos, logo_box, white(appear)).is_none() {
            ui.painter().text(logo_box.left_bottom(), Align2::LEFT_BOTTOM, &g.name, bold(64.0 * s), white(appear));
        }

        let btn = Rect::from_min_size(pos2(left, info_y + 20.0 * s), vec2(280.0 * s, 68.0 * s));
        let no_modal = self.modal.is_none() && self.pages.page == 0;
        let play_focus = self.zone == Zone::Play && no_modal;
        let p = ui.painter();
        if play_focus {
            draw::glow(p, btn, 34.0 * s, Color32::WHITE, 18.0 * s, 0.6);
        }
        p.rect_filled(btn, CornerRadius::same((34.0 * s) as u8), if play_focus { Color32::WHITE } else { white(0.88) });
        p.text(btn.center(), Align2::CENTER_CENTER, if g.is_app() { "Открыть" } else { "Играть" }, semibold(26.0 * s), Color32::from_rgb(10, 12, 18));

        let more = Rect::from_center_size(pos2(btn.right() + 24.0 * s + 34.0 * s, btn.center().y), vec2(68.0 * s, 68.0 * s));
        crate::mouse::add(btn, crate::mouse::Hit::Play);
        crate::mouse::add(more, crate::mouse::Hit::More);
        let more_focus = self.zone == Zone::More && no_modal;
        if more_focus {
            draw::glow(p, more, 34.0 * s, Color32::WHITE, 18.0 * s, 0.6);
        }
        p.circle_filled(more.center(), 34.0 * s, if more_focus { white(0.95) } else { black(0.45) });
        p.circle_stroke(more.center(), 34.0 * s, Stroke::new(1.5_f32, white(0.5)));
        for dx in [-12.0, 0.0, 12.0] {
            p.circle_filled(more.center() + vec2(dx * s, 0.0), 3.5 * s, if more_focus { Color32::BLACK } else { Color32::WHITE });
        }

        let mut info = Vec::new();
        if self.is_running(g) {
            info.push("● Запущена".to_string());
        }
        let secs = self.playtime(g);
        if secs >= 60.0 {
            info.push(format_playtime(secs));
        }
        if g.steam_app_id().is_some() {
            info.push("Steam".to_string());
        } else if g.category() == "app" {
            info.push("Приложение".to_string());
        }
        if !info.is_empty() {
            p.text(pos2(left + 4.0 * s, btn.bottom() + 26.0 * s), Align2::LEFT_TOP, info.join("  ·  "), font(20.0 * s), white(0.7 * appear));
        }
    }

    fn paint_background(&self, ui: &egui::Ui, rect: Rect) {
        let p = ui.painter();
        let cur = self.games.get(self.sel).map(|g| self.art(g).backgrounds).unwrap_or_default();
        let t = (self.bg_changed.elapsed().as_secs_f32() / 0.45).min(1.0);
        let dim = Color32::from_gray(150);
        if t < 1.0 && !self.bg_prev.is_empty() {
            draw::cover(ui, &self.bg_prev, rect, 0.0, dim, (0.5, 0.3));
        }
        if !cur.is_empty() {
            let e = ease_out(t);
            let a = (e * 150.0) as u8;
            draw::cover(ui, &cur, rect, 0.0, Color32::from_rgba_unmultiplied(a, a, a, (e * 255.0) as u8), (0.5, 0.3));
        }
        // the pages below the home screen sit on a darker background
        let down = self.pages.pos.min(1.0);
        if down > 0.0 {
            p.rect_filled(rect, 0.0, black(0.6 * down));
        }
        // keep text readable: darken the top, the bottom and the left side
        let h = rect.height();
        draw::vgradient(p, Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + h * 0.3)), black(0.7), black(0.0));
        draw::vgradient(p, Rect::from_min_max(pos2(rect.left(), rect.top() + h * 0.4), rect.max), black(0.0), black(0.92));
        draw::hgradient(p, Rect::from_min_max(rect.min, pos2(rect.left() + rect.width() * 0.55, rect.bottom())), black(0.55), black(0.0));
    }

    fn paint_top_bar(&self, ui: &egui::Ui, rect: Rect, s: f32, a: f32) {
        let p = ui.painter();
        let y = 62.0 * s;
        let items = self.top_items();
        let focus = match self.zone {
            Zone::Top(i) if self.modal.is_none() && !self.social.open && self.pages.page == 0 => match items.get(i.min(items.len() - 1)) {
                Some(top::TopItem::Tab(t)) => Some(*t),
                _ => None,
            },
            _ => None,
        };
        for (r, (t, _)) in self.paint_tab_names(ui, s, a, focus).into_iter().zip(tabs::TABS) {
            if let Some(i) = items.iter().position(|x| *x == top::TopItem::Tab(t)) {
                crate::mouse::add(r, crate::mouse::Hit::Top(i));
            }
        }

        let right = rect.right() - 96.0 * s;
        let clock = chrono::Local::now().format("%H:%M").to_string();
        let c = p.text(pos2(right, y), Align2::RIGHT_CENTER, clock, font(28.0 * s), white(a));

        let user = self.lib.setting_str("user");
        let user = if user.is_empty() { "Игрок" } else { user };
        let av = pos2(c.left() - 40.0 * s, y);
        p.circle_filled(av, 22.0 * s, draw::name_color(user));
        p.text(av, Align2::CENTER_CENTER, draw::initials(user), semibold(17.0 * s), white(a));
        // unread messages and friend requests
        let unread = self.social_unread();
        if unread > 0 {
            let b = av + vec2(17.0 * s, -15.0 * s);
            p.circle_filled(b, 11.0 * s, Color32::from_rgb(237, 66, 69));
            p.text(b, Align2::CENTER_CENTER, unread.min(99).to_string(), semibold(13.0 * s), Color32::WHITE);
        }
        self.paint_top_items(ui, rect, s, a, av);
    }

    /// Hint bar: (button, label) where button 0 = confirm, 1 = back, 2 = options, 3 = add.
    fn paint_hints(&self, ui: &egui::Ui, rect: Rect, s: f32, hints: &[(usize, &str)]) {
        let glyphs: [&str; 4] = if self.pad_kind() == PadKind::PlayStation { ["✕", "○", "△", "□"] } else { ["A", "B", "Y", "X"] };
        let p = ui.painter();
        let mut x = rect.right() - 96.0 * s;
        let y = rect.bottom() - 54.0 * s;
        for (button, label) in hints.iter().rev() {
            let t = p.text(pos2(x, y), Align2::RIGHT_CENTER, *label, font(19.0 * s), white(0.75));
            let c = pos2(t.left() - 22.0 * s, y);
            draw::pad_glyph(p, c, 14.0 * s, glyphs[*button], Color32::WHITE);
            x = c.x - 40.0 * s;
        }
    }

    fn paint_toast(&mut self, ui: &egui::Ui, rect: Rect, s: f32) {
        let Some(t) = &self.toast else { return };
        let age = t.at.elapsed().as_secs_f32();
        if age > 3.2 {
            self.toast = None;
            return;
        }
        let a = (age / 0.2).min(1.0) * ((3.2 - age) / 0.4).min(1.0);
        let p = ui.painter();
        let galley = p.layout_no_wrap(t.text.clone(), font(21.0 * s), white(a));
        let size = galley.size() + vec2(48.0 * s, 28.0 * s);
        let box_ = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 130.0 * s + (1.0 - a) * 20.0 * s), size);
        let bg = if t.error { Color32::from_rgb(120, 24, 32) } else { Color32::from_rgb(28, 32, 44) };
        p.rect_filled(box_, CornerRadius::same((size.y / 2.0) as u8), bg.gamma_multiply(0.95 * a));
        p.rect_stroke(box_, CornerRadius::same((size.y / 2.0) as u8), Stroke::new(1.0_f32, white(0.15 * a)), StrokeKind::Inside);
        p.galley(box_.center() - galley.size() / 2.0, galley, Color32::WHITE);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_tile(ui: &egui::Ui, g: &Game, covers: &[String], r: Rect, radius: f32, alpha: f32, focused: bool, s: f32) {
    if alpha <= 0.01 {
        return;
    }
    let p = ui.painter();
    if focused {
        draw::glow(p, r, radius, Color32::WHITE, 16.0 * s, 0.55 * alpha);
    }
    if !draw::cover(ui, covers, r, radius, white(alpha), (0.5, 0.35)) {
        let cr = CornerRadius::same(radius as u8);
        p.rect_filled(r, cr, draw::name_color(&g.name).gamma_multiply(alpha));
        p.text(r.center(), Align2::CENTER_CENTER, draw::initials(&g.name), bold(r.height() * 0.32), white(0.9 * alpha));
    }
    if focused {
        p.rect_stroke(r, CornerRadius::same(radius as u8), Stroke::new(3.0 * s, white(alpha)), StrokeKind::Outside);
    }
}

fn format_playtime(secs: f64) -> String {
    let m = (secs / 60.0).floor() as i64;
    if m < 60 {
        format!("{} мин", m.max(1))
    } else {
        format!("{} ч {} мин", m / 60, m % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playtime_is_seconds() {
        assert_eq!(format_playtime(2100.0), "35 мин");
        assert_eq!(format_playtime(8340.0), "2 ч 19 мин");
    }

    #[test]
    fn plurals() {
        assert_eq!(plural(1, "игра", "игры", "игр"), "игра");
        assert_eq!(plural(3, "игра", "игры", "игр"), "игры");
        assert_eq!(plural(11, "игра", "игры", "игр"), "игр");
        assert_eq!(plural(22, "игра", "игры", "игр"), "игры");
    }

    #[test]
    fn names() {
        assert_eq!(name_from_path(std::path::Path::new("C:\\G\\my_cool-game.exe")), "my cool game");
    }
}

impl App {
    fn type_from_keyboard(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        let Some(Modal::Keyboard(k)) = self.modal.as_mut() else { return };
        self.last_text_target = Some(k.target);
        let mut done = None;
        for e in events {
            match e {
                egui::Event::Text(t) => k.type_text(&t),
                egui::Event::Paste(t) => k.type_text(&t),
                egui::Event::Key { key: egui::Key::Backspace, pressed: true, .. } => k.erase(),
                egui::Event::Key { key: egui::Key::Enter, pressed: true, .. } => done = Some(k.finish()),
                _ => {}
            }
        }
        if let Some(o) = done {
            self.text_done(o);
        }
    }
}

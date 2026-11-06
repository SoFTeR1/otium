//! What's behind the home screen. The same order as the Electron build: the game's own background,
//! then its art (unless "Общий фон" is chosen), then the shared backgrounds (pictures or videos,
//! changing on a timer), then its cover. Resting on a Steam game fades its trailer in, like a PS5.
//! Stored in the same fields: settings.bgMode, wallpapers, slideshow, liveTrailers.

use super::{App, Tab};
use crate::data::{image_uri, Game, Library};
use crate::draw;
use crate::modal::{Modal, SetItem, SetKey, SetMenu};
use crate::video::{is_video, Player};
use crate::watch::Event;
use egui::load::SizedTexture;
use egui::{pos2, Color32, Rect, TextureHandle};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Resting on a game this long starts its trailer.
const TRAILER_AFTER: Duration = Duration::from_millis(2500);
const FADE_IN: f32 = 0.6;
const FADE_OUT: f32 = 0.35;
/// OTIUM out of sight this long: the video leaves memory (until then it's only paused).
const UNLOAD_AFTER: Duration = Duration::from_secs(30);
const SLIDES: [(u64, &str); 5] = [(0, "Выкл"), (30, "Каждые 30 сек"), (60, "Каждую минуту"), (300, "Каждые 5 мин"), (900, "Каждые 15 мин")];
const IMAGE_EXT: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];
pub(super) const VIDEO_EXT: [&str; 7] = ["mp4", "webm", "mkv", "mov", "m4v", "avi", "wmv"];

pub struct Bg {
    player: Option<Player>,
    tex: Option<TextureHandle>,
    /// the film bars of the video on `tex`, kept while its last frame fades out
    bars: f32,
    /// when the current video's first frame appeared (fading in)
    shown_at: Option<Instant>,
    /// when the video was told to go (its last frame is fading out)
    out_at: Option<Instant>,
    /// when OTIUM went out of sight
    hidden_at: Option<Instant>,
    slide: usize,
    slide_at: Instant,
    /// Steam app id -> its trailer file (None: there is none), for this session
    trailers: HashMap<String, Option<PathBuf>>,
    fetching: HashSet<String>,
    /// files that turned out unplayable: the picture under them stays
    failed: HashSet<PathBuf>,
}

impl Default for Bg {
    fn default() -> Self {
        Self {
            player: None,
            tex: None,
            bars: 0.0,
            shown_at: None,
            out_at: None,
            hidden_at: None,
            slide: 0,
            slide_at: Instant::now(),
            trailers: HashMap::new(),
            fetching: HashSet::new(),
            failed: HashSet::new(),
        }
    }
}

/// The local file behind a stored background (`file:///D:/a%20b.mp4` or a plain path).
fn local_path(url: &str) -> Option<PathBuf> {
    let uri = image_uri(url)?;
    uri.strip_prefix("file://").map(|p| PathBuf::from(p.replace('/', "\\")))
}

impl App {
    fn wallpapers(&self) -> Vec<String> {
        let list = self.lib.settings.get("wallpapers").and_then(Value::as_array);
        match list {
            Some(l) => l.iter().filter_map(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string).collect(),
            // the single background of old versions
            None => Some(self.lib.setting_str("wallpaper")).filter(|s| !s.is_empty()).map(str::to_string).into_iter().collect(),
        }
    }

    fn current_wallpaper(&self) -> Option<String> {
        let w = self.wallpapers();
        (!w.is_empty()).then(|| w[self.bg.slide % w.len()].clone())
    }

    fn global_mode(&self) -> bool {
        self.lib.setting_str("bgMode") == "global"
    }

    /// Pictures for the screen behind `g`, best first (ones that fail to load are skipped).
    pub(super) fn bg_stills(&self, g: Option<&Game>) -> Vec<String> {
        let art = g.map(|g| self.art(g)).unwrap_or_default();
        let mut out = Vec::new();
        if let Some(c) = g.and_then(Game::custom_background).filter(|c| !is_video(c)) {
            out.extend(image_uri(c));
        }
        if !self.global_mode() {
            out.extend(art.heroes.iter().cloned());
        }
        if let Some(w) = self.current_wallpaper().filter(|w| !is_video(w)) {
            out.extend(image_uri(&w));
        }
        out.extend(art.covers.iter().cloned());
        let mut seen = HashSet::new();
        out.retain(|u| seen.insert(u.to_lowercase()));
        out
    }

    /// The video that should play behind `g` right now, if any. May start fetching its trailer.
    fn wanted_video(&mut self, g: Option<&Game>) -> Option<PathBuf> {
        if let Some(c) = g.and_then(Game::custom_background) {
            return is_video(c).then(|| local_path(c)).flatten();
        }
        let art = g.map(|g| self.art(g)).unwrap_or_default();
        if !self.global_mode() && !art.heroes.is_empty() {
            // the game's art is on screen: its trailer replaces it after a moment
            let id = g.and_then(Game::steam_app_id)?;
            if !self.lib.setting_bool("liveTrailers", true) || self.bg_changed.elapsed() < TRAILER_AFTER {
                return None;
            }
            if let Some(known) = self.bg.trailers.get(&id) {
                return known.clone();
            }
            if self.bg.fetching.insert(id.clone()) {
                let bus = self.bus.clone();
                std::thread::spawn(move || {
                    let path = crate::trailer::fetch(&id);
                    bus.send(Event::Trailer { appid: id, path });
                });
            }
            return None;
        }
        self.current_wallpaper().filter(|w| is_video(w)).and_then(|w| local_path(&w))
    }

    pub(super) fn on_trailer(&mut self, appid: String, path: Option<PathBuf>) {
        self.bg.fetching.remove(&appid);
        self.bg.trailers.insert(appid, path);
    }

    fn stop_video(&mut self) {
        if let Some(p) = &self.bg.player {
            self.bg.bars = p.bars();
        }
        if self.bg.player.take().is_some() && self.bg.tex.is_some() {
            self.bg.out_at = Some(Instant::now());
        }
        self.bg.shown_at = None;
    }

    /// Every frame: the slideshow, and which video runs.
    pub(super) fn tick_bg(&mut self, ctx: &egui::Context, front: bool) {
        // out of sight: paused, and after a while out of memory
        if !front {
            let since = *self.bg.hidden_at.get_or_insert_with(Instant::now);
            if let Some(p) = &self.bg.player {
                p.set_playing(false);
            }
            if since.elapsed() > UNLOAD_AFTER && (self.bg.player.is_some() || self.bg.tex.is_some()) {
                self.bg.player = None;
                self.bg.tex = None;
                self.bg.shown_at = None;
                self.bg.out_at = None;
            }
            return;
        }
        self.bg.hidden_at = None;
        if self.bg.out_at.is_some_and(|t| t.elapsed().as_secs_f32() >= FADE_OUT) {
            self.bg.out_at = None;
            if self.bg.player.is_none() {
                self.bg.tex = None;
            }
        }
        let on_home = self.tab() == Tab::Games && self.pages.page == 0 && !self.gallery.open;
        let free = on_home && self.splash_start.is_none() && self.launching.is_none();
        let selected = self.games.get(self.sel).cloned();
        self.tick_slides(selected.as_ref());

        let wanted = if free { self.wanted_video(selected.as_ref()).filter(|p| !self.bg.failed.contains(p)) } else { None };
        // pages and other tabs cover it: keep the place, don't decode
        if !free && !self.launching.is_some() && self.bg.player.is_some() {
            if let Some(p) = &self.bg.player {
                p.set_playing(false);
            }
            return;
        }
        let current = self.bg.player.as_ref().map(|p| p.path().to_path_buf());
        if wanted != current {
            self.stop_video();
            if let Some(path) = &wanted {
                let c = ctx.clone();
                self.bg.player = Some(Player::start(path, move || c.request_repaint()));
            }
        }
        let Some(p) = &self.bg.player else { return };
        p.set_playing(true);
        if p.failed() {
            let path = p.path().to_path_buf();
            eprintln!("background video can't be played: {path:?}");
            self.bg.failed.insert(path);
            self.stop_video();
            return;
        }
        if let Some(frame) = p.take() {
            match &mut self.bg.tex {
                Some(t) => t.set(frame, draw::TEX),
                None => self.bg.tex = Some(ctx.load_texture("bg-video", frame, draw::TEX)),
            }
            if self.bg.shown_at.is_none() {
                self.bg.shown_at = Some(Instant::now());
                self.bg.out_at = None;
            }
        }
    }

    fn tick_slides(&mut self, g: Option<&Game>) {
        let every = self.lib.settings.get("slideshow").and_then(Value::as_u64).unwrap_or(0);
        let n = self.wallpapers().len();
        if every == 0 || n < 2 || self.bg.slide_at.elapsed() < Duration::from_secs(every) {
            return;
        }
        self.bg.slide_at = Instant::now();
        let before = self.bg_stills(g);
        // any other one, at random
        let step = 1 + (uuid::Uuid::new_v4().as_u128() % (n as u128 - 1)) as usize;
        self.bg.slide = (self.bg.slide % n + step) % n;
        let after = self.bg_stills(g);
        if before.first() != after.first() {
            self.bg_prev = before;
            self.bg_changed = Instant::now();
        }
    }

    /// The video over the picture, in the same dimmed tone.
    pub(super) fn paint_bg_video(&self, ui: &egui::Ui, rect: Rect) {
        let Some(tex) = &self.bg.tex else { return };
        let k = match (self.bg.out_at, self.bg.shown_at) {
            (Some(t), _) => 1.0 - (t.elapsed().as_secs_f32() / FADE_OUT).min(1.0),
            (None, Some(t)) => (t.elapsed().as_secs_f32() / FADE_IN).min(1.0),
            (None, None) => return,
        };
        let size = tex.size_vec2();
        // film bars are cut off, whatever is left fills the screen
        let bars = self.bg.player.as_ref().map_or(self.bg.bars, |p| p.bars()).clamp(0.0, 0.25);
        let inner = (size.x, size.y * (1.0 - 2.0 * bars));
        let uv = draw::cover_uv(inner, rect, (0.5, 0.3));
        let squeeze = |y: f32| bars + y * (1.0 - 2.0 * bars);
        let uv = Rect::from_min_max(pos2(uv.min.x, squeeze(uv.min.y)), pos2(uv.max.x, squeeze(uv.max.y)));
        let a = (k * 170.0) as u8;
        egui::Image::from_texture(SizedTexture::new(tex.id(), size))
            .uv(uv)
            .tint(Color32::from_rgba_premultiplied(a, a, a, (k * 255.0) as u8))
            .paint_at(ui, rect);
    }

    // ---------- Настройки → Фоны ----------

    pub(super) fn bg_menu(&self) -> (String, Vec<SetItem>) {
        let row = |key, label: String, value: &str, danger| SetItem { key, label, value: value.into(), danger };
        let every = self.lib.settings.get("slideshow").and_then(Value::as_u64).unwrap_or(0);
        let slide = SLIDES.iter().find(|s| s.0 == every).map_or(SLIDES[0].1, |s| s.1);
        let mut items = vec![
            row(SetKey::BgMode, "Фон на играх".into(), if self.global_mode() { "Общий фон" } else { "Арт игры" }, false),
            row(SetKey::BgTrailers, "Живые фоны (трейлеры игр)".into(), if self.lib.setting_bool("liveTrailers", true) { "Вкл" } else { "Выкл" }, false),
            row(SetKey::BgSlide, "Смена общих фонов".into(), slide, false),
            row(SetKey::BgAdd, "Добавить общий фон…".into(), "картинки или видео", false),
        ];
        for (i, w) in self.wallpapers().iter().enumerate() {
            let kind = if is_video(w) { "видео" } else { "картинка" };
            items.push(row(SetKey::BgRemove, format!("Фон {} · {kind}", i + 1), "убрать", false));
        }
        ("Фоны".to_string(), items)
    }

    pub(super) fn bg_setting(&mut self, key: SetKey, focus: usize) {
        match key {
            SetKey::BgMode => {
                let v = if self.global_mode() { "game" } else { "global" };
                self.lib.settings.insert("bgMode".into(), v.into());
                self.bg_changed_now();
            }
            SetKey::BgTrailers => {
                let v = !self.lib.setting_bool("liveTrailers", true);
                self.lib.settings.insert("liveTrailers".into(), v.into());
            }
            SetKey::BgSlide => {
                let every = self.lib.settings.get("slideshow").and_then(Value::as_u64).unwrap_or(0);
                let i = SLIDES.iter().position(|s| s.0 == every).unwrap_or(0);
                self.lib.settings.insert("slideshow".into(), SLIDES[(i + 1) % SLIDES.len()].0.into());
                self.bg.slide_at = Instant::now();
            }
            SetKey::BgAdd => self.bg_add(),
            SetKey::BgRemove => {
                let mut w = self.wallpapers();
                // the first rows are the settings above
                if let Some(i) = focus.checked_sub(4).filter(|i| *i < w.len()) {
                    w.remove(i);
                    self.set_wallpapers(w);
                    self.bg.slide = 0;
                    self.bg_changed_now();
                }
            }
            _ => {}
        }
        self.save();
        self.open_settings(SetMenu::Backgrounds, focus);
    }

    fn bg_changed_now(&mut self) {
        self.bg_prev.clear();
        self.bg_changed = Instant::now();
    }

    fn set_wallpapers(&mut self, w: Vec<String>) {
        self.lib.settings.insert("wallpapers".into(), w.into());
    }

    fn bg_add(&mut self) {
        let all: Vec<&str> = IMAGE_EXT.iter().chain(VIDEO_EXT.iter()).copied().collect();
        let Some(files) = self.dialog("Общие фоны").add_filter("Картинки и видео", &all).pick_files() else { return };
        let mut w = self.wallpapers();
        for f in files {
            match stored_background(&f) {
                Ok(u) => w.push(u),
                Err(e) => self.toast(&format!("Не удалось добавить фон: {e}"), true),
            }
        }
        // several backgrounds and no timer yet: they should change
        if w.len() > 1 && self.lib.settings.get("slideshow").and_then(Value::as_u64).unwrap_or(0) == 0 {
            self.lib.settings.insert("slideshow".into(), 60.into());
        }
        self.set_wallpapers(w);
        self.bg_changed_now();
        // the dialog took the menu's place in the window's attention, not in the state
        if !matches!(self.modal, Some(Modal::Settings { menu: SetMenu::Backgrounds, .. })) {
            self.open_settings(SetMenu::Backgrounds, 3);
        }
    }
}

/// What to store for a picked background: pictures are copied to the media folder; videos can be
/// huge, so they stay where they are and are referred to.
pub(super) fn stored_background(file: &Path) -> std::io::Result<String> {
    if is_video(&file.to_string_lossy()) {
        Ok(crate::data::file_url(&file.to_string_lossy()))
    } else {
        Library::import_media(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_links_lead_back_to_the_file() {
        assert_eq!(local_path("file:///D:/Видео/a%20b.mp4").unwrap(), PathBuf::from(r"D:\Видео\a b.mp4"));
        assert_eq!(local_path(r"D:\x\y.mp4").unwrap(), PathBuf::from(r"D:\x\y.mp4"));
        assert_eq!(local_path("https://x/y.mp4"), None);
        let u = crate::data::file_url(r"D:\Мои видео\фон 1.mp4");
        assert_eq!(local_path(&u).unwrap(), PathBuf::from(r"D:\Мои видео\фон 1.mp4"));
    }
}

//! Clips and screenshots: saving them (View + RB, L3 + R3, the keyboard, the Control Center), the
//! gallery in the Media tab, watching them, the on-screen folder chooser, and sending them to a
//! friend: the chat carries the offer, the file itself goes straight between the PCs.

use super::{App, PickFor};
use crate::clips::{self, Recorder, Saved};
use crate::data::{image_uri, Library};
use crate::draw::{self, bold, font, icon, semibold, white};
use crate::input::Nav;
use crate::modal::{Modal, SetItem, SetKey, SetMenu};
use crate::transfer::{self, Cmd, Transfer};
use crate::video::Player;
use crate::watch::Event;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind, TextureHandle};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

const COLS: usize = 4;
const CTRL: &str = "ps5l";
/// a friend who's online is asked for a waiting clip this often
const RETRY: Duration = Duration::from_secs(120);
const KEEP_WAITING_MS: i64 = 7 * 24 * 3600 * 1000;
const POSTER_WIDTH: u32 = 640;
const KEYS_HELP: &str = "View + RB или Ctrl+Alt+C — клип   ·   L3 + R3 или Ctrl+Alt+S — скриншот";

#[derive(Clone)]
pub struct Item {
    pub path: PathBuf,
    title: String,
    when: f64,
    size: u64,
    video: bool,
    from_friend: bool,
    /// what the tile shows; a video's still is made in the background
    poster: PathBuf,
}

enum FolderRow {
    Here,
    New,
    Up,
    Go(PathBuf),
}

/// A clip's sound, decoded at once and played on its own thread (audio output can't move threads).
struct Sound {
    ready: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl Sound {
    fn start(path: &Path) -> Self {
        let (ready, paused, stop) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (r, pa, st, path) = (ready.clone(), paused.clone(), stop.clone(), path.to_path_buf());
        std::thread::Builder::new()
            .name("clip-play".into())
            .spawn(move || {
                let pcm = crate::video::sound_of(&path);
                let out = rodio::OutputStream::try_default().ok();
                let sink = out.as_ref().and_then(|(_, h)| rodio::Sink::try_new(h).ok());
                if let Some(s) = &sink {
                    if !pcm.is_empty() {
                        s.append(rodio::buffer::SamplesBuffer::new(2, 48_000, pcm));
                    }
                }
                r.store(true, Ordering::Relaxed);
                while !st.load(Ordering::Relaxed) {
                    if let Some(s) = &sink {
                        if pa.load(Ordering::Relaxed) { s.pause() } else { s.play() }
                    }
                    std::thread::sleep(Duration::from_millis(30));
                }
            })
            .ok();
        Self { ready, paused, stop }
    }
}

impl Drop for Sound {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

struct Viewer {
    idx: usize,
    player: Option<Player>,
    sound: Option<Sound>,
    tex: Option<TextureHandle>,
    paused: bool,
    opened: Instant,
}

/// A clip a friend offered that hasn't come yet. Kept in the library: it can take days.
#[derive(Clone)]
struct Waiting {
    id: String,
    from: String,
    title: String,
    ext: String,
    size: u64,
    at: i64,
}

pub struct Gallery {
    pub open: bool,
    pub rec: Recorder,
    net: Transfer,
    items: Vec<Item>,
    sel: usize,
    scroll: f32,
    viewer: Option<Viewer>,
    /// picking a clip for this friend
    send_to: Option<String>,
    /// the file its menu was opened for
    menu_item: Option<PathBuf>,
    posters: Option<Sender<(PathBuf, PathBuf)>>,
    /// the folder chooser: where it is (None: the list of disks), what its rows do
    folder: Option<PathBuf>,
    rows: Vec<FolderRow>,
    last_clip: Option<Instant>,
    last_shot: Option<Instant>,
    /// transfers running now: id -> (friend, title)
    active: HashMap<String, (String, String)>,
    tried: HashMap<String, Instant>,
    checked: Instant,
}

impl Gallery {
    pub fn new(own: Arc<AtomicIsize>, bus: crate::watch::Bus) -> Self {
        Self {
            open: false,
            rec: Recorder::start(own),
            net: Transfer::start(move |e| bus.send(Event::Transfer(e))),
            items: Vec::new(),
            sel: 0,
            scroll: 0.0,
            viewer: None,
            send_to: None,
            menu_item: None,
            posters: None,
            folder: None,
            rows: Vec::new(),
            last_clip: None,
            last_shot: None,
            active: HashMap::new(),
            tried: HashMap::new(),
            checked: Instant::now(),
        }
    }

    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// A clip is playing with its sound: the background music keeps quiet.
    pub fn playing(&self) -> bool {
        self.viewer.as_ref().is_some_and(|v| v.player.is_some() && !v.paused)
    }

    pub fn viewing(&self) -> bool {
        self.viewer.is_some()
    }

    pub fn viewer_hints(&self) -> Vec<(usize, &'static str)> {
        match &self.viewer {
            Some(v) if v.player.as_ref().is_some_and(|p| p.ended()) => vec![(0, "Сначала"), (2, "Меню"), (1, "Назад")],
            Some(v) if v.player.is_some() && v.paused => vec![(0, "Играть"), (2, "Меню"), (1, "Назад")],
            Some(v) if v.player.is_some() || v.sound.is_some() => vec![(0, "Пауза"), (2, "Меню"), (1, "Назад")],
            _ => vec![(2, "Меню"), (1, "Назад")],
        }
    }
}

/// A file's title: its name without the date OTIUM added.
fn title_of(stem: &str) -> String {
    // "<title> 2026-11-06 20-50-23" and "... (2)"
    let cut = stem.rfind(" (").filter(|_| stem.ends_with(')')).map_or(stem, |i| &stem[..i]);
    const STAMP: usize = 20;
    let n = cut.chars().count();
    if n > STAMP {
        let (head, tail): (String, String) = (cut.chars().take(n - STAMP).collect(), cut.chars().skip(n - STAMP).collect());
        let looks = tail.starts_with(' ') && tail.chars().skip(1).all(|c| c.is_ascii_digit() || c == '-' || c == ' ');
        if looks {
            return head;
        }
    }
    stem.to_string()
}

fn poster_path(path: &Path, size: u64) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.to_string_lossy().to_lowercase().hash(&mut h);
    size.hash(&mut h);
    Library::data_dir().join("clips").join(format!("{:016x}.jpg", h.finish()))
}

fn scan_dir(dir: &Path, from_friend: bool, out: &mut Vec<Item>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let path = e.path();
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let ext = path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
        let video = ext == "mp4";
        if !(video || ext == "png" || ext == "jpg") || name.contains(".part.") {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        let when = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_millis() as f64);
        let stem = path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let poster = if video { poster_path(&path, meta.len()) } else { path.clone() };
        out.push(Item { title: title_of(&stem), when, size: meta.len(), video, from_friend, poster, path });
    }
}

fn megabytes(b: u64) -> String {
    let mb = b as f64 / 1_048_576.0;
    if mb >= 10.0 { format!("{mb:.0} МБ") } else { format!("{mb:.1} МБ") }
}

fn gigabytes(bytes: u64) -> String {
    format!("свободно {} ГБ", bytes / (1024 * 1024 * 1024))
}

/// A long path for a title: its end is what tells where one is.
fn short_path(p: &str) -> String {
    const MAX: usize = 44;
    let n = p.chars().count();
    if n <= MAX {
        return p.to_string();
    }
    format!("…{}", p.chars().skip(n - MAX).collect::<String>())
}

/// A picture in the middle of `area`, whole.
fn fit(size: egui::Vec2, area: Rect) -> Rect {
    let k = (area.width() / size.x.max(1.0)).min(area.height() / size.y.max(1.0));
    Rect::from_center_size(area.center(), size * k)
}

impl App {
    // ---------- recording ----------

    /// Off, in games (the default) or always.
    fn clip_mode(&self) -> &str {
        match self.lib.setting_str("clipMode") {
            "off" => "off",
            "always" => "always",
            _ => "games",
        }
    }

    pub(super) fn clip_mode_label(&self) -> &'static str {
        match self.clip_mode() {
            "off" => "Выкл",
            "always" => "Всегда",
            _ => "В играх",
        }
    }

    pub(super) fn clip_mode_next(&mut self) {
        let next = match self.clip_mode() {
            "games" => "always",
            "always" => "off",
            _ => "games",
        };
        self.lib.settings.insert("clipMode".into(), next.into());
        self.save();
    }

    /// A game has the screen. A Media service isn't one: it's a film, and usually protected anyway.
    fn game_on_screen(&self) -> bool {
        let service = self.service_window(self.game_hwnd);
        !self.running.is_empty() || self.steam_now != 0 || (self.game_hwnd != 0 && crate::win::is_window(self.game_hwnd) && !service)
    }

    /// What files get called.
    fn capture_title(&self, fallback: &str) -> String {
        match self.games.iter().find(|g| self.running.contains(&g.id)) {
            Some(g) => g.name.clone(),
            None => fallback.to_string(),
        }
    }

    pub(super) fn clip_save(&mut self) {
        if self.gallery.last_clip.is_some_and(|t| t.elapsed() < Duration::from_millis(1500)) {
            return;
        }
        self.gallery.last_clip = Some(Instant::now());
        if !self.gallery.rec.wanted() {
            let text = if self.clip_mode() == "off" { "Запись клипов выключена в настройках" } else { "Клипы записываются, пока идёт игра" };
            return self.toast(text, true);
        }
        let title = self.capture_title("Клип");
        let bus = self.bus.clone();
        self.gallery.rec.save_clip(&title, move |res| bus.send(Event::ClipSaved(res)));
    }

    pub(super) fn clip_shot(&mut self) {
        if self.gallery.last_shot.is_some_and(|t| t.elapsed() < Duration::from_millis(700)) {
            return;
        }
        self.gallery.last_shot = Some(Instant::now());
        let title = self.capture_title("Скриншот");
        let bus = self.bus.clone();
        self.gallery.rec.screenshot(&title, move |res| bus.send(Event::ClipSaved(res)));
    }

    pub(super) fn on_clip_saved(&mut self, res: Result<Saved, String>) {
        match res {
            Ok(s) => {
                if self.sounds_on() {
                    crate::sound::play(crate::sound::Sfx::Ok);
                }
                self.gallery_scan();
                // the new one is what's selected when the gallery opens
                self.gallery.sel = self.gallery.items.iter().position(|i| i.path == s.path).unwrap_or(0);
                let text = if s.secs > 0 { format!("Клип сохранён · {} сек", s.secs) } else { "Скриншот сохранён".to_string() };
                self.toast(&text, false);
            }
            Err(e) => self.toast(&e, true),
        }
    }

    /// A press of the combos on the pad or keyboard.
    pub(super) fn on_clip_key(&mut self, shot: bool) {
        if self.input.test_mode || self.splash_start.is_some() {
            return;
        }
        if shot {
            self.clip_shot();
        } else {
            self.clip_save();
        }
    }

    pub(super) fn tick_gallery(&mut self, ctx: &egui::Context) {
        let want = match self.clip_mode() {
            "off" => false,
            "always" => true,
            _ => self.game_on_screen(),
        };
        self.gallery.rec.set_wanted(want);
        for shot in self.input.take_clip_keys() {
            self.on_clip_key(shot);
        }
        // nobody is watching behind a game
        if self.gallery.viewing() && self.away() {
            self.gallery.viewer = None;
        }
        self.tick_viewer(ctx);
        self.tick_waiting();
    }

    // ---------- the gallery ----------

    pub(super) fn gallery_scan(&mut self) {
        let mut items = Vec::new();
        scan_dir(&clips::folder(), false, &mut items);
        scan_dir(&clips::received_folder(), true, &mut items);
        items.sort_by(|a, b| b.when.total_cmp(&a.when));
        for it in items.iter().filter(|i| i.video && !i.poster.exists()) {
            self.gallery_poster(it.path.clone(), it.poster.clone());
        }
        self.gallery.sel = self.gallery.sel.min(items.len().saturating_sub(1));
        self.gallery.items = items;
    }

    /// Video stills are made one at a time, on one thread: decoding is the slow part.
    fn gallery_poster(&mut self, path: PathBuf, out: PathBuf) {
        let tx = self.gallery.posters.get_or_insert_with(|| {
            let (tx, rx) = channel::<(PathBuf, PathBuf)>();
            std::thread::Builder::new()
                .name("clip-poster".into())
                .spawn(move || {
                    while let Ok((path, out)) = rx.recv() {
                        if !out.exists() && !crate::video::poster(&path, &out, POSTER_WIDTH) {
                            eprintln!("no poster for {path:?}");
                        }
                    }
                })
                .ok();
            tx
        });
        let _ = tx.send((path, out));
    }

    pub(super) fn open_gallery(&mut self) {
        self.gallery_scan();
        self.gallery.open = true;
        self.gallery.send_to = None;
        self.gallery.viewer = None;
    }

    pub(super) fn gallery_hints(&self) -> Vec<(usize, &'static str)> {
        if self.gallery.viewing() {
            return self.gallery.viewer_hints();
        }
        if self.gallery.send_to.is_some() {
            return vec![(0, "Отправить"), (1, "Назад")];
        }
        if self.gallery.items.is_empty() {
            return vec![(1, "Назад")];
        }
        vec![(0, "Смотреть"), (2, "Меню"), (1, "Назад")]
    }

    pub(super) fn gallery_hover(&mut self, i: usize) {
        if i < self.gallery.items.len() {
            self.gallery.sel = i;
        }
    }

    pub(super) fn gallery_nav(&mut self, nav: Nav) {
        if self.gallery.viewer.is_some() {
            return self.viewer_nav(nav);
        }
        let n = self.gallery.items.len();
        let sel = self.gallery.sel;
        match nav {
            Nav::Back => {
                self.gallery.open = false;
                self.gallery.send_to = None;
            }
            Nav::Left if sel > 0 => self.gallery.sel -= 1,
            Nav::Right if sel + 1 < n => self.gallery.sel += 1,
            Nav::Up if sel >= COLS => self.gallery.sel -= COLS,
            Nav::Down if sel + COLS < n => self.gallery.sel += COLS,
            Nav::Down if sel / COLS < n.saturating_sub(1) / COLS => self.gallery.sel = n - 1,
            Nav::Ok if n > 0 => match self.gallery.send_to.take() {
                Some(pk) => {
                    let path = self.gallery.items[sel].path.clone();
                    self.gallery.open = false;
                    self.clip_send(&path, &[pk]);
                }
                None => self.viewer_open(sel),
            },
            Nav::Options if n > 0 && self.gallery.send_to.is_none() => self.clip_menu(sel),
            _ => {}
        }
    }

    /// Opens the gallery to pick what to send to a friend (from their menu).
    pub(super) fn gallery_for_friend(&mut self, pk: &str) {
        self.open_gallery();
        if self.gallery.items.is_empty() {
            self.gallery.open = false;
            return self.open(Modal::Message { title: "Отправить клип".into(), text: format!("Клипов пока нет. {KEYS_HELP}") });
        }
        self.gallery.send_to = Some(pk.to_string());
    }

    fn clip_menu(&mut self, i: usize) {
        let Some(it) = self.gallery.items.get(i).cloned() else { return };
        self.gallery.menu_item = Some(it.path.clone());
        let row = |key, label: &str, value: String, danger| SetItem { key, label: label.into(), value, danger };
        let items = vec![
            row(SetKey::ClipSend, "Отправить другу", megabytes(it.size), false),
            row(SetKey::ClipShow, "Показать в папке", String::new(), false),
            row(SetKey::ClipDelete, "Удалить", String::new(), true),
        ];
        self.open(Modal::Settings { menu: SetMenu::Clip, title: it.title, items, focus: 0, scroll: 0.0 });
    }

    pub(super) fn clip_setting(&mut self, key: SetKey) {
        let Some(path) = self.gallery.menu_item.clone() else { return };
        match key {
            SetKey::ClipSend => {
                self.modal = None;
                if self.social.me_pk().is_none() {
                    return self.toast("Сначала создайте аккаунт в «Друзья и сообщения»", true);
                }
                self.open_picker("Кому отправить", PickFor::SendClip(path), &[], 5);
            }
            SetKey::ClipShow => {
                self.modal = None;
                crate::win::show_in_folder(&path);
            }
            SetKey::ClipDelete => self.open(Modal::Confirm {
                title: "Удалить навсегда?".into(),
                text: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                yes: SetKey::ClipDelete,
                yes_label: "Удалить",
                focus: 0,
            }),
            _ => {}
        }
    }

    pub(super) fn clip_delete_confirmed(&mut self) {
        let Some(path) = self.gallery.menu_item.take() else { return };
        // the file can't go while it's being played
        let viewed = self.gallery.viewer.as_ref().map(|v| v.idx);
        self.gallery.viewer = None;
        let poster = self.gallery.items.iter().find(|i| i.path == path).filter(|i| i.video).map(|i| i.poster.clone());
        // the player lets go of the file a moment after it's told to stop
        let mut res = std::fs::remove_file(&path);
        for _ in 0..5 {
            if res.is_ok() || viewed.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            res = std::fs::remove_file(&path);
        }
        match res {
            Ok(()) => {
                if let Some(p) = poster {
                    let _ = std::fs::remove_file(p);
                }
                self.toast("Удалено", false);
            }
            Err(e) => self.toast(&format!("Не удалось удалить: {e}"), true),
        }
        self.gallery_scan();
        if let Some(i) = viewed {
            let last = self.gallery.items.len().saturating_sub(1);
            if !self.gallery.items.is_empty() {
                self.viewer_open(i.min(last));
            }
        }
    }

    // ---------- watching ----------

    fn viewer_open(&mut self, i: usize) {
        let Some(it) = self.gallery.items.get(i) else { return };
        let sound = it.video.then(|| Sound::start(&it.path));
        self.gallery.sel = i;
        self.gallery.viewer = Some(Viewer { idx: i, player: None, sound, tex: None, paused: false, opened: Instant::now() });
    }

    fn viewer_nav(&mut self, nav: Nav) {
        let Some(v) = self.gallery.viewer.as_mut() else { return };
        let idx = v.idx;
        match nav {
            Nav::Back => self.gallery.viewer = None,
            Nav::Left if idx > 0 => self.viewer_open(idx - 1),
            Nav::Right if idx + 1 < self.gallery.items.len() => self.viewer_open(idx + 1),
            Nav::Ok => {
                if v.player.as_ref().is_some_and(|p| p.ended()) {
                    return self.viewer_open(idx);
                }
                v.paused = !v.paused;
                if let Some(p) = &v.player {
                    p.set_playing(!v.paused);
                }
                if let Some(s) = &v.sound {
                    s.paused.store(v.paused, Ordering::Relaxed);
                }
            }
            Nav::Options => self.clip_menu(idx),
            _ => {}
        }
    }

    fn tick_viewer(&mut self, ctx: &egui::Context) {
        let path = {
            let Some(v) = self.gallery.viewer.as_ref() else { return };
            self.gallery.items.get(v.idx).map(|i| i.path.clone())
        };
        let Some(path) = path else { return };
        let Some(v) = self.gallery.viewer.as_mut() else { return };
        // the picture starts when the sound can: decoding that takes a moment
        if v.player.is_none() && v.sound.as_ref().is_some_and(|s| s.ready.load(Ordering::Relaxed) || v.opened.elapsed() > Duration::from_secs(2)) {
            let c = ctx.clone();
            v.player = Some(Player::start_once(&path, move || c.request_repaint()));
        }
        if let Some(frame) = v.player.as_ref().and_then(|p| p.take()) {
            match &mut v.tex {
                Some(t) => t.set(frame, draw::TEX),
                None => v.tex = Some(ctx.load_texture("clip-view", frame, draw::TEX)),
            }
        }
    }

    // ---------- the folder chooser ----------

    pub(super) fn folder_value(&self) -> String {
        short_path(&clips::folder().display().to_string())
    }

    pub(super) fn folder_open(&mut self) {
        // from where it is now, if that's there
        self.gallery.folder = Some(clips::folder()).filter(|d| d.is_dir());
        self.modal_at = Instant::now();
        self.folder_show();
    }

    /// Fills the chooser with what's at its current place.
    pub(super) fn folder_show(&mut self) {
        let row = |label: String, value: String| SetItem { key: SetKey::FolderRow, label, value, danger: false };
        let (mut rows, mut items) = (Vec::new(), Vec::new());
        let title = match self.gallery.folder.clone() {
            None => {
                for (root, name) in crate::win::drives() {
                    let label = if name.is_empty() { root.display().to_string() } else { format!("{} {name}", root.display()) };
                    items.push(row(label, crate::win::disk_free(&root).map(gigabytes).unwrap_or_default()));
                    rows.push(FolderRow::Go(root));
                }
                "Куда сохранять клипы и скриншоты?".to_string()
            }
            Some(dir) => {
                items.push(row("Сохранять сюда".into(), crate::win::disk_free(&dir).map(gigabytes).unwrap_or_default()));
                rows.push(FolderRow::Here);
                items.push(row("Новая папка…".into(), String::new()));
                rows.push(FolderRow::New);
                items.push(row("‹ Наверх".into(), String::new()));
                rows.push(FolderRow::Up);
                for sub in clips::subfolders(&dir) {
                    items.push(row(sub.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), String::new()));
                    rows.push(FolderRow::Go(sub));
                }
                short_path(&dir.display().to_string())
            }
        };
        self.gallery.rows = rows;
        self.modal = Some(Modal::Settings { menu: SetMenu::Folder, title, items, focus: 0, scroll: 0.0 });
    }

    pub(super) fn folder_row(&mut self, i: usize) {
        match self.gallery.rows.get(i) {
            Some(FolderRow::Here) => self.folder_chosen(),
            Some(FolderRow::New) => self.keyboard("Новая папка", "", crate::keyboard::Target::NewFolder, 60, false),
            Some(FolderRow::Up) => self.folder_up(),
            Some(FolderRow::Go(p)) => {
                self.gallery.folder = Some(p.clone());
                self.folder_show();
            }
            None => {}
        }
    }

    /// B: one level up; from the list of disks, back to the settings.
    pub(super) fn folder_up(&mut self) {
        match self.gallery.folder.take() {
            None => self.settings_at(SetKey::ClipFolder),
            Some(d) => {
                self.gallery.folder = d.parent().map(PathBuf::from);
                self.folder_show();
            }
        }
    }

    pub(super) fn folder_create(&mut self, name: &str) {
        let Some(dir) = self.gallery.folder.clone() else { return self.folder_show() };
        if !clips::folder_name_ok(name) {
            self.toast("Такое имя папки Windows не примет", true);
            return self.folder_show();
        }
        let new = dir.join(name.trim());
        match std::fs::create_dir_all(&new) {
            Ok(()) => self.gallery.folder = Some(new),
            Err(e) => self.toast(&format!("Не удалось создать папку: {e}"), true),
        }
        self.folder_show();
    }

    fn folder_chosen(&mut self) {
        let Some(dir) = self.gallery.folder.clone() else { return };
        // a disk or a system folder may not let us write: better to know now than at the best moment of a game
        let probe = dir.join(".otium-write-test");
        if std::fs::write(&probe, b"").is_err() {
            return self.toast("В эту папку нельзя записывать: выберите другую", true);
        }
        let _ = std::fs::remove_file(&probe);
        self.lib.settings.insert("clipsDir".into(), dir.to_string_lossy().to_string().into());
        self.save();
        clips::set_folder(Some(dir.clone()));
        self.gallery_scan();
        self.toast(&format!("Клипы и скриншоты: {}", short_path(&dir.display().to_string())), false);
        self.settings_at(SetKey::ClipFolder);
    }

    /// The main settings with this row in focus.
    fn settings_at(&mut self, key: SetKey) {
        self.open_settings(SetMenu::Main, 0);
        if let Some(Modal::Settings { items, focus, .. }) = self.modal.as_mut() {
            *focus = items.iter().position(|i| i.key == key).unwrap_or(0);
        }
    }

    // ---------- sending ----------

    fn clip_ctrl(&self, pk: &str, t: &str, extra: Value) {
        let name = self.account().map(|a| a.name).unwrap_or_default();
        let mut body = json!({ "pl": CTRL, "t": t, "n": name });
        if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
            b.extend(e.clone());
        }
        self.social.send_raw(pk, body.to_string());
    }

    /// Offers a file to friends: the chat says so, and whoever is online asks for it right away.
    pub(super) fn clip_send(&mut self, path: &Path, pks: &[String]) {
        let Ok(meta) = std::fs::metadata(path) else { return self.toast("Файла больше нет", true) };
        if meta.len() > transfer::MAX_SIZE {
            return self.toast("Файл слишком большой для отправки", true);
        }
        let ext = path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
        let stem = path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let title = title_of(&stem);
        for pk in pks {
            let id = uuid::Uuid::new_v4().simple().to_string();
            let offers = self.lib.extra.entry("clipOffers").or_insert_with(|| json!({}));
            if let Some(o) = offers.as_object_mut() {
                o.insert(id.clone(), json!({ "path": path.to_string_lossy(), "to": pk, "at": now_ms() }));
                let week_ago = now_ms() - KEEP_WAITING_MS;
                o.retain(|_, v| v["at"].as_i64().unwrap_or(0) > week_ago);
            }
            self.clip_ctrl(pk, "clip", json!({ "id": id, "title": title, "size": meta.len(), "ext": ext }));
        }
        self.save();
        self.toast("Предложено другу: файл передаётся напрямую, пока вы оба в OTIUM", false);
    }

    /// "clip": a line in the chat on both sides; for the one it's meant for, something to fetch.
    pub(super) fn on_clip_offer(&mut self, id: String, other: String, mine: bool, c: &Value, at: i64) {
        let title = c["title"].as_str().unwrap_or("Клип").to_string();
        let size = c["size"].as_u64().unwrap_or(0);
        let ext = c["ext"].as_str().unwrap_or("").to_string();
        let clip_id = c["id"].as_str().unwrap_or("").to_string();
        let what = if ext == "mp4" { "🎬 Клип" } else { "🖼 Скриншот" };
        let tail = if mine { "" } else { " · он будет в «Медиа → Клипы и скриншоты»" };
        self.chat_line(id, other.clone(), mine, format!("{what} «{title}» · {}{tail}", megabytes(size)), at);
        let is_friend = self.account().is_some_and(|a| a.is_friend(&other));
        // only what the app itself sends: the extension picks where a file goes and how it opens
        let ok = !clip_id.is_empty() && matches!(ext.as_str(), "mp4" | "png" | "jpg") && size > 0 && size <= transfer::MAX_SIZE;
        if mine || !is_friend || !ok || now_ms() - at > KEEP_WAITING_MS {
            return;
        }
        let list = self.lib.extra.entry("clipsWaiting").or_insert_with(|| json!([]));
        if let Some(l) = list.as_array_mut() {
            if !l.iter().any(|x| x["id"] == clip_id.as_str()) {
                l.push(json!({ "id": clip_id, "from": other, "title": title, "ext": ext, "size": size, "at": at }));
            }
        }
        self.save();
    }

    fn waiting(&self) -> Vec<Waiting> {
        let list = self.lib.extra.get("clipsWaiting").and_then(Value::as_array);
        list.map(|l| {
            l.iter()
                .filter_map(|v| {
                    Some(Waiting {
                        id: v["id"].as_str()?.into(),
                        from: v["from"].as_str()?.into(),
                        title: v["title"].as_str().unwrap_or("Клип").into(),
                        ext: v["ext"].as_str()?.into(),
                        size: v["size"].as_u64()?,
                        at: v["at"].as_i64().unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
    }

    fn waiting_done(&mut self, id: &str) {
        if let Some(l) = self.lib.extra.get_mut("clipsWaiting").and_then(Value::as_array_mut) {
            l.retain(|x| x["id"] != id);
        }
        self.save();
    }

    /// Asks online friends for what they offered. Runs a few times a minute.
    fn tick_waiting(&mut self) {
        if self.gallery.checked.elapsed() < Duration::from_secs(3) || self.social.me_pk().is_none() {
            return;
        }
        self.gallery.checked = Instant::now();
        for w in self.waiting() {
            if now_ms() - w.at > KEEP_WAITING_MS {
                self.waiting_done(&w.id);
                continue;
            }
            let recent = self.gallery.tried.get(&w.id).is_some_and(|t| t.elapsed() < RETRY);
            if self.gallery.active.contains_key(&w.id) || recent || self.gallery.active.len() >= 2 {
                continue;
            }
            // the very first try doesn't wait for their status to arrive
            if self.gallery.tried.contains_key(&w.id) && !self.presence_of(&w.from).0 {
                continue;
            }
            self.gallery.tried.insert(w.id.clone(), Instant::now());
            self.gallery.active.insert(w.id.clone(), (w.from.clone(), w.title.clone()));
            let path = clips::received_path(&w.title, &w.ext);
            self.gallery.net.send(Cmd::Fetch { id: w.id, path, size: w.size });
        }
    }

    /// "clipreq": a friend asks for what we offered them, and for nothing else.
    pub(super) fn on_clip_request(&mut self, other: &str, c: &Value, at: i64) {
        // an old request replayed by the relays: nobody is waiting on it any more
        if now_ms() - at > 120_000 {
            return;
        }
        let (Some(id), Some(offer)) = (c["id"].as_str(), c["sdp"].as_str()) else { return };
        let entry = self.lib.extra.get("clipOffers").and_then(|o| o.get(id));
        let Some(path) = entry.filter(|e| e["to"] == other).and_then(|e| e["path"].as_str()).map(PathBuf::from) else { return };
        if !path.is_file() {
            return;
        }
        self.gallery.active.insert(id.to_string(), (other.to_string(), String::new()));
        self.gallery.net.send(Cmd::Serve { id: id.to_string(), path, offer: offer.to_string() });
    }

    /// "clipans": the answer to our request.
    pub(super) fn on_clip_answer(&mut self, other: &str, c: &Value, at: i64) {
        if now_ms() - at > 120_000 {
            return;
        }
        let (Some(id), Some(sdp)) = (c["id"].as_str(), c["sdp"].as_str()) else { return };
        if self.gallery.active.get(id).is_some_and(|(pk, _)| pk == other) {
            self.gallery.net.send(Cmd::Accepted { id: id.to_string(), sdp: sdp.to_string() });
        }
    }

    pub(super) fn on_transfer(&mut self, e: transfer::Event) {
        match e {
            transfer::Event::Sdp { id, sdp, offer } => {
                if let Some((pk, _)) = self.gallery.active.get(&id).cloned() {
                    self.clip_ctrl(&pk, if offer { "clipreq" } else { "clipans" }, json!({ "id": id, "sdp": sdp }));
                }
            }
            transfer::Event::Received { id, path } => {
                let who = self.gallery.active.remove(&id).map(|(pk, _)| pk);
                self.waiting_done(&id);
                self.gallery_scan();
                let name = who.and_then(|pk| self.account().map(|a| (self.display_name(&a, &pk), pk)));
                let what = if path.extension().is_some_and(|x| x == "mp4") { "Клип получен — он в «Медиа»" } else { "Скриншот получен — он в «Медиа»" };
                match name {
                    Some((n, pk)) => self.notify(&n.clone(), what, (pk, n)),
                    None => self.toast(what, false),
                }
            }
            transfer::Event::Sent { id } => {
                let who = self.gallery.active.remove(&id).map(|(pk, _)| pk);
                let name = who.and_then(|pk| self.account().map(|a| self.display_name(&a, &pk))).unwrap_or_else(|| "Друг".into());
                self.toast(&format!("{name} получил(а) файл"), false);
            }
            transfer::Event::Failed { id, begun } => {
                let was = self.gallery.active.remove(&id);
                // nobody answering is normal (they'll be asked again); a broken transfer isn't
                if begun && was.is_some() {
                    self.toast("Передача файла оборвалась — попробуем ещё раз позже", true);
                }
            }
        }
    }

    // ---------- painting ----------

    pub(super) fn paint_gallery(&mut self, ui: &egui::Ui, rect: Rect, s: f32, dt: f32) {
        if self.gallery.viewer.is_some() {
            return self.paint_viewer(ui, rect, s);
        }
        let p = ui.painter();
        p.rect_filled(rect, 0.0, Color32::from_rgb(8, 10, 16));
        let left = 96.0 * s;
        let area = Rect::from_min_max(pos2(left, 118.0 * s), pos2(rect.right() - left, rect.bottom() - 96.0 * s));
        let items = &self.gallery.items;
        if items.is_empty() {
            p.text(rect.center() - vec2(0.0, 30.0 * s), Align2::CENTER_CENTER, "Пока пусто", semibold(36.0 * s), Color32::WHITE);
            p.text(rect.center() + vec2(0.0, 24.0 * s), Align2::CENTER_CENTER, KEYS_HELP, font(21.0 * s), white(0.65));
        }
        let gap = 20.0 * s;
        let w = (area.width() - gap * (COLS as f32 - 1.0)) / COLS as f32;
        let pic_h = w * 9.0 / 16.0;
        let h = pic_h + 62.0 * s;
        let row = self.gallery.sel / COLS;
        let rows_fit = ((area.height() + gap) / (h + gap)).floor().max(1.0) as usize;
        let target = row.saturating_sub(rows_fit - 1) as f32 * (h + gap);
        self.gallery.scroll += (target - self.gallery.scroll) * (1.0 - (-dt * 16.0).exp());
        let free_modal = self.modal.is_none();
        for (i, it) in items.iter().enumerate() {
            let x = area.left() + (i % COLS) as f32 * (w + gap);
            let y = area.top() + (i / COLS) as f32 * (h + gap) - self.gallery.scroll;
            let r = Rect::from_min_size(pos2(x, y), vec2(w, h));
            if r.bottom() < area.top() || r.top() > area.bottom() {
                continue;
            }
            let f = free_modal && i == self.gallery.sel;
            let pic = Rect::from_min_size(r.min, vec2(w, pic_h));
            let radius = 12.0 * s;
            let cr = CornerRadius::same(radius as u8);
            if f {
                draw::glow(p, pic, radius, Color32::WHITE, 14.0 * s, 0.5);
            }
            p.rect_filled(pic, cr, white(0.07));
            let uri = it.poster.exists().then(|| image_uri(&it.poster.to_string_lossy())).flatten();
            let drawn = uri.is_some_and(|u| draw::cover(ui, &[u], pic, radius, Color32::WHITE, (0.5, 0.5)));
            if !drawn {
                p.text(pic.center(), Align2::CENTER_CENTER, if it.video { draw::ic::VIDEO } else { draw::ic::CAMERA }, icon(40.0 * s), white(0.3));
            }
            // what it is, in the corner
            let badge = Rect::from_min_size(pic.min + vec2(10.0 * s, 10.0 * s), vec2(34.0 * s, 34.0 * s));
            p.rect_filled(badge, CornerRadius::same((8.0 * s) as u8), Color32::from_black_alpha(170));
            p.text(badge.center(), Align2::CENTER_CENTER, if it.video { draw::ic::VIDEO } else { draw::ic::CAMERA }, icon(18.0 * s), Color32::WHITE);
            if f {
                p.rect_stroke(pic, cr, Stroke::new(3.0 * s, Color32::WHITE), StrokeKind::Outside);
            }
            let text = Rect::from_min_max(pos2(r.left(), pic.bottom()), r.max);
            let clip = p.with_clip_rect(text.intersect(area));
            clip.text(pos2(r.left() + 2.0 * s, pic.bottom() + 20.0 * s), Align2::LEFT_CENTER, &it.title, semibold(19.0 * s), white(if f { 1.0 } else { 0.85 }));
            let mut sub = format!("{} · {}", crate::app::fmt_date(it.when), megabytes(it.size));
            if it.from_friend {
                sub.push_str(" · от друга");
            }
            clip.text(pos2(r.left() + 2.0 * s, pic.bottom() + 45.0 * s), Align2::LEFT_CENTER, sub, font(15.0 * s), white(0.55));
            crate::mouse::add(r.intersect(area), crate::mouse::Hit::Gallery(i));
        }
        // the title and the hint bar stay above what scrolls under them
        let bg = Color32::from_rgb(8, 10, 16);
        p.rect_filled(Rect::from_min_max(rect.min, pos2(rect.right(), area.top())), 0.0, bg);
        p.rect_filled(Rect::from_min_max(pos2(rect.left(), area.bottom()), rect.max), 0.0, bg);
        let title = match &self.gallery.send_to {
            Some(pk) => format!("Что отправить — {}", self.account().map(|a| self.display_name(&a, pk)).unwrap_or_default()),
            None => "Клипы и скриншоты".to_string(),
        };
        p.text(pos2(left, 64.0 * s), Align2::LEFT_CENTER, title, bold(34.0 * s), Color32::WHITE);
        if !self.gallery.items.is_empty() {
            p.text(pos2(rect.right() - left, 64.0 * s), Align2::RIGHT_CENTER, KEYS_HELP, font(17.0 * s), white(0.55));
        }
    }

    fn paint_viewer(&mut self, ui: &egui::Ui, rect: Rect, s: f32) {
        let p = ui.painter();
        p.rect_filled(rect, 0.0, Color32::BLACK);
        let Some(v) = self.gallery.viewer.as_ref() else { return };
        let Some(it) = self.gallery.items.get(v.idx) else { return };
        if let Some(t) = &v.tex {
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            p.image(t.id(), fit(t.size_vec2(), rect), uv, Color32::WHITE);
        } else if !it.video {
            if let Some(uri) = image_uri(&it.path.to_string_lossy()) {
                if let Some(t) = draw::texture(ui.ctx(), &[uri], rect.width()) {
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                    p.image(t.id, fit(t.size, rect), uv, Color32::WHITE);
                }
            }
        }
        let failed = v.player.as_ref().is_some_and(|x| x.failed());
        if failed {
            p.text(rect.center(), Align2::CENTER_CENTER, "Этот файл не открывается", font(26.0 * s), white(0.7));
        } else if v.paused {
            let c = rect.center();
            p.circle_filled(c, 54.0 * s, Color32::from_black_alpha(170));
            p.text(c, Align2::CENTER_CENTER, draw::ic::PLAY, icon(42.0 * s), Color32::WHITE);
        }
        draw::vgradient(p, Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 130.0 * s)), Color32::from_black_alpha(190), Color32::TRANSPARENT);
        draw::vgradient(p, Rect::from_min_max(pos2(rect.left(), rect.bottom() - 110.0 * s), rect.max), Color32::TRANSPARENT, Color32::from_black_alpha(190));
        p.text(pos2(64.0 * s, 44.0 * s), Align2::LEFT_CENTER, &it.title, semibold(26.0 * s), Color32::WHITE);
        p.text(pos2(64.0 * s, 78.0 * s), Align2::LEFT_CENTER, format!("{} · {} · {} из {}", crate::app::fmt_date(it.when), megabytes(it.size), v.idx + 1, self.gallery.items.len()), font(17.0 * s), white(0.65));
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_lose_the_date() {
        assert_eq!(title_of("UFC 5 2026-11-06 20-50-23"), "UFC 5");
        assert_eq!(title_of("UFC 5 2026-11-06 20-50-23 (2)"), "UFC 5");
        assert_eq!(title_of("мой клип"), "мой клип");
        assert_eq!(title_of("2026-11-06 20-50-23"), "2026-11-06 20-50-23");
    }

    #[test]
    fn files_of_the_gallery() {
        let d = std::env::temp_dir().join(format!("otium-gallery-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for f in ["a 2026-11-06 20-50-23.mp4", "b.png", "c.jpg", "half.part.mp4", "half.part", "note.txt"] {
            std::fs::write(d.join(f), "x").unwrap();
        }
        let mut out = Vec::new();
        scan_dir(&d, false, &mut out);
        let mut names: Vec<&str> = out.iter().map(|i| i.title.as_str()).collect();
        names.sort();
        assert_eq!(names, ["a", "b", "c"]);
        assert_eq!(out.iter().filter(|i| i.video).count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn long_paths_keep_their_end() {
        assert_eq!(short_path(r"D:\Клипы"), r"D:\Клипы");
        let long = format!(r"D:\{}\Клипы", "a".repeat(60));
        let s = short_path(&long);
        assert!(s.starts_with('…') && s.ends_with("Клипы") && s.chars().count() == 45);
    }

    #[test]
    fn pictures_fit_whole() {
        let r = fit(vec2(1920.0, 1080.0), Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 1000.0)));
        assert_eq!(r.width(), 1000.0);
        assert!((r.height() - 562.5).abs() < 0.01);
        assert_eq!(r.center().y, 500.0);
    }
}

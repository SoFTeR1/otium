//! The Control Center (Guide button / View+Start / Ctrl+Alt+Home), like the PS5's and the
//! Electron build's: a row of tiles along the bottom of the screen — the running game, the call,
//! music, friends, the volume mixer, the shutdown timer, settings and power.

use super::calls::CallState;
use super::App;
use crate::draw::{self, bold, font, ic, icon, semibold, white};
use crate::input::Nav;
use crate::media::MediaCmd;
use crate::modal::{Modal, SetItem, SetKey, SetMenu};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const RED: Color32 = Color32::from_rgb(239, 68, 68);
const DARK: Color32 = Color32::from_rgb(10, 12, 18);
const ACCENT: Color32 = Color32::from_rgb(61, 123, 255);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CtlAction {
    ReturnToGame,
    CloseGame,
    AcceptCall,
    DeclineCall,
    OpenCall,
    CallMic,
    CallSound,
    CallMenu,
    HangUp,
    Home,
    MusicToggle,
    MusicPrev,
    MusicNext,
    Social,
    Mixer,
    Timer,
    Settings,
    Minimize,
    Power,
}

enum Pic {
    Uris(Vec<String>),
    Glyph(&'static str),
    Letter(String, String),
}

struct Wide {
    pic: Pic,
    kicker: String,
    title: String,
    sub: String,
}

struct Tile {
    act: CtlAction,
    glyph: &'static str,
    label: String,
    /// shown above the tile while it has the focus
    hint: String,
    danger: bool,
    /// a switched-off control (muted mic): its icon goes red
    off: bool,
    wide: Option<Wide>,
}

fn tile(act: CtlAction, glyph: &'static str, label: &str, hint: &str) -> Tile {
    Tile { act, glyph, label: label.into(), hint: hint.into(), danger: false, off: false, wide: None }
}

// ---------- shutdown timer ----------

#[derive(Default)]
pub struct SleepTimer {
    pub at: Option<Instant>,
    /// sleep instead of shutting down
    pub sleep: bool,
    /// warnings already given: 5 and 1 minutes before
    warned: [bool; 2],
    /// Windows was told to shut down in a minute; "Отменить" aborts it
    pub pending: bool,
}

impl SleepTimer {
    pub fn left(&self) -> Option<Duration> {
        self.at.map(|t| t.saturating_duration_since(Instant::now()))
    }

    fn what(&self) -> &'static str {
        if self.sleep { "перейдёт в сон" } else { "выключится" }
    }
}

/// "45:03", "1:05:00"
pub fn fmt_left(d: Duration) -> String {
    let s = d.as_secs_f64().ceil() as u64;
    let (h, m, ss) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{ss:02}") } else { format!("{m}:{ss:02}") }
}

/// What the user typed for "Своё время": "45" = in 45 minutes, "1:30" = in an hour and a half,
/// "23:30" = at 23:30 (today, or tomorrow if that has passed). Minutes from `now`.
pub fn parse_timer(v: &str, now: chrono::NaiveDateTime) -> Option<i64> {
    let v: String = v.trim().chars().map(|c| if matches!(c, '.' | ',' | ' ') { ':' } else { c }).collect();
    if !v.is_empty() && v.len() <= 4 && v.chars().all(|c| c.is_ascii_digit()) {
        return v.parse().ok().filter(|m| *m > 0);
    }
    let (h, m) = v.split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 || !h.chars().chain(m.chars()).all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (hh, mm): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
    if mm > 59 {
        return None;
    }
    // one-digit hours ("1:30") are a duration; two digits ("01:30", "23:30") a time of day
    if h.len() == 1 {
        return Some(hh * 60 + mm).filter(|m| *m > 0);
    }
    if hh > 23 {
        return None;
    }
    let mut at = now.date().and_hms_opt(hh as u32, mm as u32, 0)?;
    if at <= now {
        at += chrono::Duration::days(1);
    }
    Some(((at - now).num_seconds() as f64 / 60.0).round() as i64)
}

// ---------- volume mixer ----------

pub struct MixerPanel {
    rows: Vec<MixRow>,
    focus: usize,
    scroll: f32,
    error: Option<String>,
}

struct MixRow {
    master: bool,
    name: String,
    pids: Vec<u32>,
    icon: Option<PathBuf>,
    volume: i32,
    muted: bool,
}

impl MixerPanel {
    fn load() -> Self {
        match crate::mixer::list() {
            Ok(m) => {
                let mut rows = vec![MixRow { master: true, name: "Общая громкость".into(), pids: vec![], icon: None, volume: m.master, muted: m.master_muted }];
                for a in m.apps {
                    let icon = a.exe.as_deref().map(app_icon);
                    rows.push(MixRow { master: false, name: a.name, pids: a.pids, icon, volume: a.volume, muted: a.muted });
                }
                Self { rows, focus: 0, scroll: 0.0, error: None }
            }
            Err(e) => Self { rows: vec![], focus: 0, scroll: 0.0, error: Some(format!("Не удалось получить громкость приложений: {e}")) },
        }
    }

    pub fn set_focus(&mut self, i: usize) {
        if i < self.rows.len() {
            self.focus = i;
        }
    }

    fn nav(&mut self, nav: Nav) {
        let n = self.rows.len();
        let Some(r) = self.rows.get_mut(self.focus) else { return };
        match nav {
            Nav::Up if self.focus > 0 => self.focus -= 1,
            Nav::Down if self.focus + 1 < n => self.focus += 1,
            Nav::Left | Nav::Right => {
                r.volume = (r.volume + if nav == Nav::Right { 5 } else { -5 }).clamp(0, 100);
                // turning it up un-mutes, like the Windows mixer
                if r.muted && nav == Nav::Right {
                    r.muted = false;
                    set_mute(r);
                }
                if r.master {
                    crate::mixer::set_master(r.volume);
                } else {
                    crate::mixer::set_app(&r.pids, r.volume);
                }
            }
            Nav::Ok => {
                r.muted = !r.muted;
                set_mute(r);
            }
            _ => {}
        }
    }
}

fn set_mute(r: &MixRow) {
    if r.master {
        crate::mixer::set_master_mute(r.muted);
    } else {
        crate::mixer::set_app_mute(&r.pids, r.muted);
    }
}

/// The app's exe icon as a PNG in the temp folder, made in the background the first time;
/// painted once the file is there.
fn app_icon(exe: &std::path::Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    exe.to_string_lossy().to_lowercase().hash(&mut h);
    let dir = std::env::temp_dir().join("otium-icons");
    let out = dir.join(format!("{:016x}.png", h.finish()));
    if !out.exists() {
        let (exe, out) = (exe.to_path_buf(), out.clone());
        std::thread::spawn(move || {
            let _ = std::fs::create_dir_all(&dir);
            let tmp = out.with_extension("tmp");
            // written aside and renamed, so a half-written file is never painted (and cached as broken)
            if crate::win::exe_icon_png(&exe, &tmp) {
                let _ = std::fs::rename(&tmp, &out);
            }
        });
    }
    out
}

impl App {
    pub(super) fn open_control(&mut self) {
        let _ = self.media_tx.send(MediaCmd::Poll);
        self.open(Modal::Control { focus: 0, scroll: 0.0, mixer: None });
    }

    fn running_game(&self) -> Option<&crate::data::Game> {
        self.games.iter().find(|g| self.running.contains(&g.id))
    }

    fn control_tiles(&self) -> Vec<Tile> {
        let mut t = Vec::new();
        if let Some(g) = self.running_game() {
            let covers = self.art.get(&g.id).map(|a| a.covers.clone()).unwrap_or_default();
            t.push(Tile {
                wide: Some(Wide { pic: Pic::Uris(covers), kicker: "Сейчас запущено".into(), title: g.name.clone(), sub: "Вернуться в игру".into() }),
                ..tile(CtlAction::ReturnToGame, ic::GAME, "", "Вернуться в игру")
            });
            t.push(Tile { danger: true, ..tile(CtlAction::CloseGame, ic::CLOSE, "Закрыть", &format!("Закрыть «{}»", g.name)) });
        } else if self.has_game() {
            // a game OTIUM didn't start: the window it was summoned over
            t.push(tile(CtlAction::ReturnToGame, ic::BACK, "Вернуться", "Вернуться в игру"));
            t.push(Tile { danger: true, ..tile(CtlAction::CloseGame, ic::CLOSE, "Закрыть игру", "Закрыть окно игры") });
        }

        // calls: an incoming one first, then whatever call or voice room is on
        let me = self.me_pk();
        if let Some(r) = &self.calls.ring {
            if let Some(g) = self.group(&r.gid) {
                let title = g.title(&me);
                t.push(Tile {
                    wide: Some(Wide { pic: Pic::Letter(g.id.clone(), title.clone()), kicker: "Зовут в голосовой".into(), title, sub: self.member_name(&g, &r.from) }),
                    ..tile(CtlAction::AcceptCall, ic::PHONE, "", "Войти в голосовой")
                });
                t.push(Tile { danger: true, ..tile(CtlAction::DeclineCall, ic::HANG_UP, "Не сейчас", "Не входить") });
            }
        } else if self.calls.call.state == CallState::Incoming {
            let name = self.account().map(|a| self.display_name(&a, &self.calls.call.pk)).unwrap_or_default();
            t.push(Tile {
                wide: Some(Wide { pic: Pic::Letter(self.calls.call.pk.clone(), name.clone()), kicker: "Входящий звонок".into(), title: name, sub: "Принять".into() }),
                ..tile(CtlAction::AcceptCall, ic::PHONE, "", "Принять звонок")
            });
            t.push(Tile { danger: true, ..tile(CtlAction::DeclineCall, ic::HANG_UP, "Отклонить", "Отклонить звонок") });
        }
        let call = if let Some(r) = &self.calls.room {
            self.group(&r.gid).map(|g| {
                let n = r.peers.values().filter(|p| p.connected).count() + 1;
                (Pic::Letter(g.id.clone(), g.title(&me)), format!("Голосовой · {n}"), g.title(&me), "открыть группу".to_string(), r.muted, r.deafened)
            })
        } else if matches!(self.calls.call.state, CallState::Active | CallState::Connecting | CallState::Outgoing) {
            let c = &self.calls.call;
            let name = self.account().map(|a| self.display_name(&a, &c.pk)).unwrap_or_default();
            let sub = match c.state {
                CallState::Active => c.started.map(|s| super::calls::mmss(s.elapsed())).unwrap_or_default(),
                CallState::Connecting => "соединяем…".into(),
                _ => "звоним…".into(),
            };
            Some((Pic::Letter(c.pk.clone(), name.clone()), "Звонок".to_string(), name, sub, c.muted, c.deafened))
        } else {
            None
        };
        if let Some((pic, kicker, title, sub, muted, deafened)) = call {
            t.push(Tile { wide: Some(Wide { pic, kicker, title, sub }), ..tile(CtlAction::OpenCall, ic::PHONE, "", "Открыть звонок") });
            t.push(Tile { off: muted, ..tile(CtlAction::CallMic, if muted { ic::MIC_OFF } else { ic::MIC }, "", if muted { "Включить микрофон" } else { "Выключить микрофон" }) });
            t.push(Tile { off: deafened, ..tile(CtlAction::CallSound, if deafened { ic::MUTE } else { ic::HEADPHONE }, "", if deafened { "Включить звук" } else { "Выключить звук" }) });
            t.push(tile(CtlAction::CallMenu, ic::SETTINGS, "", "Настройки звонка"));
            let hang = if self.calls.room.is_some() { "Выйти из голосового" } else { "Завершить звонок" };
            t.push(Tile { danger: true, ..tile(CtlAction::HangUp, ic::HANG_UP, "", hang) });
        }

        t.push(tile(CtlAction::Home, ic::HOME, "Главная", "Главный экран"));

        if let Some(m) = &self.pages.music {
            let pic = m.art.clone().map(|a| Pic::Uris(vec![a])).unwrap_or(Pic::Glyph(ic::MUSIC));
            let toggle = if m.playing { "Пауза" } else { "Играть" };
            t.push(Tile {
                wide: Some(Wide { pic, kicker: crate::media::app_name(&m.app), title: m.title.clone(), sub: m.artist.clone() }),
                ..tile(CtlAction::MusicToggle, ic::MUSIC, "", toggle)
            });
            t.push(tile(CtlAction::MusicPrev, ic::PREV, "", "Предыдущий трек"));
            t.push(tile(CtlAction::MusicToggle, if m.playing { ic::PAUSE } else { ic::PLAY }, "", toggle));
            t.push(tile(CtlAction::MusicNext, ic::NEXT, "", "Следующий трек"));
        }

        let unread = self.social_unread();
        let friends = if unread > 0 { format!("Друзья · {unread}") } else { "Друзья".into() };
        t.push(tile(CtlAction::Social, ic::CHAT, &friends, "Друзья и сообщения"));
        t.push(tile(CtlAction::Mixer, ic::VOLUME, "Звук", "Громкость приложений"));
        match self.timer.left() {
            Some(left) => t.push(Tile { off: true, ..tile(CtlAction::Timer, ic::TIMER, &fmt_left(left), &format!("Таймер: ПК {} через {}", self.timer.what(), fmt_left(left))) }),
            None if self.timer.pending => t.push(Tile { danger: true, ..tile(CtlAction::Timer, ic::TIMER, "Отменить", "Отменить выключение") }),
            None => t.push(tile(CtlAction::Timer, ic::TIMER, "Таймер", "Таймер выключения")),
        }
        t.push(tile(CtlAction::Settings, ic::SETTINGS, "Настройки", "Настройки"));
        t.push(tile(CtlAction::Minimize, ic::MINIMIZE, "Свернуть", "Свернуть OTIUM"));
        t.push(tile(CtlAction::Power, ic::POWER, "Питание", "Сон, перезагрузка, выключение"));
        t
    }

    /// Gamepad / keyboard while the Control Center is open. Back or Up closes it.
    pub(super) fn control_nav(&mut self, nav: Nav) {
        let tiles = self.control_tiles();
        let Some(Modal::Control { focus, mixer, .. }) = self.modal.as_mut() else { return };
        if let Some(m) = mixer {
            if nav == Nav::Back {
                *mixer = None;
            } else {
                m.nav(nav);
            }
            return;
        }
        *focus = (*focus).min(tiles.len().saturating_sub(1));
        match nav {
            Nav::Left if *focus > 0 => *focus -= 1,
            Nav::Right if *focus + 1 < tiles.len() => *focus += 1,
            Nav::Back | Nav::Up => self.modal = None,
            Nav::Ok => {
                let act = tiles[*focus].act;
                self.control(act);
            }
            _ => {}
        }
        if std::env::var_os("OTIUM_NO_OPEN").is_some() {
            if let Some(Modal::Control { focus, .. }) = &self.modal {
                eprintln!("ctl focus {:?}", tiles.get(*focus).map(|t| t.act));
            }
        }
    }

    /// Something opened from the Control Center (settings, power, the timer, the call menu, the
    /// friends panel) goes back to it on B, like on the console; anything else ends that.
    pub(super) fn after_control_center(&mut self, nav: Nav, was_cc: bool) {
        let cc_open = matches!(self.modal, Some(Modal::Control { .. }));
        let elsewhere = self.modal.is_some() || self.social.open;
        if was_cc && !cc_open && elsewhere {
            // left the Control Center for a menu or panel: remember which tile
            return;
        }
        if cc_open || !elsewhere {
            if nav == Nav::Back && !cc_open {
                if let Some(focus) = self.cc_return.take() {
                    let _ = self.media_tx.send(MediaCmd::Poll);
                    self.open(Modal::Control { focus, scroll: 0.0, mixer: None });
                }
            }
            if !matches!(self.modal, Some(Modal::Control { .. })) {
                self.cc_return = None;
            }
        }
    }

    pub(super) fn control(&mut self, c: CtlAction) {
        if let Some(Modal::Control { focus, .. }) = &self.modal {
            self.cc_return = Some(*focus);
        }
        let close = |app: &mut App| app.modal = None;
        match c {
            CtlAction::ReturnToGame => self.back_to_game(),
            CtlAction::CloseGame => {
                close(self);
                self.close_game(None);
            }
            CtlAction::AcceptCall => {
                close(self);
                match self.calls.ring.as_ref().map(|r| r.gid.clone()) {
                    Some(gid) => self.join_room(&gid),
                    None => self.accept_call(),
                }
            }
            CtlAction::DeclineCall => {
                if self.calls.ring.is_some() {
                    self.stop_group_ring();
                } else {
                    self.decline_call();
                }
            }
            CtlAction::OpenCall => match self.calls.room.as_ref().map(|r| r.gid.clone()) {
                Some(gid) => {
                    close(self);
                    self.social.open_chat(&format!("g:{gid}"));
                }
                None => self.open_call_menu(0),
            },
            CtlAction::CallMic => self.toggle_mute(),
            CtlAction::CallSound => self.toggle_deafen(),
            CtlAction::CallMenu => self.open_call_menu(0),
            CtlAction::HangUp => {
                if self.calls.room.is_some() {
                    self.leave_room();
                } else {
                    self.hang_up();
                }
            }
            CtlAction::Home => {
                close(self);
                self.social.open = false;
            }
            CtlAction::MusicToggle => {
                let _ = self.media_tx.send(MediaCmd::Toggle);
            }
            CtlAction::MusicPrev => {
                let _ = self.media_tx.send(MediaCmd::Prev);
            }
            CtlAction::MusicNext => {
                let _ = self.media_tx.send(MediaCmd::Next);
            }
            CtlAction::Social => self.open_social(),
            CtlAction::Mixer => {
                if let Some(Modal::Control { mixer, .. }) = self.modal.as_mut() {
                    *mixer = Some(Box::new(MixerPanel::load()));
                }
            }
            CtlAction::Timer if self.timer.pending => {
                self.cancel_shutdown();
            }
            CtlAction::Timer => self.open_settings(SetMenu::Timer, 0),
            CtlAction::Settings => self.open_settings(SetMenu::Main, 0),
            CtlAction::Minimize => {
                close(self);
                self.pending_cmd.push(egui::ViewportCommand::Minimized(true));
            }
            CtlAction::Power => self.open_settings(SetMenu::Power, 0),
        }
    }

    // ---------- shutdown timer ----------

    /// Rows of Питание → Таймер выключения.
    pub(super) fn timer_items(&self) -> (String, Vec<SetItem>) {
        let row = |key, label: &str, value: String, danger| SetItem { key, label: label.into(), value, danger };
        let mut items = vec![
            row(SetKey::Timer30, "Через 30 минут", String::new(), false),
            row(SetKey::Timer60, "Через 1 час", String::new(), false),
            row(SetKey::Timer90, "Через 1,5 часа", String::new(), false),
            row(SetKey::Timer120, "Через 2 часа", String::new(), false),
            row(SetKey::TimerCustom, "Своё время…", "45 · 1:30 · 23:30".into(), false),
            row(SetKey::TimerAction, "Действие", if self.timer.sleep { "Спящий режим" } else { "Выключить ПК" }.into(), false),
        ];
        if self.timer.at.is_some() {
            items.push(row(SetKey::TimerCancel, "Отменить таймер", String::new(), true));
        }
        if self.timer.pending {
            items.push(row(SetKey::TimerCancel, "Отменить выключение", String::new(), true));
        }
        let title = match self.timer.left() {
            Some(left) => format!("Таймер · ПК {} через {}", self.timer.what(), fmt_left(left)),
            None => "Таймер выключения".into(),
        };
        (title, items)
    }

    pub(super) fn timer_setting(&mut self, key: SetKey, focus: usize) {
        match key {
            SetKey::Timer30 => return self.start_timer(30),
            SetKey::Timer60 => return self.start_timer(60),
            SetKey::Timer90 => return self.start_timer(90),
            SetKey::Timer120 => return self.start_timer(120),
            SetKey::TimerCustom => {
                return self.keyboard("Через сколько минут или во сколько", "", crate::keyboard::Target::Timer, 5, false);
            }
            SetKey::TimerAction => self.timer.sleep = !self.timer.sleep,
            SetKey::TimerCancel => {
                self.modal = None;
                if self.timer.pending {
                    return self.cancel_shutdown();
                }
                self.timer.at = None;
                return self.toast("Таймер выключения отменён", false);
            }
            _ => {}
        }
        self.open_settings(SetMenu::Timer, focus);
    }

    pub(super) fn timer_text(&mut self, text: &str) {
        match parse_timer(text, chrono::Local::now().naive_local()) {
            Some(m) if m <= 24 * 60 => self.start_timer(m as u64),
            _ => {
                self.toast("Не понял время: напишите 45, 1:30 или 23:30", true);
                self.open_settings(SetMenu::Timer, 4);
            }
        }
    }

    fn start_timer(&mut self, minutes: u64) {
        self.modal = None;
        let left = Duration::from_secs(minutes * 60);
        self.timer.at = Some(Instant::now() + left);
        self.timer.warned = [false; 2];
        let clock = (chrono::Local::now() + chrono::Duration::minutes(minutes as i64)).format("%H:%M");
        let what = self.timer.what();
        self.toast(&format!("ПК {what} в {clock} (через {})", fmt_left(left)), false);
    }

    fn cancel_shutdown(&mut self) {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("shutdown").arg("/a").creation_flags(0x0800_0000).spawn();
        self.timer.pending = false;
        self.toast("Выключение отменено", false);
    }

    /// Warnings 5 and 1 minutes before, then the shutdown or sleep. Called every frame.
    pub(super) fn tick_timer(&mut self) {
        let Some(left) = self.timer.left() else { return };
        let what = self.timer.what();
        for (i, (min, label)) in [(5, "5 минут"), (1, "1 минуту")].into_iter().enumerate() {
            if left <= Duration::from_secs(min * 60) && !self.timer.warned[i] && left > Duration::ZERO {
                self.timer.warned = [true, i == 1];
                self.toast(&format!("ПК {what} через {label}. Отменить — Центр управления → Таймер"), false);
            }
        }
        if left > Duration::ZERO {
            return;
        }
        self.timer.at = None;
        if self.timer.sleep {
            super::settings::power_sleep();
        } else {
            // Windows shows its own "you're about to be signed out" notice for the last minute
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("shutdown")
                .args(["/s", "/t", "60", "/c", "OTIUM: таймер выключения. Отменить — Центр управления → Таймер"])
                .creation_flags(0x0800_0000)
                .spawn();
            self.timer.pending = true;
            self.toast("ПК выключится через минуту", false);
        }
    }

    // ---------- painting ----------

    pub(super) fn paint_control(&mut self, ui: &egui::Ui, rect: Rect, s: f32, dt: f32, appear: f32) {
        let tiles = self.control_tiles();
        let Some(Modal::Control { focus, scroll, mixer }) = self.modal.as_mut() else { return };
        *focus = (*focus).min(tiles.len().saturating_sub(1));
        let p = ui.painter();
        p.rect_filled(rect, 0.0, Color32::from_black_alpha((60.0 * appear) as u8));
        draw::vgradient(p, Rect::from_min_max(pos2(rect.left(), rect.bottom() - 520.0 * s), rect.max), Color32::TRANSPARENT, Color32::from_black_alpha((235.0 * appear) as u8));

        let h = 132.0 * s;
        let small = 116.0 * s;
        let wide = 400.0 * s;
        let gap = 12.0 * s;
        let margin = 64.0 * s;
        let top = rect.bottom() - 112.0 * s - h + (1.0 - appear) * 40.0 * s;
        let widths: Vec<f32> = tiles.iter().map(|t| if t.wide.is_some() { wide } else { small }).collect();
        let total: f32 = widths.iter().sum::<f32>() + gap * (tiles.len().saturating_sub(1)) as f32;
        let lefts: Vec<f32> = widths.iter().scan(0.0, |x, w| {
            let l = *x;
            *x += w + gap;
            Some(l)
        }).collect();
        // centred when it fits; otherwise scrolled so the focused tile stays on screen
        let room = rect.width() - 2.0 * margin;
        let target = if total <= room {
            -(rect.width() - total) / 2.0 + rect.left()
        } else {
            let f = *focus;
            let (l, r) = (lefts[f], lefts[f] + widths[f]);
            let cur = *scroll;
            let mut want = cur;
            if l - want < margin + 1.5 * small {
                want = l - margin - 1.5 * small;
            }
            if r - want > rect.width() - margin - 1.5 * small {
                want = r - rect.width() + margin + 1.5 * small;
            }
            want.clamp(-margin, total - rect.width() + margin)
        };
        if (*scroll - target).abs() > rect.width() {
            *scroll = target;
        }
        *scroll += (target - *scroll) * (1.0 - (-dt * 16.0).exp());
        let x0 = rect.left() - *scroll;

        let mixer_x;
        {
            let mut mx = rect.center().x;
            for (i, t) in tiles.iter().enumerate() {
                let r = Rect::from_min_size(pos2(x0 + lefts[i], top), vec2(widths[i], h));
                if t.act == CtlAction::Mixer {
                    mx = r.center().x;
                }
                if r.right() < rect.left() || r.left() > rect.right() {
                    continue;
                }
                paint_tile(ui, t, r, s, i == *focus && mixer.is_none(), appear);
                crate::mouse::add(r, crate::mouse::Hit::Cc(i));
            }
            mixer_x = mx;
        }

        // what the focused tile does, above it
        if let Some(t) = tiles.get(*focus).filter(|_| mixer.is_none()) {
            let r = Rect::from_min_size(pos2(x0 + lefts[*focus], top), vec2(widths[*focus], h));
            let g = p.layout_no_wrap(t.hint.clone(), semibold(19.0 * s), white(appear));
            let x = (r.center().x - g.size().x / 2.0).clamp(rect.left() + margin, rect.right() - margin - g.size().x);
            p.galley(pos2(x, top - 18.0 * s - g.size().y), g, Color32::WHITE);
        }

        // the shutdown timer at the top right of the bar (the clock is in the top bar already)
        if let Some(left) = self.timer.left() {
            p.text(
                pos2(rect.right() - margin, top - 58.0 * s),
                Align2::RIGHT_BOTTOM,
                format!("ПК {} через {}", self.timer.what(), fmt_left(left)),
                font(17.0 * s),
                Color32::from_rgb(255, 170, 120).gamma_multiply(appear),
            );
        }

        if let Some(m) = mixer.as_mut() {
            paint_mixer(ui, rect, s, dt, appear, m, mixer_x, top - 24.0 * s);
        }
    }
}

fn paint_tile(ui: &egui::Ui, t: &Tile, r: Rect, s: f32, focused: bool, a: f32) {
    let p = ui.painter();
    let r = if focused { r.expand(4.0 * s) } else { r };
    let radius = CornerRadius::same((16.0 * s) as u8);
    let (bg, fg) = match (focused, t.danger) {
        (true, false) => (white(a), DARK),
        (true, true) => (RED.gamma_multiply(a), Color32::WHITE),
        (false, true) => (Color32::from_rgba_unmultiplied(239, 68, 68, (60.0 * a) as u8), Color32::from_rgb(255, 150, 150).gamma_multiply(a)),
        (false, false) => (white(0.09 * a), white(a)),
    };
    if focused {
        draw::glow(p, r, 16.0 * s, Color32::WHITE, 18.0 * s, 0.25 * a);
    }
    p.rect_filled(r, radius, bg);
    if !focused {
        p.rect_stroke(r, radius, Stroke::new(1.0_f32, white(0.08 * a)), StrokeKind::Inside);
    }
    let icon_color = if t.off { if focused { Color32::from_rgb(200, 30, 40) } else { Color32::from_rgb(255, 110, 110).gamma_multiply(a) } } else { fg };
    match &t.wide {
        None => {
            let cy = if t.label.is_empty() { r.center().y } else { r.top() + r.height() * 0.42 };
            p.text(pos2(r.center().x, cy), Align2::CENTER_CENTER, t.glyph, icon(38.0 * s), icon_color);
            if !t.label.is_empty() {
                p.text(pos2(r.center().x, r.bottom() - 20.0 * s), Align2::CENTER_CENTER, &t.label, font(15.0 * s), fg.gamma_multiply(0.9));
            }
        }
        Some(w) => {
            let side = r.height() - 24.0 * s;
            let pic = Rect::from_min_size(pos2(r.left() + 12.0 * s, r.top() + 12.0 * s), vec2(side, side));
            let pr = 10.0 * s;
            match &w.pic {
                Pic::Uris(uris) if !uris.is_empty() && draw::cover(ui, uris, pic, pr, white(a), (0.5, 0.5)) => {}
                Pic::Letter(key, name) => {
                    p.rect_filled(pic, CornerRadius::same(pr as u8), draw::name_color(key).gamma_multiply(a));
                    let l = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                    p.text(pic.center(), Align2::CENTER_CENTER, l, bold(44.0 * s), white(a));
                }
                Pic::Glyph(g) => {
                    p.rect_filled(pic, CornerRadius::same(pr as u8), white(0.1 * a));
                    p.text(pic.center(), Align2::CENTER_CENTER, *g, icon(40.0 * s), fg);
                }
                Pic::Uris(_) => {
                    p.rect_filled(pic, CornerRadius::same(pr as u8), white(0.1 * a));
                    p.text(pic.center(), Align2::CENTER_CENTER, t.glyph, icon(40.0 * s), fg);
                }
            }
            let tx = pic.right() + 16.0 * s;
            let clip = p.with_clip_rect(Rect::from_min_max(pos2(tx, r.top()), pos2(r.right() - 12.0 * s, r.bottom())));
            clip.text(pos2(tx, r.top() + 24.0 * s), Align2::LEFT_CENTER, &w.kicker, font(15.0 * s), fg.gamma_multiply(0.65));
            clip.text(pos2(tx, r.center().y), Align2::LEFT_CENTER, &w.title, semibold(21.0 * s), fg);
            clip.text(pos2(tx, r.bottom() - 24.0 * s), Align2::LEFT_CENTER, &w.sub, font(15.0 * s), fg.gamma_multiply(0.65));
        }
    }
}

const MIX_ROW: f32 = 64.0;
const MIX_VISIBLE: usize = 7;

#[allow(clippy::too_many_arguments)]
fn paint_mixer(ui: &egui::Ui, rect: Rect, s: f32, dt: f32, a: f32, m: &mut MixerPanel, cx: f32, bottom: f32) {
    let p = ui.painter();
    let w = 640.0 * s;
    let visible = m.rows.len().clamp(1, MIX_VISIBLE);
    let head = 64.0 * s;
    let h = head + visible as f32 * MIX_ROW * s + 16.0 * s;
    let x = (cx - w / 2.0).clamp(rect.left() + 32.0 * s, rect.right() - 32.0 * s - w);
    let panel = Rect::from_min_size(pos2(x, bottom - h), vec2(w, h));
    let radius = CornerRadius::same((20.0 * s) as u8);
    draw::glow(p, panel, 20.0 * s, Color32::BLACK, 30.0 * s, 0.5 * a);
    p.rect_filled(panel, radius, Color32::from_rgba_unmultiplied(22, 25, 34, (248.0 * a) as u8));
    p.rect_stroke(panel, radius, Stroke::new(1.0_f32, white(0.1 * a)), StrokeKind::Inside);
    p.text(pos2(panel.left() + 24.0 * s, panel.top() + head / 2.0), Align2::LEFT_CENTER, "Звук", semibold(24.0 * s), white(a));
    p.text(pos2(panel.right() - 24.0 * s, panel.top() + head / 2.0), Align2::RIGHT_CENTER, "◀ ▶ громкость", font(16.0 * s), white(0.55 * a));
    if let Some(e) = &m.error {
        let g = p.layout(e.clone(), font(17.0 * s), white(0.7 * a), w - 48.0 * s);
        p.galley(pos2(panel.left() + 24.0 * s, panel.top() + head), g, Color32::WHITE);
        return;
    }
    let list = Rect::from_min_max(pos2(panel.left() + 12.0 * s, panel.top() + head), pos2(panel.right() - 12.0 * s, panel.bottom() - 12.0 * s));
    let target = (m.focus as f32 - (MIX_VISIBLE as f32 - 2.0)).clamp(0.0, m.rows.len().saturating_sub(MIX_VISIBLE) as f32);
    m.scroll += (target - m.scroll) * (1.0 - (-dt * 18.0).exp());
    let clip = p.with_clip_rect(list);
    for (i, r) in m.rows.iter().enumerate() {
        let y = list.top() + (i as f32 - m.scroll) * MIX_ROW * s;
        if y + MIX_ROW * s < list.top() || y > list.bottom() {
            continue;
        }
        let row = Rect::from_min_size(pos2(list.left(), y), vec2(list.width(), MIX_ROW * s - 4.0 * s));
        crate::mouse::add(row.intersect(list), crate::mouse::Hit::Mixer(i));
        if i == m.focus {
            clip.rect_filled(row, CornerRadius::same((12.0 * s) as u8), white(0.12 * a));
            clip.rect_stroke(row, CornerRadius::same((12.0 * s) as u8), Stroke::new(1.5_f32, white(0.5 * a)), StrokeKind::Inside);
        }
        let ic_r = Rect::from_center_size(pos2(row.left() + 34.0 * s, row.center().y), vec2(40.0 * s, 40.0 * s));
        let has_icon = r.icon.as_ref().is_some_and(|p| p.exists());
        if r.master {
            clip.circle_filled(ic_r.center(), 20.0 * s, ACCENT.gamma_multiply(a));
            clip.text(ic_r.center(), Align2::CENTER_CENTER, ic::VOLUME, icon(20.0 * s), white(a));
        } else if has_icon && list.contains_rect(ic_r) {
            let uri = crate::data::image_uri(&r.icon.as_ref().unwrap().to_string_lossy()).unwrap_or_default();
            draw::cover(ui, &[uri], ic_r, 6.0 * s, white(a), (0.5, 0.5));
        } else {
            clip.circle_filled(ic_r.center(), 20.0 * s, draw::name_color(&r.name).gamma_multiply(a));
            let l = r.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
            clip.text(ic_r.center(), Align2::CENTER_CENTER, l, bold(18.0 * s), white(a));
        }
        let tx = ic_r.right() + 18.0 * s;
        let value_w = 64.0 * s;
        clip.text(pos2(tx, row.top() + 20.0 * s), Align2::LEFT_CENTER, &r.name, semibold(18.0 * s), white(if r.muted { 0.5 } else { 1.0 } * a));
        let bar = Rect::from_min_max(pos2(tx, row.bottom() - 20.0 * s), pos2(row.right() - value_w - 12.0 * s, row.bottom() - 14.0 * s));
        clip.rect_filled(bar, CornerRadius::same((3.0 * s) as u8), white(0.15 * a));
        let level = if r.muted { 0.0 } else { r.volume as f32 / 100.0 };
        let fill = Rect::from_min_size(bar.min, vec2(bar.width() * level, bar.height()));
        clip.rect_filled(fill, CornerRadius::same((3.0 * s) as u8), if i == m.focus { white(a) } else { ACCENT.gamma_multiply(a) });
        let vp = pos2(row.right() - 22.0 * s, row.center().y);
        if r.muted {
            clip.text(vp, Align2::RIGHT_CENTER, ic::MUTE, icon(22.0 * s), RED.gamma_multiply(a));
        } else {
            clip.text(vp, Align2::RIGHT_CENTER, r.volume.to_string(), semibold(20.0 * s), white(a));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_input() {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 11, 5).unwrap().and_hms_opt(22, 0, 0).unwrap();
        assert_eq!(parse_timer("45", now), Some(45));
        assert_eq!(parse_timer("1:30", now), Some(90));
        assert_eq!(parse_timer("1.30", now), Some(90));
        assert_eq!(parse_timer("23:30", now), Some(90));
        // a time that has passed today means tomorrow
        assert_eq!(parse_timer("21:00", now), Some(23 * 60));
        assert_eq!(parse_timer("01:00", now), Some(180));
        assert_eq!(parse_timer("25:00", now), None);
        assert_eq!(parse_timer("1:75", now), None);
        assert_eq!(parse_timer("abc", now), None);
        assert_eq!(parse_timer("0", now), None);
        assert_eq!(fmt_left(Duration::from_secs(3725)), "1:02:05");
        assert_eq!(fmt_left(Duration::from_secs(65)), "1:05");
    }
}

//! Running games: the launch splash, stepping aside for the game and coming back when it exits,
//! playtime sessions, the Guide-button Control Center and closing a game.

use super::{ease_out, App};
use crate::data::Game;
use crate::draw::{self, bold, font, semibold, white};
use crate::input::PadKind;
use crate::modal::{CtlAction, Modal};
use crate::watch::{self, Event};
use crate::win;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

enum Phase {
    /// waiting for the game to come up; `ready_at` is set once it has
    Starting { ready_at: Option<Instant> },
    Ready,
    Failed(String),
    /// fading out; `step_aside` = the game is up, raise it above us
    Leaving,
}

pub struct Launching {
    game: Game,
    covers: Vec<String>,
    steam: Option<u32>,
    tracked: bool,
    phase: Phase,
    phase_at: Instant,
    at: Instant,
}

const STEAM_TIMEOUT: Duration = Duration::from_secs(120);
const FADE: f32 = 0.56;

fn ms(t: SystemTime) -> f64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

impl App {
    fn own(&self) -> isize {
        self.own.load(Ordering::Relaxed)
    }

    pub(super) fn is_running(&self, g: &Game) -> bool {
        self.running.contains(&g.id)
    }

    // ---------- launching ----------

    pub(super) fn start_launch(&mut self, game: Game) {
        let covers = self.art(&game).covers;
        let steam = game.steam_app_id().and_then(|s| s.parse::<u32>().ok());
        let now = Instant::now();
        // stay above every other window (Steam's "launching…" dialog included) until the game is up
        win::set_topmost(self.own(), true);
        self.covered_at = now;
        let (phase, tracked) = match crate::launch::launch(&game, &self.bus, &self.pids) {
            Err(e) => (Phase::Failed(e), false),
            Ok(started) => {
                let at = SystemTime::now();
                self.last_launch = Some(at);
                if let Some(g) = self.lib.games.iter_mut().find(|x| x.id == game.id) {
                    g.last_played = Some(ms(at));
                }
                self.save();
                if started.tracked {
                    self.running.insert(game.id.clone());
                }
                (Phase::Starting { ready_at: None }, started.tracked)
            }
        };
        self.launching = Some(Launching { game, covers, steam, tracked, phase, phase_at: now, at: now });
    }

    /// B / Esc on the splash: stop waiting, let the game have the screen.
    pub(super) fn hide_launch(&mut self) {
        let Some(l) = self.launching.as_mut() else { return };
        match l.phase {
            Phase::Starting { .. } | Phase::Ready => self.step_aside(),
            Phase::Failed(_) => self.leave(false),
            Phase::Leaving => {}
        }
    }

    fn leave(&mut self, _stepped_aside: bool) {
        if let Some(l) = self.launching.as_mut() {
            l.phase = Phase::Leaving;
            l.phase_at = Instant::now();
        }
        win::set_topmost(self.own(), false);
    }

    /// The game is up: stop covering it and put its window above ours. The desktop never shows.
    fn step_aside(&mut self) {
        self.leave(true);
        self.away_for_game = true;
        if let Some(at) = self.last_launch {
            watch::focus_new_game(self.bus.clone(), self.own(), at);
        }
    }

    /// Advances the launch splash and other timers; called every frame.
    pub(super) fn tick(&mut self, ctx: &egui::Context) {
        if self.reload_steam_at.is_some_and(|t| Instant::now() >= t) {
            self.reload_steam_at = None;
            self.steam_stats = crate::steam::local_stats().1;
        }
        let Some(l) = self.launching.as_mut() else { return };
        let now = Instant::now();
        let age = now - l.phase_at;
        let total = now - l.at;
        let mut next: Option<&str> = None;
        match &mut l.phase {
            Phase::Starting { ready_at } => {
                if ready_at.is_none() {
                    *ready_at = match l.steam {
                        // Steam reports the running app once the game process has started
                        Some(id) if self.steam_now == id => Some(now + Duration::from_millis(2500)),
                        Some(_) if total > STEAM_TIMEOUT => Some(now),
                        Some(_) => None,
                        None if l.tracked => Some(l.at + Duration::from_secs(4)),
                        None => Some(l.at + Duration::from_millis(2500)),
                    };
                }
                if l.tracked && l.steam.is_none() && !self.running.contains(&l.game.id) {
                    next = Some("exited"); // the exe already closed: nothing to step aside for
                } else if ready_at.is_some_and(|t| now >= t) && total > Duration::from_millis(1500) {
                    l.phase = Phase::Ready;
                    l.phase_at = now;
                }
            }
            Phase::Ready if age > Duration::from_millis(900) => next = Some("aside"),
            Phase::Failed(_) if age > Duration::from_millis(2600) => next = Some("leave"),
            Phase::Leaving if age.as_secs_f32() > 2.5 + FADE => next = Some("done"),
            _ => {}
        }
        match next {
            Some("aside") => self.step_aside(),
            Some("leave") | Some("exited") => self.leave(false),
            Some("done") => self.launching = None,
            _ => {}
        }
        // Steam's dialogs and some launchers make themselves topmost too; the last one wins
        if matches!(self.launching.as_ref().map(|l| &l.phase), Some(Phase::Starting { .. } | Phase::Ready))
            && self.covered_at.elapsed() > Duration::from_millis(600)
        {
            win::set_topmost(self.own(), true);
            self.covered_at = Instant::now();
        }
        ctx.request_repaint();
    }

    pub(super) fn paint_launch(&self, ui: &egui::Ui) {
        let Some(l) = &self.launching else { return };
        let rect = ui.max_rect();
        let s = (rect.height() / 1080.0).min(rect.width() / 1920.0 * 1.25);
        let t_in = ease_out((l.at.elapsed().as_secs_f32() / 0.45).min(1.0));
        // a failed launch fades right away; a successful one keeps the splash while the game window appears
        let a = match l.phase {
            Phase::Leaving => {
                let wait = if self.away_for_game { 2.5 } else { 0.0 };
                1.0 - ((l.phase_at.elapsed().as_secs_f32() - wait) / FADE).clamp(0.0, 1.0)
            }
            _ => t_in,
        };
        if a <= 0.0 {
            return;
        }
        let p = ui.painter();
        p.rect_filled(rect, 0.0, Color32::from_rgb(4, 6, 12).gamma_multiply(a));

        // the tile flies from its place in the row to the centre
        let size = (rect.height() * 0.26).min(300.0 * s);
        let cy = rect.height() * 0.4;
        let from = Rect::from_min_size(pos2(96.0 * s, 150.0 * s), vec2(164.0 * s, 164.0 * s));
        let to = Rect::from_center_size(pos2(rect.center().x, cy), vec2(size, size));
        let r = Rect::from_min_max(from.min.lerp(to.min, t_in), from.max.lerp(to.max, t_in));
        let ok = matches!(l.phase, Phase::Ready) || (matches!(l.phase, Phase::Leaving) && self.away_for_game);
        let failed = matches!(l.phase, Phase::Failed(_));
        let glow = if failed { Color32::from_rgb(255, 80, 80) } else { Color32::from_rgb(61, 123, 255) };
        draw::glow(p, r, 22.0 * s, glow, 50.0 * s, 0.9 * a);
        if !draw::cover(ui, &l.covers, r, 22.0 * s, white(a), (0.5, 0.35)) {
            p.rect_filled(r, CornerRadius::same((22.0 * s) as u8), draw::name_color(&l.game.name).gamma_multiply(a));
            p.text(r.center(), Align2::CENTER_CENTER, draw::initials(&l.game.name), bold(size * 0.3), white(a));
        }

        let app = l.game.category() == "app";
        let sub = match (&l.phase, l.steam) {
            (Phase::Failed(_), _) => "Не удалось запустить",
            _ if ok => if app { "Открыто" } else { "Игра запущена" },
            (_, Some(_)) => "Steam запускает игру",
            _ => if app { "Открытие" } else { "Запуск игры" },
        };
        let ty = to.bottom() + 34.0 * s;
        let sub_color = if failed { Color32::from_rgb(255, 120, 120) } else { white(0.65) };
        p.text(pos2(rect.center().x, ty), Align2::CENTER_TOP, sub, font(22.0 * s), sub_color.gamma_multiply(a));
        p.text(pos2(rect.center().x, ty + 34.0 * s), Align2::CENTER_TOP, &l.game.name, semibold(40.0 * s), white(a));
        if let Phase::Failed(e) = &l.phase {
            let galley = p.layout(e.clone(), font(19.0 * s), white(0.7 * a), 900.0 * s);
            p.galley(pos2(rect.center().x - galley.size().x / 2.0, ty + 92.0 * s), galley, Color32::WHITE);
        } else {
            // indeterminate progress bar, full once the game is up
            let bar = Rect::from_center_size(pos2(rect.center().x, ty + 110.0 * s), vec2(320.0 * s, 4.0 * s));
            p.rect_filled(bar, CornerRadius::same(2), white(0.15 * a));
            if ok {
                p.rect_filled(bar, CornerRadius::same(2), glow.gamma_multiply(a));
            } else {
                let t = (l.at.elapsed().as_secs_f32() * 0.8).fract();
                let w = bar.width() * 0.3;
                let x = bar.left() + (bar.width() + w) * t - w;
                let seg = Rect::from_min_max(pos2(x.max(bar.left()), bar.top()), pos2((x + w).min(bar.right()), bar.bottom()));
                if seg.width() > 0.0 {
                    p.rect_filled(seg, CornerRadius::same(2), glow.gamma_multiply(a));
                }
            }
        }
        if matches!(l.phase, Phase::Starting { .. }) && l.at.elapsed().as_secs_f32() > 1.5 {
            let b = if self.input.pad_kind == PadKind::PlayStation { "○" } else { "B" };
            p.text(
                pos2(rect.center().x, rect.bottom() - 70.0 * s),
                Align2::CENTER_CENTER,
                format!("Нажмите {b} или Esc, чтобы скрыть заставку"),
                font(19.0 * s),
                white(0.5 * a),
            );
        }
    }

    // ---------- events from the watchers ----------

    pub(super) fn on_event(&mut self, e: Event) {
        match e {
            Event::SteamRunning { id, prev, at } => {
                self.steam_now = id;
                if prev != 0 {
                    let since = self.steam_since.remove(&prev);
                    let ids: Vec<String> = self.games_with_steam_id(prev);
                    for gid in &ids {
                        self.running.remove(gid);
                        if let Some(start) = since {
                            self.add_session(gid, start, at);
                        }
                    }
                    if !ids.is_empty() {
                        self.save();
                        self.refresh(None);
                    }
                    // Steam writes its own playtime a moment after the game closes
                    self.reload_steam_at = Some(Instant::now() + Duration::from_secs(6));
                    self.trophies_changed(prev);
                }
                if id != 0 {
                    self.steam_since.insert(id, at);
                    for gid in self.games_with_steam_id(id) {
                        self.running.insert(gid);
                    }
                } else if prev != 0 {
                    self.game_ended();
                }
            }
            Event::Exited { id, start, end } => {
                self.running.remove(&id);
                self.add_session(&id, start, end);
                self.save();
                self.refresh(None);
                let still_starting = self.launching.as_ref().is_some_and(|l| l.game.id == id && matches!(l.phase, Phase::Starting { .. }));
                if !still_starting {
                    self.game_ended();
                }
            }
            Event::GameWindow { hwnd } => self.game_hwnd = hwnd,
            Event::GuideFront { prev } => {
                if prev != 0 && prev != self.own() {
                    self.game_hwnd = prev;
                }
                self.refocus = true;
                if self.launching.is_none() {
                    self.open_control();
                }
            }
            Event::GuideToggle => self.guide_pressed(),
            Event::Toast { text, error } => self.toast(&text, error),
            e @ (Event::News { .. } | Event::Trophies { .. } | Event::FeedDone { .. } | Event::Music(_)) => self.on_feed_event(e),
        }
    }

    fn games_with_steam_id(&self, id: u32) -> Vec<String> {
        let id = id.to_string();
        self.lib.games.iter().filter(|g| g.steam_app_id().as_deref() == Some(&id)).map(|g| g.id.clone()).collect()
    }

    /// Adds a play session to the game's playtime and the session log, like the old version did.
    fn add_session(&mut self, id: &str, start: SystemTime, end: SystemTime) {
        let secs = end.duration_since(start).map(|d| d.as_secs()).unwrap_or(0);
        if secs < 5 {
            return;
        }
        let Some(g) = self.lib.games.iter_mut().find(|g| g.id == id) else { return };
        g.playtime = Some(g.playtime.unwrap_or(0.0) + secs as f64);
        let sessions = self.lib.extra.entry("sessions").or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(list) = sessions {
            list.push(json!({ "id": id, "start": ms(start) as i64, "end": ms(end) as i64 }));
            let extra = list.len().saturating_sub(3000);
            list.drain(..extra);
        }
    }

    /// The game closed: if OTIUM stepped aside for it, come back to the front.
    fn game_ended(&mut self) {
        self.game_hwnd = 0;
        if matches!(self.modal, Some(Modal::Control { .. })) {
            self.modal = None;
        }
        if std::mem::take(&mut self.away_for_game) {
            win::activate(self.own());
            self.refocus = true;
        }
    }

    // ---------- Guide button / Control Center ----------

    fn has_game(&self) -> bool {
        !self.running.is_empty() || win::is_window(self.game_hwnd)
    }

    pub(super) fn guide_pressed(&mut self) {
        if matches!(self.modal, Some(Modal::Control { .. })) {
            // second press over a game = back to the game, like the PS button
            if self.has_game() {
                self.back_to_game();
            } else {
                self.modal = None;
            }
            return;
        }
        self.open_control();
    }

    pub(super) fn open_control(&mut self) {
        let mut items = Vec::new();
        if self.has_game() {
            items.push((CtlAction::ReturnToGame, "Вернуться в игру"));
            items.push((CtlAction::CloseGame, "Закрыть игру"));
        }
        items.push((CtlAction::Home, "Главный экран"));
        self.open(Modal::Control { focus: 0, items });
    }

    pub(super) fn control(&mut self, c: CtlAction) {
        match c {
            CtlAction::ReturnToGame => self.back_to_game(),
            CtlAction::CloseGame => self.close_game(None),
            CtlAction::Home => {}
        }
    }

    pub(super) fn back_to_game(&mut self) {
        self.modal = None;
        win::set_topmost(self.own(), false);
        self.away_for_game = self.has_game();
        if win::is_window(self.game_hwnd) {
            win::activate(self.game_hwnd);
        } else if let Some(at) = self.last_launch {
            watch::focus_new_game(self.bus.clone(), self.own(), at);
        }
    }

    /// Closes `game`, or the running game / the window OTIUM was summoned over. Runs in the
    /// background: polite close first, forced after a few seconds.
    pub(super) fn close_game(&mut self, game: Option<Game>) {
        let game = game.or_else(|| self.games.iter().find(|g| self.running.contains(&g.id)).cloned());
        let (bus, pids, hwnd) = (self.bus.clone(), self.pids.clone(), self.game_hwnd);
        self.toast("Закрываю игру…", false);
        self.away_for_game = false;
        std::thread::spawn(move || {
            let result = match &game {
                Some(g) => crate::launch::close(g, &pids),
                None if hwnd != 0 => crate::launch::close_window_owner(hwnd),
                None => Err("Не нашёл запущенную игру".into()),
            };
            let (text, error) = match result {
                Ok(()) => (game.map_or("Игра закрыта".to_string(), |g| format!("«{}» закрыта", g.name)), false),
                Err(e) => (e, true),
            };
            bus.send(Event::Toast { text, error });
        });
        self.game_hwnd = 0;
    }
}

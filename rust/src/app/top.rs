//! The top bar's right side like the PS5's (search, settings, profile, and the update pill when a
//! new version is out), game search, updates from GitHub, the build label, and the mouse.

use super::{App, Zone};
use crate::draw::{self, font, ic, icon, semibold, white};
use crate::input::Nav;
use crate::modal::{Modal, SetItem, SetKey, SetMenu};
use crate::mouse::Hit;
use crate::watch::Event;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TopItem {
    /// Игры / Медиа / Магазин on the left
    Tab(super::Tab),
    Update,
    Search,
    Settings,
    Profile,
}

#[derive(Default)]
pub struct Updates {
    pub avail: Option<crate::update::Release>,
    /// download progress while installing
    pub progress: Option<u8>,
    checked_at: Option<Instant>,
    checking: bool,
    /// the user asked: say "up to date" when nothing is found
    manual: bool,
}

const BLUE: Color32 = Color32::from_rgb(61, 123, 255);

impl App {
    pub(super) fn top_items(&self) -> Vec<TopItem> {
        let mut v: Vec<TopItem> = super::tabs::TABS.iter().map(|(t, _)| TopItem::Tab(*t)).collect();
        if self.updates.avail.is_some() {
            v.push(TopItem::Update);
        }
        v.extend([TopItem::Search, TopItem::Settings, TopItem::Profile]);
        v
    }

    /// Up from the game row: the top bar, on the search icon.
    pub(super) fn enter_top(&mut self) {
        let items = self.top_items();
        let i = items.iter().position(|t| *t == TopItem::Search).unwrap_or(0);
        self.zone = Zone::Top(i);
    }

    pub(super) fn top_nav(&mut self, i: usize, nav: Nav) {
        let items = self.top_items();
        let i = i.min(items.len() - 1);
        match nav {
            Nav::Left if i > 0 => self.zone = Zone::Top(i - 1),
            Nav::Right if i + 1 < items.len() => self.zone = Zone::Top(i + 1),
            Nav::Down | Nav::Back => self.zone = Zone::Row,
            Nav::Ok => self.top_activate(items[i]),
            _ => {}
        }
    }

    fn top_activate(&mut self, t: TopItem) {
        match t {
            TopItem::Tab(t) => self.set_tab(t),
            TopItem::Update => self.start_update(),
            TopItem::Search => self.keyboard("Поиск игры", "", crate::keyboard::Target::Search, 40, false),
            TopItem::Settings => self.open_settings(SetMenu::Main, 0),
            TopItem::Profile => self.open_social(),
        }
    }

    /// Search icon, settings icon, the profile (and the update pill) — right part of the top bar.
    pub(super) fn paint_top_items(&self, ui: &egui::Ui, rect: Rect, s: f32, a: f32, avatar: egui::Pos2) {
        let p = ui.painter();
        let y = avatar.y;
        let items = self.top_items();
        let focus = match self.zone {
            Zone::Top(i) if self.modal.is_none() && !self.social.open && self.pages.page == 0 => Some(items[i.min(items.len() - 1)]),
            _ => None,
        };
        let idx = |t: TopItem| items.iter().position(|x| *x == t).unwrap_or(0);
        // profile: the avatar drawn by the top bar, ringed when focused
        let prof = Rect::from_center_size(avatar, vec2(48.0 * s, 48.0 * s));
        crate::mouse::add(prof, Hit::Top(idx(TopItem::Profile)));
        if focus == Some(TopItem::Profile) {
            p.circle_stroke(avatar, 27.0 * s, Stroke::new(3.0 * s, white(a)));
        }
        let mut x = avatar.x - 64.0 * s;
        for t in [TopItem::Settings, TopItem::Search] {
            let c = pos2(x, y);
            let f = focus == Some(t);
            if f {
                draw::glow(p, Rect::from_center_size(c, vec2(48.0 * s, 48.0 * s)), 24.0 * s, Color32::WHITE, 12.0 * s, 0.4 * a);
                p.circle_filled(c, 25.0 * s, white(a));
            }
            let glyph = if t == TopItem::Search { ic::SEARCH } else { ic::SETTINGS };
            p.text(c, Align2::CENTER_CENTER, glyph, icon(24.0 * s), if f { Color32::from_rgb(10, 12, 18) } else { white(0.9 * a) });
            crate::mouse::add(Rect::from_center_size(c, vec2(50.0 * s, 50.0 * s)), Hit::Top(idx(t)));
            x -= 60.0 * s;
        }
        if self.input.pad_connected {
            p.text(pos2(x + 8.0 * s, y), Align2::RIGHT_CENTER, "🎮", font(24.0 * s), white(0.6 * a));
        }

        // the update pill, in the middle of the bar
        if let Some(r) = &self.updates.avail {
            let text = match self.updates.progress {
                Some(pc) => format!("Скачиваем OTIUM {} · {pc}%", r.version),
                None => format!("Вышла версия {} · Обновить", r.version),
            };
            let g = p.layout_no_wrap(text, semibold(18.0 * s), Color32::PLACEHOLDER);
            let pill = Rect::from_center_size(pos2(rect.center().x, y), vec2(g.size().x + 70.0 * s, 44.0 * s));
            let f = focus == Some(TopItem::Update);
            let radius = CornerRadius::same((22.0 * s) as u8);
            if f {
                draw::glow(p, pill, 22.0 * s, BLUE, 16.0 * s, 0.6 * a);
            }
            p.rect_filled(pill, radius, if f { Color32::WHITE } else { BLUE.gamma_multiply(a) });
            if let Some(pc) = self.updates.progress {
                let done = Rect::from_min_size(pill.min, vec2(pill.width() * pc as f32 / 100.0, pill.height()));
                p.rect_filled(done, radius, white(0.25 * a));
            }
            let fg = if f { Color32::from_rgb(10, 12, 18) } else { white(a) };
            p.text(pos2(pill.left() + 26.0 * s, y), Align2::CENTER_CENTER, ic::DOWNLOAD, icon(18.0 * s), fg);
            p.galley(pos2(pill.left() + 46.0 * s, y - g.size().y / 2.0), g, fg);
            crate::mouse::add(pill, Hit::Top(idx(TopItem::Update)));
        }
    }

    /// "stable 3.0.0 by SoFTeR" in the bottom left corner of the home screen.
    pub(super) fn paint_build_label(&self, ui: &egui::Ui, rect: Rect, s: f32) {
        ui.painter().text(pos2(96.0 * s, rect.bottom() - 54.0 * s), Align2::LEFT_CENTER, format!("{} by SoFTeR", crate::update::label()), font(15.0 * s), white(0.35));
    }

    // ---------- search ----------

    pub(super) fn search_text(&mut self, text: &str) {
        let q = text.trim().to_lowercase();
        self.modal = None;
        if q.is_empty() {
            return;
        }
        let hits: Vec<usize> = self.games.iter().enumerate().filter(|(_, g)| g.name.to_lowercase().contains(&q)).map(|(i, _)| i).collect();
        match hits.as_slice() {
            [] => self.toast(&format!("Ничего не нашлось по «{}»", text.trim()), false),
            [one] => self.show_game(*one),
            _ => {
                self.search_hits = hits.iter().map(|i| self.games[*i].id.clone()).collect();
                let items = hits
                    .iter()
                    .map(|i| {
                        let g = &self.games[*i];
                        SetItem { key: SetKey::SearchHit, label: g.name.clone(), value: if g.is_app() { "приложение".into() } else { String::new() }, danger: false }
                    })
                    .collect();
                self.open(Modal::Settings { menu: SetMenu::Search, title: format!("Найдено: {}", hits.len()), items, focus: 0, scroll: 0.0 });
            }
        }
    }

    pub(super) fn search_pick(&mut self, focus: usize) {
        let id = self.search_hits.get(focus).cloned();
        self.modal = None;
        if let Some(i) = id.and_then(|id| self.games.iter().position(|g| g.id == id)) {
            self.show_game(i);
        }
    }

    fn show_game(&mut self, i: usize) {
        self.social.open = false;
        self.set_tab(super::Tab::Games);
        self.go_page(0);
        self.select(i);
        self.zone = Zone::Row;
    }

    // ---------- updates ----------

    /// Checks GitHub a few seconds after start and then every six hours.
    pub(super) fn tick_updates(&mut self) {
        // OTIUM_FAKE_UPDATE=3.0.1: show the update pill without asking GitHub (for testing the look)
        if let (Some(v), None) = (std::env::var("OTIUM_FAKE_UPDATE").ok(), &self.updates.avail) {
            self.updates.avail = Some(crate::update::Release { version: v, url: String::new(), name: String::new(), size: 0 });
        }
        if std::env::var_os("OTIUM_NO_UPDATE").is_some() || self.updates.checking || self.updates.progress.is_some() {
            return;
        }
        let due = match self.updates.checked_at {
            None => self.started.elapsed() > Duration::from_secs(8),
            Some(t) => t.elapsed() > Duration::from_secs(6 * 3600),
        };
        if due {
            self.check_updates(false);
        }
    }

    pub(super) fn check_updates(&mut self, manual: bool) {
        if self.updates.checking {
            return;
        }
        self.updates.checking = true;
        self.updates.manual = manual;
        self.updates.checked_at = Some(Instant::now());
        let bus = self.bus.clone();
        std::thread::spawn(move || bus.send(Event::UpdateFound(crate::update::check())));
    }

    pub(super) fn on_update_found(&mut self, r: Option<crate::update::Release>) {
        self.updates.checking = false;
        match (&r, self.updates.manual) {
            (None, true) => self.toast(&format!("У вас последняя версия — {}", crate::update::current()), false),
            (Some(r), _) if self.updates.avail.as_ref().is_none_or(|a| a.version != r.version) => {
                self.toast(&format!("Вышла версия OTIUM {} — обновить можно сверху", r.version), false);
            }
            _ => {}
        }
        self.updates.manual = false;
        if r.is_some() || self.updates.progress.is_none() {
            self.updates.avail = r;
        }
    }

    pub(super) fn start_update(&mut self) {
        let Some(r) = self.updates.avail.clone() else { return self.check_updates(true) };
        if self.updates.progress.is_some() {
            return;
        }
        self.modal = None;
        self.updates.progress = Some(0);
        let bus = self.bus.clone();
        std::thread::spawn(move || {
            let b2 = bus.clone();
            let res = crate::update::download(&r, move |pc| b2.send(Event::UpdateProgress(pc)));
            bus.send(Event::UpdateReady(res.map(|p| p.to_string_lossy().to_string())));
        });
    }

    pub(super) fn on_update_ready(&mut self, res: Result<String, String>) {
        match res.and_then(|p| crate::update::apply(std::path::Path::new(&p))) {
            Ok(()) => {
                // the installer waits for us to exit, swaps the exe and starts the new version
                self.toast("Устанавливаем обновление — OTIUM перезапустится", false);
                self.quit();
            }
            Err(e) => {
                self.updates.progress = None;
                self.toast(&e, true);
            }
        }
    }

    /// The Версия row: what it says and does.
    pub(super) fn version_value(&self) -> String {
        let v = crate::update::current();
        match (&self.updates.avail, self.updates.progress, self.updates.checking) {
            (_, Some(pc), _) => format!("{v} · скачиваем {pc}%"),
            (Some(r), None, _) => format!("{v} → обновить до {}", r.version),
            (None, None, true) => format!("{v} · проверяем…"),
            (None, None, false) => format!("{v} · проверить обновления"),
        }
    }

    // ---------- mouse ----------

    /// Turns last frame's pointer into focus moves and presses. Returns navs for the wheel and the
    /// right button; hover and clicks are applied directly.
    pub(super) fn mouse(&mut self, ctx: &egui::Context, frame: &eframe::Frame) -> Vec<Nav> {
        let hits = crate::mouse::take();
        let (pos, moved, click, back, wheel) = ctx.input(|i| {
            (
                i.pointer.hover_pos(),
                i.pointer.delta() != egui::Vec2::ZERO,
                i.pointer.primary_clicked(),
                i.pointer.secondary_clicked() || i.pointer.button_clicked(egui::PointerButton::Extra1),
                i.raw_scroll_delta.y,
            )
        });
        // moving to the TV puts the pointer in the middle of the new screen: that's no hovering,
        // and it would land on "Отмена" of the question being asked
        let moved = moved && !self.tv_asking();
        let mut navs = Vec::new();
        if back {
            navs.push(Nav::Back);
        }
        self.wheel += wheel;
        let step = 50.0;
        while self.wheel.abs() >= step {
            let up = self.wheel > 0.0;
            self.wheel -= step.copysign(self.wheel);
            let home_row = self.modal.is_none() && !self.social.open && self.pages.page == 0 && self.zone == Zone::Row;
            navs.push(match (home_row, up) {
                (true, true) => Nav::Left,
                (true, false) => Nav::Right,
                (false, true) => Nav::Up,
                (false, false) => Nav::Down,
            });
        }
        if wheel == 0.0 && !moved {
            self.wheel *= 0.0;
        }
        let Some(pos) = pos else { return navs };
        if !(moved || click) {
            return navs;
        }
        // the last one painted is on top
        let Some(hit) = hits.iter().rev().find(|(r, h)| r.contains(pos) && self.hit_active(*h)).map(|(_, h)| *h) else { return navs };
        if let Hit::Game(_) | Hit::Media(_) = hit {
            // a tile is chosen by clicking (hovering would change the background at every move);
            // a double click starts it
            if click {
                self.hover(hit);
                if ctx.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary)) {
                    self.handle(Nav::Ok, frame);
                }
            }
            return navs;
        }
        self.hover(hit);
        if click {
            self.handle(Nav::Ok, frame);
        }
        navs
    }

    /// Whether a hit belongs to what is on top right now (a menu covers the home screen).
    fn hit_active(&self, h: Hit) -> bool {
        match &self.modal {
            Some(Modal::Control { mixer: Some(_), .. }) => matches!(h, Hit::Mixer(_)),
            Some(Modal::Control { .. }) => matches!(h, Hit::Cc(_)),
            Some(_) => matches!(h, Hit::Row(_)),
            None if self.social.open => matches!(h, Hit::Social(_)),
            None if self.launching.is_some() || self.reorder => false,
            None => matches!(h, Hit::Game(_) | Hit::Media(_) | Hit::Play | Hit::More | Hit::Top(_)) && self.pages.page == 0,
        }
    }

    fn hover(&mut self, h: Hit) {
        match h {
            Hit::Game(i) => {
                self.select(i);
                self.zone = Zone::Row;
            }
            Hit::Media(i) => self.tab_hover(i),
            Hit::Play => self.zone = Zone::Play,
            Hit::More => self.zone = Zone::More,
            Hit::Top(i) => self.zone = Zone::Top(i),
            Hit::Row(i) => {
                if let Some(m) = self.modal.as_mut() {
                    m.set_focus(i);
                }
            }
            Hit::Cc(i) => {
                if let Some(Modal::Control { focus, .. }) = self.modal.as_mut() {
                    *focus = i;
                }
            }
            Hit::Mixer(i) => {
                if let Some(Modal::Control { mixer: Some(m), .. }) = self.modal.as_mut() {
                    m.set_focus(i);
                }
            }
            Hit::Social(i) => self.social_hover(i),
        }
    }
}

//! The Media and Store tabs of the top bar. Media: streaming services, each opened as its own
//! fullscreen browser window. Store: a test catalog of ports and free games (nothing installs yet).

use super::{ease_out, App, Zone};
use crate::draw::{self, bold, font, semibold, white};
use crate::input::Nav;
use crate::services::{self, SERVICES, STORE};
use crate::watch::Event;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tab {
    #[default]
    Games,
    Media,
    Store,
}

pub const TABS: [(Tab, &str); 3] = [(Tab::Games, "Игры"), (Tab::Media, "Медиа"), (Tab::Store, "Магазин")];

#[derive(Default)]
pub struct Tabs {
    pub tab: Tab,
    media_sel: usize,
    store_sel: usize,
    scroll: f32,
    /// animated selection (tile sizes follow it)
    focus: f32,
    /// when the tab or its selection changed, for the background fade
    changed: Option<Instant>,
    prev_color: Option<Color32>,
    /// service id -> its browser window, while open
    windows: Vec<(&'static str, isize)>,
    /// service being opened (its window isn't up yet)
    opening: Option<(&'static str, Instant)>,
}

/// One tile of a tab: what the painter needs.
struct Entry {
    name: &'static str,
    kicker: &'static str,
    about: &'static str,
    note: Option<String>,
    color: Color32,
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

impl App {
    pub(super) fn tab(&self) -> Tab {
        self.tabs.tab
    }

    pub(super) fn set_tab(&mut self, t: Tab) {
        if self.tabs.tab != t {
            self.tabs.prev_color = self.tab_color();
            self.tabs.changed = Some(Instant::now());
            self.tabs.tab = t;
            self.tabs.scroll = 0.0;
            self.tabs.focus = self.tab_sel() as f32;
        }
        self.zone = Zone::Row;
    }

    fn tab_sel(&self) -> usize {
        match self.tabs.tab {
            Tab::Media => self.tabs.media_sel,
            Tab::Store => self.tabs.store_sel,
            Tab::Games => self.sel,
        }
    }

    fn set_tab_sel(&mut self, i: usize) {
        let n = self.tab_entries().len();
        if i >= n || i == self.tab_sel() {
            return;
        }
        self.tabs.prev_color = self.tab_color();
        self.tabs.changed = Some(Instant::now());
        match self.tabs.tab {
            Tab::Media => self.tabs.media_sel = i,
            Tab::Store => self.tabs.store_sel = i,
            Tab::Games => {}
        }
    }

    fn tab_entries(&self) -> Vec<Entry> {
        match self.tabs.tab {
            Tab::Games => Vec::new(),
            Tab::Media => SERVICES
                .iter()
                .map(|s| Entry {
                    name: s.name,
                    kicker: "Приложение",
                    about: s.about,
                    note: self.service_open(s.id).then(|| "● Открыто".to_string()),
                    color: rgb(s.color),
                })
                .collect(),
            Tab::Store => STORE
                .iter()
                .map(|i| Entry {
                    name: i.name,
                    kicker: i.kind,
                    about: i.about,
                    note: Some(match i.needs {
                        Some(n) => format!("Нужно: {n}"),
                        None => "Ничего не нужно — бесплатно и с открытым кодом".to_string(),
                    }),
                    color: rgb(i.color),
                })
                .collect(),
        }
    }

    fn tab_color(&self) -> Option<Color32> {
        self.tab_entries().get(self.tab_sel()).map(|e| e.color)
    }

    fn service_open(&self, id: &str) -> bool {
        self.tabs.windows.iter().any(|(s, h)| *s == id && crate::win::is_window(*h))
    }

    // ---------- input ----------

    /// The Media / Store tab has the screen (not the top bar).
    pub(super) fn tab_nav(&mut self, nav: Nav) {
        let n = self.tab_entries().len();
        let sel = self.tab_sel();
        match (self.zone, nav) {
            (Zone::Row, Nav::Up) => self.enter_tab_name(),
            (Zone::Row, Nav::Left) if sel > 0 => self.set_tab_sel(sel - 1),
            (Zone::Row, Nav::Right) if sel + 1 < n => self.set_tab_sel(sel + 1),
            (Zone::Row, Nav::Down) => self.zone = Zone::Play,
            (Zone::Row | Zone::Play, Nav::Ok) => self.tab_activate(),
            (Zone::Play, Nav::Up | Nav::Back) => self.zone = Zone::Row,
            // B on the row goes back to the games
            (Zone::Row, Nav::Back) => self.set_tab(Tab::Games),
            _ => {}
        }
    }

    /// Up from a tab's row: onto its name in the top bar.
    fn enter_tab_name(&mut self) {
        let items = self.top_items();
        let i = items.iter().position(|t| *t == super::top::TopItem::Tab(self.tabs.tab)).unwrap_or(0);
        self.zone = Zone::Top(i);
    }

    pub(super) fn tab_hover(&mut self, i: usize) {
        self.set_tab_sel(i);
        self.zone = Zone::Row;
    }

    fn tab_activate(&mut self) {
        match self.tabs.tab {
            Tab::Media => self.open_service(self.tabs.media_sel),
            Tab::Store => {
                let name = STORE.get(self.tabs.store_sel).map_or("", |i| i.name);
                self.toast(&format!("Магазин пока тестовый: установка «{name}» появится в следующих версиях"), false);
            }
            Tab::Games => {}
        }
    }

    fn open_service(&mut self, i: usize) {
        let Some(s) = SERVICES.get(i) else { return };
        // already open: bring it back
        if let Some(&(_, h)) = self.tabs.windows.iter().find(|(id, h)| *id == s.id && crate::win::is_window(*h)) {
            self.game_hwnd = h;
            self.away_for_game = true;
            crate::win::activate(h);
            return;
        }
        if self.tabs.opening.is_some_and(|(id, t)| id == s.id && t.elapsed() < Duration::from_secs(10)) {
            return;
        }
        if std::env::var_os("OTIUM_NO_OPEN").is_some() {
            eprintln!("open service (suppressed): {} {}", s.id, s.url);
            return self.toast(&format!("Открываем {}", s.name), false);
        }
        let mut child = match services::launch(s) {
            Ok(c) => c,
            Err(e) => return self.toast(&e, true),
        };
        self.toast(&format!("Открываем {}…", s.name), false);
        self.tabs.opening = Some((s.id, Instant::now()));
        let (bus, id, pid) = (self.bus.clone(), s.id, child.id());
        std::thread::spawn(move || {
            let at = Instant::now();
            // the browser shows its window within a few seconds; it exits when the window closes
            let mut shown = false;
            loop {
                if let Ok(Some(_)) = child.try_wait() {
                    break;
                }
                if !shown && at.elapsed() < Duration::from_secs(30) {
                    if let Some(h) = crate::win::main_window(pid) {
                        shown = true;
                        bus.send(Event::ServiceWindow { id, hwnd: h });
                        std::thread::spawn(move || services::make_fullscreen(h));
                    }
                }
                std::thread::sleep(Duration::from_millis(if shown { 1000 } else { 250 }));
            }
            bus.send(Event::ServiceClosed { id, secs: at.elapsed().as_secs() });
        });
    }

    pub(super) fn on_service_event(&mut self, e: Event) {
        match e {
            Event::ServiceWindow { id, hwnd } => {
                self.tabs.opening = None;
                self.tabs.windows.retain(|(s, _)| *s != id);
                self.tabs.windows.push((id, hwnd));
                // like a game: Guide comes back to OTIUM, "Вернуться" goes back to the window
                self.game_hwnd = hwnd;
                self.away_for_game = true;
                crate::win::activate(hwnd);
            }
            Event::ServiceClosed { id, secs } => {
                if self.tabs.opening.is_some_and(|(s, _)| s == id) {
                    self.tabs.opening = None;
                }
                let had = self.tabs.windows.iter().find(|(s, _)| *s == id).map(|(_, h)| *h);
                self.tabs.windows.retain(|(s, _)| *s != id);
                if let Some(h) = had {
                    if self.game_hwnd == h {
                        self.game_hwnd = 0;
                        self.away_for_game = false;
                    }
                    // the window was closed: OTIUM comes back to the front
                    let own = self.own.load(std::sync::atomic::Ordering::Relaxed);
                    crate::win::activate(own);
                    self.refocus = true;
                } else if secs < 5 {
                    // the browser handed the request to a window of its profile that was already
                    // open (from before OTIUM started) and quit: that window is in front now
                    eprintln!("service {id}: handed over to an open window");
                }
            }
            _ => {}
        }
    }

    pub(super) fn tab_hints(&self) -> Vec<(usize, &'static str)> {
        match self.tabs.tab {
            Tab::Store => vec![(0, "Установить"), (1, "Назад")],
            _ => vec![(0, "Открыть"), (1, "Назад")],
        }
    }

    // ---------- painting ----------

    pub(super) fn paint_tab_background(&self, ui: &egui::Ui, rect: Rect) {
        let p = ui.painter();
        p.rect_filled(rect, 0.0, Color32::from_rgb(8, 10, 16));
        let t = self.tabs.changed.map_or(1.0, |c| (c.elapsed().as_secs_f32() / 0.45).min(1.0));
        let cur = self.tab_color().unwrap_or(Color32::from_rgb(61, 123, 255));
        let glow = |c: Color32, a: f32| {
            let r = Rect::from_min_max(rect.min, pos2(rect.left() + rect.width() * 0.8, rect.bottom()));
            draw::hgradient(p, r, c.gamma_multiply(0.42 * a), Color32::TRANSPARENT);
            let top = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + rect.height() * 0.7));
            draw::vgradient(p, top, c.gamma_multiply(0.22 * a), Color32::TRANSPARENT);
        };
        if t < 1.0 {
            if let Some(prev) = self.tabs.prev_color {
                glow(prev, 1.0 - t);
            }
        }
        glow(cur, ease_out(t));
        let h = rect.height();
        draw::vgradient(p, Rect::from_min_max(pos2(rect.left(), rect.top() + h * 0.5), rect.max), Color32::TRANSPARENT, Color32::from_black_alpha(220));
    }

    pub(super) fn paint_tab(&mut self, ui: &egui::Ui, rect: Rect, s: f32, dt: f32, appear: f32) {
        let entries = self.tab_entries();
        let sel = self.tab_sel().min(entries.len().saturating_sub(1));
        let tile = 116.0 * s;
        let big = 164.0 * s;
        let gap = 20.0 * s;
        let left = 96.0 * s;
        let row_y = 150.0 * s;
        let k = 1.0 - (-dt * 16.0).exp();
        self.tabs.focus += (sel as f32 - self.tabs.focus) * k;
        // a row that fits stays put; a longer one keeps the selected tile at the left edge
        let fits = left + entries.len() as f32 * (tile + gap) + (big - tile) < rect.right() - left;
        let target = if fits { 0.0 } else { sel as f32 };
        self.tabs.scroll += (target - self.tabs.scroll) * k;
        let in_row = self.zone == Zone::Row && self.modal.is_none() && !self.social.open;
        let p = ui.painter();

        for (i, e) in entries.iter().enumerate() {
            let d = i as f32 - self.tabs.scroll;
            let df = i as f32 - self.tabs.focus;
            let shift = df.clamp(0.0, 1.0) * (big - tile);
            let x = left + d * (tile + gap) + shift;
            if x > rect.right() || x + big < rect.left() - big {
                continue;
            }
            let focus = 1.0 - df.abs().min(1.0);
            let size = tile + (big - tile) * focus;
            let r = Rect::from_min_size(pos2(x, row_y), vec2(size, size));
            let fade = if d < -0.5 { (1.0 + d).max(0.0) } else { 1.0 } * appear;
            paint_entry_tile(p, e, r, 14.0 * s, fade, i == sel && in_row, s);
            crate::mouse::add(r, crate::mouse::Hit::Media(i));
        }
        let Some(e) = entries.get(sel) else { return };

        // ---- title, button, about ----
        let info_y = rect.height() * 0.50;
        p.text(pos2(left + 4.0 * s, info_y - 100.0 * s), Align2::LEFT_BOTTOM, e.kicker.to_uppercase(), semibold(18.0 * s), white(0.6 * appear));
        p.text(pos2(left, info_y - 10.0 * s), Align2::LEFT_BOTTOM, e.name, bold(72.0 * s), white(appear));

        let btn = Rect::from_min_size(pos2(left, info_y + 20.0 * s), vec2(280.0 * s, 68.0 * s));
        let focus = self.zone == Zone::Play && self.modal.is_none() && !self.social.open;
        if focus {
            draw::glow(p, btn, 34.0 * s, Color32::WHITE, 18.0 * s, 0.6);
        }
        p.rect_filled(btn, CornerRadius::same((34.0 * s) as u8), if focus { Color32::WHITE } else { white(0.88) });
        let label = match self.tabs.tab {
            Tab::Store => "Установить",
            _ if self.tabs.opening.is_some_and(|(id, t)| SERVICES.get(sel).is_some_and(|s| s.id == id) && t.elapsed() < Duration::from_secs(10)) => "Открываем…",
            _ => "Открыть",
        };
        p.text(btn.center(), Align2::CENTER_CENTER, label, semibold(26.0 * s), Color32::from_rgb(10, 12, 18));
        crate::mouse::add(btn, crate::mouse::Hit::Play);

        let mut y = btn.bottom() + 26.0 * s;
        if let Some(n) = &e.note {
            p.text(pos2(left + 4.0 * s, y), Align2::LEFT_TOP, n, font(20.0 * s), white(0.7 * appear));
            y += 34.0 * s;
        }
        let about = p.layout(e.about.to_string(), font(21.0 * s), white(0.85 * appear), 760.0 * s);
        p.galley(pos2(left + 4.0 * s, y), about, Color32::WHITE);
        if self.tabs.tab == Tab::Store {
            let y = y + 70.0 * s;
            p.text(
                pos2(left + 4.0 * s, y),
                Align2::LEFT_TOP,
                "Тестовая витрина: каталог, скачивание с GitHub и проверка своих файлов игры появятся позже",
                font(17.0 * s),
                white(0.45 * appear),
            );
        }
    }

    /// The tab names on the left of the top bar; returns their rects for focus and the mouse.
    pub(super) fn paint_tab_names(&self, ui: &egui::Ui, s: f32, a: f32, focus: Option<Tab>) -> Vec<Rect> {
        let p = ui.painter();
        let y = 62.0 * s;
        let mut x = 96.0 * s;
        let mut out = Vec::new();
        for (t, name) in TABS {
            let cur = self.tabs.tab == t;
            let f = focus == Some(t);
            let g = p.layout_no_wrap(name.to_string(), semibold(30.0 * s), Color32::PLACEHOLDER);
            let r = Rect::from_min_size(pos2(x, y - g.size().y / 2.0), g.size());
            if f {
                let pill = r.expand2(vec2(18.0 * s, 8.0 * s));
                p.rect_filled(pill, CornerRadius::same((pill.height() / 2.0) as u8), white(a));
            }
            let color = if f { Color32::from_rgb(10, 12, 18) } else if cur { white(a) } else { white(0.5 * a) };
            p.galley(r.min, g, color);
            if cur && !f {
                p.rect_filled(Rect::from_min_size(pos2(r.left(), r.bottom() + 6.0 * s), vec2(r.width(), 3.0 * s)), CornerRadius::same(2), white(a));
            }
            if t == Tab::Store {
                let tg = p.layout_no_wrap("тест".into(), semibold(13.0 * s), Color32::PLACEHOLDER);
                let b = Rect::from_min_size(pos2(r.right() + 8.0 * s, y - 20.0 * s), tg.size() + vec2(12.0 * s, 4.0 * s));
                p.rect_filled(b, CornerRadius::same((b.height() / 2.0) as u8), Color32::from_rgb(61, 123, 255).gamma_multiply(a));
                p.galley(b.min + vec2(6.0 * s, 2.0 * s), tg, white(a));
                x = b.right() + 30.0 * s;
            } else {
                x = r.right() + 36.0 * s;
            }
            out.push(r.expand2(vec2(18.0 * s, 10.0 * s)));
        }
        out
    }
}

fn paint_entry_tile(p: &egui::Painter, e: &Entry, r: Rect, radius: f32, alpha: f32, focused: bool, s: f32) {
    if alpha <= 0.01 {
        return;
    }
    if focused {
        draw::glow(p, r, radius, Color32::WHITE, 16.0 * s, 0.55 * alpha);
    }
    let cr = CornerRadius::same(radius as u8);
    p.rect_filled(r, cr, e.color.gamma_multiply(alpha));
    // the name, centred line by line; the font shrinks until the longest word fits
    let room = r.width() - 20.0 * s;
    let mut size = r.height() * 0.15;
    let widest = e.name.split(' ').map(|w| p.layout_no_wrap(w.to_string(), bold(size), Color32::WHITE).size().x).fold(0.0, f32::max);
    if widest > room {
        size *= room / widest;
    }
    let mut lines: Vec<String> = Vec::new();
    for w in e.name.split(' ') {
        match lines.last_mut() {
            Some(l) if p.layout_no_wrap(format!("{l} {w}"), bold(size), Color32::WHITE).size().x <= room => {
                l.push(' ');
                l.push_str(w);
            }
            _ => lines.push(w.to_string()),
        }
    }
    let lh = size * 1.2;
    let top = r.center().y - lh * (lines.len() as f32 - 1.0) / 2.0;
    for (i, l) in lines.iter().enumerate() {
        p.text(pos2(r.center().x, top + lh * i as f32), Align2::CENTER_CENTER, l, bold(size), white(alpha));
    }
    if focused {
        p.rect_stroke(r, cr, Stroke::new(3.0 * s, white(alpha)), StrokeKind::Outside);
    }
}
